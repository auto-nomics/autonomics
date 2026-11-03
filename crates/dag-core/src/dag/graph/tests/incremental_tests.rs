//! Incremental execution: fingerprint reuse across runs, invalidation on
//! mutation, and the plugin-identity constituent (WO-R09).

use std::sync::Arc;

use super::common::*;
use crate::dag::graph::DAG;
use crate::dag::runtime::{RuntimeStatus, SchedulerConfig};

#[tokio::test]
async fn incremental_run_detects_changed_file_output() {
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    let path = std::env::temp_dir().join(format!(
        "dag-core-file-output-{}-{nanos}.txt",
        std::process::id()
    ));
    let node = FileOutputNode::new(path.clone());
    let runs = node.runs.clone();
    let mut dag = DAG::default();
    dag.add_node("writer".into(), Box::new(node)).unwrap();

    let mut cfg = SchedulerConfig::default();
    cfg.incremental = true;
    dag.run(&cfg, &test_ctx(), None).await.unwrap();
    dag.run(&cfg, &test_ctx(), None).await.unwrap();
    assert_eq!(runs.load(std::sync::atomic::Ordering::SeqCst), 1);

    std::fs::write(&path, "externally changed").unwrap();
    dag.run(&cfg, &test_ctx(), None).await.unwrap();
    assert_eq!(runs.load(std::sync::atomic::Ordering::SeqCst), 2);
    let _ = std::fs::remove_file(&path);
}

/// First run in incremental mode should execute all nodes (all are dirty
/// because they were just added).
#[tokio::test]
async fn incremental_first_run_executes_all() {
    let mut dag = DAG::default();
    let ctr_a = Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let ctr_b = Arc::new(std::sync::atomic::AtomicUsize::new(0));

    dag.add_node("a".into(), Box::new(CountingEcho::new(ctr_a.clone())))
        .unwrap();
    dag.add_node("b".into(), Box::new(CountingEcho::new(ctr_b.clone())))
        .unwrap();
    dag.add_edge("a", "b", 0, 0).unwrap();
    dag.validate().unwrap();

    let ctx = test_ctx();
    let report = dag.run(&incremental_cfg(), &ctx, None).await.unwrap();
    assert!(report.ok);
    assert_eq!(cnt(&ctr_a), 1, "node a should execute once");
    assert_eq!(cnt(&ctr_b), 1, "node b should execute once");
    // After successful run, both should be clean.
    assert!(!dag.is_dirty("a"));
    assert!(!dag.is_dirty("b"));
}

/// A second incremental run with no changes should skip ALL nodes and reuse
/// cached outputs.
#[tokio::test]
async fn incremental_rerun_no_changes_skips_all() {
    let mut dag = DAG::default();
    let ctr_a = Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let ctr_b = Arc::new(std::sync::atomic::AtomicUsize::new(0));

    dag.add_node("a".into(), Box::new(CountingEcho::new(ctr_a.clone())))
        .unwrap();
    dag.add_node("b".into(), Box::new(CountingEcho::new(ctr_b.clone())))
        .unwrap();
    dag.add_edge("a", "b", 0, 0).unwrap();

    let ctx = test_ctx();
    let cfg = incremental_cfg();

    // First run.
    dag.run(&cfg, &ctx, None).await.unwrap();
    assert_eq!(cnt(&ctr_a), 1);
    assert_eq!(cnt(&ctr_b), 1);

    // Second run — nothing changed, all clean.
    let r2 = dag.run(&cfg, &ctx, None).await.unwrap();
    assert!(r2.ok);
    assert_eq!(cnt(&ctr_a), 1, "node a should NOT re-execute");
    assert_eq!(cnt(&ctr_b), 1, "node b should NOT re-execute");
    assert_eq!(dag.status("a"), Some(RuntimeStatus::Success));
    assert_eq!(dag.status("b"), Some(RuntimeStatus::Success));
    // Output still cached.
    assert!(dag.output("a").is_some());
    assert!(dag.output("b").is_some());
}

