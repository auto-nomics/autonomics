use super::*;
use serde_json::{Value, json};

fn fixture(text: &[u8]) -> (tempfile::TempDir, VfsBashTool) {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join("page.html"), text).unwrap();
    let tool = VfsBashTool {
        storage: Arc::new(OpendalFileStorage::new(dir.path())),
    };
    (dir, tool)
}
fn input(offset: u64, token: Option<&str>, budget: usize) -> VfsBashInput {
    serde_json::from_value(json!({"op":"read_chunk","path":"/page.html","offset":offset,"max_bytes":budget,"source_version":token})).unwrap()
}
fn value(result: AgentToolResult) -> Value {
    assert_ne!(result.is_error, Some(true), "{result:?}");
    match result.content {
        ToolResultContent::Json(v) => v,
        other => panic!("Expected structured page, got {other:?}"),
    }
}

#[tokio::test]
async fn long_single_line_roundtrips_through_real_tool_and_output_bound() {
    let original = format!(
        "<html><body>{}</body></html>",
        "取材🧬<&\"\\>".repeat(16000)
    );
    let (_dir, tool) = fixture(original.as_bytes());
    let mut offset = 0;
    let mut token: Option<String> = None;
    let mut recovered = String::new();
    let mut pages = 0;
    loop {
        let page = value(
            tool.run(input(offset, token.as_deref(), 4096))
                .await
                .unwrap(),
        );
        assert_eq!(page["output_truncated"], false);
        assert!(
            serde_json::to_string(&page).unwrap().chars().count()
                < DEFAULT_MAX_CHARS - VFS_OUTPUT_MARGIN
        );
        assert_eq!(page["byte_offset"], offset);
        let content = page["content"].as_str().unwrap();
        assert!(content.len() <= 4096);
        assert_eq!(page["bytes_read"], content.len());
        recovered.push_str(content);
        pages += 1;
        let next_token = page["source_version"].as_str().unwrap().to_string();
        if let Some(previous) = &token {
            assert_eq!(previous, &next_token);
        }
        token = Some(next_token);
        if page["eof"] == true {
            assert!(page["next_offset"].is_null());
            break;
        }
        let next = page["next_offset"].as_u64().unwrap();
        assert!(next > offset);
        offset = next;
    }
    assert!(pages > 10);
    assert_eq!(recovered, original);
}

#[tokio::test]
async fn worst_case_json_escaping_keeps_cursor_and_complete_page() {
    let original = "\u{1}".repeat(9000);
    let (_dir, tool) = fixture(original.as_bytes());
    let page = value(tool.run(input(0, None, 4096)).await.unwrap());
    assert_eq!(page["content"], "\u{1}".repeat(4096));
    assert_eq!(page["next_offset"], 4096);
    assert_eq!(page["output_truncated"], false);
    assert!(serde_json::to_string(&page).unwrap().len() < DEFAULT_MAX_CHARS - VFS_OUTPUT_MARGIN);
}

#[tokio::test]
async fn changed_source_refuses_continuation() {
    let (dir, tool) = fixture(b"0123456789");
    let first = value(tool.run(input(0, None, 4)).await.unwrap());
    std::fs::write(dir.path().join("page.html"), b"a different source length").unwrap();
    assert!(
        tool.run(input(4, first["source_version"].as_str(), 4))
            .await
            .unwrap_err()
            .to_string()
            .contains("metadata changed")
    );
}

#[tokio::test]
async fn invalid_utf8_and_non_boundary_offsets_are_refused() {
    let (_dir, tool) = fixture("🧬🧬🧬".as_bytes());
    let first = value(tool.run(input(0, None, 4)).await.unwrap());
    assert_eq!(first["content"], "🧬");
    assert_eq!(first["next_offset"], 4);
    assert!(
        tool.run(input(1, first["source_version"].as_str(), 4))
            .await
            .is_err()
    );
    for source in [
        b"bad\xfftext".as_slice(),
        b"bad\0data".as_slice(),
        b"last\xf0\x9f".as_slice(),
    ] {
        let (_dir, tool) = fixture(source);
        assert!(tool.run(input(0, None, 4096)).await.is_err());
    }
}

