use super::*;
use crate::kernel::instruction::AtomicInstruction;
use tokio::sync::mpsc;

type Call = (String, TaskInput, oneshot::Sender<Delivery>);

async fn database() -> SqlitePool {
    let db = sqlx::sqlite::SqlitePoolOptions::new().max_connections(1).connect("sqlite::memory:").await.unwrap();
    sqlx::query("CREATE TABLE capability_audit(request_id TEXT,app TEXT,capability TEXT,route TEXT,target TEXT,effect TEXT,parameters TEXT,decision TEXT)")
        .execute(&db).await.unwrap();
    db
}

fn config(window: u64, merged_interval: u64, max_pending: u32) -> QueueConfig {
    QueueConfig { merge_window_secs: window, merged_interval_ms: merged_interval, max_pending, retention_secs: 3600 }
}

async fn start(db: &SqlitePool, config: QueueConfig) -> (Arc<InstructionQueue>, mpsc::UnboundedReceiver<Call>) {
    let (sender, receiver) = mpsc::unbounded_channel::<Call>();
    let executor: Executor = Arc::new(move |request_id, input| {
        let sender = sender.clone();
        Box::pin(async move {
            let (reply, result) = oneshot::channel();
            sender.send((request_id, input, reply)).unwrap();
            result.await.unwrap()
        })
    });
    (InstructionQueue::start(db.clone(), config, executor).await.unwrap(), receiver)
}

fn send(nickname: &str, texts: &[&str], interval_ms: u64) -> TaskInput {
    TaskInput::ExactInstruction {
        app: "wecom".into(),
        steps: vec![TaskInstruction::Application(ApplicationInstruction::SendMessageByNickname {
            nickname: nickname.into(),
            messages: texts.iter().map(|t| Message::Text { content: (*t).into() }).collect(),
            interval_ms,
        })],
    }
}

fn unpack(input: &TaskInput) -> (String, Vec<String>, u64) {
    let (_, nickname, messages, interval) = nickname_send(input).expect("nickname send");
    let texts = messages.iter().map(|m| match m { Message::Text { content } => content.clone(), _ => panic!("text") }).collect();
    (nickname.to_owned(), texts, interval)
}

fn output(states: &[&str], completed: bool, stop_index: Option<usize>) -> Value {
    json!({
        "outcome": if completed { "completed" } else { "unknown" },
        "outcomes": states.iter().enumerate().filter(|(_, s)| **s != "not_started").map(|(i, s)| json!({"index": i, "submitted": if *s == "observed" { json!(true) } else { Value::Null }, "message_observed": *s == "observed"})).collect::<Vec<_>>(),
        "completed_count": states.iter().filter(|s| **s == "observed").count(),
        "unknown_index": states.iter().position(|s| *s == "submission_unknown"),
        "cache_hits": {"message_navigation": true, "search": false, "composer": true, "contact_result": false},
        "verification": "local_ui_only",
        "progress": {"phase": if completed { "completed" } else { "send" }, "operation": if completed { "parent_audit" } else { "return_and_observe" }, "message_index": stop_index, "status": if completed { "completed" } else { "stopped" }, "failure": if completed { Value::Null } else { json!("postcondition") }, "contact_diagnostic": null},
        "message_states": states.iter().enumerate().map(|(i, s)| json!({"index": i, "state": s})).collect::<Vec<_>>(),
    })
}

fn reply_ok(call: Call, value: Value) {
    let instruction = call.1.resolve_instruction().unwrap();
    call.2.send(Ok((instruction, Some(value)))).unwrap();
}

async fn rows(db: &SqlitePool) -> Vec<(String, String, Option<String>, Option<String>)> {
    sqlx::query_as("SELECT request_id, status, payload, batch_id FROM instruction_queue ORDER BY id").fetch_all(db).await.unwrap()
}

async fn wait_pending(db: &SqlitePool, count: i64) {
    for _ in 0..200 {
        let pending: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM instruction_queue WHERE status = 'pending'").fetch_one(db).await.unwrap();
        if pending == count { return; }
        tokio::time::sleep(Duration::from_millis(5)).await;
    }
    panic!("pending count never reached {count}");
}

fn spawn_submit(queue: &Arc<InstructionQueue>, request_id: &str, input: TaskInput) -> tokio::task::JoinHandle<Delivery> {
    let queue = Arc::clone(queue);
    let request_id = request_id.to_owned();
    tokio::spawn(async move { queue.submit(&request_id, input).await })
}

