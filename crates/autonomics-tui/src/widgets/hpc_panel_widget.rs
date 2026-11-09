//! Compute panel — edit the selected Slurm target and inspect jobs and targets.

use ratatui::{
    buffer::Buffer,
    layout::{Constraint, Layout, Rect},
    style::{Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Borders, Paragraph, StatefulWidgetRef, Widget},
};

use crate::theme;

// ═══════════════════════════════════════════════════════════════════════
// Editable field schema
// ═══════════════════════════════════════════════════════════════════════

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FieldKind {
    Bool,
    /// Free text; empty clears the profile entry (falls back to env/default).
    Text,
    /// Same as [`FieldKind::Text`] but validated as an unsigned integer.
    Number,
}

/// One editable site-profile row. `key` is the JSON key on both the
/// `GET /hpc/config` `config` object and the `PUT` profile body.
pub struct FieldDef {
    pub key: &'static str,
    pub label: &'static str,
    pub kind: FieldKind,
}

/// Operator-editable fields, in panel display order. Read-only derived
/// state (effective snapshot, manifest contents) renders below this list.
pub const FIELD_DEFS: &[FieldDef] = &[
    FieldDef {
        key: "tools_enabled",
        label: "HPC account tools",
        kind: FieldKind::Bool,
    },
    FieldDef {
        key: "ssh_host",
        label: "SSH host",
        kind: FieldKind::Text,
    },
    FieldDef {
        key: "ssh_command",
        label: "SSH command",
        kind: FieldKind::Text,
    },
    FieldDef {
        key: "scratch_remote_dir",
        label: "Remote scratch dir",
        kind: FieldKind::Text,
    },
    FieldDef {
        key: "artifact_root",
        label: "Local artifact root",
        kind: FieldKind::Text,
    },
    FieldDef {
        key: "partition",
        label: "Partition",
        kind: FieldKind::Text,
    },
    FieldDef {
        key: "account",
        label: "Account",
        kind: FieldKind::Text,
    },
    FieldDef {
        key: "mem_per_cpu",
        label: "Mem per CPU",
        kind: FieldKind::Text,
    },
    FieldDef {
        key: "default_time",
        label: "Default time limit",
        kind: FieldKind::Text,
    },
    FieldDef {
        key: "gpu_partition",
        label: "GPU partition",
        kind: FieldKind::Text,
    },
    FieldDef {
        key: "gpu_cpus_per_gpu",
        label: "CPUs per GPU",
        kind: FieldKind::Number,
    },
    FieldDef {
        key: "gpu_qos",
        label: "GPU QOS",
        kind: FieldKind::Text,
    },
    FieldDef {
        key: "poll_interval_secs",
        label: "Poll interval (s)",
        kind: FieldKind::Number,
    },
    FieldDef {
        key: "ssh_connect_timeout_secs",
        label: "SSH connect timeout (s)",
        kind: FieldKind::Number,
    },
    FieldDef {
        key: "apptainer_manifest",
        label: "Apptainer manifest",
        kind: FieldKind::Text,
    },
    FieldDef {
        key: "typed_worker_config",
        label: "Typed worker profile",
        kind: FieldKind::Text,
    },
    FieldDef {
        key: "panel_cache_remote_dir",
        label: "Panel cache remote dir",
        kind: FieldKind::Text,
    },
    FieldDef {
        key: "data_ssh_host",
        label: "Data SSH host",
        kind: FieldKind::Text,
    },
    FieldDef {
        key: "pi_ssh_host",
        label: "Pi SSH host",
        kind: FieldKind::Text,
    },
    FieldDef {
        key: "remote_python",
        label: "Remote python",
        kind: FieldKind::Text,
    },
];

// ═══════════════════════════════════════════════════════════════════════
// State
// ═══════════════════════════════════════════════════════════════════════

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum HpcTab {
    #[default]
    Config,
    Jobs,
    Targets,
}

/// One enrolled compute target from `GET /compute`.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct HpcTargetRow {
    pub id: String,
    pub revision: u64,
    pub selected: bool,
    pub enabled: bool,
    /// `local` / `slurm` / `ssh`.
    pub backend: String,
    /// Frozen endpoint identities: (alias, hostname, port).
    pub endpoints: Vec<(String, String, u16)>,
    /// Declared data domains: `id → vfs_prefix (remote_root)`.
    pub domains: Vec<String>,
}

/// One VFS mount partition from `GET /compute`.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct HpcMountRow {
    pub path: String,
    pub backend: String,
    pub read_only: bool,
    pub domain: Option<String>,
}

/// One monitor row from `GET /hpc/jobs`.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct HpcJobRow {
    pub id: String,
    pub job_id: Option<String>,
    pub state: String,
    pub scheduler_state: Option<String>,
    pub agent_path: String,
    pub cluster: String,
    pub last_error: Option<String>,
    pub updated_at: u64,
}

#[derive(Debug, Clone, Default)]
pub struct HpcPanelState {
    pub visible: bool,
    pub tab: HpcTab,
    /// Current profile values, keyed by [`FIELD_DEFS`] order.
    pub values: Vec<String>,
    pub selected_field: usize,
    pub jobs: Vec<HpcJobRow>,
    pub selected_job: usize,
    /// Enrolled compute targets with their frozen endpoints.
    pub targets: Vec<HpcTargetRow>,
    /// VFS mount partitions (explicit domain separations).
    pub mounts: Vec<HpcMountRow>,
    pub selected_target: usize,
    /// Inline edit buffer — `Some` while a text/number field is being edited.
    pub edit_buffer: Option<String>,
    pub edit_cursor: usize,
    /// One-line operator feedback (save result, refresh errors).
    pub status: Option<String>,
    pub loading: bool,
    pub target_id: Option<String>,
    pub target_revision: u64,
    pub registry_revision: u64,
    pub target_editable: bool,
    pub dashboard_ready: bool,
    pub dashboard_targets: Vec<serde_json::Value>,
    pub dashboard_focus_form: bool,
    pub dashboard_editing: bool,
    pub dashboard_new: bool,
    pub dashboard_confirm_delete: bool,
    pub dashboard_draft: serde_json::Value,
    pub dashboard_fields: Vec<String>,
    pub dashboard_field: usize,
    pub dashboard_pending_id: Option<String>,
    pub dashboard_show_jobs: bool,
    pub repair_events: Vec<String>,
    pub repair_show_events: bool,
}

impl HpcPanelState {
    pub fn push_repair_event(&mut self, message: String) {
        if self.repair_events.len() >= 200 { self.repair_events.remove(0); }
        let mut bounded: String = message.chars().take(8192).collect();
        if bounded.len() < message.len() { bounded.push_str("… [truncated]"); }
        self.repair_events.push(bounded);
    }
    fn dashboard_visible_fields(&self) -> Vec<usize> {
        if self
            .dashboard_fields
            .get(2)
            .is_some_and(|kind| kind == "local")
        {
            vec![0, 1, 2, 3, 4 + FIELD_DEFS.len()]
        } else if self
            .dashboard_fields
            .get(2)
            .is_some_and(|kind| kind == "ssh")
        {
            (0..self.dashboard_fields.len())
                .filter(|&index| {
                    index < 4
                        || index == 4 + FIELD_DEFS.len()
                        || !matches!(
                            FIELD_DEFS[index - 4].key,
                            "partition"
                                | "account"
                                | "mem_per_cpu"
                                | "default_time"
                                | "gpu_partition"
                                | "gpu_cpus_per_gpu"
                                | "gpu_qos"
                                | "poll_interval_secs"
                                | "apptainer_manifest"
                                | "tools_enabled"
                        )
                })
                .collect()
        } else {
            (0..self.dashboard_fields.len()).collect()
        }
    }

    fn dashboard_move_field(&mut self, down: bool) {
        self.status = None;
        let visible = self.dashboard_visible_fields();
        let index = visible
            .iter()
            .position(|field| *field == self.dashboard_field)
            .unwrap_or(0);
        let next = if down {
            (index + 1).min(visible.len().saturating_sub(1))
        } else {
            index.saturating_sub(1)
        };
        if let Some(field) = visible.get(next) {
            self.dashboard_field = *field;
        }
    }

