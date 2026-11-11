use super::*;
use std::time::{Duration, Instant};

const NAME: &str = "exact private nickname";
fn messages() -> Vec<Message> {
    vec![
        Message::Text {
            content: "first exact text".into(),
        },
        Message::Text {
            content: "second exact text".into(),
        },
        Message::Text {
            content: "third exact text".into(),
        },
    ]
}
fn geometry() -> Geometry {
    Geometry {
        pid: 42,
        window_number: 7,
        display_id: 1,
        window_bounds: [100., 100., 800., 600.],
        display_bounds: [0., 0., 1440., 900.],
        image_size: [1600, 1200],
    }
}
fn capture() -> Arc<AppWindowCapture> {
    Arc::new(AppWindowCapture {
        path: "isolated-no-image-access".into(),
        capture_id: "isolated-capture".into(),
        image_width: 1600,
        image_height: 1200,
        window_bounds_points: geometry().window_bounds,
    })
}
fn contact_capture() -> Arc<AppWindowCapture> {
    Arc::new(AppWindowCapture {
        path: "isolated-contact-image".into(),
        capture_id: "isolated-contact-capture".into(),
        image_width: 1600,
        image_height: 1200,
        window_bounds_points: geometry().window_bounds,
    })
}
fn bounds(slot: Slot) -> [f64; 4] {
    match slot {
        Slot::Navigation => [10., 100., 50., 140.],
        Slot::Search => [100., 10., 200., 50.],
        Slot::Composer => [700., 900., 1100., 1100.],
    }
}

