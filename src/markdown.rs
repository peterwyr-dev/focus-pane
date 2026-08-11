use crate::{
    config::default_data_dir,
    models::{Task, TaskMetadata, TaskPriority},
};
use anyhow::{Context, Result, bail};
use chrono::NaiveDate;
use fs2::FileExt;
use regex::{Captures, Regex};
use std::{
    env, fs,
    fs::{File, OpenOptions},
    io::Write,
    path::{Path, PathBuf},
    sync::OnceLock,
};
use thiserror::Error;
use uuid::Uuid;

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct FileRevision {
    exists: bool,
    length: u64,
    fingerprint: u64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TaskSnapshot {
    pub tasks: Vec<Task>,
    pub revision: FileRevision,
}

#[derive(Debug, Error)]
#[error("Markdown task {task_text:?} changed externally in {path}: {reason}")]
pub struct MarkdownConflict {
    path: String,
    task_text: String,
    reason: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MoveDirection {
    Up,
    Down,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum IndentDirection {
    Increase,
    Decrease,
}

pub struct MarkdownTaskRepository {
    path: PathBuf,
}

impl MarkdownTaskRepository {
    pub fn new(path: PathBuf) -> Self {
        let path = if path.is_absolute() {
            path
        } else {
            env::current_dir().unwrap_or_default().join(path)
        };
        Self { path }
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    pub fn read_tasks(&self) -> Result<Vec<Task>> {
        Ok(self.read_snapshot()?.tasks)
    }

    pub fn read_snapshot(&self) -> Result<TaskSnapshot> {
        let (source, revision) = self.read_source()?;
        Ok(TaskSnapshot {
            tasks: self.parse_tasks(&source),
            revision,
        })
    }

    pub fn revision(&self) -> Result<FileRevision> {
        Ok(self.read_source()?.1)
    }

    pub fn toggle(&self, task: &Task) -> Result<Task> {
        let _lock_file = self.acquire_write_lock()?;
        let (mut lines, revision) = self.read_lines()?;
        let (index, mark_start) = self.locate_unchanged_task(&lines, task)?;
        let checked = lines[index].as_bytes()[mark_start].eq_ignore_ascii_case(&b'x');
        lines[index].replace_range(mark_start..mark_start + 1, if checked { " " } else { "x" });
        self.atomic_write(&lines, revision)?;
        self.task_at(index + 1, task.heading.clone())
    }

    pub fn delete(&self, task: &Task) -> Result<()> {
        let _lock_file = self.acquire_write_lock()?;
        let (mut lines, revision) = self.read_lines()?;
        let (index, _) = self.locate_unchanged_task(&lines, task)?;
        lines.remove(index);
        self.atomic_write(&lines, revision)
    }

    pub fn ensure_id(&self, task: &Task) -> Result<Task> {
        let _lock_file = self.acquire_write_lock()?;
        let (mut lines, revision) = self.read_lines()?;
        let (index, _) = self.locate_unchanged_task(&lines, task)?;
        if task.task_id.is_some() {
            return self.task_at(index + 1, task.heading.clone());
        }
        let task_id = Uuid::new_v4().simple().to_string();
        let newline = if lines[index].ends_with("\r\n") {
            "\r\n"
        } else if lines[index].ends_with('\n') {
            "\n"
        } else {
            ""
        };
        let content_length = lines[index].len() - newline.len();
        lines[index].truncate(content_length);
        lines[index].push_str(&format!(" <!-- focus:id={task_id} -->{newline}"));
        self.atomic_write(&lines, revision)?;
        self.task_at(index + 1, task.heading.clone())
    }

    pub fn add(&self, text: &str) -> Result<Task> {
        let clean_text = clean_task_text(text)?;
        let _lock_file = self.acquire_write_lock()?;
        let task_id = Uuid::new_v4().simple().to_string();
        let (mut lines, revision) = self.read_lines()?;
        if !revision.exists {
            lines.push("# Tasks\n".into());
            lines.push("\n".into());
        }
        if let Some(last) = lines.last_mut()
            && !last.ends_with('\n')
        {
            last.push('\n');
        }
        lines.push(format!("- [ ] {clean_text} <!-- focus:id={task_id} -->\n"));
        self.atomic_write(&lines, revision)?;
        self.read_tasks()?
            .into_iter()
            .find(|task| task.task_id.as_deref() == Some(task_id.as_str()))
            .ok_or_else(|| anyhow::anyhow!("new task could not be read back"))
    }

    pub fn edit(&self, task: &Task, text: &str) -> Result<Task> {
        let clean_text = clean_task_text(text)?;
        let _lock_file = self.acquire_write_lock()?;
        let (mut lines, revision) = self.read_lines()?;
        let (index, _) = self.locate_unchanged_task(&lines, task)?;
        let (body_start, body_end, suffix) = {
            let line = lines[index].trim_end_matches(['\r', '\n']);
            let captures = checkbox_regex()
                .captures(line)
                .ok_or_else(|| self.conflict(task, "the target line is no longer a task"))?;
            let body = captures.name("body").expect("task body capture");
            let metadata_start = focus_metadata_start(body.as_str());
            (
                body.start(),
                body.end(),
                body.as_str()[metadata_start..].to_owned(),
            )
        };
        let separator = if suffix.is_empty() || suffix.starts_with(char::is_whitespace) {
            ""
        } else {
            " "
        };
        lines[index].replace_range(
            body_start..body_end,
            &format!("{clean_text}{separator}{suffix}"),
        );
        self.atomic_write(&lines, revision)?;
        self.task_at(index + 1, task.heading.clone())
    }

    pub fn set_estimate(&self, task: &Task, estimate: Option<u32>) -> Result<Task> {
        if estimate == Some(0) {
            bail!("task estimate must be at least one Pomodoro");
        }
        let value = estimate.map(|estimate| estimate.to_string());
        self.update_metadata(task, "estimate", value.as_deref())
    }

    pub fn toggle_blocked(&self, task: &Task) -> Result<Task> {
        let value = (!task.metadata.blocked).then_some("blocked");
        self.update_metadata(task, "status", value)
    }

    fn update_metadata(&self, task: &Task, key: &str, value: Option<&str>) -> Result<Task> {
        let _lock_file = self.acquire_write_lock()?;
        let (mut lines, revision) = self.read_lines()?;
        let (index, _) = self.locate_unchanged_task(&lines, task)?;
        let (body_start, body_end, updated_body) = {
            let line = lines[index].trim_end_matches(['\r', '\n']);
            let captures = checkbox_regex()
                .captures(line)
                .ok_or_else(|| self.conflict(task, "the target line is no longer a task"))?;
            let body = captures.name("body").expect("task body capture");
            (
                body.start(),
                body.end(),
                update_focus_metadata(body.as_str(), key, value),
            )
        };
        lines[index].replace_range(body_start..body_end, &updated_body);
        self.atomic_write(&lines, revision)?;
        self.task_at(index + 1, task.heading.clone())
    }

    pub fn insert_after(&self, task: &Task, text: &str) -> Result<Task> {
        let clean_text = clean_task_text(text)?;
        let _lock_file = self.acquire_write_lock()?;
        let (mut lines, revision) = self.read_lines()?;
        let (index, _) = self.locate_unchanged_task(&lines, task)?;
        let insert_index = self.subtree_end(&lines, index, task.indent);
        let (indent, bullet, gap, after, newline) = {
            let line = lines[index].trim_end_matches(['\r', '\n']);
            let captures = checkbox_regex()
                .captures(line)
                .ok_or_else(|| self.conflict(task, "the target line is no longer a task"))?;
            let newline = if lines[index].ends_with("\r\n") {
                "\r\n"
            } else {
                "\n"
            };
            (
                captures["indent"].to_owned(),
                captures["bullet"].to_owned(),
                captures["gap"].to_owned(),
                captures["after"].to_owned(),
                newline,
            )
        };
        if insert_index > 0 && !lines[insert_index - 1].ends_with('\n') {
            lines[insert_index - 1].push_str(newline);
        }
        let task_id = Uuid::new_v4().simple().to_string();
        lines.insert(
            insert_index,
            format!(
                "{indent}{bullet}{gap}[ ]{after}{clean_text} <!-- focus:id={task_id} -->{newline}"
            ),
        );
        self.atomic_write(&lines, revision)?;
        self.task_by_id(&task_id)
    }

    pub fn move_subtree(&self, task: &Task, direction: MoveDirection) -> Result<Option<Task>> {
        let task_id = task
            .task_id
            .as_deref()
            .ok_or_else(|| anyhow::anyhow!("moving a task requires a stable task ID"))?;
        let _lock_file = self.acquire_write_lock()?;
        let (mut lines, revision) = self.read_lines()?;
        let (start, _) = self.locate_unchanged_task(&lines, task)?;
        let end = self.subtree_end(&lines, start, task.indent);

        let range = match direction {
            MoveDirection::Down => {
                let Some(next_task) = lines.get(end).and_then(|line| {
                    self.parse_task_line(line.trim_end_matches(['\r', '\n']), end + 1, None)
                }) else {
                    return Ok(None);
                };
                if next_task.indent != task.indent {
                    return Ok(None);
                }
                let next_end = self.subtree_end(&lines, end, task.indent);
                let replacement = lines[end..next_end]
                    .iter()
                    .chain(lines[start..end].iter())
                    .cloned()
                    .collect::<Vec<_>>();
                lines.splice(start..next_end, replacement);
                start..next_end
            }
            MoveDirection::Up => {
                let Some(previous_start) = self.previous_sibling_index(&lines, start, task.indent)
                else {
                    return Ok(None);
                };
                let replacement = lines[start..end]
                    .iter()
                    .chain(lines[previous_start..start].iter())
                    .cloned()
                    .collect::<Vec<_>>();
                lines.splice(previous_start..end, replacement);
                previous_start..end
            }
        };
        debug_assert!(!range.is_empty());
        self.atomic_write(&lines, revision)?;
        Ok(Some(self.task_by_id(task_id)?))
    }

    pub fn change_indent(&self, task: &Task, direction: IndentDirection) -> Result<Option<Task>> {
        let task_id = task
            .task_id
            .as_deref()
            .ok_or_else(|| anyhow::anyhow!("indenting a task requires a stable task ID"))?;
        let _lock_file = self.acquire_write_lock()?;
        let (mut lines, revision) = self.read_lines()?;
        let (start, _) = self.locate_unchanged_task(&lines, task)?;
        let end = self.subtree_end(&lines, start, task.indent);

        match direction {
            IndentDirection::Increase => {
                if self
                    .previous_sibling_index(&lines, start, task.indent)
                    .is_none()
                {
                    return Ok(None);
                }
                for line in &mut lines[start..end] {
                    if !line.trim().is_empty() {
                        line.insert_str(0, "  ");
                    }
                }
            }
            IndentDirection::Decrease => {
                if task.indent == 0 {
                    return Ok(None);
                }
                let columns = task.indent.min(2);
                for line in &mut lines[start..end] {
                    remove_indent_columns(line, columns);
                }
            }
        }

        self.atomic_write(&lines, revision)?;
        Ok(Some(self.task_by_id(task_id)?))
    }

    fn read_source(&self) -> Result<(String, FileRevision)> {
        match fs::read_to_string(&self.path) {
            Ok(source) => {
                let revision = revision_for_source(true, &source);
                Ok((source, revision))
            }
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                Ok((String::new(), FileRevision::default()))
            }
            Err(error) => Err(error.into()),
        }
    }

    fn parse_tasks(&self, source: &str) -> Vec<Task> {
        let mut heading = None;
        let mut tasks = Vec::new();
        for (index, line) in source.lines().enumerate() {
            if let Some(captures) = heading_regex().captures(line) {
                heading = Some(captures["heading"].trim().to_owned());
                continue;
            }
            if let Some(task) = self.parse_task_line(line, index + 1, heading.clone()) {
                tasks.push(task);
            }
        }
        tasks
    }

    fn read_lines(&self) -> Result<(Vec<String>, FileRevision)> {
        let (source, revision) = self.read_source()?;
        Ok((
            source.split_inclusive('\n').map(str::to_owned).collect(),
            revision,
        ))
    }

    fn acquire_write_lock(&self) -> Result<File> {
        let parent = self.path.parent().unwrap_or_else(|| Path::new("."));
        fs::create_dir_all(parent)?;
        let file_name = self.path.file_name().unwrap_or_default().to_string_lossy();
        let lock_path = parent.join(format!(".{file_name}.focus-pane.lock"));
        let lock_file = OpenOptions::new()
            .create(true)
            .truncate(false)
            .read(true)
            .write(true)
            .open(&lock_path)
            .with_context(|| format!("could not open Markdown lock {}", lock_path.display()))?;
        lock_file
            .lock_exclusive()
            .with_context(|| format!("could not lock Markdown file {}", self.path.display()))?;
        Ok(lock_file)
    }

    fn locate_unchanged_task(&self, lines: &[String], task: &Task) -> Result<(usize, usize)> {
        let location = self.locate_task(lines, task)?;
        let current = self
            .parse_task_line(
                lines[location.0].trim_end_matches(['\r', '\n']),
                location.0 + 1,
                task.heading.clone(),
            )
            .ok_or_else(|| self.conflict(task, "the target line is no longer a task"))?;
        if current.task_id != task.task_id
            || current.text != task.text
            || current.checked != task.checked
            || current.indent != task.indent
        {
            return Err(self.conflict(task, "the selected task content changed"));
        }
        Ok(location)
    }

    fn locate_task(&self, lines: &[String], task: &Task) -> Result<(usize, usize)> {
        if let Some(task_id) = &task.task_id {
            let matches: Vec<_> = lines
                .iter()
                .enumerate()
                .filter_map(|(index, line)| {
                    let line = line.trim_end_matches(['\r', '\n']);
                    let captures = checkbox_regex().captures(line)?;
                    let current = self.parse_task_line(line, index + 1, None)?;
                    if current.task_id.as_deref() != Some(task_id.as_str()) {
                        return None;
                    }
                    Some((index, captures.name("mark").unwrap().start()))
                })
                .collect();
            return match matches.len() {
                1 => Ok(matches[0]),
                0 => Err(self.conflict(task, "its stable ID no longer exists")),
                count => Err(self.conflict(task, &format!("its stable ID appears {count} times"))),
            };
        }

        let expected = task.line_number.saturating_sub(1);
        if let Some(line) = lines.get(expected)
            && let Some(captures) = checkbox_regex().captures(line.trim_end_matches(['\r', '\n']))
            && text_from_captures(&captures) == task.text
        {
            return Ok((expected, captures.name("mark").unwrap().start()));
        }

        let matches: Vec<_> = lines
            .iter()
            .enumerate()
            .filter_map(|(index, line)| {
                let captures = checkbox_regex().captures(line.trim_end_matches(['\r', '\n']))?;
                (text_from_captures(&captures) == task.text)
                    .then(|| (index, captures.name("mark").unwrap().start()))
            })
            .collect();
        if matches.len() == 1 {
            return Ok(matches[0]);
        }
        Err(self.conflict(task, "it could not be located unambiguously"))
    }

    fn conflict(&self, task: &Task, reason: &str) -> anyhow::Error {
        MarkdownConflict {
            path: self.path.display().to_string(),
            task_text: task.text.clone(),
            reason: reason.to_owned(),
        }
        .into()
    }

    fn file_conflict(&self, reason: &str) -> anyhow::Error {
        MarkdownConflict {
            path: self.path.display().to_string(),
            task_text: "<file>".into(),
            reason: reason.to_owned(),
        }
        .into()
    }

    fn subtree_end(&self, lines: &[String], start: usize, indent: usize) -> usize {
        for (index, line) in lines.iter().enumerate().skip(start + 1) {
            let line = line.trim_end_matches(['\r', '\n']);
            if heading_regex().is_match(line) {
                return index;
            }
            if let Some(task) = self.parse_task_line(line, index + 1, None)
                && task.indent <= indent
            {
                return index;
            }
        }
        lines.len()
    }

    fn previous_sibling_index(
        &self,
        lines: &[String],
        start: usize,
        indent: usize,
    ) -> Option<usize> {
        for index in (0..start).rev() {
            let line = lines[index].trim_end_matches(['\r', '\n']);
            if heading_regex().is_match(line) {
                return None;
            }
            if let Some(task) = self.parse_task_line(line, index + 1, None) {
                if task.indent < indent {
                    return None;
                }
                if task.indent == indent {
                    return Some(index);
                }
            }
        }
        None
    }

    fn task_by_id(&self, task_id: &str) -> Result<Task> {
        self.read_tasks()?
            .into_iter()
            .find(|task| task.task_id.as_deref() == Some(task_id))
            .ok_or_else(|| anyhow::anyhow!("task {task_id:?} could not be read back"))
    }

    fn task_at(&self, line_number: usize, fallback_heading: Option<String>) -> Result<Task> {
        let source = fs::read_to_string(&self.path)?;
        let lines: Vec<_> = source.lines().collect();
        let mut heading = None;
        for line in lines.iter().take(line_number) {
            if let Some(captures) = heading_regex().captures(line) {
                heading = Some(captures["heading"].trim().to_owned());
            }
        }
        self.parse_task_line(
            lines
                .get(line_number - 1)
                .ok_or_else(|| anyhow::anyhow!("line {line_number} no longer exists"))?,
            line_number,
            heading.or(fallback_heading),
        )
        .ok_or_else(|| anyhow::anyhow!("line {line_number} is no longer a task"))
    }

    fn atomic_write(&self, lines: &[String], expected_revision: FileRevision) -> Result<()> {
        let parent = self.path.parent().unwrap_or_else(|| Path::new("."));
        fs::create_dir_all(parent)?;
        let temporary = parent.join(format!(
            ".{}.{}.tmp",
            self.path.file_name().unwrap_or_default().to_string_lossy(),
            Uuid::new_v4().simple()
        ));
        let permissions = fs::metadata(&self.path)
            .ok()
            .map(|metadata| metadata.permissions());
        let result = (|| -> Result<()> {
            let mut file = fs::File::create(&temporary)?;
            for line in lines {
                file.write_all(line.as_bytes())?;
            }
            file.sync_all()?;
            if let Some(permissions) = permissions {
                fs::set_permissions(&temporary, permissions)?;
            }
            if self.revision()? != expected_revision {
                return Err(self.file_conflict("the file changed while a write was in progress"));
            }
            fs::rename(&temporary, &self.path)?;
            Ok(())
        })();
        if result.is_err() {
            let _ = fs::remove_file(&temporary);
        }
        result
    }

    fn parse_task_line(
        &self,
        line: &str,
        line_number: usize,
        heading: Option<String>,
    ) -> Option<Task> {
        let captures = checkbox_regex().captures(line)?;
        let parsed_body = parse_task_body(&captures["body"]);
        Some(Task {
            source_path: self.path.clone(),
            line_number,
            text: parsed_body.text,
            checked: captures["mark"].eq_ignore_ascii_case("x"),
            heading,
            task_id: parsed_body.task_id,
            indent: expanded_indent(&captures["indent"]),
            metadata: parsed_body.metadata,
        })
    }
}

fn clean_task_text(text: &str) -> Result<String> {
    let clean_text = text.lines().collect::<Vec<_>>().join(" ");
    let clean_text = clean_text.trim();
    if clean_text.is_empty() {
        bail!("Task text must not be empty");
    }
    Ok(clean_text.to_owned())
}

fn remove_indent_columns(line: &mut String, columns: usize) {
    let mut remaining = columns;
    let mut byte_end = 0;
    let mut replacement_spaces = 0;
    for byte in line.as_bytes() {
        if remaining == 0 {
            break;
        }
        match byte {
            b' ' => {
                remaining -= 1;
                byte_end += 1;
            }
            b'\t' => {
                byte_end += 1;
                if remaining >= 4 {
                    remaining -= 4;
                } else {
                    replacement_spaces = 4 - remaining;
                    remaining = 0;
                }
            }
            _ => break,
        }
    }
    line.replace_range(0..byte_end, &" ".repeat(replacement_spaces));
}

fn revision_for_source(exists: bool, source: &str) -> FileRevision {
    let mut fingerprint = 0xcbf29ce484222325_u64;
    for byte in source.as_bytes() {
        fingerprint ^= u64::from(*byte);
        fingerprint = fingerprint.wrapping_mul(0x100000001b3);
    }
    FileRevision {
        exists,
        length: source.len() as u64,
        fingerprint,
    }
}

fn checkbox_regex() -> &'static Regex {
    static REGEX: OnceLock<Regex> = OnceLock::new();
    REGEX.get_or_init(|| {
        Regex::new(
            r"^(?P<indent>\s*)(?P<bullet>[-*+])(?P<gap>\s+)\[(?P<mark>[ xX])\](?P<after>\s+)(?P<body>.*)$",
        )
        .expect("valid checkbox regex")
    })
}

fn heading_regex() -> &'static Regex {
    static REGEX: OnceLock<Regex> = OnceLock::new();
    REGEX.get_or_init(|| {
        Regex::new(r"^\s{0,3}#{1,6}\s+(?P<heading>.+?)\s*#*\s*$").expect("valid heading regex")
    })
}

fn update_focus_metadata(body: &str, key: &str, value: Option<&str>) -> String {
    let metadata_start = focus_metadata_start(body);
    let visible_text = body[..metadata_start].trim_end();
    let mut regular_comments = Vec::new();
    let mut id_comments = Vec::new();

    for captures in focus_comment_regex().captures_iter(&body[metadata_start..]) {
        let raw_comment = captures.get(0).expect("focus comment capture").as_str();
        let tokens = captures["values"].split_whitespace().collect::<Vec<_>>();
        let has_target = tokens.iter().any(|token| metadata_token_key(token) == key);
        let retained = tokens
            .into_iter()
            .filter(|token| metadata_token_key(token) != key)
            .collect::<Vec<_>>();
        if retained.is_empty() {
            continue;
        }
        let has_id = retained
            .iter()
            .any(|token| metadata_token_key(token) == "id");
        let comment = if has_target {
            format!("<!-- focus:{} -->", retained.join(" "))
        } else {
            raw_comment.to_owned()
        };
        if has_id {
            id_comments.push(comment);
        } else {
            regular_comments.push(comment);
        }
    }

    if let Some(value) = value {
        regular_comments.push(format!("<!-- focus:{key}={value} -->"));
    }
    regular_comments.extend(id_comments);

    let mut updated = visible_text.to_owned();
    for comment in regular_comments {
        if !updated.is_empty() {
            updated.push(' ');
        }
        updated.push_str(&comment);
    }
    updated
}

fn metadata_token_key(token: &str) -> &str {
    token.split_once('=').map_or(token, |(key, _)| key)
}

#[derive(Debug)]
struct ParsedTaskBody {
    text: String,
    task_id: Option<String>,
    metadata: TaskMetadata,
}

fn parse_task_body(body: &str) -> ParsedTaskBody {
    let metadata_start = focus_metadata_start(body);
    let text = body[..metadata_start].trim().to_owned();
    let mut task_id = None;
    let mut metadata = TaskMetadata {
        tags: tag_regex()
            .captures_iter(&text)
            .map(|captures| captures["tag"].to_owned())
            .collect(),
        ..TaskMetadata::default()
    };

    for comment in focus_comment_regex().captures_iter(&body[metadata_start..]) {
        for token in metadata_token_regex().captures_iter(&comment["values"]) {
            let key = &token["key"];
            let value = &token["value"];
            match key {
                "id" => task_id = Some(value.to_owned()),
                "estimate" => {
                    metadata.estimate_pomodoros =
                        value.parse::<u32>().ok().filter(|estimate| *estimate > 0);
                }
                "status" => metadata.blocked = value.eq_ignore_ascii_case("blocked"),
                "priority" => metadata.priority = TaskPriority::parse(value),
                "due" => {
                    metadata.due_date = NaiveDate::parse_from_str(value, "%Y-%m-%d").ok();
                }
                _ => {}
            }
        }
    }

    ParsedTaskBody {
        text,
        task_id,
        metadata,
    }
}

fn focus_metadata_start(body: &str) -> usize {
    trailing_focus_metadata_regex()
        .captures(body)
        .and_then(|captures| captures.name("suffix"))
        .map_or(body.len(), |suffix| suffix.start())
}

fn trailing_focus_metadata_regex() -> &'static Regex {
    static REGEX: OnceLock<Regex> = OnceLock::new();
    REGEX.get_or_init(|| {
        Regex::new(r"(?P<suffix>(?:\s*<!--\s*focus:[^>]*-->\s*)+)$")
            .expect("valid trailing focus metadata regex")
    })
}

