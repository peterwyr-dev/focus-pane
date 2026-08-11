use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use focus_pane::{
    config::{AppConfig, FocusPreset},
    markdown::MarkdownTaskRepository,
    models::TimerPhase,
    store::FocusStore,
    ui::UiApp,
    workspace::WorkspaceDefinition,
};
use ratatui::{Terminal, backend::TestBackend, style::Color};
use std::fs;
use tempfile::tempdir;

#[test]
fn renders_herdr_style_search_tree_full_row_selection_and_status_footer() {
    let directory = tempdir().unwrap();
    let todo = directory.path().join("TODO.md");
    fs::write(
        &todo,
        "# Study plan\n\n- [ ] Learn CS50\n- [ ] Read Distributed Systems\n",
    )
    .unwrap();
    let repository = MarkdownTaskRepository::new(todo);
    let store = FocusStore::open(&directory.path().join("focus.db")).unwrap();
    let mut app = UiApp::new(repository, store, AppConfig::default(), None).unwrap();
    let backend = TestBackend::new(100, 30);
    let mut terminal = Terminal::new(backend).unwrap();

    terminal.draw(|frame| app.render(frame)).unwrap();

    let buffer = terminal.backend().buffer();
    let screen = screen_text(buffer, 100, 30);
    assert!(screen.contains("/ search tasks"));
    assert!(screen.contains("◇ Study plan (2)"));
    assert!(screen.contains("├─ ○ Learn CS50"));
    assert!(screen.contains("└─ ○ Read Distributed Systems"));
    assert!(screen.contains("READY"));
    assert!(screen.contains("enter focus/next"));

    let selected_row = (0..30)
        .find(|&y| row_text(buffer, 100, y).contains("Learn CS50"))
        .unwrap();
    assert_eq!(
        buffer.cell((50, selected_row)).unwrap().bg,
        Color::Rgb(196, 167, 231)
    );
}

#[test]
fn command_palette_filters_and_executes_registry_commands() {
    let directory = tempdir().unwrap();
    let todo = directory.path().join("TODO.md");
    fs::write(&todo, "- [ ] Palette task <!-- focus:id=palette -->\n").unwrap();
    let repository = MarkdownTaskRepository::new(todo.clone());
    let store = FocusStore::open(&directory.path().join("focus.db")).unwrap();
    let mut app = UiApp::new(repository, store, AppConfig::default(), None).unwrap();

    app.handle_key(KeyEvent::new(KeyCode::Char(':'), KeyModifiers::NONE))
        .unwrap();
    type_text(&mut app, "blocked");

    let backend = TestBackend::new(110, 32);
    let mut terminal = Terminal::new(backend).unwrap();
    terminal.draw(|frame| app.render(frame)).unwrap();
    let screen = screen_text(terminal.backend().buffer(), 110, 32);
    assert!(screen.contains("Command palette"));
    assert!(screen.contains("Toggle blocked"));
    assert!(!screen.contains("Append task —"));

    app.handle_key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE))
        .unwrap();
    assert!(fs::read_to_string(todo).unwrap().contains("status=blocked"));
}

#[test]
fn question_mark_opens_help_generated_from_the_command_registry() {
    let directory = tempdir().unwrap();
    let todo = directory.path().join("TODO.md");
    fs::write(&todo, "- [ ] Help task <!-- focus:id=help -->\n").unwrap();
    let repository = MarkdownTaskRepository::new(todo);
    let store = FocusStore::open(&directory.path().join("focus.db")).unwrap();
    let mut app = UiApp::new(repository, store, AppConfig::default(), None).unwrap();

    app.handle_key(KeyEvent::new(KeyCode::Char('?'), KeyModifiers::NONE))
        .unwrap();

    let backend = TestBackend::new(110, 32);
    let mut terminal = Terminal::new(backend).unwrap();
    terminal.draw(|frame| app.render(frame)).unwrap();
    let screen = screen_text(terminal.backend().buffer(), 110, 32);
    assert!(screen.contains("Focus Pane help"));
    assert!(screen.contains("Set estimate"));
    assert!(screen.contains("Command palette"));
    assert!(screen.contains("dimmed = unavailable now"));
}

#[test]
fn uppercase_p_cycles_and_persists_the_idle_focus_preset() {
    let directory = tempdir().unwrap();
    let todo = directory.path().join("TODO.md");
    fs::write(&todo, "- [ ] Study <!-- focus:id=study -->\n").unwrap();
    let repository = MarkdownTaskRepository::new(todo);
    let store = FocusStore::open(&directory.path().join("focus.db")).unwrap();
    let config = AppConfig {
        default_preset: "standard".into(),
        presets: vec![
            FocusPreset {
                name: "standard".into(),
                focus_minutes: 25.0,
                break_minutes: 5.0,
                long_break_minutes: 15.0,
                cycles_before_long_break: 4,
            },
            FocusPreset {
                name: "deep".into(),
                focus_minutes: 50.0,
                break_minutes: 10.0,
                long_break_minutes: 20.0,
                cycles_before_long_break: 3,
            },
        ],
        ..AppConfig::default()
    };
    let mut app = UiApp::new(repository, store.clone(), config, None).unwrap();

    app.handle_key(KeyEvent::new(KeyCode::Char('P'), KeyModifiers::SHIFT))
        .unwrap();

    let state = store.get_timer().unwrap();
    assert_eq!(state.preset_name, "deep");
    assert_eq!(state.preset_focus_seconds, 3000);
    assert_eq!(state.preset_break_seconds, 600);
    assert_eq!(state.preset_cycles_before_long_break, 3);
    let backend = TestBackend::new(100, 30);
    let mut terminal = Terminal::new(backend).unwrap();
    terminal.draw(|frame| app.render(frame)).unwrap();
    assert!(screen_text(terminal.backend().buffer(), 100, 30).contains("preset deep"));
}

