use anyhow::{Result, bail};
use serde::Deserialize;
use std::{collections::HashSet, env, fs, path::Path, path::PathBuf};

#[derive(Debug, Clone, PartialEq)]
pub struct FocusPreset {
    pub name: String,
    pub focus_minutes: f64,
    pub break_minutes: f64,
    pub long_break_minutes: f64,
    pub cycles_before_long_break: u32,
}

impl FocusPreset {
    pub fn focus_seconds(&self) -> u64 {
        (self.focus_minutes * 60.0).round() as u64
    }

    pub fn break_seconds(&self) -> u64 {
        (self.break_minutes * 60.0).round() as u64
    }

    pub fn long_break_seconds(&self) -> u64 {
        (self.long_break_minutes * 60.0).round() as u64
    }

    fn validate(&self) -> Result<()> {
        if self.name.trim().is_empty() {
            bail!("timer preset name must not be empty");
        }
        if self.focus_minutes <= 0.0 || self.break_minutes <= 0.0 || self.long_break_minutes <= 0.0
        {
            bail!("timer preset durations must be greater than zero");
        }
        if self.cycles_before_long_break == 0 {
            bail!("timer preset cycles_before_long_break must be at least one");
        }
        Ok(())
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct WorkspaceConfig {
    pub name: String,
    pub files: Vec<String>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct AppConfig {
    pub task_file: String,
    pub focus_minutes: f64,
    pub break_minutes: f64,
    pub long_break_minutes: f64,
    pub cycles_before_long_break: u32,
    pub auto_start_break: bool,
    pub auto_start_focus: bool,
    pub default_preset: String,
    pub presets: Vec<FocusPreset>,
    pub workspaces: Vec<WorkspaceConfig>,
}

impl Default for AppConfig {
    fn default() -> Self {
        Self {
            task_file: "TODO.md".into(),
            focus_minutes: 25.0,
            break_minutes: 5.0,
            long_break_minutes: 15.0,
            cycles_before_long_break: 4,
            auto_start_break: true,
            auto_start_focus: true,
            default_preset: "standard".into(),
            presets: Vec::new(),
            workspaces: Vec::new(),
        }
    }
}

impl AppConfig {
    pub fn focus_seconds(&self) -> u64 {
        (self.focus_minutes * 60.0).round() as u64
    }

    pub fn break_seconds(&self) -> u64 {
        (self.break_minutes * 60.0).round() as u64
    }

    pub fn long_break_seconds(&self) -> u64 {
        (self.long_break_minutes * 60.0).round() as u64
    }

    pub fn available_presets(&self) -> Vec<FocusPreset> {
        if self.presets.is_empty() {
            vec![self.legacy_preset()]
        } else {
            self.presets.clone()
        }
    }

    pub fn preset(&self, name: &str) -> Option<FocusPreset> {
        if self.presets.is_empty() {
            let preset = self.legacy_preset();
            (preset.name == name).then_some(preset)
        } else {
            self.presets
                .iter()
                .find(|preset| preset.name == name)
                .cloned()
        }
    }

    pub fn default_focus_preset(&self) -> FocusPreset {
        self.preset(&self.default_preset)
            .unwrap_or_else(|| self.available_presets().remove(0))
    }

    fn legacy_preset(&self) -> FocusPreset {
        FocusPreset {
            name: self.default_preset.clone(),
            focus_minutes: self.focus_minutes,
            break_minutes: self.break_minutes,
            long_break_minutes: self.long_break_minutes,
            cycles_before_long_break: self.cycles_before_long_break,
        }
    }

    fn validate(&self) -> Result<()> {
        if self.task_file.trim().is_empty() {
            bail!("tasks.file must not be empty");
        }
        if self.focus_minutes <= 0.0 {
            bail!("timer.focus_minutes must be greater than zero");
        }
        if self.break_minutes <= 0.0 || self.long_break_minutes <= 0.0 {
            bail!("break durations must be greater than zero");
        }
        if self.cycles_before_long_break == 0 {
            bail!("timer.cycles_before_long_break must be at least one");
        }
        let mut names = HashSet::new();
        for preset in &self.presets {
            preset.validate()?;
            if !names.insert(preset.name.as_str()) {
                bail!("duplicate timer preset {:?}", preset.name);
            }
        }
        if !self.presets.is_empty() && !names.contains(self.default_preset.as_str()) {
            bail!(
                "timer.default_preset {:?} does not match a configured preset",
                self.default_preset
            );
        }
        let mut workspace_names = HashSet::new();
        for workspace in &self.workspaces {
            if workspace.name.trim().is_empty() {
                bail!("workspace name must not be empty");
            }
            if workspace.files.is_empty() {
                bail!(
                    "workspace {:?} must contain at least one file or glob",
                    workspace.name
                );
            }
            if !workspace_names.insert(workspace.name.as_str()) {
                bail!("duplicate workspace {:?}", workspace.name);
            }
            if workspace.files.iter().any(|file| file.trim().is_empty()) {
                bail!(
                    "workspace {:?} contains an empty file pattern",
                    workspace.name
                );
            }
        }
        Ok(())
    }
}

#[derive(Debug, Default, Deserialize)]
struct ConfigFile {
    #[serde(default)]
    tasks: TasksConfig,
    #[serde(default)]
    timer: TimerConfig,
    #[serde(default)]
    workspaces: Vec<WorkspaceConfig>,
}

#[derive(Debug, Default, Deserialize)]
struct TasksConfig {
    file: Option<String>,
}

#[derive(Debug, Default, Deserialize)]
struct TimerConfig {
    focus_minutes: Option<f64>,
    break_minutes: Option<f64>,
    long_break_minutes: Option<f64>,
    cycles_before_long_break: Option<u32>,
    auto_start_break: Option<bool>,
    auto_start_focus: Option<bool>,
    default_preset: Option<String>,
    #[serde(default)]
    presets: Vec<PresetConfig>,
}

#[derive(Debug, Deserialize)]
struct PresetConfig {
    name: String,
    focus_minutes: f64,
    break_minutes: f64,
    long_break_minutes: f64,
    cycles_before_long_break: u32,
}

impl From<PresetConfig> for FocusPreset {
    fn from(raw: PresetConfig) -> Self {
        Self {
            name: raw.name,
            focus_minutes: raw.focus_minutes,
            break_minutes: raw.break_minutes,
            long_break_minutes: raw.long_break_minutes,
            cycles_before_long_break: raw.cycles_before_long_break,
        }
    }
}

pub fn load_config(path: &Path) -> Result<AppConfig> {
    if !path.exists() {
        return Ok(AppConfig::default());
    }
    let source = fs::read_to_string(path)?;
    let raw: ConfigFile = toml::from_str(&source)?;
    let defaults = AppConfig::default();
    let presets = raw
        .timer
        .presets
        .into_iter()
        .map(FocusPreset::from)
        .collect::<Vec<_>>();
    let default_preset = raw.timer.default_preset.unwrap_or_else(|| {
        presets
            .first()
            .map(|preset| preset.name.clone())
            .unwrap_or_else(|| defaults.default_preset.clone())
    });
    let config = AppConfig {
        task_file: raw.tasks.file.unwrap_or(defaults.task_file),
        focus_minutes: raw.timer.focus_minutes.unwrap_or(defaults.focus_minutes),
        break_minutes: raw.timer.break_minutes.unwrap_or(defaults.break_minutes),
        long_break_minutes: raw
            .timer
            .long_break_minutes
            .unwrap_or(defaults.long_break_minutes),
        cycles_before_long_break: raw
            .timer
            .cycles_before_long_break
            .unwrap_or(defaults.cycles_before_long_break),
        auto_start_break: raw
            .timer
            .auto_start_break
            .unwrap_or(defaults.auto_start_break),
        auto_start_focus: raw
            .timer
            .auto_start_focus
            .unwrap_or(defaults.auto_start_focus),
        default_preset,
        presets,
        workspaces: raw.workspaces,
    };
    config.validate()?;
    Ok(config)
}

pub const DEFAULT_CONFIG: &str = r#"# Focus Pane configuration

# Relative task paths are stored under the Focus Pane data directory.
[tasks]
file = "TODO.md"

[timer]
focus_minutes = 25
break_minutes = 5
long_break_minutes = 15
cycles_before_long_break = 4
auto_start_break = true
auto_start_focus = true
default_preset = "standard"

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

# Optional named workspaces. Paths are relative to this config file;
# explicit missing files remain visible and globs are expanded at startup.
# [[workspaces]]
# name = "Study"
# files = ["~/notes/TODO.md"]
#
# [[workspaces]]
# name = "Projects"
# files = ["~/code/*/TODO.md"]
"#;

pub fn ensure_default_config(path: &Path) -> Result<()> {
    if path.exists() {
        return Ok(());
    }
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    fs::write(path, DEFAULT_CONFIG)?;
    Ok(())
}

pub fn default_config_path() -> PathBuf {
    xdg_path("XDG_CONFIG_HOME", ".config").join("focus-pane/config.toml")
}

pub fn default_data_dir() -> PathBuf {
    xdg_path("XDG_DATA_HOME", ".local/share").join("focus-pane")
}

pub fn default_db_path() -> PathBuf {
    default_data_dir().join("focus.db")
}

fn xdg_path(variable: &str, fallback: &str) -> PathBuf {
    env::var_os(variable).map(PathBuf::from).unwrap_or_else(|| {
        let home = env::var_os("HOME").map(PathBuf::from).unwrap_or_default();
        home.join(fallback)
    })
}