#[derive(Default)]
struct State {
    events: Vec<String>,
    navigation: bool,
    searched: bool,
    contact: bool,
    draft: String,
    history: Vec<String>,
    times: Vec<Instant>,
    ambiguous: bool,
    wrong_contact: bool,
    dirty_draft: bool,
    fail_send: Option<usize>,
    reject_launch: bool,
    fail_send_audit: bool,
    fail_send_intent: bool,
    fail_final_audit: bool,
    wrong_capture_reply: bool,
    fail_draft: Option<usize>,
    contact_error: Option<native::ContactError>,
    precheck_error: Option<native::ContactError>,
    reload_error: Option<native::ContactError>,
    changed_after_vision: bool,
    missing_prediction: bool,
    live_geometry: Option<Geometry>,
    contact_image_bounds: Option<[f64; 4]>,
    change_window_during_localization: bool,
    tamper_attachment_after_paste: bool,
    attachment_paths: Vec<std::path::PathBuf>,
}
impl State {
    fn geometry(&self) -> Geometry {
        self.live_geometry.clone().unwrap_or_else(geometry)
    }
    fn point(&self, slot: Slot) -> [f64; 2] {
        let g = self.geometry();
        super::super::screen_box(g.image_size, g.window_bounds, bounds(slot)).unwrap().1
    }
    fn capture(&self, contact: bool) -> Arc<AppWindowCapture> {
        let g = self.geometry();
        Arc::new(AppWindowCapture {
            path: "isolated-no-image-access".into(),
            capture_id: if contact { "isolated-contact-capture" } else { "isolated-capture" }.into(),
            image_width: g.image_size[0],
            image_height: g.image_size[1],
            window_bounds_points: g.window_bounds,
        })
    }
    fn popup_bounds(&self) -> [f64; 4] {
        let g = self.geometry();
        [g.window_bounds[0] + 40.0, g.window_bounds[1] + 40.0, 300.0, 200.0]
    }
    fn contact_point(&self) -> [f64; 2] {
        generated_point(&self.capture(true), self.contact_image_bounds.unwrap_or([100., 100., 300., 200.])).unwrap()
    }
}
struct Model {
    state: Arc<Mutex<State>>,
    db: SqlitePool,
    request: String,
}
impl Backend for Model {
    fn execute(
        &mut self,
        command: Command,
        nickname: &str,
        messages: &[Message],
        attachments: &media::PreparedMessages,
        _interval: u64,
    ) -> anyhow::Result<Reply> {
        // The model refuses every operation lacking a durable, currently pending child intent.
        let pending:i64=tokio::runtime::Handle::current().block_on(sqlx::query_scalar("SELECT (SELECT count(*) FROM capability_audit WHERE request_id=? AND decision='delegated_dispatch_intent')-(SELECT count(*) FROM capability_audit WHERE request_id=? AND decision IN ('delegated_dispatch_succeeded','delegated_dispatch_unknown'))").bind(&self.request).bind(&self.request).fetch_one(&self.db))?;
        anyhow::ensure!(pending == 1, "effect was not preceded by durable intent");
        let mut state = self.state.lock().unwrap();
        match command {
            Command::Launch => {
                state.events.push("launch".into());
                let count: i64 = tokio::runtime::Handle::current().block_on(
                    sqlx::query_scalar("SELECT count(*) FROM local_capability_grants")
                        .fetch_one(&self.db),
                )?;
                anyhow::ensure!(count == 0, "parent not consumed");
                anyhow::ensure!(!state.reject_launch, "launch failed");
            }
            Command::Center => {
                anyhow::ensure!(
                    state.events.last().map(String::as_str) == Some("launch"),
                    "center before launch"
                );
                state.events.push("center".into());
            }
            Command::Capture => {
                anyhow::ensure!(
                    state.events.iter().any(|e| e == "center"),
                    "capture before center"
                );
                if state.wrong_capture_reply {
                    return Ok(Reply::Unit);
                }
                state.events.push("capture".into());
                return Ok(Reply::Capture(state.capture(false)));
            }
            Command::CaptureContact => {
                state.events.push("contact_capture".into());
                anyhow::ensure!(state.searched, "contact capture before search");
                if let Some(error) = state.precheck_error {
                    return Err(anyhow::Error::new(error).context("private OS detail"));
                }
                if state.ambiguous {
                    return Err(native::ContactError {
                        dispatch: native::ContactDispatch::NotDispatched,
                        code: native::ContactCode::ContactResultAmbiguous,
                    }.into());
                }
                return Ok(Reply::Capture(state.capture(true)));
            }
            Command::Geometry(c) => {
                anyhow::ensure!(c.capture_id == capture().capture_id, "contact capture contaminated the measurement capture");
                state.events.push("geometry".into());
                return Ok(Reply::Geometry(state.geometry()));
            }
            Command::CacheRead(_) | Command::CacheWrite(..) | Command::CacheTouch(_) | Command::CacheInvalidate(_) => unreachable!("runtime handles SQLite"),
            Command::ContactGeometry => {
                if let Some(error) = state.reload_error {
                    return Err(error.into());
                }
                return Ok(Reply::ContactGeometry(native::ContactGeometry {
                    main: state.geometry(), popup_bounds: state.popup_bounds(),
                }));
            }
            Command::ValidateContact(point) => {
                state.events.push("contact_guard".into());
                anyhow::ensure!(!state.wrong_contact && !state.ambiguous, "contact is not unique exact");
                return Ok(Reply::Valid(point == state.contact_point()));
            }
            Command::Validate(slot, p) => {
                state.events.push(format!("guard:{}", slot.index()));
                let valid = p == state.point(slot)
                    && match slot {
                        Slot::Navigation => true,
                        Slot::Search => state.navigation,
                        Slot::Composer => {
                            state.contact && !state.dirty_draft && state.draft.is_empty()
                        }
                    };
                return Ok(Reply::Valid(valid));
            }
            Command::Locate(c, q) => {
                state.events.push(format!("vision:{q}"));
                if q == contact_query(nickname) {
                    anyhow::ensure!(c.capture_id == contact_capture().capture_id, "contact vision used the wrong capture");
                    if let Some(error) = state.reload_error {
                        return Err(anyhow::Error::new(error).context("private OS detail"));
                    }
                }
                let slot = if q == NAV_QUERY {
                    Some(Slot::Navigation)
                } else if q == SEARCH_QUERY {
                    Some(Slot::Search)
                } else if q == COMPOSER_QUERY {
                    Some(Slot::Composer)
                } else {
                    None
                };
                let b = if let Some(slot) = slot {
                    bounds(slot)
                } else {
                    anyhow::ensure!(
                        state.searched && q == contact_query(nickname),
                        "contact query not bound"
                    );
                    state.contact_image_bounds.unwrap_or([100., 100., 300., 200.])
                };
                let predictions = if slot.is_none() && state.missing_prediction {
                    json!([])
                } else if slot.is_none() && state.ambiguous {
                    json!([{"coords":b},{"coords":b}])
                } else {
                    json!([{"coords":b}])
                };
                let value = json!({"image_size":[c.image_width,c.image_height],"objects":[{"query":q,"predictions":predictions}]});
                if state.change_window_during_localization {
                    let mut changed = state.geometry();
                    changed.window_number += 1;
                    state.live_geometry = Some(changed);
                    state.change_window_during_localization = false;
                }
                return Ok(Reply::Box(unique_prediction(&value, &c, &q)?));
            }
            Command::Convert(c, b) => {
                state.events.push("convert".into());
                return Ok(Reply::Point(Some(generated_point(&c, b)?)));
            }
            Command::ConvertContact(c, b) => {
                state.events.push("contact_convert".into());
                anyhow::ensure!(c.capture_id == contact_capture().capture_id, "contact conversion used the wrong capture");
                if state.changed_after_vision {
                    return Err(anyhow::Error::new(native::ContactError {
                        dispatch: native::ContactDispatch::NotDispatched,
                        code: native::ContactCode::ContactCaptureChanged,
                    }).context("private OS detail"));
                }
                return Ok(Reply::Point(Some(generated_point(&c, b)?)));
            }
            Command::Navigation(p) => {
                anyhow::ensure!(p == state.point(Slot::Navigation), "wrong navigation");
                state.navigation = true;
                state.events.push("navigation".into());
            }
            Command::Search(p) => {
                anyhow::ensure!(state.navigation && p == state.point(Slot::Search), "wrong search");
                state.searched = true;
                state.events.push("search".into());
            }
            Command::Contact(p) => {
                anyhow::ensure!(p == state.contact_point(), "wrong whole-window coordinate frame");
                if let Some(error) = state.contact_error {
                    if state.fail_final_audit {
                        tokio::runtime::Handle::current().block_on(sqlx::query("CREATE TRIGGER reject_contact_terminal BEFORE INSERT ON capability_audit WHEN NEW.decision='delegation_stopped_unknown' BEGIN SELECT RAISE(ABORT,'isolated terminal failure'); END").execute(&self.db))?;
                    }
                    return Err(anyhow::Error::new(error).context("private OS detail"));
                }
                anyhow::ensure!(
                    state.searched && !state.wrong_contact && !state.ambiguous,
                    "contact is not unique exact"
                );
                state.contact = true;
                state.events.push("contact".into());
            }
            Command::Draft(p, index) => {
                anyhow::ensure!(state.fail_draft != Some(index), "draft failed");
                anyhow::ensure!(
                    state.contact
                        && state.draft.is_empty()
                        && !state.dirty_draft
                        && p == state.point(Slot::Composer),
                    "nonempty draft or wrong header"
                );
                state.draft = match &messages[index] {
                    Message::Text { content } => content.clone(),
                    Message::Image { filename, .. } | Message::File { filename, .. } => {
                        let attachment = attachments.get(index).ok_or_else(|| anyhow::anyhow!("missing snapshot"))?;
                        state.attachment_paths.push(attachment.path.clone());
                        if state.tamper_attachment_after_paste {
                            #[cfg(unix)] {
                                use std::os::unix::fs::PermissionsExt;
                                std::fs::set_permissions(&attachment.path, std::fs::Permissions::from_mode(0o600))?;
                            }
                            std::fs::write(&attachment.path, b"changed payload")?;
                        }
                        filename.clone()
                    }
                };
                state.events.push(format!("draft:{index}"));
            }
            Command::SendGuard(_, index) => {
                let content = match &messages[index] {
                    Message::Text { content } => content,
                    Message::Image { filename, .. } | Message::File { filename, .. } => filename,
                };
                anyhow::ensure!(state.contact && &state.draft == content, "draft changed");
                state.events.push(format!("send_guard:{index}"));
                if state.fail_send_intent {
                    tokio::runtime::Handle::current().block_on(sqlx::query("CREATE TRIGGER reject_send_intent BEFORE INSERT ON capability_audit WHEN NEW.capability='wecom.chat.draft.send' AND NEW.decision='delegated_dispatch_intent' BEGIN SELECT RAISE(ABORT,'isolated intent failure'); END").execute(&self.db))?;
                }
            }
            Command::Send(index) => {
                let content = match &messages[index] {
                    Message::Text { content } => content,
                    Message::Image { filename, .. } | Message::File { filename, .. } => filename,
                };
                anyhow::ensure!(
                    state.contact && &state.draft == content,
                    "send no longer exact"
                );
                state.events.push(format!("send:{index}"));
                if state.fail_send == Some(index) {
                    anyhow::bail!("unknown after Return");
                }
                let draft = std::mem::take(&mut state.draft);
                state.history.push(draft);
                state.times.push(Instant::now());
                if state.fail_send_audit {
                    tokio::runtime::Handle::current().block_on(sqlx::query("CREATE TRIGGER reject_send_result BEFORE INSERT ON capability_audit WHEN NEW.capability='wecom.chat.draft.send' AND NEW.decision='delegated_dispatch_succeeded' BEGIN SELECT RAISE(ABORT,'isolated result audit failure'); END").execute(&self.db))?;
                }
                if state.fail_final_audit && index + 1 == messages.len() {
                    tokio::runtime::Handle::current().block_on(sqlx::query("CREATE TRIGGER reject_terminal BEFORE INSERT ON capability_audit WHEN NEW.decision='delegation_completed' BEGIN SELECT RAISE(ABORT,'isolated terminal failure'); END").execute(&self.db))?;
                }
            }
            Command::Delay => unreachable!(),
        }
        Ok(Reply::Unit)
    }
}
async fn database() -> SqlitePool {
    let db = sqlx::sqlite::SqlitePoolOptions::new()
        .max_connections(1)
        .connect("sqlite::memory:")
        .await
        .unwrap();
    sqlx::query("CREATE TABLE local_capability_grants(app TEXT,capability TEXT,route TEXT,target TEXT,effect TEXT,parameters TEXT,PRIMARY KEY(app,capability,route,target,effect,parameters))").execute(&db).await.unwrap(); sqlx::query(crate::runtime::grants::CREATE_TABLE).execute(&db).await.unwrap();
    sqlx::query("CREATE TABLE capability_audit(request_id TEXT,app TEXT,capability TEXT,route TEXT,target TEXT,effect TEXT,parameters TEXT,decision TEXT)").execute(&db).await.unwrap();
    coordinate_cache::initialize(&db).await.unwrap();
    db
}
async fn grant(db: &SqlitePool, nickname: &str, messages: &[Message], interval: u64) -> String {
    let p = send_message_by_nickname_parameters(nickname, messages, interval).unwrap();
    sqlx::query("INSERT INTO local_capability_grants VALUES('wecom',?,?,?,?,?)")
        .bind(SCOPE.0)
        .bind(ROUTE)
        .bind(SCOPE.1)
        .bind(SCOPE.2)
        .bind(AUTHORIZATION)
        .execute(db)
        .await
        .unwrap();
    p
}
async fn run(
    db: &SqlitePool,
    request: &str,
    state: Arc<Mutex<State>>,
    messages: &[Message],
    interval: u64,
) -> Value {
    run_named(db, request, NAME, state, messages, interval).await
}
async fn run_named(
    db: &SqlitePool,
    request: &str,
    nickname: &str,
    state: Arc<Mutex<State>>,
    messages: &[Message],
    interval: u64,
) -> Value {
    grant(db, nickname, messages, interval).await;
    invoke_backend(
        db,
        request,
        nickname,
        messages,
        interval,
        Model {
            state,
            db: db.clone(),
            request: request.into(),
        },
    )
    .await
    .unwrap()
}
fn reset_gui(state: &mut State) {
    state.events.clear();
    state.navigation = false;
    state.searched = false;
    state.contact = false;
    state.draft.clear();
    state.history.clear();
    state.times.clear();
}

