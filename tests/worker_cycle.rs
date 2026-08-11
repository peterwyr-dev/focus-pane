use focus_pane::{
    config::{AppConfig, FocusPreset},
    markdown::MarkdownTaskRepository,
    models::TimerPhase,
    store::FocusStore,
    worker::run_worker,
};
use std::{
    env, fs,
    path::Path,
    thread,
    time::{Duration, Instant},
};
use tempfile::tempdir;

#[test]
fn worker_runs_focus_break_focus_with_both_sounds_without_completing_markdown() {
    let directory = tempdir().unwrap();
    let todo = directory.path().join("TODO.md");
    fs::write(&todo, "# Study\n\n- [ ] Learn CS50\n").unwrap();
    let repository = MarkdownTaskRepository::new(todo.clone());
    let task = repository
        .ensure_id(&repository.read_tasks().unwrap()[0])
        .unwrap();
    let db_path = directory.path().join("focus.db");
    let store = FocusStore::open(&db_path).unwrap();
    let preset = FocusPreset {
        name: "test".into(),
        focus_minutes: 1.0 / 60.0,
        break_minutes: 1.0 / 60.0,
        long_break_minutes: 1.0 / 60.0,
        cycles_before_long_break: 4,
    };
    let initial = store
        .start_focus_with_preset_at(&task, &preset, None, now())
        .unwrap();
    // Deliberately different legacy values verify that the detached worker
    // follows the persisted preset snapshot rather than reinterpreting config.
    let config = AppConfig {
        focus_minutes: 10.0,
        break_minutes: 10.0,
        long_break_minutes: 10.0,
        auto_start_break: true,
        auto_start_focus: true,
        ..AppConfig::default()
    };

    let log = directory.path().join("herdr.log");
    let fake_herdr = directory.path().join("herdr");
    fs::write(
        &fake_herdr,
        "#!/bin/sh\nprintf '%s\\n' \"$*\" >> \"$FOCUS_HERDR_LOG\"\n",
    )
    .unwrap();
    make_executable(&fake_herdr);
    unsafe {
        env::set_var("HERDR_ENV", "1");
        env::set_var("HERDR_BIN_PATH", &fake_herdr);
        env::set_var("FOCUS_HERDR_LOG", &log);
    }

    let worker_store = store.clone();
    let worker_config = config.clone();
    let initial_id = initial.timer_id.clone();
    let worker = thread::spawn(move || {
        run_worker(worker_store.path(), &worker_config, &initial_id).unwrap()
    });

    let deadline = Instant::now() + Duration::from_secs(5);
    loop {
        let state = store.get_timer().unwrap();
        let total = store
            .totals_for_tasks(&[task.task_id.clone().unwrap()])
            .unwrap()
            .get(task.task_id.as_deref().unwrap())
            .copied();
        if state.phase == TimerPhase::Focus
            && state.timer_id != initial.timer_id
            && total == Some(1)
        {
            break;
        }
        assert!(
            Instant::now() < deadline,
            "worker did not start next focus: {state:?}"
        );
        thread::sleep(Duration::from_millis(50));
    }

    assert!(
        fs::read_to_string(todo)
            .unwrap()
            .contains("- [ ] Learn CS50")
    );
    let notifications = fs::read_to_string(log).unwrap();
    assert!(notifications.contains("notification show Focus complete"));
    assert!(notifications.contains("--sound done"));
    assert!(notifications.contains("notification show Break complete"));
    assert!(notifications.contains("--sound request"));

    store.stop_at(now()).unwrap();
    worker.join().unwrap();
}

fn now() -> f64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_secs_f64()
}

#[cfg(unix)]
fn make_executable(path: &Path) {
    use std::os::unix::fs::PermissionsExt;
    let mut permissions = fs::metadata(path).unwrap().permissions();
    permissions.set_mode(0o755);
    fs::set_permissions(path, permissions).unwrap();
}
