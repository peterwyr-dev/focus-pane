use crate::{
    config::{AppConfig, FocusPreset, default_config_path},
    herdr::HerdrBridge,
    markdown::{
        FileRevision, IndentDirection, MarkdownConflict, MarkdownTaskRepository, MoveDirection,
    },
    models::{Task, TaskFocusSummary, TimerPhase, TimerState},
    store::FocusStore,
    worker::{spawn_worker, unix_now},
    workspace::WorkspaceDefinition,
};
use anyhow::{Result, bail};
use chrono::{DateTime, Local};
use crossterm::event::{self, Event, KeyCode, KeyEvent, KeyEventKind, KeyModifiers};
use ratatui::{
    Frame,
    layout::{Alignment, Constraint, Direction, Layout, Rect},
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Borders, Clear, HighlightSpacing, List, ListItem, ListState, Paragraph},
};
use std::{
    collections::HashMap,
    path::{Path, PathBuf},
    time::{Duration, Instant},
};

const BASE: Color = Color::Rgb(25, 23, 36);
const SURFACE: Color = Color::Rgb(31, 29, 46);
const OVERLAY: Color = Color::Rgb(38, 35, 58);
const MUTED: Color = Color::Rgb(110, 106, 134);
const SUBTLE: Color = Color::Rgb(144, 140, 170);
const TEXT: Color = Color::Rgb(224, 222, 244);
const GOLD: Color = Color::Rgb(246, 193, 119);
const IRIS: Color = Color::Rgb(196, 167, 231);

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum TaskFilter {
    Open,
    Completed,
    All,
}