#[tokio::test]
async fn repeated_nickname_uses_four_sqlite_points_and_only_two_fresh_captures() {
    let db = database().await;
    let state = Arc::new(Mutex::new(State::default()));
    let text = messages();
    let cold = run(&db, "cold", Arc::clone(&state), &text[..1], 0).await;
    assert_eq!(cold["outcome"], "completed");
    assert_eq!(cold["cache_hits"], json!({"message_navigation":false,"search":false,"composer":false,"contact_result":false}));
    {
        let mut s = state.lock().unwrap();
        assert_eq!(s.events.iter().filter(|e| e.starts_with("vision:")).count(), 4);
        assert_eq!(s.history, ["first exact text"]);
        reset_gui(&mut s);
    }
    let rows: i64 = sqlx::query_scalar("SELECT count(*) FROM coordinate_cache").fetch_one(&db).await.unwrap();
    assert_eq!(rows, 4);
    // Different body/interval must not defeat nickname localization reuse.
    let warm = run(&db, "warm", Arc::clone(&state), &text[1..], 1).await;
    assert_eq!(warm["outcome"], "completed");
    assert_eq!(warm["cache_hits"], json!({"message_navigation":true,"search":true,"composer":true,"contact_result":true}));
    let s = state.lock().unwrap();
    assert_eq!(&s.events[..2], &["launch", "center"]);
    assert_eq!(s.events.iter().filter(|e| e.starts_with("vision:")).count(), 0);
    assert_eq!(s.events.iter().filter(|e| e.as_str() == "capture" || e.as_str() == "contact_capture").count(), 2);
    assert_eq!(s.history, ["second exact text", "third exact text"]);
}

#[tokio::test]
async fn stale_coordinate_is_relocalized_without_reusing_another_geometry() {
    let db = database().await;
    let state = Arc::new(Mutex::new(State::default()));
    run(&db, "populate", Arc::clone(&state), &messages()[..1], 0).await;
    sqlx::query("UPDATE coordinate_cache SET point_x=1.0,point_y=1.0").execute(&db).await.unwrap();
    reset_gui(&mut state.lock().unwrap());
    let output = run(&db, "stale", Arc::clone(&state), &messages()[..1], 0).await;
    assert_eq!(output["outcome"], "completed");
    assert_eq!(output["cache_hits"], json!({"message_navigation":false,"search":false,"composer":false,"contact_result":false}));
    assert_eq!(state.lock().unwrap().events.iter().filter(|e| e.starts_with("vision:")).count(), 4);
}

#[tokio::test]
async fn new_identity_and_translated_window_reuse_points_but_resizing_does_not() {
    let db = database().await;
    let first = Arc::new(Mutex::new(State::default()));
    run(&db, "original", first, &messages()[..1], 0).await;
    let mut moved = geometry();
    moved.pid = 99;
    moved.window_number = 20;
    moved.window_bounds[0] += 120.0;
    moved.window_bounds[1] += 50.0;
    let state = Arc::new(Mutex::new(State { live_geometry: Some(moved.clone()), ..State::default() }));
    let warm = run(&db, "restarted", Arc::clone(&state), &messages()[..1], 0).await;
    assert_eq!(warm["outcome"], "completed");
    assert_eq!(warm["cache_hits"], json!({"message_navigation":true,"search":true,"composer":true,"contact_result":true}));
    assert_eq!(state.lock().unwrap().history, ["first exact text"]);
    {
        let mut s = state.lock().unwrap();
        reset_gui(&mut s);
        moved.window_bounds[2] = 900.0;
        moved.image_size[0] = 1800;
        s.live_geometry = Some(moved);
    }
    let resized = run(&db, "resized", Arc::clone(&state), &messages()[..1], 0).await;
    assert_eq!(resized["outcome"], "completed");
    assert_eq!(resized["cache_hits"], json!({"message_navigation":false,"search":false,"composer":false,"contact_result":false}));
}

#[tokio::test]
async fn different_nickname_shares_only_global_points_and_cache_contains_no_plaintext() {
    let db = database().await;
    let state = Arc::new(Mutex::new(State::default()));
    run(&db, "one-name", Arc::clone(&state), &messages()[..1], 0).await;
    reset_gui(&mut state.lock().unwrap());
    let next = run_named(&db, "other-name", "another exact nickname", Arc::clone(&state), &messages()[..1], 0).await;
    assert_eq!(next["outcome"], "completed");
    assert_eq!(next["cache_hits"], json!({"message_navigation":true,"search":true,"composer":true,"contact_result":false}));
    let keys: Vec<(String, String)> = sqlx::query_as("SELECT content_digest,geometry_key FROM coordinate_cache").fetch_all(&db).await.unwrap();
    assert_eq!(keys.len(), 5);
    assert!(keys.iter().all(|(digest, geometry)| !digest.contains(NAME) && !geometry.contains(NAME)
        && !digest.contains("another exact nickname") && !geometry.contains("exact text")));
    assert_eq!(state.lock().unwrap().events.iter().filter(|e| e.starts_with("vision:")).count(), 1);
}

