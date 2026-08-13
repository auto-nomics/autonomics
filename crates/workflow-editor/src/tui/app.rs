//! Top-level TUI app — event loop + state plumbing.

use std::io::{Stdout, stdout};
use std::path::PathBuf;
use std::time::Duration;

use crossterm::event::{
    self, DisableMouseCapture, EnableMouseCapture, Event, KeyCode, KeyEvent, KeyModifiers,
};
use crossterm::execute;
use crossterm::terminal::{
    EnterAlternateScreen, LeaveAlternateScreen, disable_raw_mode, enable_raw_mode,
};
use ratatui::Terminal;
use ratatui::backend::CrosstermBackend;
use tokio::sync::mpsc;

use crate::api::{WorkflowClient, WorkflowManager};
use crate::error::{Result, WorkflowError};
use crate::model::{EdgeEntry, NodeEntry, SnapshotInfo, WorkflowManifest};
use crate::registry::NodeRegistry;

use super::event::{AppEvent, AppEventSender};
use super::keymap::{Action, action_for, hint};
use super::layout::PaneLayout;
use super::mode::{Focus, Mode};
use super::state::{AppState, CommandKind};
use super::widgets::{
    canvas::CanvasWidget, command_palette::CommandPaletteWidget, help_overlay::HelpOverlayWidget,
    history_panel::HistoryPanelWidget, inspector::InspectorWidget, node_palette::NodePaletteWidget,
    sop_editor::SopEditorWidget, spec_form::SpecFormWidget, status_bar::StatusBarWidget,
};

type Term = Terminal<CrosstermBackend<Stdout>>;

/// The editor app.
pub struct App {
    /// Manager owning DB + registries.
    manager: WorkflowManager,
    /// Per-session client.
    client: WorkflowClient,
    /// Mutable UI state.
    pub state: AppState,
    /// In-memory node registry snapshot.
    pub node_registry: std::sync::Arc<NodeRegistry>,
    /// Event receiver (fed by the event-loop task).
    rx: mpsc::UnboundedReceiver<AppEvent>,
    /// Sender side (cloned for the event-loop task).
    tx: AppEventSender,
    /// Quit flag.
    quit: bool,
    /// Currently drawing an edge: source (node_id, port_id) if started.
    edge_draw_source: Option<(uuid::Uuid, String)>,
}

impl App {
    /// Create an `App` over the given `WorkflowManager`. The manager's
    /// session is consumed and a fresh event channel is wired up.
    pub fn new(manager: WorkflowManager) -> std::io::Result<Self> {
        let client = manager.new_session();
        let node_registry = manager.node_registry();
        let (tx, rx) = mpsc::unbounded_channel();
        Ok(Self {
            manager,
            client,
            state: AppState::default(),
            node_registry,
            rx,
            tx,
            quit: false,
            edge_draw_source: None,
        })
    }

    /// Run the TUI until the user quits.
    pub fn run(mut self) -> std::io::Result<()> {
        enable_raw_mode()?;
        let mut out = stdout();
        execute!(out, EnterAlternateScreen, EnableMouseCapture)?;
        let backend = CrosstermBackend::new(out);
        let mut terminal = Terminal::new(backend)?;

        // Spawn the event-loop task.
        let tx_clone = self.tx.clone();
        std::thread::spawn(move || {
            loop {
                if event::poll(Duration::from_millis(200)).unwrap_or(false) {
                    match event::read() {
                        Ok(Event::Key(k)) => {
                            if tx_clone.send(AppEvent::Key(k)).is_err() {
                                return;
                            }
                        }
                        Ok(Event::Mouse(m)) => {
                            if tx_clone.send(AppEvent::Mouse(m)).is_err() {
                                return;
                            }
                        }
                        Ok(Event::Resize(w, h)) => {
                            if tx_clone.send(AppEvent::Resize(w, h)).is_err() {
                                return;
                            }
                        }
                        Err(_) => return,
                        _ => {}
                    }
                } else {
                    if tx_clone.send(AppEvent::Tick).is_err() {
                        return;
                    }
                }
            }
        });

        // Pre-populate history panel: ask the actor for current workflow list.
        self.refresh_workflow_list();

        let res = self.event_loop(&mut terminal);
        disable_raw_mode().ok();
        execute!(
            terminal.backend_mut(),
            LeaveAlternateScreen,
            DisableMouseCapture
        )
        .ok();
        terminal.show_cursor().ok();
        res
    }

