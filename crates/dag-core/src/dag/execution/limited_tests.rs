use super::*;
use crate::dag::{graph::PortOutputs, node_event::NodeReporter};
use crate::{DagNode, NodeCtx, NodePorts, TaskResources, TaskSpec, TaskSubmission};
use datafusion::prelude::SessionContext;
use std::sync::atomic::{AtomicUsize, Ordering};
use tokio_util::sync::CancellationToken;

#[derive(Clone, Default)]
struct UnusedNode(NodePorts);
#[async_trait::async_trait]
impl DagNode for UnusedNode {
    fn ports(&self) -> &NodePorts {
        &self.0
    }
    async fn execute(
        &mut self,
        _: &NodeCtx,
        _: &[crate::NodeInput],
        _: &NodeReporter,
    ) -> Result<PortOutputs, DagError> {
        panic!("the throttle must never execute a local node")
    }
    fn clone_box(&self) -> Box<dyn DagNode> {
        Box::new(self.clone())
    }
    fn kind(&self) -> &'static str {
        "unused"
    }
    fn as_any(&self) -> &dyn std::any::Any {
        self
    }
}

fn execution(mode: &str) -> TaskExecution {
    TaskExecution {
        submission: TaskSubmission {
            spec: TaskSpec {
                id: mode.into(),
                logical_node: None,
                kind: "remote-toy".into(),
                spec: serde_json::json!({}),
                axis: None,
                item_key: None,
                item: None,
                inputs: vec![],
                outputs: vec![],
            },
            inputs: vec![],
            resources: TaskResources::default(),
        },
        node: Box::<UnusedNode>::default(),
        engine_ctx: Arc::new(NodeCtx::new(SessionContext::new().runtime_env(), None)),
        reporter: NodeReporter::noop(),
        cancellation: CancellationToken::new(),
    }
}

struct Probe {
    calls: AtomicUsize,
    active: AtomicUsize,
    peak: AtomicUsize,
    release: Semaphore,
    cancelled: AtomicUsize,
}
impl Default for Probe {
    fn default() -> Self {
        Self {
            calls: AtomicUsize::new(0),
            active: AtomicUsize::new(0),
            peak: AtomicUsize::new(0),
            release: Semaphore::new(0),
            cancelled: AtomicUsize::new(0),
        }
    }
}
struct Active<'a>(&'a AtomicUsize);
impl Drop for Active<'_> {
    fn drop(&mut self) {
        self.0.fetch_sub(1, Ordering::SeqCst);
    }
}
#[async_trait::async_trait]
impl TaskExecutor for Probe {
    fn name(&self) -> &'static str {
        "remote-probe"
    }
    async fn run(&self, execution: TaskExecution) -> JobResult {
        let active = self.active.fetch_add(1, Ordering::SeqCst) + 1;
        let _guard = Active(&self.active);
        self.peak.fetch_max(active, Ordering::SeqCst);
        self.calls.fetch_add(1, Ordering::SeqCst);
        match execution.submission.spec.id.as_str() {
            "panic" => panic!("toy remote executor panic"),
            "error" => (),
            "cancel-cleanup" => {
                execution.cancellation.cancelled().await;
                self.cancelled.fetch_add(1, Ordering::SeqCst);
                self.release.acquire().await.unwrap().forget();
            }
            _ => self.release.acquire().await.unwrap().forget(),
        }
        JobResult::Failed {
            id: execution.submission.spec.id,
            error: DagError::Schedule("toy terminal failure".into()),
            duration: Duration::ZERO,
            details: None,
        }
    }
}

async fn eventually(predicate: impl Fn() -> bool) {
    tokio::time::timeout(Duration::from_secs(5), async {
        while !predicate() {
            tokio::task::yield_now().await;
        }
    })
    .await
    .expect("toy executor did not reach expected state");
}