#[tokio::test]
async fn reordered_contact_relocalizes_only_that_nickname_and_warm_ambiguity_stops() {
    let db = database().await;
    let state = Arc::new(Mutex::new(State::default()));
    run(&db, "before-move", Arc::clone(&state), &messages()[..1], 0).await;
    {
        let mut s = state.lock().unwrap();
        reset_gui(&mut s);
        s.contact_image_bounds = Some([100., 220., 300., 300.]);
    }
    let moved = run(&db, "after-move", Arc::clone(&state), &messages()[..1], 0).await;
    assert_eq!(moved["outcome"], "completed");
    assert_eq!(moved["cache_hits"], json!({"message_navigation":true,"search":true,"composer":true,"contact_result":false}));
    assert_eq!(state.lock().unwrap().events.iter().filter(|e| e.starts_with("vision:")).count(), 1);
    {
        let mut s = state.lock().unwrap();
        reset_gui(&mut s);
        s.ambiguous = true;
    }
    let ambiguous = run(&db, "ambiguous-warm", Arc::clone(&state), &messages()[..1], 0).await;
    assert_eq!(ambiguous["outcome"], "unknown");
    assert_eq!(ambiguous["cache_hits"]["contact_result"], false);
    assert_eq!(ambiguous["progress"]["contact_diagnostic"]["code"], "contact_result_ambiguous");
    assert!(state.lock().unwrap().events.iter().all(|e| !e.starts_with("draft:") && !e.starts_with("send:")));
}

#[tokio::test]
async fn cache_storage_faults_fall_back_without_replaying_effects() {
    for missing_table in [true, false] {
        let db = database().await;
        if missing_table {
            sqlx::query("DROP TABLE coordinate_cache").execute(&db).await.unwrap();
        } else {
            sqlx::query("CREATE TRIGGER reject_cache_write BEFORE INSERT ON coordinate_cache BEGIN SELECT RAISE(ABORT,'isolated cache fault'); END")
                .execute(&db).await.unwrap();
        }
        let state = Arc::new(Mutex::new(State::default()));
        let output = run(&db, "cache-fault", Arc::clone(&state), &messages()[..1], 0).await;
        assert_eq!(output["outcome"], "completed");
        assert_eq!(output["cache_hits"], json!({"message_navigation":false,"search":false,"composer":false,"contact_result":false}));
        let s = state.lock().unwrap();
        assert_eq!(s.history, ["first exact text"]);
        assert_eq!(s.events.iter().filter(|e| e.as_str() == "send:0").count(), 1);
        assert_eq!(s.events.iter().filter(|e| e.starts_with("vision:")).count(), 4);
    }
}

#[tokio::test]
async fn warm_partial_unknown_preserves_all_validated_cache_hits() {
    let db = database().await;
    let state = Arc::new(Mutex::new(State::default()));
    run(&db, "warmup", Arc::clone(&state), &messages()[..1], 0).await;
    {
        let mut s = state.lock().unwrap();
        reset_gui(&mut s);
        s.fail_send = Some(1);
    }
    let partial = run(&db, "warm-partial", Arc::clone(&state), &messages(), 0).await;
    assert_eq!(partial["outcome"], "unknown");
    assert_eq!(partial["completed_count"], 1);
    assert_eq!(partial["unknown_index"], 1);
    assert_eq!(partial["cache_hits"], json!({"message_navigation":true,"search":true,"composer":true,"contact_result":true}));
    assert!(!state.lock().unwrap().events.iter().any(|e| e == "draft:2" || e == "send:2"));
}

#[tokio::test]
async fn changed_capture_binding_blocks_even_a_warm_contact_candidate() {
    let db = database().await;
    let state = Arc::new(Mutex::new(State::default()));
    run(&db, "binding-warmup", Arc::clone(&state), &messages()[..1], 0).await;
    {
        let mut s = state.lock().unwrap();
        reset_gui(&mut s);
        s.reload_error = Some(native::ContactError {
            dispatch: native::ContactDispatch::NotDispatched,
            code: native::ContactCode::ContactCaptureChanged,
        });
    }
    let output = run(&db, "binding-changed", Arc::clone(&state), &messages()[..1], 0).await;
    assert_eq!(output["outcome"], "unknown");
    assert_eq!(output["cache_hits"]["contact_result"], false);
    assert_eq!(output["progress"]["contact_diagnostic"], json!({"dispatch":"not_dispatched","code":"contact_capture_changed"}));
    assert!(state.lock().unwrap().events.iter().all(|e| e != "contact" && !e.starts_with("draft:") && !e.starts_with("send:")));
}

#[tokio::test]
async fn window_change_during_localization_stops_before_navigation() {
    let db = database().await;
    let state = Arc::new(Mutex::new(State {
        change_window_during_localization: true, ..State::default()
    }));
    let output = run(&db, "geometry-changed", Arc::clone(&state), &messages()[..1], 0).await;
    assert_eq!(output["outcome"], "unknown");
    assert_eq!(output["progress"]["failure"], "postcondition");
    let s = state.lock().unwrap();
    assert!(!s.events.iter().any(|e| e == "navigation" || e == "search" || e.starts_with("send:")));
    drop(s);
    let cached: i64 = sqlx::query_scalar("SELECT count(*) FROM coordinate_cache").fetch_one(&db).await.unwrap();
    assert_eq!(cached, 0);
}

#[tokio::test]
async fn stale_contact_is_removed_without_deleting_global_points_when_relocation_fails() {
    let db = database().await;
    let state = Arc::new(Mutex::new(State::default()));
    run(&db, "stale-warmup", Arc::clone(&state), &messages()[..1], 0).await;
    {
        let mut s = state.lock().unwrap();
        reset_gui(&mut s);
        s.contact_image_bounds = Some([100., 220., 300., 300.]);
        s.missing_prediction = true;
    }
    let output = run(&db, "stale-contact", Arc::clone(&state), &messages()[..1], 0).await;
    assert_eq!(output["outcome"], "unknown");
    assert!(state.lock().unwrap().events.iter().all(|e| e != "contact" && !e.starts_with("draft:") && !e.starts_with("send:")));
    let counts: Vec<(String, i64)> = sqlx::query_as("SELECT scope,count(*) FROM coordinate_cache GROUP BY scope").fetch_all(&db).await.unwrap();
    assert_eq!(counts, [("global".to_owned(), 3)]);
}

#[tokio::test]
async fn contact_ambiguity_wrong_header_and_dirty_draft_never_send() {
    for case in 0..3 {
        let db = database().await;
        let mut s = State::default();
        s.ambiguous = case == 0;
        s.wrong_contact = case == 1;
        s.dirty_draft = case == 2;
        let state = Arc::new(Mutex::new(s));
        let output = run(
            &db,
            &format!("guard-{case}"),
            Arc::clone(&state),
            &messages(),
            0,
        )
        .await;
        assert_eq!(output["outcome"], "unknown");
        assert_eq!(output["completed_count"], 0);
        assert_eq!(output["message_states"], json!([{"index":0,"state":"not_started"},{"index":1,"state":"not_started"},{"index":2,"state":"not_started"}]));
        let (phase, operation, failure) = match case {
            0 => ("contact", "window_capture", "operation"),
            1 => ("contact", "live_control_guard", "operation"),
            _ => ("composer", "live_control_guard", "postcondition"),
        };
        let diagnostic = if case == 0 { json!({"dispatch":"not_dispatched","code":"contact_result_ambiguous"}) } else if case == 1 { json!({"dispatch":"not_dispatched","code":"contact_operation_failed"}) } else { Value::Null };
        assert_eq!(output["progress"], json!({"phase":phase,"operation":operation,"message_index":null,"status":"stopped","failure":failure,"contact_diagnostic":diagnostic}));
        assert!(
            state
                .lock()
                .unwrap()
                .events
                .iter()
                .all(|e| !e.starts_with("send:") && !e.starts_with("draft:"))
        );
    }
}

