//! VFS browser app glue — gateway fetches for the listing/preview/mounts,
//! mutation dispatch, and key routing for the browser widget. Async
//! replies come back as [`crate::app_event::AppEvent`] variants applied
//! in `runtime_events`.

use crossterm::event::{KeyCode, KeyEvent};

use super::*;
use crate::widgets::vfs_browser::{VfsBrowserCommand, VfsBrowserState};

impl App {
    /// Open the browser and kick off the mounts + root listing fetches.
    pub(super) fn open_vfs_browser(&mut self) {
        self.state.vfs_browser.open();
        self.state.vfs_browser.loading = true;
        self.fetch_vfs_mounts();
        self.fetch_vfs_listing(self.state.vfs_browser.cwd.clone());
        self.dirty = true;
    }

    fn fetch_vfs_mounts(&mut self) {
        let client = self.client.clone();
        let event_tx = self.app_event_tx.clone();
        self.spawn_client_task("vfs_mounts_fetch", move || async move {
            let result = client
                .vfs_mounts()
                .await
                .map(|reply| {
                    reply
                        .mounts
                        .into_iter()
                        .map(|mount| (mount.path, mount.read_only))
                        .collect::<Vec<_>>()
                })
                .map_err(|error| error.to_string());
            event_tx.send(crate::app_event::AppEvent::VfsMountsLoaded(result));
        });
    }

    fn fetch_vfs_listing(&mut self, path: String) {
        let client = self.client.clone();
        let event_tx = self.app_event_tx.clone();
        self.state.vfs_browser.loading = true;
        self.spawn_client_task("vfs_listing_fetch", move || async move {
            // Follow pagination so the pane shows the whole directory.
            let mut entries: Vec<vfs::browse::VfsEntry> = Vec::new();
            let mut offset: usize = 0;
            let mut cwd: String;
            loop {
                match client.vfs_list(&path, 500, offset as u32).await {
                    Ok(page) => {
                        cwd = page.path.clone();
                        let truncated = page.truncated;
                        entries.extend(page.entries);
                        if !truncated {
                            break;
                        }
                        offset = page.next_offset.unwrap_or(offset + 500);
                    }
                    Err(error) => {
                        event_tx.send(crate::app_event::AppEvent::VfsListingLoaded(Err(
                            error.to_string()
                        )));
                        return;
                    }
                }
            }
            event_tx.send(crate::app_event::AppEvent::VfsListingLoaded(Ok((
                cwd, entries,
            ))));
        });
    }

    fn fetch_vfs_text(&mut self, path: String) {
        let client = self.client.clone();
        let event_tx = self.app_event_tx.clone();
        self.spawn_client_task("vfs_text_fetch", move || async move {
            let result = client.vfs_read_text(&path).await.map_err(|e| e.to_string());
            event_tx.send(crate::app_event::AppEvent::VfsTextLoaded(result));
        });
    }

    fn run_vfs_mutation(
        &mut self,
        label: String,
        run: impl FnOnce(
            gateway::GatewayClient,
        ) -> std::pin::Pin<
            Box<
                dyn std::future::Future<Output = std::result::Result<serde_json::Value, String>>
                    + Send,
            >,
        > + Send
        + 'static,
    ) {
        let client = self.client.clone();
        let event_tx = self.app_event_tx.clone();
        self.spawn_client_task("vfs_mutation", move || async move {
            let result = run(client).await;
            event_tx.send(crate::app_event::AppEvent::VfsMutationDone(
                result.map(move |_| label),
            ));
        });
    }