    fn event_loop(&mut self, terminal: &mut Term) -> std::io::Result<()> {
        loop {
            terminal.draw(|f| {
                let area = f.area();
                let layout = PaneLayout::new(area);
                self.render(f, layout);
            })?;

            // Block on the next event (200ms is enforced by the polling thread).
            let ev = match self.rx.blocking_recv() {
                Some(ev) => ev,
                None => return Ok(()),
            };
            self.handle_event(ev);
            if self.quit {
                return Ok(());
            }
        }
    }

    fn render(&self, f: &mut ratatui::Frame<'_>, layout: PaneLayout) {
        use ratatui::widgets::{Block, Borders};

        // Header
        f.render_widget(
            HeaderWidget {
                name: self
                    .state
                    .manifest
                    .as_ref()
                    .map(|m| m.name.clone())
                    .unwrap_or_else(|| "<none>".into()),
                dirty: self.state.dirty,
                mode: self.state.mode,
            },
            layout.header,
        );

        // Palette
        let palette_block = Block::default().borders(Borders::ALL).title("palette");
        let palette_inner = palette_block.inner(layout.palette);
        f.render_widget(palette_block, layout.palette);
        f.render_widget(
            NodePaletteWidget {
                registry: &self.node_registry,
                focus: self.state.focus == Focus::Palette,
            },
            palette_inner,
        );

        // Canvas
        let canvas_block = Block::default().borders(Borders::ALL).title("canvas");
        let canvas_inner = canvas_block.inner(layout.canvas);
        f.render_widget(canvas_block, layout.canvas);
        f.render_widget(
            CanvasWidget {
                state: &self.state,
                focus: self.state.focus == Focus::Canvas,
                edge_draw_source: self.edge_draw_source.clone(),
            },
            canvas_inner,
        );

        // Inspector
        let inspector_block = Block::default().borders(Borders::ALL).title("inspector");
        let inspector_inner = inspector_block.inner(layout.inspector);
        f.render_widget(inspector_block, layout.inspector);
        f.render_widget(
            InspectorWidget {
                state: &self.state,
                focus: self.state.focus == Focus::Inspector,
            },
            inspector_inner,
        );

        // Status bar
        f.render_widget(
            StatusBarWidget {
                mode: self.state.mode,
                status: &self.state.status,
                hint: hint(self.state.mode),
            },
            layout.status,
        );

        // Overlays (rendered last so they sit on top).
        match self.state.mode {
            Mode::HelpOverlay => {
                let area = centered(60, 70, layout.root);
                f.render_widget(HelpOverlayWidget {}, area);
            }
            Mode::CommandPalette => {
                let area = centered(60, 20, layout.root);
                f.render_widget(
                    CommandPaletteWidget {
                        buffer: &self.state.command_buffer,
                        suggestions: self.state.command_suggestions(),
                    },
                    area,
                );
            }
            Mode::SopEditor => {
                let area = centered(70, 60, layout.root);
                f.render_widget(
                    SopEditorWidget {
                        buffer: &self.state.sop_buffer,
                    },
                    area,
                );
            }
            Mode::SpecEditor => {
                let area = centered(70, 70, layout.root);
                f.render_widget(
                    SpecFormWidget {
                        buffer: &self.state.spec_buffer,
                    },
                    area,
                );
            }
            Mode::NodePicker => {
                let area = centered(60, 70, layout.root);
                let kinds: Vec<String> = self
                    .state
                    .picker_filter(&self.node_registry)
                    .into_iter()
                    .map(String::from)
                    .collect();
                f.render_widget(
                    NodePaletteWidget {
                        registry: &self.node_registry,
                        focus: true,
                    },
                    area,
                );
                // Draw the picker as a search overlay (reuse the same widget):
                let _ = kinds;
            }
            Mode::HistoryPanel => {
                let area = centered(80, 80, layout.root);
                f.render_widget(
                    HistoryPanelWidget {
                        history: &self.state.history,
                        selected: self.state.selected_node,
                    },
                    area,
                );
            }
            _ => {}
        }
    }