#[tokio::test]
async fn ordered_messages_delay_only_between_completed_sends_and_hash_only_audit() {
    let db = database().await;
    let state = Arc::new(Mutex::new(State::default()));
    let messages = messages();
    let output = run(&db, "ordered", Arc::clone(&state), &messages, 10).await;
    assert_eq!(output["completed_count"], 3);
    assert_eq!(output["unknown_index"], Value::Null);
    assert_eq!(output["progress"], json!({"phase":"completed","operation":"parent_audit","message_index":null,"status":"completed","failure":null,"contact_diagnostic":null}));
    assert_eq!(output["message_states"], json!([{"index":0,"state":"observed"},{"index":1,"state":"observed"},{"index":2,"state":"observed"}]));
    {
        let s = state.lock().unwrap();
        assert_eq!(
            s.history,
            vec!["first exact text", "second exact text", "third exact text"]
        );
        assert!(
            s.times
                .windows(2)
                .all(|times| times[1].duration_since(times[0]) >= Duration::from_millis(10))
        );
    }
    let delays:i64=sqlx::query_scalar("SELECT count(*) FROM capability_audit WHERE capability=? AND decision='delegated_dispatch_intent'").bind(SCOPE.0).fetch_one(&db).await.unwrap();
    assert_eq!(delays, 2);
    let rows: Vec<String> = sqlx::query_scalar("SELECT parameters FROM capability_audit")
        .fetch_all(&db)
        .await
        .unwrap();
    assert!(
        rows.iter()
            .all(|p| !p.contains(NAME) && !p.contains("exact text"))
    );
    assert!(
        rows.iter()
            .all(|p| p.starts_with(PREFIX) || p.starts_with("nickname-child-v1-sha256:"))
    );
}

#[tokio::test]
async fn second_send_unknown_preserves_first_and_stops_without_delay_or_retry() {
    let db = database().await;
    let mut s = State::default();
    s.fail_send = Some(1);
    let state = Arc::new(Mutex::new(s));
    let output = run(&db, "partial", Arc::clone(&state), &messages(), 1).await;
    assert_eq!(output["outcome"], "unknown");
    assert_eq!(output["completed_count"], 1);
    assert_eq!(output["unknown_index"], 1);
    assert_eq!(output["progress"], json!({"phase":"send","operation":"return_and_observe","message_index":1,"status":"stopped","failure":"operation","contact_diagnostic":null}));
    assert_eq!(output["message_states"], json!([{"index":0,"state":"observed"},{"index":1,"state":"submission_unknown"},{"index":2,"state":"not_started"}]));
    assert_eq!(
        output["outcomes"],
        json!([{"index":0,"submitted":true,"message_observed":true},{"index":1,"submitted":null,"message_observed":false}])
    );
    let s = state.lock().unwrap();
    assert_eq!(
        s.events.iter().filter(|e| e.as_str() == "send:1").count(),
        1
    );
    assert!(!s.events.iter().any(|e| e == "draft:2" || e == "send:2"));
    drop(s);
    let delays:i64=sqlx::query_scalar("SELECT count(*) FROM capability_audit WHERE capability=? AND decision='delegated_dispatch_intent'").bind(SCOPE.0).fetch_one(&db).await.unwrap();
    assert_eq!(delays, 1);
}

#[tokio::test]
async fn absent_or_replayed_parent_grant_has_no_gui_effect() {
    let db = database().await;
    let state = Arc::new(Mutex::new(State::default()));
    let messages = messages();
    let result = invoke_backend(
        &db,
        "absent",
        NAME,
        &messages,
        0,
        Model {
            state: Arc::clone(&state),
            db: db.clone(),
            request: "absent".into(),
        },
    )
    .await;
    assert_eq!(result, Err(InvokeError::Forbidden));
    assert!(state.lock().unwrap().events.is_empty());
    state.lock().unwrap().reject_launch = true;
    let output = run(&db, "consumed", Arc::clone(&state), &messages, 0).await;
    assert_eq!(output["outcome"], "unknown");
    state.lock().unwrap().events.clear();
    let result = invoke_backend(
        &db,
        "replay",
        NAME,
        &messages,
        0,
        Model {
            state: Arc::clone(&state),
            db: db.clone(),
            request: "replay".into(),
        },
    )
    .await;
    assert_eq!(result, Err(InvokeError::Forbidden));
    assert!(state.lock().unwrap().events.is_empty());
}

#[tokio::test]
async fn instruction_grant_accepts_changed_content_and_audits_current_request() {
    let original = messages();
    for (nickname, content, interval) in [
        (NAME, "different exact body", 0),
        ("different exact nickname", "another body", 1000),
    ] {
        let db = database().await;
        let original_digest = grant(&db, NAME, &original, 0).await;
        let actual = vec![Message::Text { content: content.into() }];
        let digest = send_message_by_nickname_parameters(nickname, &actual, interval).unwrap();
        assert_ne!(digest, original_digest);
        let state = Arc::new(Mutex::new(State { reject_launch: true, ..State::default() }));
        let result = invoke_backend(
            &db, "instruction-auth", nickname, &actual, interval,
            Model {
                state: Arc::clone(&state), db: db.clone(),
                request: "instruction-auth".into(),
            },
        ).await.unwrap();
        // The injected backend stops at launch: no native input or sending.
        assert_eq!(result["progress"]["phase"], "launch");
        assert_eq!(state.lock().unwrap().events, ["launch"]);
        let remaining: i64 = sqlx::query_scalar("SELECT count(*) FROM local_capability_grants")
            .fetch_one(&db).await.unwrap();
        assert_eq!(remaining, 0);
        let audit: Vec<String> = sqlx::query_scalar("SELECT parameters FROM capability_audit WHERE decision IN ('authorization_check','dispatch_authorized')")
            .fetch_all(&db).await.unwrap();
        assert_eq!(audit, [digest.clone(), digest]);
        assert!(audit.iter().all(|value| !value.contains(content)));
    }
}

#[test]
fn long_exact_nickname_is_not_truncated_or_rejected_by_query() {
    let name = "x".repeat(256);
    assert_eq!(contact_query(&name), name);
}