fn focus_comment_regex() -> &'static Regex {
    static REGEX: OnceLock<Regex> = OnceLock::new();
    REGEX.get_or_init(|| {
        Regex::new(r"<!--\s*focus:(?P<values>[^>]*)-->").expect("valid focus comment regex")
    })
}

fn metadata_token_regex() -> &'static Regex {
    static REGEX: OnceLock<Regex> = OnceLock::new();
    REGEX.get_or_init(|| {
        Regex::new(r"(?P<key>[A-Za-z][A-Za-z0-9_-]*)=(?P<value>[^\s]+)")
            .expect("valid focus metadata token regex")
    })
}

fn tag_regex() -> &'static Regex {
    static REGEX: OnceLock<Regex> = OnceLock::new();
    REGEX.get_or_init(|| {
        Regex::new(r"(?:^|\s)#(?P<tag>[\p{L}\p{N}_/-]+)").expect("valid task tag regex")
    })
}

fn text_from_captures(captures: &Captures<'_>) -> String {
    parse_task_body(&captures["body"]).text
}

fn expanded_indent(indent: &str) -> usize {
    indent
        .chars()
        .map(|character| if character == '\t' { 4 } else { 1 })
        .sum()
}

pub fn resolve_task_file(configured_file: &str) -> PathBuf {
    let configured = expand_home(configured_file);
    if configured.is_absolute() {
        configured
    } else {
        default_data_dir().join(configured)
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