    fn handle_event(&mut self, ev: AppEvent) {
        // Text input modes always consume the character first.
        if self.state.mode.is_text_input() {
            if !matches!(
                ev,
                AppEvent::Key(KeyEvent {
                    code: KeyCode::Esc,
                    ..
                })
            ) {
                self.state.apply_text_event(&ev);
            }
        }

        if let Some(action) = action_for(self.state.mode, &ev) {
            self.apply_action(action);
        } else {
            self.apply_modal_event(&ev);
        }
    }

    fn apply_action(&mut self, action: Action) {
        match action {
            Action::Quit => {
                let _ = self.request_quit();
            }
            Action::SaveWorkflow => {
                let _ = self.save_current();
            }
            Action::RunWorkflow => {
                let _ = self.request_run();
            }
            Action::Undo => {
                if !self.state.undo() {
                    self.state.flash("nothing to undo");
                }
            }
            Action::Redo => {
                if !self.state.redo() {
                    self.state.flash("nothing to redo");
                }
            }
            Action::OpenCommandPalette => {
                self.state.mode = Mode::CommandPalette;
                self.state.command_buffer.clear();
            }
            Action::ShowHelp => self.state.mode = Mode::HelpOverlay,
            Action::OpenHistory => {
                let _ = self.open_history();
            }
            Action::OpenNodePicker => {
                self.state.mode = Mode::NodePicker;
                self.state.picker_buffer.clear();
                self.state.picker_index = 0;
            }
            Action::FocusPalette => self.state.focus = Focus::Palette,
            Action::FocusCanvas => self.state.focus = Focus::Canvas,
            Action::FocusInspector => self.state.focus = Focus::Inspector,
            Action::PanUp => self.state.pan(0, -1),
            Action::PanDown => self.state.pan(0, 1),
            Action::PanLeft => self.state.pan(-1, 0),
            Action::PanRight => self.state.pan(1, 0),
            Action::ZoomIn => self.state.zoom(1),
            Action::ZoomOut => self.state.zoom(-1),
            Action::SelectNextNode => self.state.select_next_node(),
            Action::SelectPrevNode => self.state.select_prev_node(),
            Action::DeleteSelection => self.delete_selection(),
            Action::EditSelection => self.edit_selection(),
            Action::StartEdgeDraw => self.start_edge_draw(),
            Action::EditSop => self.start_sop_edit(),
            Action::EnterInsertMode => self.state.mode = Mode::Insert,
            Action::SelectNextHistory => {
                if !self.state.history.is_empty() {
                    let cur = self.state.history_index.unwrap_or(0);
                    let next = (cur + 1).min(self.state.history.len() - 1);
                    self.state.history_index = Some(next);
                }
            }
            Action::SelectPrevHistory => {
                if !self.state.history.is_empty() {
                    let cur = self.state.history_index.unwrap_or(0);
                    let prev = cur.saturating_sub(1);
                    self.state.history_index = Some(prev);
                }
            }
            Action::CheckoutSnapshot => {
                let _ = self.checkout_snapshot();
            }
        };
    }

    fn apply_modal_event(&mut self, ev: &AppEvent) {
        let AppEvent::Key(k) = ev else { return };
        match self.state.mode {
            Mode::CommandPalette => {
                if matches!(k.code, KeyCode::Enter) {
                    if let Some(cmd) = self.state.command_match() {
                        match cmd.run(self) {
                            Ok(()) => self.state.flash("ok"),
                            Err(e) => self.state.flash_err(e.to_string()),
                        }
                    } else {
                        self.state
                            .flash_err(format!("unknown command: {}", self.state.command_buffer));
                    }
                    self.state.command_buffer.clear();
                    self.state.mode = Mode::Normal;
                }
            }
            Mode::SopEditor => {
                if matches!(k.code, KeyCode::Esc) {
                    self.commit_sop_edit();
                }
            }
            Mode::SpecEditor => {
                if matches!(k.code, KeyCode::Esc) {
                    self.commit_spec_edit();
                }
            }
            Mode::NodePicker => {
                if matches!(k.code, KeyCode::Enter) {
                    self.place_picked_node();
                }
            }
            Mode::HelpOverlay | Mode::HistoryPanel => {
                if matches!(k.code, KeyCode::Esc | KeyCode::Char('q')) {
                    self.state.mode = Mode::Normal;
                }
            }
            Mode::EdgeDrawing => {
                if matches!(k.code, KeyCode::Esc) {
                    self.edge_draw_source = None;
                    self.state.mode = Mode::Normal;
                }
            }
            _ => {}
        }
    }

