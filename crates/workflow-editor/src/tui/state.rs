//! Editor state — workflow being edited, mode, focus, undo stack, etc.
//!
//! All mutations live here so the `App` loop only needs to read events,
//! mutate this state, and render. The state is cloneable (cheap) so undo
//! can snapshot it freely.

use std::collections::HashMap;

use crate::error::Result;
use crate::executor::{ValidationReport, WorkflowResult};
use crate::model::{SnapshotInfo, WorkflowManifest};
use crate::registry::NodeRegistry;

use super::event::AppEvent;
use super::mode::{Focus, Mode};

/// Number of undo steps retained in memory.
const UNDO_DEPTH: usize = 64;

/// One entry in the undo / redo stack: a snapshot of the manifest + cursor.
#[derive(Debug, Clone)]
pub struct UndoEntry {
    /// Snapshot label.
    pub label: String,
    /// Full manifest at this point.
    pub manifest: WorkflowManifest,
}

/// Whole editor state.
#[derive(Debug, Clone)]
pub struct AppState {
    /// Current workflow being edited. None if the user has not picked one yet.
    pub manifest: Option<WorkflowManifest>,
    /// Currently selected node id, if any.
    pub selected_node: Option<uuid::Uuid>,
    /// Currently selected edge id, if any.
    pub selected_edge: Option<uuid::Uuid>,
    /// Editor mode.
    pub mode: Mode,
    /// Focused pane.
    pub focus: Focus,
    /// Status message at the bottom of the screen.
    pub status: String,
    /// Last validation report (recomputed on save).
    pub validation: Option<ValidationReport>,
    /// Pending run output (None until a run finishes).
    pub last_run: Option<WorkflowResult>,
    /// Undo stack, oldest first.
    pub undo: Vec<UndoEntry>,
    /// Redo stack, newest first.
    pub redo: Vec<UndoEntry>,
    /// Snapshot history (loaded on demand).
    pub history: Vec<SnapshotInfo>,
    /// Currently highlighted entry in the history panel (None = HEAD).
    pub history_index: Option<usize>,
    /// Currently selected skill (None = first in list).
    pub selected_skill: Option<uuid::Uuid>,
    /// Open command palette buffer.
    pub command_buffer: String,
    /// Buffer for the node picker search.
    pub picker_buffer: String,
    /// Buffer for the SOP editor.
    pub sop_buffer: String,
    /// Buffer for the spec editor (raw JSON string).
    pub spec_buffer: String,
    /// Picker selection index.
    pub picker_index: usize,
    /// Last error string to flash in the status bar.
    pub last_error: Option<String>,
    /// Whether the workflow has unsaved changes since the last save.
    pub dirty: bool,
}

impl Default for AppState {
    fn default() -> Self {
        Self {
            manifest: None,
            selected_node: None,
            selected_edge: None,
            mode: Mode::Normal,
            focus: Focus::Canvas,
            status: "ready".to_string(),
            validation: None,
            last_run: None,
            undo: Vec::new(),
            redo: Vec::new(),
            history: Vec::new(),
            history_index: None,
            selected_skill: None,
            command_buffer: String::new(),
            picker_buffer: String::new(),
            sop_buffer: String::new(),
            spec_buffer: String::new(),
            picker_index: 0,
            last_error: None,
            dirty: false,
        }
    }
}

impl AppState {
    /// Mutable accessor that marks the state as dirty.
    pub fn touch(&mut self) {
        self.dirty = true;
    }

    /// Push the current manifest onto the undo stack.
    pub fn push_undo(&mut self, label: impl Into<String>) {
        if let Some(m) = &self.manifest {
            self.undo.push(UndoEntry {
                label: label.into(),
                manifest: m.clone(),
            });
            if self.undo.len() > UNDO_DEPTH {
                self.undo.remove(0);
            }
            // Any new edit invalidates the redo stack.
            self.redo.clear();
        }
    }