#[test]
fn ready_for_break_is_rendered_as_waiting_instead_of_paused() {
    let directory = tempdir().unwrap();
    let todo = directory.path().join("TODO.md");
    fs::write(&todo, "- [ ] Study <!-- focus:id=study -->\n").unwrap();
    let repository = MarkdownTaskRepository::new(todo);
    let task = repository.read_tasks().unwrap().remove(0);
    let store = FocusStore::open(&directory.path().join("focus.db")).unwrap();
    let preset = FocusPreset {
        name: "sprint".into(),
        focus_minutes: 1.0,
        break_minutes: 5.0,
        long_break_minutes: 15.0,
        cycles_before_long_break: 4,
    };
    let config = AppConfig {
        auto_start_break: false,
        presets: vec![preset.clone()],
        default_preset: "sprint".into(),
        ..AppConfig::default()
    };
    let focus = store
        .start_focus_with_preset_at(&task, &preset, None, 100.0)
        .unwrap();
    store
        .complete_due_at(&focus.timer_id, &config, 160.0)
        .unwrap()
        .unwrap();
    let mut app = UiApp::new(repository, store, config, None).unwrap();

    let backend = TestBackend::new(100, 30);
    let mut terminal = Terminal::new(backend).unwrap();
    terminal.draw(|frame| app.render(frame)).unwrap();
    let screen = screen_text(terminal.backend().buffer(), 100, 30);
    assert!(screen.contains("READY FOR BREAK"));
    assert!(screen.contains("press p to start"));
    assert!(!screen.contains("BREAK PAUSED"));
}

#[test]
fn pressing_n_opens_a_centered_add_task_panel() {
    let directory = tempdir().unwrap();
    let todo = directory.path().join("TODO.md");
    fs::write(&todo, "# Study\n\n- [ ] Existing task\n").unwrap();
    let repository = MarkdownTaskRepository::new(todo);
    let store = FocusStore::open(&directory.path().join("focus.db")).unwrap();
    let mut app = UiApp::new(repository, store, AppConfig::default(), None).unwrap();

    app.handle_key(KeyEvent::new(KeyCode::Char('n'), KeyModifiers::NONE))
        .unwrap();
    let backend = TestBackend::new(100, 30);
    let mut terminal = Terminal::new(backend).unwrap();
    terminal.draw(|frame| app.render(frame)).unwrap();
    let screen = screen_text(terminal.backend().buffer(), 100, 30);

    assert!(screen.contains("Add a Markdown task"));
    assert!(screen.contains("enter add"));
    assert!(screen.contains("esc cancel"));
}

#[test]
fn d_opens_confirmation_and_enter_deletes_the_selected_task() {
    let directory = tempdir().unwrap();
    let todo = directory.path().join("TODO.md");
    fs::write(&todo, "# Study\n\n- [ ] Delete me\n- [ ] Keep me\n").unwrap();
    let repository = MarkdownTaskRepository::new(todo.clone());
    let store = FocusStore::open(&directory.path().join("focus.db")).unwrap();
    let mut app = UiApp::new(repository, store, AppConfig::default(), None).unwrap();

    app.handle_key(KeyEvent::new(KeyCode::Char('d'), KeyModifiers::NONE))
        .unwrap();
    let backend = TestBackend::new(100, 30);
    let mut terminal = Terminal::new(backend).unwrap();
    terminal.draw(|frame| app.render(frame)).unwrap();
    let screen = screen_text(terminal.backend().buffer(), 100, 30);
    assert!(screen.contains("Delete task?"));
    assert!(screen.contains("Delete me"));

    app.handle_key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE))
        .unwrap();

    let markdown = fs::read_to_string(todo).unwrap();
    assert!(!markdown.contains("Delete me"));
    assert!(markdown.contains("Keep me"));
}

#[test]
fn deleting_a_queued_task_clears_its_timer_context() {
    let directory = tempdir().unwrap();
    let todo = directory.path().join("TODO.md");
    fs::write(
        &todo,
        "# Study\n\n- [ ] Active task <!-- focus:id=active -->\n",
    )
    .unwrap();
    let repository = MarkdownTaskRepository::new(todo.clone());
    let task = repository.read_tasks().unwrap().remove(0);
    let store = FocusStore::open(&directory.path().join("focus.db")).unwrap();
    store.start_focus_at(&task, 60, None, 100.0).unwrap();
    let mut app = UiApp::new(repository, store.clone(), AppConfig::default(), None).unwrap();

    app.handle_key(KeyEvent::new(KeyCode::Char('d'), KeyModifiers::NONE))
        .unwrap();
    app.handle_key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE))
        .unwrap();

    assert!(!fs::read_to_string(todo).unwrap().contains("Active task"));
    let state = store.get_timer().unwrap();
    assert!(state.next_task_id.is_none());
    assert_eq!(state.active_task_id.as_deref(), Some("active"));
}