#[tokio::test]
async fn send_intent_failure_preserves_known_unsubmitted_output() {
    let db = database().await;
    let state = Arc::new(Mutex::new(State { fail_send_intent: true, ..State::default() }));
    let output = run(&db, "send-intent-failure", Arc::clone(&state), &messages(), 0).await;
    assert_eq!(output["outcome"], "unknown");
    assert_eq!(output["progress"]["failure"], "audit");
    assert_eq!(output["outcomes"], json!([{"index":0,"submitted":false,"message_observed":false}]));
    assert_eq!(output["message_states"], json!([
        {"index":0,"state":"preparing"},{"index":1,"state":"not_started"},{"index":2,"state":"not_started"}
    ]));
    let state = state.lock().unwrap();
    assert!(state.history.is_empty());
    assert!(!state.events.iter().any(|event| event.starts_with("send:") || event == "draft:1"));
}

#[tokio::test]
async fn completed_native_send_survives_result_audit_failure_in_partial_output() {
    let db = database().await;
    let mut s = State::default();
    s.fail_send_audit = true;
    let state = Arc::new(Mutex::new(s));
    let output = run(&db, "audit-after-send", Arc::clone(&state), &messages(), 0).await;
    assert_eq!(output["outcome"], "unknown");
    assert_eq!(output["completed_count"], 1);
    assert_eq!(output["progress"], json!({"phase":"send","operation":"return_and_observe","message_index":0,"status":"stopped","failure":"audit","contact_diagnostic":null}));
    assert_eq!(output["message_states"], json!([{"index":0,"state":"observed"},{"index":1,"state":"not_started"},{"index":2,"state":"not_started"}]));
    assert_eq!(
        output["outcomes"],
        json!([{"index":0,"submitted":true,"message_observed":true}])
    );
    let s = state.lock().unwrap();
    assert_eq!(s.history, vec!["first exact text"]);
    assert!(!s.events.iter().any(|e| e == "draft:1"));
}

#[test]
fn scaled_vision_center_is_quantized_only_inside_prediction_and_window() {
    let c = AppWindowCapture {
        path: "not-read".into(),
        capture_id: "scaled".into(),
        image_width: 2120,
        image_height: 1280,
        window_bounds_points: [226., 171., 1060., 640.],
    };
    let b = [
        700. * 2120. / 1280.,
        500. * 1280. / 772.,
        800. * 2120. / 1280.,
        550. * 1280. / 772.,
    ];
    let (screen, exact) =
        super::super::screen_box([2120, 1280], c.window_bounds_points, b).unwrap();
    let point = generated_point(&c, b).unwrap();
    assert_ne!(point, exact);
    assert!(
        point
            .into_iter()
            .all(|coordinate| f64::from(coordinate as f32) == coordinate)
    );
    assert!(
        point[0] > screen[0]
            && point[0] < screen[2]
            && point[1] > screen[1]
            && point[1] < screen[3]
    );
    assert!(point[0] > 226. && point[0] < 1286. && point[1] > 171. && point[1] < 811.);
    // Rounding a microscopic box onto its edge is rejected, never clamped.
    assert!(generated_point(&c, [1., 1., 1. + 1e-12, 1. + 1e-12]).is_err());
}

#[tokio::test]
async fn instruction_grants_are_isolated_and_legacy_content_grants_are_rejected() {
    let text = messages();
    let digest = send_message_by_nickname_parameters(NAME, &text, 0).unwrap();
    let rows = [
        ("other", SCOPE.0, ROUTE, SCOPE.1, SCOPE.2, AUTHORIZATION),
        ("wecom", "desktop.keyboard.key_input", ROUTE, SCOPE.1, SCOPE.2, AUTHORIZATION),
        ("wecom", SCOPE.0, "other_route", SCOPE.1, SCOPE.2, AUTHORIZATION),
        ("wecom", SCOPE.0, ROUTE, "other_target", SCOPE.2, AUTHORIZATION),
        ("wecom", SCOPE.0, ROUTE, SCOPE.1, "other_effect", AUTHORIZATION),
        ("wecom", SCOPE.0, ROUTE, SCOPE.1, SCOPE.2, digest.as_str()),
        ("wecom", SCOPE.0, ROUTE, SCOPE.1, SCOPE.2, "none:v1"),
    ];
    for (app, capability, route, target, effect, parameters) in rows {
        let db = database().await;
        sqlx::query("INSERT INTO local_capability_grants VALUES(?,?,?,?,?,?)")
            .bind(app).bind(capability).bind(route).bind(target).bind(effect).bind(parameters)
            .execute(&db).await.unwrap();
        assert!(!send_message_by_nickname_any_grant(&db).await.unwrap());
        let state = Arc::new(Mutex::new(State::default()));
        let result = invoke_backend(
            &db, "wrong-scope", NAME, &text, 0,
            Model { state: Arc::clone(&state), db: db.clone(), request: "wrong-scope".into() },
        ).await;
        assert_eq!(result, Err(InvokeError::Forbidden));
        assert!(state.lock().unwrap().events.is_empty());
        let remaining: i64 = sqlx::query_scalar("SELECT count(*) FROM local_capability_grants")
            .fetch_one(&db).await.unwrap();
        assert_eq!(remaining, 1);
    }
    let db = database().await;
    grant(&db, NAME, &text, 0).await;
    assert!(send_message_by_nickname_any_grant(&db).await.unwrap());
    assert!(authorize_parent(&db, "catalog-consume", &digest).await.unwrap());
    assert!(!send_message_by_nickname_any_grant(&db).await.unwrap());
}

#[tokio::test]
async fn parent_intent_audit_failure_rolls_back_instruction_grant_consumption() {
    let db = database().await;
    let text = messages();
    grant(&db, NAME, &text, 0).await;
    sqlx::query("CREATE TRIGGER reject_parent_intent BEFORE INSERT ON capability_audit WHEN NEW.decision='dispatch_authorized' BEGIN SELECT RAISE(ABORT,'isolated audit failure'); END")
        .execute(&db).await.unwrap();
    let state = Arc::new(Mutex::new(State::default()));
    let result = invoke_backend(
        &db, "parent-audit-failure", NAME, &text, 0,
        Model { state: Arc::clone(&state), db: db.clone(), request: "parent-audit-failure".into() },
    ).await;
    assert_eq!(result, Err(InvokeError::AuditUnavailable));
    assert!(state.lock().unwrap().events.is_empty());
    assert!(send_message_by_nickname_any_grant(&db).await.unwrap());
    let audits: i64 = sqlx::query_scalar("SELECT count(*) FROM capability_audit")
        .fetch_one(&db).await.unwrap();
    assert_eq!(audits, 0);
}

#[tokio::test]
async fn concurrent_requests_cannot_reuse_one_instruction_grant() {
    let db = database().await;
    let text = messages();
    grant(&db, NAME, &text, 0).await;
    let first = Arc::new(Mutex::new(State { reject_launch: true, ..State::default() }));
    let second = Arc::new(Mutex::new(State { reject_launch: true, ..State::default() }));
    let (a, b) = tokio::join!(
        invoke_backend(&db, "first", NAME, &text, 0, Model {
            state: Arc::clone(&first), db: db.clone(), request: "first".into(),
        }),
        invoke_backend(&db, "second", NAME, &text, 0, Model {
            state: Arc::clone(&second), db: db.clone(), request: "second".into(),
        }),
    );
    assert_eq!(usize::from(a.is_ok()) + usize::from(b.is_ok()), 1);
    assert!(a == Err(InvokeError::Forbidden) || b == Err(InvokeError::Forbidden));
    let launched = first.lock().unwrap().events.len() + second.lock().unwrap().events.len();
    assert_eq!(launched, 1);
    assert!(!send_message_by_nickname_any_grant(&db).await.unwrap());
}

