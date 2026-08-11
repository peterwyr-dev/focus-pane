use crate::{
    config::{AppConfig, FocusPreset},
    models::{FocusSession, Task, TaskFocusSummary, TimerPhase, TimerState, TimerTransition},
};
use anyhow::{Context, Result, bail};
use rusqlite::{Connection, Transaction, TransactionBehavior, params};
use std::{
    collections::HashSet,
    fs,
    path::{Path, PathBuf},
};
use uuid::Uuid;

#[derive(Debug, Clone)]
pub struct FocusStore {
    path: PathBuf,
}

impl FocusStore {
    pub fn open(path: &Path) -> Result<Self> {
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent)?;
        }
        let store = Self {
            path: path.to_path_buf(),
        };
        store.initialize()?;
        Ok(store)
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    fn connect(&self) -> Result<Connection> {
        let connection = Connection::open(&self.path)?;
        connection.pragma_update(None, "journal_mode", "WAL")?;
        connection.busy_timeout(std::time::Duration::from_secs(5))?;
        Ok(connection)
    }

    fn initialize(&self) -> Result<()> {
        let mut connection = self.connect()?;
        connection.execute_batch(
            "
            CREATE TABLE IF NOT EXISTS timer_state (
                singleton INTEGER PRIMARY KEY CHECK (singleton = 1),
                phase TEXT NOT NULL DEFAULT 'idle',
                timer_id TEXT NOT NULL DEFAULT '',
                task_id TEXT,
                task_text TEXT,
                source_path TEXT,
                source_line INTEGER,
                next_task_id TEXT,
                next_task_text TEXT,
                next_source_path TEXT,
                next_source_line INTEGER,
                origin_pane_id TEXT,
                preset_name TEXT NOT NULL DEFAULT '',
                preset_focus_seconds INTEGER NOT NULL DEFAULT 0,
                preset_break_seconds INTEGER NOT NULL DEFAULT 0,
                preset_long_break_seconds INTEGER NOT NULL DEFAULT 0,
                preset_cycles_before_long_break INTEGER NOT NULL DEFAULT 0,
                cycle INTEGER NOT NULL DEFAULT 0,
                session_started_at REAL,
                phase_started_at REAL,
                deadline_at REAL,
                remaining_seconds REAL NOT NULL DEFAULT 0,
                accumulated_seconds REAL NOT NULL DEFAULT 0,
                updated_at REAL NOT NULL DEFAULT 0
            );
            INSERT OR IGNORE INTO timer_state (singleton) VALUES (1);
            CREATE TABLE IF NOT EXISTS focus_sessions (
                id INTEGER PRIMARY KEY AUTOINCREMENT,
                task_id TEXT NOT NULL,
                task_text TEXT NOT NULL,
                source_path TEXT NOT NULL,
                source_line INTEGER,
                started_at REAL NOT NULL,
                ended_at REAL NOT NULL,
                duration_seconds INTEGER NOT NULL,
                outcome TEXT NOT NULL CHECK (outcome IN ('completed', 'stopped'))
            );
            CREATE INDEX IF NOT EXISTS focus_sessions_task_id
                ON focus_sessions(task_id);
            CREATE INDEX IF NOT EXISTS focus_sessions_started_at
                ON focus_sessions(started_at);
            ",
        )?;
        migrate_timer_state(&mut connection)?;
        Ok(())
    }

    pub fn get_timer(&self) -> Result<TimerState> {
        read_state(&self.connect()?)
    }

    pub fn start_focus_at(
        &self,
        task: &Task,
        focus_seconds: u64,
        origin_pane_id: Option<&str>,
        now: f64,
    ) -> Result<TimerState> {
        self.start_focus_internal(task, focus_seconds, None, origin_pane_id, now)
    }

    pub fn start_focus_with_preset_at(
        &self,
        task: &Task,
        preset: &FocusPreset,
        origin_pane_id: Option<&str>,
        now: f64,
    ) -> Result<TimerState> {
        self.start_focus_internal(
            task,
            preset.focus_seconds(),
            Some(preset),
            origin_pane_id,
            now,
        )
    }

    fn start_focus_internal(
        &self,
        task: &Task,
        focus_seconds: u64,
        preset: Option<&FocusPreset>,
        origin_pane_id: Option<&str>,
        now: f64,
    ) -> Result<TimerState> {
        let mut connection = self.connect()?;
        let transaction = connection.transaction_with_behavior(TransactionBehavior::Immediate)?;
        let current = read_state(&transaction)?;
        if !matches!(current.phase, TimerPhase::Idle | TimerPhase::ReadyForFocus) {
            bail!("A focus or break timer is already active");
        }
        let preset_name = preset.map_or("", |preset| preset.name.as_str());
        let cycle = if preset.is_some() && current.preset_name != preset_name {
            0
        } else {
            current.cycle
        };
        transaction.execute(
            "
            UPDATE timer_state SET
                phase = 'focus', timer_id = ?1, task_id = ?2, task_text = ?3,
                source_path = ?4, source_line = ?5,
                next_task_id = ?2, next_task_text = ?3,
                next_source_path = ?4, next_source_line = ?5,
                origin_pane_id = ?6, cycle = ?7,
                preset_name = ?8, preset_focus_seconds = ?9,
                preset_break_seconds = ?10, preset_long_break_seconds = ?11,
                preset_cycles_before_long_break = ?12,
                session_started_at = ?13, phase_started_at = ?13, deadline_at = ?14,
                remaining_seconds = ?15, accumulated_seconds = 0, updated_at = ?13
            WHERE singleton = 1
            ",
            params![
                new_id(),
                task.task_id.clone().unwrap_or_else(|| task.key()),
                task.text,
                task.source_path.to_string_lossy(),
                task.line_number as i64,
                origin_pane_id,
                cycle,
                preset_name,
                focus_seconds as i64,
                preset.map_or(0, |preset| preset.break_seconds()) as i64,
                preset.map_or(0, |preset| preset.long_break_seconds()) as i64,
                preset.map_or(0, |preset| preset.cycles_before_long_break) as i64,
                now,
                now + focus_seconds as f64,
                focus_seconds as f64,
            ],
        )?;
        let state = read_state(&transaction)?;
        transaction.commit()?;
        Ok(state)
    }

    /// Updates the task shown as the next thing to work on.  It is display
    /// context, not the owner of the currently running Pomodoro.
    pub fn set_next_task_at(&self, task: &Task, now: f64) -> Result<TimerState> {
        let mut connection = self.connect()?;
        let transaction = connection.transaction_with_behavior(TransactionBehavior::Immediate)?;
        let current = read_state(&transaction)?;
        if matches!(current.phase, TimerPhase::Idle) {
            bail!("Start a focus cycle before choosing the next task");
        }
        transaction.execute(
            "
            UPDATE timer_state SET next_task_id = ?1, next_task_text = ?2,
                next_source_path = ?3, next_source_line = ?4, updated_at = ?5
            WHERE singleton = 1
            ",
            params![
                task.task_id.clone().unwrap_or_else(|| task.key()),
                task.text,
                task.source_path.to_string_lossy(),
                task.line_number as i64,
                now,
            ],
        )?;
        let state = read_state(&transaction)?;
        transaction.commit()?;
        Ok(state)
    }

    pub fn clear_next_task_at(&self, now: f64) -> Result<TimerState> {
        let mut connection = self.connect()?;
        let transaction = connection.transaction_with_behavior(TransactionBehavior::Immediate)?;
        let current = read_state(&transaction)?;
        if matches!(current.phase, TimerPhase::Idle) {
            transaction.commit()?;
            return Ok(current);
        }
        transaction.execute(
            "
            UPDATE timer_state SET next_task_id = NULL, next_task_text = NULL,
                next_source_path = NULL, next_source_line = NULL, updated_at = ?1
            WHERE singleton = 1
            ",
            params![now],
        )?;
        let state = read_state(&transaction)?;
        transaction.commit()?;
        Ok(state)
    }

    /// Backwards-compatible task-independent timer entry point.
    pub fn start_empty_focus_at(
        &self,
        focus_seconds: u64,
        origin_pane_id: Option<&str>,
        now: f64,
    ) -> Result<TimerState> {
        self.start_empty_focus_internal(focus_seconds, None, origin_pane_id, now)
    }

    pub fn start_empty_focus_with_preset_at(
        &self,
        preset: &FocusPreset,
        origin_pane_id: Option<&str>,
        now: f64,
    ) -> Result<TimerState> {
        self.start_empty_focus_internal(preset.focus_seconds(), Some(preset), origin_pane_id, now)
    }

    fn start_empty_focus_internal(
        &self,
        focus_seconds: u64,
        preset: Option<&FocusPreset>,
        origin_pane_id: Option<&str>,
        now: f64,
    ) -> Result<TimerState> {
        let mut connection = self.connect()?;
        let transaction = connection.transaction_with_behavior(TransactionBehavior::Immediate)?;
        let current = read_state(&transaction)?;
        if !matches!(current.phase, TimerPhase::Idle | TimerPhase::ReadyForFocus) {
            bail!("A focus or break timer is already active");
        }
        let preset_name = preset.map_or("", |preset| preset.name.as_str());
        let cycle = if preset.is_some() && current.preset_name != preset_name {
            0
        } else {
            current.cycle
        };
        transaction.execute(
            "
            UPDATE timer_state SET phase = 'focus', timer_id = ?1,
                task_id = NULL, task_text = NULL, source_path = NULL, source_line = NULL,
                next_task_id = NULL, next_task_text = NULL,
                next_source_path = NULL, next_source_line = NULL,
                origin_pane_id = ?2, cycle = ?3,
                preset_name = ?4, preset_focus_seconds = ?5,
                preset_break_seconds = ?6, preset_long_break_seconds = ?7,
                preset_cycles_before_long_break = ?8,
                session_started_at = ?9, phase_started_at = ?9, deadline_at = ?10,
                remaining_seconds = ?11, accumulated_seconds = 0, updated_at = ?9
            WHERE singleton = 1
            ",
            params![
                new_id(),
                origin_pane_id,
                cycle,
                preset_name,
                focus_seconds as i64,
                preset.map_or(0, |preset| preset.break_seconds()) as i64,
                preset.map_or(0, |preset| preset.long_break_seconds()) as i64,
                preset.map_or(0, |preset| preset.cycles_before_long_break) as i64,
                now,
                now + focus_seconds as f64,
                focus_seconds as f64,
            ],
        )?;
        let state = read_state(&transaction)?;
        transaction.commit()?;
        Ok(state)
    }

    pub fn queue_next_task_at(&self, task: &Task, now: f64) -> Result<TimerState> {
        let current = self.get_timer()?;
        if !matches!(
            current.phase,
            TimerPhase::ReadyForBreak | TimerPhase::Break | TimerPhase::PausedBreak
        ) {
            bail!("The next task can only be queued during a break");
        }
        self.set_next_task_at(task, now)
    }

    pub fn select_preset_at(&self, preset: &FocusPreset, now: f64) -> Result<TimerState> {
        let mut connection = self.connect()?;
        let transaction = connection.transaction_with_behavior(TransactionBehavior::Immediate)?;
        let current = read_state(&transaction)?;
        if current.phase != TimerPhase::Idle {
            bail!("A preset can only be selected while the timer is idle");
        }
        transaction.execute(
            "UPDATE timer_state SET preset_name = ?1,
                preset_focus_seconds = ?2, preset_break_seconds = ?3,
                preset_long_break_seconds = ?4,
                preset_cycles_before_long_break = ?5,
                cycle = 0, updated_at = ?6 WHERE singleton = 1",
            params![
                preset.name,
                preset.focus_seconds() as i64,
                preset.break_seconds() as i64,
                preset.long_break_seconds() as i64,
                preset.cycles_before_long_break as i64,
                now,
            ],
        )?;
        let state = read_state(&transaction)?;
        transaction.commit()?;
        Ok(state)
    }

    pub fn start_ready_break_at(&self, now: f64) -> Result<TimerState> {
        let mut connection = self.connect()?;
        let transaction = connection.transaction_with_behavior(TransactionBehavior::Immediate)?;
        let current = read_state(&transaction)?;
        if current.phase != TimerPhase::ReadyForBreak {
            bail!("Break can only start after a completed focus");
        }
        transaction.execute(
            "UPDATE timer_state SET phase = 'break', timer_id = ?1,
                phase_started_at = ?2, deadline_at = ?3, updated_at = ?2
             WHERE singleton = 1",
            params![new_id(), now, now + current.remaining_seconds],
        )?;
        let state = read_state(&transaction)?;
        transaction.commit()?;
        Ok(state)
    }

    pub fn start_ready_focus_at(&self, focus_seconds: u64, now: f64) -> Result<TimerState> {
        let mut connection = self.connect()?;
        let transaction = connection.transaction_with_behavior(TransactionBehavior::Immediate)?;
        let current = read_state(&transaction)?;
        if current.phase != TimerPhase::ReadyForFocus {
            bail!("Focus can only start after a completed break");
        }
        let focus_seconds = if current.preset_focus_seconds > 0 {
            current.preset_focus_seconds
        } else {
            focus_seconds
        };
        transaction.execute(
            "
            UPDATE timer_state SET phase = 'focus', timer_id = ?1,
                task_id = next_task_id, task_text = next_task_text,
                source_path = next_source_path, source_line = next_source_line,
                session_started_at = ?2, phase_started_at = ?2, deadline_at = ?3,
                remaining_seconds = ?4, accumulated_seconds = 0, updated_at = ?2
            WHERE singleton = 1
            ",
            params![
                new_id(),
                now,
                now + focus_seconds as f64,
                focus_seconds as f64
            ],
        )?;
        let state = read_state(&transaction)?;
        transaction.commit()?;
        Ok(state)
    }

    pub fn reset_at(&self, config: &AppConfig, now: f64) -> Result<TimerState> {
        let mut connection = self.connect()?;
        let transaction = connection.transaction_with_behavior(TransactionBehavior::Immediate)?;
        let current = read_state(&transaction)?;
        let (phase, seconds) = match current.phase {
            TimerPhase::Focus | TimerPhase::PausedFocus => (
                TimerPhase::Focus,
                if current.preset_focus_seconds > 0 {
                    current.preset_focus_seconds
                } else {
                    config.focus_seconds()
                },
            ),
            TimerPhase::Break | TimerPhase::PausedBreak => {
                // A completed long break resets the cycle counter to zero.
                let seconds = if current.cycle == 0 {
                    if current.preset_long_break_seconds > 0 {
                        current.preset_long_break_seconds
                    } else {
                        config.long_break_seconds()
                    }
                } else if current.preset_break_seconds > 0 {
                    current.preset_break_seconds
                } else {
                    config.break_seconds()
                };
                (TimerPhase::Break, seconds)
            }
            TimerPhase::Idle | TimerPhase::ReadyForBreak | TimerPhase::ReadyForFocus => {
                transaction.commit()?;
                return Ok(current);
            }
        };
        transaction.execute(
            "
            UPDATE timer_state SET phase = ?1, timer_id = ?2,
                session_started_at = ?3, phase_started_at = ?3, deadline_at = ?4,
                remaining_seconds = ?5, accumulated_seconds = 0, updated_at = ?3
            WHERE singleton = 1
            ",
            params![
                phase.as_str(),
                new_id(),
                now,
                now + seconds as f64,
                seconds as f64,
            ],
        )?;
        let state = read_state(&transaction)?;
        transaction.commit()?;
        Ok(state)
    }

    pub fn pause_at(&self, now: f64) -> Result<TimerState> {
        let mut connection = self.connect()?;
        let transaction = connection.transaction_with_behavior(TransactionBehavior::Immediate)?;
        let current = read_state(&transaction)?;
        if !matches!(current.phase, TimerPhase::Focus | TimerPhase::Break) {
            transaction.commit()?;
            return Ok(current);
        }
        let deadline = current
            .deadline_at
            .context("running timer has no deadline")?;
        let elapsed = (now - current.phase_started_at.unwrap_or(now)).max(0.0);
        let accumulated = if current.phase == TimerPhase::Focus {
            current.accumulated_seconds + elapsed
        } else {
            current.accumulated_seconds
        };
        let remaining = (deadline - now).max(0.0);
        let paused_phase = if current.phase == TimerPhase::Focus {
            TimerPhase::PausedFocus
        } else {
            TimerPhase::PausedBreak
        };
        transaction.execute(
            "
            UPDATE timer_state SET phase = ?1, timer_id = ?2, deadline_at = NULL,
                remaining_seconds = ?3, accumulated_seconds = ?4, updated_at = ?5
            WHERE singleton = 1
            ",
            params![paused_phase.as_str(), new_id(), remaining, accumulated, now],
        )?;
        let state = read_state(&transaction)?;
        transaction.commit()?;
        Ok(state)
    }

    pub fn resume_at(&self, now: f64) -> Result<TimerState> {
        let mut connection = self.connect()?;
        let transaction = connection.transaction_with_behavior(TransactionBehavior::Immediate)?;
        let current = read_state(&transaction)?;
        let phase = match current.phase {
            TimerPhase::PausedFocus => TimerPhase::Focus,
            TimerPhase::PausedBreak => TimerPhase::Break,
            _ => {
                transaction.commit()?;
                return Ok(current);
            }
        };
        transaction.execute(
            "
            UPDATE timer_state SET phase = ?1, timer_id = ?2, phase_started_at = ?3,
                deadline_at = ?4, updated_at = ?3 WHERE singleton = 1
            ",
            params![
                phase.as_str(),
                new_id(),
                now,
                now + current.remaining_seconds,
            ],
        )?;
        let state = read_state(&transaction)?;
        transaction.commit()?;
        Ok(state)
    }

    pub fn stop_at(&self, now: f64) -> Result<(TimerState, Option<String>)> {
        let mut connection = self.connect()?;
        let transaction = connection.transaction_with_behavior(TransactionBehavior::Immediate)?;
        let current = read_state(&transaction)?;
        let pane_id = current.origin_pane_id.clone();
        if matches!(current.phase, TimerPhase::Focus | TimerPhase::PausedFocus) {
            let mut duration = current.accumulated_seconds;
            if current.phase == TimerPhase::Focus {
                duration += (now - current.phase_started_at.unwrap_or(now)).max(0.0);
            }
            insert_session(
                &transaction,
                &current,
                now,
                duration.round() as u64,
                "stopped",
            )?;
        }
        transaction.execute(
            "
            UPDATE timer_state SET phase = 'idle', timer_id = ?1,
                task_id = NULL, task_text = NULL, source_path = NULL, source_line = NULL,
                next_task_id = NULL, next_task_text = NULL,
                next_source_path = NULL, next_source_line = NULL,
                origin_pane_id = NULL, session_started_at = NULL, phase_started_at = NULL,
                deadline_at = NULL, remaining_seconds = 0, accumulated_seconds = 0,
                updated_at = ?2 WHERE singleton = 1
            ",
            params![new_id(), now],
        )?;
        let state = read_state(&transaction)?;
        transaction.commit()?;
        Ok((state, pane_id))
    }

    pub fn complete_due_at(
        &self,
        expected_timer_id: &str,
        config: &AppConfig,
        now: f64,
    ) -> Result<Option<TimerTransition>> {
        let mut connection = self.connect()?;
        let transaction = connection.transaction_with_behavior(TransactionBehavior::Immediate)?;
        let current = read_state(&transaction)?;
        if current.timer_id != expected_timer_id
            || !matches!(current.phase, TimerPhase::Focus | TimerPhase::Break)
            || current.deadline_at.is_none_or(|deadline| deadline > now)
        {
            transaction.commit()?;
            return Ok(None);
        }

        if current.phase == TimerPhase::Focus {
            let deadline = current.deadline_at.unwrap_or(now);
            let duration = current.accumulated_seconds
                + (deadline - current.phase_started_at.unwrap_or(deadline)).max(0.0);
            insert_session(
                &transaction,
                &current,
                deadline,
                duration.round() as u64,
                "completed",
            )?;
            let completed_cycles = current.cycle + 1;
            let cycles_before_long_break = if current.preset_cycles_before_long_break > 0 {
                current.preset_cycles_before_long_break
            } else {
                config.cycles_before_long_break
            };
            let long_break = completed_cycles >= cycles_before_long_break;
            let next_cycle = if long_break { 0 } else { completed_cycles };
            let break_seconds = if long_break {
                if current.preset_long_break_seconds > 0 {
                    current.preset_long_break_seconds
                } else {
                    config.long_break_seconds()
                }
            } else if current.preset_break_seconds > 0 {
                current.preset_break_seconds
            } else {
                config.break_seconds()
            };
            let break_phase = if config.auto_start_break {
                TimerPhase::Break
            } else {
                TimerPhase::ReadyForBreak
            };
            let break_started_at = config.auto_start_break.then_some(now);
            let break_deadline = config
                .auto_start_break
                .then_some(now + break_seconds as f64);
            transaction.execute(
                "
                UPDATE timer_state SET phase = ?1, timer_id = ?2, cycle = ?3,
                    session_started_at = NULL, phase_started_at = ?4, deadline_at = ?5,
                    remaining_seconds = ?6, accumulated_seconds = 0, updated_at = ?7
                WHERE singleton = 1
                ",
                params![
                    break_phase.as_str(),
                    new_id(),
                    next_cycle,
                    break_started_at,
                    break_deadline,
                    break_seconds as f64,
                    now,
                ],
            )?;
            let state = read_state(&transaction)?;
            transaction.commit()?;
            return Ok(Some(TimerTransition {
                kind: "focus_completed",
                state,
            }));
        }

        let focus_seconds = if current.preset_focus_seconds > 0 {
            current.preset_focus_seconds
        } else {
            config.focus_seconds()
        };
        transaction.execute(
            "
            UPDATE timer_state SET phase = 'ready_for_focus', timer_id = ?1,
                session_started_at = NULL, phase_started_at = NULL, deadline_at = NULL,
                remaining_seconds = ?2, accumulated_seconds = 0, updated_at = ?3
            WHERE singleton = 1
            ",
            params![new_id(), focus_seconds as f64, now],
        )?;
        let state = read_state(&transaction)?;
        transaction.commit()?;
        Ok(Some(TimerTransition {
            kind: "break_completed",
            state,
        }))
    }

    pub fn recent_sessions(&self, task_id: &str, limit: usize) -> Result<Vec<FocusSession>> {
        self.recent_sessions_matching(task_id, None, limit)
    }

    pub fn recent_sessions_for_source(
        &self,
        task_id: &str,
        source_path: &str,
        limit: usize,
    ) -> Result<Vec<FocusSession>> {
        self.recent_sessions_matching(task_id, Some(source_path), limit)
    }

    fn recent_sessions_matching(
        &self,
        task_id: &str,
        source_path: Option<&str>,
        limit: usize,
    ) -> Result<Vec<FocusSession>> {
        let connection = self.connect()?;
        let sql = if source_path.is_some() {
            "
            SELECT task_id, task_text, source_path, source_line,
                   started_at, ended_at, duration_seconds, outcome
            FROM focus_sessions WHERE task_id = ?1 AND source_path = ?2
            ORDER BY started_at DESC LIMIT ?3
            "
        } else {
            "
            SELECT task_id, task_text, source_path, source_line,
                   started_at, ended_at, duration_seconds, outcome
            FROM focus_sessions WHERE task_id = ?1
            ORDER BY started_at DESC LIMIT ?2
            "
        };
        let mut statement = connection.prepare(sql)?;
        let map_row = |row: &rusqlite::Row<'_>| {
            Ok(FocusSession {
                task_id: row.get(0)?,
                task_text: row.get(1)?,
                source_path: row.get(2)?,
                source_line: row.get::<_, Option<i64>>(3)?.map(|value| value as usize),
                started_at: row.get(4)?,
                ended_at: row.get(5)?,
                duration_seconds: row.get::<_, i64>(6)? as u64,
                outcome: row.get(7)?,
            })
        };
        let rows = if let Some(source_path) = source_path {
            statement.query_map(params![task_id, source_path, limit as i64], map_row)?
        } else {
            statement.query_map(params![task_id, limit as i64], map_row)?
        };
        Ok(rows.collect::<rusqlite::Result<Vec<_>>>()?)
    }

    pub fn summaries_for_task_sources(
        &self,
        tasks: &[(String, String)],
    ) -> Result<std::collections::HashMap<(String, String), TaskFocusSummary>> {
        let mut summaries = std::collections::HashMap::new();
        if tasks.is_empty() {
            return Ok(summaries);
        }
        let connection = self.connect()?;
        let placeholders = std::iter::repeat_n("(?, ?)", tasks.len())
            .collect::<Vec<_>>()
            .join(",");
        let sql = format!(
            "SELECT task_id, source_path,
                    COALESCE(SUM(duration_seconds), 0),
                    COALESCE(SUM(CASE WHEN outcome = 'completed'
                                      THEN duration_seconds ELSE 0 END), 0),
                    COALESCE(SUM(CASE WHEN outcome = 'stopped'
                                      THEN duration_seconds ELSE 0 END), 0),
                    SUM(CASE WHEN outcome = 'completed' THEN 1 ELSE 0 END),
                    SUM(CASE WHEN outcome = 'stopped' THEN 1 ELSE 0 END)
             FROM focus_sessions WHERE (task_id, source_path) IN ({placeholders})
             GROUP BY task_id, source_path"
        );
        let mut statement = connection.prepare(&sql)?;
        let parameters = rusqlite::params_from_iter(
            tasks
                .iter()
                .flat_map(|(task_id, source_path)| [task_id.as_str(), source_path.as_str()]),
        );
        let rows = statement.query_map(parameters, |row| {
            Ok((
                (row.get::<_, String>(0)?, row.get::<_, String>(1)?),
                TaskFocusSummary {
                    total_seconds: row.get::<_, i64>(2)? as u64,
                    completed_seconds: row.get::<_, i64>(3)? as u64,
                    stopped_seconds: row.get::<_, i64>(4)? as u64,
                    completed_sessions: row.get::<_, i64>(5)? as u32,
                    stopped_sessions: row.get::<_, i64>(6)? as u32,
                },
            ))
        })?;
        for row in rows {
            let (key, summary) = row?;
            summaries.insert(key, summary);
        }
        Ok(summaries)
    }

    pub fn summaries_for_tasks(
        &self,
        task_ids: &[String],
    ) -> Result<std::collections::HashMap<String, TaskFocusSummary>> {
        let mut summaries = std::collections::HashMap::new();
        if task_ids.is_empty() {
            return Ok(summaries);
        }
        let connection = self.connect()?;
        let placeholders = std::iter::repeat_n("?", task_ids.len())
            .collect::<Vec<_>>()
            .join(",");
        let sql = format!(
            "SELECT task_id,
                    COALESCE(SUM(duration_seconds), 0),
                    COALESCE(SUM(CASE WHEN outcome = 'completed'
                                      THEN duration_seconds ELSE 0 END), 0),
                    COALESCE(SUM(CASE WHEN outcome = 'stopped'
                                      THEN duration_seconds ELSE 0 END), 0),
                    SUM(CASE WHEN outcome = 'completed' THEN 1 ELSE 0 END),
                    SUM(CASE WHEN outcome = 'stopped' THEN 1 ELSE 0 END)
             FROM focus_sessions WHERE task_id IN ({placeholders}) GROUP BY task_id"
        );
        let mut statement = connection.prepare(&sql)?;
        let parameters = rusqlite::params_from_iter(task_ids.iter());
        let rows = statement.query_map(parameters, |row| {
            Ok((
                row.get::<_, String>(0)?,
                TaskFocusSummary {
                    total_seconds: row.get::<_, i64>(1)? as u64,
                    completed_seconds: row.get::<_, i64>(2)? as u64,
                    stopped_seconds: row.get::<_, i64>(3)? as u64,
                    completed_sessions: row.get::<_, i64>(4)? as u32,
                    stopped_sessions: row.get::<_, i64>(5)? as u32,
                },
            ))
        })?;
        for row in rows {
            let (task_id, summary) = row?;
            summaries.insert(task_id, summary);
        }
        Ok(summaries)
    }

    pub fn totals_for_tasks(
        &self,
        task_ids: &[String],
    ) -> Result<std::collections::HashMap<String, u64>> {
        Ok(self
            .summaries_for_tasks(task_ids)?
            .into_iter()
            .map(|(task_id, summary)| (task_id, summary.total_seconds))
            .collect())
    }
}