#[test]
fn space_is_the_only_ui_action_that_completes_the_selected_markdown_task() {
    let directory = tempdir().unwrap();
    let todo = directory.path().join("TODO.md");
    fs::write(&todo, "# Study\n\n- [ ] Learn CS50\n- [ ] Read systems\n").unwrap();
    let repository = MarkdownTaskRepository::new(todo.clone());
    let store = FocusStore::open(&directory.path().join("focus.db")).unwrap();
    let mut app = UiApp::new(repository, store, AppConfig::default(), None).unwrap();

    app.handle_key(KeyEvent::new(KeyCode::Char(' '), KeyModifiers::NONE))
        .unwrap();

    let markdown = fs::read_to_string(todo).unwrap();
    assert!(markdown.contains("- [x] Learn CS50"));
    assert!(markdown.contains("- [ ] Read systems"));
}

#[test]
fn u_reopens_the_most_recently_completed_task() {
    let directory = tempdir().unwrap();
    let todo = directory.path().join("TODO.md");
    fs::write(
        &todo,
        "# Study\n\n- [ ] First <!-- focus:id=first -->\n- [ ] Second <!-- focus:id=second -->\n",
    )
    .unwrap();
    let repository = MarkdownTaskRepository::new(todo.clone());
    let store = FocusStore::open(&directory.path().join("focus.db")).unwrap();
    let mut app = UiApp::new(repository, store, AppConfig::default(), None).unwrap();

    app.handle_key(KeyEvent::new(KeyCode::Char(' '), KeyModifiers::NONE))
        .unwrap();
    assert!(fs::read_to_string(&todo).unwrap().contains("- [x] First"));

    app.handle_key(KeyEvent::new(KeyCode::Char('u'), KeyModifiers::NONE))
        .unwrap();

    let markdown = fs::read_to_string(todo).unwrap();
    assert!(markdown.contains("- [ ] First"));
    assert!(markdown.contains("- [ ] Second"));
}

#[test]
fn undoing_completion_restores_the_previous_next_task() {
    let directory = tempdir().unwrap();
    let todo = directory.path().join("TODO.md");
    fs::write(
        &todo,
        "# Study\n\n- [ ] First <!-- focus:id=first -->\n- [ ] Second <!-- focus:id=second -->\n",
    )
    .unwrap();
    let repository = MarkdownTaskRepository::new(todo);
    let tasks = repository.read_tasks().unwrap();
    let store = FocusStore::open(&directory.path().join("focus.db")).unwrap();
    store.start_focus_at(&tasks[0], 60, None, 100.0).unwrap();
    let mut app = UiApp::new(repository, store.clone(), AppConfig::default(), None).unwrap();

    app.handle_key(KeyEvent::new(KeyCode::Char(' '), KeyModifiers::NONE))
        .unwrap();
    assert_eq!(
        store.get_timer().unwrap().next_task_id.as_deref(),
        Some("second")
    );

    app.handle_key(KeyEvent::new(KeyCode::Char('u'), KeyModifiers::NONE))
        .unwrap();

    assert_eq!(
        store.get_timer().unwrap().next_task_id.as_deref(),
        Some("first")
    );
}

#[test]
fn completing_a_task_advances_the_next_task_without_resetting_focus() {
    let directory = tempdir().unwrap();
    let todo = directory.path().join("TODO.md");
    fs::write(
        &todo,
        "# Study\n\n- [ ] First <!-- focus:id=first -->\n- [ ] Second <!-- focus:id=second -->\n",
    )
    .unwrap();
    let repository = MarkdownTaskRepository::new(todo);
    let tasks = repository.read_tasks().unwrap();
    let store = FocusStore::open(&directory.path().join("focus.db")).unwrap();
    store.start_focus_at(&tasks[0], 60, None, 100.0).unwrap();
    let mut app = UiApp::new(repository, store.clone(), AppConfig::default(), None).unwrap();

    app.handle_key(KeyEvent::new(KeyCode::Char(' '), KeyModifiers::NONE))
        .unwrap();

    let state = store.get_timer().unwrap();
    assert_eq!(state.phase, TimerPhase::Focus);
    assert_eq!(state.active_task_id.as_deref(), Some("first"));
    assert_eq!(state.next_task_id.as_deref(), Some("second"));
    assert_eq!(state.next_task_text.as_deref(), Some("Second"));
}

#[test]
fn enter_queues_the_selected_task_during_a_break() {
    let directory = tempdir().unwrap();
    let todo = directory.path().join("TODO.md");
    fs::write(
        &todo,
        "# Study\n\n".to_owned()
            + "- [ ] Learn CS50 <!-- focus:id=cs50 -->\n"
            + "- [ ] Read systems <!-- focus:id=systems -->\n",
    )
    .unwrap();
    let repository = MarkdownTaskRepository::new(todo);
    let tasks = repository.read_tasks().unwrap();
    let store = FocusStore::open(&directory.path().join("focus.db")).unwrap();
    let config = AppConfig {
        focus_minutes: 1.0 / 60.0,
        break_minutes: 5.0,
        auto_start_break: false,
        auto_start_focus: true,
        ..AppConfig::default()
    };
    let focus = store.start_focus_at(&tasks[0], 1, None, 100.0).unwrap();
    let transition = store
        .complete_due_at(&focus.timer_id, &config, 101.0)
        .unwrap()
        .unwrap();
    assert_eq!(transition.state.phase, TimerPhase::ReadyForBreak);
    let mut app = UiApp::new(repository, store.clone(), config, None).unwrap();

    app.handle_key(KeyEvent::new(KeyCode::Down, KeyModifiers::NONE))
        .unwrap();
    app.handle_key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE))
        .unwrap();

    let queued = store.get_timer().unwrap();
    assert_eq!(queued.phase, TimerPhase::ReadyForBreak);
    assert_eq!(queued.active_task_id.as_deref(), Some("cs50"));
    assert_eq!(queued.next_task_id.as_deref(), Some("systems"));
}