    // ── Action handlers exposed to CommandKind ──────────────────────────

    /// Save the current workflow.
    pub fn save_current(&mut self) -> Result<()> {
        let Some(m) = self.state.manifest.clone() else {
            self.state.flash_err("nothing to save");
            return Ok(());
        };
        let manifest = m.clone();
        // Sync save via repo (bypasses actor for snappier UI).
        self.manager
            .workflow_repo()
            .save(manifest.id, &manifest, "save")?;
        self.state.dirty = false;
        self.state.flash(format!("saved workflow {}", manifest.id));
        Ok(())
    }

    /// Request quit.
    pub fn request_quit(&mut self) -> Result<()> {
        if self.state.dirty {
            self.state
                .flash_err("unsaved changes — :w to save, :q! to discard (TODO)");
            return Ok(());
        }
        self.quit = true;
        Ok(())
    }

    /// Kick off a workflow run. Phase 1 uses a sync repo + scheduler call
    /// to keep the TUI responsive; Phase 4 will move this to a background
    /// task with progress reporting.
    pub fn request_run(&mut self) -> Result<()> {
        let Some(m) = self.state.manifest.clone() else {
            self.state.flash_err("nothing to run");
            return Ok(());
        };
        self.state.mode = Mode::Running;
        // Sync run via scheduler (no cancellation in Phase 1 TUI yet).
        let sched = self.manager.scheduler.clone();
        let result = futures::executor::block_on(async {
            sched
                .run(
                    &m,
                    Default::default(),
                    tokio_util::sync::CancellationToken::new(),
                )
                .await
        });
        match result {
            Ok(r) => {
                self.state.last_run = Some(r);
                self.state.flash("run finished");
            }
            Err(e) => self.state.flash_err(format!("run failed: {e}")),
        }
        self.state.mode = Mode::Normal;
        Ok(())
    }

    /// Open history panel: ask actor for snapshot history.
    pub fn open_history(&mut self) -> Result<()> {
        let Some(m) = &self.state.manifest else {
            self.state.flash_err("no workflow");
            return Ok(());
        };
        let id = m.id;
        let mgr_client = self.client.clone();
        let history_res = futures::executor::block_on(async move { mgr_client.history(id).await });
        match history_res {
            Ok(h) => {
                self.state.history = h;
                self.state.history_index = None;
                self.state.mode = Mode::HistoryPanel;
                self.state.flash("history loaded");
            }
            Err(e) => self.state.flash_err(e.to_string()),
        }
        Ok(())
    }

    /// Restore the workflow to the snapshot currently highlighted in the
    /// history panel. After a successful checkout the workflow is dirty
    /// (the working copy diverges from the newly restored HEAD).
    pub fn checkout_snapshot(&mut self) -> Result<()> {
        let Some(m) = &self.state.manifest else {
            self.state.flash_err("no workflow");
            return Ok(());
        };
        let wf_id = m.id;
        let Some(idx) = self.state.history_index else {
            self.state.flash_err("no snapshot selected");
            return Ok(());
        };
        let Some(snap) = self.state.history.get(idx).cloned() else {
            self.state.flash_err("snapshot out of range");
            return Ok(());
        };
        // Push current manifest onto the undo stack so the checkout itself
        // is reversible.
        self.state.push_undo("checkout");
        let mgr_client = self.client.clone();
        let res =
            futures::executor::block_on(async move { mgr_client.checkout(wf_id, snap.id).await });
        match res {
            Ok(()) => {
                // Reload manifest from storage so the canvas reflects the
                // restored state.
                let mgr_client = self.client.clone();
                let loaded =
                    futures::executor::block_on(
                        async move { mgr_client.load_workflow(wf_id).await },
                    );
                match loaded {
                    Ok(new_manifest) => {
                        self.state.manifest = Some(new_manifest);
                        self.state.mode = Mode::Normal;
                        self.state.dirty = true;
                        self.state
                            .flash(format!("checked out {}", &snap.commit_message));
                    }
                    Err(e) => self.state.flash_err(e.to_string()),
                }
            }
            Err(e) => self.state.flash_err(e.to_string()),
        }
        Ok(())
    }

