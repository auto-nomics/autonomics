//! VFS browser overlay: a two-pane file manager over the daemon's
//! `/api/v1/vfs` endpoints — the directory list on the left, the
//! selected file's text preview on the right. Mutations (mkdir / new
//! file / rename / delete) confirm in-widget; the app glue dispatches
//! them through the gateway client and reloads the listing.

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use ratatui::{
    layout::{Constraint, Direction, Layout, Rect},
    prelude::Buffer,
    style::{Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Borders, List, ListItem, ListState, Paragraph, StatefulWidget, StatefulWidgetRef, Widget, Wrap},
};
use vfs::browse::{TextRead, VfsEntry};

use crate::theme;
use crate::widgets::popup::{Popup, PopupControls};
use crate::xai_textarea::{TextArea, TextAreaState};

/// Read actions the app glue must perform (fetches + mutations).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum VfsBrowserCommand {
    Close,
    /// (Re)fetch the listing for the current directory and the mounts.
    Refresh,
    /// Fetch the listing for a directory (navigation).
    LoadDir(String),
    /// Fetch the text preview for a file.
    LoadText(String),
    /// Create a directory.
    Mkdir(String),
    /// Create an empty text file.
    NewFile(String),
    /// Rename/move within the current directory's parent.
    Rename { src: String, dst: String },
    /// Delete a file or (recursively) a directory.
    Remove { path: String, recursive: bool },
}

/// A pending inline name input (mkdir / new file / rename).
pub struct VfsInput {
    pub kind: VfsInputKind,
    pub textarea: TextArea,
    pub textarea_state: TextAreaState,
}

pub enum VfsInputKind {
    Mkdir,
    NewFile,
    Rename { src: String },
}

/// Browser state: navigation, listing snapshot, preview, pending input.
#[derive(Default)]
pub struct VfsBrowserState {
    pub visible: bool,
    pub loading: bool,
    pub error: Option<String>,
    pub status: Option<String>,
    /// Declarative mounts, for the read-only indicator.
    pub mounts: Vec<(String, bool)>,
    pub cwd: String,
    pub entries: Vec<VfsEntry>,
    pub list_state: ListState,
    pub preview: Option<TextRead>,
    pub preview_scroll: u16,
    pub input: Option<VfsInput>,
    /// Delete confirmation target (message shown until y/n).
    pub confirm_delete: Option<(String, bool)>,
}

impl VfsBrowserState {
    pub fn open(&mut self) {
        self.visible = true;
        self.error = None;
        self.status = None;
        if self.cwd.is_empty() {
            self.cwd = "/".into();
            self.list_state.select(Some(0));
        }
    }

    pub fn close(&mut self) {
        self.visible = false;
        self.input = None;
        self.confirm_delete = None;
    }

    /// Longest read-only mount covering the current directory, if any.
    pub fn read_only_cwd(&self) -> bool {
        self.mounts.iter().any(|(mount, read_only)| {
            *read_only
                && (self.cwd == *mount || self.cwd.starts_with(&format!("{}/", mount.trim_end_matches('/'))))
        })
    }

    pub fn selected(&self) -> Option<&VfsEntry> {
        self.list_state
            .selected()
            .and_then(|index| self.entries.get(index))
    }

    fn parent_of(path: &str) -> String {
        let trimmed = path.trim_end_matches('/');
        match trimmed.rfind('/') {
            Some(0) | None => "/".to_string(),
            Some(index) => trimmed[..index].to_string(),
        }
    }

    fn child_path(dir: &str, name: &str) -> String {
        if dir == "/" {
            format!("/{name}")
        } else {
            format!("{}/{}", dir.trim_end_matches('/'), name)
        }
    }

    /// Apply a freshly loaded listing, keeping the cursor when possible.
    pub fn apply_listing(&mut self, cwd: String, entries: Vec<VfsEntry>) {
        self.cwd = cwd;
        self.entries = entries;
        self.loading = false;
        let selected = self.list_state.selected().unwrap_or(0).min(self.entries.len().saturating_sub(1));
        self.list_state.select(Some(selected));
    }