#[test]
fn e_edits_task_text_and_preserves_focus_metadata() {
    let directory = tempdir().unwrap();
    let todo = directory.path().join("TODO.md");
    fs::write(
        &todo,
        "# Study\n\n- [ ] Old text <!-- focus:estimate=2 custom=keep --> <!-- focus:id=edit -->\n",
    )
    .unwrap();
    let repository = MarkdownTaskRepository::new(todo.clone());
    let store = FocusStore::open(&directory.path().join("focus.db")).unwrap();
    let mut app = UiApp::new(repository, store, AppConfig::default(), None).unwrap();

    app.handle_key(KeyEvent::new(KeyCode::Char('e'), KeyModifiers::NONE))
        .unwrap();
    app.handle_key(KeyEvent::new(KeyCode::Char('u'), KeyModifiers::CONTROL))
        .unwrap();
    type_text(&mut app, "Edited #rust");
    app.handle_key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE))
        .unwrap();

    assert_eq!(
        fs::read_to_string(todo).unwrap(),
        "# Study\n\n- [ ] Edited #rust <!-- focus:estimate=2 custom=keep --> <!-- focus:id=edit -->\n"
    );
}

#[test]
fn edit_modal_supports_inserting_at_the_cursor() {
    let directory = tempdir().unwrap();
    let todo = directory.path().join("TODO.md");
    fs::write(&todo, "- [ ] ABCD <!-- focus:id=edit -->\n").unwrap();
    let repository = MarkdownTaskRepository::new(todo.clone());
    let store = FocusStore::open(&directory.path().join("focus.db")).unwrap();
    let mut app = UiApp::new(repository, store, AppConfig::default(), None).unwrap();

    app.handle_key(KeyEvent::new(KeyCode::Char('e'), KeyModifiers::NONE))
        .unwrap();
    app.handle_key(KeyEvent::new(KeyCode::Left, KeyModifiers::NONE))
        .unwrap();
    app.handle_key(KeyEvent::new(KeyCode::Left, KeyModifiers::NONE))
        .unwrap();
    app.handle_key(KeyEvent::new(KeyCode::Char('X'), KeyModifiers::SHIFT))
        .unwrap();
    app.handle_key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE))
        .unwrap();

    assert!(fs::read_to_string(todo).unwrap().contains("ABXCD"));
}

#[test]
fn a_inserts_a_sibling_after_the_selected_subtree() {
    let directory = tempdir().unwrap();
    let todo = directory.path().join("TODO.md");
    fs::write(
        &todo,
        "# Study\n- [ ] Parent <!-- focus:id=parent -->\n  - [ ] Child <!-- focus:id=child -->\n- [ ] Existing sibling <!-- focus:id=sibling -->\n",
    )
    .unwrap();
    let repository = MarkdownTaskRepository::new(todo.clone());
    let store = FocusStore::open(&directory.path().join("focus.db")).unwrap();
    let mut app = UiApp::new(repository, store, AppConfig::default(), None).unwrap();

    app.handle_key(KeyEvent::new(KeyCode::Char('a'), KeyModifiers::NONE))
        .unwrap();
    type_text(&mut app, "New sibling");
    app.handle_key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE))
        .unwrap();

    let markdown = fs::read_to_string(todo).unwrap();
    assert!(
        markdown.find("Child").unwrap() < markdown.find("New sibling").unwrap()
            && markdown.find("New sibling").unwrap() < markdown.find("Existing sibling").unwrap()
    );
}

#[test]
fn uppercase_j_moves_the_selected_task_and_its_children_down() {
    let directory = tempdir().unwrap();
    let todo = directory.path().join("TODO.md");
    fs::write(
        &todo,
        "# Study\n- [ ] Parent <!-- focus:id=parent -->\n  - [ ] Child <!-- focus:id=child -->\n- [ ] Sibling <!-- focus:id=sibling -->\n",
    )
    .unwrap();
    let repository = MarkdownTaskRepository::new(todo.clone());
    let store = FocusStore::open(&directory.path().join("focus.db")).unwrap();
    let mut app = UiApp::new(repository, store, AppConfig::default(), None).unwrap();

    app.handle_key(KeyEvent::new(KeyCode::Char('J'), KeyModifiers::SHIFT))
        .unwrap();

    let markdown = fs::read_to_string(todo).unwrap();
    assert!(markdown.find("Sibling").unwrap() < markdown.find("Parent").unwrap());
    assert!(markdown.find("Parent").unwrap() < markdown.find("Child").unwrap());
}

#[test]
fn angle_brackets_indent_and_outdent_the_selected_subtree() {
    let directory = tempdir().unwrap();
    let todo = directory.path().join("TODO.md");
    fs::write(
        &todo,
        "- [ ] First <!-- focus:id=first -->\n- [ ] Second <!-- focus:id=second -->\n  - [ ] Child <!-- focus:id=child -->\n",
    )
    .unwrap();
    let repository = MarkdownTaskRepository::new(todo.clone());
    let store = FocusStore::open(&directory.path().join("focus.db")).unwrap();
    let mut app = UiApp::new(repository, store, AppConfig::default(), None).unwrap();
    app.handle_key(KeyEvent::new(KeyCode::Down, KeyModifiers::NONE))
        .unwrap();

    app.handle_key(KeyEvent::new(KeyCode::Char('>'), KeyModifiers::SHIFT))
        .unwrap();
    let indented = fs::read_to_string(&todo).unwrap();
    assert!(indented.contains("  - [ ] Second"));
    assert!(indented.contains("    - [ ] Child"));

    app.handle_key(KeyEvent::new(KeyCode::Char('<'), KeyModifiers::SHIFT))
        .unwrap();
    let outdented = fs::read_to_string(todo).unwrap();
    assert!(outdented.contains("- [ ] Second"));
    assert!(outdented.contains("  - [ ] Child"));
}

