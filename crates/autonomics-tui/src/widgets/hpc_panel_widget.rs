//! HPC site panel — operator view/edit of the Slurm site profile plus the
//! durable job monitor inventory. Thin-client: state mirrors the gateway's
//! `GET /hpc/config` and `GET /hpc/jobs` replies; edits `PUT` the site
//! profile and take effect on daemon restart (surfaced as a banner).

use ratatui::{
    buffer::Buffer,
    layout::Rect,
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
        key: "enabled",
        label: "Route process tasks",
        kind: FieldKind::Bool,
    },
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
    /// One-line operator feedback (save result, refresh errors).
    pub status: Option<String>,
    pub pending_restart: bool,
    pub site_profile_path: Option<String>,
    pub apptainer_manifest_path: Option<String>,
    pub apptainer_manifest_exists: bool,
    pub loading: bool,
}

impl HpcPanelState {
    /// Fill profile values from a `GET /hpc/config` reply.
    pub fn load_config(&mut self, reply: &serde_json::Value) {
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
        self.pending_restart = reply
            .get("pending_restart")
            .and_then(serde_json::Value::as_bool)
            .unwrap_or(false);
        self.site_profile_path = reply
            .get("site_profile")
            .and_then(|p| p.get("path"))
            .and_then(serde_json::Value::as_str)
            .map(String::from);
        let manifest = reply.get("apptainer_manifest");
        self.apptainer_manifest_path = manifest
            .and_then(|m| m.get("path"))
            .and_then(serde_json::Value::as_str)
            .map(String::from);
        self.apptainer_manifest_exists = manifest
            .and_then(|m| m.get("exists"))
            .and_then(serde_json::Value::as_bool)
            .unwrap_or(false);
    }

    /// Assemble the `PUT /hpc/config` profile body. Empty values are
    /// omitted (clearing an entry falls back to env/default). Returns the
    /// field key on a malformed number instead of sending a bad profile.
    pub fn build_profile(&self) -> Result<serde_json::Value, &'static str> {
        let mut profile = serde_json::Map::new();
        for (def, value) in FIELD_DEFS.iter().zip(&self.values) {
            let value = value.trim();
            if value.is_empty() {
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
        Ok(serde_json::Value::Object(profile))
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
    /// Re-fetch the monitor inventory.
    RefreshJobs,
}

impl HpcPanelState {
    /// Route a key. Returns `None` when the key was consumed internally.
    pub fn handle_key(&mut self, key: crossterm::event::KeyEvent) -> Option<HpcPanelCommand> {
        use crossterm::event::KeyCode;

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
            KeyCode::Char('r') => Some(HpcPanelCommand::RefreshJobs),
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
                HpcTab::Jobs | HpcTab::Targets => None,
            },
            KeyCode::Char('s') if self.tab == HpcTab::Config => match self.build_profile() {
                Ok(profile) => Some(HpcPanelCommand::SaveProfile(profile)),
                Err(key) => {
                    self.status = Some(format!(
                        "invalid value for '{key}' — expected a number or boolean flag"
                    ));
                    None
                }
            },
            _ => None,
        }
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
            tab("Site config", state.tab == HpcTab::Config),
            tab("Jobs", state.tab == HpcTab::Jobs),
        ]));
        lines.push(Line::from(""));

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

        // Status / pending-restart footer.
        if state.pending_restart {
            lines.push(Line::from(Span::styled(
                "! saved profile differs from the running daemon — restart to apply",
                Style::default().fg(theme.warning),
            )));
        }
        if let Some(status) = &state.status {
            lines.push(Line::from(Span::styled(
                status.clone(),
                Style::default().fg(theme.text_faint),
            )));
        }
        // Keymap hint — the panel's persistent footer.
        lines.push(Line::from(Span::styled(
            "Tab switch · r refresh · Esc close",
            Style::default().fg(theme.text_faint),
        )));

        Paragraph::new(lines)
            .block(Block::default().borders(Borders::NONE))
            .wrap(ratatui::widgets::Wrap { trim: false })
            .render(area, buf);
    }
}

impl HpcPanelWidget {
    fn render_config(
        &self,
        lines: &mut Vec<Line<'static>>,
        state: &HpcPanelState,
        theme: &crate::theme::Theme,
    ) {
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

        lines.push(Line::from(""));
        let manifest = state
            .apptainer_manifest_path
            .clone()
            .unwrap_or_else(|| "(unset)".into());
        let manifest_flag =
            if state.apptainer_manifest_path.is_some() && !state.apptainer_manifest_exists {
                "  ! file missing"
            } else {
                ""
            };
        lines.push(Line::from(Span::styled(
            format!("manifest: {manifest}{manifest_flag}"),
            Style::default().fg(theme.text_faint),
        )));
        if let Some(path) = &state.site_profile_path {
            lines.push(Line::from(Span::styled(
                format!("profile: {path}   s save"),
                Style::default().fg(theme.text_faint),
            )));
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

    fn state_with_values() -> HpcPanelState {
        let mut state = HpcPanelState::default();
        state.values = FIELD_DEFS.iter().map(|_| String::new()).collect();
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
                "enabled": true,
                "ssh_host": "sjtu",
                "poll_interval_secs": 15,
                "account": null
            },
            "pending_restart": true,
            "site_profile": {"path": "/s/slurm-site.json", "exists": false},
            "apptainer_manifest": {"path": "/m.json", "exists": false}
        });
        let mut state = state_with_values();
        state.load_config(&reply);
        assert!(state.pending_restart);
        assert_eq!(
            state.site_profile_path.as_deref(),
            Some("/s/slurm-site.json")
        );
        let enabled = FIELD_DEFS.iter().position(|d| d.key == "enabled").unwrap();
        assert_eq!(state.values[enabled], "true");
        let host = FIELD_DEFS.iter().position(|d| d.key == "ssh_host").unwrap();
        assert_eq!(state.values[host], "sjtu");
    }

    #[test]
    fn build_profile_skips_empty_and_rejects_bad_numbers() {
        let mut state = state_with_values();
        set_value(&mut state, "enabled", "true");
        set_value(&mut state, "ssh_host", "sjtu");
        set_value(&mut state, "account", "");
        set_value(&mut state, "poll_interval_secs", "15");
        let profile = state.build_profile().unwrap();
        assert_eq!(profile["enabled"], serde_json::json!(true));
        assert_eq!(profile["ssh_host"], serde_json::json!("sjtu"));
        assert_eq!(profile["poll_interval_secs"], serde_json::json!(15));
        assert!(profile.get("account").is_none());

        set_value(&mut state, "gpu_cpus_per_gpu", "sixteen");
        assert_eq!(state.build_profile().err(), Some("gpu_cpus_per_gpu"));
    }

    #[test]
    fn bool_toggle_and_text_edit_via_enter() {
        let mut state = state_with_values();
        set_value(&mut state, "enabled", "false");
        state.selected_field = FIELD_DEFS.iter().position(|d| d.key == "enabled").unwrap();
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
        set_value(&mut state, "enabled", "true");
        let cmd = state.handle_key(crossterm::event::KeyEvent::new(
            crossterm::event::KeyCode::Char('s'),
            crossterm::event::KeyModifiers::empty(),
        ));
        match cmd {
            Some(HpcPanelCommand::SaveProfile(profile)) => {
                assert_eq!(profile["enabled"], serde_json::json!(true));
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