/// After a **spec-changing** replace on `a`, `a` re-executes and its
/// descendant `b` cascades through the identity chain (its input
/// references `a`'s new fingerprint); `c` (independent branch) reuses.
#[tokio::test]
async fn incremental_replace_reexecutes_only_descendants() {
    let mut dag = DAG::default();
    let ctr_a = Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let ctr_b = Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let ctr_c = Arc::new(std::sync::atomic::AtomicUsize::new(0));

    dag.add_node_with_spec(
        "a".into(),
        Box::new(CountingEcho::new(ctr_a.clone())),
        "counting-echo".into(),
        serde_json::json!({"v": 1}),
    )
    .unwrap();
    dag.add_node("b".into(), Box::new(CountingEcho::new(ctr_b.clone())))
        .unwrap();
    dag.add_node("c".into(), Box::new(CountingEcho::new(ctr_c.clone())))
        .unwrap();
    dag.add_edge("a", "b", 0, 0).unwrap();
    // c is independent (no edge from a).

    let ctx = test_ctx();
    let cfg = incremental_cfg();

    // First run.
    dag.run(&cfg, &ctx, None).await.unwrap();
    assert_eq!(cnt(&ctr_a), 1);
    assert_eq!(cnt(&ctr_b), 1);
    assert_eq!(cnt(&ctr_c), 1);

    // Replace node "a" with a fresh CountingEcho (new counter) and a
    // changed spec — the identity change is what cascades downstream.
    let ctr_a2 = Arc::new(std::sync::atomic::AtomicUsize::new(0));
    dag.replace_node_with_spec(
        "a",
        Box::new(CountingEcho::new(ctr_a2.clone())),
        "counting-echo".into(),
        serde_json::json!({"v": 2}),
    )
    .unwrap();

    assert!(dag.is_dirty("a"), "a should re-execute after replace");
    // `b` still holds its recorded fingerprint: whether it re-executes is
    // decided at dispatch, when `a`'s new fingerprint enters `b`'s
    // candidate identity.
    assert!(!dag.is_dirty("b"), "descendants re-evaluate at dispatch");
    assert!(!dag.is_dirty("c"), "c (independent) should be clean");

    // Second run.
    dag.run(&cfg, &ctx, None).await.unwrap();
    assert_eq!(cnt(&ctr_a2), 1, "replaced a should execute once");
    assert_eq!(cnt(&ctr_b), 2, "b should re-execute (a's identity changed)");
    assert_eq!(
        cnt(&ctr_c),
        1,
        "c should NOT re-execute (independent branch)"
    );
}

/// A payload-only swap (same kind + spec) re-executes just the swapped
/// node: its re-execution reproduces an identical identity, so
/// descendants are correctly reused instead of eagerly invalidated.
#[tokio::test]
async fn incremental_payload_swap_reruns_only_the_node() {
    let mut dag = DAG::default();
    let ctr_a = Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let ctr_b = Arc::new(std::sync::atomic::AtomicUsize::new(0));

    dag.add_node("a".into(), Box::new(CountingEcho::new(ctr_a.clone())))
        .unwrap();
    dag.add_node("b".into(), Box::new(CountingEcho::new(ctr_b.clone())))
        .unwrap();
    dag.add_edge("a", "b", 0, 0).unwrap();

    let ctx = test_ctx();
    let cfg = incremental_cfg();

    dag.run(&cfg, &ctx, None).await.unwrap();
    assert_eq!(cnt(&ctr_a), 1);
    assert_eq!(cnt(&ctr_b), 1);

    // Same kind, same (absent) spec — identity unchanged.
    let ctr_a2 = Arc::new(std::sync::atomic::AtomicUsize::new(0));
    dag.replace_node("a", Box::new(CountingEcho::new(ctr_a2.clone())))
        .unwrap();
    assert!(dag.is_dirty("a"), "the swapped node re-executes");

    dag.run(&cfg, &ctx, None).await.unwrap();
    assert_eq!(cnt(&ctr_a2), 1, "swapped a should execute once");
    assert_eq!(
        cnt(&ctr_b),
        1,
        "b should be reused — a reproduced an identical identity"
    );
}

/// After `add_edge`, the target node and its descendants should be dirty.
#[tokio::test]
async fn incremental_add_edge_marks_target_dirty() {
    let mut dag = DAG::default();
    let ctr_a = Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let ctr_b = Arc::new(std::sync::atomic::AtomicUsize::new(0));

    dag.add_node("a".into(), Box::new(CountingEcho::new(ctr_a.clone())))
        .unwrap();
    dag.add_node("b".into(), Box::new(CountingEcho::new(ctr_b.clone())))
        .unwrap();

    let ctx = test_ctx();
    let cfg = incremental_cfg();

    // First run — two independent nodes.
    dag.run(&cfg, &ctx, None).await.unwrap();
    assert_eq!(cnt(&ctr_a), 1);
    assert_eq!(cnt(&ctr_b), 1);

    // Connect a → b.
    dag.add_edge("a", "b", 0, 0).unwrap();
    assert!(dag.is_dirty("b"), "b should be dirty after add_edge");
    assert!(!dag.is_dirty("a"), "a should remain clean");

    // Second run — only b should re-execute.
    dag.run(&cfg, &ctx, None).await.unwrap();
    assert_eq!(cnt(&ctr_a), 1, "a should NOT re-execute");
    assert_eq!(cnt(&ctr_b), 2, "b should re-execute");
}