    /// Route a key; returns the command for the app glue, if any.
    pub fn handle_key(&mut self, key: KeyEvent) -> Option<VfsBrowserCommand> {
        // ── Inline input mode captures everything ──
        if self.input.is_some() {
            match key.code {
                KeyCode::Enter => {
                    let mut input = self.input.take().expect("checked above");
                    let value = input.textarea.text().trim().to_string();
                    let kind = std::mem::replace(&mut input.kind, VfsInputKind::Mkdir);
                    if value.is_empty() {
                        return None;
                    }
                    return match kind {
                        VfsInputKind::Mkdir => Some(VfsBrowserCommand::Mkdir(Self::child_path(&self.cwd, &value))),
                        VfsInputKind::NewFile => Some(VfsBrowserCommand::NewFile(Self::child_path(&self.cwd, &value))),
                        VfsInputKind::Rename { src } => {
                            let parent = Self::parent_of(&src);
                            Some(VfsBrowserCommand::Rename {
                                src,
                                dst: Self::child_path(&parent, &value),
                            })
                        }
                    };
                }
                KeyCode::Esc => {
                    self.input = None;
                    None
                }
                _ => {
                    if let Some(input) = self.input.as_mut() {
                        input.textarea.input(key);
                    }
                    None
                }
            }
        } else if let Some((target, is_dir)) = self.confirm_delete.clone() {
            match key.code {
                KeyCode::Char('y') | KeyCode::Char('Y') => {
                    self.confirm_delete = None;
                    Some(VfsBrowserCommand::Remove { path: target, recursive: is_dir })
                }
                KeyCode::Char('n') | KeyCode::Char('N') | KeyCode::Esc => {
                    self.confirm_delete = None;
                    None
                }
                _ => None,
            }
        } else {
            self.handle_browse_key(key)
        }
    }

    fn handle_browse_key(&mut self, key: KeyEvent) -> Option<VfsBrowserCommand> {
        if key.modifiers.contains(KeyModifiers::CONTROL) {
            return match key.code {
                KeyCode::Char('f') => Some(VfsBrowserCommand::Close),
                _ => None,
            };
        }
        match key.code {
            KeyCode::Esc | KeyCode::Char('q') => Some(VfsBrowserCommand::Close),
            KeyCode::Up | KeyCode::Char('k') => {
                self.step(-1);
                None
            }
            KeyCode::Down | KeyCode::Char('j') => {
                self.step(1);
                None
            }
            KeyCode::Enter | KeyCode::Char('l') | KeyCode::Right => {
                let selected = self.selected().cloned();
                match selected {
                    Some(entry) if entry.is_dir => Some(VfsBrowserCommand::LoadDir(entry.path.clone())),
                    Some(entry) => Some(VfsBrowserCommand::LoadText(entry.path.clone())),
                    None => None,
                }
            }
            KeyCode::Backspace | KeyCode::Char('h') | KeyCode::Left => {
                if self.cwd != "/" {
                    Some(VfsBrowserCommand::LoadDir(Self::parent_of(&self.cwd)))
                } else {
                    None
                }
            }
            KeyCode::Char('r') => Some(VfsBrowserCommand::Refresh),
            KeyCode::Char('R') => {
                // Rename the selected entry.
                let selected = self.selected().cloned()?;
                let mut textarea = TextArea::new();
                textarea.show_scrollbar = false;
                textarea.set_text(&selected.name);
                textarea.set_cursor(selected.name.len());
                self.input = Some(VfsInput {
                    kind: VfsInputKind::Rename { src: selected.path.clone() },
                    textarea,
                    textarea_state: TextAreaState::default(),
                });
                None
            }
            KeyCode::Char('d') => {
                let selected = self.selected().cloned()?;
                self.confirm_delete = Some((selected.path.clone(), selected.is_dir));
                None
            }
            KeyCode::Char('n') => {
                if self.read_only_cwd() {
                    self.error = Some("只读挂载，不能新建文件。".into());
                    return None;
                }
                let mut textarea = TextArea::new();
                textarea.show_scrollbar = false;
                self.input = Some(VfsInput {
                    kind: VfsInputKind::NewFile,
                    textarea,
                    textarea_state: TextAreaState::default(),
                });
                None
            }
            KeyCode::Char('m') => {
                if self.read_only_cwd() {
                    self.error = Some("只读挂载，不能新建目录。".into());
                    return None;
                }
                let mut textarea = TextArea::new();
                textarea.show_scrollbar = false;
                self.input = Some(VfsInput {
                    kind: VfsInputKind::Mkdir,
                    textarea,
                    textarea_state: TextAreaState::default(),
                });
                None
            }
            KeyCode::PageDown => {
                self.preview_scroll = self.preview_scroll.saturating_add(10);
                None
            }
            KeyCode::PageUp => {
                self.preview_scroll = self.preview_scroll.saturating_sub(10);
                None
            }
            _ => None,
        }
    }

    fn step(&mut self, delta: i32) {
        if self.entries.is_empty() {
            self.list_state.select(None);
            return;
        }
        let current = self.list_state.selected().unwrap_or(0) as i32;
        let next = (current + delta).clamp(0, self.entries.len() as i32 - 1) as usize;
        self.list_state.select(Some(next));
    }
}