    /// Try to undo the last edit. Returns true if state changed.
    pub fn undo(&mut self) -> bool {
        let Some(prev) = self.undo.pop() else {
            return false;
        };
        if let Some(cur) = &self.manifest {
            self.redo.push(UndoEntry {
                label: "redo".into(),
                manifest: cur.clone(),
            });
        }
        self.manifest = Some(prev.manifest);
        self.status = format!("undo: {}", prev.label);
        true
    }

    /// Try to redo.
    pub fn redo(&mut self) -> bool {
        let Some(next) = self.redo.pop() else {
            return false;
        };
        if let Some(cur) = &self.manifest {
            self.undo.push(UndoEntry {
                label: "undo".into(),
                manifest: cur.clone(),
            });
        }
        self.manifest = Some(next.manifest);
        self.status = format!("redo: {}", next.label);
        true
    }

    /// Select the next node in node-list order.
    pub fn select_next_node(&mut self) {
        let Some(m) = &self.manifest else { return };
        if m.nodes.is_empty() {
            return;
        }
        let idx = self
            .selected_node
            .and_then(|id| m.nodes.iter().position(|n| n.id == id))
            .map(|i| (i + 1) % m.nodes.len())
            .unwrap_or(0);
        self.selected_node = Some(m.nodes[idx].id);
        self.selected_edge = None;
    }

    /// Select the previous node.
    pub fn select_prev_node(&mut self) {
        let Some(m) = &self.manifest else { return };
        if m.nodes.is_empty() {
            return;
        }
        let idx = self
            .selected_node
            .and_then(|id| m.nodes.iter().position(|n| n.id == id))
            .map(|i| (i + m.nodes.len() - 1) % m.nodes.len())
            .unwrap_or(m.nodes.len() - 1);
        self.selected_node = Some(m.nodes[idx].id);
        self.selected_edge = None;
    }

    /// Pan the canvas viewport.
    pub fn pan(&mut self, dx: i32, dy: i32) {
        if let Some(m) = self.manifest.as_mut() {
            m.viewport.pan.0 = m.viewport.pan.0.saturating_add(dx);
            m.viewport.pan.1 = m.viewport.pan.1.saturating_add(dy);
        }
    }

    /// Zoom in or out by adjusting grid_step (1, 2, 4).
    pub fn zoom(&mut self, factor: i32) {
        if let Some(m) = self.manifest.as_mut() {
            let cur = m.viewport.grid_step as i32;
            let next = (cur + factor).clamp(1, 8);
            m.viewport.grid_step = next as u8;
        }
    }

    /// Reset status / error to a friendly message.
    pub fn flash(&mut self, msg: impl Into<String>) {
        self.status = msg.into();
        self.last_error = None;
    }

    /// Record an error in the status bar.
    pub fn flash_err(&mut self, msg: impl Into<String>) {
        let m = msg.into();
        self.status = format!("error: {m}");
        self.last_error = Some(m);
    }

    /// Apply an [`AppEvent`] to the buffers when in text-input modes.
    pub fn apply_text_event(&mut self, ev: &AppEvent) {
        let AppEvent::Key(k) = ev else { return };
        let buf = match self.mode {
            Mode::CommandPalette => Some(&mut self.command_buffer),
            Mode::NodePicker => Some(&mut self.picker_buffer),
            Mode::SopEditor => Some(&mut self.sop_buffer),
            Mode::SpecEditor => Some(&mut self.spec_buffer),
            _ => None,
        };
        if let Some(buf) = buf {
            use crossterm::event::KeyCode;
            match k.code {
                KeyCode::Char(c) => buf.push(c),
                KeyCode::Backspace => {
                    buf.pop();
                }
                _ => {}
            }
        }
    }

    /// Filter node kinds for the picker. Always returns the full list if
    /// the buffer is empty.
    pub fn picker_filter(&self, registry: &NodeRegistry) -> Vec<String> {
        let kinds = registry.list();
        if self.picker_buffer.is_empty() {
            kinds.iter().map(|k| k.kind.clone()).collect()
        } else {
            let q = self.picker_buffer.to_lowercase();
            kinds
                .iter()
                .filter(|k| {
                    k.kind.to_lowercase().contains(&q)
                        || k.label.to_lowercase().contains(&q)
                        || k.category.to_lowercase().contains(&q)
                })
                .map(|k| k.kind.clone())
                .collect()
        }
    }