/// After `delete_edge`, the target node and its descendants should be dirty.
#[tokio::test]
async fn incremental_delete_edge_marks_target_dirty() {
    let mut dag = DAG::default();
    let ctr_a = Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let ctr_b = Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let ctr_c = Arc::new(std::sync::atomic::AtomicUsize::new(0));

    dag.add_node("a".into(), Box::new(CountingEcho::new(ctr_a.clone())))
        .unwrap();
    dag.add_node("b".into(), Box::new(CountingEcho::new(ctr_b.clone())))
        .unwrap();
    dag.add_node("c".into(), Box::new(CountingEcho::new(ctr_c.clone())))
        .unwrap();
    dag.add_edge("a", "b", 0, 0).unwrap();
    dag.add_edge("b", "c", 0, 0).unwrap();

    let ctx = test_ctx();
    let cfg = incremental_cfg();

    // First run.
    dag.run(&cfg, &ctx, None).await.unwrap();
    assert_eq!(cnt(&ctr_a), 1);
    assert_eq!(cnt(&ctr_b), 1);
    assert_eq!(cnt(&ctr_c), 1);

    // Delete edge a → b.
    dag.delete_edge("a", "b", 0, 0).unwrap();
    assert!(dag.is_dirty("b"), "b should be dirty after delete_edge");
    // c re-evaluates at dispatch: b's new identity (one fewer input)
    // enters c's candidate fingerprint there.
    assert!(!dag.is_dirty("a"), "a should remain clean");

    // Second run — only b and c should re-execute.
    dag.run(&cfg, &ctx, None).await.unwrap();
    assert_eq!(cnt(&ctr_a), 1, "a should NOT re-execute");
    assert_eq!(cnt(&ctr_b), 2, "b should re-execute");
    assert_eq!(cnt(&ctr_c), 2, "c should re-execute");
}

/// Manual `mark_dirty` re-executes the marked node; descendants are
/// reused when the node reproduces an identical identity (the
/// fingerprint-model improvement over eager propagation).
#[tokio::test]
async fn incremental_manual_mark_dirty_reruns_marked_node() {
    let mut dag = DAG::default();
    // a → b → c → d (linear chain)
    let ctrs: Vec<Arc<std::sync::atomic::AtomicUsize>> = (0..4)
        .map(|_| Arc::new(std::sync::atomic::AtomicUsize::new(0)))
        .collect();
    for (i, id) in ["a", "b", "c", "d"].iter().enumerate() {
        dag.add_node((*id).into(), Box::new(CountingEcho::new(ctrs[i].clone())))
            .unwrap();
    }
    dag.add_edge("a", "b", 0, 0).unwrap();
    dag.add_edge("b", "c", 0, 0).unwrap();
    dag.add_edge("c", "d", 0, 0).unwrap();

    let ctx = test_ctx();
    let cfg = incremental_cfg();

    // First run.
    dag.run(&cfg, &ctx, None).await.unwrap();
    for ctr in &ctrs {
        assert_eq!(ctr.load(std::sync::atomic::Ordering::SeqCst), 1);
    }

    // Manually mark "b" — e.g. an external input it reads has changed.
    dag.mark_dirty("b");
    assert!(!dag.is_dirty("a"));
    assert!(dag.is_dirty("b"));
    // c and d still hold recorded fingerprints: they re-evaluate at
    // dispatch through the identity chain.
    assert!(!dag.is_dirty("c"));
    assert!(!dag.is_dirty("d"));

    // Second run. b re-executes; because b is a pure echo its
    // re-execution reproduces the identical identity, so c and d are
    // correctly reused instead of eagerly invalidated.
    dag.run(&cfg, &ctx, None).await.unwrap();
    assert_eq!(
        ctrs[0].load(std::sync::atomic::Ordering::SeqCst),
        1,
        "a should NOT re-execute"
    );
    assert_eq!(
        ctrs[1].load(std::sync::atomic::Ordering::SeqCst),
        2,
        "b should re-execute"
    );
    assert_eq!(
        ctrs[2].load(std::sync::atomic::Ordering::SeqCst),
        1,
        "c should be reused (b reproduced identical identity)"
    );
    assert_eq!(
        ctrs[3].load(std::sync::atomic::Ordering::SeqCst),
        1,
        "d should be reused (identity chain unchanged)"
    );
}

