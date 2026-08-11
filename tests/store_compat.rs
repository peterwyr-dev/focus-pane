use focus_pane::{
    config::{AppConfig, FocusPreset},
    models::{Task, TaskMetadata, TimerPhase},
    store::FocusStore,
};
use rusqlite::Connection;
use std::path::PathBuf;
use tempfile::tempdir;

fn task(path: PathBuf, id: &str, text: &str) -> Task {
    Task {
        source_path: path,
        line_number: 2,
        text: text.into(),
        checked: false,
        heading: Some("Today".into()),
        task_id: Some(id.into()),
        indent: 0,
        metadata: TaskMetadata::default(),
    }
}

#[test]
fn python_database_schema_supports_pause_resume_and_partial_sessions() {
    let directory = tempdir().unwrap();
    let db_path = directory.path().join("focus.db");
    let store = FocusStore::open(&db_path).unwrap();
    let task = task(
        directory.path().join("TODO.md"),
        "task-1",
        "Implement timer",
    );

    let started = store
        .start_focus_at(&task, 60, Some("w1:p1"), 100.0)
        .unwrap();
    assert_eq!(started.phase, TimerPhase::Focus);
    assert_eq!(started.deadline_at, Some(160.0));

    let paused = store.pause_at(110.0).unwrap();
    assert_eq!(paused.phase, TimerPhase::PausedFocus);
    assert_eq!(paused.remaining_seconds, 50.0);
    assert_eq!(paused.accumulated_seconds, 10.0);

    let resumed = store.resume_at(200.0).unwrap();
    assert_eq!(resumed.phase, TimerPhase::Focus);
    assert_eq!(resumed.deadline_at, Some(250.0));

    let (idle, pane_id) = store.stop_at(205.0).unwrap();
    let sessions = store.recent_sessions("task-1", 10).unwrap();

    assert_eq!(idle.phase, TimerPhase::Idle);
    assert_eq!(pane_id.as_deref(), Some("w1:p1"));
    assert_eq!(sessions.len(), 1);
    assert_eq!(sessions[0].duration_seconds, 15);
    assert_eq!(sessions[0].started_at, 100.0);
    assert_eq!(sessions[0].ended_at, 205.0);
    assert_eq!(sessions[0].outcome, "stopped");
}

#[test]
fn task_summaries_distinguish_completed_and_stopped_focus_time() {
    let directory = tempdir().unwrap();
    let store = FocusStore::open(&directory.path().join("focus.db")).unwrap();
    let task = task(directory.path().join("TODO.md"), "task-1", "Study");
    let config = AppConfig::default();
    let completed = store.start_focus_at(&task, 10, None, 100.0).unwrap();
    store
        .complete_due_at(&completed.timer_id, &config, 110.0)
        .unwrap()
        .unwrap();
    store.stop_at(111.0).unwrap();
    store.start_focus_at(&task, 60, None, 200.0).unwrap();
    store.stop_at(205.0).unwrap();

    let summary = store
        .summaries_for_tasks(&["task-1".into()])
        .unwrap()
        .remove("task-1")
        .unwrap();

    assert_eq!(summary.total_seconds, 15);
    assert_eq!(summary.completed_seconds, 10);
    assert_eq!(summary.stopped_seconds, 5);
    assert_eq!(summary.completed_sessions, 1);
    assert_eq!(summary.stopped_sessions, 1);
}

