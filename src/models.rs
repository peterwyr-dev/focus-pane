use chrono::NaiveDate;
use std::path::PathBuf;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TaskPriority {
    High,
    Medium,
    Low,
}

impl TaskPriority {
    pub fn parse(value: &str) -> Option<Self> {
        match value.to_ascii_lowercase().as_str() {
            "high" => Some(Self::High),
            "medium" => Some(Self::Medium),
            "low" => Some(Self::Low),
            _ => None,
        }
    }

    pub fn as_str(self) -> &'static str {
        match self {
            Self::High => "high",
            Self::Medium => "medium",
            Self::Low => "low",
        }
    }
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct TaskMetadata {
    pub tags: Vec<String>,
    pub priority: Option<TaskPriority>,
    pub due_date: Option<NaiveDate>,
    pub estimate_pomodoros: Option<u32>,
    pub blocked: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Task {
    pub source_path: PathBuf,
    pub line_number: usize,
    pub text: String,
    pub checked: bool,
    pub heading: Option<String>,
    pub task_id: Option<String>,
    pub indent: usize,
    pub metadata: TaskMetadata,
}

impl Task {
    pub fn key(&self) -> String {
        match &self.task_id {
            Some(task_id) => format!("{}\u{1f}{task_id}", self.source_path.display()),
            None => format!(
                "{}:{}:{}",
                self.source_path.display(),
                self.line_number,
                self.text
            ),
        }
    }

    pub fn history_key(&self) -> Option<(String, String)> {
        self.task_id.as_ref().map(|task_id| {
            (
                task_id.clone(),
                self.source_path.to_string_lossy().into_owned(),
            )
        })
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TimerPhase {
    Idle,
    Focus,
    PausedFocus,
    Break,
    PausedBreak,
    ReadyForBreak,
    ReadyForFocus,
}

impl TimerPhase {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Idle => "idle",
            Self::Focus => "focus",
            Self::PausedFocus => "paused_focus",
            Self::Break => "break",
            Self::PausedBreak => "paused_break",
            Self::ReadyForBreak => "ready_for_break",
            Self::ReadyForFocus => "ready_for_focus",
        }
    }

    pub fn parse(value: &str) -> Option<Self> {
        match value {
            "idle" => Some(Self::Idle),
            "focus" => Some(Self::Focus),
            "paused_focus" => Some(Self::PausedFocus),
            "break" => Some(Self::Break),
            "paused_break" => Some(Self::PausedBreak),
            "ready_for_break" => Some(Self::ReadyForBreak),
            "ready_for_focus" | "ready" => Some(Self::ReadyForFocus),
            _ => None,
        }
    }

    pub fn is_running(self) -> bool {
        matches!(self, Self::Focus | Self::Break)
    }

    pub fn is_paused(self) -> bool {
        matches!(self, Self::PausedFocus | Self::PausedBreak)
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct TimerState {
    pub phase: TimerPhase,
    pub timer_id: String,
    /// Task that owns the currently running or paused focus session.
    pub active_task_id: Option<String>,
    pub active_task_text: Option<String>,
    pub active_source_path: Option<String>,
    pub active_source_line: Option<usize>,
    /// Task context to promote when the next focus session starts.
    pub next_task_id: Option<String>,
    pub next_task_text: Option<String>,
    pub next_source_path: Option<String>,
    pub next_source_line: Option<usize>,
    pub origin_pane_id: Option<String>,
    pub preset_name: String,
    pub preset_focus_seconds: u64,
    pub preset_break_seconds: u64,
    pub preset_long_break_seconds: u64,
    pub preset_cycles_before_long_break: u32,
    pub cycle: u32,
    pub session_started_at: Option<f64>,
    pub phase_started_at: Option<f64>,
    pub deadline_at: Option<f64>,
    pub remaining_seconds: f64,
    pub accumulated_seconds: f64,
    pub updated_at: f64,
}

impl TimerState {
    pub fn seconds_left(&self, now: f64) -> u64 {
        if self.phase.is_running() {
            self.deadline_at
                .map(|deadline| (deadline - now).max(0.0).round() as u64)
                .unwrap_or(0)
        } else if self.phase.is_paused()
            || matches!(
                self.phase,
                TimerPhase::ReadyForBreak | TimerPhase::ReadyForFocus
            )
        {
            self.remaining_seconds.max(0.0).round() as u64
        } else {
            0
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct TimerTransition {
    pub kind: &'static str,
    pub state: TimerState,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct TaskFocusSummary {
    pub total_seconds: u64,
    pub completed_seconds: u64,
    pub stopped_seconds: u64,
    pub completed_sessions: u32,
    pub stopped_sessions: u32,
}

#[derive(Debug, Clone, PartialEq)]
pub struct FocusSession {
    pub task_id: String,
    pub task_text: String,
    pub source_path: String,
    pub source_line: Option<usize>,
    pub started_at: f64,
    pub ended_at: f64,
    pub duration_seconds: u64,
    pub outcome: String,
}