#[tokio::test]
async fn merges_pending_same_nickname_messages_in_order_with_configured_interval() {
    let db = database().await;
    let (queue, mut calls) = start(&db, config(300, 250, 100)).await;
    let blocker = spawn_submit(&queue, "x", send("其他", &["x"], 0));
    let first = calls.recv().await.unwrap();
    assert_eq!(first.0, "x");
    let a1 = spawn_submit(&queue, "a1", send("赵德华", &["a1"], 5000));
    wait_pending(&db, 1).await;
    let b1 = spawn_submit(&queue, "b1", send("张三", &["b1"], 7));
    wait_pending(&db, 2).await;
    let a2 = spawn_submit(&queue, "a2", send("赵德华", &["a2", "a3"], 0));
    wait_pending(&db, 3).await;
    reply_ok(first, output(&["observed"], true, None));

    let merged = calls.recv().await.unwrap();
    assert_eq!(merged.0, "a1", "batch runs under the head request id");
    assert_eq!(unpack(&merged.1), ("赵德华".into(), vec!["a1".into(), "a2".into(), "a3".into()], 250));
    reply_ok(merged, output(&["observed", "observed", "observed"], true, None));

    let single = calls.recv().await.unwrap();
    assert_eq!(single.0, "b1");
    assert_eq!(unpack(&single.1), ("张三".into(), vec!["b1".into()], 7), "an unmerged request keeps its own interval");
    reply_ok(single, output(&["observed"], true, None));

    blocker.await.unwrap().unwrap();
    let (instruction, out) = a1.await.unwrap().unwrap();
    assert_eq!(unpack_instruction(&instruction).1, vec!["a1".to_owned()], "each caller receives its own original instruction");
    let out = out.unwrap();
    assert_eq!(out["outcome"], "completed");
    assert_eq!(out["queue"]["merged_requests"], 2);
    assert_eq!(out["queue"]["message_offset"], 0);
    let out2 = a2.await.unwrap().unwrap().1.unwrap();
    assert_eq!(out2["queue"]["message_offset"], 1);
    assert_eq!(out2["queue"]["batch_position"], 1);
    assert_eq!(out2["queue"]["batch_id"], out["queue"]["batch_id"]);
    assert_eq!(out2["message_states"], json!([{"index":0,"state":"observed"},{"index":1,"state":"observed"}]));
    assert_eq!(out2["outcomes"][1]["index"], 1);
    assert_eq!(out2["completed_count"], 2);
    assert_eq!(b1.await.unwrap().unwrap().1.unwrap()["queue"]["merged_requests"], 1);

    let rows = rows(&db).await;
    assert!(rows.iter().all(|(_, status, payload, _)| status == "succeeded" && payload.is_none()), "{rows:?}");
    let merged_audit: Vec<String> = sqlx::query_scalar("SELECT request_id FROM capability_audit WHERE decision = 'queue_merged' ORDER BY rowid").fetch_all(&db).await.unwrap();
    assert_eq!(merged_audit, ["a1", "a2"]);
    let audited: Vec<String> = sqlx::query_scalar("SELECT parameters FROM capability_audit").fetch_all(&db).await.unwrap();
    assert!(audited.iter().all(|p| p.starts_with("send-by-nickname-v2-sha256:")), "audit holds digests only");
    let stored: Vec<Option<String>> = sqlx::query_scalar("SELECT merge_key FROM instruction_queue").fetch_all(&db).await.unwrap();
    assert!(stored.iter().flatten().all(|k| k.starts_with("nickname-v1-sha256:") && !k.contains("赵德华")));
}

fn unpack_instruction(instruction: &TaskInstruction) -> (String, Vec<String>, u64) {
    unpack(&TaskInput::ExactInstruction { app: "wecom".into(), steps: vec![instruction.clone()] })
}

#[tokio::test]
async fn records_outside_the_window_or_limits_are_not_merged() {
    let db = database().await;
    let (queue, mut calls) = start(&db, config(0, 0, 100)).await;
    let blocker = spawn_submit(&queue, "x", send("其他", &["x"], 0));
    let first = calls.recv().await.unwrap();
    let a1 = spawn_submit(&queue, "a1", send("赵德华", &["a1"], 0));
    wait_pending(&db, 1).await;
    tokio::time::sleep(Duration::from_millis(15)).await;
    let a2 = spawn_submit(&queue, "a2", send("赵德华", &["a2"], 0));
    wait_pending(&db, 2).await;
    reply_ok(first, output(&["observed"], true, None));
    for expected in ["a1", "a2"] {
        let call = calls.recv().await.unwrap();
        assert_eq!(call.0, expected);
        assert_eq!(unpack(&call.1).1.len(), 1);
        reply_ok(call, output(&["observed"], true, None));
    }
    for handle in [blocker, a1, a2] { handle.await.unwrap().unwrap(); }

    // Merging stops at the message-count limit.
    let db = database().await;
    let (queue, mut calls) = start(&db, config(300, 0, 100)).await;
    let blocker = spawn_submit(&queue, "x", send("其他", &["x"], 0));
    let first = calls.recv().await.unwrap();
    let hundred: Vec<String> = (0..100).map(|i| format!("m{i}")).collect();
    let hundred: Vec<&str> = hundred.iter().map(String::as_str).collect();
    let big = spawn_submit(&queue, "big", send("赵德华", &hundred, 0));
    wait_pending(&db, 1).await;
    let small = spawn_submit(&queue, "small", send("赵德华", &["s"], 0));
    wait_pending(&db, 2).await;
    reply_ok(first, output(&["observed"], true, None));
    let call = calls.recv().await.unwrap();
    assert_eq!(unpack(&call.1).1.len(), 100);
    reply_ok(call, output(&vec!["observed"; 100], true, None));
    let call = calls.recv().await.unwrap();
    assert_eq!(call.0, "small");
    reply_ok(call, output(&["observed"], true, None));
    for handle in [blocker, big, small] { handle.await.unwrap().unwrap(); }
}

