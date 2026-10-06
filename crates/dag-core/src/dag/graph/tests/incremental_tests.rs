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