    fn dashboard_field_help(&self) -> &'static str {
        match self.dashboard_field {
            0 => "ID: letters, digits, _ or -; fixed after creation.",
            1 => "Name: a recognizable display name.",
            2 if self.dashboard_new => "Backend: Enter cycles Slurm, SSH and local.",
            2 => "Backend is fixed after creation.",
            3 => "Enabled: Enter toggles whether the target can be selected.",
            index if index == 4 + FIELD_DEFS.len() => {
                "Data domains: JSON array; [] means no in-place data domain."
            }
            index if index >= 4 => match FIELD_DEFS[index - 4].key {
                "ssh_host" => "SSH host: a configured alias or user@host, available to the daemon.",
                "typed_worker_config" => {
                    "Worker profile: absolute path on the daemon; required for new Slurm targets."
                }
                "apptainer_manifest" => {
                    "Image manifest: absolute daemon-side path; needed for remote container workloads."
                }
                "scratch_remote_dir" => {
                    "Scratch directory on the remote host; ~/ expands to the remote home."
                }
                "account" | "gpu_qos" | "data_ssh_host" | "pi_ssh_host" => {
                    "Optional: leave blank to clear."
                }
                _ => "Leave blank to use the default; Enter edits the value.",
            },
            _ => "",
        }
    }

    pub fn paste_into_edit(&mut self, text: &str) {
        if let Some(buffer) = &mut self.edit_buffer {
            self.status = None;
            let offset = buffer
                .char_indices()
                .nth(self.edit_cursor)
                .map(|(i, _)| i)
                .unwrap_or(buffer.len());
            let clean = text.replace(['\r', '\n'], " ");
            buffer.insert_str(offset, &clean);
            self.edit_cursor += clean.chars().count();
        }
    }
    pub fn load_dashboard(&mut self, status: &serde_json::Value) {
        if self.tab == HpcTab::Config {
            self.tab = HpcTab::Targets;
        }
        let registry = &status["state"];
        let old_id = self.dashboard_pending_id.take().or_else(|| {
            self.dashboard_targets
                .get(self.selected_target)
                .and_then(|t| t["id"].as_str())
                .map(str::to_owned)
        });
        let mut latest = std::collections::BTreeMap::<String, serde_json::Value>::new();
        if let Some(targets) = registry["targets"].as_array() {
            for target in targets {
                if let Some(id) = target["id"].as_str() {
                    if latest
                        .get(id)
                        .and_then(|t| t["revision"].as_u64())
                        .unwrap_or(0)
                        < target["revision"].as_u64().unwrap_or(0)
                    {
                        latest.insert(id.to_string(), target.clone());
                    }
                }
            }
        }
        if let Some(deleted) = registry["deleted_target_ids"].as_array() {
            for id in deleted.iter().filter_map(|v| v.as_str()) {
                latest.remove(id);
            }
        }
        self.dashboard_targets = latest.into_values().collect();
        self.registry_revision = registry["revision"].as_u64().unwrap_or(0);
        self.target_id = registry["policy"]["target_id"].as_str().map(str::to_owned);
        self.target_revision = registry["policy"]["target_revision"].as_u64().unwrap_or(0);
        self.selected_target = old_id
            .as_deref()
            .and_then(|id| {
                self.dashboard_targets
                    .iter()
                    .position(|target| target["id"] == id)
            })
            .or_else(|| {
                self.target_id.as_deref().and_then(|id| {
                    self.dashboard_targets
                        .iter()
                        .position(|target| target["id"] == id)
                })
            })
            .unwrap_or(0);
        self.dashboard_ready = true;
        self.dashboard_editing = false;
        self.dashboard_new = false;
        self.dashboard_confirm_delete = false;
        self.dashboard_focus_form = self.tab == HpcTab::Jobs;
        self.dashboard_show_jobs = self.tab == HpcTab::Jobs;
        self.load_dashboard_draft();
    }

    fn load_dashboard_draft(&mut self) {
        self.dashboard_draft = self
            .dashboard_targets
            .get(self.selected_target)
            .cloned()
            .unwrap_or_default();
        self.dashboard_fields = vec![
            self.dashboard_draft["id"].as_str().unwrap_or("").into(),
            self.dashboard_draft["name"].as_str().unwrap_or("").into(),
            self.dashboard_draft["backend"]["type"]
                .as_str()
                .unwrap_or("local")
                .into(),
            self.dashboard_draft["enabled"]
                .as_bool()
                .unwrap_or(true)
                .to_string(),
        ];
        let settings = &self.dashboard_draft["backend"]["settings"];
        self.dashboard_fields
            .extend(FIELD_DEFS.iter().map(|field| match &settings[field.key] {
                serde_json::Value::String(v) => v.clone(),
                serde_json::Value::Bool(v) => v.to_string(),
                serde_json::Value::Number(v) => v.to_string(),
                _ => String::new(),
            }));
        self.dashboard_fields.push(
            self.dashboard_draft
                .get("data_domains")
                .map(serde_json::Value::to_string)
                .unwrap_or_else(|| "[]".into()),
        );
        self.dashboard_field = 0;
    }

    fn dashboard_save_command(&mut self) -> Option<HpcPanelCommand> {
        match self.dashboard_save() {
            Ok(request) => {
                self.status = None;
                Some(HpcPanelCommand::SaveTarget(request))
            }
            Err(error) => {
                self.dashboard_field = if error.contains("ID") {
                    0
                } else if error.contains("name") {
                    1
                } else if error.contains("Backend") {
                    2
                } else if error.contains("SSH host") {
                    4 + FIELD_DEFS
                        .iter()
                        .position(|field| field.key == "ssh_host")
                        .unwrap()
                } else if error.contains("Worker profile") {
                    4 + FIELD_DEFS
                        .iter()
                        .position(|field| field.key == "typed_worker_config")
                        .unwrap()
                } else if error.contains("Data domains") {
                    4 + FIELD_DEFS.len()
                } else {
                    FIELD_DEFS
                        .iter()
                        .position(|field| error.contains(field.label))
                        .map(|index| index + 4)
                        .unwrap_or(self.dashboard_field)
                };
                self.dashboard_focus_form = true;
                self.status = Some(error);
                None
            }
        }
    }

    fn dashboard_save(&self) -> Result<serde_json::Value, String> {
        let id = self.dashboard_fields[0].trim();
        let name = self.dashboard_fields[1].trim();
        let kind = self.dashboard_fields[2].trim();
        let enabled = self.dashboard_fields[3]
            .parse::<bool>()
            .map_err(|_| "Enabled must be true or false")?;
        if id.is_empty() {
            return Err("ID is required".into());
        }
        if name.is_empty() {
            return Err("Target name is required".into());
        }
        if id.len() > 64
            || !id
                .bytes()
                .all(|c| c.is_ascii_alphanumeric() || b"_-".contains(&c))
        {
            return Err("ID must contain 1–64 letters, digits, underscores or dashes".into());
        }
        if self.dashboard_new
            && self
                .dashboard_targets
                .iter()
                .any(|target| target["id"] == id)
        {
            return Err("Target ID already exists".into());
        }
        if !matches!(kind, "local" | "slurm" | "ssh") {
            return Err("Backend must be local, slurm or ssh".into());
        }
        if !self.dashboard_new && !enabled && self.target_id.as_deref() == Some(id) {
            return Err("Select another target before disabling this one".into());
        }
        let mut settings = self.dashboard_draft["backend"]["settings"]
            .as_object()
            .cloned()
            .unwrap_or_default();
        for (index, field) in FIELD_DEFS.iter().enumerate() {
            let value = self.dashboard_fields[index + 4].trim();
            if value.is_empty() {
                if matches!(
                    field.key,
                    "account"
                        | "gpu_qos"
                        | "apptainer_manifest"
                        | "typed_worker_config"
                        | "data_ssh_host"
                        | "pi_ssh_host"
                ) {
                    settings.insert(field.key.into(), serde_json::Value::Null);
                } else {
                    settings.remove(field.key);
                }
                continue;
            }
            let value = match field.kind {
                FieldKind::Bool => serde_json::Value::Bool(
                    value
                        .parse()
                        .map_err(|_| format!("{} must be true or false", field.label))?,
                ),
                FieldKind::Number => serde_json::json!(
                    value
                        .parse::<u64>()
                        .map_err(|_| format!("{} must be a number", field.label))?
                ),
                FieldKind::Text => serde_json::json!(value),
            };
            settings.insert(field.key.into(), value);
        }
        if kind != "local"
            && settings
                .get("ssh_host")
                .and_then(serde_json::Value::as_str)
                .is_none_or(|v| v.trim().is_empty())
        {
            return Err("SSH host is required".into());
        }
        if kind == "slurm"
            && id != "legacy-slurm"
            && settings
                .get("typed_worker_config")
                .and_then(serde_json::Value::as_str)
                .is_none_or(|path| path.trim().is_empty())
        {
            return Err("Worker profile is required for Slurm".into());
        }
        let backend = if kind == "local" {
            serde_json::json!({"type":"local"})
        } else {
            serde_json::json!({"type":kind,"settings":settings})
        };
        let domains: serde_json::Value =
            serde_json::from_str(&self.dashboard_fields[4 + FIELD_DEFS.len()])
                .map_err(|_| "Data domains must be a JSON array")?;
        if !domains.is_array() {
            return Err("Data domains must be a JSON array".into());
        }
        Ok(
            serde_json::json!({"expected_revision":self.registry_revision,"id":id,"name":name,"enabled":enabled,"backend":backend,"data_domains":domains}),
        )
    }
    /// Fill selected target settings from a `GET /hpc/config` reply.
    pub fn load_config(&mut self, reply: &serde_json::Value) {
        self.target_id = reply
            .get("target_id")
            .and_then(serde_json::Value::as_str)
            .map(str::to_owned);
        self.target_revision = reply
            .get("target_revision")
            .and_then(serde_json::Value::as_u64)
            .unwrap_or(0);
        self.registry_revision = reply
            .get("registry_revision")
            .and_then(serde_json::Value::as_u64)
            .unwrap_or(0);
        self.target_editable = reply
            .get("editable")
            .and_then(serde_json::Value::as_bool)
            .unwrap_or(false);
        let config = reply.get("config").cloned().unwrap_or_default();
        self.values = FIELD_DEFS
            .iter()
            .map(|def| match config.get(def.key) {
                Some(serde_json::Value::Bool(b)) => {
                    if *b {
                        "true".into()
                    } else {
                        "false".into()
                    }
                }
                Some(serde_json::Value::Number(n)) => n.to_string(),
                Some(serde_json::Value::String(s)) => s.clone(),
                _ => String::new(),
            })
            .collect();
    }

    /// Assemble the selected-target settings update. Returns the
    /// field key on a malformed number instead of sending a bad profile.
    pub fn build_profile(&self) -> Result<serde_json::Value, &'static str> {
        let mut profile = serde_json::Map::new();
        for (def, value) in FIELD_DEFS.iter().zip(&self.values) {
            let value = value.trim();
            if value.is_empty() {
                if matches!(
                    def.key,
                    "account"
                        | "gpu_qos"
                        | "apptainer_manifest"
                        | "typed_worker_config"
                        | "data_ssh_host"
                        | "pi_ssh_host"
                ) {
                    profile.insert(def.key.into(), serde_json::Value::Null);
                }
                continue;
            }
            match def.kind {
                FieldKind::Bool => {
                    let parsed = match value {
                        "true" | "1" | "yes" | "on" => true,
                        "false" | "0" | "no" | "off" => false,
                        _ => {
                            return Err(def.key);
                        }
                    };
                    profile.insert(def.key.into(), serde_json::Value::Bool(parsed));
                }
                FieldKind::Number => {
                    let parsed: u64 = value.parse().map_err(|_| -> &'static str { def.key })?;
                    profile.insert(def.key.into(), serde_json::Value::Number(parsed.into()));
                }
                FieldKind::Text => {
                    profile.insert(def.key.into(), serde_json::Value::String(value.into()));
                }
            }
        }
        Ok(
            serde_json::json!({"target_id":self.target_id,"target_revision":self.target_revision,"expected_revision":self.registry_revision,"config":profile}),
        )
    }

    /// Fill job rows from a `GET /hpc/jobs` reply (newest first).
    pub fn load_jobs(&mut self, reply: &serde_json::Value) {
        let monitors = reply.get("monitors").and_then(serde_json::Value::as_array);
        let mut rows: Vec<HpcJobRow> = monitors
            .map(|list| {
                list.iter()
                    .map(|m| HpcJobRow {
                        id: m
                            .get("id")
                            .and_then(serde_json::Value::as_str)
                            .unwrap_or_default()
                            .into(),
                        job_id: m
                            .get("job_id")
                            .and_then(serde_json::Value::as_str)
                            .map(String::from),
                        state: m
                            .get("state")
                            .and_then(serde_json::Value::as_str)
                            .unwrap_or_default()
                            .into(),
                        scheduler_state: m
                            .get("scheduler_state")
                            .and_then(serde_json::Value::as_str)
                            .map(String::from),
                        agent_path: m
                            .get("agent_path")
                            .and_then(serde_json::Value::as_str)
                            .unwrap_or_default()
                            .into(),
                        cluster: m
                            .get("cluster")
                            .and_then(serde_json::Value::as_str)
                            .unwrap_or_default()
                            .into(),
                        last_error: m
                            .get("last_error")
                            .and_then(serde_json::Value::as_str)
                            .map(String::from),
                        updated_at: m
                            .get("updated_at")
                            .and_then(serde_json::Value::as_u64)
                            .unwrap_or(0),
                    })
                    .collect()
            })
            .unwrap_or_default();
        rows.sort_by(|a, b| b.updated_at.cmp(&a.updated_at));
        self.jobs = rows;
        self.selected_job = self.selected_job.min(self.jobs.len().saturating_sub(1));
    }
}