    pub(super) fn handle_vfs_browser_key(&mut self, key: &KeyEvent) {
        let command = self
            .state
            .vfs_browser
            .handle_key(KeyEvent::new(key.code, key.modifiers));
        self.dirty = true;
        match command {
            Some(VfsBrowserCommand::Close) => {
                self.state.vfs_browser.close();
            }
            Some(VfsBrowserCommand::Refresh) => {
                let cwd = self.state.vfs_browser.cwd.clone();
                self.state.vfs_browser.error = None;
                self.fetch_vfs_mounts();
                self.fetch_vfs_listing(cwd);
            }
            Some(VfsBrowserCommand::LoadDir(path)) => {
                self.state.vfs_browser.preview = None;
                self.fetch_vfs_listing(path);
            }
            Some(VfsBrowserCommand::LoadText(path)) => {
                self.fetch_vfs_text(path);
            }
            Some(VfsBrowserCommand::Mkdir(path)) => {
                self.state.vfs_browser.status = Some("创建目录…".into());
                self.run_vfs_mutation(format!("已创建 {path}"), move |client| {
                    Box::pin(
                        async move { client.vfs_mkdir(&path).await.map_err(|e| e.to_string()) },
                    )
                });
            }
            Some(VfsBrowserCommand::NewFile(path)) => {
                self.state.vfs_browser.status = Some("创建文件…".into());
                let path_for_save = path.clone();
                self.run_vfs_mutation(format!("已创建 {path}"), move |client| {
                    Box::pin(async move {
                        client
                            .vfs_write_text(&path_for_save, "")
                            .await
                            .map_err(|e| e.to_string())
                    })
                });
            }
            Some(VfsBrowserCommand::Rename { src, dst }) => {
                self.state.vfs_browser.status = Some("重命名…".into());
                self.run_vfs_mutation(format!("已重命名为 {dst}"), move |client| {
                    Box::pin(
                        async move { client.vfs_move(&src, &dst).await.map_err(|e| e.to_string()) },
                    )
                });
            }
            Some(VfsBrowserCommand::Remove { path, recursive }) => {
                self.state.vfs_browser.status = Some("删除…".into());
                self.run_vfs_mutation(format!("已删除 {path}"), move |client| {
                    Box::pin(async move {
                        client
                            .vfs_delete(&path, recursive)
                            .await
                            .map_err(|e| e.to_string())
                    })
                });
            }
            None => {}
        }
    }

    // ── AppEvent appliers (invoked from runtime_events) ──────────────

    pub(super) fn apply_vfs_listing(
        &mut self,
        reply: std::result::Result<(String, Vec<vfs::browse::VfsEntry>), String>,
    ) {
        match reply {
            Ok((cwd, entries)) => self.state.vfs_browser.apply_listing(cwd, entries),
            Err(error) => {
                self.state.vfs_browser.loading = false;
                self.state.vfs_browser.error = Some(error);
            }
        }
        self.dirty = true;
    }

    pub(super) fn apply_vfs_text(
        &mut self,
        reply: std::result::Result<vfs::browse::TextRead, String>,
    ) {
        match reply {
            Ok(text) => {
                self.state.vfs_browser.preview = Some(text);
                self.state.vfs_browser.preview_scroll = 0;
                self.state.vfs_browser.error = None;
            }
            Err(error) => {
                self.state.vfs_browser.error = Some(error);
            }
        }
        self.dirty = true;
    }

    pub(super) fn apply_vfs_mounts(
        &mut self,
        reply: std::result::Result<Vec<(String, bool)>, String>,
    ) {
        match reply {
            Ok(mounts) => self.state.vfs_browser.mounts = mounts,
            Err(error) => {
                // Mounts are cosmetic (read-only badge); keep browsing.
                self.state.vfs_browser.error = Some(error);
            }
        }
        self.dirty = true;
    }

