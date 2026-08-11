use focus_pane::{
    config::default_data_dir,
    markdown::{IndentDirection, MarkdownTaskRepository, MoveDirection, resolve_task_file},
    models::TaskPriority,
};
use std::{
    collections::HashSet,
    fs,
    sync::{Arc, Barrier},
    thread,
};
use tempfile::tempdir;

#[test]
fn reads_python_markdown_format_with_headings_indentation_and_ids() {
    let directory = tempdir().unwrap();
    let path = directory.path().join("TODO.md");
    fs::write(
        &path,
        "# Today\n\n- [ ] Write parser\n  * [X] Add tests <!-- focus:id=known-id -->\n",
    )
    .unwrap();

    let tasks = MarkdownTaskRepository::new(path).read_tasks().unwrap();

    assert_eq!(tasks.len(), 2);
    assert_eq!(tasks[0].text, "Write parser");
    assert!(!tasks[0].checked);
    assert_eq!(tasks[0].heading.as_deref(), Some("Today"));
    assert_eq!(tasks[1].text, "Add tests");
    assert!(tasks[1].checked);
    assert_eq!(tasks[1].task_id.as_deref(), Some("known-id"));
    assert_eq!(tasks[1].indent, 2);
}

#[test]
fn parses_rich_focus_metadata_and_standard_hashtags_without_showing_comments() {
    let directory = tempdir().unwrap();
    let path = directory.path().join("TODO.md");
    fs::write(
        &path,
        "- [ ] Read chapter #study #course/distsys \
         <!-- focus:estimate=3 status=blocked priority=high due=2026-04-01 unknown=keep --> \
         <!-- focus:id=rich-task -->\n",
    )
    .unwrap();

    let task = MarkdownTaskRepository::new(path)
        .read_tasks()
        .unwrap()
        .remove(0);

    assert_eq!(task.text, "Read chapter #study #course/distsys");
    assert_eq!(task.task_id.as_deref(), Some("rich-task"));
    assert_eq!(task.metadata.tags, ["study", "course/distsys"]);
    assert_eq!(task.metadata.priority, Some(TaskPriority::High));
    assert_eq!(
        task.metadata.due_date.map(|date| date.to_string()),
        Some("2026-04-01".into())
    );
    assert_eq!(task.metadata.estimate_pomodoros, Some(3));
    assert!(task.metadata.blocked);
}

#[test]
fn toggles_only_the_checkbox_and_keeps_python_hidden_id_format() {
    let directory = tempdir().unwrap();
    let path = directory.path().join("TODO.md");
    let original = "- [ ] Keep **all** formatting <!-- focus:id=abc -->\n";
    fs::write(&path, original).unwrap();
    let repository = MarkdownTaskRepository::new(path.clone());

    let updated = repository
        .toggle(&repository.read_tasks().unwrap()[0])
        .unwrap();

    assert!(updated.checked);
    assert_eq!(
        fs::read_to_string(path).unwrap(),
        original.replace("[ ]", "[x]")
    );
}

#[test]
fn deletes_only_the_selected_markdown_task_line() {
    let directory = tempdir().unwrap();
    let path = directory.path().join("TODO.md");
    fs::write(
        &path,
        "# Tasks\n- [ ] Keep me <!-- focus:id=keep -->\n- [ ] Delete me <!-- focus:id=delete -->\n",
    )
    .unwrap();
    let repository = MarkdownTaskRepository::new(path.clone());
    let task = repository
        .read_tasks()
        .unwrap()
        .into_iter()
        .find(|task| task.task_id.as_deref() == Some("delete"))
        .unwrap();

    repository.delete(&task).unwrap();

    let markdown = fs::read_to_string(path).unwrap();
    assert!(markdown.contains("Keep me"));
    assert!(!markdown.contains("Delete me"));
}

#[test]
fn hidden_id_follows_a_task_when_markdown_lines_move() {
    let directory = tempdir().unwrap();
    let path = directory.path().join("TODO.md");
    fs::write(&path, "# Tasks\n- [ ] Move me\n").unwrap();
    let repository = MarkdownTaskRepository::new(path.clone());

    let identified = repository
        .ensure_id(&repository.read_tasks().unwrap()[0])
        .unwrap();
    fs::write(
        &path,
        format!(
            "# Tasks\nSome note\n- [ ] Move me <!-- focus:id={} -->\n",
            identified.task_id.as_deref().unwrap()
        ),
    )
    .unwrap();
    let toggled = repository.toggle(&identified).unwrap();

    assert!(toggled.checked);
    assert_eq!(toggled.line_number, 3);
}