#[test]
fn b_toggles_blocked_metadata_and_uses_a_distinct_list_marker() {
    let directory = tempdir().unwrap();
    let todo = directory.path().join("TODO.md");
    fs::write(&todo, "# Study\n- [ ] Block me <!-- focus:id=blocked -->\n").unwrap();
    let repository = MarkdownTaskRepository::new(todo.clone());
    let store = FocusStore::open(&directory.path().join("focus.db")).unwrap();
    let mut app = UiApp::new(repository, store, AppConfig::default(), None).unwrap();

    app.handle_key(KeyEvent::new(KeyCode::Char('b'), KeyModifiers::NONE))
        .unwrap();

    assert!(
        fs::read_to_string(&todo)
            .unwrap()
            .contains("status=blocked")
    );
    let backend = TestBackend::new(100, 30);
    let mut terminal = Terminal::new(backend).unwrap();
    terminal.draw(|frame| app.render(frame)).unwrap();
    let screen = screen_text(terminal.backend().buffer(), 100, 30);
    assert!(screen.contains("! Block me"));
    assert!(screen.contains("blocked"));

    app.handle_key(KeyEvent::new(KeyCode::Char('b'), KeyModifiers::NONE))
        .unwrap();
    assert!(!fs::read_to_string(todo).unwrap().contains("status="));
}

#[test]
fn blocked_tasks_can_be_found_by_searching_for_blocked() {
    let directory = tempdir().unwrap();
    let todo = directory.path().join("TODO.md");
    fs::write(
        &todo,
        "- [ ] Blocked task <!-- focus:status=blocked --> <!-- focus:id=blocked -->\n- [ ] Ordinary task <!-- focus:id=ordinary -->\n",
    )
    .unwrap();
    let repository = MarkdownTaskRepository::new(todo);
    let store = FocusStore::open(&directory.path().join("focus.db")).unwrap();
    let mut app = UiApp::new(repository, store, AppConfig::default(), None).unwrap();

    app.handle_key(KeyEvent::new(KeyCode::Char('/'), KeyModifiers::NONE))
        .unwrap();
    type_text(&mut app, "blocked");

    let backend = TestBackend::new(100, 30);
    let mut terminal = Terminal::new(backend).unwrap();
    terminal.draw(|frame| app.render(frame)).unwrap();
    let screen = screen_text(terminal.backend().buffer(), 100, 30);
    assert!(screen.contains("Blocked task"));
    assert!(!screen.contains("Ordinary task"));
}

#[test]
fn uppercase_e_sets_and_clears_a_pomodoro_estimate() {
    let directory = tempdir().unwrap();
    let todo = directory.path().join("TODO.md");
    fs::write(
        &todo,
        "# Study\n- [ ] Estimate me <!-- focus:id=estimate -->\n",
    )
    .unwrap();
    let repository = MarkdownTaskRepository::new(todo.clone());
    let store = FocusStore::open(&directory.path().join("focus.db")).unwrap();
    let mut app = UiApp::new(repository, store, AppConfig::default(), None).unwrap();

    app.handle_key(KeyEvent::new(KeyCode::Char('E'), KeyModifiers::SHIFT))
        .unwrap();
    type_text(&mut app, "3");
    app.handle_key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE))
        .unwrap();

    assert!(fs::read_to_string(&todo).unwrap().contains("estimate=3"));
    let backend = TestBackend::new(100, 30);
    let mut terminal = Terminal::new(backend).unwrap();
    terminal.draw(|frame| app.render(frame)).unwrap();
    let screen = screen_text(terminal.backend().buffer(), 100, 30);
    assert!(screen.contains("0/3p"));

    app.handle_key(KeyEvent::new(KeyCode::Char('E'), KeyModifiers::SHIFT))
        .unwrap();
    app.handle_key(KeyEvent::new(KeyCode::Char('u'), KeyModifiers::CONTROL))
        .unwrap();
    app.handle_key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE))
        .unwrap();
    assert!(!fs::read_to_string(todo).unwrap().contains("estimate="));
}

#[test]
fn completed_estimate_status_shows_variance_and_session_outcome_breakdown() {
    let directory = tempdir().unwrap();
    let todo = directory.path().join("TODO.md");
    fs::write(
        &todo,
        "# Study\n- [x] Estimated <!-- focus:estimate=2 --> <!-- focus:id=estimate -->\n",
    )
    .unwrap();
    let repository = MarkdownTaskRepository::new(todo);
    let task = repository.read_tasks().unwrap().remove(0);
    let store = FocusStore::open(&directory.path().join("focus.db")).unwrap();
    let config = AppConfig {
        focus_minutes: 1.0,
        ..AppConfig::default()
    };
    let completed = store.start_focus_at(&task, 60, None, 100.0).unwrap();
    store
        .complete_due_at(&completed.timer_id, &config, 160.0)
        .unwrap()
        .unwrap();
    store.stop_at(161.0).unwrap();
    store.start_focus_at(&task, 60, None, 200.0).unwrap();
    store.stop_at(230.0).unwrap();
    let mut app = UiApp::new(repository, store, config, None).unwrap();
    app.handle_key(KeyEvent::new(KeyCode::Tab, KeyModifiers::NONE))
        .unwrap();

    let backend = TestBackend::new(120, 30);
    let mut terminal = Terminal::new(backend).unwrap();
    terminal.draw(|frame| app.render(frame)).unwrap();
    let screen = screen_text(terminal.backend().buffer(), 120, 30);

    assert!(screen.contains("estimate 1.5/2p"));
    assert!(screen.contains("under estimate"));
    assert!(screen.contains("full 1m (1x)"));
    assert!(screen.contains("stopped 1m (1x)"));
}