    /// Create a fresh workflow.
    pub fn new_workflow(&mut self) -> Result<()> {
        let m = WorkflowManifest::new("untitled");
        self.manager.workflow_repo().create(&m)?;
        self.state.manifest = Some(m);
        self.state.selected_node = None;
        self.state.selected_edge = None;
        self.state.undo.clear();
        self.state.redo.clear();
        self.state.dirty = false;
        self.state.flash("created new workflow");
        Ok(())
    }

    /// Export current workflow to a JSON file.
    pub fn export_to(&mut self, path: Option<&str>) -> Result<()> {
        let Some(m) = &self.state.manifest else {
            self.state.flash_err("no workflow");
            return Ok(());
        };
        let path: PathBuf = match path {
            Some(p) => PathBuf::from(p),
            None => {
                self.state.flash_err("usage: :export <path>");
                return Ok(());
            }
        };
        let json = serde_json::to_string_pretty(m)?;
        std::fs::write(&path, json)?;
        self.state.flash(format!("exported to {}", path.display()));
        Ok(())
    }

    /// Import a workflow from a JSON file.
    pub fn import_from(&mut self, path: Option<&str>) -> Result<()> {
        let path: PathBuf = match path {
            Some(p) => PathBuf::from(p),
            None => {
                self.state.flash_err("usage: :import <path>");
                return Ok(());
            }
        };
        let json = std::fs::read_to_string(&path)?;
        let m: WorkflowManifest = serde_json::from_str(&json)?;
        self.manager.workflow_repo().create(&m)?;
        self.state.manifest = Some(m);
        self.state.dirty = false;
        self.state.undo.clear();
        self.state.redo.clear();
        self.state
            .flash(format!("imported from {}", path.display()));
        Ok(())
    }

    /// Export the currently-loaded skill (selected by the user beforehand via
    /// the node picker or set via `--skill` flag at startup) to JSON.
    ///
    /// For Phase 5 the TUI expects the user to have just listed skills; the
    /// most recent list result's first skill is exported. Without selection
    /// state the command errors.
    pub fn export_skill_to(&mut self, path: Option<&str>) -> Result<()> {
        let path: PathBuf = match path {
            Some(p) => PathBuf::from(p),
            None => {
                self.state.flash_err("usage: :export-skill <path>");
                return Ok(());
            }
        };
        // Pick a skill: prefer a stored selection; otherwise first in list.
        let client = self.client.clone();
        let summaries = futures::executor::block_on(async move { client.list_skills().await })?;
        let pick = match self.state.selected_skill.take() {
            Some(id) => id,
            None => match summaries.first() {
                Some(s) => s.id,
                None => {
                    self.state.flash_err("no skills to export");
                    return Ok(());
                }
            },
        };
        let client = self.client.clone();
        let json = futures::executor::block_on(async move { client.export_skill(pick).await })?;
        std::fs::write(&path, json)?;
        self.state
            .flash(format!("exported skill to {}", path.display()));
        Ok(())
    }

    /// Import a skill from a JSON file.
    pub fn import_skill_from(&mut self, path: Option<&str>) -> Result<()> {
        let path: PathBuf = match path {
            Some(p) => PathBuf::from(p),
            None => {
                self.state.flash_err("usage: :import-skill <path>");
                return Ok(());
            }
        };
        let json = std::fs::read_to_string(&path)?;
        let client = self.client.clone();
        let saved = futures::executor::block_on(async move { client.import_skill(json).await })?;
        self.state.flash(format!(
            "imported skill '{}' as v{}",
            saved.name, saved.version
        ));
        Ok(())
    }

    // ── Lower-level helpers ─────────────────────────────────────────────