#[tokio::test]
async fn cloned_remote_policy_shares_four_slots_and_releases_terminal_errors() {
    let probe = Arc::new(Probe::default());
    let limiter = Arc::new(LimitedTaskExecutor::new(probe.clone(), 4).unwrap());
    let mut jobs = vec![];
    // Independent callers share the same Arc, as per-agent engines do.
    for _ in 0..12 {
        let policy = limiter.clone();
        jobs.push(tokio::spawn(
            async move { policy.run(execution("wait")).await },
        ));
    }
    eventually(|| probe.calls.load(Ordering::SeqCst) == 4).await;
    assert_eq!(probe.active.load(Ordering::SeqCst), 4);
    assert!(probe.peak.load(Ordering::SeqCst) > 2);
    probe.release.add_permits(12);
    for job in jobs {
        assert!(matches!(job.await.unwrap(), JobResult::Failed { .. }));
    }
    assert_eq!(probe.calls.load(Ordering::SeqCst), 12);
    assert_eq!(probe.peak.load(Ordering::SeqCst), 4);
    assert_eq!(probe.active.load(Ordering::SeqCst), 0);
    assert!(matches!(
        limiter.run(execution("error")).await,
        JobResult::Failed { .. }
    ));
}

#[tokio::test]
async fn cancellation_waits_for_active_cleanup_and_never_submits_cancelled_waiters() {
    let probe = Arc::new(Probe::default());
    let limiter = Arc::new(LimitedTaskExecutor::new(probe.clone(), 1).unwrap());
    let active = execution("cancel-cleanup");
    let active_cancel = active.cancellation.clone();
    let policy = limiter.clone();
    let active_job = tokio::spawn(async move { policy.run(active).await });
    eventually(|| probe.calls.load(Ordering::SeqCst) == 1).await;
    let waiting = execution("wait");
    let waiting_cancel = waiting.cancellation.clone();
    let policy = limiter.clone();
    let queued = tokio::spawn(async move { policy.run(waiting).await });
    waiting_cancel.cancel();
    assert!(matches!(queued.await.unwrap(), JobResult::Failed { .. }));
    assert_eq!(probe.calls.load(Ordering::SeqCst), 1);
    active_cancel.cancel();
    eventually(|| probe.cancelled.load(Ordering::SeqCst) == 1).await;
    assert_eq!(
        limiter.permits.available_permits(),
        0,
        "active cancellation must retain its slot until cleanup resolves"
    );
    probe.release.add_permits(1);
    active_job.await.unwrap();
    assert_eq!(limiter.permits.available_permits(), 1);
    let pre_cancelled = execution("wait");
    pre_cancelled.cancellation.cancel();
    limiter.run(pre_cancelled).await;
    assert_eq!(probe.calls.load(Ordering::SeqCst), 1);
}

#[tokio::test]
async fn panic_and_aborted_futures_release_coordinator_permits() {
    let probe = Arc::new(Probe::default());
    let limiter = Arc::new(LimitedTaskExecutor::new(probe.clone(), 1).unwrap());
    let policy = limiter.clone();
    assert!(
        tokio::spawn(async move { policy.run(execution("panic")).await })
            .await
            .unwrap_err()
            .is_panic()
    );
    assert_eq!(limiter.permits.available_permits(), 1);
    let policy = limiter.clone();
    let task = tokio::spawn(async move { policy.run(execution("wait")).await });
    eventually(|| probe.calls.load(Ordering::SeqCst) == 2).await;
    task.abort();
    assert!(task.await.unwrap_err().is_cancelled());
    assert_eq!(limiter.permits.available_permits(), 1);
    assert_eq!(probe.active.load(Ordering::SeqCst), 0);
    limiter.run(execution("error")).await;
    assert!(LimitedTaskExecutor::new(probe.clone(), 0).is_err());
    assert!(LimitedTaskExecutor::new(probe, MAX_REMOTE_TASKS + 1).is_err());
}