// ═══════════════════════════════════════════════════════════════════════
// Commands + key handling
// ═══════════════════════════════════════════════════════════════════════

/// What the panel wants the App layer to do after a key.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum HpcPanelCommand {
    Close,
    /// Persist the profile as assembled by [`HpcPanelState::build_profile`].
    SaveProfile(serde_json::Value),
    SelectTarget(String, u64),
    SaveTarget(serde_json::Value),
    RepairWorker(serde_json::Value),
    DeleteTarget(String, u64),
    /// Re-fetch the monitor inventory.
    /// Reload gateway registry and monitor records without probing the cluster.
    RefreshSnapshot,
}

impl HpcPanelState {
    /// Route a key. Returns `None` when the key was consumed internally.
    pub fn handle_key(&mut self, key: crossterm::event::KeyEvent) -> Option<HpcPanelCommand> {
        use crossterm::event::KeyCode;

        if self.dashboard_ready {
            return self.handle_dashboard_key(key);
        }

        // Inline editing captures everything until Enter/Esc.
        if let Some(buffer) = &mut self.edit_buffer {
            match key.code {
                KeyCode::Enter => {
                    let value = std::mem::take(buffer);
                    self.edit_buffer = None;
                    if let Some(slot) = self.values.get_mut(self.selected_field) {
                        *slot = value.trim().to_string();
                    }
                    return None;
                }
                KeyCode::Esc => {
                    self.edit_buffer = None;
                    return None;
                }
                KeyCode::Backspace => {
                    buffer.pop();
                }
                KeyCode::Char(ch) => buffer.push(ch),
                _ => {}
            }
            return None;
        }

        match key.code {
            KeyCode::Esc => Some(HpcPanelCommand::Close),
            KeyCode::Tab => {
                self.tab = match self.tab {
                    HpcTab::Config => HpcTab::Jobs,
                    HpcTab::Jobs => HpcTab::Targets,
                    HpcTab::Targets => HpcTab::Config,
                };
                None
            }
            KeyCode::Char('r') => Some(HpcPanelCommand::RefreshSnapshot),
            KeyCode::Up | KeyCode::Char('k') => {
                match self.tab {
                    HpcTab::Config => {
                        self.selected_field = self.selected_field.saturating_sub(1);
                    }
                    HpcTab::Jobs => {
                        self.selected_job = self.selected_job.saturating_sub(1);
                    }
                    HpcTab::Targets => {
                        self.selected_target = self.selected_target.saturating_sub(1);
                    }
                }
                None
            }
            KeyCode::Down | KeyCode::Char('j') => {
                match self.tab {
                    HpcTab::Config => {
                        if self.selected_field + 1 < FIELD_DEFS.len() {
                            self.selected_field += 1;
                        }
                    }
                    HpcTab::Jobs => {
                        if self.selected_job + 1 < self.jobs.len() {
                            self.selected_job += 1;
                        }
                    }
                    HpcTab::Targets => {
                        if self.selected_target + 1 < self.targets.len() {
                            self.selected_target += 1;
                        }
                    }
                }
                None
            }
            KeyCode::Enter => match self.tab {
                HpcTab::Config => {
                    if !self.target_editable {
                        self.status =
                            Some("Select a Slurm compute target to edit its settings.".into());
                        return None;
                    }
                    let def = &FIELD_DEFS[self.selected_field];
                    match def.kind {
                        FieldKind::Bool => {
                            if let Some(slot) = self.values.get_mut(self.selected_field) {
                                let next = match slot.as_str() {
                                    "true" => "false",
                                    _ => "true",
                                };
                                *slot = next.into();
                            }
                            None
                        }
                        _kind => {
                            let current = self
                                .values
                                .get(self.selected_field)
                                .cloned()
                                .unwrap_or_default();
                            self.edit_buffer = Some(current);
                            None
                        }
                    }
                }
                HpcTab::Targets => self.targets.get(self.selected_target).map(|target| {
                    HpcPanelCommand::SelectTarget(target.id.clone(), self.registry_revision)
                }),
                HpcTab::Jobs => None,
            },
            KeyCode::Char('s') if self.tab == HpcTab::Config && self.target_editable => {
                match self.build_profile() {
                    Ok(profile) => Some(HpcPanelCommand::SaveProfile(profile)),
                    Err(key) => {
                        self.status = Some(format!(
                            "invalid value for '{key}' — expected a number or boolean flag"
                        ));
                        None
                    }
                }
            }
            _ => None,
        }
    }