    fn delete_selection(&mut self) {
        if self.state.manifest.is_none() {
            self.state.flash_err("no manifest loaded");
            return;
        }
        let mut did_delete = false;
        let mut label = "nothing selected";
        {
            let m = self.state.manifest.as_mut().unwrap();
            if let Some(nid) = self.state.selected_node.take() {
                m.nodes.retain(|n| n.id != nid);
                m.edges.retain(|e| e.source != nid && e.target != nid);
                did_delete = true;
                label = "deleted node";
            } else if let Some(eid) = self.state.selected_edge.take() {
                m.edges.retain(|e| e.id != eid);
                did_delete = true;
                label = "deleted edge";
            }
        }
        if did_delete {
            self.state.push_undo("delete_selection");
            self.state.touch();
        }
        self.state.flash(label);
    }

    fn edit_selection(&mut self) {
        let Some(m) = self.state.manifest.clone() else {
            return;
        };
        let Some(nid) = self.state.selected_node else {
            self.state.flash_err("no node selected");
            return;
        };
        if let Some(node) = m.nodes.iter().find(|n| n.id == nid).cloned() {
            // If this is a skill node, opening the spec form edits params;
            // for an SOP-bearing node, route to the SOP editor instead.
            if node.kind == "skill" {
                self.state.mode = Mode::SopEditor;
                self.state.sop_buffer = node
                    .params
                    .get("sop_text")
                    .and_then(|v| v.as_str())
                    .unwrap_or("")
                    .to_string();
            } else {
                self.state.mode = Mode::SpecEditor;
                self.state.spec_buffer =
                    serde_json::to_string_pretty(&node.params).unwrap_or_default();
            }
        }
    }

    fn start_edge_draw(&mut self) {
        let Some(m) = &self.state.manifest else {
            return;
        };
        let Some(nid) = self.state.selected_node else {
            self.state.flash_err("select a source node first");
            return;
        };
        let Some(node) = m.nodes.iter().find(|n| n.id == nid) else {
            return;
        };
        let port = node
            .outputs
            .first()
            .map(|p| p.id.clone())
            .unwrap_or_else(|| "out".into());
        self.edge_draw_source = Some((nid, port));
        self.state.mode = Mode::EdgeDrawing;
        self.state.flash("select target node + port");
    }

    fn start_sop_edit(&mut self) {
        let Some(m) = self.state.manifest.clone() else {
            return;
        };
        self.state.mode = Mode::SopEditor;
        self.state.sop_buffer = m.sop.clone().unwrap_or_default();
    }

    fn commit_sop_edit(&mut self) {
        if self.state.manifest.is_none() {
            return;
        }
        let new_sop = if self.state.sop_buffer.is_empty() {
            None
        } else {
            Some(self.state.sop_buffer.clone())
        };
        self.state.push_undo("edit_sop");
        if let Some(m) = self.state.manifest.as_mut() {
            m.sop = new_sop;
        }
        self.state.mode = Mode::Normal;
        self.state.touch();
        self.state.flash("sop updated");
    }

    fn commit_spec_edit(&mut self) {
        let Some(nid) = self.state.selected_node else {
            return;
        };
        let new_params: serde_json::Value = match serde_json::from_str(&self.state.spec_buffer) {
            Ok(v) => v,
            Err(e) => {
                self.state.flash_err(format!("bad json: {e}"));
                return;
            }
        };
        self.state.push_undo("edit_params");
        if let Some(m) = self.state.manifest.as_mut() {
            if let Some(node) = m.nodes.iter_mut().find(|n| n.id == nid) {
                node.params = new_params;
            }
        }
        self.state.mode = Mode::Normal;
        self.state.touch();
        self.state.flash("params updated");
    }