/// WO-R09 acceptance at the scheduler level: the payload's plugin
/// identity is a fingerprint constituent, a pure re-run reuses, and a
/// plugin swap under identical kind + spec invalidates the cache.
#[tokio::test]
async fn plugin_identity_participates_in_reuse() {
    let identity = |manifest_sha256: &str| crate::fingerprint::PluginIdentity {
        manifest_sha256: manifest_sha256.to_string(),
        script_sha256: Some("sha256:bbbb".to_string()),
        image_reference: "reg.example/acme/plug@sha256:1".to_string(),
        panels: Vec::new(),
    };
    // Two identities differing in exactly one manifest-hash nibble —
    // the "one character edited in manifest.toml" scenario.
    let identity_a = identity("sha256:aaaa");
    let identity_b = identity("sha256:aaab");
    let ctr = Arc::new(std::sync::atomic::AtomicUsize::new(0));

    let mut dag = DAG::default();
    let spec = serde_json::json!({"x": 1});
    dag.add_node_with_spec(
        "p".into(),
        Box::new(PluginBackedEcho::new(ctr.clone(), identity_a.clone())),
        "plugin_echo".to_string(),
        spec.clone(),
    )
    .unwrap();

    let cfg = incremental_cfg();
    dag.run(&cfg, &test_ctx(), None).await.unwrap();
    assert_eq!(ctr.load(std::sync::atomic::Ordering::SeqCst), 1);

    // Wiring: the recorded fingerprint must be the digest computed over
    // the payload's identity — not over no plugin and not over another
    // identity. (Expected values come from the public fingerprint API,
    // so this pins *what was fed*, not the digest formula itself.)
    let recorded = dag.recorded_fingerprint("p").unwrap().to_string();
    let feed = |plugin: Option<&crate::fingerprint::PluginIdentity>| {
        crate::fingerprint::compute_node_fingerprint(
            "plugin_echo",
            Some(&spec),
            crate::engine_version(),
            &[],
            plugin,
        )
    };
    assert_eq!(recorded, feed(Some(&identity_a)));
    assert_ne!(recorded, feed(Some(&identity_b)));
    assert_ne!(recorded, feed(None));

    // Pure data re-run: nothing changed — reused, not re-executed.
    dag.run(&cfg, &test_ctx(), None).await.unwrap();
    assert_eq!(
        ctr.load(std::sync::atomic::Ordering::SeqCst),
        1,
        "unchanged identity + unchanged inputs must reuse"
    );

    // Plugin swap under identical kind + spec: the fingerprint moves
    // and the node re-executes.
    dag.replace_node_with_spec(
        "p",
        Box::new(PluginBackedEcho::new(ctr.clone(), identity_b.clone())),
        "plugin_echo".to_string(),
        spec.clone(),
    )
    .unwrap();
    dag.run(&cfg, &test_ctx(), None).await.unwrap();
    assert_eq!(
        ctr.load(std::sync::atomic::Ordering::SeqCst),
        2,
        "a plugin-implementation change must invalidate the cache"
    );
    assert_eq!(
        dag.recorded_fingerprint("p").unwrap(),
        feed(Some(&identity_b))
    );
}

/// `mark_all_dirty` forces a full re-run even in incremental mode.
#[tokio::test]
async fn incremental_mark_all_forces_full_rerun() {
    let mut dag = DAG::default();
    let ctr_a = Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let ctr_b = Arc::new(std::sync::atomic::AtomicUsize::new(0));

    dag.add_node("a".into(), Box::new(CountingEcho::new(ctr_a.clone())))
        .unwrap();
    dag.add_node("b".into(), Box::new(CountingEcho::new(ctr_b.clone())))
        .unwrap();
    dag.add_edge("a", "b", 0, 0).unwrap();

    let ctx = test_ctx();
    let cfg = incremental_cfg();

    // First run.
    dag.run(&cfg, &ctx, None).await.unwrap();
    assert_eq!(cnt(&ctr_a), 1);
    assert_eq!(cnt(&ctr_b), 1);

    // Mark all dirty.
    dag.mark_all_dirty();
    assert!(dag.is_dirty("a"));
    assert!(dag.is_dirty("b"));

    // Second run — everything re-executes.
    dag.run(&cfg, &ctx, None).await.unwrap();
    assert_eq!(cnt(&ctr_a), 2);
    assert_eq!(cnt(&ctr_b), 2);
}