    fn handle_dashboard_key(&mut self, key: crossterm::event::KeyEvent) -> Option<HpcPanelCommand> {
        use crossterm::event::{KeyCode, KeyModifiers};
        let ctrl = key.modifiers.contains(KeyModifiers::CONTROL);
        if self.loading {
            return None;
        }
        if let Some(buffer) = &mut self.edit_buffer {
            if matches!(
                key.code,
                KeyCode::Char(_) | KeyCode::Backspace | KeyCode::Delete
            ) {
                self.status = None;
            }
            match key.code {
                KeyCode::Enter | KeyCode::Tab | KeyCode::BackTab | KeyCode::Char('s')
                    if key.code != KeyCode::Char('s') || ctrl =>
                {
                    let navigate = matches!(key.code, KeyCode::Tab | KeyCode::BackTab);
                    let backwards = key.code == KeyCode::BackTab;
                    let save = ctrl && key.code == KeyCode::Char('s');
                    self.dashboard_fields[self.dashboard_field] = std::mem::take(buffer);
                    self.edit_buffer = None;
                    if navigate {
                        self.dashboard_move_field(!backwards);
                    }
                    if save {
                        return self.dashboard_save_command();
                    }
                }
                KeyCode::Esc => self.edit_buffer = None,
                KeyCode::Left => self.edit_cursor = self.edit_cursor.saturating_sub(1),
                KeyCode::Right => {
                    self.edit_cursor = (self.edit_cursor + 1).min(buffer.chars().count())
                }
                KeyCode::Home => self.edit_cursor = 0,
                KeyCode::End => self.edit_cursor = buffer.chars().count(),
                KeyCode::Backspace => {
                    if self.edit_cursor > 0 {
                        let start = buffer
                            .char_indices()
                            .nth(self.edit_cursor - 1)
                            .map(|(i, _)| i)
                            .unwrap();
                        let end = buffer
                            .char_indices()
                            .nth(self.edit_cursor)
                            .map(|(i, _)| i)
                            .unwrap_or(buffer.len());
                        buffer.replace_range(start..end, "");
                        self.edit_cursor -= 1;
                    }
                }
                KeyCode::Delete => {
                    if let Some((start, ch)) = buffer.char_indices().nth(self.edit_cursor) {
                        buffer.replace_range(start..start + ch.len_utf8(), "");
                    }
                }
                KeyCode::Char('u') if ctrl => {
                    buffer.clear();
                    self.edit_cursor = 0;
                }
                KeyCode::Char(ch) if !ctrl => {
                    let offset = buffer
                        .char_indices()
                        .nth(self.edit_cursor)
                        .map(|(i, _)| i)
                        .unwrap_or(buffer.len());
                    buffer.insert(offset, ch);
                    self.edit_cursor += 1;
                }
                _ => {}
            }
            return None;
        }
        if self.dashboard_confirm_delete {
            self.dashboard_confirm_delete = false;
            if key.code == KeyCode::Char('y') || (ctrl && key.code == KeyCode::Char('x')) {
                return self
                    .dashboard_targets
                    .get(self.selected_target)
                    .and_then(|target| target["id"].as_str())
                    .map(|id| {
                        HpcPanelCommand::DeleteTarget(id.to_string(), self.registry_revision)
                    });
            }
            self.status = Some("Delete cancelled".into());
            return None;
        }
        match key.code {
            KeyCode::Esc if self.repair_show_events => {
                self.repair_show_events = false;
            }
            KeyCode::Esc if self.dashboard_editing => {
                self.dashboard_editing = false;
                self.dashboard_new = false;
                self.load_dashboard_draft();
            }
            KeyCode::Esc => return Some(HpcPanelCommand::Close),
            KeyCode::Tab | KeyCode::BackTab if self.dashboard_editing => {
                self.dashboard_move_field(key.code == KeyCode::Tab)
            }
            KeyCode::Tab | KeyCode::BackTab if !self.dashboard_editing => {
                self.repair_show_events = false;
                self.tab = if self.tab == HpcTab::Jobs {
                    HpcTab::Targets
                } else {
                    HpcTab::Jobs
                };
                self.dashboard_show_jobs = self.tab == HpcTab::Jobs;
                self.dashboard_focus_form = self.dashboard_show_jobs;
            }
            KeyCode::Left | KeyCode::Char('h')
                if self.tab == HpcTab::Targets && !self.dashboard_editing =>
            {
                self.dashboard_focus_form = false
            }
            KeyCode::Right | KeyCode::Char('l')
                if self.tab == HpcTab::Targets && !self.dashboard_editing =>
            {
                self.dashboard_focus_form = true
            }
            KeyCode::Char('r') if !self.dashboard_editing => {
                return Some(HpcPanelCommand::RefreshSnapshot);
            }
            KeyCode::Char('w') if !self.dashboard_editing && self.tab == HpcTab::Targets => {
                if self.dashboard_targets.get(self.selected_target).and_then(|t| t["backend"]["type"].as_str()) != Some("slurm") {
                    self.status = Some("Select a Slurm target to repair its worker".into());
                } else {
                    let id = self.dashboard_targets[self.selected_target]["id"].as_str().unwrap_or_default();
                    return Some(HpcPanelCommand::RepairWorker(serde_json::json!({"expected_revision": self.registry_revision, "target_id": id})));
                }
            }
            KeyCode::Char('J') if !self.dashboard_editing => {
                self.repair_show_events = false;
                self.dashboard_show_jobs = !self.dashboard_show_jobs;
                self.tab = if self.dashboard_show_jobs {
                    HpcTab::Jobs
                } else {
                    HpcTab::Targets
                };
                self.dashboard_focus_form = self.dashboard_show_jobs;
            }
            KeyCode::Char('n') if !self.dashboard_editing => {
                self.repair_show_events = false;
                self.status = None;
                self.dashboard_show_jobs = false;
                self.tab = HpcTab::Targets;
                self.dashboard_new = true;
                self.dashboard_editing = true;
                self.dashboard_focus_form = true;
                self.dashboard_draft = serde_json::json!({"id":"", "name":"", "enabled":true,"backend":{"type":"slurm","settings":{}},"data_domains":[]});
                self.dashboard_fields = vec!["".into(), "".into(), "slurm".into(), "true".into()];
                self.dashboard_fields.extend(FIELD_DEFS.iter().map(|f| {
                    if f.kind == FieldKind::Bool {
                        "false".into()
                    } else {
                        String::new()
                    }
                }));
                self.dashboard_fields.push("[]".into());
                self.dashboard_field = 0;
            }
            KeyCode::Char('e') if !self.dashboard_editing && !self.dashboard_targets.is_empty() => {
                self.repair_show_events = false;
                self.status = None;
                self.dashboard_show_jobs = false;
                self.tab = HpcTab::Targets;
                self.dashboard_editing = true;
                self.dashboard_focus_form = true;
                self.dashboard_field = 1;
            }
            KeyCode::Char('d') | KeyCode::Char('x')
                if !self.dashboard_editing
                    && !self.dashboard_targets.is_empty()
                    && (key.code == KeyCode::Char('d') || ctrl) =>
            {
                self.dashboard_confirm_delete = true;
                self.status = Some(format!(
                    "Delete {}? y or Ctrl+X confirms; any other key cancels",
                    self.dashboard_targets[self.selected_target]["id"]
                        .as_str()
                        .unwrap_or("?")
                ));
            }
            KeyCode::Char('x') if !self.dashboard_editing => {
                return self
                    .dashboard_targets
                    .get(self.selected_target)
                    .and_then(|target| target["id"].as_str())
                    .map(|id| {
                        HpcPanelCommand::SelectTarget(id.to_string(), self.registry_revision)
                    });
            }
            KeyCode::Char('s') if self.dashboard_editing => return self.dashboard_save_command(),
            KeyCode::Up | KeyCode::Char('k')
                if !self.dashboard_focus_form && !self.dashboard_editing =>
            {
                self.selected_target = self.selected_target.saturating_sub(1);
                self.load_dashboard_draft();
            }
            KeyCode::Down | KeyCode::Char('j')
                if !self.dashboard_focus_form && !self.dashboard_editing =>
            {
                if self.selected_target + 1 < self.dashboard_targets.len() {
                    self.selected_target += 1;
                }
                self.load_dashboard_draft();
            }
            KeyCode::Up | KeyCode::Char('k') if self.dashboard_editing => {
                self.dashboard_move_field(false)
            }
            KeyCode::Down | KeyCode::Char('j') if self.dashboard_editing => {
                self.dashboard_move_field(true)
            }
            KeyCode::Up | KeyCode::Char('k') if self.dashboard_show_jobs => {
                self.selected_job = self.selected_job.saturating_sub(1);
            }
            KeyCode::Down | KeyCode::Char('j') if self.dashboard_show_jobs => {
                self.selected_job = (self.selected_job + 1).min(self.jobs.len().saturating_sub(1));
            }
            KeyCode::Up | KeyCode::Char('k') if self.dashboard_focus_form => {
                self.dashboard_move_field(false)
            }
            KeyCode::Down | KeyCode::Char('j') if self.dashboard_focus_form => {
                self.dashboard_move_field(true)
            }
            KeyCode::Enter if self.dashboard_editing => {
                if (!self.dashboard_new && self.dashboard_field == 0)
                    || (!self.dashboard_new && self.dashboard_field == 2)
                {
                    self.status = Some("ID and backend are fixed for existing targets".into());
                } else if self.dashboard_new && self.dashboard_field == 2 {
                    self.dashboard_fields[2] = match self.dashboard_fields[2].as_str() {
                        "slurm" => "ssh",
                        "ssh" => "local",
                        _ => "slurm",
                    }
                    .into();
                } else if self.dashboard_field == 3
                    || (self.dashboard_field >= 4
                        && self.dashboard_field < 4 + FIELD_DEFS.len()
                        && FIELD_DEFS[self.dashboard_field - 4].kind == FieldKind::Bool)
                {
                    self.dashboard_fields[self.dashboard_field] =
                        (self.dashboard_fields[self.dashboard_field] != "true").to_string();
                } else {
                    self.edit_buffer = Some(self.dashboard_fields[self.dashboard_field].clone());
                    self.edit_cursor = self.dashboard_fields[self.dashboard_field].chars().count();
                }
            }
            _ => {}
        }
        None
    }
}

