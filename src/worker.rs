use crate::{config::AppConfig, herdr::HerdrBridge, models::TimerPhase, store::FocusStore};
use anyhow::Result;
use fs2::FileExt;
use std::{
    fs::{self, File},
    path::{Path, PathBuf},
    process::{Command, Stdio},
    thread,
    time::{Duration, SystemTime, UNIX_EPOCH},
};

#[cfg(unix)]
use std::os::unix::process::CommandExt;

pub fn spawn_worker(db_path: &Path, config_path: &Path, timer_id: &str) -> Result<()> {
    let executable = std::env::current_exe()?;
    let mut command = Command::new(executable);
    command
        .arg("worker")
        .arg("--db")
        .arg(db_path)
        .arg("--config")
        .arg(config_path)
        .arg("--timer-id")
        .arg(timer_id)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null());
    #[cfg(unix)]
    unsafe {
        command.pre_exec(|| {
            if libc::setsid() == -1 {
                return Err(std::io::Error::last_os_error());
            }
            Ok(())
        });
    }
    command.spawn()?;
    Ok(())
}

pub fn run_worker(db_path: &Path, config: &AppConfig, timer_id: &str) -> Result<()> {
    let lock_path = worker_lock_path(db_path, timer_id);
    let lock_file = File::create(&lock_path)?;
    if lock_file.try_lock_exclusive().is_err() {
        return Ok(());
    }
    let result = run_worker_loop(db_path, config, timer_id);
    let _ = FileExt::unlock(&lock_file);
    let _ = fs::remove_file(lock_path);
    result
}

fn run_worker_loop(db_path: &Path, config: &AppConfig, timer_id: &str) -> Result<()> {
    let store = FocusStore::open(db_path)?;
    let bridge = HerdrBridge::from_environment();
    let mut expected_timer_id = timer_id.to_owned();
    let mut last_sidebar_update = 0.0;

    loop {
        let now = unix_now();
        let state = store.get_timer()?;
        if state.timer_id != expected_timer_id
            || !matches!(state.phase, TimerPhase::Focus | TimerPhase::Break)
        {
            return Ok(());
        }

        if now - last_sidebar_update >= 15.0 {
            bridge.update_sidebar(&state, now);
            last_sidebar_update = now;
        }

        if state.deadline_at.is_none_or(|deadline| deadline > now) {
            let remaining = state
                .deadline_at
                .map(|deadline| (deadline - now).max(0.1))
                .unwrap_or(1.0);
            thread::sleep(Duration::from_secs_f64(remaining.min(1.0)));
            continue;
        }

        let Some(transition) = store.complete_due_at(&expected_timer_id, config, now)? else {
            return Ok(());
        };
        if transition.kind == "focus_completed" {
            bridge.notify_focus_complete(
                state.active_task_text.as_deref(),
                transition.state.phase == TimerPhase::Break,
            );
            bridge.update_sidebar(&transition.state, now);
            expected_timer_id = transition.state.timer_id;
            last_sidebar_update = now;
            continue;
        }

        bridge.notify_break_complete(state.next_task_text.as_deref(), config.auto_start_focus);
        if config.auto_start_focus {
            let started = store.start_ready_focus_at(config.focus_seconds(), now)?;
            bridge.update_sidebar(&started, now);
            expected_timer_id = started.timer_id;
            last_sidebar_update = now;
            continue;
        }
        bridge.update_sidebar(&transition.state, now);
        return Ok(());
    }
}

fn worker_lock_path(db_path: &Path, timer_id: &str) -> PathBuf {
    let file_name = db_path.file_name().unwrap_or_default().to_string_lossy();
    db_path.with_file_name(format!(".{file_name}.{timer_id}.worker.lock"))
}

pub fn unix_now() -> f64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs_f64()
}
