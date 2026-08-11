use focus_pane::config::{AppConfig, load_config};
use std::fs;
use tempfile::tempdir;

#[test]
fn python_configuration_is_compatible_and_auto_cycles_by_default() {
    let directory = tempdir().unwrap();
    let path = directory.path().join("config.toml");
    fs::write(
        &path,
        r#"
[tasks]
file = "STUDY.md"

[timer]
focus_minutes = 40
break_minutes = 8
long_break_minutes = 20
cycles_before_long_break = 3
auto_start_break = true
auto_start_focus = true
"#,
    )
    .unwrap();

    let config = load_config(&path).unwrap();

    assert_eq!(config.task_file, "STUDY.md");
    assert_eq!(config.focus_seconds(), 2400);
    assert_eq!(config.break_seconds(), 480);
    assert_eq!(config.long_break_seconds(), 1200);
    assert_eq!(config.cycles_before_long_break, 3);
    assert!(config.auto_start_break);
    assert!(config.auto_start_focus);

    let defaults = AppConfig::default();
    assert!(defaults.auto_start_break);
    assert!(defaults.auto_start_focus);
}

#[test]
fn named_workspaces_load_explicit_files_and_glob_patterns() {
    let directory = tempdir().unwrap();
    let path = directory.path().join("config.toml");
    fs::write(
        &path,
        r#"
[[workspaces]]
name = "Study"
files = ["~/notes/TODO.md"]

[[workspaces]]
name = "Projects"
files = ["~/code/*/TODO.md"]
"#,
    )
    .unwrap();

    let config = load_config(&path).unwrap();

    assert_eq!(config.workspaces.len(), 2);
    assert_eq!(config.workspaces[0].name, "Study");
    assert_eq!(config.workspaces[1].files, ["~/code/*/TODO.md"]);
}

#[test]
fn named_focus_presets_are_loaded_and_the_default_is_selected() {
    let directory = tempdir().unwrap();
    let path = directory.path().join("config.toml");
    fs::write(
        &path,
        r#"
[timer]
default_preset = "deep"
auto_start_break = false
auto_start_focus = false

[[timer.presets]]
name = "standard"
focus_minutes = 25
break_minutes = 5
long_break_minutes = 15
cycles_before_long_break = 4

[[timer.presets]]
name = "deep"
focus_minutes = 50
break_minutes = 10
long_break_minutes = 20
cycles_before_long_break = 3
"#,
    )
    .unwrap();

    let config = load_config(&path).unwrap();
    let preset = config.default_focus_preset();

    assert_eq!(config.available_presets().len(), 2);
    assert_eq!(preset.name, "deep");
    assert_eq!(preset.focus_seconds(), 3000);
    assert_eq!(preset.break_seconds(), 600);
    assert_eq!(preset.long_break_seconds(), 1200);
    assert_eq!(preset.cycles_before_long_break, 3);
    assert!(!config.auto_start_break);
    assert!(!config.auto_start_focus);
}