// ═══════════════════════════════════════════════════════════════════════
// Widget
// ═══════════════════════════════════════════════════════════════════════

/// Age string like `42s`, `7m`, `3h`, `2d` from epoch milliseconds.
fn age(updated_at_ms: u64) -> String {
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0);
    let delta = now.saturating_sub(updated_at_ms) / 1000;
    match delta {
        0..=59 => format!("{delta}s"),
        60..=3599 => format!("{}m", delta / 60),
        3600..=86399 => format!("{}h", delta / 3600),
        _ => format!("{}d", delta / 86400),
    }
}

pub struct HpcPanelWidget;

impl StatefulWidgetRef for HpcPanelWidget {
    type State = HpcPanelState;

    fn render_ref(&self, area: Rect, buf: &mut Buffer, state: &mut Self::State) {
        if state.dashboard_ready {
            self.render_dashboard(area, buf, state);
            return;
        }
        let theme = theme::current();
        let mut lines: Vec<Line> = Vec::new();

        // Tab row.
        let tab = |label: &str, active: bool| {
            let style = if active {
                Style::default()
                    .fg(theme.accent_alt)
                    .add_modifier(Modifier::BOLD | Modifier::REVERSED)
            } else {
                Style::default().fg(theme.text_faint)
            };
            Span::styled(format!(" {label} "), style)
        };
        lines.push(Line::from(vec![
            tab("Compute settings", state.tab == HpcTab::Config),
            tab("Jobs", state.tab == HpcTab::Jobs),
            tab("Targets", state.tab == HpcTab::Targets),
        ]));
        lines.push(Line::from(Span::styled(
            "─".repeat(usize::from(area.width)),
            Style::default().fg(theme.text_faint),
        )));

        if state.loading {
            lines.push(Line::from(Span::styled(
                "Loading…",
                Style::default().fg(theme.text_faint),
            )));
        } else {
            match state.tab {
                HpcTab::Config => self.render_config(&mut lines, state, &theme),
                HpcTab::Jobs => self.render_jobs(&mut lines, state, &theme),
                HpcTab::Targets => self.render_targets(&mut lines, state, &theme),
            }
        }

        let mut footer: Vec<Line<'static>> = Vec::new();
        if !state.loading && state.tab == HpcTab::Config && state.target_editable {
            footer.push(Line::from(Span::styled(
                "s save target",
                Style::default().fg(theme.text_faint),
            )));
        }
        if let Some(status) = &state.status {
            footer.push(Line::from(Span::styled(
                status.clone(),
                Style::default().fg(theme.text_faint),
            )));
        }
        // Keymap hint — the panel's persistent footer.
        footer.push(Line::from(Span::styled(
            if state.tab == HpcTab::Targets {
                "Enter select target · Tab switch · r reload gateway snapshot · Esc close"
            } else {
                "Tab switch · r reload gateway snapshot · Esc close"
            },
            Style::default().fg(theme.text_faint),
        )));

        let width = usize::from(area.width.max(1));
        let footer_height = footer
            .iter()
            .map(|line| line.width().max(1).div_ceil(width))
            .sum::<usize>()
            .min(usize::from(area.height)) as u16;
        let content = Rect {
            height: area.height.saturating_sub(footer_height),
            ..area
        };
        let footer_area = Rect {
            y: area.y + content.height,
            height: footer_height,
            ..area
        };
        Paragraph::new(lines)
            .block(Block::default().borders(Borders::NONE))
            .wrap(ratatui::widgets::Wrap { trim: false })
            .render(content, buf);
        Paragraph::new(footer)
            .wrap(ratatui::widgets::Wrap { trim: false })
            .render(footer_area, buf);
    }
}