impl TaskFilter {
    fn label(self) -> &'static str {
        match self {
            Self::Open => "open",
            Self::Completed => "completed",
            Self::All => "all",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum CommandId {
    SelectNext,
    SelectPrevious,
    Search,
    FocusSelected,
    StartTasklessFocus,
    ToggleDone,
    UndoCompletion,
    AppendTask,
    AddAfter,
    EditTask,
    EstimateTask,
    ToggleBlocked,
    MoveTaskDown,
    MoveTaskUp,
    IndentTask,
    OutdentTask,
    DeleteTask,
    StartPauseResume,
    ResetTimer,
    StopTimer,
    CycleFilter,
    CyclePreset,
    CycleWorkspace,
    CycleSource,
    OpenPalette,
    OpenHelp,
    Quit,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum CommandKey {
    Char(char),
    Enter,
    Tab,
    Up,
    Down,
}

impl CommandKey {
    fn matches(self, key: KeyEvent) -> bool {
        match self {
            Self::Char(character) => {
                key.code == KeyCode::Char(character)
                    && !key
                        .modifiers
                        .intersects(KeyModifiers::CONTROL | KeyModifiers::ALT)
            }
            Self::Enter => key.code == KeyCode::Enter,
            Self::Tab => key.code == KeyCode::Tab,
            Self::Up => key.code == KeyCode::Up,
            Self::Down => key.code == KeyCode::Down,
        }
    }
}

#[derive(Debug)]
struct CommandSpec {
    id: CommandId,
    key_label: &'static str,
    label: &'static str,
    description: &'static str,
    keys: &'static [CommandKey],
    palette: bool,
    footer: bool,
}

const COMMANDS: &[CommandSpec] = &[
    CommandSpec {
        id: CommandId::SelectNext,
        key_label: "j/↓",
        label: "Select next task",
        description: "Move selection to the next visible task",
        keys: &[CommandKey::Char('j'), CommandKey::Down],
        palette: false,
        footer: false,
    },
    CommandSpec {
        id: CommandId::SelectPrevious,
        key_label: "k/↑",
        label: "Select previous task",
        description: "Move selection to the previous visible task",
        keys: &[CommandKey::Char('k'), CommandKey::Up],
        palette: false,
        footer: false,
    },
    CommandSpec {
        id: CommandId::Search,
        key_label: "/",
        label: "Search tasks",
        description: "Filter tasks by text, heading, status, priority, or due date",
        keys: &[CommandKey::Char('/')],
        palette: true,
        footer: true,
    },
    CommandSpec {
        id: CommandId::FocusSelected,
        key_label: "enter",
        label: "Focus or queue selected",
        description: "Start focus, start a ready focus, or choose the next task",
        keys: &[CommandKey::Enter],
        palette: true,
        footer: true,
    },
    CommandSpec {
        id: CommandId::StartTasklessFocus,
        key_label: "t",
        label: "Start taskless focus",
        description: "Start an idle focus cycle without assigning a task",
        keys: &[CommandKey::Char('t')],
        palette: true,
        footer: true,
    },
    CommandSpec {
        id: CommandId::ToggleDone,
        key_label: "space",
        label: "Toggle completed",
        description: "Toggle the selected Markdown checkbox",
        keys: &[CommandKey::Char(' ')],
        palette: true,
        footer: true,
    },
    CommandSpec {
        id: CommandId::UndoCompletion,
        key_label: "u",
        label: "Undo last completion",
        description: "Reopen the task most recently completed in this window",
        keys: &[CommandKey::Char('u')],
        palette: true,
        footer: true,
    },
    CommandSpec {
        id: CommandId::AppendTask,
        key_label: "n",
        label: "Append task",
        description: "Append a new task to the Markdown file",
        keys: &[CommandKey::Char('n')],
        palette: true,
        footer: true,
    },
    CommandSpec {
        id: CommandId::AddAfter,
        key_label: "a",
        label: "Add task after selection",
        description: "Insert a sibling after the selected subtree",
        keys: &[CommandKey::Char('a')],
        palette: true,
        footer: true,
    },
    CommandSpec {
        id: CommandId::EditTask,
        key_label: "e",
        label: "Edit task",
        description: "Edit task text while preserving metadata",
        keys: &[CommandKey::Char('e')],
        palette: true,
        footer: true,
    },
    CommandSpec {
        id: CommandId::EstimateTask,
        key_label: "E",
        label: "Set estimate",
        description: "Set or clear the Pomodoro estimate",
        keys: &[CommandKey::Char('E')],
        palette: true,
        footer: true,
    },
    CommandSpec {
        id: CommandId::ToggleBlocked,
        key_label: "b",
        label: "Toggle blocked",
        description: "Mark the task blocked or unblocked",
        keys: &[CommandKey::Char('b')],
        palette: true,
        footer: true,
    },
    CommandSpec {
        id: CommandId::MoveTaskDown,
        key_label: "J",
        label: "Move task down",
        description: "Move the selected subtree down within its heading",
        keys: &[CommandKey::Char('J')],
        palette: true,
        footer: true,
    },
    CommandSpec {
        id: CommandId::MoveTaskUp,
        key_label: "K",
        label: "Move task up",
        description: "Move the selected subtree up within its heading",
        keys: &[CommandKey::Char('K')],
        palette: true,
        footer: true,
    },
    CommandSpec {
        id: CommandId::IndentTask,
        key_label: ">",
        label: "Indent task",
        description: "Indent the selected task and subtree",
        keys: &[CommandKey::Char('>')],
        palette: true,
        footer: true,
    },
    CommandSpec {
        id: CommandId::OutdentTask,
        key_label: "<",
        label: "Outdent task",
        description: "Outdent the selected task and subtree",
        keys: &[CommandKey::Char('<')],
        palette: true,
        footer: true,
    },
    CommandSpec {
        id: CommandId::DeleteTask,
        key_label: "d",
        label: "Delete task",
        description: "Delete the Markdown task after confirmation",
        keys: &[CommandKey::Char('d')],
        palette: true,
        footer: true,
    },
    CommandSpec {
        id: CommandId::StartPauseResume,
        key_label: "p",
        label: "Start, pause, or resume timer",
        description: "Start a ready phase or pause/resume its countdown",
        keys: &[CommandKey::Char('p')],
        palette: true,
        footer: true,
    },
    CommandSpec {
        id: CommandId::ResetTimer,
        key_label: "r",
        label: "Reset timer phase",
        description: "Restart the current focus or break countdown",
        keys: &[CommandKey::Char('r')],
        palette: true,
        footer: true,
    },
    CommandSpec {
        id: CommandId::StopTimer,
        key_label: "s",
        label: "Stop timer",
        description: "Stop the cycle and save partial focus time",
        keys: &[CommandKey::Char('s')],
        palette: true,
        footer: false,
    },
    CommandSpec {
        id: CommandId::CycleFilter,
        key_label: "tab",
        label: "Cycle task filter",
        description: "Cycle open, completed, and all tasks",
        keys: &[CommandKey::Tab],
        palette: true,
        footer: true,
    },
    CommandSpec {
        id: CommandId::CyclePreset,
        key_label: "P",
        label: "Cycle focus preset",
        description: "Choose the next configured preset while idle",
        keys: &[CommandKey::Char('P')],
        palette: true,
        footer: true,
    },
    CommandSpec {
        id: CommandId::CycleWorkspace,
        key_label: "W",
        label: "Switch workspace",
        description: "Switch to the next configured workspace",
        keys: &[CommandKey::Char('W')],
        palette: true,
        footer: true,
    },
    CommandSpec {
        id: CommandId::CycleSource,
        key_label: "F",
        label: "Switch source view",
        description: "Cycle the aggregate and individual Markdown file views",
        keys: &[CommandKey::Char('F')],
        palette: true,
        footer: true,
    },
    CommandSpec {
        id: CommandId::OpenPalette,
        key_label: ":",
        label: "Command palette",
        description: "Search and execute an available command",
        keys: &[CommandKey::Char(':')],
        palette: false,
        footer: true,
    },
    CommandSpec {
        id: CommandId::OpenHelp,
        key_label: "?",
        label: "Help",
        description: "Show contextual keybindings and commands",
        keys: &[CommandKey::Char('?')],
        palette: false,
        footer: true,
    },
    CommandSpec {
        id: CommandId::Quit,
        key_label: "q",
        label: "Close Focus Pane",
        description: "Close the popup without stopping an active timer",
        keys: &[CommandKey::Char('q')],
        palette: true,
        footer: false,
    },
];

fn command_spec(id: CommandId) -> &'static CommandSpec {
    COMMANDS
        .iter()
        .find(|command| command.id == id)
        .expect("registered command")
}

fn command_for_key(key: KeyEvent) -> Option<CommandId> {
    COMMANDS
        .iter()
        .find(|command| command.keys.iter().any(|binding| binding.matches(key)))
        .map(|command| command.id)
}

fn footer_label(command: CommandId) -> &'static str {
    match command {
        CommandId::OpenPalette => "commands",
        CommandId::OpenHelp => "help",
        CommandId::FocusSelected => "focus/next",
        CommandId::StartPauseResume => "timer",
        CommandId::ToggleDone => "done",
        CommandId::UndoCompletion => "undo",
        CommandId::Search => "search",
        CommandId::AppendTask => "add",
        CommandId::EditTask => "edit",
        CommandId::AddAfter => "after",
        CommandId::EstimateTask => "estimate",
        CommandId::ToggleBlocked => "blocked",
        CommandId::CycleFilter => "states",
        CommandId::CyclePreset => "preset",
        CommandId::CycleWorkspace => "workspace",
        CommandId::CycleSource => "source",
        CommandId::ResetTimer => "reset",
        CommandId::DeleteTask => "delete",
        CommandId::MoveTaskDown | CommandId::MoveTaskUp => "reorder",
        CommandId::IndentTask | CommandId::OutdentTask => "indent",
        CommandId::StartTasklessFocus => "taskless",
        _ => command_spec(command).label,
    }
}

fn queued_task_from_state(state: &TimerState) -> Option<Task> {
    Some(Task {
        source_path: PathBuf::from(state.next_source_path.as_deref()?),
        line_number: state.next_source_line?,
        text: state.next_task_text.clone()?,
        checked: false,
        heading: None,
        task_id: Some(state.next_task_id.clone()?),
        indent: 0,
        metadata: Default::default(),
    })
}

#[derive(Debug, Clone)]
struct DisplayRow {
    text: String,
    task_index: Option<usize>,
    heading: bool,
    blocked: bool,
}

#[derive(Debug, Clone)]
struct CompletionUndo {
    task: Task,
    timer_id: Option<String>,
    previous_next_task: Option<Task>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum InputMode {
    Normal,
    Search,
    Add,
    AddAfter,
    Edit,
    Estimate,
    DeleteConfirm,
    CommandPalette,
    Help,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ViewportMode {
    Compact,
    Normal,
    Wide,
}

impl ViewportMode {
    fn for_area(area: Rect) -> Self {
        if area.width < 72 || area.height < 18 {
            Self::Compact
        } else if area.width < 110 {
            Self::Normal
        } else {
            Self::Wide
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum UiOutcome {
    Continue,
    Quit,
}

struct UiWorkspace {
    name: String,
    repositories: Vec<MarkdownTaskRepository>,
    warnings: Vec<String>,
}

impl UiWorkspace {
    fn from_definition(definition: WorkspaceDefinition) -> Self {
        Self {
            name: definition.name,
            repositories: definition
                .files
                .into_iter()
                .map(MarkdownTaskRepository::new)
                .collect(),
            warnings: definition.warnings,
        }
    }
}

pub struct UiApp {
    workspaces: Vec<UiWorkspace>,
    workspace_index: usize,
    source_view: Option<usize>,
    store: FocusStore,
    config: AppConfig,
    config_path: PathBuf,
    origin_pane_id: Option<String>,
    tasks: Vec<Task>,
    rows: Vec<DisplayRow>,
    list_state: ListState,
    filter: TaskFilter,
    search: String,
    input_mode: InputMode,
    input_text: String,
    input_cursor: usize,
    message: String,
    known_revisions: HashMap<PathBuf, FileRevision>,
    pending_revisions: HashMap<PathBuf, FileRevision>,
    source_errors: HashMap<PathBuf, String>,
    pending_source_errors: HashMap<PathBuf, String>,
    selected_preset_name: String,
    palette_query: String,
    palette_selection: usize,
    help_scroll: usize,
    last_completion: Option<CompletionUndo>,
}

impl UiApp {
    pub fn new(
        repository: MarkdownTaskRepository,
        store: FocusStore,
        config: AppConfig,
        origin_pane_id: Option<String>,
    ) -> Result<Self> {
        Self::new_with_config_path(
            repository,
            store,
            config,
            default_config_path(),
            origin_pane_id,
        )
    }

    pub fn new_with_config_path(
        repository: MarkdownTaskRepository,
        store: FocusStore,
        config: AppConfig,
        config_path: PathBuf,
        origin_pane_id: Option<String>,
    ) -> Result<Self> {
        let workspace = UiWorkspace {
            name: repository
                .path()
                .parent()
                .and_then(Path::file_name)
                .and_then(|name| name.to_str())
                .unwrap_or("Current")
                .to_owned(),
            repositories: vec![repository],
            warnings: Vec::new(),
        };
        Self::new_with_ui_workspaces(
            vec![workspace],
            0,
            store,
            config,
            config_path,
            origin_pane_id,
        )
    }

    pub fn new_with_workspaces(
        definitions: Vec<WorkspaceDefinition>,
        initial_workspace: usize,
        store: FocusStore,
        config: AppConfig,
        config_path: PathBuf,
        origin_pane_id: Option<String>,
    ) -> Result<Self> {
        if definitions.is_empty() {
            bail!("at least one workspace is required");
        }
        Self::new_with_ui_workspaces(
            definitions
                .into_iter()
                .map(UiWorkspace::from_definition)
                .collect(),
            initial_workspace,
            store,
            config,
            config_path,
            origin_pane_id,
        )
    }

    fn new_with_ui_workspaces(
        workspaces: Vec<UiWorkspace>,
        initial_workspace: usize,
        store: FocusStore,
        config: AppConfig,
        config_path: PathBuf,
        origin_pane_id: Option<String>,
    ) -> Result<Self> {
        let timer = store.get_timer()?;
        let default_preset = config.default_focus_preset();
        let selected_preset_name =
            if timer.updated_at > 0.0 && config.preset(&timer.preset_name).is_some() {
                timer.preset_name
            } else {
                default_preset.name
            };
        let workspace_index = initial_workspace.min(workspaces.len().saturating_sub(1));
        let mut app = Self {
            workspaces,
            workspace_index,
            source_view: None,
            store,
            config,
            config_path,
            origin_pane_id,
            tasks: Vec::new(),
            rows: Vec::new(),
            list_state: ListState::default(),
            filter: TaskFilter::Open,
            search: String::new(),
            input_mode: InputMode::Normal,
            input_text: String::new(),
            input_cursor: 0,
            message: String::new(),
            known_revisions: HashMap::new(),
            pending_revisions: HashMap::new(),
            source_errors: HashMap::new(),
            pending_source_errors: HashMap::new(),
            selected_preset_name,
            palette_query: String::new(),
            palette_selection: 0,
            help_scroll: 0,
            last_completion: None,
        };
        app.reload(None)?;
        Ok(app)
    }

    pub fn render(&mut self, frame: &mut Frame<'_>) {
        let full_area = frame.area();
        let viewport = ViewportMode::for_area(full_area);
        frame.render_widget(Block::new().style(Style::default().bg(BASE)), full_area);

        let constraints = match viewport {
            ViewportMode::Compact => [
                Constraint::Length(2),
                Constraint::Min(1),
                Constraint::Length(1),
                Constraint::Length(2),
                Constraint::Length(1),
            ],
            ViewportMode::Normal | ViewportMode::Wide => [
                Constraint::Length(3),
                Constraint::Min(4),
                Constraint::Length(2),
                Constraint::Length(2),
                Constraint::Length(2),
            ],
        };
        let chunks = Layout::default()
            .direction(Direction::Vertical)
            .constraints(constraints)
            .split(full_area);
        self.render_search(frame, chunks[0], viewport);
        self.render_tasks(frame, chunks[1]);
        self.render_selection_status(frame, chunks[2]);
        self.render_timer(frame, chunks[3]);
        self.render_keys(frame, chunks[4], viewport);
        if matches!(
            self.input_mode,
            InputMode::Add | InputMode::AddAfter | InputMode::Edit | InputMode::Estimate
        ) {
            self.render_text_input_modal(frame, frame.area());
        } else if self.input_mode == InputMode::DeleteConfirm {
            self.render_delete_modal(frame, frame.area());
        } else if self.input_mode == InputMode::CommandPalette {
            self.render_command_palette(frame, frame.area());
        } else if self.input_mode == InputMode::Help {
            self.render_help(frame, frame.area());
        }
    }

    fn render_search(&self, frame: &mut Frame<'_>, area: Rect, viewport: ViewportMode) {
        let count_width = if viewport == ViewportMode::Compact {
            0
        } else {
            36.min(area.width / 2)
        };
        let columns = Layout::default()
            .direction(Direction::Horizontal)
            .constraints([Constraint::Min(1), Constraint::Length(count_width)])
            .split(area);
        let search = match self.input_mode {
            _ if self.search.is_empty() => Line::from(vec![
                Span::styled("/", Style::default().fg(IRIS).add_modifier(Modifier::BOLD)),
                Span::styled(" search tasks", Style::default().fg(MUTED)),
            ]),
            _ => Line::from(vec![
                Span::styled("/", Style::default().fg(IRIS).add_modifier(Modifier::BOLD)),
                Span::styled(format!(" {}", self.search), Style::default().fg(TEXT)),
            ]),
        };
        frame.render_widget(
            Paragraph::new(search)
                .block(
                    Block::new()
                        .borders(Borders::BOTTOM)
                        .border_style(Style::default().fg(OVERLAY)),
                )
                .style(Style::default().bg(BASE)),
            columns[0],
        );
        if count_width > 0 {
            frame.render_widget(
                Paragraph::new(format!(
                    "{} tasks · {} · {} / {}",
                    self.visible_task_count(),
                    self.filter.label(),
                    self.current_workspace().name,
                    self.view_label()
                ))
                .alignment(Alignment::Right)
                .block(
                    Block::new()
                        .borders(Borders::BOTTOM)
                        .border_style(Style::default().fg(OVERLAY)),
                )
                .style(Style::default().fg(MUTED).bg(BASE)),
                columns[1],
            );
        }
    }

    fn render_tasks(&mut self, frame: &mut Frame<'_>, area: Rect) {
        let items = self
            .rows
            .iter()
            .map(|row| {
                let style = if row.heading {
                    Style::default().fg(TEXT).add_modifier(Modifier::BOLD)
                } else if row.blocked {
                    Style::default().fg(GOLD).add_modifier(Modifier::ITALIC)
                } else {
                    Style::default().fg(TEXT)
                };
                ListItem::new(row.text.clone()).style(style)
            })
            .collect::<Vec<_>>();
        let list = List::new(items)
            .style(Style::default().fg(TEXT).bg(SURFACE))
            .highlight_style(
                Style::default()
                    .fg(BASE)
                    .bg(IRIS)
                    .add_modifier(Modifier::BOLD),
            )
            .highlight_spacing(HighlightSpacing::Always);
        frame.render_stateful_widget(list, area, &mut self.list_state);
    }

    fn render_selection_status(&self, frame: &mut Frame<'_>, area: Rect) {
        if area.height <= 1 {
            let text = if !self.message.is_empty() {
                self.message.clone()
            } else if let Some(task) = self.selected_task() {
                format!(
                    "{} · line {}{}",
                    task.text,
                    task.line_number,
                    if task.metadata.blocked {
                        " · blocked"
                    } else {
                        ""
                    }
                )
            } else {
                "no matching tasks".into()
            };
            frame.render_widget(
                Paragraph::new(text).style(Style::default().fg(SUBTLE).bg(BASE)),
                area,
            );
            return;
        }
        let lines = self
            .selected_task()
            .map(|task| {
                let task_sources = task.history_key().into_iter().collect::<Vec<_>>();
                let summary = self
                    .store
                    .summaries_for_task_sources(&task_sources)
                    .ok()
                    .and_then(|summaries| {
                        task.history_key()
                            .as_ref()
                            .and_then(|key| summaries.get(key).cloned())
                    })
                    .unwrap_or_default();
                let last = task
                    .task_id
                    .as_deref()
                    .and_then(|id| {
                        self.store
                            .recent_sessions_for_source(
                                id,
                                task.source_path.to_string_lossy().as_ref(),
                                1,
                            )
                            .ok()
                    })
                    .and_then(|sessions| sessions.into_iter().next())
                    .map(|session| {
                        format!(
                            "last {}–{}",
                            format_clock(Some(session.started_at)),
                            format_clock(Some(session.ended_at))
                        )
                    })
                    .unwrap_or_else(|| "no sessions".into());
                let plan = task_plan_status(task, &summary, self.selected_preset().focus_seconds());
                let activity = format!(
                    "{}focus {} · full {} ({}x) · stopped {} ({}x) · {}",
                    if self.message.is_empty() {
                        String::new()
                    } else {
                        format!("{} · ", self.message)
                    },
                    format_duration(summary.total_seconds),
                    format_duration(summary.completed_seconds),
                    summary.completed_sessions,
                    format_duration(summary.stopped_seconds),
                    summary.stopped_sessions,
                    last,
                );
                vec![
                    Line::from(format!(
                        "{} › {} · line {}{plan}",
                        source_label(&task.source_path),
                        task.heading.as_deref().unwrap_or("Tasks"),
                        task.line_number,
                    )),
                    Line::from(activity),
                ]
            })
            .unwrap_or_else(|| {
                vec![Line::from(format!(
                    "{} · {} · no matching tasks",
                    self.current_workspace().name,
                    self.view_label()
                ))]
            });
        frame.render_widget(
            Paragraph::new(lines).style(Style::default().fg(SUBTLE).bg(BASE)),
            area,
        );
    }

    fn render_timer(&self, frame: &mut Frame<'_>, area: Rect) {
        let state = self.store.get_timer().ok();
        let text = state
            .as_ref()
            .map(|state| timer_line(state, &self.config, unix_now()))
            .unwrap_or_else(|| "◇ READY · select a task and press enter".into());
        frame.render_widget(
            Paragraph::new(text)
                .block(
                    Block::new()
                        .borders(Borders::TOP)
                        .border_style(Style::default().fg(OVERLAY)),
                )
                .style(
                    Style::default()
                        .fg(GOLD)
                        .bg(BASE)
                        .add_modifier(Modifier::BOLD),
                ),
            area,
        );
    }

    fn render_text_input_modal(&self, frame: &mut Frame<'_>, area: Rect) {
        let (title, placeholder, action) = match self.input_mode {
            InputMode::Add => (" Add a Markdown task ", "What needs to be done?", "add"),
            InputMode::AddAfter => (" Add after selected task ", "New sibling task", "insert"),
            InputMode::Edit => (" Edit task ", "Task text", "save"),
            InputMode::Estimate => (" Estimate task ", "Pomodoros; empty or 0 clears", "save"),
            _ => return,
        };
        let modal = centered_modal(area, 68, 7);
        frame.render_widget(Clear, modal);
        let block = Block::new()
            .title(title)
            .title_style(Style::default().fg(IRIS).add_modifier(Modifier::BOLD))
            .borders(Borders::ALL)
            .border_style(Style::default().fg(IRIS))
            .style(Style::default().fg(TEXT).bg(SURFACE));
        let inner = block.inner(modal);
        frame.render_widget(block, modal);
        let rows = Layout::default()
            .direction(Direction::Vertical)
            .constraints([
                Constraint::Length(1),
                Constraint::Length(2),
                Constraint::Length(1),
            ])
            .split(inner);
        let input_text = if self.input_text.is_empty() {
            Line::from(vec![
                Span::styled("> ", Style::default().fg(IRIS)),
                Span::styled(placeholder, Style::default().fg(MUTED)),
            ])
        } else {
            Line::from(vec![
                Span::styled("> ", Style::default().fg(IRIS)),
                Span::raw(&self.input_text),
            ])
        };
        frame.render_widget(
            Paragraph::new(input_text)
                .block(
                    Block::new()
                        .borders(Borders::BOTTOM)
                        .border_style(Style::default().fg(OVERLAY)),
                )
                .style(Style::default().fg(TEXT).bg(SURFACE)),
            rows[1],
        );
        frame.render_widget(
            Paragraph::new(Line::from(vec![
                Span::styled(
                    "enter",
                    Style::default().fg(IRIS).add_modifier(Modifier::BOLD),
                ),
                Span::styled(format!(" {action}  ·  "), Style::default().fg(SUBTLE)),
                Span::styled(
                    "esc",
                    Style::default().fg(IRIS).add_modifier(Modifier::BOLD),
                ),
                Span::styled(" cancel", Style::default().fg(SUBTLE)),
            ]))
            .alignment(Alignment::Right)
            .style(Style::default().bg(SURFACE)),
            rows[2],
        );
        let cursor_x = rows[1]
            .x
            .saturating_add(2)
            .saturating_add(self.input_cursor as u16)
            .min(rows[1].right().saturating_sub(1));
        if rows[1].width > 0 && rows[1].height > 0 {
            frame.set_cursor_position((cursor_x, rows[1].y));
        }
    }

    fn render_command_palette(&self, frame: &mut Frame<'_>, area: Rect) {
        let modal = centered_modal(area, 88, 22);
        frame.render_widget(Clear, modal);
        let block = Block::new()
            .title(" Command palette ")
            .title_style(Style::default().fg(IRIS).add_modifier(Modifier::BOLD))
            .borders(Borders::ALL)
            .border_style(Style::default().fg(IRIS))
            .style(Style::default().fg(TEXT).bg(SURFACE));
        let inner = block.inner(modal);
        frame.render_widget(block, modal);
        let rows = Layout::default()
            .direction(Direction::Vertical)
            .constraints([
                Constraint::Length(2),
                Constraint::Min(1),
                Constraint::Length(1),
            ])
            .split(inner);
        frame.render_widget(
            Paragraph::new(Line::from(vec![
                Span::styled(": ", Style::default().fg(IRIS)),
                Span::raw(&self.palette_query),
            ]))
            .block(
                Block::new()
                    .borders(Borders::BOTTOM)
                    .border_style(Style::default().fg(OVERLAY)),
            )
            .style(Style::default().fg(TEXT).bg(SURFACE)),
            rows[0],
        );
        let commands = self.palette_commands();
        let items = if commands.is_empty() {
            vec![ListItem::new("No matching available commands").style(Style::default().fg(MUTED))]
        } else {
            let visible_height = rows[1].height.max(1) as usize;
            let start = self
                .palette_selection
                .saturating_add(1)
                .saturating_sub(visible_height);
            commands
                .iter()
                .enumerate()
                .skip(start)
                .take(visible_height)
                .map(|(index, command)| {
                    let style = if index == self.palette_selection {
                        Style::default()
                            .fg(BASE)
                            .bg(IRIS)
                            .add_modifier(Modifier::BOLD)
                    } else {
                        Style::default().fg(TEXT)
                    };
                    ListItem::new(format!(
                        "{:<8} {} — {}",
                        command.key_label, command.label, command.description
                    ))
                    .style(style)
                })
                .collect()
        };
        frame.render_widget(
            List::new(items).style(Style::default().bg(SURFACE)),
            rows[1],
        );
        frame.render_widget(
            Paragraph::new("↑/↓ select · enter run · esc close")
                .alignment(Alignment::Right)
                .style(Style::default().fg(SUBTLE).bg(SURFACE)),
            rows[2],
        );
        let cursor_x = rows[0]
            .x
            .saturating_add(2)
            .saturating_add(self.palette_query.chars().count() as u16)
            .min(rows[0].right().saturating_sub(1));
        if rows[0].width > 0 && rows[0].height > 0 {
            frame.set_cursor_position((cursor_x, rows[0].y));
        }
    }

    fn render_help(&self, frame: &mut Frame<'_>, area: Rect) {
        let modal = centered_modal(area, 96, 28);
        frame.render_widget(Clear, modal);
        let block = Block::new()
            .title(" Focus Pane help ")
            .title_style(Style::default().fg(IRIS).add_modifier(Modifier::BOLD))
            .borders(Borders::ALL)
            .border_style(Style::default().fg(IRIS))
            .style(Style::default().fg(TEXT).bg(SURFACE));
        let inner = block.inner(modal);
        frame.render_widget(block, modal);
        let rows = Layout::default()
            .direction(Direction::Vertical)
            .constraints([Constraint::Min(1), Constraint::Length(1)])
            .split(inner);
        let phase = self.store.get_timer().ok().map(|state| state.phase);
        let visible_height = rows[0].height as usize;
        let items = COMMANDS
            .iter()
            .skip(self.help_scroll)
            .take(visible_height)
            .map(|command| {
                let available = self.command_available(command.id, phase);
                ListItem::new(format!(
                    "{:<8} {} — {}",
                    command.key_label, command.label, command.description
                ))
                .style(if available {
                    Style::default().fg(TEXT)
                } else {
                    Style::default().fg(MUTED).add_modifier(Modifier::DIM)
                })
            })
            .collect::<Vec<_>>();
        frame.render_widget(
            List::new(items).style(Style::default().bg(SURFACE)),
            rows[0],
        );
        frame.render_widget(
            Paragraph::new("dimmed = unavailable now · ↑/↓ scroll · ?/esc close")
                .alignment(Alignment::Right)
                .style(Style::default().fg(SUBTLE).bg(SURFACE)),
            rows[1],
        );
    }

    fn render_delete_modal(&self, frame: &mut Frame<'_>, area: Rect) {
        let modal = centered_modal(area, 68, 7);
        frame.render_widget(Clear, modal);
        let block = Block::new()
            .title(" Delete task? ")
            .title_style(Style::default().fg(GOLD).add_modifier(Modifier::BOLD))
            .borders(Borders::ALL)
            .border_style(Style::default().fg(GOLD))
            .style(Style::default().fg(TEXT).bg(SURFACE));
        let inner = block.inner(modal);
        frame.render_widget(block, modal);
        let rows = Layout::default()
            .direction(Direction::Vertical)
            .constraints([
                Constraint::Length(1),
                Constraint::Length(2),
                Constraint::Length(1),
            ])
            .split(inner);
        let task_text = self
            .selected_task()
            .map(|task| task.text.as_str())
            .unwrap_or("No task selected");
        frame.render_widget(
            Paragraph::new(format!("Delete “{task_text}” from Markdown?"))
                .style(Style::default().fg(TEXT).bg(SURFACE)),
            rows[1],
        );
        frame.render_widget(
            Paragraph::new(Line::from(vec![
                Span::styled(
                    "enter/y",
                    Style::default().fg(GOLD).add_modifier(Modifier::BOLD),
                ),
                Span::styled(" delete  ·  ", Style::default().fg(SUBTLE)),
                Span::styled(
                    "esc/n",
                    Style::default().fg(IRIS).add_modifier(Modifier::BOLD),
                ),
                Span::styled(" cancel", Style::default().fg(SUBTLE)),
            ]))
            .alignment(Alignment::Right)
            .style(Style::default().bg(SURFACE)),
            rows[2],
        );
    }

    fn render_keys(&self, frame: &mut Frame<'_>, area: Rect, viewport: ViewportMode) {
        const ORDER: &[CommandId] = &[
            CommandId::OpenPalette,
            CommandId::OpenHelp,
            CommandId::FocusSelected,
            CommandId::StartPauseResume,
            CommandId::ToggleDone,
            CommandId::UndoCompletion,
            CommandId::Search,
            CommandId::CycleWorkspace,
            CommandId::CycleSource,
            CommandId::AppendTask,
            CommandId::EditTask,
            CommandId::AddAfter,
            CommandId::EstimateTask,
            CommandId::ToggleBlocked,
            CommandId::CycleFilter,
            CommandId::CyclePreset,
            CommandId::ResetTimer,
            CommandId::DeleteTask,
            CommandId::MoveTaskDown,
            CommandId::MoveTaskUp,
            CommandId::IndentTask,
            CommandId::OutdentTask,
            CommandId::StartTasklessFocus,
        ];
        let phase = self.store.get_timer().ok().map(|state| state.phase);
        let max_lines = match viewport {
            ViewportMode::Compact => 1,
            ViewportMode::Normal | ViewportMode::Wide => area.height.min(2) as usize,
        };
        let max_width = area.width as usize;
        let mut lines: Vec<Line<'static>> = Vec::new();
        let mut spans: Vec<Span<'static>> = Vec::new();
        let mut line_width = 0;
        for command_id in ORDER {
            let command = command_spec(*command_id);
            if !command.footer || !self.command_available(command.id, phase) {
                continue;
            }
            let label = footer_label(command.id);
            let segment_width = command.key_label.chars().count() + 1 + label.chars().count();
            let separator_width = usize::from(!spans.is_empty()) * 3;
            if !spans.is_empty() && line_width + separator_width + segment_width > max_width {
                lines.push(Line::from(std::mem::take(&mut spans)));
                line_width = 0;
                if lines.len() >= max_lines {
                    break;
                }
            }
            if !spans.is_empty() {
                spans.push(Span::styled(" · ", Style::default().fg(SUBTLE)));
                line_width += 3;
            }
            spans.push(Span::styled(
                command.key_label,
                Style::default().fg(IRIS).add_modifier(Modifier::BOLD),
            ));
            spans.push(Span::styled(
                format!(" {label}"),
                Style::default().fg(SUBTLE),
            ));
            line_width += segment_width;
        }
        if !spans.is_empty() && lines.len() < max_lines {
            lines.push(Line::from(spans));
        }
        frame.render_widget(Paragraph::new(lines).style(Style::default().bg(BASE)), area);
    }

    pub fn handle_key(&mut self, key: KeyEvent) -> Result<UiOutcome> {
        if key.kind == KeyEventKind::Release {
            return Ok(UiOutcome::Continue);
        }
        if key.code == KeyCode::Esc {
            if self.input_mode == InputMode::Normal {
                return Ok(UiOutcome::Quit);
            }
            self.input_mode = InputMode::Normal;
            self.input_text.clear();
            self.input_cursor = 0;
            self.palette_query.clear();
            return Ok(UiOutcome::Continue);
        }
        if key.code == KeyCode::Char('c') && key.modifiers.contains(KeyModifiers::CONTROL) {
            return Ok(UiOutcome::Quit);
        }
        match self.input_mode {
            InputMode::Search => self.handle_search_key(key)?,
            InputMode::Add | InputMode::AddAfter | InputMode::Edit | InputMode::Estimate => {
                self.handle_text_input_key(key)?
            }
            InputMode::DeleteConfirm => self.handle_delete_key(key)?,
            InputMode::CommandPalette => return self.handle_palette_key(key),
            InputMode::Help => self.handle_help_key(key),
            InputMode::Normal => return self.handle_normal_key(key),
        }
        Ok(UiOutcome::Continue)
    }

    fn handle_normal_key(&mut self, key: KeyEvent) -> Result<UiOutcome> {
        let Some(command) = command_for_key(key) else {
            return Ok(UiOutcome::Continue);
        };
        self.execute_command(command)
    }

    fn current_workspace(&self) -> &UiWorkspace {
        &self.workspaces[self.workspace_index]
    }

    fn repository_for_task(&self, task: &Task) -> Option<&MarkdownTaskRepository> {
        self.current_workspace()
            .repositories
            .iter()
            .find(|repository| repository.path() == task.source_path)
    }

    fn active_source_index(&self) -> Option<usize> {
        self.selected_task()
            .and_then(|task| {
                self.current_workspace()
                    .repositories
                    .iter()
                    .position(|repository| repository.path() == task.source_path)
            })
            .or(self.source_view)
            .or_else(|| (!self.current_workspace().repositories.is_empty()).then_some(0))
    }

    fn active_repository(&self) -> Option<&MarkdownTaskRepository> {
        self.active_source_index()
            .and_then(|index| self.current_workspace().repositories.get(index))
            .filter(|repository| !self.source_errors.contains_key(repository.path()))
    }

    fn view_label(&self) -> String {
        match self.source_view {
            None => "All sources".into(),
            Some(index) => self
                .current_workspace()
                .repositories
                .get(index)
                .map(|repository| source_label(repository.path()))
                .unwrap_or_else(|| "Unavailable source".into()),
        }
    }

    fn command_available(&self, command: CommandId, phase: Option<TimerPhase>) -> bool {
        let has_task = self.selected_task().is_some();
        match command {
            CommandId::SelectNext | CommandId::SelectPrevious => self.visible_task_count() > 0,
            CommandId::UndoCompletion => self.last_completion.is_some(),
            CommandId::FocusSelected
            | CommandId::ToggleDone
            | CommandId::AddAfter
            | CommandId::EditTask
            | CommandId::EstimateTask
            | CommandId::ToggleBlocked
            | CommandId::MoveTaskDown
            | CommandId::MoveTaskUp
            | CommandId::IndentTask
            | CommandId::OutdentTask
            | CommandId::DeleteTask => has_task,
            CommandId::AppendTask => self.active_repository().is_some(),
            CommandId::CycleWorkspace => self.workspaces.len() > 1,
            CommandId::CycleSource => self.current_workspace().repositories.len() > 1,
            CommandId::StartTasklessFocus => phase == Some(TimerPhase::Idle),
            CommandId::StartPauseResume | CommandId::StopTimer => {
                phase.is_some_and(|phase| phase != TimerPhase::Idle)
            }
            CommandId::ResetTimer => {
                phase.is_some_and(|phase| phase.is_running() || phase.is_paused())
            }
            CommandId::CyclePreset => {
                phase == Some(TimerPhase::Idle) && self.config.available_presets().len() > 1
            }
            _ => true,
        }
    }

    fn execute_command(&mut self, command: CommandId) -> Result<UiOutcome> {
        let phase = self.store.get_timer().ok().map(|state| state.phase);
        if !self.command_available(command, phase) {
            self.message = format!("{} is not available now", command_spec(command).label);
            return Ok(UiOutcome::Continue);
        }
        match command {
            CommandId::SelectNext => self.move_selection(1),
            CommandId::SelectPrevious => self.move_selection(-1),
            CommandId::Search => self.input_mode = InputMode::Search,
            CommandId::FocusSelected => self.start_or_queue_selected()?,
            CommandId::StartTasklessFocus => self.start_focus_without_task()?,
            CommandId::ToggleDone => self.toggle_selected()?,
            CommandId::UndoCompletion => self.undo_last_completion()?,
            CommandId::AppendTask => {
                self.input_mode = InputMode::Add;
                self.input_text.clear();
                self.input_cursor = 0;
            }
            CommandId::AddAfter => {
                self.input_mode = InputMode::AddAfter;
                self.input_text.clear();
                self.input_cursor = 0;
            }
            CommandId::EditTask => {
                let text = self.selected_task().expect("available task").text.clone();
                self.input_mode = InputMode::Edit;
                self.input_cursor = text.chars().count();
                self.input_text = text;
            }
            CommandId::EstimateTask => {
                self.input_text = self
                    .selected_task()
                    .expect("available task")
                    .metadata
                    .estimate_pomodoros
                    .map(|estimate| estimate.to_string())
                    .unwrap_or_default();
                self.input_mode = InputMode::Estimate;
                self.input_cursor = self.input_text.chars().count();
            }
            CommandId::ToggleBlocked => self.toggle_selected_blocked()?,
            CommandId::MoveTaskDown => self.move_selected_task(MoveDirection::Down)?,
            CommandId::MoveTaskUp => self.move_selected_task(MoveDirection::Up)?,
            CommandId::IndentTask => self.change_selected_indent(IndentDirection::Increase)?,
            CommandId::OutdentTask => self.change_selected_indent(IndentDirection::Decrease)?,
            CommandId::DeleteTask => self.begin_delete()?,
            CommandId::StartPauseResume => self.pause_or_resume()?,
            CommandId::ResetTimer => self.reset_timer()?,
            CommandId::StopTimer => self.stop_timer()?,
            CommandId::CycleFilter => {
                self.filter = match self.filter {
                    TaskFilter::Open => TaskFilter::Completed,
                    TaskFilter::Completed => TaskFilter::All,
                    TaskFilter::All => TaskFilter::Open,
                };
                self.rebuild_rows(None)?;
            }
            CommandId::CyclePreset => self.cycle_focus_preset()?,
            CommandId::CycleWorkspace => self.cycle_workspace()?,
            CommandId::CycleSource => self.cycle_source_view()?,
            CommandId::OpenPalette => {
                self.input_mode = InputMode::CommandPalette;
                self.palette_query.clear();
                self.palette_selection = 0;
            }
            CommandId::OpenHelp => {
                self.input_mode = InputMode::Help;
                self.help_scroll = 0;
            }
            CommandId::Quit => return Ok(UiOutcome::Quit),
        }
        Ok(UiOutcome::Continue)
    }

    fn palette_commands(&self) -> Vec<&'static CommandSpec> {
        let phase = self.store.get_timer().ok().map(|state| state.phase);
        let query = self.palette_query.to_lowercase();
        COMMANDS
            .iter()
            .filter(|command| command.palette)
            .filter(|command| self.command_available(command.id, phase))
            .filter(|command| {
                query.is_empty()
                    || command.label.to_lowercase().contains(&query)
                    || command.description.to_lowercase().contains(&query)
                    || command.key_label.to_lowercase().contains(&query)
            })
            .collect()
    }

    fn handle_palette_key(&mut self, key: KeyEvent) -> Result<UiOutcome> {
        match key.code {
            KeyCode::Down => {
                let count = self.palette_commands().len();
                if count > 0 {
                    self.palette_selection = (self.palette_selection + 1).min(count - 1);
                }
            }
            KeyCode::Up => {
                self.palette_selection = self.palette_selection.saturating_sub(1);
            }
            KeyCode::Backspace => {
                self.palette_query.pop();
                self.palette_selection = 0;
            }
            KeyCode::Enter => {
                let command = self
                    .palette_commands()
                    .get(self.palette_selection)
                    .map(|command| command.id);
                self.input_mode = InputMode::Normal;
                self.palette_query.clear();
                if let Some(command) = command {
                    return self.execute_command(command);
                }
            }
            KeyCode::Char(character)
                if !key
                    .modifiers
                    .intersects(KeyModifiers::CONTROL | KeyModifiers::ALT) =>
            {
                self.palette_query.push(character);
                self.palette_selection = 0;
            }
            _ => {}
        }
        Ok(UiOutcome::Continue)
    }

    fn handle_help_key(&mut self, key: KeyEvent) {
        match key.code {
            KeyCode::Char('?') => self.input_mode = InputMode::Normal,
            KeyCode::Down | KeyCode::Char('j') => {
                self.help_scroll = (self.help_scroll + 1).min(COMMANDS.len().saturating_sub(1));
            }
            KeyCode::Up | KeyCode::Char('k') => {
                self.help_scroll = self.help_scroll.saturating_sub(1);
            }
            _ => {}
        }
    }

    fn begin_delete(&mut self) -> Result<()> {
        if self.selected_task().is_some() {
            self.input_mode = InputMode::DeleteConfirm;
        }
        Ok(())
    }

    fn handle_delete_key(&mut self, key: KeyEvent) -> Result<()> {
        match key.code {
            KeyCode::Enter | KeyCode::Char('y') => {
                let Some(task) = self.selected_task().cloned() else {
                    self.input_mode = InputMode::Normal;
                    return Ok(());
                };
                let Some(repository) = self.repository_for_task(&task) else {
                    self.input_mode = InputMode::Normal;
                    self.message = "task source is unavailable in this workspace".into();
                    return Ok(());
                };
                if let Err(error) = repository.delete(&task) {
                    return self.recover_markdown_conflict(error, Some(&task.key()));
                }
                let state = self.store.get_timer()?;
                if state.next_task_id.as_deref() == task.task_id.as_deref()
                    && state.next_source_path.as_deref()
                        == Some(task.source_path.to_string_lossy().as_ref())
                {
                    self.store.clear_next_task_at(unix_now())?;
                }
                self.input_mode = InputMode::Normal;
                self.message = "task deleted; focus history preserved".into();
                self.reload(None)?;
            }
            KeyCode::Char('n') => self.input_mode = InputMode::Normal,
            _ => {}
        }
        Ok(())
    }

    fn handle_search_key(&mut self, key: KeyEvent) -> Result<()> {
        match key.code {
            KeyCode::Enter => self.input_mode = InputMode::Normal,
            KeyCode::Backspace => {
                self.search.pop();
                self.rebuild_rows(None)?;
            }
            KeyCode::Char(character)
                if !key
                    .modifiers
                    .intersects(KeyModifiers::CONTROL | KeyModifiers::ALT) =>
            {
                self.search.push(character);
                self.rebuild_rows(None)?;
            }
            _ => {}
        }
        Ok(())
    }

    fn handle_text_input_key(&mut self, key: KeyEvent) -> Result<()> {
        match key.code {
            KeyCode::Enter => {
                let mode = self.input_mode;
                if self.input_text.trim().is_empty() && mode != InputMode::Estimate {
                    return Ok(());
                }
                let selected = self.selected_task().cloned();
                let result = match mode {
                    InputMode::Add => {
                        let Some(repository) = self.active_repository() else {
                            self.input_mode = InputMode::Normal;
                            self.message = "workspace has no available Markdown source".into();
                            return Ok(());
                        };
                        repository.add(&self.input_text)
                    }
                    InputMode::AddAfter => {
                        let Some(task) = selected.as_ref() else {
                            self.input_mode = InputMode::Normal;
                            return Ok(());
                        };
                        let Some(repository) = self.repository_for_task(task) else {
                            self.input_mode = InputMode::Normal;
                            return Ok(());
                        };
                        repository.insert_after(task, &self.input_text)
                    }
                    InputMode::Edit => {
                        let Some(task) = selected.as_ref() else {
                            self.input_mode = InputMode::Normal;
                            return Ok(());
                        };
                        let Some(repository) = self.repository_for_task(task) else {
                            self.input_mode = InputMode::Normal;
                            return Ok(());
                        };
                        repository.edit(task, &self.input_text)
                    }
                    InputMode::Estimate => {
                        let Some(task) = selected.as_ref() else {
                            self.input_mode = InputMode::Normal;
                            return Ok(());
                        };
                        let estimate = match self.input_text.trim() {
                            "" | "0" => None,
                            value => match value.parse::<u32>() {
                                Ok(value) if value > 0 => Some(value),
                                _ => {
                                    self.message =
                                        "estimate must be a positive Pomodoro count".into();
                                    return Ok(());
                                }
                            },
                        };
                        let Some(repository) = self.repository_for_task(task) else {
                            self.input_mode = InputMode::Normal;
                            return Ok(());
                        };
                        repository.set_estimate(task, estimate)
                    }
                    _ => return Ok(()),
                };
                let task = match result {
                    Ok(task) => task,
                    Err(error) if error.downcast_ref::<MarkdownConflict>().is_some() => {
                        let draft = self.input_text.clone();
                        let draft_cursor = self.input_cursor;
                        let selected_key = selected.as_ref().map(Task::key);
                        self.reload(selected_key.as_deref())?;
                        self.input_mode = mode;
                        self.input_text = draft;
                        self.input_cursor = draft_cursor.min(self.input_text.chars().count());
                        let source = selected
                            .as_ref()
                            .map(|task| source_label(&task.source_path))
                            .or_else(|| {
                                self.active_repository()
                                    .map(|repository| source_label(repository.path()))
                            })
                            .unwrap_or_else(|| "Markdown source".into());
                        self.message =
                            format!("{source} changed externally; draft preserved for review");
                        return Ok(());
                    }
                    Err(error) => return Err(error),
                };
                if matches!(mode, InputMode::Add | InputMode::AddAfter) {
                    self.filter = TaskFilter::Open;
                }
                self.search.clear();
                self.input_mode = InputMode::Normal;
                self.input_text.clear();
                self.input_cursor = 0;
                self.reload(Some(&task.key()))?;
                self.message = match mode {
                    InputMode::Edit => "task edited".into(),
                    InputMode::Estimate => match task.metadata.estimate_pomodoros {
                        Some(estimate) => format!("estimate set to {estimate} Pomodoros"),
                        None => "estimate cleared".into(),
                    },
                    InputMode::AddAfter => "task inserted after selection".into(),
                    _ => "task added".into(),
                };
            }
            KeyCode::Left => {
                self.input_cursor = self.input_cursor.saturating_sub(1);
            }
            KeyCode::Right => {
                self.input_cursor = (self.input_cursor + 1).min(self.input_text.chars().count());
            }
            KeyCode::Home => self.input_cursor = 0,
            KeyCode::End => self.input_cursor = self.input_text.chars().count(),
            KeyCode::Backspace if self.input_cursor > 0 => {
                let start = char_byte_index(&self.input_text, self.input_cursor - 1);
                let end = char_byte_index(&self.input_text, self.input_cursor);
                self.input_text.replace_range(start..end, "");
                self.input_cursor -= 1;
            }
            KeyCode::Delete if self.input_cursor < self.input_text.chars().count() => {
                let start = char_byte_index(&self.input_text, self.input_cursor);
                let end = char_byte_index(&self.input_text, self.input_cursor + 1);
                self.input_text.replace_range(start..end, "");
            }
            KeyCode::Char('u') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                self.input_text.clear();
                self.input_cursor = 0;
            }
            KeyCode::Char(character)
                if !key
                    .modifiers
                    .intersects(KeyModifiers::CONTROL | KeyModifiers::ALT) =>
            {
                let index = char_byte_index(&self.input_text, self.input_cursor);
                self.input_text.insert(index, character);
                self.input_cursor += 1;
            }
            _ => {}
        }
        Ok(())
    }

    fn move_selection(&mut self, direction: isize) {
        if self.rows.is_empty() {
            self.list_state.select(None);
            return;
        }
        let mut index = self.list_state.selected().unwrap_or(0) as isize;
        loop {
            let next = index + direction;
            if next < 0 || next >= self.rows.len() as isize {
                return;
            }
            index = next;
            if self.rows[index as usize].task_index.is_some() {
                self.list_state.select(Some(index as usize));
                return;
            }
        }
    }

    fn selected_preset(&self) -> FocusPreset {
        self.config
            .preset(&self.selected_preset_name)
            .unwrap_or_else(|| self.config.default_focus_preset())
    }

    fn cycle_focus_preset(&mut self) -> Result<()> {
        let state = self.store.get_timer()?;
        if state.phase != TimerPhase::Idle {
            self.message = "stop the timer before changing presets".into();
            return Ok(());
        }
        let presets = self.config.available_presets();
        let current = presets
            .iter()
            .position(|preset| preset.name == self.selected_preset_name)
            .unwrap_or(0);
        let preset = presets[(current + 1) % presets.len()].clone();
        self.store.select_preset_at(&preset, unix_now())?;
        self.selected_preset_name = preset.name.clone();
        self.message = format!(
            "preset: {} ({}m/{}m)",
            preset.name, preset.focus_minutes, preset.break_minutes
        );
        let key = self.selected_task().map(Task::key);
        self.rebuild_rows(key.as_deref())
    }

    fn cycle_workspace(&mut self) -> Result<()> {
        self.workspace_index = (self.workspace_index + 1) % self.workspaces.len();
        self.source_view = None;
        self.search.clear();
        self.reload(None)?;
        self.message = format!(
            "workspace: {} · {}",
            self.current_workspace().name,
            self.view_label()
        );
        Ok(())
    }

    fn cycle_source_view(&mut self) -> Result<()> {
        let source_count = self.current_workspace().repositories.len();
        self.source_view = match self.source_view {
            None => Some(0),
            Some(index) if index + 1 < source_count => Some(index + 1),
            Some(_) => None,
        };
        self.reload(None)?;
        self.message = format!(
            "workspace: {} · {}",
            self.current_workspace().name,
            self.view_label()
        );
        Ok(())
    }

    fn toggle_selected_blocked(&mut self) -> Result<()> {
        let Some(task) = self.selected_task().cloned() else {
            return Ok(());
        };
        let Some(repository) = self.repository_for_task(&task) else {
            self.message = "task source is unavailable in this workspace".into();
            return Ok(());
        };
        let updated = match repository.toggle_blocked(&task) {
            Ok(updated) => updated,
            Err(error) => {
                return self.recover_markdown_conflict(error, Some(&task.key()));
            }
        };
        self.reload(Some(&updated.key()))?;
        self.message = if updated.metadata.blocked {
            "task marked blocked".into()
        } else {
            "task unblocked".into()
        };
        Ok(())
    }

    fn move_selected_task(&mut self, direction: MoveDirection) -> Result<()> {
        let Some(task) = self.selected_task().cloned() else {
            return Ok(());
        };
        let Some(repository) = self.repository_for_task(&task) else {
            self.message = "task source is unavailable in this workspace".into();
            return Ok(());
        };
        let task = match repository.ensure_id(&task) {
            Ok(task) => task,
            Err(error) => {
                return self.recover_markdown_conflict(error, Some(&task.key()));
            }
        };
        let moved = match repository.move_subtree(&task, direction) {
            Ok(moved) => moved,
            Err(error) => {
                return self.recover_markdown_conflict(error, Some(&task.key()));
            }
        };
        self.reload(Some(&task.key()))?;
        self.message = if moved.is_some() {
            match direction {
                MoveDirection::Up => "task subtree moved up".into(),
                MoveDirection::Down => "task subtree moved down".into(),
            }
        } else {
            "task cannot move further in this heading".into()
        };
        Ok(())
    }

    fn change_selected_indent(&mut self, direction: IndentDirection) -> Result<()> {
        let Some(task) = self.selected_task().cloned() else {
            return Ok(());
        };
        let Some(repository) = self.repository_for_task(&task) else {
            self.message = "task source is unavailable in this workspace".into();
            return Ok(());
        };
        let task = match repository.ensure_id(&task) {
            Ok(task) => task,
            Err(error) => {
                return self.recover_markdown_conflict(error, Some(&task.key()));
            }
        };
        let changed = match repository.change_indent(&task, direction) {
            Ok(changed) => changed,
            Err(error) => {
                return self.recover_markdown_conflict(error, Some(&task.key()));
            }
        };
        self.reload(Some(&task.key()))?;
        self.message = if changed.is_some() {
            match direction {
                IndentDirection::Increase => "task subtree indented".into(),
                IndentDirection::Decrease => "task subtree outdented".into(),
            }
        } else {
            match direction {
                IndentDirection::Increase => {
                    "task needs a previous sibling before indenting".into()
                }
                IndentDirection::Decrease => "task is already at the top level".into(),
            }
        };
        Ok(())
    }

    fn toggle_selected(&mut self) -> Result<()> {
        let Some(task) = self.selected_task().cloned() else {
            return Ok(());
        };
        let Some(repository) = self.repository_for_task(&task) else {
            self.message = "task source is unavailable in this workspace".into();
            return Ok(());
        };
        let state_before = self.store.get_timer()?;
        let updated = match repository.toggle(&task) {
            Ok(updated) => updated,
            Err(error) => {
                return self.recover_markdown_conflict(error, Some(&task.key()));
            }
        };
        self.reload(Some(&updated.key()))?;
        if !updated.checked {
            self.last_completion = None;
            self.message = "task reopened".into();
            return Ok(());
        }

        self.last_completion = Some(CompletionUndo {
            task: updated.clone(),
            timer_id: (state_before.phase != TimerPhase::Idle)
                .then(|| state_before.timer_id.clone()),
            previous_next_task: queued_task_from_state(&state_before),
        });

        // Completing Markdown is the explicit signal that advances the task
        // context. The Pomodoro itself keeps running independently.
        if !matches!(state_before.phase, TimerPhase::Idle) {
            if let Some(next) = self.next_open_task_after(&updated.key()) {
                let Some(repository) = self.repository_for_task(&next) else {
                    self.message = "next task source is unavailable · press u to undo".into();
                    return Ok(());
                };
                let next = match repository.ensure_id(&next) {
                    Ok(next) => next,
                    Err(error) => {
                        return self.recover_markdown_conflict(error, Some(&next.key()));
                    }
                };
                self.store.set_next_task_at(&next, unix_now())?;
                self.message = format!("task completed · next up: {} · press u to undo", next.text);
            } else {
                self.store.clear_next_task_at(unix_now())?;
                self.message = "task completed · no open task is queued · press u to undo".into();
            }
        } else {
            self.message = "task completed · press u to undo".into();
        }
        self.reload(None)
    }

    fn undo_last_completion(&mut self) -> Result<()> {
        let Some(undo) = self.last_completion.clone() else {
            return Ok(());
        };
        let Some(repository) = self.repository_for_task(&undo.task) else {
            self.message = "completed task source is unavailable in this workspace".into();
            return Ok(());
        };
        let reopened = match repository.toggle(&undo.task) {
            Ok(reopened) => reopened,
            Err(error) => {
                self.last_completion = None;
                return self.recover_markdown_conflict(error, Some(&undo.task.key()));
            }
        };

        if let Some(timer_id) = undo.timer_id {
            let current = self.store.get_timer()?;
            if current.phase != TimerPhase::Idle && current.timer_id == timer_id {
                if let Some(previous) = undo.previous_next_task {
                    self.store.set_next_task_at(&previous, unix_now())?;
                } else {
                    self.store.clear_next_task_at(unix_now())?;
                }
            }
        }

        self.last_completion = None;
        self.reload(Some(&reopened.key()))?;
        self.message = format!("task reopened: {}", reopened.text);
        Ok(())
    }

    fn start_or_queue_selected(&mut self) -> Result<()> {
        let Some(task) = self.selected_task().cloned() else {
            return Ok(());
        };
        if task.checked {
            self.message = "completed tasks cannot be focused".into();
            return Ok(());
        }
        let Some(repository) = self.repository_for_task(&task) else {
            self.message = "task source is unavailable in this workspace".into();
            return Ok(());
        };
        let task = match repository.ensure_id(&task) {
            Ok(task) => task,
            Err(error) => {
                return self.recover_markdown_conflict(error, Some(&task.key()));
            }
        };
        let now = unix_now();
        let current = self.store.get_timer()?;
        let state = match current.phase {
            TimerPhase::Idle => {
                let preset = self.selected_preset();
                let state = self.store.start_focus_with_preset_at(
                    &task,
                    &preset,
                    self.origin_pane_id.as_deref(),
                    now,
                )?;
                spawn_worker(self.store.path(), &self.config_path, &state.timer_id)?;
                self.message = format!("{} focus started: {}", preset.name, task.text);
                state
            }
            TimerPhase::ReadyForFocus => {
                self.store.set_next_task_at(&task, now)?;
                let state = self
                    .store
                    .start_ready_focus_at(self.selected_preset().focus_seconds(), now)?;
                spawn_worker(self.store.path(), &self.config_path, &state.timer_id)?;
                self.message = format!("focus started: {}", task.text);
                state
            }
            _ => {
                self.message = format!("next focus: {}", task.text);
                self.store.set_next_task_at(&task, now)?
            }
        };
        HerdrBridge::from_environment().update_sidebar(&state, now);
        self.reload(Some(&task.key()))
    }

    fn start_focus_without_task(&mut self) -> Result<()> {
        let current = self.store.get_timer()?;
        if current.phase != TimerPhase::Idle {
            self.message = "a timer cycle is already active".into();
            return Ok(());
        }
        let now = unix_now();
        let preset = self.selected_preset();
        let state = self.store.start_empty_focus_with_preset_at(
            &preset,
            self.origin_pane_id.as_deref(),
            now,
        )?;
        spawn_worker(self.store.path(), &self.config_path, &state.timer_id)?;
        HerdrBridge::from_environment().update_sidebar(&state, now);
        self.message = "focus cycle started · choose tasks whenever you are ready".into();
        self.rebuild_rows(None)
    }

    fn pause_or_resume(&mut self) -> Result<()> {
        let now = unix_now();
        let state = self.store.get_timer()?;
        let updated = if state.phase.is_running() {
            self.message = "timer paused".into();
            self.store.pause_at(now)?
        } else if state.phase.is_paused() {
            self.message = "timer resumed".into();
            let state = self.store.resume_at(now)?;
            spawn_worker(self.store.path(), &self.config_path, &state.timer_id)?;
            state
        } else if state.phase == TimerPhase::ReadyForBreak {
            self.message = "break started".into();
            let state = self.store.start_ready_break_at(now)?;
            spawn_worker(self.store.path(), &self.config_path, &state.timer_id)?;
            state
        } else if state.phase == TimerPhase::ReadyForFocus {
            self.message = "focus started".into();
            let state = self
                .store
                .start_ready_focus_at(self.selected_preset().focus_seconds(), now)?;
            spawn_worker(self.store.path(), &self.config_path, &state.timer_id)?;
            state
        } else {
            self.message = "no timer to start or pause".into();
            return Ok(());
        };
        HerdrBridge::from_environment().update_sidebar(&updated, now);
        let key = self.selected_task().map(Task::key);
        self.rebuild_rows(key.as_deref())
    }

    fn reset_timer(&mut self) -> Result<()> {
        let now = unix_now();
        let state = self.store.reset_at(&self.config, now)?;
        if matches!(
            state.phase,
            TimerPhase::Idle | TimerPhase::ReadyForBreak | TimerPhase::ReadyForFocus
        ) {
            self.message = "no running cycle to reset".into();
            return Ok(());
        }
        spawn_worker(self.store.path(), &self.config_path, &state.timer_id)?;
        HerdrBridge::from_environment().update_sidebar(&state, now);
        self.message = "cycle reset · starting fresh".into();
        let key = self.selected_task().map(Task::key);
        self.rebuild_rows(key.as_deref())
    }

    fn stop_timer(&mut self) -> Result<()> {
        let state = self.store.get_timer()?;
        if state.phase == TimerPhase::Idle {
            self.message = "no active timer".into();
            return Ok(());
        }
        let key = self.selected_task().map(Task::key);
        let (_, pane_id) = self.store.stop_at(unix_now())?;
        HerdrBridge::from_environment().clear_sidebar(pane_id.as_deref());
        self.message = "timer stopped and focus time saved".into();
        self.rebuild_rows(key.as_deref())
    }

    pub fn sync_external_changes(&mut self) -> Result<bool> {
        let observations = self
            .current_workspace()
            .repositories
            .iter()
            .map(|repository| {
                let path = repository.path().to_path_buf();
                let observation = if self.source_errors.contains_key(&path) {
                    repository
                        .read_snapshot()
                        .map(|snapshot| snapshot.revision)
                        .map_err(|error| error.to_string())
                } else {
                    repository.revision().map_err(|error| error.to_string())
                };
                (path, observation)
            })
            .collect::<Vec<_>>();
        let changed = observations
            .into_iter()
            .filter(|(path, observation)| match observation {
                Ok(revision) => {
                    self.source_errors.contains_key(path)
                        || self.known_revisions.get(path) != Some(revision)
                }
                Err(error) => self.source_errors.get(path) != Some(error),
            })
            .collect::<Vec<_>>();
        if changed.is_empty() {
            return Ok(false);
        }
        let changed_sources = changed
            .iter()
            .map(|(path, _)| source_label(path))
            .collect::<Vec<_>>()
            .join(", ");
        if matches!(
            self.input_mode,
            InputMode::Add
                | InputMode::AddAfter
                | InputMode::Edit
                | InputMode::Estimate
                | InputMode::DeleteConfirm
        ) {
            let has_new_change = changed.iter().any(|(path, observation)| match observation {
                Ok(revision) => self.pending_revisions.get(path) != Some(revision),
                Err(error) => self.pending_source_errors.get(path) != Some(error),
            });
            if has_new_change {
                self.message = format!(
                    "{changed_sources} changed or became unavailable; current dialog was preserved"
                );
                for (path, observation) in changed {
                    match observation {
                        Ok(revision) => {
                            self.pending_revisions.insert(path, revision);
                        }
                        Err(error) => {
                            self.pending_source_errors.insert(path, error);
                        }
                    }
                }
            }
            return Ok(true);
        }

        let selected_key = self.selected_task().map(Task::key);
        self.reload(selected_key.as_deref())?;
        self.message = if self.source_errors.is_empty() {
            format!("{changed_sources} · workspace Markdown reloaded after an external change")
        } else {
            format!("{changed_sources} changed; one or more sources are unavailable")
        };
        Ok(true)
    }

    fn recover_markdown_conflict(
        &mut self,
        error: anyhow::Error,
        preserve_key: Option<&str>,
    ) -> Result<()> {
        if error.downcast_ref::<MarkdownConflict>().is_none() {
            return Err(error);
        }
        let source = self
            .selected_task()
            .map(|task| source_label(&task.source_path))
            .unwrap_or_else(|| "Markdown source".into());
        self.input_mode = InputMode::Normal;
        self.input_text.clear();
        self.input_cursor = 0;
        self.reload(preserve_key)?;
        self.message = format!("{source} · task changed externally; Markdown reloaded");
        Ok(())
    }

    fn reload(&mut self, preserve_key: Option<&str>) -> Result<()> {
        let current_key = preserve_key
            .map(str::to_owned)
            .or_else(|| self.selected_task().map(Task::key));
        let snapshots = self
            .current_workspace()
            .repositories
            .iter()
            .enumerate()
            .map(|(index, repository)| {
                (
                    index,
                    repository.path().to_path_buf(),
                    repository
                        .read_snapshot()
                        .map_err(|error| error.to_string()),
                )
            })
            .collect::<Vec<_>>();
        self.tasks.clear();
        for (index, path, snapshot) in snapshots {
            self.pending_revisions.remove(&path);
            self.pending_source_errors.remove(&path);
            match snapshot {
                Ok(snapshot) => {
                    self.source_errors.remove(&path);
                    self.known_revisions.insert(path, snapshot.revision);
                    if self.source_view.is_none() || self.source_view == Some(index) {
                        self.tasks.extend(snapshot.tasks);
                    }
                }
                Err(error) => {
                    self.known_revisions.remove(&path);
                    self.source_errors.insert(path, error);
                }
            }
        }
        self.rebuild_rows(current_key.as_deref())?;
        Ok(())
    }

    fn rebuild_rows(&mut self, selected_key: Option<&str>) -> Result<()> {
        let task_sources = self
            .tasks
            .iter()
            .filter_map(Task::history_key)
            .collect::<Vec<_>>();
        let summaries = self.store.summaries_for_task_sources(&task_sources)?;
        let state = self.store.get_timer()?;
        let query = self.search.to_lowercase();
        let visible = self
            .tasks
            .iter()
            .enumerate()
            .filter(|(_, task)| match self.filter {
                TaskFilter::Open => !task.checked,
                TaskFilter::Completed => task.checked,
                TaskFilter::All => true,
            })
            .filter(|(_, task)| {
                query.is_empty()
                    || task.text.to_lowercase().contains(&query)
                    || task
                        .heading
                        .as_deref()
                        .unwrap_or("")
                        .to_lowercase()
                        .contains(&query)
                    || (task.metadata.blocked && "blocked".contains(&query))
                    || task
                        .metadata
                        .priority
                        .is_some_and(|priority| priority.as_str().contains(&query))
                    || task
                        .metadata
                        .due_date
                        .is_some_and(|date| date.to_string().contains(&query))
            })
            .map(|(index, task)| (index, task.clone()))
            .collect::<Vec<_>>();

        self.rows.clear();
        let warnings = self.current_workspace().warnings.clone();
        let source_paths = self
            .current_workspace()
            .repositories
            .iter()
            .map(|repository| repository.path().to_path_buf())
            .collect::<Vec<_>>();
        let source_errors = self.source_errors.clone();
        for warning in &warnings {
            self.rows.push(DisplayRow {
                text: format!("◇ [unavailable] {warning}"),
                task_index: None,
                heading: true,
                blocked: false,
            });
        }
        for (index, source_path) in source_paths.iter().enumerate() {
            if self.source_view.is_some() && self.source_view != Some(index) {
                continue;
            }
            if let Some(error) = source_errors.get(source_path) {
                self.rows.push(DisplayRow {
                    text: format!("◇ [unavailable] {} · {error}", source_path.display()),
                    task_index: None,
                    heading: true,
                    blocked: false,
                });
            } else if !source_path.exists() {
                self.rows.push(DisplayRow {
                    text: format!("◇ [missing] {}", source_path.display()),
                    task_index: None,
                    heading: true,
                    blocked: false,
                });
            }
        }
        let aggregate = self.source_view.is_none() && source_paths.len() > 1;
        let mut heading_order = Vec::new();
        let mut grouped: HashMap<String, Vec<(usize, Task)>> = HashMap::new();
        for (index, task) in visible {
            let heading = task.heading.clone().unwrap_or_else(|| "Tasks".into());
            let heading = if aggregate {
                format!("{} › {heading}", source_label(&task.source_path))
            } else {
                heading
            };
            if !grouped.contains_key(&heading) {
                heading_order.push(heading.clone());
            }
            grouped.entry(heading).or_default().push((index, task));
        }
        let mut selected_row = None;
        for heading in heading_order {
            let tasks = grouped.remove(&heading).unwrap_or_default();
            self.rows.push(DisplayRow {
                text: format!("◇ {heading} ({})", tasks.len()),
                task_index: None,
                heading: true,
                blocked: false,
            });
            for (position, (task_index, task)) in tasks.iter().enumerate() {
                let is_next_task = task.task_id.is_some()
                    && state.next_task_id == task.task_id
                    && state.next_source_path.as_deref()
                        == Some(task.source_path.to_string_lossy().as_ref());
                let marker = if task.metadata.blocked {
                    "!"
                } else if is_next_task {
                    match state.phase {
                        // The focus timer is deliberately task-independent;
                        // this marker only identifies the task queued for after a break.
                        TimerPhase::Focus | TimerPhase::PausedFocus => "○",
                        TimerPhase::ReadyForBreak
                        | TimerPhase::Break
                        | TimerPhase::PausedBreak
                        | TimerPhase::ReadyForFocus => "▷",
                        _ if task.checked => "✓",
                        _ => "○",
                    }
                } else if task.checked {
                    "✓"
                } else {
                    "○"
                };
                let branch = if position + 1 == tasks.len() {
                    "└─"
                } else {
                    "├─"
                };
                let total = task
                    .history_key()
                    .as_ref()
                    .and_then(|key| summaries.get(key))
                    .map_or(0, |summary| summary.total_seconds);
                let suffix = task_row_suffix(task, total, self.selected_preset().focus_seconds());
                let row_index = self.rows.len();
                if selected_key == Some(task.key().as_str()) {
                    selected_row = Some(row_index);
                }
                let nesting = " ".repeat(task.indent.min(20));
                self.rows.push(DisplayRow {
                    text: format!("  {nesting}{branch} {marker} {}{suffix}", task.text),
                    task_index: Some(*task_index),
                    heading: false,
                    blocked: task.metadata.blocked,
                });
            }
        }
        let first_task = self.rows.iter().position(|row| row.task_index.is_some());
        self.list_state.select(selected_row.or(first_task));
        Ok(())
    }

    fn selected_task(&self) -> Option<&Task> {
        let row = self
            .list_state
            .selected()
            .and_then(|index| self.rows.get(index))?;
        row.task_index.and_then(|index| self.tasks.get(index))
    }

    fn next_open_task_after(&self, completed_key: &str) -> Option<Task> {
        let completed_index = self
            .tasks
            .iter()
            .position(|task| task.key() == completed_key)?;
        self.tasks
            .iter()
            .skip(completed_index + 1)
            .chain(self.tasks.iter().take(completed_index))
            .find(|task| !task.checked)
            .cloned()
    }

    fn visible_task_count(&self) -> usize {
        self.rows
            .iter()
            .filter(|row| row.task_index.is_some())
            .count()
    }
}

pub fn run_ui(
    repository: MarkdownTaskRepository,
    store: FocusStore,
    config: AppConfig,
    config_path: &Path,
    origin_pane_id: Option<String>,
) -> Result<()> {
    let state = store.get_timer()?;
    if state.phase.is_running() {
        spawn_worker(store.path(), config_path, &state.timer_id)?;
    }
    let mut app = UiApp::new_with_config_path(
        repository,
        store,
        config,
        config_path.to_path_buf(),
        origin_pane_id,
    )?;
    run_app_loop(&mut app)
}

pub fn run_workspace_ui(
    workspaces: Vec<WorkspaceDefinition>,
    initial_workspace: usize,
    store: FocusStore,
    config: AppConfig,
    config_path: &Path,
    origin_pane_id: Option<String>,
) -> Result<()> {
    let state = store.get_timer()?;
    if state.phase.is_running() {
        spawn_worker(store.path(), config_path, &state.timer_id)?;
    }
    let mut app = UiApp::new_with_workspaces(
        workspaces,
        initial_workspace,
        store,
        config,
        config_path.to_path_buf(),
        origin_pane_id,
    )?;
    run_app_loop(&mut app)
}

fn run_app_loop(app: &mut UiApp) -> Result<()> {
    ratatui::run(|terminal| -> Result<()> {
        let mut last_file_check = Instant::now();
        loop {
            if last_file_check.elapsed() >= Duration::from_millis(500) {
                app.sync_external_changes()?;
                last_file_check = Instant::now();
            }
            terminal.draw(|frame| app.render(frame))?;
            if event::poll(Duration::from_millis(200))?
                && let Event::Key(key) = event::read()?
                && app.handle_key(key)? == UiOutcome::Quit
            {
                return Ok(());
            }
        }
    })
}

fn timer_line(state: &TimerState, config: &AppConfig, now: f64) -> String {
    let countdown = format_countdown(state.seconds_left(now));
    let preset_name = if state.preset_name.is_empty() {
        config.default_preset.as_str()
    } else {
        state.preset_name.as_str()
    };
    let cycles_before_long_break = if state.preset_cycles_before_long_break > 0 {
        state.preset_cycles_before_long_break
    } else {
        config.cycles_before_long_break
    };
    match state.phase {
        TimerPhase::Focus | TimerPhase::PausedFocus => format!(
            "● {} · {countdown} · {preset_name} · {}→{} · cycle {}/{}",
            if state.phase == TimerPhase::PausedFocus {
                "PAUSED"
            } else {
                "FOCUS"
            },
            format_clock(state.session_started_at),
            format_clock(state.deadline_at.or(Some(now + state.remaining_seconds))),
            state.cycle + 1,
            cycles_before_long_break,
        ),
        TimerPhase::Break | TimerPhase::PausedBreak => format!(
            "◇ {} · {countdown} · {preset_name} · next: {} · {}→{}",
            if state.phase == TimerPhase::PausedBreak {
                "BREAK PAUSED"
            } else {
                "COFFEE BREAK"
            },
            state.next_task_text.as_deref().unwrap_or(""),
            format_clock(state.phase_started_at),
            format_clock(state.deadline_at.or(Some(now + state.remaining_seconds))),
        ),
        TimerPhase::ReadyForBreak => {
            format!("◇ READY FOR BREAK · {countdown} · {preset_name} · press p to start")
        }
        TimerPhase::ReadyForFocus => format!(
            "✓ READY FOR FOCUS · {countdown} · {preset_name} · next: {} · press p to start",
            state.next_task_text.as_deref().unwrap_or("choose a task")
        ),
        TimerPhase::Idle => format!(
            "◇ READY · preset {preset_name} · select a task and press enter · P changes preset"
        ),
    }
}

fn source_label(path: &Path) -> String {
    let file = path
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or("Markdown");
    path.parent()
        .and_then(Path::file_name)
        .and_then(|name| name.to_str())
        .map_or_else(|| file.to_owned(), |parent| format!("{parent}/{file}"))
}

fn centered_modal(area: Rect, max_width: u16, max_height: u16) -> Rect {
    let width = area
        .width
        .saturating_sub(2)
        .min(max_width)
        .max(1)
        .min(area.width);
    let height = area
        .height
        .saturating_sub(2)
        .min(max_height)
        .max(1)
        .min(area.height);
    Rect::new(
        area.x + area.width.saturating_sub(width) / 2,
        area.y + area.height.saturating_sub(height) / 2,
        width,
        height,
    )
}

fn task_row_suffix(task: &Task, total_seconds: u64, focus_seconds: u64) -> String {
    let mut parts = Vec::new();
    if let Some(estimate) = task.metadata.estimate_pomodoros {
        parts.push(format!(
            "{}/{}p",
            format_pomodoro_count(total_seconds, focus_seconds),
            estimate
        ));
    } else if total_seconds > 0 {
        parts.push(format_duration(total_seconds));
    }
    if let Some(priority) = task.metadata.priority {
        parts.push(priority.as_str().to_owned());
    }
    if let Some(due_date) = task.metadata.due_date {
        parts.push(format!("due {due_date}"));
    }
    if parts.is_empty() {
        String::new()
    } else {
        format!(" · {}", parts.join(" · "))
    }
}

fn task_plan_status(task: &Task, summary: &TaskFocusSummary, focus_seconds: u64) -> String {
    let mut parts = Vec::new();
    if task.metadata.blocked {
        parts.push("blocked".into());
    }
    if let Some(priority) = task.metadata.priority {
        parts.push(format!("priority {}", priority.as_str()));
    }
    if let Some(due_date) = task.metadata.due_date {
        parts.push(format!("due {due_date}"));
    }
    if let Some(estimate) = task.metadata.estimate_pomodoros {
        parts.push(format!(
            "estimate {}/{}p",
            format_pomodoro_count(summary.total_seconds, focus_seconds),
            estimate
        ));
        if task.checked {
            let estimated_seconds = focus_seconds.saturating_mul(u64::from(estimate));
            let variance = i128::from(summary.total_seconds) - i128::from(estimated_seconds);
            parts.push(match variance.cmp(&0) {
                std::cmp::Ordering::Greater => {
                    format!("{} over estimate", format_duration(variance as u64))
                }
                std::cmp::Ordering::Less => {
                    format!("{} under estimate", format_duration((-variance) as u64))
                }
                std::cmp::Ordering::Equal => "on estimate".into(),
            });
        }
    }
    if parts.is_empty() {
        String::new()
    } else {
        format!(" · {}", parts.join(" · "))
    }
}

fn format_pomodoro_count(seconds: u64, focus_seconds: u64) -> String {
    let value = seconds as f64 / focus_seconds.max(1) as f64;
    if (value - value.round()).abs() < 0.05 {
        format!("{value:.0}")
    } else {
        format!("{value:.1}")
    }
}

fn char_byte_index(text: &str, character_index: usize) -> usize {
    text.char_indices()
        .nth(character_index)
        .map_or(text.len(), |(index, _)| index)
}

fn format_countdown(seconds: u64) -> String {
    format!("{:02}:{:02}", seconds / 60, seconds % 60)
}

fn format_duration(seconds: u64) -> String {
    if seconds == 0 {
        return "—".into();
    }
    let minutes = ((seconds as f64) / 60.0).round().max(1.0) as u64;
    if minutes >= 60 {
        format!("{}h {:02}m", minutes / 60, minutes % 60)
    } else {
        format!("{minutes}m")
    }
}

fn format_clock(timestamp: Option<f64>) -> String {
    timestamp
        .and_then(|value| DateTime::from_timestamp(value as i64, 0))
        .map(|utc| utc.with_timezone(&Local).format("%H:%M").to_string())
        .unwrap_or_else(|| "--:--".into())
}