#[test]
fn estimate_updates_preserve_unknown_metadata_and_keep_the_id_last() {
    let directory = tempdir().unwrap();
    let path = directory.path().join("TODO.md");
    fs::write(
        &path,
        "- [ ] Task <!-- focus:custom=keep estimate=2 --> <!-- focus:id=task -->\n",
    )
    .unwrap();
    let repository = MarkdownTaskRepository::new(path.clone());
    let task = repository.read_tasks().unwrap().remove(0);

    let estimated = repository.set_estimate(&task, Some(4)).unwrap();

    assert_eq!(estimated.metadata.estimate_pomodoros, Some(4));
    let markdown = fs::read_to_string(&path).unwrap();
    assert!(markdown.contains("custom=keep"));
    assert!(!markdown.contains("estimate=2"));
    assert_eq!(markdown.matches("estimate=4").count(), 1);
    assert!(markdown.trim_end().ends_with("<!-- focus:id=task -->"));

    let cleared = repository.set_estimate(&estimated, None).unwrap();
    assert_eq!(cleared.metadata.estimate_pomodoros, None);
    assert!(!fs::read_to_string(path).unwrap().contains("estimate="));
}

#[test]
fn blocked_status_can_be_toggled_without_changing_the_checkbox() {
    let directory = tempdir().unwrap();
    let path = directory.path().join("TODO.md");
    fs::write(&path, "- [ ] Task <!-- focus:id=task -->\n").unwrap();
    let repository = MarkdownTaskRepository::new(path.clone());
    let task = repository.read_tasks().unwrap().remove(0);

    let blocked = repository.toggle_blocked(&task).unwrap();

    assert!(blocked.metadata.blocked);
    assert!(!blocked.checked);
    assert!(
        fs::read_to_string(&path)
            .unwrap()
            .contains("status=blocked")
    );

    let unblocked = repository.toggle_blocked(&blocked).unwrap();
    assert!(!unblocked.metadata.blocked);
    let markdown = fs::read_to_string(path).unwrap();
    assert!(!markdown.contains("status="));
    assert!(markdown.contains("- [ ] Task"));
}

#[test]
fn editing_text_preserves_checkbox_format_and_unknown_focus_metadata() {
    let directory = tempdir().unwrap();
    let path = directory.path().join("TODO.md");
    fs::write(
        &path,
        "*  [ ]  Old text <!-- focus:estimate=2 custom=keep --> <!-- focus:id=edit -->\n",
    )
    .unwrap();
    let repository = MarkdownTaskRepository::new(path.clone());
    let task = repository.read_tasks().unwrap().remove(0);

    let edited = repository.edit(&task, "New text #updated").unwrap();

    assert_eq!(edited.text, "New text #updated");
    assert_eq!(edited.metadata.tags, ["updated"]);
    assert_eq!(edited.metadata.estimate_pomodoros, Some(2));
    assert_eq!(
        fs::read_to_string(path).unwrap(),
        "*  [ ]  New text #updated <!-- focus:estimate=2 custom=keep --> <!-- focus:id=edit -->\n"
    );
}

#[test]
fn inserting_after_a_parent_places_the_new_sibling_after_its_subtree() {
    let directory = tempdir().unwrap();
    let path = directory.path().join("TODO.md");
    fs::write(
        &path,
        "- [ ] Parent <!-- focus:id=parent -->\n  - [ ] Child <!-- focus:id=child -->\n- [ ] Sibling <!-- focus:id=sibling -->\n",
    )
    .unwrap();
    let repository = MarkdownTaskRepository::new(path.clone());
    let parent = repository.read_tasks().unwrap().remove(0);

    let inserted = repository
        .insert_after(&parent, "Inserted sibling")
        .unwrap();

    assert_eq!(inserted.indent, 0);
    let markdown = fs::read_to_string(path).unwrap();
    assert!(
        markdown.find("Child").unwrap() < markdown.find("Inserted sibling").unwrap()
            && markdown.find("Inserted sibling").unwrap() < markdown.find("Sibling").unwrap()
    );
}

#[test]
fn moving_a_task_moves_its_complete_subtree_without_crossing_headings() {
    let directory = tempdir().unwrap();
    let path = directory.path().join("TODO.md");
    fs::write(
        &path,
        "# One\n- [ ] Parent <!-- focus:id=parent -->\n  - [ ] Child <!-- focus:id=child -->\n- [ ] Sibling <!-- focus:id=sibling -->\n# Two\n- [ ] Other <!-- focus:id=other -->\n",
    )
    .unwrap();
    let repository = MarkdownTaskRepository::new(path.clone());
    let parent = repository.read_tasks().unwrap().remove(0);

    let moved = repository
        .move_subtree(&parent, MoveDirection::Down)
        .unwrap()
        .unwrap();

    let markdown = fs::read_to_string(&path).unwrap();
    assert!(markdown.find("Sibling").unwrap() < markdown.find("Parent").unwrap());
    assert!(markdown.find("Parent").unwrap() < markdown.find("Child").unwrap());
    assert!(markdown.find("Child").unwrap() < markdown.find("# Two").unwrap());
    assert!(
        repository
            .move_subtree(&moved, MoveDirection::Down)
            .unwrap()
            .is_none()
    );
}