#[test]
fn source_qualified_history_keeps_duplicate_task_ids_independent() {
    let directory = tempdir().unwrap();
    let store = FocusStore::open(&directory.path().join("focus.db")).unwrap();
    let first = task(directory.path().join("first.md"), "shared", "First");
    let second = task(directory.path().join("second.md"), "shared", "Second");
    store.start_focus_at(&first, 60, None, 100.0).unwrap();
    store.stop_at(105.0).unwrap();
    store.start_focus_at(&second, 60, None, 200.0).unwrap();
    store.stop_at(207.0).unwrap();
    let first_path = first.source_path.to_string_lossy().into_owned();
    let second_path = second.source_path.to_string_lossy().into_owned();

    let summaries = store
        .summaries_for_task_sources(&[
            ("shared".into(), first_path.clone()),
            ("shared".into(), second_path.clone()),
        ])
        .unwrap();

    assert_eq!(
        summaries[&("shared".into(), first_path.clone())].total_seconds,
        5
    );
    assert_eq!(
        summaries[&("shared".into(), second_path.clone())].total_seconds,
        7
    );
    assert_eq!(
        store
            .recent_sessions_for_source("shared", &first_path, 10)
            .unwrap()
            .len(),
        1
    );
}

#[test]
fn reset_restarts_a_focus_cycle_without_saving_the_interrupted_time() {
    let directory = tempdir().unwrap();
    let store = FocusStore::open(&directory.path().join("focus.db")).unwrap();
    let task = task(
        directory.path().join("TODO.md"),
        "task-1",
        "Implement timer",
    );
    let config = AppConfig {
        focus_minutes: 1.0,
        ..AppConfig::default()
    };
    store.start_focus_at(&task, 60, None, 100.0).unwrap();

    let reset = store.reset_at(&config, 125.0).unwrap();

    assert_eq!(reset.phase, TimerPhase::Focus);
    assert_eq!(reset.deadline_at, Some(185.0));
    assert_eq!(reset.accumulated_seconds, 0.0);
    assert!(store.recent_sessions("task-1", 10).unwrap().is_empty());
}

#[test]
fn preset_snapshot_and_explicit_ready_states_drive_the_whole_cycle() {
    let directory = tempdir().unwrap();
    let store = FocusStore::open(&directory.path().join("focus.db")).unwrap();
    let task = task(directory.path().join("TODO.md"), "task-1", "Study");
    let preset = FocusPreset {
        name: "sprint".into(),
        focus_minutes: 1.0 / 60.0,
        break_minutes: 2.0 / 60.0,
        long_break_minutes: 3.0 / 60.0,
        cycles_before_long_break: 2,
    };
    let config = AppConfig {
        break_minutes: 99.0,
        auto_start_break: false,
        auto_start_focus: false,
        ..AppConfig::default()
    };
    let focus = store
        .start_focus_with_preset_at(&task, &preset, None, 100.0)
        .unwrap();
    assert_eq!(focus.preset_name, "sprint");
    assert_eq!(focus.preset_focus_seconds, 1);
    assert_eq!(focus.preset_break_seconds, 2);

    let ready_break = store
        .complete_due_at(&focus.timer_id, &config, 101.0)
        .unwrap()
        .unwrap()
        .state;
    assert_eq!(ready_break.phase, TimerPhase::ReadyForBreak);
    assert!(!ready_break.phase.is_paused());
    assert_eq!(ready_break.remaining_seconds, 2.0);
    assert_eq!(ready_break.deadline_at, None);

    let break_state = store.start_ready_break_at(110.0).unwrap();
    assert_eq!(break_state.phase, TimerPhase::Break);
    assert_eq!(break_state.deadline_at, Some(112.0));
    let ready_focus = store
        .complete_due_at(&break_state.timer_id, &config, 112.0)
        .unwrap()
        .unwrap()
        .state;
    assert_eq!(ready_focus.phase, TimerPhase::ReadyForFocus);
    assert_eq!(ready_focus.remaining_seconds, 1.0);

    let next_focus = store.start_ready_focus_at(999, 120.0).unwrap();
    assert_eq!(next_focus.phase, TimerPhase::Focus);
    assert_eq!(next_focus.deadline_at, Some(121.0));
    assert_eq!(next_focus.preset_name, "sprint");
}

