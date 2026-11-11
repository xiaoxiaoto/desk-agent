//! SQLite-backed instruction queue in front of the Runtime capability invoker.
//!
//! Every queued request is executed through the same `invoke_task` path, in FIFO order, by a
//! single worker. When the consumed head is `send_message_by_nickname`, pending requests for the
//! exact same app and nickname enqueued within `[T, T + merge_window]` (T = head enqueue time)
//! are merged immediately: their `messages` are concatenated in enqueue order into one exact
//! instruction whose inter-message delay is the configured `merged_interval_ms` (an unmerged
//! request keeps its own `interval_ms`). Nothing waits for the window to elapse; merging
//! only collapses backlog to reduce RPA latency. Each message is still preserved verbatim and
//! sent once. Merged dispatch is authorized and audited by the normal parent-grant path; queue
//! membership is audited per original request digest. No record is retried automatically.

use super::InvokeError;
use crate::config::loader::QueueConfig;
use crate::infrastructure::database::instruction_queue::{self as store, NewRecord, PendingRecord, QueueStatus};
use crate::kernel::instruction::{ApplicationInstruction, Message, TaskInstruction};
use crate::kernel::task::{valid_send_message_by_nickname, TaskInput, MAX_INSTRUCTIONS};
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use sqlx::SqlitePool;
use std::{collections::HashMap, future::Future, pin::Pin, sync::Arc, time::Duration};
use tokio::sync::{oneshot, Mutex, Notify};

pub type Delivery = Result<(TaskInstruction, Option<Value>), InvokeError>;
pub type Executor = Arc<dyn Fn(String, TaskInput) -> Pin<Box<dyn Future<Output = Delivery> + Send>> + Send + Sync>;

const MERGE_SCOPE: (&str, &str, &str) = ("wecom.message.send_by_nickname", "exact_nickname_conversation", "send_messages");

pub struct InstructionQueue {
    db: SqlitePool,
    config: QueueConfig,
    waiters: Mutex<HashMap<i64, oneshot::Sender<Delivery>>>,
    notify: Notify,
    executor: Executor,
}

fn now_ms() -> i64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_millis() as i64).unwrap_or(0)
}

fn nickname_send(input: &TaskInput) -> Option<(&str, &str, &[Message], u64)> {
    match input {
        TaskInput::ExactInstruction { app, steps } => match steps.as_slice() {
            [TaskInstruction::Application(ApplicationInstruction::SendMessageByNickname { nickname, messages, interval_ms })] => {
                Some((app.as_str(), nickname.as_str(), messages.as_slice(), *interval_ms))
            }
            _ => None,
        },
        TaskInput::NaturalLanguage { .. } => None,
    }
}

fn action_name(instruction: &TaskInstruction) -> String {
    serde_json::to_value(instruction).ok()
        .and_then(|value| value.get("action").and_then(Value::as_str).map(str::to_owned))
        .unwrap_or_else(|| "unknown".to_owned())
}

/// Same format as the coordinate-cache content key: a digest, never the nickname itself.
fn merge_key(nickname: &str) -> String {
    format!("nickname-v1-sha256:{:x}", Sha256::digest(nickname.as_bytes()))
}

fn request_digest(input: &TaskInput) -> String {
    if let Some((_, nickname, messages, interval)) = nickname_send(input) {
        if let Ok(digest) = super::send_message_by_nickname_parameters(nickname, messages, interval) {
            return digest;
        }
    }
    format!("task-input-v1-sha256:{:x}", Sha256::digest(serde_json::to_vec(input).unwrap_or_default()))
}

struct Member {
    record: PendingRecord,
    input: TaskInput,
    message_offset: usize,
    message_count: usize,
}

impl InstructionQueue {
    /// Marks records left `executing` by a previous process as `unknown`, then starts the worker.
    pub async fn start(db: SqlitePool, config: QueueConfig, executor: Executor) -> Result<Arc<Self>, sqlx::Error> {
        store::initialize(&db).await?;
        let recovered = store::recover(&db, now_ms()).await?;
        if recovered > 0 {
            tracing::warn!(recovered, "instruction queue records left executing were marked unknown and will not be re-dispatched");
        }
        let queue = Arc::new(Self { db, config, waiters: Mutex::new(HashMap::new()), notify: Notify::new(), executor });
        let worker = Arc::clone(&queue);
        tokio::spawn(async move { worker.run().await });
        Ok(queue)
    }

    pub fn config(&self) -> QueueConfig {
        self.config
    }