/// Render the overlay: header (cwd + read-only badge), the two panes,
/// and a footer with key hints / the pending input or confirmation.
pub fn render_vfs_browser(area: Rect, buf: &mut Buffer, state: &mut VfsBrowserState) {
    let popup_width = area.width * 9 / 10;
    let popup_height = area.height * 9 / 10;
    let popup = Popup::new(" VFS 浏览器 ", PopupControls::default())
        .width(popup_width)
        .height(popup_height)
        .accent(theme::current().accent_alt);
    let inner = popup.render(area, buf);

    let read_only = state.read_only_cwd();
    let footer = if state.input.is_some() {
        "Enter 确认 · Esc 取消".to_string()
    } else if state.confirm_delete.is_some() {
        "y 删除 · n 取消".to_string()
    } else {
        "Enter 打开/进入 · h/u 返回 · m 新建目录 · n 新建文件 · R 重命名 · d 删除 · r 刷新 · PgUp/PgDn 预览滚动 · q 关闭".to_string()
    };

    let body = Layout::default()
        .direction(Direction::Vertical)
        .constraints([Constraint::Length(1), Constraint::Min(3), Constraint::Length(1), Constraint::Length(1)])
        .split(inner);
    let (header, panes, status_line, footer_line) = (body[0], body[1], body[2], body[3]);

    // Header: cwd + read-only badge + pending status.
    let mut header_spans = vec![
        Span::styled(state.cwd.clone(), Style::default().add_modifier(Modifier::BOLD)),
    ];
    if read_only {
        header_spans.push(Span::styled("  [只读挂载]", Style::default().fg(theme::current().text_dim)));
    }
    if state.loading {
        header_spans.push(Span::styled("  加载中…", Style::default().fg(theme::current().text_dim)));
    }
    Paragraph::new(Line::from(header_spans)).render(header, buf);

    let columns = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([Constraint::Percentage(45), Constraint::Percentage(55)])
        .split(panes);

    // Left: the directory listing (dirs first — the API already sorts).
    let items: Vec<ListItem> = state
        .entries
        .iter()
        .map(|entry| {
            let marker = if entry.is_dir { "▸ " } else { "  " };
            ListItem::new(Line::from(vec![
                Span::raw(marker),
                Span::styled(
                    entry.name.clone(),
                    if entry.is_dir {
                        Style::default().add_modifier(Modifier::BOLD)
                    } else {
                        Style::default()
                    },
                ),
                Span::raw(format!("  {}", format_size(entry.size))),
            ]))
        })
        .collect();
    let list = List::new(items)
        .block(Block::default().borders(Borders::ALL).title(" 目录 "))
        .highlight_style(Style::default().add_modifier(Modifier::REVERSED));
    StatefulWidget::render(list, columns[0], buf, &mut state.list_state);

    // Right: preview pane.
    let preview_lines: Vec<Line> = match &state.preview {
        Some(text) => text
            .contents
            .lines()
            .map(|line| Line::from(line.to_string()))
            .collect(),
        None => vec![Line::from(Span::styled(
            "选择文件后按 Enter 预览文本",
            Style::default().fg(theme::current().text_dim),
        ))],
    };
    let preview = Paragraph::new(preview_lines)
        .block(Block::default().borders(Borders::ALL).title(" 预览 "))
        .scroll((state.preview_scroll, 0))
        .wrap(Wrap { trim: false });
    preview.render(columns[1], buf);

    // Status line: error, pending input, or delete confirmation.
    let status_text = if let Some(error) = &state.error {
        Line::from(Span::styled(error.clone(), Style::default().fg(theme::current().error)))
    } else if let Some((target, _)) = &state.confirm_delete {
        Line::from(Span::styled(format!("删除 {target}? [y/n]"), Style::default().fg(theme::current().error)))
    } else if let Some(input) = &state.input {
        let label = match &input.kind {
            VfsInputKind::Mkdir => "新建目录名:",
            VfsInputKind::NewFile => "新建文件名:",
            VfsInputKind::Rename { .. } => "重命名为:",
        };
        Line::from(vec![
            Span::raw(label.to_string()),
            Span::styled(input.textarea.text().to_string(), Style::default().add_modifier(Modifier::UNDERLINED)),
        ])
    } else if let Some(status) = &state.status {
        Line::from(Span::styled(status.clone(), Style::default().fg(theme::current().text_dim)))
    } else {
        Line::from("")
    };
    Paragraph::new(status_text).render(status_line, buf);
    Paragraph::new(Line::from(Span::styled(footer, Style::default().fg(theme::current().text_dim)))).render(footer_line, buf);

    // Inline input editor: render the single-line textarea over the
    // status line so editing is visible with a cursor.
    if let Some(input) = state.input.as_mut() {
        let editor = Layout::default()
            .direction(Direction::Horizontal)
            .constraints([Constraint::Length(14), Constraint::Min(3)])
            .split(status_line)[1];
        let widget: &TextArea = &input.textarea;
        widget.render_ref(editor, buf, &mut input.textarea_state);
    }
}

fn format_size(size: u64) -> String {
    if size < 1024 {
        format!("{size}B")
    } else if size < 1024 * 1024 {
        format!("{:.1}K", size as f64 / 1024.0)
    } else {
        format!("{:.1}M", size as f64 / (1024.0 * 1024.0))
    }
}