#[test]
fn edit_conflict_reloads_the_task_but_keeps_the_local_draft_for_retry() {
    let directory = tempdir().unwrap();
    let todo = directory.path().join("TODO.md");
    fs::write(&todo, "# Study\n- [ ] Original <!-- focus:id=edit -->\n").unwrap();
    let repository = MarkdownTaskRepository::new(todo.clone());
    let store = FocusStore::open(&directory.path().join("focus.db")).unwrap();
    let mut app = UiApp::new(repository, store, AppConfig::default(), None).unwrap();
    app.handle_key(KeyEvent::new(KeyCode::Char('e'), KeyModifiers::NONE))
        .unwrap();
    app.handle_key(KeyEvent::new(KeyCode::Char('u'), KeyModifiers::CONTROL))
        .unwrap();
    type_text(&mut app, "Local draft");
    fs::write(
        &todo,
        "# Study\n- [ ] External edit <!-- focus:id=edit -->\n",
    )
    .unwrap();
    app.sync_external_changes().unwrap();

    app.handle_key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE))
        .unwrap();
    assert!(fs::read_to_string(&todo).unwrap().contains("External edit"));
    app.handle_key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE))
        .unwrap();

    assert!(fs::read_to_string(todo).unwrap().contains("Local draft"));
}

#[test]
fn external_markdown_changes_reload_and_preserve_selection_by_task_id() {
    let directory = tempdir().unwrap();
    let todo = directory.path().join("TODO.md");
    fs::write(
        &todo,
        "# Study\n\n- [ ] First <!-- focus:id=first -->\n- [ ] Selected <!-- focus:id=selected -->\n",
    )
    .unwrap();
    let repository = MarkdownTaskRepository::new(todo.clone());
    let store = FocusStore::open(&directory.path().join("focus.db")).unwrap();
    let mut app = UiApp::new(repository, store, AppConfig::default(), None).unwrap();
    app.handle_key(KeyEvent::new(KeyCode::Down, KeyModifiers::NONE))
        .unwrap();
    fs::write(
        &todo,
        "# Study\n\n- [ ] Added externally <!-- focus:id=external -->\n- [ ] First <!-- focus:id=first -->\n- [ ] Selected <!-- focus:id=selected -->\n",
    )
    .unwrap();

    assert!(app.sync_external_changes().unwrap());

    let backend = TestBackend::new(100, 30);
    let mut terminal = Terminal::new(backend).unwrap();
    terminal.draw(|frame| app.render(frame)).unwrap();
    let buffer = terminal.backend().buffer();
    let screen = screen_text(buffer, 100, 30);
    assert!(screen.contains("Added externally"));
    assert!(screen.contains("Markdown reloaded after an external change"));
    let selected_row = (0..30)
        .find(|&y| row_text(buffer, 100, y).contains("Selected"))
        .unwrap();
    assert_eq!(
        buffer.cell((50, selected_row)).unwrap().bg,
        Color::Rgb(196, 167, 231)
    );
}

#[test]
fn external_change_during_add_keeps_the_draft_and_merges_with_the_latest_file() {
    let directory = tempdir().unwrap();
    let todo = directory.path().join("TODO.md");
    fs::write(
        &todo,
        "# Study\n\n- [ ] Existing <!-- focus:id=existing -->\n",
    )
    .unwrap();
    let repository = MarkdownTaskRepository::new(todo.clone());
    let store = FocusStore::open(&directory.path().join("focus.db")).unwrap();
    let mut app = UiApp::new(repository, store, AppConfig::default(), None).unwrap();
    app.handle_key(KeyEvent::new(KeyCode::Char('n'), KeyModifiers::NONE))
        .unwrap();
    for character in "Local draft".chars() {
        app.handle_key(KeyEvent::new(KeyCode::Char(character), KeyModifiers::NONE))
            .unwrap();
    }
    fs::write(
        &todo,
        "# Study\n\n- [ ] Existing <!-- focus:id=existing -->\n- [ ] External <!-- focus:id=external -->\n",
    )
    .unwrap();

    assert!(app.sync_external_changes().unwrap());
    app.handle_key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE))
        .unwrap();

    let markdown = fs::read_to_string(todo).unwrap();
    assert!(markdown.contains("External"));
    assert!(markdown.contains("Local draft"));
}