/// Incremental mode with a diamond DAG: a spec change on one branch
/// re-runs only that branch + the merge node, not the other branch.
#[tokio::test]
async fn incremental_diamond_partial_rerun() {
    let mut dag = DAG::default();
    // Diamond: a → b → d, a → c → d
    let ctrs: Vec<Arc<std::sync::atomic::AtomicUsize>> = (0..4)
        .map(|_| Arc::new(std::sync::atomic::AtomicUsize::new(0)))
        .collect();
    dag.add_node_with_spec(
        "b".into(),
        Box::new(CountingEcho::new(ctrs[1].clone())),
        "counting-echo".into(),
        serde_json::json!({"v": 1}),
    )
    .unwrap();
    for (id, idx) in [("a", 0), ("c", 2), ("d", 3)] {
        dag.add_node(id.into(), Box::new(CountingEcho::new(ctrs[idx].clone())))
            .unwrap();
    }
    dag.add_edge("a", "b", 0, 0).unwrap();
    dag.add_edge("a", "c", 0, 0).unwrap();
    // Use distinct input ports on "d" (0 and 1) — strict 1:1 validation
    // rejects two edges to the same port even on variadic nodes.
    dag.add_edge("b", "d", 0, 0).unwrap();
    dag.add_edge("c", "d", 0, 1).unwrap();

    let ctx = test_ctx();
    let cfg = incremental_cfg();

    // First run.
    dag.run(&cfg, &ctx, None).await.unwrap();
    for ctr in &ctrs {
        assert_eq!(ctr.load(std::sync::atomic::Ordering::SeqCst), 1);
    }

    // Replace "b" with a changed spec — b re-executes and d cascades
    // through the identity chain (not a, not c).
    let ctr_b2 = Arc::new(std::sync::atomic::AtomicUsize::new(0));
    dag.replace_node_with_spec(
        "b",
        Box::new(CountingEcho::new(ctr_b2.clone())),
        "counting-echo".into(),
        serde_json::json!({"v": 2}),
    )
    .unwrap();

    assert!(!dag.is_dirty("a"));
    assert!(dag.is_dirty("b"));
    assert!(!dag.is_dirty("c"));
    // d re-evaluates at dispatch, when b's new fingerprint enters its
    // candidate identity.
    assert!(!dag.is_dirty("d"), "merge node re-evaluates at dispatch");

    // Second run.
    dag.run(&cfg, &ctx, None).await.unwrap();
    assert_eq!(
        ctrs[0].load(std::sync::atomic::Ordering::SeqCst),
        1,
        "a should NOT re-execute"
    );
    assert_eq!(cnt(&ctr_b2), 1, "replaced b should execute once");
    assert_eq!(
        ctrs[2].load(std::sync::atomic::Ordering::SeqCst),
        1,
        "c should NOT re-execute"
    );
    assert_eq!(
        ctrs[3].load(std::sync::atomic::Ordering::SeqCst),
        2,
        "d should re-execute (merge of changed b + reused c)"
    );
}

/// Non-incremental mode ignores dirty marks and always re-runs everything.
#[tokio::test]
async fn non_incremental_ignores_dirty_marks() {
    let mut dag = DAG::default();
    let ctr_a = Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let ctr_b = Arc::new(std::sync::atomic::AtomicUsize::new(0));

    dag.add_node("a".into(), Box::new(CountingEcho::new(ctr_a.clone())))
        .unwrap();
    dag.add_node("b".into(), Box::new(CountingEcho::new(ctr_b.clone())))
        .unwrap();
    dag.add_edge("a", "b", 0, 0).unwrap();

    let ctx = test_ctx();
    let cfg = SchedulerConfig::default(); // incremental = false

    // First run.
    dag.run(&cfg, &ctx, None).await.unwrap();
    assert_eq!(cnt(&ctr_a), 1);
    assert_eq!(cnt(&ctr_b), 1);

    // Second run in non-incremental mode — everything re-executes.
    dag.run(&cfg, &ctx, None).await.unwrap();
    assert_eq!(cnt(&ctr_a), 2, "a should re-execute (non-incremental)");
    assert_eq!(cnt(&ctr_b), 2, "b should re-execute (non-incremental)");
}

