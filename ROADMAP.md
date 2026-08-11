# Focus Pane Roadmap

## Product direction

Focus Pane keeps Markdown as the task source of truth and SQLite as the timer/history store.
New features should preserve ordinary Markdown readability, stable task IDs, existing focus history,
and safe operation when editors or multiple Focus Pane instances touch the same files.

## Accepted data ownership

- **Markdown:** task text, completion, hierarchy, tags, priority, due date, estimate, and blocked state.
- **SQLite:** timer state, focus sessions, actual duration, aggregate totals, and estimate/actual history.
- **Estimate unit:** Pomodoro count in the first version.
- **First richer-Markdown scope:** nested tasks, tags, priority, due date, estimate, and blocked state.
- **Deferred:** recurring-task semantics until the initial metadata grammar is stable.

Any extension to `<!-- focus:... -->` is a compatibility change and must include parser tests and an
explicit migration/compatibility decision.

## Implementation order

### Phase 1 — File safety foundation

**Status: Complete**

Implemented with content revisions, persistent sidecar advisory locks, lock-scoped
read/validate/write operations, optimistic pre-rename validation, recoverable task conflicts, and
TUI polling/reconciliation tests.

#### 1.1 Markdown concurrent-write protection

- [x] Use a stable sidecar lock file for every Markdown source.
- [x] Hold an exclusive lock across read → validate → mutate → atomic rename.
- [x] Re-read the latest file after acquiring the lock.
- [x] Prefer exact stable task IDs when locating a task.
- [x] Reject an operation when the selected task itself changed externally instead of overwriting it.
- [x] Revalidate the content revision immediately before atomic rename.
- [x] Keep atomic writes, file permissions, line endings, and unrelated Markdown intact.
- [x] Add concurrent-writer and conflict regression tests.

#### 1.2 External file-change detection

- [x] Track a content revision for the loaded Markdown snapshot.
- [x] Poll every 500 ms while the TUI is open.
- [x] Automatically reload in normal/search mode and preserve selection by stable task ID.
- [x] During add/delete modals, show a change warning instead of discarding user input.
- [x] Reconcile against the latest file when the modal is submitted.
- [x] Add UI tests for automatic reload and modal-safe reconciliation.

**Phase 1 exit criteria:** two Focus Pane instances cannot silently lose each other's writes; external
editor changes appear without reopening the popup; conflicting edits produce a recoverable UI message.

### Phase 2 — Markdown domain model and editing

**Status: Complete**

Implemented with a lossless trailing Focus metadata grammar, typed task metadata, subtree-aware
repository mutations, cursor-aware text editing, nested rendering, and conflict-safe TUI actions.

#### 2.1 Richer Markdown task parser

- [x] Introduce a lossless parsed task-line representation.
- [x] Support nested tasks/subtrees, tags, priority, due date, estimate, and blocked state.
- [x] Preserve unknown text and metadata during mutations.
- [x] Define and document the Focus Pane metadata grammar.
- [x] Keep existing checkbox and hidden-ID files compatible.

#### 2.2 Fast editing and ordering

- [x] `e`: edit selected task text with cursor movement, delete/backspace, and `Ctrl+U`.
- [x] `a`: add a sibling after the selection and its subtree.
- [x] `J` / `K`: move a task or its complete subtree down/up.
- [x] `>` / `<`: increase/decrease indentation safely.
- [x] Preserve heading boundaries unless an explicit cross-heading move is requested.
- [x] Run every mutation through Phase 1 locking and conflict validation.

### Phase 3 — Task planning states

**Status: Complete**

Implemented with lossless metadata mutations, an estimate input modal, actual/estimate progress,
completed-task variance, outcome-aware SQLite summaries, and searchable blocked styling. Optional
free-text blocker reasons remain deferred until quoted metadata values are designed.

#### 3.1 Task estimate and variance

- [x] Store estimate in Markdown as a Pomodoro count.
- [x] `E`: set an estimate; empty or `0` clears it.
- [x] Display actual/estimate on task rows and detail/status views.
- [x] Calculate completion variance from SQLite session totals.
- [x] Distinguish completed and stopped focus time and session counts.