#[tokio::test]
async fn revocation_ack_waits_for_pending_native_dispatch_and_blocks_next_step() {
    struct Paused {
        model: Model,
        entered: Arc<tokio::sync::Notify>,
        resume: std::sync::mpsc::Receiver<()>,
        pause_on_send: bool,
    }
    impl Backend for Paused {
        fn execute(
            &mut self,
            command: Command,
            nickname: &str,
            messages: &[Message],
            attachments: &media::PreparedMessages,
            interval: u64,
        ) -> anyhow::Result<Reply> {
            if self.pause_on_send && matches!(command, Command::Send(0)) {
                let result = self.model.execute(command, nickname, messages, attachments, interval);
                self.entered.notify_one();
                self.resume.recv()?;
                return result;
            }
            if !self.pause_on_send && matches!(command, Command::Launch) {
                self.entered.notify_one();
                self.resume.recv()?;
            }
            self.model.execute(command, nickname, messages, attachments, interval)
        }
    }
    for pause_on_send in [false, true] {
        let db = database().await;
        let state = Arc::new(Mutex::new(State::default()));
        let messages = messages();
        let parameters = grant(&db, NAME, &messages, 0).await;
        let entered = Arc::new(tokio::sync::Notify::new());
        let (resume, receiver) = std::sync::mpsc::channel();
        let backend = Paused {
            model: Model {
                state: Arc::clone(&state),
                db: db.clone(),
                request: "revocation-race".into(),
            },
            entered: Arc::clone(&entered),
            resume: receiver,
            pause_on_send,
        };
        let worker_db = db.clone();
        let worker = tokio::spawn(async move {
            invoke_backend(&worker_db, "revocation-race", NAME, &messages, 0, backend).await
        });
        entered.notified().await;
        let (started, ready) = tokio::sync::oneshot::channel();
        let revoke_db = db.clone();
        let revocation = tokio::spawn(async move {
            started.send(()).unwrap();
            revoke_send_message_by_nickname(&revoke_db, "revocation-race", &parameters).await
        });
        ready.await.unwrap();
        let committed: i64 = sqlx::query_scalar("SELECT count(*) FROM capability_audit WHERE request_id='revocation-race' AND decision='delegation_revoked'")
            .fetch_one(&db).await.unwrap();
        // Release before asserting so an assertion failure cannot strand the worker.
        resume.send(()).unwrap();
        let output = worker.await.unwrap().unwrap();
        revocation.await.unwrap().unwrap();
        assert_eq!(
            committed, 0,
            "revocation cannot acknowledge before an admitted native command finishes"
        );
        assert_eq!(output["outcome"], "unknown");
        assert_eq!(output["progress"]["failure"], "authorization");
        assert_eq!(output["progress"]["status"], "stopped");
        let state = state.lock().unwrap();
        if pause_on_send {
            assert_eq!(output["completed_count"], 1);
            assert_eq!(state.history, ["first exact text"]);
            assert!(!state.events.iter().any(|event| event == "draft:1"));
        } else {
            assert_eq!(state.events, ["launch"]);
        }
    }
}


#[tokio::test]
async fn second_draft_failure_preserves_preparing_state() {
    let db = database().await;
    let state = Arc::new(Mutex::new(State { fail_draft: Some(1), ..State::default() }));
    let output = run(&db, "draft-failure", state, &messages(), 0).await;
    assert_eq!(output["progress"], json!({"phase":"draft","operation":"empty_draft_write","message_index":1,"status":"stopped","failure":"operation","contact_diagnostic":null}));
    assert_eq!(output["message_states"], json!([{"index":0,"state":"observed"},{"index":1,"state":"preparing"},{"index":2,"state":"not_started"}]));
}
#[tokio::test]
async fn wrong_reply_and_terminal_audit_failure_are_distinct() {
    let db = database().await;
    let state = Arc::new(Mutex::new(State { wrong_capture_reply: true, ..State::default() }));
    let output = run(&db, "wrong-reply", state, &messages(), 0).await;
    assert_eq!(output["progress"], json!({"phase":"measurement","operation":"window_capture","message_index":null,"status":"stopped","failure":"postcondition","contact_diagnostic":null}));
    assert_eq!(output["message_states"], json!([{"index":0,"state":"not_started"},{"index":1,"state":"not_started"},{"index":2,"state":"not_started"}]));

    let db = database().await;
    let state = Arc::new(Mutex::new(State { fail_final_audit: true, ..State::default() }));
    let output = run(&db, "terminal-audit", state, &messages(), 0).await;
    assert_eq!(output["outcome"], "unknown");
    assert_eq!(output["progress"], json!({"phase":"final_audit","operation":"parent_audit","message_index":null,"status":"stopped","failure":"audit","contact_diagnostic":null}));
    assert_eq!(output["message_states"], json!([{"index":0,"state":"observed"},{"index":1,"state":"observed"},{"index":2,"state":"observed"}]));
}

#[tokio::test]
async fn contact_diagnostics_are_sanitized_and_stop_before_drafts() {
    for (dispatch, code, expected_dispatch, expected_code) in [
        (native::ContactDispatch::NotDispatched, native::ContactCode::ContactTargetUnavailable, "not_dispatched", "contact_target_unavailable"),
        (native::ContactDispatch::Dispatched, native::ContactCode::WindowChanged, "dispatched", "window_changed"),
    ] {
        let db = database().await;
        let state = Arc::new(Mutex::new(State {
            contact_error: Some(native::ContactError { dispatch, code }),
            ..State::default()
        }));
        let output = run(&db, "contact-diagnostic", Arc::clone(&state), &messages(), 0).await;
        assert_eq!(output["outcome"], "unknown");
        assert_eq!(output["progress"]["contact_diagnostic"], json!({"dispatch":expected_dispatch,"code":expected_code}));
        assert_eq!(output["message_states"], json!([{"index":0,"state":"not_started"},{"index":1,"state":"not_started"},{"index":2,"state":"not_started"}]));
        assert!(!output["progress"].to_string().contains("private OS detail"));
        assert!(state.lock().unwrap().events.iter().all(|e| !e.starts_with("draft:") && !e.starts_with("send:")));
    }
}

#[tokio::test]
async fn contact_diagnostic_survives_terminal_audit_failure() {
    let db = database().await;
    let state = Arc::new(Mutex::new(State {
        fail_final_audit: true,
        contact_error: Some(native::ContactError {
            dispatch: native::ContactDispatch::Dispatched,
            code: native::ContactCode::ConversationHeaderUnconfirmed,
        }),
        ..State::default()
    }));
    let output = run(&db, "contact-terminal-audit", Arc::clone(&state), &messages(), 0).await;
    assert_eq!(output["outcome"], "unknown");
    assert_eq!(output["progress"], json!({"phase":"final_audit","operation":"parent_audit","message_index":null,"status":"stopped","failure":"audit","contact_diagnostic":{"dispatch":"dispatched","code":"conversation_header_unconfirmed"}}));
    assert_eq!(output["message_states"], json!([{"index":0,"state":"not_started"},{"index":1,"state":"not_started"},{"index":2,"state":"not_started"}]));
    assert!(!output["progress"].to_string().contains("private OS detail"));
    assert!(state.lock().unwrap().events.iter().all(|e| !e.starts_with("draft:") && !e.starts_with("send:")));
}