#[tokio::test]
async fn restored_file_checkpoint_reuses_only_matching_hash_and_spec() {
    use crate::{
        dag::graph::PortOutputs,
        value::{FileRef, NodeValue},
    };
    use sha2::{Digest, Sha256};
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("checkpoint.txt");
    let mut first = DAG::default();
    first
        .add_node_with_spec(
            "writer".into(),
            Box::new(FileOutputNode::new(path.clone())),
            "file_output".into(),
            serde_json::json!({"v":1}),
        )
        .unwrap();
    let mut cfg = incremental_cfg();
    cfg.input_hashing = crate::dag::InputHashing::ContentRequired;
    first.run(&cfg, &test_ctx(), None).await.unwrap();
    let fingerprint = first.recorded_fingerprint("writer").unwrap().to_string();
    let mut file = FileRef::local(&path, None).unwrap();
    file.fingerprint.as_mut().unwrap().content_hash = Some(format!(
        "{:x}",
        Sha256::digest(std::fs::read(&path).unwrap())
    ));
    let mut outputs = PortOutputs::new();
    outputs.insert_file(0, file);
    drop(first);
    let node = FileOutputNode::new(path.clone());
    let count = node.runs.clone();
    let mut second = DAG::default();
    second
        .add_node_with_spec(
            "writer".into(),
            Box::new(node),
            "file_output".into(),
            serde_json::json!({"v":1}),
        )
        .unwrap();
    second
        .restore_file_checkpoint("writer", &fingerprint, outputs.clone())
        .unwrap();
    assert!(second.run(&cfg, &test_ctx(), None).await.unwrap().ok);
    assert_eq!(
        cnt(&count),
        0,
        "verified persisted output reused in fresh graph"
    );
    std::fs::write(&path, "corrupt bytes").unwrap();
    assert!(second.run(&cfg, &test_ctx(), None).await.unwrap().ok);
    assert_eq!(cnt(&count), 1, "changed output recomputed");
    let mut changed = DAG::default();
    let node = FileOutputNode::new(path.clone());
    let count = node.runs.clone();
    changed
        .add_node_with_spec(
            "writer".into(),
            Box::new(node),
            "file_output".into(),
            serde_json::json!({"v":2}),
        )
        .unwrap();
    // Restore the matching current bytes with the old execution fingerprint.
    let mut file = FileRef::local(&path, None).unwrap();
    file.fingerprint.as_mut().unwrap().content_hash = Some(format!(
        "{:x}",
        Sha256::digest(std::fs::read(&path).unwrap())
    ));
    let mut outputs = PortOutputs::new();
    outputs.insert_file(0, file);
    changed
        .restore_file_checkpoint("writer", &fingerprint, outputs)
        .unwrap();
    assert!(changed.run(&cfg, &test_ctx(), None).await.unwrap().ok);
    assert_eq!(cnt(&count), 1, "changed spec cannot reuse old fingerprint");
    let mut untyped = PortOutputs::new();
    untyped.insert(0, NodeValue::File(FileRef::local(&path, None).unwrap()));
    assert!(
        changed
            .restore_file_checkpoint("writer", &fingerprint, untyped)
            .is_err()
    );
}

#[tokio::test]
async fn completion_publication_failure_blocks_descendants_and_cache_identity() {
    let mut dag = DAG::default();
    let a = Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let b = Arc::new(std::sync::atomic::AtomicUsize::new(0));
    dag.add_node("a".into(), Box::new(CountingEcho::new(a.clone())))
        .unwrap();
    dag.add_node("b".into(), Box::new(CountingEcho::new(b.clone())))
        .unwrap();
    dag.add_edge("a", "b", 0, 0).unwrap();
    let mut cfg = incremental_cfg();
    cfg.completion_publisher = Some(Arc::new(|_, _, _, details| {
        let details = details.expect("successful task details must precede checkpoint publication");
        assert!(details.task_manifest.is_some());
        assert_eq!(details.exit_code, Some(0));
        Err(crate::dag::DagError::Schedule(
            "SYNTHETIC checkpoint ledger write failure".into(),
        ))
    }));
    assert!(!dag.run(&cfg, &test_ctx(), None).await.unwrap().ok);
    assert_eq!(cnt(&a), 1);
    assert_eq!(cnt(&b), 0);
    assert_eq!(dag.status("a"), Some(RuntimeStatus::Failed));
    assert_eq!(dag.status("b"), Some(RuntimeStatus::Skipped));
    assert!(dag.recorded_fingerprint("a").is_none());
    cfg.completion_publisher = None;
    assert!(dag.run(&cfg, &test_ctx(), None).await.unwrap().ok);
    assert_eq!(cnt(&a), 2);
    assert_eq!(cnt(&b), 1);
}