#[tokio::test]
async fn pre_dispatch_rejection_fails_every_member_and_non_send_instructions_are_queued() {
    let db = database().await;
    let (queue, mut calls) = start(&db, config(300, 0, 100)).await;
    let launch = TaskInput::ExactInstruction { app: "wecom".into(), steps: vec![TaskInstruction::Atomic(AtomicInstruction::ProcessLaunch)] };
    let blocker = spawn_submit(&queue, "launch", launch);
    let first = calls.recv().await.unwrap();
    assert_eq!(first.0, "launch");
    let a1 = spawn_submit(&queue, "a1", send("赵德华", &["a1"], 0));
    wait_pending(&db, 1).await;
    let a2 = spawn_submit(&queue, "a2", send("赵德华", &["a2"], 0));
    wait_pending(&db, 2).await;
    let instruction = first.1.resolve_instruction().unwrap();
    first.2.send(Ok((instruction, None))).unwrap();
    let merged = calls.recv().await.unwrap();
    merged.2.send(Err(InvokeError::Forbidden)).unwrap();
    assert!(blocker.await.unwrap().is_ok());
    assert_eq!(a1.await.unwrap(), Err(InvokeError::Forbidden));
    assert_eq!(a2.await.unwrap(), Err(InvokeError::Forbidden));
    let statuses: Vec<String> = rows(&db).await.into_iter().map(|r| r.1).collect();
    assert_eq!(statuses, ["succeeded", "failed", "failed"]);
}

#[test]
fn split_marks_succeeded_unknown_and_cancelled_members_without_retry() {
    let batch = output(&["observed", "observed", "submission_unknown", "not_started"], false, Some(2));
    let (s0, o0) = split_output(&batch, 0, 2, "b", 3, 0);
    let (s1, o1) = split_output(&batch, 2, 1, "b", 3, 1);
    let (s2, o2) = split_output(&batch, 3, 1, "b", 3, 2);
    assert_eq!((s0, s1, s2), (QueueStatus::Succeeded, QueueStatus::Unknown, QueueStatus::Cancelled));
    assert_eq!(o0["outcome"], "completed");
    assert_eq!(o0["progress"]["status"], "completed");
    assert_eq!(o1["outcome"], "unknown");
    assert_eq!(o1["unknown_index"], 0);
    assert_eq!(o1["progress"]["message_index"], 0);
    assert_eq!(o1["outcomes"], json!([{"index":0,"submitted":null,"message_observed":false}]));
    assert_eq!(o2["outcome"], "unknown");
    assert_eq!(o2["outcomes"], json!([]));
    assert_eq!(o2["progress"]["message_index"], Value::Null);
    assert_eq!(o2["cache_hits"]["message_navigation"], true);

    // A final parent-audit failure after every message was observed is unknown for all members.
    let mut audit = output(&["observed", "observed"], false, None);
    audit["progress"]["phase"] = json!("final_audit");
    assert_eq!(split_output(&audit, 0, 1, "b", 2, 0).0, QueueStatus::Unknown);
    assert_eq!(split_output(&audit, 1, 1, "b", 2, 1).0, QueueStatus::Unknown);
}

#[tokio::test]
async fn restart_marks_executing_records_unknown_and_full_queue_rejects() {
    let db = database().await;
    store::initialize(&db).await.unwrap();
    sqlx::query("INSERT INTO instruction_queue (request_id, app, action, request_digest, payload, enqueued_at_ms, status, updated_at_ms) VALUES ('old', 'wecom', 'send_message_by_nickname', 'd', '{}', 1, 'executing', 1)")
        .execute(&db).await.unwrap();
    let (queue, mut calls) = start(&db, config(300, 0, 1)).await;
    assert_eq!(rows(&db).await[0].1, "unknown");
    assert!(rows(&db).await[0].2.is_none());
    let blocker = spawn_submit(&queue, "x", send("其他", &["x"], 0));
    let first = calls.recv().await.unwrap();
    let pending = spawn_submit(&queue, "p", send("赵德华", &["p"], 0));
    wait_pending(&db, 1).await;
    assert_eq!(queue.submit("over", send("赵德华", &["o"], 0)).await, Err(InvokeError::RateLimited));
    reply_ok(first, output(&["observed"], true, None));
    let call = calls.recv().await.unwrap();
    reply_ok(call, output(&["observed"], true, None));
    blocker.await.unwrap().unwrap();
    pending.await.unwrap().unwrap();
    assert_eq!(rows(&db).await.len(), 3, "the rejected request was never stored");
}

#[test]
fn status_enum_round_trips_and_is_closed() {
    for status in QueueStatus::ALL {
        assert_eq!(QueueStatus::parse(status.as_str()), Some(status));
    }
    assert_eq!(QueueStatus::parse("sent"), None);
    assert!(!QueueStatus::Pending.is_terminal() && !QueueStatus::Executing.is_terminal());
    assert!(QueueStatus::Cancelled.is_terminal());
}