impl HpcPanelWidget {
    fn render_dashboard(&self, area: Rect, buf: &mut Buffer, state: &HpcPanelState) {
        let theme = theme::current();
        let hint = if state.dashboard_editing {
            "Tab/↑↓ field · Enter edit/toggle · Ctrl+S save · Esc cancel"
        } else if state.tab == HpcTab::Jobs {
            "Tab switch · ↑↓ jobs · r reload gateway snapshot · Esc close"
        } else {
            "Tab switch · ↑↓ move · n new · e edit · w repair worker · x select · Ctrl+X delete · r reload snapshot · Esc close"
        };
        let mut footer = Vec::new();
        if let Some(status) = &state.status {
            footer.push(Line::from(status.clone()));
        } else if state.dashboard_editing {
            footer.push(Line::from(state.dashboard_field_help()));
        }
        footer.push(Line::from(hint));
        let width = usize::from(area.width.max(1));
        let footer_height = footer
            .iter()
            .map(|line| line.width().max(1).div_ceil(width))
            .sum::<usize>()
            .min(usize::from(area.height)) as u16;
        let rows = Layout::vertical([
            Constraint::Length(2),
            Constraint::Min(0),
            Constraint::Length(footer_height),
        ])
        .split(area);
        let tab = |label: &str, active: bool| {
            Span::styled(
                format!(" {label} "),
                if active {
                    Style::default()
                        .fg(theme.accent_alt)
                        .add_modifier(Modifier::BOLD | Modifier::REVERSED)
                } else {
                    Style::default().fg(theme.text_faint)
                },
            )
        };
        Paragraph::new(vec![
            Line::from(vec![
                tab("Targets", state.tab == HpcTab::Targets),
                tab("Jobs", state.tab == HpcTab::Jobs),
            ]),
            Line::from(Span::styled(
                "─".repeat(usize::from(rows[0].width)),
                Style::default().fg(theme.text_faint),
            )),
        ])
        .render(rows[0], buf);
        let empty = Rect {
            width: 0,
            height: 0,
            ..rows[1]
        };
        let columns = if state.tab == HpcTab::Targets {
            let split =
                Layout::horizontal([Constraint::Percentage(35), Constraint::Percentage(65)])
                    .split(rows[1]);
            [split[0], split[1]]
        } else {
            [empty, rows[1]]
        };
        let left = Block::default()
            .title(" Compute targets ")
            .borders(Borders::ALL)
            .border_style(Style::default().fg(if !state.dashboard_focus_form {
                theme.accent_alt
            } else {
                theme.text_faint
            }));
        let list: Vec<Line> = state
            .dashboard_targets
            .iter()
            .enumerate()
            .map(|(i, target)| {
                let id = target["id"].as_str().unwrap_or("?");
                let name = target["name"].as_str().unwrap_or(id);
                let active = state.target_id.as_deref() == Some(id);
                Line::from(Span::styled(
                    format!(
                        "{}{} ({}){}",
                        if i == state.selected_target {
                            "> "
                        } else {
                            "  "
                        },
                        name,
                        id,
                        if active && target["revision"].as_u64() != Some(state.target_revision) {
                            " (active: older revision)"
                        } else if active {
                            " *"
                        } else if target["enabled"] == false {
                            " (disabled)"
                        } else {
                            ""
                        }
                    ),
                    Style::default().fg(if i == state.selected_target {
                        theme.accent_alt
                    } else {
                        theme.text
                    }),
                ))
            })
            .collect();
        let list_area = left.inner(columns[0]);
        left.render(columns[0], buf);
        let list_scroll = state
            .selected_target
            .saturating_sub(usize::from(list_area.height.saturating_sub(1)))
            as u16;
        Paragraph::new(list)
            .scroll((list_scroll, 0))
            .render(list_area, buf);

        let right = Block::default()
            .title(if state.repair_show_events {
                " Connection recovery events "
            } else if state.dashboard_show_jobs {
                " Jobs "
            } else if state.dashboard_new {
                " New target "
            } else if state.dashboard_editing {
                " Edit target "
            } else {
                " Target details "
            })
            .borders(Borders::ALL)
            .border_style(Style::default().fg(if state.dashboard_focus_form {
                theme.accent_alt
            } else {
                theme.text_faint
            }));
        let form_area = right.inner(columns[1]);
        right.render(columns[1], buf);
        let mut form: Vec<Line> = Vec::new();
        if state.repair_show_events {
            for event in &state.repair_events {
                // Escape controls so remote output cannot affect terminal rendering.
                let clean: String = event.chars().flat_map(|ch| ch.escape_debug()).collect();
                let mut line = String::new();
                let mut width = 0;
                for ch in clean.chars() {
                    let size = unicode_width::UnicodeWidthChar::width(ch).unwrap_or(0);
                    if width + size > usize::from(form_area.width.max(1)) && !line.is_empty() {
                        form.push(Line::from(std::mem::take(&mut line)));
                        width = 0;
                    }
                    line.push(ch);
                    width += size;
                }
                form.push(Line::from(line));
            }
        } else if state.dashboard_show_jobs {
            self.render_jobs(&mut form, state, &theme);
        } else if state.dashboard_fields.is_empty() {
            form.push(Line::from("Press n to add a target"));
        } else {
            let labels = ["ID", "Name", "Backend (local/slurm/ssh)", "Enabled"];
            for i in state.dashboard_visible_fields() {
                let value = &state.dashboard_fields[i];
                let label = if i < 4 {
                    labels[i]
                } else if i == 4 + FIELD_DEFS.len() {
                    "Data domains (JSON)"
                } else {
                    FIELD_DEFS[i - 4].label
                };
                let display = if state.dashboard_editing && i == state.dashboard_field && state.edit_buffer.is_some() {
                    let buffer = state.edit_buffer.as_deref().unwrap_or("");
                    let offset = buffer
                        .char_indices()
                        .nth(state.edit_cursor)
                        .map(|(index, _)| index)
                        .unwrap_or(buffer.len());
                    let mut marked = format!("{}▏{}", &buffer[..offset], &buffer[offset..]);
                    let available = usize::from(form_area.width)
                        .saturating_sub(label.chars().count() + 4)
                        .max(1);
                    let cursor = state.edit_cursor;
                    if marked.chars().count() > available && cursor >= available {
                        let start = cursor.saturating_sub(available.saturating_sub(2));
                        marked = format!("…{}", marked.chars().skip(start).collect::<String>());
                    }
                    marked
                } else if value.is_empty() {
                    "(default)".into()
                } else {
                    value.clone()
                };
                form.push(Line::from(Span::styled(
                    format!(
                        "{}{}: {}",
                        if state.dashboard_focus_form && i == state.dashboard_field {
                            "> "
                        } else {
                            "  "
                        },
                        label,
                        display
                    ),
                    Style::default().fg(
                        if state.dashboard_focus_form && i == state.dashboard_field {
                            theme.accent_alt
                        } else {
                            theme.text
                        },
                    ),
                )));
            }
        }
        let scroll = if state.repair_show_events {
            form.len().saturating_sub(usize::from(form_area.height)).min(u16::MAX as usize) as u16
        } else if state.dashboard_show_jobs {
            state
                .selected_job
                .saturating_sub(usize::from(form_area.height.saturating_sub(1))) as u16
        } else if state.dashboard_focus_form {
            state
                .dashboard_visible_fields()
                .iter()
                .position(|field| *field == state.dashboard_field)
                .unwrap_or(0)
                .saturating_sub(usize::from(form_area.height.saturating_sub(1))) as u16
        } else {
            0
        };
        Paragraph::new(form)
            .scroll((scroll, 0))
            .render(form_area, buf);
        Paragraph::new(footer)
            .style(Style::default().fg(theme.text_faint))
            .wrap(ratatui::widgets::Wrap { trim: false })
            .render(rows[2], buf);

    }
    fn render_config(
        &self,
        lines: &mut Vec<Line<'static>>,
        state: &HpcPanelState,
        theme: &crate::theme::Theme,
    ) {
        if !state.target_editable {
            lines.push(Line::from(
                "Select a Slurm compute target to edit its settings.",
            ));
            return;
        }
        lines.push(Line::from(format!(
            "Target: {} r{}",
            state.target_id.as_deref().unwrap_or("?"),
            state.target_revision
        )));
        for (index, def) in FIELD_DEFS.iter().enumerate() {
            let value = state
                .values
                .get(index)
                .map(String::as_str)
                .unwrap_or_default();
            let display = if def.kind == FieldKind::Bool {
                value.to_string()
            } else if value.is_empty() {
                "(default)".to_string()
            } else {
                value.to_string()
            };
            let selected = index == state.selected_field;
            let marker = if selected { "> " } else { "  " };
            let style = if selected {
                Style::default().fg(theme.accent_alt)
            } else {
                Style::default().fg(theme.text)
            };
            lines.push(Line::from(vec![
                Span::styled(format!("{marker}{}", def.label), style),
                Span::styled(
                    format!("  {display}"),
                    if selected {
                        Style::default().fg(theme.text)
                    } else {
                        Style::default().fg(theme.text_faint)
                    },
                ),
            ]));
            // Inline edit row under the selected field.
            if selected {
                if let Some(buffer) = &state.edit_buffer {
                    lines.push(Line::from(Span::styled(
                        format!("    edit: {buffer}_  (Enter commit · Esc cancel)"),
                        Style::default().fg(theme.accent),
                    )));
                }
            }
        }
    }

    fn render_jobs(
        &self,
        lines: &mut Vec<Line<'static>>,
        state: &HpcPanelState,
        theme: &crate::theme::Theme,
    ) {
        if state.jobs.is_empty() {
            lines.push(Line::from(Span::styled(
                "No monitored HPC jobs. Submissions from agents appear here.",
                Style::default().fg(theme.text_faint),
            )));
            return;
        }
        for (index, job) in state.jobs.iter().enumerate() {
            let selected = index == state.selected_job;
            let marker = if selected { "> " } else { "  " };
            let attention = job.state == "submission_uncertain" || job.last_error.is_some();
            let style = if attention {
                Style::default().fg(theme.warning)
            } else if selected {
                Style::default().fg(theme.accent_alt)
            } else {
                Style::default().fg(theme.text)
            };
            let job_label = job.job_id.clone().unwrap_or_else(|| "—".into());
            let sched = job.scheduler_state.clone().unwrap_or_default();
            lines.push(Line::from(Span::styled(
                format!(
                    "{marker}#{} [{}] {} · {} · {} · {} ago",
                    job_label,
                    job.state,
                    sched,
                    job.agent_path,
                    job.cluster,
                    age(job.updated_at)
                ),
                style,
            )));
            if selected {
                if let Some(error) = &job.last_error {
                    lines.push(Line::from(Span::styled(
                        format!("    ! {error}"),
                        Style::default().fg(theme.error),
                    )));
                }
            }
        }
    }

