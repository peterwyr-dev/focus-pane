use crate::models::{TimerPhase, TimerState};
use std::{
    env,
    path::PathBuf,
    process::{Command, Stdio},
};

#[derive(Debug, Clone)]
pub struct HerdrBridge {
    binary: PathBuf,
    available: bool,
}

impl HerdrBridge {
    pub fn from_environment() -> Self {
        Self {
            binary: env::var_os("HERDR_BIN_PATH")
                .map(PathBuf::from)
                .unwrap_or_else(|| PathBuf::from("herdr")),
            available: env::var("HERDR_ENV").as_deref() == Ok("1"),
        }
    }

    pub fn notify_focus_complete(&self, _task_text: Option<&str>, break_started: bool) {
        let body = if break_started {
            "Focus complete. Drink some water, look away from the screen, and take a break."
        } else {
            "Focus complete. Your break is ready; open Focus Pane and press p to begin."
        };
        self.notify("Focus complete", body, "done");
    }

    pub fn notify_break_complete(&self, task_text: Option<&str>, focus_started: bool) {
        let body = if focus_started {
            task_text
                .map(|task| format!("Break complete. Return to \"{task}\" with one small step."))
                .unwrap_or_else(|| {
                    "Break complete. Choose a task and start the next focus session.".into()
                })
        } else {
            task_text
                .map(|task| {
                    format!(
                        "Break complete. \"{task}\" is ready; open Focus Pane and press p to begin."
                    )
                })
                .unwrap_or_else(|| {
                    "Break complete. Your next focus is ready; open Focus Pane and press p to begin."
                        .into()
                })
        };
        self.notify("Break complete", &body, "request");
    }

    pub fn update_sidebar(&self, state: &TimerState, now: f64) {
        let Some(pane_id) = state.origin_pane_id.as_deref() else {
            return;
        };
        let remaining = state.seconds_left(now);
        let preset_name = if state.preset_name.is_empty() {
            "legacy"
        } else {
            state.preset_name.as_str()
        };
        let text = match state.phase {
            TimerPhase::Focus | TimerPhase::PausedFocus => format!(
                "{} focus {:02}:{:02} [{}] · {}",
                if state.phase == TimerPhase::PausedFocus {
                    "Ⅱ"
                } else {
                    "●"
                },
                remaining / 60,
                remaining % 60,
                preset_name,
                state.active_task_text.as_deref().unwrap_or("")
            ),
            TimerPhase::Break | TimerPhase::PausedBreak => format!(
                "{} break {:02}:{:02}",
                if state.phase == TimerPhase::PausedBreak {
                    "Ⅱ"
                } else {
                    "◇"
                },
                remaining / 60,
                remaining % 60
            ),
            TimerPhase::ReadyForBreak => {
                format!("◇ break ready {:02}:{:02}", remaining / 60, remaining % 60)
            }
            TimerPhase::ReadyForFocus => {
                format!("✓ focus ready {:02}:{:02}", remaining / 60, remaining % 60)
            }
            TimerPhase::Idle => {
                self.clear_sidebar(Some(pane_id));
                return;
            }
        };
        self.run(&[
            "pane",
            "report-metadata",
            pane_id,
            "--source",
            "focus-pane",
            "--token",
            &format!("focus={text}"),
            "--ttl-ms",
            "90000",
        ]);
    }

    pub fn clear_sidebar(&self, pane_id: Option<&str>) {
        let Some(pane_id) = pane_id else { return };
        self.run(&[
            "pane",
            "report-metadata",
            pane_id,
            "--source",
            "focus-pane",
            "--clear-token",
            "focus",
        ]);
    }

    fn notify(&self, title: &str, body: &str, sound: &str) {
        self.run(&[
            "notification",
            "show",
            title,
            "--body",
            body,
            "--position",
            "top-right",
            "--sound",
            sound,
        ]);
    }

    fn run(&self, arguments: &[&str]) -> bool {
        if !self.available {
            return false;
        }
        Command::new(&self.binary)
            .args(arguments)
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status()
            .is_ok_and(|status| status.success())
    }
}
