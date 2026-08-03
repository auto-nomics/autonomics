//! Quick dev tool: create a test history DB with 3 snapshots + a branch.
//! Usage: cargo run -p data-engine --example seed_history -- /tmp/test.db

use data_engine::dag::history::{DagHistory, DagManifest, EdgeEntry, NodeEntry};

#[tokio::main]
async fn main() {
    let db_path = std::env::args()
        .nth(1)
        .unwrap_or_else(|| "/tmp/dag_history_test.db".to_string());
    let _ = std::fs::remove_file(&db_path);
    let history = DagHistory::open(&db_path).await.unwrap();

    // v1: single source node
    let m1 = DagManifest {
        nodes: vec![NodeEntry {
            id: "src".into(),
            kind: "source_file".into(),
            spec: serde_json::json!({"path": "/data/iris.csv"}),
        }],
        edges: vec![],
    };
    let id1 = history
        .commit("main", &m1, None::<&serde_json::Value>, "初始：加载数据")
        .await
        .unwrap();

    // v2: add sql transform + edge
    let m2 = DagManifest {
        nodes: vec![
            NodeEntry {
                id: "src".into(),
                kind: "source_file".into(),
                spec: serde_json::json!({"path": "/data/iris.csv"}),
            },
            NodeEntry {
                id: "agg".into(),
                kind: "sql".into(),
                spec: serde_json::json!({"sql_query": "SELECT region, AVG(charges) FROM port_0 GROUP BY region"}),
            },
        ],
        edges: vec![EdgeEntry {
            from: "src".into(),
            from_port: 0,
            to: "agg".into(),
            to_port: 0,
        }],
    };
    let id2 = history
        .commit("main", &m2, None::<&serde_json::Value>, "增加 SQL 聚合节点")
        .await
        .unwrap();

    // v3: add sink
    let m3 = DagManifest {
        nodes: vec![
            NodeEntry {
                id: "src".into(),
                kind: "source_file".into(),
                spec: serde_json::json!({"path": "/data/iris.csv"}),
            },
            NodeEntry {
                id: "agg".into(),
                kind: "sql".into(),
                spec: serde_json::json!({"sql_query": "SELECT region, AVG(charges) FROM port_0 GROUP BY region"}),
            },
            NodeEntry {
                id: "out".into(),
                kind: "sink_file".into(),
                spec: serde_json::json!({"path": "/tmp/out.csv", "format": "csv"}),
            },
        ],
        edges: vec![
            EdgeEntry {
                from: "src".into(),
                from_port: 0,
                to: "agg".into(),
                to_port: 0,
            },
            EdgeEntry {
                from: "agg".into(),
                from_port: 0,
                to: "out".into(),
                to_port: 0,
            },
        ],
    };
    let id3 = history
        .commit("main", &m3, None::<&serde_json::Value>, "增加文件输出节点")
        .await
        .unwrap();

    // branch experiment from current head
    history.branch("experiment", "main").await.unwrap();

    // reset main back to v1 (for restore test)
    history.reset("main", &id1).await.unwrap();

    eprintln!("DB created: {db_path}");
    eprintln!("  v1 = {id1}");
    eprintln!("  v2 = {id2}");
    eprintln!("  v3 = {id3}");
    eprintln!("  main now points to v1, experiment points to v3");
}