#[test]
fn completed_focus_automatically_transitions_to_break_then_ready() {
    let directory = tempdir().unwrap();
    let store = FocusStore::open(&directory.path().join("focus.db")).unwrap();
    let task = task(
        directory.path().join("TODO.md"),
        "task-1",
        "Implement timer",
    );
    let config = AppConfig {
        focus_minutes: 10.0 / 60.0,
        break_minutes: 1.0 / 60.0,
        long_break_minutes: 2.0 / 60.0,
        ..AppConfig::default()
    };
    let started = store.start_focus_at(&task, 10, None, 100.0).unwrap();

    let focus_transition = store
        .complete_due_at(&started.timer_id, &config, 110.0)
        .unwrap()
        .unwrap();

    assert_eq!(focus_transition.kind, "focus_completed");
    assert_eq!(focus_transition.state.phase, TimerPhase::Break);
    assert_eq!(focus_transition.state.deadline_at, Some(111.0));
    assert_eq!(focus_transition.state.cycle, 1);
    assert_eq!(
        store.totals_for_tasks(&["task-1".into()]).unwrap()["task-1"],
        10
    );

    let break_transition = store
        .complete_due_at(&focus_transition.state.timer_id, &config, 111.0)
        .unwrap()
        .unwrap();
    assert_eq!(break_transition.kind, "break_completed");
    assert_eq!(break_transition.state.phase, TimerPhase::ReadyForFocus);
    assert_eq!(
        break_transition.state.next_task_text.as_deref(),
        Some("Implement timer")
    );
}

#[test]
fn queuing_the_next_task_does_not_reassign_the_active_focus_session() {
    let directory = tempdir().unwrap();
    let store = FocusStore::open(&directory.path().join("focus.db")).unwrap();
    let first = task(directory.path().join("TODO.md"), "task-1", "Learn CS50");
    let second = task(
        directory.path().join("TODO.md"),
        "task-2",
        "Read distributed systems",
    );
    let config = AppConfig::default();
    let focus = store.start_focus_at(&first, 10, None, 100.0).unwrap();

    let queued = store.set_next_task_at(&second, 105.0).unwrap();

    assert_eq!(queued.active_task_id.as_deref(), Some("task-1"));
    assert_eq!(queued.next_task_id.as_deref(), Some("task-2"));

    let transition = store
        .complete_due_at(&focus.timer_id, &config, 110.0)
        .unwrap()
        .unwrap();
    let first_sessions = store.recent_sessions("task-1", 10).unwrap();
    let second_sessions = store.recent_sessions("task-2", 10).unwrap();

    assert_eq!(first_sessions.len(), 1);
    assert_eq!(first_sessions[0].duration_seconds, 10);
    assert!(second_sessions.is_empty());
    assert_eq!(transition.state.active_task_id.as_deref(), Some("task-1"));
    assert_eq!(transition.state.next_task_id.as_deref(), Some("task-2"));
}

#[test]
fn stopping_after_queuing_another_task_still_records_the_active_task() {
    let directory = tempdir().unwrap();
    let store = FocusStore::open(&directory.path().join("focus.db")).unwrap();
    let first = task(directory.path().join("TODO.md"), "task-1", "Learn CS50");
    let second = task(
        directory.path().join("TODO.md"),
        "task-2",
        "Read distributed systems",
    );
    store.start_focus_at(&first, 60, None, 100.0).unwrap();
    store.set_next_task_at(&second, 105.0).unwrap();

    store.stop_at(108.0).unwrap();

    let first_sessions = store.recent_sessions("task-1", 10).unwrap();
    assert_eq!(first_sessions.len(), 1);
    assert_eq!(first_sessions[0].duration_seconds, 8);
    assert!(store.recent_sessions("task-2", 10).unwrap().is_empty());
}