#[test]
fn deleting_a_task_changed_by_an_editor_recovers_without_overwriting_it() {
    let directory = tempdir().unwrap();
    let todo = directory.path().join("TODO.md");
    fs::write(
        &todo,
        "# Study\n\n- [ ] Original <!-- focus:id=task-1 -->\n",
    )
    .unwrap();
    let repository = MarkdownTaskRepository::new(todo.clone());
    let store = FocusStore::open(&directory.path().join("focus.db")).unwrap();
    let mut app = UiApp::new(repository, store, AppConfig::default(), None).unwrap();
    app.handle_key(KeyEvent::new(KeyCode::Char('d'), KeyModifiers::NONE))
        .unwrap();
    fs::write(
        &todo,
        "# Study\n\n- [ ] Edited externally <!-- focus:id=task-1 -->\n",
    )
    .unwrap();

    app.handle_key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE))
        .unwrap();

    let markdown = fs::read_to_string(&todo).unwrap();
    assert!(markdown.contains("Edited externally"));
    let backend = TestBackend::new(100, 30);
    let mut terminal = Terminal::new(backend).unwrap();
    terminal.draw(|frame| app.render(frame)).unwrap();
    let screen = screen_text(terminal.backend().buffer(), 100, 30);
    assert!(screen.contains("task changed externally; Markdown reloaded"));
}

#[test]
fn aggregate_view_groups_duplicate_headings_by_source_and_mutates_the_selected_file() {
    let directory = tempdir().unwrap();
    let first_dir = directory.path().join("first");
    let second_dir = directory.path().join("second");
    let work_dir = directory.path().join("work");
    fs::create_dir_all(&first_dir).unwrap();
    fs::create_dir_all(&second_dir).unwrap();
    fs::create_dir_all(&work_dir).unwrap();
    let first = first_dir.join("TODO.md");
    let second = second_dir.join("TODO.md");
    let work = work_dir.join("TODO.md");
    let missing = work_dir.join("MISSING.md");
    fs::write(
        &first,
        "# Shared\n- [ ] First source <!-- focus:id=shared -->\n",
    )
    .unwrap();
    fs::write(
        &second,
        "# Shared\n- [ ] Second source <!-- focus:id=shared -->\n",
    )
    .unwrap();
    fs::write(
        &work,
        "# Shared\n- [ ] Work source <!-- focus:id=work -->\n",
    )
    .unwrap();
    let definitions = vec![
        WorkspaceDefinition {
            name: "Study".into(),
            files: vec![first.clone(), second.clone()],
            warnings: Vec::new(),
        },
        WorkspaceDefinition {
            name: "Work".into(),
            files: vec![work, missing.clone()],
            warnings: Vec::new(),
        },
    ];
    let store = FocusStore::open(&directory.path().join("focus.db")).unwrap();
    let mut app = UiApp::new_with_workspaces(
        definitions,
        0,
        store,
        AppConfig::default(),
        directory.path().join("config.toml"),
        None,
    )
    .unwrap();

    let aggregate = render_app_at(&mut app, 120, 32);
    assert!(aggregate.contains("first/TODO.md › Shared"));
    assert!(aggregate.contains("second/TODO.md › Shared"));
    assert!(aggregate.contains("First source"));
    assert!(aggregate.contains("Second source"));

    app.handle_key(KeyEvent::new(KeyCode::Char('F'), KeyModifiers::NONE))
        .unwrap();
    let first_view = render_app_at(&mut app, 120, 32);
    assert!(first_view.contains("First source"));
    assert!(!first_view.contains("Second source"));
    app.handle_key(KeyEvent::new(KeyCode::Char('F'), KeyModifiers::NONE))
        .unwrap();
    let second_view = render_app_at(&mut app, 120, 32);
    assert!(!second_view.contains("First source"));
    assert!(second_view.contains("Second source"));

    app.handle_key(KeyEvent::new(KeyCode::Char(' '), KeyModifiers::NONE))
        .unwrap();
    assert!(
        fs::read_to_string(&second)
            .unwrap()
            .contains("- [x] Second source")
    );
    assert!(
        fs::read_to_string(&first)
            .unwrap()
            .contains("- [ ] First source")
    );

    app.handle_key(KeyEvent::new(KeyCode::Char('W'), KeyModifiers::NONE))
        .unwrap();
    let work_view = render_app_at(&mut app, 120, 32);
    assert!(work_view.contains("Work source"));
    assert!(work_view.contains("[missing]"));
    assert!(work_view.contains(&missing.display().to_string()));
}

#[test]
fn unreadable_markdown_source_is_reported_without_hiding_other_files() {
    let directory = tempdir().unwrap();
    let good = directory.path().join("GOOD.md");
    let invalid = directory.path().join("INVALID.md");
    fs::write(&good, "- [ ] Good task <!-- focus:id=good -->\n").unwrap();
    fs::write(&invalid, [0xff, 0xfe]).unwrap();
    let store = FocusStore::open(&directory.path().join("focus.db")).unwrap();
    let mut app = UiApp::new_with_workspaces(
        vec![WorkspaceDefinition {
            name: "Mixed".into(),
            files: vec![good, invalid.clone()],
            warnings: Vec::new(),
        }],
        0,
        store,
        AppConfig::default(),
        directory.path().join("config.toml"),
        None,
    )
    .unwrap();

    let screen = render_app_at(&mut app, 120, 28);

    assert!(screen.contains("Good task"));
    assert!(screen.contains("[unavailable]"));
    assert!(screen.contains("INVALID.md"));
}