#[tokio::test]
async fn runtime_projection_rejects_incremental_before_any_node_dispatch() {
    let root = tempfile::tempdir().unwrap();
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(root.path(), std::fs::Permissions::from_mode(0o700)).unwrap();
    }
    let run_id = "ba4fbb5f-c9c1-4a2a-9a11-1bfe0377d5a1";
    let projection = crate::runtime_output::RuntimeOutputProjection::new(
        run_id.into(),
        root.path().into(),
        std::collections::BTreeMap::from([(
            root.path().join("logical"),
            root.path().join(run_id).join("physical"),
        )]),
    )
    .unwrap()
    .initialize_run_directory()
    .unwrap();
    let mut ctx = test_ctx();
    ctx.runtime_output_projection = Some(Arc::new(projection));
    let counter = Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let mut dag = DAG::default();
    dag.add_node("node".into(), Box::new(CountingEcho::new(counter.clone())))
        .unwrap();
    let error = dag.run(&incremental_cfg(), &ctx, None).await.unwrap_err();
    assert!(error.to_string().contains("incremental reuse is disabled"));
    assert_eq!(cnt(&counter), 0);
    assert!(dag.output("node").is_none());
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn checkpoint_validation_error_cancels_and_joins_started_task_before_return() {
    use crate::dag::node_event::NodeReporter;
    use crate::dag::{DagError, DagNode, NodeInput, NodePorts};
    use crate::value::{FileRef, PortType};
    use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
    use std::sync::{Condvar, Mutex};
    use std::time::Duration;
    use tokio::sync::{Notify, oneshot};

    struct DropProof {
        entered: Arc<Notify>,
        release: Arc<(Mutex<bool>, Condvar)>,
        finished: Arc<AtomicBool>,
    }
    impl Drop for DropProof {
        fn drop(&mut self) {
            self.entered.notify_one();
            // Hold destruction open so returning before join is observable,
            // rather than a race with an immediately completing destructor.
            let (lock, wake) = &*self.release;
            let (released, _) = wake
                .wait_timeout_while(lock.lock().unwrap(), Duration::from_secs(10), |v| !*v)
                .unwrap();
            self.finished.store(*released, Ordering::SeqCst);
        }
    }
    #[derive(Clone)]
    struct SlowNode {
        ports: NodePorts,
        started: Arc<Notify>,
        drop_entered: Arc<Notify>,
        release_drop: Arc<(Mutex<bool>, Condvar)>,
        dropped: Arc<AtomicBool>,
    }
    #[async_trait::async_trait]
    impl DagNode for SlowNode {
        fn ports(&self) -> &NodePorts {
            &self.ports
        }
        fn kind(&self) -> &'static str {
            "checkpoint_cancel_join_probe"
        }
        fn clone_box(&self) -> Box<dyn DagNode> {
            Box::new(self.clone())
        }
        fn as_any(&self) -> &dyn std::any::Any {
            self
        }
        async fn execute(
            &mut self,
            _: &crate::registry::NodeCtx,
            _: &[NodeInput],
            _: &NodeReporter,
        ) -> Result<crate::dag::graph::PortOutputs, DagError> {
            let _proof = DropProof {
                entered: self.drop_entered.clone(),
                release: self.release_drop.clone(),
                finished: self.dropped.clone(),
            };
            self.started.notify_one();
            std::future::pending().await
        }
    }
    #[derive(Clone)]
    struct GateNode {
        ports: NodePorts,
        path: std::path::PathBuf,
        wait_for_slow: Arc<AtomicBool>,
        started: Arc<Notify>,
    }
    #[async_trait::async_trait]
    impl DagNode for GateNode {
        fn ports(&self) -> &NodePorts {
            &self.ports
        }
        fn kind(&self) -> &'static str {
            "checkpoint_started_gate"
        }
        fn clone_box(&self) -> Box<dyn DagNode> {
            Box::new(self.clone())
        }
        fn as_any(&self) -> &dyn std::any::Any {
            self
        }
        fn allows_cache_reuse(&self) -> bool {
            false
        }
        async fn execute(
            &mut self,
            _: &crate::registry::NodeCtx,
            _: &[NodeInput],
            _: &NodeReporter,
        ) -> Result<crate::dag::graph::PortOutputs, DagError> {
            if self.wait_for_slow.load(Ordering::SeqCst) {
                self.started.notified().await;
            }
            let mut outputs = crate::dag::graph::PortOutputs::new();
            outputs.insert_file(0, FileRef::local(&self.path, None)?);
            Ok(outputs)
        }
    }
    #[derive(Clone)]
    struct CachedNode {
        ports: NodePorts,
        writer: FileOutputNode,
    }
    #[async_trait::async_trait]
    impl DagNode for CachedNode {
        fn ports(&self) -> &NodePorts {
            &self.ports
        }
        fn kind(&self) -> &'static str {
            "checkpoint_cached_target"
        }
        fn clone_box(&self) -> Box<dyn DagNode> {
            Box::new(self.clone())
        }
        fn as_any(&self) -> &dyn std::any::Any {
            self
        }
        async fn execute(
            &mut self,
            ctx: &crate::registry::NodeCtx,
            inputs: &[NodeInput],
            reporter: &NodeReporter,
        ) -> Result<crate::dag::graph::PortOutputs, DagError> {
            self.writer.execute(ctx, inputs, reporter).await
        }
    }
    let root = tempfile::tempdir().unwrap();
    let input = root.path().join("gate.txt");
    std::fs::write(&input, b"SYN-UNCHANGED-CACHE-INPUT").unwrap();
    let started = Arc::new(Notify::new());
    let wait_for_slow = Arc::new(AtomicBool::new(false));
    let drop_entered = Arc::new(Notify::new());
    let release_drop = Arc::new((Mutex::new(false), Condvar::new()));
    let dropped = Arc::new(AtomicBool::new(false));
    let writer = FileOutputNode::new(root.path().join("cached.txt"));
    let calls = writer.runs.clone();
    let mut dag = DAG::default();
    dag.set_task_executor(Arc::new(RecordingTaskExecutor::with_submissions(Arc::new(
        Mutex::new(Vec::new()),
    ))));
    dag.add_node(
        "gate".into(),
        Box::new(GateNode {
            ports: NodePorts::new().add_output_port_of_type(None, PortType::File),
            path: input,
            wait_for_slow: wait_for_slow.clone(),
            started: started.clone(),
        }),
    )
    .unwrap();
    dag.add_node(
        "cached".into(),
        Box::new(CachedNode {
            ports: NodePorts::new()
                .add_input_port_of_type(None, PortType::File)
                .add_output_port_of_type(None, PortType::File),
            writer,
        }),
    )
    .unwrap();
    dag.add_edge("gate", "cached", 0, 0).unwrap();
    let mut cfg = incremental_cfg();
    cfg.max_concurrency = 2;
    cfg.input_hashing = crate::dag::InputHashing::ContentRequired;
    assert!(dag.run(&cfg, &test_ctx(), None).await.unwrap().ok);
    assert_eq!(calls.load(Ordering::SeqCst), 1);
    wait_for_slow.store(true, Ordering::SeqCst);
    dag.add_node(
        "slow".into(),
        Box::new(SlowNode {
            ports: NodePorts::new(),
            started,
            drop_entered: drop_entered.clone(),
            release_drop: release_drop.clone(),
            dropped: dropped.clone(),
        }),
    )
    .unwrap();
    let validations = Arc::new(AtomicUsize::new(0));
    let observed = validations.clone();
    cfg.checkpoint_reuse_validator = Some(Arc::new(move |id, _, _| {
        assert_eq!(id, "cached");
        observed.fetch_add(1, Ordering::SeqCst);
        Err(DagError::Schedule(
            "SYN EXACT CHECKPOINT VALIDATION FAILURE".into(),
        ))
    }));
    let (done, mut result) = oneshot::channel();
    let task = tokio::spawn(async move {
        let outcome = dag.run(&cfg, &test_ctx(), None).await;
        let _ = done.send(outcome);
        dag
    });
    tokio::time::timeout(Duration::from_secs(3), drop_entered.notified())
        .await
        .unwrap();
    // Exceed the normal scheduler's 2s abort grace: its timeout must not
    // detach this fatal path and release the caller's execution lease.
    let early = tokio::time::timeout(Duration::from_millis(2200), &mut result).await;
    let returned_before_drop_finished = early.is_ok();
    {
        let (lock, wake) = &*release_drop;
        *lock.lock().unwrap() = true;
        wake.notify_all();
    }
    let outcome = match early {
        Ok(outcome) => outcome.unwrap(),
        Err(_) => tokio::time::timeout(Duration::from_secs(3), &mut result)
            .await
            .unwrap()
            .unwrap(),
    };
    let dag = task.await.unwrap();
    assert!(
        !returned_before_drop_finished,
        "scheduler returned while an aborted task was still being destroyed"
    );
    assert!(
        dropped.load(Ordering::SeqCst),
        "started task must finish destruction before return"
    );
    assert_eq!(
        validations.load(Ordering::SeqCst),
        1,
        "actual cache validator branch reached"
    );
    assert_eq!(
        calls.load(Ordering::SeqCst),
        1,
        "validator Err must not become a cache miss"
    );
    assert!(
        matches!(outcome, Err(DagError::Schedule(message)) if message == "SYN EXACT CHECKPOINT VALIDATION FAILURE")
    );
    assert_eq!(dag.status("cached"), Some(RuntimeStatus::Failed));
    assert_eq!(dag.status("slow"), Some(RuntimeStatus::Cancelled));
    assert!(dag.output("cached").is_none());
    assert!(dag.recorded_fingerprint("slow").is_none());
}