#[tokio::test]
async fn popup_preflight_and_binding_failures_stop_before_contact_or_draft() {
    for case in 0..4 {
        let db = database().await;
        let error = native::ContactError {
            dispatch: native::ContactDispatch::NotDispatched,
            code: native::ContactCode::PopupUnavailable,
        };
        let mut model = State::default();
        match case {
            0 => model.precheck_error = Some(error),
            1 => model.reload_error = Some(error),
            2 => model.changed_after_vision = true,
            _ => model.missing_prediction = true,
        }
        let state = Arc::new(Mutex::new(model));
        let output = run(&db, &format!("popup-failure-{case}"), Arc::clone(&state), &messages(), 0).await;
        assert_eq!(output["outcome"], "unknown");
        let events = &state.lock().unwrap().events;
        assert!(events.iter().all(|e| e != "contact" && !e.starts_with("draft:") && !e.starts_with("send:")));
        if case == 0 {
            assert!(!events.iter().any(|e| e == &format!("vision:{}", contact_query(NAME))));
        }
        if case < 2 {
            assert_eq!(output["progress"]["contact_diagnostic"], json!({"dispatch":"not_dispatched","code":"popup_unavailable"}));
        } else if case == 2 {
            assert_eq!(output["progress"]["contact_diagnostic"], json!({"dispatch":"not_dispatched","code":"contact_capture_changed"}));
        } else {
            assert_eq!(output["progress"]["contact_diagnostic"], json!({"dispatch":"not_dispatched","code":"contact_operation_failed"}));
        }
        assert!(!output.to_string().contains("private OS detail"));
    }
}



#[tokio::test]
async fn mixed_messages_use_pinned_bytes_and_stop_after_unknown_attachment() {
    let root = std::env::temp_dir().join(format!("desk-agent-test-{}", uuid::Uuid::new_v4()));
    std::fs::create_dir(&root).unwrap();
    let path = root.join("report.pdf");
    std::fs::write(&path, b"%PDF-1.7\nisolated attachment fixture").unwrap();
    let mut list = messages()[..1].to_vec();
    list.push(Message::File {
        path: path.to_str().unwrap().into(),
        filename: "report.pdf".into(),
        mime_type: "application/pdf".into(),
    });
    list.push(messages()[1].clone());
    let db = database().await;
    let state = Arc::new(Mutex::new(State { fail_send: Some(1), ..State::default() }));
    let output = run(&db, "mixed-unknown", Arc::clone(&state), &list, 0).await;
    assert_eq!(output["completed_count"], 1);
    assert_eq!(output["unknown_index"], 1);
    assert_eq!(output["outcomes"][1]["submitted"], Value::Null);
    let s = state.lock().unwrap();
    assert_eq!(s.history, ["first exact text"]);
    assert!(!s.events.iter().any(|event| event == "draft:2" || event == "send:2"));
    let staged = s.attachment_paths[0].clone();
    assert_ne!(staged, path);
    assert_eq!(std::fs::read(&staged).unwrap(), std::fs::read(&path).unwrap());
    drop(s);
    let audits: Vec<String> = sqlx::query_scalar("SELECT parameters FROM capability_audit").fetch_all(&db).await.unwrap();
    assert!(audits.iter().all(|row| !row.contains("report.pdf") && !row.contains("application/pdf") && !row.contains("%PDF")));
    std::fs::remove_dir_all(staged.parent().unwrap().parent().unwrap()).unwrap();
    std::fs::remove_dir_all(root).unwrap();
}

#[tokio::test]
async fn changed_snapshot_cannot_reach_attachment_return_or_next_message() {
    let root = std::env::temp_dir().join(format!("desk-agent-test-{}", uuid::Uuid::new_v4()));
    std::fs::create_dir(&root).unwrap();
    let path = root.join("original.bin");
    std::fs::write(&path, b"original payload").unwrap();
    let list = vec![
        Message::File { path: path.to_str().unwrap().into(), filename: "original.bin".into(), mime_type: "application/octet-stream".into() },
        messages()[0].clone(),
    ];
    let db = database().await;
    let state = Arc::new(Mutex::new(State { tamper_attachment_after_paste: true, ..State::default() }));
    let output = run(&db, "changed-media", Arc::clone(&state), &list, 0).await;
    assert_eq!(output["outcome"], "unknown");
    assert_eq!(output["completed_count"], 0);
    assert_eq!(output["outcomes"][0]["submitted"], false);
    let s = state.lock().unwrap();
    assert!(s.events.iter().any(|event| event == "draft:0"));
    assert!(!s.events.iter().any(|event| event.starts_with("send:") || event == "draft:1"));
    let staged = s.attachment_paths[0].clone();
    drop(s);
    std::fs::remove_dir_all(staged.parent().unwrap().parent().unwrap()).unwrap();
    std::fs::remove_dir_all(root).unwrap();
}

#[tokio::test]
async fn old_text_grant_cannot_authorize_attachment_or_read_its_source() {
    let db = database().await;
    sqlx::query("INSERT INTO local_capability_grants VALUES('wecom',?,'macos_gui',?,'send_text_messages','instruction:send_message_by_nickname:v1')")
        .bind(SCOPE.0).bind(SCOPE.1).execute(&db).await.unwrap();
    let messages = vec![Message::File {
        path: "/nonexistent/desk-agent-private.pdf".into(),
        filename: "desk-agent-private.pdf".into(),
        mime_type: "application/pdf".into(),
    }];
    let state = Arc::new(Mutex::new(State::default()));
    let result = invoke_backend(&db, "old-media-grant", NAME, &messages, 0, Model {
        state: Arc::clone(&state), db: db.clone(), request: "old-media-grant".into(),
    }).await;
    assert_eq!(result, Err(InvokeError::Forbidden));
    assert!(state.lock().unwrap().events.is_empty());
    let remaining: i64 = sqlx::query_scalar("SELECT count(*) FROM local_capability_grants").fetch_one(&db).await.unwrap();
    assert_eq!(remaining, 1);
}

#[tokio::test]
async fn invalid_attachment_consumes_authorization_but_never_launches() {
    let db = database().await;
    let messages = vec![Message::File {
        path: "/nonexistent/desk-agent-private.pdf".into(),
        filename: "desk-agent-private.pdf".into(),
        mime_type: "application/pdf".into(),
    }];
    grant(&db, NAME, &messages, 0).await;
    let state = Arc::new(Mutex::new(State::default()));
    let result = invoke_backend(&db, "missing-media", NAME, &messages, 0, Model {
        state: Arc::clone(&state), db: db.clone(), request: "missing-media".into(),
    }).await;
    assert_eq!(result, Err(InvokeError::InvalidInput));
    assert!(state.lock().unwrap().events.is_empty());
    assert!(!send_message_by_nickname_any_grant(&db).await.unwrap());
}