    /// Enqueues a preflighted request and waits for its own result. Dropping the returned future
    /// does not cancel the queued record or an executing batch.
    pub async fn submit(&self, request_id: &str, input: TaskInput) -> Delivery {
        let instruction = input.resolve_instruction().map_err(|_| InvokeError::InvalidInput)?;
        let payload = serde_json::to_string(&input).map_err(|_| InvokeError::InvalidInput)?;
        let key = nickname_send(&input).map(|(_, nickname, _, _)| merge_key(nickname));
        let digest = request_digest(&input);
        let action = action_name(&instruction);
        let (sender, receiver) = oneshot::channel();
        {
            // Hold the waiter map across the insert so the worker cannot finish this record
            // before its waiter is registered.
            let mut waiters = self.waiters.lock().await;
            let record = NewRecord {
                request_id,
                app: input.app(),
                action: &action,
                merge_key: key.as_deref(),
                request_digest: &digest,
                payload: &payload,
                enqueued_at_ms: now_ms(),
            };
            let id = match store::enqueue(&self.db, &record, self.config.max_pending).await {
                Ok(id) => id,
                Err(store::EnqueueError::Full) => return Err(InvokeError::RateLimited),
                Err(store::EnqueueError::Storage(_)) => return Err(InvokeError::AuditUnavailable),
            };
            waiters.insert(id, sender);
        }
        self.notify.notify_one();
        receiver.await.unwrap_or(Err(InvokeError::OutcomeUnknown))
    }

    async fn run(self: Arc<Self>) {
        let mut last_purge = 0;
        loop {
            match self.next_batch().await {
                Ok(Some(batch)) => self.execute(batch).await,
                Ok(None) => {
                    let now = now_ms();
                    if now - last_purge > 60_000 {
                        last_purge = now;
                        let cutoff = now.saturating_sub(self.config.retention_secs.saturating_mul(1000) as i64);
                        if store::purge_terminal(&self.db, cutoff).await.is_err() {
                            tracing::warn!("instruction queue retention purge failed");
                        }
                    }
                    self.notify.notified().await;
                }
                Err(_) => {
                    tracing::warn!("instruction queue storage read failed; retrying claim later");
                    tokio::time::sleep(Duration::from_secs(1)).await;
                }
            }
        }
    }

    /// Selects and durably claims the next batch: the FIFO head plus, for an exact nickname send,
    /// every directly following mergeable record that keeps the merged instruction valid.
    async fn next_batch(&self) -> Result<Option<Vec<Member>>, sqlx::Error> {
        loop {
            let Some(head) = store::head(&self.db).await? else { return Ok(None) };
            let Some(input) = head.payload.as_deref().and_then(|p| serde_json::from_str::<TaskInput>(p).ok()) else {
                store::finish(&self.db, head.id, QueueStatus::Failed, now_ms()).await?;
                self.deliver(head.id, Err(InvokeError::InvalidInput)).await;
                continue;
            };
            let head_count = nickname_send(&input).map_or(0, |(_, _, messages, _)| messages.len());
            let mut members = vec![Member { record: head.clone(), input, message_offset: 0, message_count: head_count }];
            if let Some((app, nickname, messages, _)) = nickname_send(&members[0].input) {
                let interval = self.config.merged_interval_ms;
                let (app, nickname) = (app.to_owned(), nickname.to_owned());
                let mut merged: Vec<Message> = messages.to_vec();
                let window_ms = self.config.merge_window_secs.saturating_mul(1000) as i64;
                for candidate in store::merge_candidates(&self.db, &head, window_ms, MAX_INSTRUCTIONS as i64).await? {
                    let Some(input) = candidate.payload.as_deref().and_then(|p| serde_json::from_str::<TaskInput>(p).ok()) else { break };
                    // Stop at the first record that cannot join, so per-nickname order is kept.
                    let Some((candidate_app, candidate_nickname, candidate_messages, _)) = nickname_send(&input) else { break };
                    if candidate_app != app || candidate_nickname != nickname {
                        break;
                    }
                    let mut next = merged.clone();
                    next.extend_from_slice(candidate_messages);
                    if !valid_send_message_by_nickname(&nickname, &next, interval) {
                        break;
                    }
                    let count = candidate_messages.len();
                    members.push(Member { record: candidate, input, message_offset: merged.len(), message_count: count });
                    merged = next;
                }
            }
            let batch_id = uuid::Uuid::new_v4().to_string();
            let claims: Vec<(i64, i64)> = members.iter().map(|m| (m.record.id, m.message_offset as i64)).collect();
            if store::claim(&self.db, &batch_id, &claims, now_ms()).await? {
                return Ok(Some(members));
            }
        }
    }

    async fn deliver(&self, id: i64, delivery: Delivery) {
        if let Some(sender) = self.waiters.lock().await.remove(&id) {
            let _ = sender.send(delivery);
        }
    }

    async fn finish(&self, id: i64, status: QueueStatus) {
        if store::finish(&self.db, id, status, now_ms()).await.is_err() {
            // The record stays `executing` and becomes `unknown` at the next start.
            tracing::warn!(status = status.as_str(), "instruction queue terminal state could not be stored");
        }
    }

