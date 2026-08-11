use crate::config::AppConfig;
use anyhow::{Context, Result};
use glob::glob;
use std::{
    collections::HashSet,
    env, fs,
    path::{Path, PathBuf},
};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WorkspaceDefinition {
    pub name: String,
    pub files: Vec<PathBuf>,
    pub warnings: Vec<String>,
}

impl WorkspaceDefinition {
    pub fn single(name: impl Into<String>, file: PathBuf) -> Self {
        Self {
            name: name.into(),
            files: vec![absolute_path(file)],
            warnings: Vec::new(),
        }
    }
}

pub fn resolve_workspaces(
    config: &AppConfig,
    config_path: &Path,
    fallback_file: PathBuf,
) -> Result<Vec<WorkspaceDefinition>> {
    if config.workspaces.is_empty() {
        let name = fallback_file
            .parent()
            .and_then(Path::file_name)
            .and_then(|name| name.to_str())
            .filter(|name| !name.is_empty())
            .unwrap_or("Current")
            .to_owned();
        return Ok(vec![WorkspaceDefinition::single(name, fallback_file)]);
    }

    let base = config_path.parent().unwrap_or_else(|| Path::new("."));
    config
        .workspaces
        .iter()
        .map(|workspace| {
            let mut files = Vec::new();
            let mut warnings = Vec::new();
            let mut seen = HashSet::new();
            for configured in &workspace.files {
                let expanded = resolve_configured_path(configured, base);
                if contains_glob(configured) {
                    let pattern = expanded.to_string_lossy();
                    let mut matched = 0;
                    for entry in glob(&pattern)
                        .with_context(|| format!("invalid workspace glob {configured:?}"))?
                    {
                        match entry {
                            Ok(path) if path.is_file() => {
                                let path = absolute_path(path);
                                if seen.insert(path.clone()) {
                                    files.push(path);
                                }
                                matched += 1;
                            }
                            Ok(path) => warnings.push(format!(
                                "glob {configured:?} ignored non-file {}",
                                path.display()
                            )),
                            Err(error) => warnings.push(format!(
                                "glob {configured:?} could not read an entry: {error}"
                            )),
                        }
                    }
                    if matched == 0 {
                        warnings.push(format!("glob {configured:?} matched no Markdown files"));
                    }
                } else {
                    let path = absolute_path(expanded);
                    if seen.insert(path.clone()) {
                        files.push(path);
                    }
                }
            }
            files.sort();
            Ok(WorkspaceDefinition {
                name: workspace.name.clone(),
                files,
                warnings,
            })
        })
        .collect()
}

pub fn initial_workspace_index(workspaces: &[WorkspaceDefinition], cwd: &Path) -> usize {
    let cwd = fs::canonicalize(cwd).unwrap_or_else(|_| absolute_path(cwd.to_path_buf()));
    workspaces
        .iter()
        .enumerate()
        .filter_map(|(index, workspace)| {
            workspace
                .files
                .iter()
                .filter_map(|file| file.parent())
                .filter(|parent| cwd.starts_with(parent))
                .map(|parent| parent.components().count())
                .max()
                .map(|depth| (index, depth))
        })
        .max_by_key(|(_, depth)| *depth)
        .map_or(0, |(index, _)| index)
}

fn resolve_configured_path(configured: &str, base: &Path) -> PathBuf {
    let expanded = expand_home(configured);
    if expanded.is_absolute() {
        expanded
    } else {
        base.join(expanded)
    }
}

fn expand_home(path: &str) -> PathBuf {
    if path == "~" {
        return env::var_os("HOME")
            .map(PathBuf::from)
            .unwrap_or_else(|| PathBuf::from(path));
    }
    if let Some(suffix) = path.strip_prefix("~/")
        && let Some(home) = env::var_os("HOME")
    {
        return PathBuf::from(home).join(suffix);
    }
    PathBuf::from(path)
}

fn contains_glob(path: &str) -> bool {
    path.contains(['*', '?', '['])
}

fn absolute_path(path: PathBuf) -> PathBuf {
    fs::canonicalize(&path).unwrap_or_else(|_| {
        if path.is_absolute() {
            path
        } else {
            env::current_dir().unwrap_or_default().join(path)
        }
    })
}