    fn place_picked_node(&mut self) {
        let kinds = self.state.picker_filter(&self.node_registry);
        let Some(chosen) = kinds.get(self.state.picker_index).cloned() else {
            self.state.flash_err("nothing to place");
            return;
        };
        if self.state.manifest.is_none() {
            self.state.flash_err("no manifest loaded");
            return;
        }
        let id = uuid::Uuid::new_v4();
        let kind_info = self
            .node_registry
            .list()
            .into_iter()
            .find(|k| k.kind == chosen);
        let (label, category) = kind_info
            .map(|k| (k.label.clone(), k.category.clone()))
            .unwrap_or_else(|| (chosen.clone(), "?".to_string()));
        let pos = {
            let m = self.state.manifest.as_ref().unwrap();
            (
                m.viewport.pan.0 + (m.canvas_size_hint().0 / 2) as i32,
                m.viewport.pan.1 + (m.canvas_size_hint().1 / 2) as i32,
            )
        };
        self.state.push_undo("add_node");
        if let Some(m) = self.state.manifest.as_mut() {
            m.nodes.push(NodeEntry {
                id,
                kind: chosen.clone(),
                label,
                position: pos,
                inputs: vec![crate::model::PortSpec::new("in", "in")],
                outputs: vec![crate::model::PortSpec::new("out", "out")],
                params: serde_json::json!({ "_category": category }),
            });
        }
        self.state.selected_node = Some(id);
        self.state.mode = Mode::Normal;
        self.state.touch();
        self.state.flash(format!("placed {chosen}"));
    }

    fn refresh_workflow_list(&mut self) {
        let summaries = futures::executor::block_on(async { self.client.list_workflows().await });
        if let Ok(list) = summaries {
            if let Some(first) = list.first() {
                if let Ok(m) =
                    futures::executor::block_on(async { self.client.load_workflow(first.id).await })
                {
                    self.state.manifest = Some(m);
                    self.state.flash(format!("loaded workflow {}", first.name));
                }
            }
        }
    }
}

/// Header widget (title + dirty marker).
struct HeaderWidget {
    name: String,
    dirty: bool,
    mode: Mode,
}

impl ratatui::widgets::Widget for HeaderWidget {
    fn render(self, area: ratatui::layout::Rect, buf: &mut ratatui::buffer::Buffer) {
        use ratatui::style::{Modifier, Style};
        use ratatui::text::{Line, Span};
        use ratatui::widgets::Widget;
        let title = format!(" workflow: {} ", self.name);
        let dirty = if self.dirty { " (modified)" } else { "" };
        let mode = format!(" [{}] ", self.mode.label());
        let title_len = title.chars().count();
        let dirty_len = dirty.chars().count();
        let mode_len = mode.chars().count();
        let used = title_len + dirty_len + mode_len;
        let available = area.width as usize;
        let pad = if available > used {
            " ".repeat(available - used)
        } else {
            String::new()
        };
        let line = Line::from(vec![
            Span::styled(title, Style::default().add_modifier(Modifier::BOLD)),
            Span::styled(dirty, Style::default().add_modifier(Modifier::ITALIC)),
            Span::raw(pad),
            Span::styled(mode, Style::default().add_modifier(Modifier::REVERSED)),
        ]);
        Widget::render(line, area, buf);
    }
}

/// Center a rectangle inside `area` with the given width / height percentages.
fn centered(w_pct: u16, h_pct: u16, area: ratatui::layout::Rect) -> ratatui::layout::Rect {
    use ratatui::layout::{Constraint, Direction, Layout, Rect};
    let h = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([
            Constraint::Percentage((100 - w_pct) / 2),
            Constraint::Percentage(w_pct),
            Constraint::Percentage((100 - w_pct) / 2),
        ])
        .split(area)[1];
    Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Percentage((100 - h_pct) / 2),
            Constraint::Percentage(h_pct),
            Constraint::Percentage((100 - h_pct) / 2),
        ])
        .split(h)[1]
}

// ── Manifest convenience extension ──────────────────────────────────────

trait ManifestExt {
    fn canvas_size_hint(&self) -> (u16, u16);
}

impl ManifestExt for WorkflowManifest {
    fn canvas_size_hint(&self) -> (u16, u16) {
        if let (Some(min_x), Some(max_x), Some(min_y), Some(max_y)) = (
            self.nodes.iter().map(|n| n.position.0).min(),
            self.nodes.iter().map(|n| n.position.0).max(),
            self.nodes.iter().map(|n| n.position.1).min(),
            self.nodes.iter().map(|n| n.position.1).max(),
        ) {
            ((max_x - min_x + 20) as u16, (max_y - min_y + 10) as u16)
        } else {
            (60, 20)
        }
    }
}
