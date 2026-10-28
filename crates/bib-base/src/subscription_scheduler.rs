//! Host-owned, bounded subscription scheduling. The background task holds no
//! sender/handle to itself: releasing all handles closes its command channel,
//! cancels active requests and drains their durable settlement.
use crate::{
    subscription_execution::{SubscriptionExecutor, now_ms},
    subscriptions::*,
};
use serde::Serialize;
use std::{collections::HashMap, time::Duration};
use tokio::{
    sync::{mpsc, oneshot, watch},
    task::JoinSet,
};
#[derive(Debug, Clone, Serialize)]
pub struct SubscriptionSchedulerStatus {
    pub running: bool,
    pub active: usize,
    pub last_error: Option<String>,
}
#[derive(Clone)]
pub struct SubscriptionScheduler {
    commands: mpsc::Sender<Command>,
    status: watch::Receiver<SubscriptionSchedulerStatus>,
}
enum Command {
    Check {
        rule: String,
        id: String,
        reply: oneshot::Sender<Result<SubscriptionRun>>,
    },
    Cancel {
        id: String,
        reply: oneshot::Sender<Result<SubscriptionRun>>,
    },
    Shutdown(oneshot::Sender<()>),
}
impl SubscriptionScheduler {
    pub fn start(executor: SubscriptionExecutor, concurrency: usize) -> Result<Self> {
        if !(1..=8).contains(&concurrency) {
            return Err(invalid("scheduler concurrency must be 1..8"));
        }
        let runtime = tokio::runtime::Handle::try_current()
            .map_err(|_| invalid("scheduler requires an active async runtime"))?;
        let (commands, receiver) = mpsc::channel(64);
        let (status, observations) = watch::channel(SubscriptionSchedulerStatus {
            running: true,
            active: 0,
            last_error: None,
        });
        runtime.spawn(serve(executor, concurrency, receiver, status));
        Ok(Self {
            commands,
            status: observations,
        })
    }
    pub fn status(&self) -> SubscriptionSchedulerStatus {
        self.status.borrow().clone()
    }
    pub fn observe(&self) -> watch::Receiver<SubscriptionSchedulerStatus> {
        self.status.clone()
    }
    pub async fn check_now(&self, rule: &str, id: &str) -> Result<SubscriptionRun> {
        let (reply, response) = oneshot::channel();
        self.commands
            .send(Command::Check {
                rule: rule.into(),
                id: id.into(),
                reply,
            })
            .await
            .map_err(|_| conflict("scheduler stopped"))?;
        response
            .await
            .map_err(|_| conflict("scheduler stopped before acknowledging check"))?
    }
    pub async fn cancel(&self, id: &str) -> Result<SubscriptionRun> {
        let (reply, response) = oneshot::channel();
        self.commands
            .send(Command::Cancel {
                id: id.into(),
                reply,
            })
            .await
            .map_err(|_| conflict("scheduler stopped"))?;
        response
            .await
            .map_err(|_| conflict("scheduler stopped before acknowledging cancellation"))?
    }
    /// Host shutdown, shared by every clone. Completion waits for cancellation
    /// to settle; dropping all handles triggers the same cleanup automatically.
    pub async fn shutdown(&self) {
        let (reply, response) = oneshot::channel();
        if self.commands.send(Command::Shutdown(reply)).await.is_ok() {
            let _ = response.await;
        }
    }
}
fn spawn(
    tasks: &mut JoinSet<(String, Result<SubscriptionRun>)>,
    active: &mut HashMap<String, watch::Sender<bool>>,
    executor: &SubscriptionExecutor,
    lease: SubscriptionLease,
    id: String,
) {
    let (cancel, receiver) = watch::channel(false);
    active.insert(id.clone(), cancel);
    let executor = executor.clone();
    tasks.spawn(async move { (id, executor.execute(lease, receiver).await) });
}
fn report_error(
    status: &watch::Sender<SubscriptionSchedulerStatus>,
    error: impl std::fmt::Display,
) {
    let mut value = status.borrow().clone();
    let mut message = error.to_string();
    if message.len() > 512 {
        let mut end = 512;
        while !message.is_char_boundary(end) {
            end -= 1;
        }
        message.truncate(end);
    }
    value.last_error = Some(message);
    status.send_replace(value);
}
async fn serve(
    executor: SubscriptionExecutor,
    concurrency: usize,
    mut commands: mpsc::Receiver<Command>,
    status: watch::Sender<SubscriptionSchedulerStatus>,
) {
    let mut tasks: JoinSet<(String, Result<SubscriptionRun>)> = JoinSet::new();
    let mut active: HashMap<String, watch::Sender<bool>> = HashMap::new();
    let mut tick = tokio::time::interval(Duration::from_secs(1));
    tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
    let mut stopped = None;
    loop {
        tokio::select! {
            biased;
            outcome=tasks.join_next(),if !tasks.is_empty()=>{
                match outcome {
                    Some(Ok((id,result)))=>{
                        active.remove(&id);
                        if let Err(error)=result {report_error(&status,error);}
                    }
                    Some(Err(error))=>{
                        // A panicked coordinator must not retain a source's
                        // cancellation sender indefinitely. Other workers are
                        // drained before this service exits; leases recover on
                        // restart if durable settlement itself failed.
                        report_error(&status,error);break;
                    }
                    None=>{}
                }
            }
            command=commands.recv()=>{
                match command {
                    None=>break,
                    Some(Command::Shutdown(reply))=>{stopped=Some(reply);break;}
                    Some(Command::Cancel{id,reply})=>{
                        let result=executor.store.cancel_run(&id,now_ms()).await;
                        if result.is_ok() {
                            if let Some(cancel)=active.get(&id){let _=cancel.send(true);}
                        }
                        let _=reply.send(result);
                    }
                    Some(Command::Check{rule,id,reply})=>{
                        // Retained retries work even while all worker slots are
                        // occupied. They cannot acquire or restart an old lease.
                        let result=match executor.store.run(&id).await {
                            Ok(run)=>if run.rule_id==rule {Ok(run)} else {Err(conflict("run UUID belongs to another rule"))},
                            Err(SubscriptionError::NotFound)=>{
                                if tasks.len()>=concurrency {Err(conflict("subscription workers are busy"))}
                                else {
                                    match executor.start(&rule,&id,true).await {
                                        Ok((lease,run))=>{
                                            if let Some(lease)=lease {spawn(&mut tasks,&mut active,&executor,lease,run.id.clone());}
                                            Ok(run)
                                        }
                                        Err(error)=>Err(error),
                                    }
                                }
                            }
                            Err(error)=>Err(error),
                        };
                        let _=reply.send(result);
                    }
                }
            }
            _=tick.tick(),if tasks.len()<concurrency=>{
                let slots=(concurrency-tasks.len()) as u16;
                match executor.store.due(now_ms(),slots).await {
                    Ok(ids)=>for rule in ids {
                        let id=uuid::Uuid::new_v4().to_string();
                        match executor.start(&rule,&id,false).await {
                            Ok((Some(lease),run))=>spawn(&mut tasks,&mut active,&executor,lease,run.id),
                            Ok((None,_))=>{},
                            Err(SubscriptionError::Conflict(_))=>{}, // another process won the claim
                            Err(error)=>report_error(&status,error),
                        }
                    },
                    Err(error)=>report_error(&status,error),
                }
            }
        }
        let mut value = status.borrow().clone();
        value.active = tasks.len();
        status.send_replace(value);
    }
    commands.close();
    for cancel in active.values() {
        let _ = cancel.send(true);
    }
    while let Some(outcome) = tasks.join_next().await {
        match outcome {
            Ok((_, Err(error))) => report_error(&status, error),
            Err(error) => report_error(&status, error),
            _ => {}
        }
    }
    let mut value = status.borrow().clone();
    value.running = false;
    value.active = 0;
    status.send_replace(value);
    if let Some(reply) = stopped {
        let _ = reply.send(());
    }
}