#[test]
fn external_changes_in_any_aggregate_source_are_detected() {
    let directory = tempdir().unwrap();
    let first = directory.path().join("FIRST.md");
    let second = directory.path().join("SECOND.md");
    fs::write(&first, "- [ ] First <!-- focus:id=first -->\n").unwrap();
    fs::write(&second, "- [ ] Before <!-- focus:id=second -->\n").unwrap();
    let store = FocusStore::open(&directory.path().join("focus.db")).unwrap();
    let mut app = UiApp::new_with_workspaces(
        vec![WorkspaceDefinition {
            name: "Combined".into(),
            files: vec![first, second.clone()],
            warnings: Vec::new(),
        }],
        0,
        store,
        AppConfig::default(),
        directory.path().join("config.toml"),
        None,
    )
    .unwrap();

    fs::write(&second, "- [ ] After <!-- focus:id=second -->\n").unwrap();

    assert!(app.sync_external_changes().unwrap());
    assert!(render_app_at(&mut app, 110, 28).contains("After"));
}

#[test]
fn compact_layout_keeps_tasks_timer_and_core_registry_hints_visible() {
    let directory = tempdir().unwrap();
    let todo = directory.path().join("TODO.md");
    fs::write(
        &todo,
        "# Study\n- [ ] Compact task <!-- focus:id=compact -->\n",
    )
    .unwrap();
    let repository = MarkdownTaskRepository::new(todo);
    let store = FocusStore::open(&directory.path().join("focus.db")).unwrap();
    let mut app = UiApp::new(repository, store, AppConfig::default(), None).unwrap();

    let screen = render_app_at(&mut app, 50, 14);

    assert!(screen.contains("Compact task"));
    assert!(screen.contains("READY"));
    assert!(screen.contains(": commands"));
    assert!(screen.contains("? help"));
    assert!(!screen.contains("1 tasks · open"));
}

#[test]
fn normal_layout_shows_task_count_status_and_wrapped_registry_footer() {
    let directory = tempdir().unwrap();
    let todo = directory.path().join("TODO.md");
    fs::write(
        &todo,
        "# Study\n- [ ] Normal task <!-- focus:id=normal -->\n",
    )
    .unwrap();
    let repository = MarkdownTaskRepository::new(todo);
    let store = FocusStore::open(&directory.path().join("focus.db")).unwrap();
    let mut app = UiApp::new(repository, store, AppConfig::default(), None).unwrap();

    let screen = render_app_at(&mut app, 90, 24);

    assert!(screen.contains("1 tasks · open"));
    assert!(screen.contains("focus —"));
    assert!(screen.contains("enter focus/next"));
    assert!(screen.contains("e edit"));
}

#[test]
fn main_view_uses_the_entire_popup_content_area_without_its_own_outer_frame() {
    let directory = tempdir().unwrap();
    let todo = directory.path().join("TODO.md");
    fs::write(&todo, "# Study\n- [ ] Full task <!-- focus:id=full -->\n").unwrap();
    let repository = MarkdownTaskRepository::new(todo);
    let store = FocusStore::open(&directory.path().join("focus.db")).unwrap();
    let mut app = UiApp::new(repository, store, AppConfig::default(), None).unwrap();

    let backend = TestBackend::new(130, 36);
    let mut terminal = Terminal::new(backend).unwrap();
    terminal.draw(|frame| app.render(frame)).unwrap();
    let buffer = terminal.backend().buffer();
    let screen = screen_text(buffer, 130, 36);
    let first_row = screen.lines().next().unwrap();

    assert!(first_row.starts_with("/ search tasks"));
    assert!(!screen.contains('┌'));
}

#[test]
fn wide_layout_and_small_overlays_render_without_exceeding_the_terminal() {
    let directory = tempdir().unwrap();
    let todo = directory.path().join("TODO.md");
    fs::write(&todo, "# Study\n- [ ] Wide task <!-- focus:id=wide -->\n").unwrap();
    let repository = MarkdownTaskRepository::new(todo);
    let store = FocusStore::open(&directory.path().join("focus.db")).unwrap();
    let mut app = UiApp::new(repository, store, AppConfig::default(), None).unwrap();

    let wide = render_app_at(&mut app, 130, 36);
    assert!(wide.contains("Wide task"));
    assert!(wide.contains("E estimate"));

    app.handle_key(KeyEvent::new(KeyCode::Char(':'), KeyModifiers::NONE))
        .unwrap();
    let compact_palette = render_app_at(&mut app, 42, 12);
    assert!(compact_palette.contains("Command palette"));
    app.handle_key(KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE))
        .unwrap();
    app.handle_key(KeyEvent::new(KeyCode::Char('n'), KeyModifiers::NONE))
        .unwrap();
    let compact_input = render_app_at(&mut app, 32, 10);
    assert!(compact_input.contains("Add a Markdown task"));
}

fn render_app_at(app: &mut UiApp, width: u16, height: u16) -> String {
    let backend = TestBackend::new(width, height);
    let mut terminal = Terminal::new(backend).unwrap();
    terminal.draw(|frame| app.render(frame)).unwrap();
    screen_text(terminal.backend().buffer(), width, height)
}

fn type_text(app: &mut UiApp, text: &str) {
    for character in text.chars() {
        app.handle_key(KeyEvent::new(KeyCode::Char(character), KeyModifiers::NONE))
            .unwrap();
    }
}

fn screen_text(buffer: &ratatui::buffer::Buffer, width: u16, height: u16) -> String {
    (0..height)
        .map(|y| row_text(buffer, width, y))
        .collect::<Vec<_>>()
        .join("\n")
}

fn row_text(buffer: &ratatui::buffer::Buffer, width: u16, y: u16) -> String {
    (0..width)
        .map(|x| buffer.cell((x, y)).unwrap().symbol())
        .collect::<String>()
}