    /// Names of built-in commands shown in the command palette.
    pub fn command_suggestions(&self) -> Vec<(&'static str, &'static str)> {
        vec![
            ("w", "save workflow"),
            ("q", "quit"),
            ("r", "run workflow"),
            ("h", "show history"),
            ("new", "create a new workflow"),
            ("export <path>", "export the current workflow to JSON"),
            ("import <path>", "import a workflow from JSON"),
            ("undo", "undo last edit"),
            ("redo", "redo last undone edit"),
        ]
    }

    /// Try to match the current command buffer to a built-in command.
    /// Returns the resolved command name if matched.
    pub fn command_match(&self) -> Option<CommandKind> {
        let s = self.command_buffer.trim();
        if s.is_empty() {
            return None;
        }
        // Longest match first.
        let tokens: Vec<&str> = s.split_whitespace().collect();
        let first = *tokens.first()?;
        Some(match first {
            "w" | "write" => CommandKind::Save,
            "q" | "quit" => CommandKind::Quit,
            "r" | "run" => CommandKind::Run,
            "h" | "history" => CommandKind::History,
            "new" => CommandKind::New,
            "export" => CommandKind::Export(tokens.get(1).map(|s| s.to_string())),
            "import" => CommandKind::Import(tokens.get(1).map(|s| s.to_string())),
            "export-skill" => {
                CommandKind::ExportSkill(tokens.get(1).map(|s| s.to_string()))
            }
            "import-skill" => {
                CommandKind::ImportSkill(tokens.get(1).map(|s| s.to_string()))
            }
            "undo" => CommandKind::Undo,
            "redo" => CommandKind::Redo,
            _ => return None,
        })
    }

    /// Build a node filter map for the inspector. Indexed by node id.
    pub fn nodes_by_id(&self) -> HashMap<uuid::Uuid, &crate::model::NodeEntry> {
        self.manifest
            .as_ref()
            .map(|m| m.nodes.iter().map(|n| (n.id, n)).collect())
            .unwrap_or_default()
    }

    /// Compute the validation report from the current manifest.
    pub fn validate(&self, scheduler: &crate::executor::Scheduler) -> Option<ValidationReport> {
        self.manifest.as_ref().map(|m| scheduler.validate(m))
    }
}

/// Resolved command-palette command.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CommandKind {
    /// `:w` save
    Save,
    /// `:q` quit
    Quit,
    /// `:r` run
    Run,
    /// `:h` history
    History,
    /// `:new` create a fresh workflow
    New,
    /// `:export <path>`
    Export(Option<String>),
    /// `:import <path>`
    Import(Option<String>),
    /// `:export-skill <path>` — export the most recently listed skill.
    ExportSkill(Option<String>),
    /// `:import-skill <path>` — import a skill from JSON.
    ImportSkill(Option<String>),
    /// `:undo`
    Undo,
    /// `:redo`
    Redo,
}

impl CommandKind {
    /// Run the command if it has access to the API. Returns a string status.
    pub fn run(self, app: &mut super::app::App) -> Result<()> {
        use super::app::App;
        match self {
            CommandKind::Save => app.save_current(),
            CommandKind::Quit => app.request_quit(),
            CommandKind::Run => app.request_run(),
            CommandKind::History => app.open_history(),
            CommandKind::New => app.new_workflow(),
            CommandKind::Export(p) => app.export_to(p.as_deref()),
            CommandKind::Import(p) => app.import_from(p.as_deref()),
            CommandKind::ExportSkill(p) => app.export_skill_to(p.as_deref()),
            CommandKind::ImportSkill(p) => app.import_skill_from(p.as_deref()),
            CommandKind::Undo => {
                if !app.state.undo() {
                    app.state.flash("nothing to undo");
                }
                Ok(())
            }
            CommandKind::Redo => {
                if !app.state.redo() {
                    app.state.flash("nothing to redo");
                }
                Ok(())
            }
        }
    }
}