#[test]
fn indenting_and_outdenting_moves_the_complete_subtree() {
    let directory = tempdir().unwrap();
    let path = directory.path().join("TODO.md");
    let original = "- [ ] First <!-- focus:id=first -->\n- [ ] Parent <!-- focus:id=parent -->\n  - [ ] Child <!-- focus:id=child -->\n";
    fs::write(&path, original).unwrap();
    let repository = MarkdownTaskRepository::new(path.clone());
    let parent = repository.read_tasks().unwrap().remove(1);

    let indented = repository
        .change_indent(&parent, IndentDirection::Increase)
        .unwrap()
        .unwrap();
    let markdown = fs::read_to_string(&path).unwrap();
    assert!(markdown.contains("  - [ ] Parent"));
    assert!(markdown.contains("    - [ ] Child"));

    repository
        .change_indent(&indented, IndentDirection::Decrease)
        .unwrap()
        .unwrap();
    assert_eq!(fs::read_to_string(path).unwrap(), original);
}

#[test]
fn stable_ids_are_matched_exactly_when_one_is_a_prefix_of_another() {
    let directory = tempdir().unwrap();
    let path = directory.path().join("TODO.md");
    fs::write(
        &path,
        "# Tasks\n- [ ] Short ID <!-- focus:id=task -->\n- [ ] Long ID <!-- focus:id=task-long -->\n",
    )
    .unwrap();
    let repository = MarkdownTaskRepository::new(path.clone());
    let task = repository.read_tasks().unwrap().remove(0);

    repository.toggle(&task).unwrap();

    let markdown = fs::read_to_string(path).unwrap();
    assert!(markdown.contains("- [x] Short ID"));
    assert!(markdown.contains("- [ ] Long ID"));
}

#[test]
fn refuses_to_mutate_a_task_that_changed_externally() {
    let directory = tempdir().unwrap();
    let path = directory.path().join("TODO.md");
    fs::write(
        &path,
        "# Tasks\n- [ ] Original text <!-- focus:id=task-1 -->\n",
    )
    .unwrap();
    let repository = MarkdownTaskRepository::new(path.clone());
    let stale_task = repository.read_tasks().unwrap().remove(0);
    fs::write(
        &path,
        "# Tasks\n- [ ] Externally edited <!-- focus:id=task-1 -->\n",
    )
    .unwrap();

    let result = repository.toggle(&stale_task);

    assert!(result.is_err());
    let markdown = fs::read_to_string(path).unwrap();
    assert!(markdown.contains("- [ ] Externally edited"));
    assert!(!markdown.contains("- [x]"));
}

#[test]
fn concurrent_adds_preserve_every_task() {
    let directory = tempdir().unwrap();
    let path = directory.path().join("TODO.md");
    fs::write(&path, "# Tasks\n\n").unwrap();
    let writer_count = 16;
    let barrier = Arc::new(Barrier::new(writer_count));
    let writers = (0..writer_count)
        .map(|index| {
            let path = path.clone();
            let barrier = Arc::clone(&barrier);
            thread::spawn(move || {
                let repository = MarkdownTaskRepository::new(path);
                barrier.wait();
                repository.add(&format!("Concurrent task {index}")).unwrap();
            })
        })
        .collect::<Vec<_>>();
    for writer in writers {
        writer.join().unwrap();
    }

    let tasks = MarkdownTaskRepository::new(path).read_tasks().unwrap();
    let texts = tasks
        .iter()
        .map(|task| task.text.as_str())
        .collect::<HashSet<_>>();
    assert_eq!(tasks.len(), writer_count);
    assert_eq!(texts.len(), writer_count);
}

#[test]
fn adds_a_stable_task_and_resolves_relative_task_files_in_the_app_data_directory() {
    let directory = tempdir().unwrap();
    let project = directory.path().join("project");
    fs::create_dir_all(&project).unwrap();
    let path = project.join("TODO.md");
    let repository = MarkdownTaskRepository::new(path.clone());

    let task = repository.add("Learn Ratatui").unwrap();

    assert_eq!(task.text, "Learn Ratatui");
    assert!(task.task_id.is_some());
    assert_eq!(
        resolve_task_file("TODO.md"),
        default_data_dir().join("TODO.md")
    );
    assert!(
        fs::read_to_string(path)
            .unwrap()
            .contains("- [ ] Learn Ratatui <!-- focus:id=")
    );
}