fn migrate_timer_state(connection: &mut Connection) -> Result<()> {
    let transaction = connection.transaction_with_behavior(TransactionBehavior::Immediate)?;
    let existing_columns = {
        let mut statement = transaction.prepare("PRAGMA table_info(timer_state)")?;
        let rows = statement.query_map([], |row| row.get::<_, String>(1))?;
        rows.collect::<rusqlite::Result<HashSet<_>>>()?
    };
    let next_task_columns = [
        ("next_task_id", "TEXT"),
        ("next_task_text", "TEXT"),
        ("next_source_path", "TEXT"),
        ("next_source_line", "INTEGER"),
    ];
    let mut migrated_next_tasks = false;
    for (column, column_type) in next_task_columns {
        if !existing_columns.contains(column) {
            transaction.execute(
                &format!("ALTER TABLE timer_state ADD COLUMN {column} {column_type}"),
                [],
            )?;
            migrated_next_tasks = true;
        }
    }
    if migrated_next_tasks {
        // Legacy task_* values served both meanings. Preserve them as the
        // best-known active owner and initial next-task context.
        transaction.execute(
            "UPDATE timer_state SET
                next_task_id = task_id, next_task_text = task_text,
                next_source_path = source_path, next_source_line = source_line
             WHERE singleton = 1",
            [],
        )?;
    }
    let preset_columns = [
        ("preset_name", "TEXT NOT NULL DEFAULT ''"),
        ("preset_focus_seconds", "INTEGER NOT NULL DEFAULT 0"),
        ("preset_break_seconds", "INTEGER NOT NULL DEFAULT 0"),
        ("preset_long_break_seconds", "INTEGER NOT NULL DEFAULT 0"),
        (
            "preset_cycles_before_long_break",
            "INTEGER NOT NULL DEFAULT 0",
        ),
    ];
    for (column, definition) in preset_columns {
        if !existing_columns.contains(column) {
            transaction.execute(
                &format!("ALTER TABLE timer_state ADD COLUMN {column} {definition}"),
                [],
            )?;
        }
    }
    transaction.execute(
        "UPDATE timer_state SET phase = 'ready_for_focus' WHERE phase = 'ready'",
        [],
    )?;
    transaction.pragma_update(None, "user_version", 2)?;
    transaction.commit()?;
    Ok(())
}