    fn render_targets(
        &self,
        lines: &mut Vec<Line<'static>>,
        state: &HpcPanelState,
        theme: &crate::theme::Theme,
    ) {
        if state.targets.is_empty() {
            lines.push(Line::from(Span::styled(
                "No enrolled compute targets.",
                Style::default().fg(theme.text_faint),
            )));
            return;
        }
        for (index, target) in state.targets.iter().enumerate() {
            let selected_row = index == state.selected_target;
            let marker = if selected_row { "> " } else { "  " };
            let header_style = if target.selected {
                Style::default().fg(theme.accent)
            } else if selected_row {
                Style::default().fg(theme.accent_alt)
            } else {
                Style::default().fg(theme.text)
            };
            let state_tag = if target.selected {
                " · SELECTED"
            } else if !target.enabled {
                " · disabled"
            } else {
                ""
            };
            lines.push(Line::from(Span::styled(
                format!(
                    "{marker}{} r{} [{}]{}",
                    target.id, target.revision, target.backend, state_tag
                ),
                header_style,
            )));
            if selected_row {
                if target.endpoints.is_empty() {
                    lines.push(Line::from(Span::styled(
                        "    endpoints: (local, none frozen)".to_string(),
                        Style::default().fg(theme.text_faint),
                    )));
                }
                for (alias, hostname, port) in &target.endpoints {
                    lines.push(Line::from(Span::styled(
                        format!("    {alias:<10} → {hostname}:{port}  [frozen]"),
                        Style::default().fg(theme.text),
                    )));
                }
                for domain in &target.domains {
                    lines.push(Line::from(Span::styled(
                        format!("    domain     {domain}"),
                        Style::default().fg(theme.text),
                    )));
                }
            }
        }
        if !state.mounts.is_empty() {
            lines.push(Line::from(""));
            lines.push(Line::from(Span::styled(
                "VFS mount partitions:".to_string(),
                Style::default().fg(theme.text_faint),
            )));
            for mount in &state.mounts {
                let domain = mount.domain.as_deref().unwrap_or("—");
                lines.push(Line::from(Span::styled(
                    format!(
                        "  {:<18} {:<8} {} {}",
                        mount.path,
                        if mount.read_only { "ro" } else { "rw" },
                        mount.backend,
                        domain
                    ),
                    Style::default().fg(theme.text_faint),
                )));
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn worker_repair_starts_without_configuration() {
        use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
        let mut panel = HpcPanelState::default();
        panel.load_dashboard(&serde_json::json!({"state":{"revision":7,"policy":{"target_id":"Csu"},"targets":[{"id":"Csu","name":"Csu","revision":1,"enabled":true,"backend":{"type":"slurm","settings":{}}}]}}));
        match panel.handle_key(KeyEvent::new(KeyCode::Char('w'), KeyModifiers::NONE)) {
            Some(HpcPanelCommand::RepairWorker(request)) => {
                assert_eq!(request, serde_json::json!({"target_id":"Csu", "expected_revision":7}));
                assert!(!panel.dashboard_editing);
                assert!(panel.edit_buffer.is_none());
            }
            _ => panic!("expected immediate worker repair request"),
        }
    }

    #[test]
    fn repair_events_render_in_dashboard() {
        let mut panel = HpcPanelState::default();
        panel.dashboard_ready = true;
        panel.tab = HpcTab::Targets;
        panel.repair_show_events = true;
        panel.push_repair_event("[authenticate] stdout: connected".into());
        panel.push_repair_event("[upload] 16/32 MiB".into());
        let area = Rect::new(0, 0, 120, 30);
        let mut buffer = Buffer::empty(area);
        HpcPanelWidget.render_ref(area, &mut buffer, &mut panel);
        let rendered = (0..area.height).map(|y| (0..area.width).map(|x| buffer[(x, y)].symbol()).collect::<String>()).collect::<Vec<_>>().join("\n");
        assert!(rendered.contains("Connection recovery events"));
        assert!(rendered.contains("stdout: connected"));
        assert!(rendered.contains("16/32 MiB"));
    }

    #[test]
    fn dashboard_edits_unicode_at_cursor_and_tabs_between_fields() {
        use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
        let mut panel = HpcPanelState::default();
        panel.load_dashboard(&serde_json::json!({"state":{"revision":1,"policy":{"target_id":"local"},"targets":[{"id":"local","name":"中文","revision":1,"enabled":true,"backend":{"type":"local"}}]}}));
        let key = |code| KeyEvent::new(code, KeyModifiers::NONE);
        panel.handle_key(key(KeyCode::Char('e')));
        panel.handle_key(key(KeyCode::Enter));
        panel.handle_key(key(KeyCode::Left));
        panel.handle_key(key(KeyCode::Char('好')));
        panel.paste_into_edit("X");
        panel.handle_key(key(KeyCode::Backspace));
        panel.handle_key(key(KeyCode::Tab));
        assert_eq!(panel.dashboard_fields[1], "中好文");
        assert_eq!(panel.dashboard_field, 2);
    }

    #[test]
    fn dashboard_ctrl_save_commits_the_active_editor() {
        use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
        let mut panel = HpcPanelState::default();
        panel.load_dashboard(&serde_json::json!({"state":{"revision":1,"policy":{"target_id":"local"},"targets":[{"id":"local","name":"Local","revision":1,"enabled":true,"backend":{"type":"local"}}]}}));
        let key = |code| KeyEvent::new(code, KeyModifiers::NONE);
        panel.handle_key(key(KeyCode::Char('e')));
        panel.handle_key(key(KeyCode::Enter));
        panel.handle_key(key(KeyCode::Char('s')));
        let command = panel.handle_key(KeyEvent::new(KeyCode::Char('s'), KeyModifiers::CONTROL));
        match command {
            Some(HpcPanelCommand::SaveTarget(request)) => assert_eq!(request["name"], "Locals"),
            other => panic!("expected committed save, got {other:?}"),
        }
    }

    #[test]
    fn dashboard_backend_selection_filters_ssh_and_validates_required_fields() {
        use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
        let mut panel = HpcPanelState::default();
        panel.load_dashboard(&serde_json::json!({"state":{"revision":1,"policy":{"target_id":"local"},"targets":[{"id":"local","name":"Local","revision":1,"enabled":true,"backend":{"type":"local"}}]}}));
        let key = |code| KeyEvent::new(code, KeyModifiers::NONE);
        panel.handle_key(key(KeyCode::Char('n')));
        panel.dashboard_field = 2;
        panel.handle_key(key(KeyCode::Enter));
        assert_eq!(panel.dashboard_fields[2], "ssh");
        let partition = 4 + FIELD_DEFS
            .iter()
            .position(|f| f.key == "partition")
            .unwrap();
        assert!(!panel.dashboard_visible_fields().contains(&partition));
        panel.dashboard_fields[0] = "remote".into();
        panel.handle_key(key(KeyCode::Char('s')));
        assert_eq!(panel.dashboard_field, 1);
        assert!(panel.status.as_deref().unwrap().contains("name"));
    }

    #[test]
    fn dashboard_edits_highlighted_target_without_changing_selection() {
        let mut panel = HpcPanelState::default();
        panel.load_dashboard(&serde_json::json!({"state":{"revision":7,"policy":{"target_id":"local"},"targets":[{"id":"local","name":"Local","revision":1,"enabled":true,"backend":{"type":"local"}}, {"id":"remote","name":"Remote","revision":1,"enabled":true,"backend":{"type":"ssh","settings":{"ssh_host":"server"}},"data_domains":[]}, {"id":"remote","name":"Remote 2","revision":2,"enabled":true,"backend":{"type":"ssh","settings":{"ssh_host":"server"}},"data_domains":[]}]}}));
        assert_eq!(panel.dashboard_targets.len(), 2);
        panel.selected_target = 1;
        panel.load_dashboard_draft();
        panel.dashboard_fields[1] = "Changed".into();
        let request = panel.dashboard_save().unwrap();
        assert_eq!(request["id"], "remote");
        assert_eq!(request["name"], "Changed");
        assert_eq!(request["expected_revision"], 7);
        assert_eq!(panel.target_id.as_deref(), Some("local"));
    }

    #[test]
    fn dashboard_hides_deleted_and_confirms_delete() {
        let mut panel = HpcPanelState::default();
        panel.load_dashboard(&serde_json::json!({"state":{"revision":4,"policy":{"target_id":"local"},"deleted_target_ids":["deleted"],"targets":[{"id":"local","name":"Local","revision":1,"enabled":true,"backend":{"type":"local"}},{"id":"deleted","name":"Deleted","revision":1,"enabled":true,"backend":{"type":"local"}}]}}));
        assert_eq!(panel.dashboard_targets.len(), 1);
        let key = |c| {
            crossterm::event::KeyEvent::new(
                crossterm::event::KeyCode::Char(c),
                crossterm::event::KeyModifiers::empty(),
            )
        };
        assert_eq!(panel.handle_key(key('d')), None);
        assert_eq!(panel.handle_key(key('n')), None);
        assert!(!panel.dashboard_confirm_delete);
        panel.handle_key(key('d'));
        assert_eq!(
            panel.handle_key(key('y')),
            Some(HpcPanelCommand::DeleteTarget("local".into(), 4))
        );
    }

    #[test]
    fn dashboard_renders_tabs_and_bottom_hints() {
        let mut panel = HpcPanelState::default();
        panel.load_dashboard(&serde_json::json!({"state":{"revision":1,"policy":{"target_id":"local"},"targets":[{"id":"local","name":"Local","revision":1,"enabled":true,"backend":{"type":"local"}}]}}));
        let area = Rect::new(0, 0, 80, 20);
        let mut buffer = Buffer::empty(area);
        HpcPanelWidget.render_ref(area, &mut buffer, &mut panel);
        let row = |y| {
            (0..80)
                .map(|x| buffer[(x, y)].symbol().to_owned())
                .collect::<String>()
        };
        assert!(!row(0).contains("Compute settings"));
        assert!(row(0).contains("Jobs"));
        assert!(row(0).contains("Targets"));
        assert!(row(1).contains('─'));
        assert!(row(2).contains("Compute targets"));
        assert!(row(2).contains("Target details"));
        assert!(row(3).contains("local"));
        assert!(row(19).contains("Esc close"));
    }

    #[test]
    fn dashboard_tabs_navigate_and_edit_target_in_place() {
        use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
        let mut panel = HpcPanelState::default();
        panel.load_dashboard(&serde_json::json!({"state":{"revision":1,"policy":{"target_id":"local"},"targets":[{"id":"local","name":"Local","revision":1,"enabled":true,"backend":{"type":"local"}}]}}));
        let key = |code| KeyEvent::new(code, KeyModifiers::NONE);
        assert_eq!(panel.tab, HpcTab::Targets);
        panel.handle_key(key(KeyCode::Right));
        assert!(panel.dashboard_focus_form);
        panel.handle_key(key(KeyCode::Left));
        assert!(!panel.dashboard_focus_form);
        panel.handle_key(key(KeyCode::Tab));
        assert_eq!(panel.tab, HpcTab::Jobs);
        panel.handle_key(key(KeyCode::Tab));
        assert_eq!(panel.tab, HpcTab::Targets);
        panel.handle_key(key(KeyCode::Char('e')));
        assert_eq!(panel.tab, HpcTab::Targets);
        assert!(panel.dashboard_editing);
        panel.handle_key(key(KeyCode::Tab));
        assert_eq!(panel.tab, HpcTab::Targets);
    }

    #[test]
    fn local_form_skips_inapplicable_remote_settings() {
        let mut panel = HpcPanelState::default();
        panel.load_dashboard(&serde_json::json!({"state":{"revision":1,"policy":{"target_id":"local"},"targets":[{"id":"local","name":"Local","revision":1,"enabled":true,"backend":{"type":"local"}}]}}));
        panel.dashboard_field = 3;
        panel.dashboard_move_field(true);
        assert_eq!(panel.dashboard_field, 4 + FIELD_DEFS.len());
        panel.dashboard_move_field(false);
        assert_eq!(panel.dashboard_field, 3);
    }

    #[test]
    fn dashboard_creates_local_target_from_form() {
        let mut panel = HpcPanelState::default();
        panel.load_dashboard(&serde_json::json!({"state":{"revision":2,"policy":{"target_id":"local"},"targets":[{"id":"local","name":"Local","revision":1,"enabled":true,"backend":{"type":"local"}}]}}));
        let key = |c| {
            crossterm::event::KeyEvent::new(
                crossterm::event::KeyCode::Char(c),
                crossterm::event::KeyModifiers::empty(),
            )
        };
        panel.handle_key(key('n'));
        panel.dashboard_fields[0] = "new-host".into();
        panel.dashboard_fields[1] = "New host".into();
        panel.dashboard_fields[2] = "local".into();
        match panel.handle_key(key('s')) {
            Some(HpcPanelCommand::SaveTarget(request)) => {
                assert_eq!(request["id"], "new-host");
                assert_eq!(request["backend"]["type"], "local");
                assert_eq!(request["expected_revision"], 2);
            }
            other => panic!("expected target save, got {other:?}"),
        }
    }

    fn state_with_values() -> HpcPanelState {
        let mut state = HpcPanelState::default();
        state.values = FIELD_DEFS.iter().map(|_| String::new()).collect();
        state.target_editable = true;
        state.target_id = Some("test".into());
        state.target_revision = 1;
        state.registry_revision = 1;
        state
    }

    fn set_value(state: &mut HpcPanelState, key: &str, value: &str) {
        let index = FIELD_DEFS.iter().position(|d| d.key == key).unwrap();
        state.values[index] = value.into();
    }

    #[test]
    fn load_config_reads_json_fields() {
        let reply = serde_json::json!({
            "config": {
                "tools_enabled": true,
                "ssh_host": "sjtu",
                "poll_interval_secs": 15,
                "account": null
            },
            "target_id": "sjtu", "target_revision": 2, "registry_revision": 3, "editable": true
        });
        let mut state = state_with_values();
        state.load_config(&reply);
        assert_eq!(state.target_id.as_deref(), Some("sjtu"));
        let enabled = FIELD_DEFS
            .iter()
            .position(|d| d.key == "tools_enabled")
            .unwrap();
        assert_eq!(state.values[enabled], "true");
        let host = FIELD_DEFS.iter().position(|d| d.key == "ssh_host").unwrap();
        assert_eq!(state.values[host], "sjtu");
    }

    #[test]
    fn build_profile_skips_empty_and_rejects_bad_numbers() {
        let mut state = state_with_values();
        set_value(&mut state, "tools_enabled", "true");
        set_value(&mut state, "ssh_host", "sjtu");
        set_value(&mut state, "account", "");
        set_value(&mut state, "poll_interval_secs", "15");
        let profile = state.build_profile().unwrap();
        assert_eq!(profile["config"]["tools_enabled"], serde_json::json!(true));
        assert_eq!(profile["config"]["ssh_host"], serde_json::json!("sjtu"));
        assert_eq!(
            profile["config"]["poll_interval_secs"],
            serde_json::json!(15)
        );
        assert!(profile["config"]["account"].is_null());

        set_value(&mut state, "gpu_cpus_per_gpu", "sixteen");
        assert_eq!(state.build_profile().err(), Some("gpu_cpus_per_gpu"));
    }

    #[test]
    fn bool_toggle_and_text_edit_via_enter() {
        let mut state = state_with_values();
        set_value(&mut state, "tools_enabled", "false");
        state.selected_field = FIELD_DEFS
            .iter()
            .position(|d| d.key == "tools_enabled")
            .unwrap();
        // Enter on a bool toggles in place.
        let _ = state.handle_key(crossterm::event::KeyEvent::new(
            crossterm::event::KeyCode::Enter,
            crossterm::event::KeyModifiers::empty(),
        ));
        assert_eq!(state.values[state.selected_field], "true");

        // Enter on a text field opens the edit buffer.
        state.selected_field = FIELD_DEFS.iter().position(|d| d.key == "ssh_host").unwrap();
        let _ = state.handle_key(crossterm::event::KeyEvent::new(
            crossterm::event::KeyCode::Enter,
            crossterm::event::KeyModifiers::empty(),
        ));
        assert!(state.edit_buffer.is_some());
        for ch in "hpc-login".chars() {
            let _ = state.handle_key(crossterm::event::KeyEvent::new(
                crossterm::event::KeyCode::Char(ch),
                crossterm::event::KeyModifiers::empty(),
            ));
        }
        let _ = state.handle_key(crossterm::event::KeyEvent::new(
            crossterm::event::KeyCode::Enter,
            crossterm::event::KeyModifiers::empty(),
        ));
        assert_eq!(state.values[state.selected_field], "hpc-login");
    }

    #[test]
    fn save_command_carries_profile() {
        let mut state = state_with_values();
        set_value(&mut state, "tools_enabled", "true");
        let cmd = state.handle_key(crossterm::event::KeyEvent::new(
            crossterm::event::KeyCode::Char('s'),
            crossterm::event::KeyModifiers::empty(),
        ));
        match cmd {
            Some(HpcPanelCommand::SaveProfile(profile)) => {
                assert_eq!(profile["config"]["tools_enabled"], serde_json::json!(true));
            }
            other => panic!("expected SaveProfile, got {other:?}"),
        }
    }

    #[test]
    fn load_jobs_sorts_newest_first() {
        let reply = serde_json::json!({
            "monitors": [
                {"id": "old", "state": "terminal", "agent_path": "/root/a", "updated_at": 100},
                {"id": "new", "state": "watching", "job_id": "4201", "scheduler_state": "RUNNING",
                 "agent_path": "/root/b", "cluster": "sjtu", "updated_at": 200, "last_error": "boom"}
            ]
        });
        let mut state = HpcPanelState::default();
        state.load_jobs(&reply);
        assert_eq!(state.jobs.len(), 2);
        assert_eq!(state.jobs[0].id, "new");
        assert_eq!(state.jobs[0].job_id.as_deref(), Some("4201"));
        // The fixture puts `last_error` on the *newest* job, which sorts to
        // index 0; the old assertion read index 1 (the error-free job) and
        // could never pass.
        assert_eq!(state.jobs[0].last_error.as_deref(), Some("boom"));
        assert_eq!(state.jobs[1].last_error.as_deref(), None);
    }
}