#[test]
fn queued_task_becomes_active_when_the_next_focus_starts() {
    let directory = tempdir().unwrap();
    let store = FocusStore::open(&directory.path().join("focus.db")).unwrap();
    let first = task(directory.path().join("TODO.md"), "task-1", "Learn CS50");
    let second = task(
        directory.path().join("TODO.md"),
        "task-2",
        "Read distributed systems",
    );
    let config = AppConfig {
        break_minutes: 1.0 / 60.0,
        ..AppConfig::default()
    };
    let focus = store.start_focus_at(&first, 1, None, 100.0).unwrap();
    store.set_next_task_at(&second, 100.5).unwrap();
    let break_state = store
        .complete_due_at(&focus.timer_id, &config, 101.0)
        .unwrap()
        .unwrap()
        .state;
    store
        .complete_due_at(&break_state.timer_id, &config, 102.0)
        .unwrap()
        .unwrap();

    let next_focus = store.start_ready_focus_at(10, 102.0).unwrap();

    assert_eq!(next_focus.active_task_id.as_deref(), Some("task-2"));
    assert_eq!(next_focus.next_task_id.as_deref(), Some("task-2"));
}

#[test]
fn opening_a_legacy_database_migrates_existing_task_context() {
    let directory = tempdir().unwrap();
    let db_path = directory.path().join("legacy.db");
    let connection = Connection::open(&db_path).unwrap();
    connection
        .execute_batch(
            "
            CREATE TABLE timer_state (
                singleton INTEGER PRIMARY KEY CHECK (singleton = 1),
                phase TEXT NOT NULL DEFAULT 'idle',
                timer_id TEXT NOT NULL DEFAULT '',
                task_id TEXT,
                task_text TEXT,
                source_path TEXT,
                source_line INTEGER,
                origin_pane_id TEXT,
                cycle INTEGER NOT NULL DEFAULT 0,
                session_started_at REAL,
                phase_started_at REAL,
                deadline_at REAL,
                remaining_seconds REAL NOT NULL DEFAULT 0,
                accumulated_seconds REAL NOT NULL DEFAULT 0,
                updated_at REAL NOT NULL DEFAULT 0
            );
            INSERT INTO timer_state (
                singleton, phase, timer_id, task_id, task_text, source_path,
                source_line, remaining_seconds, accumulated_seconds, updated_at
            ) VALUES (
                1, 'ready', 'legacy-timer', 'legacy-task', 'Legacy task',
                '/tmp/TODO.md', 7, 30, 15, 100
            );
            ",
        )
        .unwrap();
    drop(connection);

    let store = FocusStore::open(&db_path).unwrap();
    let state = store.get_timer().unwrap();

    assert_eq!(state.phase, TimerPhase::ReadyForFocus);
    assert_eq!(state.active_task_id.as_deref(), Some("legacy-task"));
    assert_eq!(state.next_task_id.as_deref(), Some("legacy-task"));
    assert_eq!(state.active_task_text.as_deref(), Some("Legacy task"));
    assert_eq!(state.next_task_text.as_deref(), Some("Legacy task"));
    assert_eq!(state.active_source_line, Some(7));
    assert_eq!(state.next_source_line, Some(7));
    assert_eq!(state.preset_name, "");
    assert_eq!(state.preset_focus_seconds, 0);
}

#[test]
fn another_task_can_be_queued_without_changing_the_break_deadline() {
    let directory = tempdir().unwrap();
    let store = FocusStore::open(&directory.path().join("focus.db")).unwrap();
    let first = task(directory.path().join("TODO.md"), "task-1", "Learn CS50");
    let second = task(
        directory.path().join("TODO.md"),
        "task-2",
        "Read distributed systems",
    );
    let config = AppConfig {
        break_minutes: 1.0 / 60.0,
        ..AppConfig::default()
    };
    let focus = store.start_focus_at(&first, 1, None, 100.0).unwrap();
    let break_state = store
        .complete_due_at(&focus.timer_id, &config, 101.0)
        .unwrap()
        .unwrap()
        .state;

    let queued = store.queue_next_task_at(&second, 101.5).unwrap();

    assert_eq!(queued.phase, TimerPhase::Break);
    assert_eq!(queued.deadline_at, break_state.deadline_at);
    assert_eq!(queued.active_task_id.as_deref(), Some("task-1"));
    assert_eq!(queued.next_task_id.as_deref(), Some("task-2"));
    assert_eq!(
        queued.next_task_text.as_deref(),
        Some("Read distributed systems")
    );
}