fn read_state(connection: &Connection) -> Result<TimerState> {
    let raw = connection.query_row(
        "SELECT phase, timer_id, task_id, task_text, source_path, source_line,
                next_task_id, next_task_text, next_source_path, next_source_line,
                origin_pane_id, preset_name, preset_focus_seconds,
                preset_break_seconds, preset_long_break_seconds,
                preset_cycles_before_long_break, cycle, session_started_at,
                phase_started_at, deadline_at, remaining_seconds,
                accumulated_seconds, updated_at
         FROM timer_state WHERE singleton = 1",
        [],
        |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, Option<String>>(2)?,
                row.get::<_, Option<String>>(3)?,
                row.get::<_, Option<String>>(4)?,
                row.get::<_, Option<i64>>(5)?,
                row.get::<_, Option<String>>(6)?,
                row.get::<_, Option<String>>(7)?,
                row.get::<_, Option<String>>(8)?,
                row.get::<_, Option<i64>>(9)?,
                row.get::<_, Option<String>>(10)?,
                row.get::<_, String>(11)?,
                row.get::<_, i64>(12)?,
                row.get::<_, i64>(13)?,
                row.get::<_, i64>(14)?,
                row.get::<_, i64>(15)?,
                row.get::<_, i64>(16)?,
                row.get::<_, Option<f64>>(17)?,
                row.get::<_, Option<f64>>(18)?,
                row.get::<_, Option<f64>>(19)?,
                row.get::<_, f64>(20)?,
                row.get::<_, f64>(21)?,
                row.get::<_, f64>(22)?,
            ))
        },
    )?;
    Ok(TimerState {
        phase: TimerPhase::parse(&raw.0)
            .with_context(|| format!("unknown timer phase {}", raw.0))?,
        timer_id: raw.1,
        active_task_id: raw.2,
        active_task_text: raw.3,
        active_source_path: raw.4,
        active_source_line: raw.5.map(|value| value as usize),
        next_task_id: raw.6,
        next_task_text: raw.7,
        next_source_path: raw.8,
        next_source_line: raw.9.map(|value| value as usize),
        origin_pane_id: raw.10,
        preset_name: raw.11,
        preset_focus_seconds: raw.12 as u64,
        preset_break_seconds: raw.13 as u64,
        preset_long_break_seconds: raw.14 as u64,
        preset_cycles_before_long_break: raw.15 as u32,
        cycle: raw.16 as u32,
        session_started_at: raw.17,
        phase_started_at: raw.18,
        deadline_at: raw.19,
        remaining_seconds: raw.20,
        accumulated_seconds: raw.21,
        updated_at: raw.22,
    })
}

fn insert_session(
    transaction: &Transaction<'_>,
    state: &TimerState,
    ended_at: f64,
    duration_seconds: u64,
    outcome: &str,
) -> Result<()> {
    if duration_seconds == 0 {
        return Ok(());
    }
    let (Some(task_id), Some(task_text), Some(source_path)) = (
        &state.active_task_id,
        &state.active_task_text,
        &state.active_source_path,
    ) else {
        return Ok(());
    };
    transaction.execute(
        "
        INSERT INTO focus_sessions (
            task_id, task_text, source_path, source_line,
            started_at, ended_at, duration_seconds, outcome
        ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)
        ",
        params![
            task_id,
            task_text,
            source_path,
            state.active_source_line.map(|value| value as i64),
            state.session_started_at.unwrap_or(ended_at),
            ended_at,
            duration_seconds as i64,
            outcome,
        ],
    )?;
    Ok(())
}

fn new_id() -> String {
    Uuid::new_v4().simple().to_string()
}