#[tokio::test]
async fn empty_and_exact_end_are_unambiguous() {
    let (_dir, tool) = fixture(b"");
    let page = value(tool.run(input(0, None, 4)).await.unwrap());
    assert_eq!(page["content"], "");
    assert_eq!(page["bytes_read"], 0);
    assert_eq!(page["eof"], true);
    assert_eq!(page["content_complete"], true);
    assert!(page["next_offset"].is_null());
    let (_dir, tool) = fixture(b"abcd");
    let page = value(tool.run(input(0, None, 4)).await.unwrap());
    assert_eq!(page["content_complete"], true);
    let last = value(
        tool.run(input(4, page["source_version"].as_str(), 4))
            .await
            .unwrap(),
    );
    assert_eq!(last["eof"], true);
    assert_eq!(last["content"], "");
    assert_eq!(last["content_complete"], false);
}

#[tokio::test]
async fn invalid_budgets_offsets_and_missing_paths_fail_explicitly() {
    let (_dir, tool) = fixture(b"abcdef");
    for budget in [0, 1, 3, 4097, usize::MAX] {
        assert!(tool.run(input(0, None, budget)).await.is_err());
    }
    assert!(tool.run(input(4, None, 4)).await.is_err());
    let first = value(tool.run(input(0, None, 4)).await.unwrap());
    assert!(
        tool.run(input(7, first["source_version"].as_str(), 4))
            .await
            .is_err()
    );
    let mut request = input(0, None, 4);
    request.limit = Some(1);
    assert!(tool.run(request).await.is_err());
    let mut request = input(0, None, 4);
    request.path = None;
    assert!(tool.run(request).await.is_err());
}

#[tokio::test]
async fn read_chunk_obeys_vfs_mount_and_permission_checks() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join("page.html"), b"mounted source").unwrap();
    let mut manifest = crate::VfsManifest::local_root(dir.path().to_string_lossy());
    manifest.mount[0].path = "/mounted".into();
    manifest.mount[0].read_only = true;
    let mounts = Arc::new(crate::MountedObjectStore::from_manifest(&manifest).unwrap());
    let tool = VfsBashTool {
        storage: Arc::new(OpendalFileStorage::with_mounts(dir.path(), mounts)),
    };
    let mut request = input(0, None, 4096);
    request.path = Some("/mounted/page.html".into());
    assert_eq!(
        value(tool.run(request).await.unwrap())["content"],
        "mounted source"
    );
    let mut request = input(0, None, 4096);
    request.path = Some("/outside/page.html".into());
    assert!(tool.run(request).await.is_err());
    use crate::permission::{
        MountPermissions, VfsAccess, VfsMode, VfsOwnership, VfsPathRule, VfsPrincipal,
    };
    manifest.mount[0].permissions = MountPermissions::unix(
        VfsOwnership { uid: 0, gid: 100 },
        VfsMode::from_bits(0o777),
        VfsMode::from_bits(0o666),
        VfsMode::from_bits(0o777),
    )
    .with_rules(vec![VfsPathRule::Deny {
        path: "page.html".into(),
        access: vec![VfsAccess::Read],
    }]);
    let denied_mounts = Arc::new(crate::MountedObjectStore::from_manifest(&manifest).unwrap());
    let denied = VfsBashTool {
        storage: Arc::new(
            OpendalFileStorage::with_mounts(dir.path(), denied_mounts)
                .with_principal(VfsPrincipal::plugin_developer(10000)),
        ),
    };
    let mut request = input(0, None, 4096);
    request.path = Some("/mounted/page.html".into());
    assert!(denied.run(request).await.is_err());
}

#[tokio::test]
async fn old_long_line_read_gives_an_actionable_range_recovery_hint() {
    let original = "x".repeat(100000);
    let (_dir, tool) = fixture(original.as_bytes());
    let mut request = input(0, None, 4096);
    request.op = "read".into();
    request.offset = Some(1);
    request.limit = Some(1);
    let result = value(tool.run(request).await.unwrap());
    assert_eq!(result["output_truncated"], true);
    assert_eq!(result["read_chunk_request"]["op"], "read_chunk");
    assert_eq!(result["read_chunk_request"]["path"], "/page.html");
    assert_eq!(result["read_chunk_request"]["offset"], 0);
}