#### 3.2 Blocked mode

- [x] `b`: toggle blocked status without completing the checkbox.
- [x] Give blocked tasks a distinct marker/style.
- [ ] Optional blocker reason — deferred pending a quoted metadata grammar.
- [x] Keep blocked tasks searchable, focusable, and retain all focus history.

### Phase 4 — Timer model

**Status: Complete**

Implemented with named TOML presets, idle `P` selection, SQLite schema version 2 preset snapshots,
explicit waiting states, state-aware notifications/sidebar text, and detached-worker tests. Command
palette selection is deferred to Phase 5, where the central command registry will be introduced.

#### 4.1 Configurable focus presets

- [x] Add named presets containing focus, short-break, long-break, and cycle settings.
- [x] Keep the existing flat timer config as the backward-compatible implicit preset.
- [x] `P`: cycle and persist preset selection from the TUI while idle.
- [x] Select presets from the command palette through the shared Phase 5 command.
- [x] Persist the active preset name and duration snapshot so workers use the same values.

#### 4.2 Clear transitions

Replace ambiguous waiting/paused semantics with explicit states:

```text
Idle
Focus
PausedFocus
ReadyForBreak
Break
PausedBreak
ReadyForFocus
```

- [x] `auto_start_break=false` transitions to `ReadyForBreak`, not `PausedBreak`.
- [x] `auto_start_focus=false` transitions to `ReadyForFocus`.
- [x] `p` starts a ready phase; paused states only represent explicitly paused countdowns.
- [x] Update worker behavior, sidebar text, notifications, schema migration, and tests together.

### Phase 5 — TUI command system and responsive layout

**Status: Complete**

Implemented with a shared command registry, searchable/context-aware palette, dimmed contextual help,
registry-generated wrapping footers, three viewport modes, clamped overlays, and breakpoint tests.

#### 5.1 Command palette and help

- [x] Add a central command registry containing ID, label, keybinding, availability, and handler.
- [x] `:` opens a searchable command palette.
- [x] `?` opens contextual help with unavailable commands dimmed.
- [x] Generate key dispatch, footer hints, palette entries, and help from the same registry.

#### 5.2 Small-window responsive layout

- [x] Define compact (<72 columns or <18 rows), normal, and wide (≥110 columns) modes.
- [x] Collapse secondary statistics and shorten hints in compact mode.
- [x] Wrap the registry-generated footer to the available lines.
- [x] Clamp modals and preserve tasks/timer controls down to the tested 32×10 minimum.
- [x] Add compact, normal, wide, and small-overlay snapshots.

### Phase 6 — Multiple TODO files and workspaces

**Status: Complete**

Implemented with named workspace resolution, startup glob expansion, origin-based initial selection,
aggregate/per-source views, independent repositories/revisions/locks, source-qualified UI and history,
and visible missing/unavailable source rows.

- [x] Replace the single repository assumption with a workspace/repository collection.
- [x] Configure named workspaces and explicit files/globs relative to the config file.
- [x] Keep one global data-directory `TODO.md` as the zero-config default and `--file` as an override.
- [x] Switch workspaces with `W`, and aggregate/individual source views with `F`.
- [x] Include source identity in selection, locking, external-change messages, and task grouping.
- [x] Preserve task history with `(source_path, stable task ID)` identity using the existing schema.
- [x] Qualify duplicate headings and show missing, unmatched-glob, and unreadable sources clearly.

Example target configuration:

```toml
[[workspaces]]
name = "Study"
files = [
  "~/study/TODO.md",
  "~/courses/distributed-systems/TODO.md",
]

[[workspaces]]
name = "Projects"
files = ["~/projects/*/TODO.md"]
```

## Engineering rules for every phase

1. Add a regression test at the real behavior seam before fixing a bug or changing state semantics.
2. Keep schema changes additive or provide an explicit migration.
3. Preserve unknown Markdown rather than normalizing entire files.
4. Prefer small vertical slices that leave the app usable.
5. Run before completion:

```bash
cargo fmt --check
cargo clippy --all-targets --all-features -- -D warnings
cargo test -- --test-threads=1
cargo build --release
```
