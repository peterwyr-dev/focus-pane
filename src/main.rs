use anyhow::Result;
use clap::{Parser, Subcommand};
use focus_pane::{
    config::{default_config_path, default_db_path, ensure_default_config, load_config},
    markdown::resolve_task_file,
    store::FocusStore,
    ui::run_workspace_ui,
    worker::run_worker,
    workspace::{WorkspaceDefinition, initial_workspace_index, resolve_workspaces},
};
use std::{env, path::PathBuf};

#[derive(Debug, Parser)]
#[command(
    name = "focus-pane",
    version,
    about = "Markdown tasks and Pomodoro timing in the terminal"
)]
struct Cli {
    /// Originating directory used to select the initial workspace.
    #[arg(long)]
    cwd: Option<PathBuf>,
    /// Explicit Markdown task file.
    #[arg(long)]
    file: Option<PathBuf>,
    /// Optional originating Herdr pane ID for sidebar metadata.
    #[arg(long)]
    pane: Option<String>,
    /// Focus Pane TOML configuration.
    #[arg(long)]
    config: Option<PathBuf>,
    /// SQLite state database.
    #[arg(long)]
    db: Option<PathBuf>,
    #[command(subcommand)]
    command: Option<Command>,
}

#[derive(Debug, Subcommand)]
enum Command {
    #[command(hide = true)]
    Worker {
        #[arg(long)]
        timer_id: String,
        #[arg(long)]
        db: PathBuf,
        #[arg(long)]
        config: PathBuf,
    },
    /// Create the default config and print an optional Herdr popup snippet.
    Setup,
}

fn main() -> Result<()> {
    let cli = Cli::parse();
    match cli.command {
        Some(Command::Worker {
            timer_id,
            db,
            config,
        }) => {
            let config_value = load_config(&config)?;
            run_worker(&db, &config_value, &timer_id)
        }
        Some(Command::Setup) => setup(),
        None => run_app(cli),
    }
}

fn run_app(cli: Cli) -> Result<()> {
    let config_path = cli.config.unwrap_or_else(default_config_path);
    let config = load_config(&config_path)?;
    let db_path = cli.db.unwrap_or_else(default_db_path);
    let cwd = cli
        .cwd
        .or_else(|| env::var_os("HERDR_ACTIVE_PANE_CWD").map(PathBuf::from))
        .unwrap_or(env::current_dir()?);
    let explicit_file = cli.file;
    let fallback_file = explicit_file
        .clone()
        .unwrap_or_else(|| resolve_task_file(&config.task_file));
    let workspaces = if let Some(file) = explicit_file {
        vec![WorkspaceDefinition::single("Explicit file", file)]
    } else {
        resolve_workspaces(&config, &config_path, fallback_file)?
    };
    let initial_workspace = initial_workspace_index(&workspaces, &cwd);
    let pane_id = cli.pane.or_else(|| env::var("HERDR_ACTIVE_PANE_ID").ok());
    let store = FocusStore::open(&db_path)?;
    run_workspace_ui(
        workspaces,
        initial_workspace,
        store,
        config,
        &config_path,
        pane_id,
    )
}

fn setup() -> Result<()> {
    let config_path = default_config_path();
    ensure_default_config(&config_path)?;
    let executable = env::current_exe()?;
    println!("Created or found config: {}", config_path.display());
    println!("\nOptional Herdr popup configuration:\n");
    println!("[[keys.command]]");
    println!("key = \"prefix+t\"");
    println!("type = \"popup\"");
    println!(
        "command = 'exec \"{}\" --cwd \"$HERDR_ACTIVE_PANE_CWD\" --pane \"$HERDR_ACTIVE_PANE_ID\"'",
        executable.display()
    );
    println!("description = \"Focus Pane\"");
    println!("width = \"88%\"");
    println!("height = \"84%\"");
    Ok(())
}