    pub(super) fn apply_vfs_mutation(&mut self, reply: std::result::Result<String, String>) {
        match reply {
            Ok(status) => {
                self.state.vfs_browser.status = Some(status);
                // The mutation landed server-side; refresh the pane.
                let cwd = self.state.vfs_browser.cwd.clone();
                self.fetch_vfs_listing(cwd);
            }
            Err(error) => {
                self.state.vfs_browser.status = Some(error);
            }
        }
        self.dirty = true;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::widgets::vfs_browser::{VfsBrowserCommand, VfsInputKind};
    use crossterm::event::KeyCode;

    fn key(code: KeyCode) -> KeyEvent {
        KeyEvent::new(code, crossterm::event::KeyModifiers::NONE)
    }

    fn state_with_entries() -> VfsBrowserState {
        let mut state = VfsBrowserState::default();
        state.open();
        state.apply_listing(
            "/".into(),
            vec![
                vfs::browse::VfsEntry {
                    path: "/data".into(),
                    name: "data".into(),
                    is_dir: true,
                    size: 0,
                    last_modified: None,
                },
                vfs::browse::VfsEntry {
                    path: "/notes.md".into(),
                    name: "notes.md".into(),
                    is_dir: false,
                    size: 12,
                    last_modified: None,
                },
            ],
        );
        state
    }

    #[test]
    fn navigation_moves_cursor_and_descends() {
        let mut state = state_with_entries();
        assert_eq!(state.list_state.selected(), Some(0));
        state.handle_key(key(KeyCode::Down));
        assert_eq!(state.list_state.selected(), Some(1));
        // Enter on a file previews it.
        assert_eq!(
            state.handle_key(key(KeyCode::Enter)),
            Some(VfsBrowserCommand::LoadText("/notes.md".into()))
        );
        state.handle_key(key(KeyCode::Up));
        assert_eq!(
            state.handle_key(key(KeyCode::Enter)),
            Some(VfsBrowserCommand::LoadDir("/data".into()))
        );
        // Ascending from a nested dir goes to the parent.
        state.apply_listing("/data".into(), vec![]);
        assert_eq!(
            state.handle_key(key(KeyCode::Backspace)),
            Some(VfsBrowserCommand::LoadDir("/".into()))
        );
    }

    #[test]
    fn delete_requires_confirmation() {
        let mut state = state_with_entries();
        state.handle_key(key(KeyCode::Down));
        // d arms the confirmation; only y dispatches the removal.
        assert_eq!(state.handle_key(key(KeyCode::Char('d'))), None);
        assert_eq!(state.handle_key(key(KeyCode::Char('n'))), None);
        state.handle_key(key(KeyCode::Char('d')));
        assert_eq!(
            state.handle_key(key(KeyCode::Char('y'))),
            Some(VfsBrowserCommand::Remove {
                path: "/notes.md".into(),
                recursive: false
            })
        );
    }

    #[test]
    fn mkdir_input_flow_submits_child_path() {
        let mut state = state_with_entries();
        assert_eq!(state.handle_key(key(KeyCode::Char('m'))), None);
        assert!(state.input.is_some());
        let input = state.input.as_mut().unwrap();
        assert!(matches!(input.kind, VfsInputKind::Mkdir));
        input.textarea.insert_str("scratch");
        assert_eq!(
            state.handle_key(key(KeyCode::Enter)),
            Some(VfsBrowserCommand::Mkdir("/scratch".into()))
        );
        assert!(state.input.is_none());
    }

    #[test]
    fn rename_input_prefills_and_targets_parent() {
        let mut state = state_with_entries();
        state.apply_listing(
            "/data".into(),
            vec![vfs::browse::VfsEntry {
                path: "/data/panel.parquet".into(),
                name: "panel.parquet".into(),
                is_dir: false,
                size: 5,
                last_modified: None,
            }],
        );
        state.handle_key(key(KeyCode::Char('R')));
        let input = state.input.as_mut().unwrap();
        assert_eq!(input.textarea.text(), "panel.parquet");
        input.textarea.set_text("renamed.parquet");
        assert_eq!(
            state.handle_key(key(KeyCode::Enter)),
            Some(VfsBrowserCommand::Rename {
                src: "/data/panel.parquet".into(),
                dst: "/data/renamed.parquet".into(),
            })
        );
    }

    #[test]
    fn read_only_mount_blocks_creation_shortcuts() {
        let mut state = state_with_entries();
        state.mounts = vec![("/".into(), true)];
        state.handle_key(key(KeyCode::Char('m')));
        assert!(
            state.input.is_none(),
            "mkdir input must not open on a read-only mount"
        );
        assert!(state.error.is_some());
    }
}