    async fn execute(&self, members: Vec<Member>) {
        if members.len() == 1 && nickname_send(&members[0].input).is_none() {
            let member = members.into_iter().next().expect("one member");
            let result = (self.executor)(member.record.request_id.clone(), member.input).await;
            let status = match &result {
                Ok(_) => QueueStatus::Succeeded,
                Err(InvokeError::OutcomeUnknown) => QueueStatus::Unknown,
                Err(_) => QueueStatus::Failed,
            };
            self.finish(member.record.id, status).await;
            self.deliver(member.record.id, result).await;
            return;
        }

        let head = &members[0];
        let (app, nickname, _, own_interval) = nickname_send(&head.input).expect("nickname batch head");
        let interval = if members.len() > 1 { self.config.merged_interval_ms } else { own_interval };
        let merged: Vec<Message> = members.iter()
            .flat_map(|m| nickname_send(&m.input).map(|(_, _, messages, _)| messages.to_vec()).unwrap_or_default())
            .collect();
        let merged_input = TaskInput::ExactInstruction {
            app: app.to_owned(),
            steps: vec![TaskInstruction::Application(ApplicationInstruction::SendMessageByNickname {
                nickname: nickname.to_owned(), messages: merged, interval_ms: interval,
            })],
        };
        let batch_request = head.record.request_id.clone();
        let batch_id = uuid::Uuid::new_v4().to_string();

        let mut result = Ok(());
        if members.len() > 1 {
            // Durable merge linkage before dispatch; failure means nothing is dispatched.
            for member in &members {
                if result.is_ok() {
                    result = super::audit_text_input(&self.db, &member.record.request_id, &member.record.request_digest, "queue_merged", MERGE_SCOPE).await;
                }
            }
        }
        let delivery = match result {
            Ok(()) => (self.executor)(batch_request, merged_input).await,
            Err(_) => Err(InvokeError::AuditUnavailable),
        };

        let total = members.len();
        for (position, member) in members.iter().enumerate() {
            let instruction = member.input.resolve_instruction().ok();
            let (status, member_delivery) = match (&delivery, instruction) {
                (Ok((_, Some(output))), Some(instruction)) => {
                    let (status, slice) = split_output(output, member.message_offset, member.message_count, &batch_id, total, position);
                    (status, Ok((instruction, Some(slice))))
                }
                (Ok(_), _) | (Err(InvokeError::OutcomeUnknown), _) => (QueueStatus::Unknown, Err(InvokeError::OutcomeUnknown)),
                (Err(error), _) => (QueueStatus::Failed, Err(*error)),
            };
            self.finish(member.record.id, status).await;
            self.deliver(member.record.id, member_delivery).await;
        }
    }
}

/// Projects one merged nickname-send output onto a member's message range, re-indexed from 0.
fn split_output(output: &Value, offset: usize, count: usize, batch_id: &str, total: usize, position: usize) -> (QueueStatus, Value) {
    let range = offset..offset + count;
    let local = |index: Option<u64>| index.map(|i| i as usize).filter(|i| range.contains(i)).map(|i| i - offset);
    let states: Vec<&str> = output.get("message_states").and_then(Value::as_array)
        .map(|items| items.iter().skip(offset).take(count).map(|s| s.get("state").and_then(Value::as_str).unwrap_or("submission_unknown")).collect())
        .unwrap_or_default();
    let states: Vec<&str> = if states.len() == count { states } else { vec!["submission_unknown"; count] };
    let outcomes: Vec<Value> = output.get("outcomes").and_then(Value::as_array).into_iter().flatten()
        .filter_map(|o| local(o.get("index").and_then(Value::as_u64)).map(|i| {
            let mut o = o.clone();
            o["index"] = json!(i);
            o
        }))
        .collect();
    let batch_completed = output.get("outcome").and_then(Value::as_str) == Some("completed");
    let all_observed = states.iter().all(|s| *s == "observed");
    let stop_index = output.pointer("/progress/message_index").and_then(Value::as_u64).map(|i| i as usize);
    // A member counts as completed only if every one of its messages was observed and the batch
    // either completed or stopped at a later message (not during final audit or earlier phases).
    let completed = all_observed && (batch_completed || stop_index.is_some_and(|i| i >= range.end));
    let status = if completed {
        QueueStatus::Succeeded
    } else if states.iter().all(|s| *s == "not_started") {
        QueueStatus::Cancelled
    } else {
        QueueStatus::Unknown
    };
    let progress = if completed {
        json!({"phase":"completed","operation":"parent_audit","message_index":null,"status":"completed","failure":null,"contact_diagnostic":null})
    } else {
        let mut progress = output.get("progress").cloned().unwrap_or(Value::Null);
        if progress.is_object() {
            progress["message_index"] = json!(local(stop_index.map(|i| i as u64)));
        }
        progress
    };
    let slice = json!({
        "outcome": if completed { "completed" } else { "unknown" },
        "outcomes": outcomes,
        "completed_count": states.iter().filter(|s| **s == "observed").count(),
        "unknown_index": local(output.get("unknown_index").and_then(Value::as_u64)),
        "cache_hits": output.get("cache_hits").cloned().unwrap_or(Value::Null),
        "verification": output.get("verification").cloned().unwrap_or(json!("local_ui_only")),
        "progress": progress,
        "message_states": states.iter().enumerate().map(|(i, s)| json!({"index": i, "state": s})).collect::<Vec<_>>(),
        "queue": {"batch_id": batch_id, "merged_requests": total, "batch_position": position, "message_offset": offset},
    });
    (status, slice)
}

#[cfg(test)]
#[path = "queue_tests.rs"]
mod tests;
