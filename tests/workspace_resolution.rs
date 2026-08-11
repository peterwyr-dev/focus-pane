use focus_pane::{
    config::{AppConfig, WorkspaceConfig},
    workspace::{initial_workspace_index, resolve_workspaces},
};
use std::fs;
use tempfile::tempdir;

#[test]
fn resolves_explicit_files_globs_missing_sources_and_origin_workspace() {
    let directory = tempdir().unwrap();
    let config_path = directory.path().join("config/focus-pane.toml");
    let study = directory.path().join("study");
    let projects = directory.path().join("projects");
    fs::create_dir_all(config_path.parent().unwrap()).unwrap();
    fs::create_dir_all(&study).unwrap();
    fs::create_dir_all(projects.join("one")).unwrap();
    fs::create_dir_all(projects.join("two/src")).unwrap();
    fs::write(study.join("TODO.md"), "# Study\n").unwrap();
    fs::write(projects.join("one/TODO.md"), "# One\n").unwrap();
    fs::write(projects.join("two/TODO.md"), "# Two\n").unwrap();
    let config = AppConfig {
        workspaces: vec![
            WorkspaceConfig {
                name: "Study".into(),
                files: vec!["../study/TODO.md".into(), "../study/MISSING.md".into()],
            },
            WorkspaceConfig {
                name: "Projects".into(),
                files: vec!["../projects/*/TODO.md".into()],
            },
        ],
        ..AppConfig::default()
    };

    let workspaces = resolve_workspaces(
        &config,
        &config_path,
        directory.path().join("fallback/TODO.md"),
    )
    .unwrap();

    assert_eq!(workspaces.len(), 2);
    assert_eq!(workspaces[0].files.len(), 2);
    assert!(
        workspaces[0]
            .files
            .iter()
            .any(|path| path.ends_with("MISSING.md"))
    );
    assert_eq!(workspaces[1].files.len(), 2);
    assert_eq!(
        initial_workspace_index(&workspaces, &projects.join("two/src")),
        1
    );
}

#[test]
fn zero_config_keeps_the_resolved_task_file_as_a_single_workspace() {
    let directory = tempdir().unwrap();
    let fallback = directory.path().join("project/TODO.md");

    let workspaces = resolve_workspaces(
        &AppConfig::default(),
        &directory.path().join("config.toml"),
        fallback.clone(),
    )
    .unwrap();

    assert_eq!(workspaces.len(), 1);
    assert_eq!(workspaces[0].name, "project");
    assert_eq!(workspaces[0].files, [fallback]);
}
