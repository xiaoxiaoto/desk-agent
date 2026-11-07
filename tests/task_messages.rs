use desk_agent::{kernel::task::{TaskInput, TaskInstruction, TaskRequest}, runtime::invoke_task};

async fn runtime_database() -> sqlx::SqlitePool {
    let pool = sqlx::sqlite::SqlitePoolOptions::new().max_connections(1).connect("sqlite::memory:").await.unwrap();
    sqlx::query("CREATE TABLE local_capability_grants (app TEXT NOT NULL, capability TEXT NOT NULL, route TEXT NOT NULL, target TEXT NOT NULL, effect TEXT NOT NULL, parameters TEXT NOT NULL)").execute(&pool).await.unwrap();
    sqlx::query("CREATE TABLE capability_audit (id INTEGER PRIMARY KEY, request_id TEXT NOT NULL, app TEXT NOT NULL, capability TEXT NOT NULL, route TEXT NOT NULL, target TEXT NOT NULL, effect TEXT NOT NULL, parameters TEXT NOT NULL, decision TEXT NOT NULL)").execute(&pool).await.unwrap();
    pool
}

#[tokio::test]
async fn string_messages_dispatch_and_multiple_messages_are_not_silently_ignored() {
    let request: TaskRequest = serde_json::from_str(r#"{"protocol_version":"1","app":"wecom","messages":["请打开企业微信"]}"#).unwrap();
    let input = request.into_task_input().unwrap();
    input.preflight().unwrap();
    let result = invoke_task(None, "launch", input, || Ok(()), || Ok(())).await.unwrap();
    assert_eq!(result, (TaskInstruction::LaunchApp, None));

    let request: TaskRequest = serde_json::from_str(r#"{"protocol_version":"1","app":"wecom","messages":["请打开企业微信","其他要求"]}"#).unwrap();
    let input = request.into_task_input().unwrap();
    assert_eq!(input, TaskInput::NaturalLanguage { app: "wecom".into(), messages: vec!["请打开企业微信".into(), "其他要求".into()] });
    assert!(input.preflight().is_err());
}

#[test]
fn wake_wecom_resolves_without_accepting_extra_actions() {
    for message in ["唤醒企业微信", "请唤醒企业微信"] {
        let input = TaskInput::NaturalLanguage { app: "wecom".into(), messages: vec![message.into()] };
        assert_eq!(input.resolve_instruction().unwrap(), TaskInstruction::LaunchApp);
    }
    let input = TaskInput::NaturalLanguage { app: "wecom".into(), messages: vec!["唤醒企业微信并发送消息".into()] };
    assert!(input.preflight().is_err());
}


#[test]
fn center_window_accepts_exact_and_unambiguous_natural_language() {
    let request: TaskRequest = serde_json::from_str(r#"{"protocol_version":"1","app":"wecom","instructions":[{"action":"center_app_window"}]}"#).unwrap();
    let input = request.into_task_input().unwrap();
    assert_eq!(input.resolve_instruction().unwrap(), TaskInstruction::CenterAppWindow);
    input.preflight().unwrap();
    let input = TaskInput::NaturalLanguage { app: "wecom".into(), messages: vec!["将企业微信窗口居中".into()] };
    assert_eq!(input.resolve_instruction().unwrap(), TaskInstruction::CenterAppWindow);
    let input = TaskInput::NaturalLanguage { app: "wecom".into(), messages: vec!["将企业微信窗口居中并发送消息".into()] };
    assert!(input.preflight().is_err());
}

#[test]
fn app_window_capture_accepts_new_action_and_rejects_legacy_actions() {
    let request: TaskRequest = serde_json::from_str(r#"{"protocol_version":"1","app":"wecom","instructions":[{"action":"capture_app_window"}]}"#).unwrap();
    let input = request.into_task_input().unwrap();
    assert_eq!(input.resolve_instruction().unwrap(), TaskInstruction::CaptureAppWindow);
    input.preflight().unwrap();
    for action in ["capture_screen", "capture_wecom_window", "locate_screen_objects"] {
        let body = format!(r#"{{"protocol_version":"1","app":"wecom","instructions":[{{"action":"{action}"}}]}}"#);
        assert!(serde_json::from_str::<TaskRequest>(&body).is_err());
    }
    let request: TaskRequest = serde_json::from_str(r#"{"protocol_version":"1","app":"wecom","instructions":[{"action":"detection_app_window_objects","params":{"objects":["button"]}}]}"#).unwrap();
    assert_eq!(request.into_task_input().unwrap().resolve_instruction().unwrap(), TaskInstruction::DetectionAppWindowObjects { objects: vec!["button".into()] });
    assert!(serde_json::from_str::<TaskRequest>(r#"{"protocol_version":"1","app":"wecom","instructions":[{"action":"locate_app_window_objects","params":{"objects":["button"]}}]}"#).is_err());
    assert!(serde_json::from_str::<TaskRequest>(r#"{"protocol_version":"1","app":"wecom","instructions":[{"action":"locate_screen_objects","params":{"objects":["button"]}}]}"#).is_err());
    let input = TaskInput::NaturalLanguage { app: "wecom".into(), messages: vec!["截取企业微信窗口".into()] };
    assert_eq!(input.resolve_instruction().unwrap(), TaskInstruction::CaptureAppWindow);
    input.preflight().unwrap();
    let input = TaskInput::NaturalLanguage { app: "wecom".into(), messages: vec!["截取屏幕".into()] };
    assert!(input.preflight().is_err());
    assert!(serde_json::from_str::<TaskRequest>(r#"{"protocol_version":"1","app":"other","instructions":[{"action":"capture_app_window"}]}"#).unwrap().into_task_input().is_ok());
}

#[tokio::test]
async fn app_window_capture_requires_its_own_grant_and_audit_storage() {
    use desk_agent::runtime::{app_window_capture_authorized, InvokeError};
    let database = runtime_database().await;
    let input = TaskInput::ExactInstruction { app: "wecom".into(), steps: vec![TaskInstruction::CaptureAppWindow] };
    input.preflight().unwrap();
    for (app, capability, route, target, effect, parameters) in [
        ("wecom", "wecom.window.center", "macos_gui", "main_or_single_wecom_window", "move_without_resize", "none:v1"),
        ("wecom", "wecom.window.capture", "macos_gui", "main_or_single_wecom_window", "capture", "none:v1"),
        ("wecom", "desktop.screen.capture", "macos_gui", "primary_display", "capture", "none:v1"),
        ("other", "desktop.window.capture", "macos_gui", "main_or_single_app_window", "capture", "none:v1"),
        ("wecom", "desktop.window.capture", "other_route", "main_or_single_app_window", "capture", "none:v1"),
        ("wecom", "desktop.window.capture", "macos_gui", "primary_display", "capture", "none:v1"),
        ("wecom", "desktop.window.capture", "macos_gui", "main_or_single_app_window", "other", "none:v1"),
        ("wecom", "desktop.window.capture", "macos_gui", "main_or_single_app_window", "capture", "other"),
    ] {
        sqlx::query("INSERT INTO local_capability_grants VALUES (?, ?, ?, ?, ?, ?)").bind(app).bind(capability).bind(route).bind(target).bind(effect).bind(parameters).execute(&database).await.unwrap();
        assert!(!app_window_capture_authorized(&database).await.unwrap());
    }
    assert_eq!(invoke_task(Some(&database), "capture-denied", input.clone(), || Ok(()), || Ok(())).await, Err(InvokeError::Forbidden));
    let audit: (String, String, String, String) = sqlx::query_as("SELECT capability, target, effect, decision FROM capability_audit WHERE request_id = 'capture-denied' ORDER BY id DESC LIMIT 1").fetch_one(&database).await.unwrap();
    assert_eq!(audit, ("desktop.window.capture".into(), "main_or_single_app_window".into(), "capture".into(), "denied".into()));
    sqlx::query("INSERT INTO local_capability_grants VALUES ('wecom', 'desktop.window.capture', 'macos_gui', 'main_or_single_app_window', 'capture', 'none:v1')").execute(&database).await.unwrap();
    assert!(app_window_capture_authorized(&database).await.unwrap());
    sqlx::query("DELETE FROM local_capability_grants WHERE app = 'wecom' AND capability = 'desktop.window.capture' AND route = 'macos_gui' AND target = 'main_or_single_app_window' AND effect = 'capture' AND parameters = 'none:v1'").execute(&database).await.unwrap();
    assert!(!app_window_capture_authorized(&database).await.unwrap());
    sqlx::query("INSERT INTO local_capability_grants VALUES ('wecom', 'desktop.window.capture', 'macos_gui', 'main_or_single_app_window', 'capture', 'none:v1')").execute(&database).await.unwrap();
    sqlx::query("DROP TABLE capability_audit").execute(&database).await.unwrap();
    assert_eq!(invoke_task(Some(&database), "capture-no-audit", input, || Ok(()), || Ok(())).await, Err(InvokeError::AuditUnavailable));
}

#[tokio::test]
async fn detection_requires_separate_exact_grants_and_durable_audit() {
    use desk_agent::runtime::{app_window_locate_parameters, InvokeError};
    let database = runtime_database().await;
    let objects = vec!["button".to_owned(), "聊天".to_owned()];
    let input = TaskInput::ExactInstruction { app: "wecom".into(), steps: vec![TaskInstruction::DetectionAppWindowObjects { objects: objects.clone() }] };
    let digest = app_window_locate_parameters(&objects).unwrap();
    assert_eq!(digest, "objects-sha256:40f180e80fa146c779b2f5c060cda3821acd9c408c3894384e3bd95e6a79ec2d");
    assert_ne!(digest, app_window_locate_parameters(&["聊天".into(), "button".into()]).unwrap());
    sqlx::query("INSERT INTO local_capability_grants VALUES ('wecom', 'desktop.window.capture', 'macos_gui', 'main_or_single_app_window', 'capture', 'none:v1')").execute(&database).await.unwrap();
    sqlx::query("INSERT INTO local_capability_grants VALUES ('wecom', 'vision.screen.locate', 'local_vision', 'primary_display', 'local_inference', ?)").bind(&digest).execute(&database).await.unwrap();
    for (route, target, effect, parameters) in [
        ("macos_gui", "main_or_single_app_window", "local_inference", digest.as_str()),
        ("local_vision", "primary_display", "local_inference", digest.as_str()),
        ("local_vision", "main_or_single_app_window", "capture", digest.as_str()),
        ("local_vision", "main_or_single_app_window", "local_inference", "objects-sha256:wrong"),
    ] {
        sqlx::query("INSERT INTO local_capability_grants VALUES ('wecom', 'vision.window.locate', ?, ?, ?, ?)").bind(route).bind(target).bind(effect).bind(parameters).execute(&database).await.unwrap();
    }
    assert_eq!(invoke_task(Some(&database), "vision-denied", input.clone(), || Ok(()), || Ok(())).await, Err(InvokeError::Forbidden));
    let audit: (String, String, String, String, String, String) = sqlx::query_as("SELECT capability, route, target, effect, parameters, decision FROM capability_audit WHERE request_id = 'vision-denied' ORDER BY id DESC LIMIT 1").fetch_one(&database).await.unwrap();
    assert_eq!(audit, ("vision.window.locate".into(), "local_vision".into(), "main_or_single_app_window".into(), "local_inference".into(), digest.clone(), "denied".into()));
    let captures: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM capability_audit WHERE request_id = 'vision-denied' AND capability = 'desktop.window.capture'").fetch_one(&database).await.unwrap();
    assert_eq!(captures, 0);

    sqlx::query("INSERT INTO local_capability_grants VALUES ('wecom', 'vision.window.locate', 'local_vision', 'main_or_single_app_window', 'local_inference', ?)").bind(&digest).execute(&database).await.unwrap();
    sqlx::query("DELETE FROM local_capability_grants WHERE capability = 'desktop.window.capture'").execute(&database).await.unwrap();
    assert_eq!(invoke_task(Some(&database), "capture-denied-for-vision", input.clone(), || Ok(()), || Ok(())).await, Err(InvokeError::Forbidden));
    let audit: (String, String) = sqlx::query_as("SELECT capability, decision FROM capability_audit WHERE request_id = 'capture-denied-for-vision' ORDER BY id DESC LIMIT 1").fetch_one(&database).await.unwrap();
    assert_eq!(audit, ("desktop.window.capture".into(), "denied".into()));
    let dispatches: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM capability_audit WHERE request_id = 'capture-denied-for-vision' AND decision = 'dispatch_authorized'").fetch_one(&database).await.unwrap();
    assert_eq!(dispatches, 0);

    sqlx::query("DROP TABLE capability_audit").execute(&database).await.unwrap();
    assert_eq!(invoke_task(Some(&database), "vision-no-audit", input.clone(), || Ok(()), || Ok(())).await, Err(InvokeError::AuditUnavailable));
    assert_eq!(invoke_task(None, "vision-no-storage", input, || Ok(()), || Ok(())).await, Err(InvokeError::AuditUnavailable));
}

#[tokio::test]
async fn conversion_requires_exact_request_grant_and_audits_before_loading() {
    use desk_agent::runtime::{app_window_convert_parameters, InvokeError};
    let database = runtime_database().await;
    let id = "123e4567-e89b-12d3-a456-426614174000";
    let boxes = vec![[0.0, 1.0, 20.0, 30.0], [2.5, 3.0, 4.0, 5.0]];
    let input = TaskInput::ExactInstruction { app: "wecom".into(), steps: vec![TaskInstruction::ConvertAppWindowCoordinates { capture_id: id.into(), boxes: boxes.clone() }] };
    let digest = app_window_convert_parameters(id, &boxes).unwrap();
    assert_eq!(digest, "conversion-sha256:6f1ab1a8bb6bdcc8f15cd641d56fffd2860b3c85a0c62a3911c4512cefe57000");
    assert_ne!(digest, app_window_convert_parameters(id, &[boxes[1], boxes[0]]).unwrap());
    assert_ne!(digest, app_window_convert_parameters("123e4567-e89b-12d3-a456-426614174001", &boxes).unwrap());
    for (app, capability, route, target, effect, parameters) in [
        ("other", "desktop.window.coordinates.convert", "macos_gui", "main_or_single_app_window", "read_geometry", digest.as_str()),
        ("wecom", "desktop.window.capture", "macos_gui", "main_or_single_app_window", "capture", "none:v1"),
        ("wecom", "desktop.window.coordinates.convert", "other", "main_or_single_app_window", "read_geometry", digest.as_str()),
        ("wecom", "desktop.window.coordinates.convert", "macos_gui", "other", "read_geometry", digest.as_str()),
        ("wecom", "desktop.window.coordinates.convert", "macos_gui", "main_or_single_app_window", "other", digest.as_str()),
        ("wecom", "desktop.window.coordinates.convert", "macos_gui", "main_or_single_app_window", "read_geometry", "conversion-sha256:wrong"),
    ] {
        sqlx::query("INSERT INTO local_capability_grants VALUES (?, ?, ?, ?, ?, ?)").bind(app).bind(capability).bind(route).bind(target).bind(effect).bind(parameters).execute(&database).await.unwrap();
    }
    assert_eq!(invoke_task(Some(&database), "convert-denied", input.clone(), || Ok(()), || Ok(())).await, Err(InvokeError::Forbidden));
    let audit: (String, String, String, String, String, String) = sqlx::query_as("SELECT capability, route, target, effect, parameters, decision FROM capability_audit WHERE request_id = 'convert-denied' ORDER BY id DESC LIMIT 1").fetch_one(&database).await.unwrap();
    assert_eq!(audit, ("desktop.window.coordinates.convert".into(), "macos_gui".into(), "main_or_single_app_window".into(), "read_geometry".into(), digest.clone(), "denied".into()));
    sqlx::query("INSERT INTO local_capability_grants VALUES ('wecom', 'desktop.window.coordinates.convert', 'macos_gui', 'main_or_single_app_window', 'read_geometry', ?)").bind(&digest).execute(&database).await.unwrap();
    // A nonexistent (or expired) capture cannot be converted, even with the exact grant.
    assert_eq!(invoke_task(Some(&database), "convert-missing", input.clone(), || Ok(()), || Ok(())).await, Err(InvokeError::CapabilityUnavailable));
    let decisions: Vec<String> = sqlx::query_scalar("SELECT decision FROM capability_audit WHERE request_id = 'convert-missing' ORDER BY id").fetch_all(&database).await.unwrap();
    assert_eq!(decisions, vec!["authorization_check", "grant_allowed", "authorization_check", "dispatch_authorized", "dispatch_unavailable"]);
    let other_operations: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM capability_audit WHERE request_id = 'convert-missing' AND capability != 'desktop.window.coordinates.convert'").fetch_one(&database).await.unwrap();
    assert_eq!(other_operations, 0);
    sqlx::query("DROP TABLE capability_audit").execute(&database).await.unwrap();
    assert_eq!(invoke_task(Some(&database), "convert-no-audit", input, || Ok(()), || Ok(())).await, Err(InvokeError::AuditUnavailable));
}

#[test]
fn app_element_requests_accept_explicit_identity_modes_and_reject_capture_id() {
    for action in ["click_app_element", "inspect_app_element"] {
        for (label_field, expected_label) in [
            (r#","expected_label":"label""#, Some("label".to_owned())),
            ("", None),
            (r#","expected_label":null"#, None),
        ] {
            let body = format!(r#"{{"protocol_version":"1","app":"wecom","instructions":[{{"action":"{action}","params":{{"point":[1.0,2.0],"expected_role":"AXButton"{label_field}}}}}]}}"#);
            let request: TaskRequest = serde_json::from_str(&body).unwrap();
            let instruction = request.into_task_input().unwrap().resolve_instruction().unwrap();
            let expected = if action == "click_app_element" {
                TaskInstruction::ClickAppElement { point: [1.0, 2.0], expected_role: "AXButton".into(), expected_label }
            } else {
                TaskInstruction::InspectAppElement { point: [1.0, 2.0], expected_role: "AXButton".into(), expected_label }
            };
            assert_eq!(instruction, expected);
        }
        let old_body = format!(r#"{{"protocol_version":"1","app":"wecom","instructions":[{{"action":"{action}","params":{{"capture_id":"123e4567-e89b-12d3-a456-426614174000","point":[1.0,2.0],"expected_role":"AXButton","expected_label":"label"}}}}]}}"#);
        assert!(serde_json::from_str::<TaskRequest>(&old_body).is_err());
    }
}

#[tokio::test]
async fn app_element_catalog_accepts_only_complete_v3_digests() {
    use desk_agent::runtime::{app_element_activation_any_grant, app_element_diagnosis_any_grant};
    for (capability, effect, prefix) in [
        ("wecom.element.activate", "activate_element", "activate-v3-sha256:"),
        ("wecom.element.diagnose", "inspect_element", "diagnose-v3-sha256:"),
    ] {
        let db = runtime_database().await;
        assert_eq!(prefix.len(), 19);
        for invalid in [
            format!("{prefix}{}", "0".repeat(63)),
            format!("{prefix}{}", "0".repeat(65)),
            format!("{prefix}{}", "A".repeat(64)),
            format!("{prefix}{}g", "0".repeat(63)),
        ] {
            sqlx::query("INSERT INTO local_capability_grants VALUES ('wecom', ?, 'macos_gui', 'main_or_single_app_window', ?, ?)").bind(capability).bind(effect).bind(&invalid).execute(&db).await.unwrap();
            assert!(!app_element_activation_any_grant(&db).await.unwrap());
            assert!(!app_element_diagnosis_any_grant(&db).await.unwrap());
        }
        let valid = format!("{prefix}{}", "0".repeat(64));
        assert_eq!(valid.len(), 83);
        sqlx::query("INSERT INTO local_capability_grants VALUES ('wecom', ?, 'macos_gui', 'main_or_single_app_window', ?, ?)").bind(capability).bind(effect).bind(valid).execute(&db).await.unwrap();
        assert_eq!(app_element_activation_any_grant(&db).await.unwrap(), capability == "wecom.element.activate");
        assert_eq!(app_element_diagnosis_any_grant(&db).await.unwrap(), capability == "wecom.element.diagnose");
    }
}

#[tokio::test]
async fn activation_requires_exact_one_shot_grant_and_live_preflight() {
    use desk_agent::runtime::{app_element_activation_any_grant, app_element_activation_parameters, InvokeError};
    use sha2::{Digest, Sha256};
    let db = runtime_database().await;
    let point = [1.0, 2.0];
    let digest = app_element_activation_parameters(point, "AXButton", Some("private label")).unwrap();
    let encoded = br#"{"point":[1.0,2.0],"expected_role":"AXButton","expected_label":"private label"}"#;
    assert_eq!(digest, format!("activate-v3-sha256:{:x}", Sha256::digest(encoded)));
    let coordinate_digest = app_element_activation_parameters(point, "AXButton", None).unwrap();
    let coordinate_encoded = br#"{"point":[1.0,2.0],"expected_role":"AXButton","expected_label":null}"#;
    assert_eq!(coordinate_digest, format!("activate-v3-sha256:{:x}", Sha256::digest(coordinate_encoded)));
    assert_ne!(digest, coordinate_digest);
    assert_ne!(coordinate_digest, app_element_activation_parameters(point, "AXButton", Some("")).unwrap());
    assert_ne!(coordinate_digest, format!("activate-v3-sha256:{:x}", Sha256::digest(br#"{"point":[1.0,2.0],"expected_role":"AXButton"}"#)));
    assert_ne!(digest, app_element_activation_parameters([2.0, 1.0], "AXButton", Some("private label")).unwrap());
    assert_ne!(digest, app_element_activation_parameters(point, "AXLink", Some("private label")).unwrap());
    assert_ne!(digest, app_element_activation_parameters(point, "AXButton", Some("other")).unwrap());
    let input = TaskInput::ExactInstruction { app: "wecom".into(), steps: vec![TaskInstruction::ClickAppElement { point, expected_role: "AXButton".into(), expected_label: Some("private label".into()) }] };
    let coordinate_input = TaskInput::ExactInstruction { app: "wecom".into(), steps: vec![TaskInstruction::ClickAppElement { point, expected_role: "AXButton".into(), expected_label: None }] };
    assert_eq!(invoke_task(Some(&db), "denied", input.clone(), || Ok(()), || Ok(())).await, Err(InvokeError::Forbidden));
    assert!(!app_element_activation_any_grant(&db).await.unwrap());
    for prefix in ["activate-sha256:", "activate-v2-sha256:"] {
        let legacy = format!("{prefix}{:x}", Sha256::digest(encoded));
        sqlx::query("INSERT INTO local_capability_grants VALUES ('wecom', 'wecom.element.activate', 'macos_gui', 'main_or_single_app_window', 'activate_element', ?)").bind(&legacy).execute(&db).await.unwrap();
        assert!(!app_element_activation_any_grant(&db).await.unwrap());
        for old_input in [input.clone(), coordinate_input.clone()] {
            assert_eq!(invoke_task(Some(&db), "legacy-policy", old_input, || Ok(()), || Ok(())).await, Err(InvokeError::Forbidden));
        }
    }
    sqlx::query("INSERT INTO local_capability_grants VALUES ('wecom', 'wecom.element.activate', 'macos_gui', 'main_or_single_app_window', 'activate_element', ?)").bind(&digest).execute(&db).await.unwrap();
    assert!(app_element_activation_any_grant(&db).await.unwrap());
    assert_eq!(invoke_task(Some(&db), "coordinate-denied", coordinate_input.clone(), || Ok(()), || Ok(())).await, Err(InvokeError::Forbidden));
    for (point, role, label) in [([2.0, 1.0], "AXButton", "private label"), (point, "AXLink", "private label"), (point, "AXButton", "other")] {
        let other = TaskInput::ExactInstruction { app: "wecom".into(), steps: vec![TaskInstruction::ClickAppElement { point, expected_role: role.into(), expected_label: Some(label.into()) }] };
        assert_eq!(invoke_task(Some(&db), "other", other, || Ok(()), || Ok(())).await, Err(InvokeError::Forbidden));
    }
    sqlx::query("DELETE FROM local_capability_grants WHERE parameters = ?").bind(&digest).execute(&db).await.unwrap();
    assert!(!app_element_activation_any_grant(&db).await.unwrap());
    sqlx::query("INSERT INTO local_capability_grants VALUES ('wecom', 'wecom.element.activate', 'macos_gui', 'main_or_single_app_window', 'activate_element', ?)").bind(&coordinate_digest).execute(&db).await.unwrap();
    assert!(app_element_activation_any_grant(&db).await.unwrap());
    assert_eq!(invoke_task(Some(&db), "label-denied", input.clone(), || Ok(()), || Ok(())).await, Err(InvokeError::Forbidden));
    sqlx::query("DELETE FROM local_capability_grants WHERE parameters = ?").bind(&coordinate_digest).execute(&db).await.unwrap();
    assert!(!app_element_activation_any_grant(&db).await.unwrap());
    assert_eq!(invoke_task(Some(&db), "revoked", coordinate_input, || Ok(()), || Ok(())).await, Err(InvokeError::Forbidden));
    // An unrepresentable native coordinate cannot pass preflight or consume the grant.
    for label in [Some("private label"), None] {
        let point = [f64::MAX, 2.0];
        let digest = app_element_activation_parameters(point, "AXButton", label).unwrap();
        let input = TaskInput::ExactInstruction { app: "wecom".into(), steps: vec![TaskInstruction::ClickAppElement { point, expected_role: "AXButton".into(), expected_label: label.map(str::to_owned) }] };
        sqlx::query("INSERT INTO local_capability_grants VALUES ('wecom', 'wecom.element.activate', 'macos_gui', 'main_or_single_app_window', 'activate_element', ?)").bind(&digest).execute(&db).await.unwrap();
        let diagnostic = match invoke_task(Some(&db), "unavailable", input.clone(), || Ok(()), || Ok(())).await {
            Err(InvokeError::ActivationPreflight(code)) => code,
            other => panic!("expected safe preflight diagnostic, got {other:?}"),
        };
        assert!(app_element_activation_any_grant(&db).await.unwrap());
        let decisions: Vec<String> = sqlx::query_scalar("SELECT decision FROM capability_audit WHERE request_id = 'unavailable' AND parameters = ? ORDER BY id").bind(&digest).fetch_all(&db).await.unwrap();
        assert_eq!(decisions, ["authorization_check".to_owned(), "grant_allowed".to_owned(), format!("not_dispatched_{}", diagnostic.as_str())]);
        let remains: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM local_capability_grants WHERE parameters = ?").bind(&digest).fetch_one(&db).await.unwrap();
        assert_eq!(remains, 1);
    }
    let raw_labels: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM capability_audit WHERE parameters LIKE '%private label%'").fetch_one(&db).await.unwrap();
    assert_eq!(raw_labels, 0);
    sqlx::query("DROP TABLE capability_audit").execute(&db).await.unwrap();
    assert_eq!(invoke_task(Some(&db), "no-audit", input, || Ok(()), || Ok(())).await, Err(InvokeError::AuditUnavailable));
    assert!(app_element_activation_any_grant(&db).await.unwrap());
}

#[tokio::test]
async fn diagnosis_requires_separate_exact_grant_and_preserves_activation_grant() {
    use desk_agent::runtime::{app_element_activation_parameters, app_element_diagnosis_any_grant, app_element_diagnosis_parameters, InvokeError};
    use sha2::{Digest, Sha256};
    let db = runtime_database().await;
    let point = [f64::MAX, 2.0];
    let role = "AXButton";
    let label = "private label";
    let activation = app_element_activation_parameters(point, role, Some(label)).unwrap();
    let diagnosis = app_element_diagnosis_parameters(point, role, Some(label)).unwrap();
    let coordinate_diagnosis = app_element_diagnosis_parameters(point, role, None).unwrap();
    let expected = format!(r#"{{"point":[{},2.0],"expected_role":"AXButton","expected_label":"private label"}}"#, serde_json::to_string(&point[0]).unwrap());
    assert_eq!(diagnosis, format!("diagnose-v3-sha256:{:x}", Sha256::digest(expected.as_bytes())));
    let coordinate_expected = format!(r#"{{"point":[{},2.0],"expected_role":"AXButton","expected_label":null}}"#, serde_json::to_string(&point[0]).unwrap());
    assert_eq!(coordinate_diagnosis, format!("diagnose-v3-sha256:{:x}", Sha256::digest(coordinate_expected.as_bytes())));
    assert_ne!(diagnosis, coordinate_diagnosis);
    assert_ne!(activation, diagnosis);
    assert_ne!(coordinate_diagnosis, app_element_activation_parameters(point, role, None).unwrap());
    let input = TaskInput::ExactInstruction { app: "wecom".into(), steps: vec![TaskInstruction::InspectAppElement { point, expected_role: role.into(), expected_label: Some(label.into()) }] };
    let coordinate_input = TaskInput::ExactInstruction { app: "wecom".into(), steps: vec![TaskInstruction::InspectAppElement { point, expected_role: role.into(), expected_label: None }] };
    input.preflight().unwrap();
    coordinate_input.preflight().unwrap();
    sqlx::query("INSERT INTO local_capability_grants VALUES ('wecom', 'wecom.element.activate', 'macos_gui', 'main_or_single_app_window', 'activate_element', ?)").bind(&activation).execute(&db).await.unwrap();
    assert_eq!(invoke_task(Some(&db), "diagnose-denied", input.clone(), || Ok(()), || Ok(())).await, Err(InvokeError::Forbidden));
    assert!(!app_element_diagnosis_any_grant(&db).await.unwrap());
    let legacy = format!("diagnose-v2-sha256:{:x}", Sha256::digest(expected.as_bytes()));
    sqlx::query("INSERT INTO local_capability_grants VALUES ('wecom', 'wecom.element.diagnose', 'macos_gui', 'main_or_single_app_window', 'inspect_element', ?)").bind(&legacy).execute(&db).await.unwrap();
    assert!(!app_element_diagnosis_any_grant(&db).await.unwrap());
    for old_input in [input.clone(), coordinate_input.clone()] {
        assert_eq!(invoke_task(Some(&db), "diagnose-legacy", old_input, || Ok(()), || Ok(())).await, Err(InvokeError::Forbidden));
    }
    sqlx::query("INSERT INTO local_capability_grants VALUES ('wecom', 'wecom.element.diagnose', 'macos_gui', 'main_or_single_app_window', 'inspect_element', ?)").bind(&diagnosis).execute(&db).await.unwrap();
    assert!(app_element_diagnosis_any_grant(&db).await.unwrap());
    assert_eq!(invoke_task(Some(&db), "diagnose-coordinate-denied", coordinate_input.clone(), || Ok(()), || Ok(())).await, Err(InvokeError::Forbidden));
    for (point, role, label) in [([2.0, 1.0], role, label), (point, "AXLink", label), (point, role, "other")] {
        let other = TaskInput::ExactInstruction { app: "wecom".into(), steps: vec![TaskInstruction::InspectAppElement { point, expected_role: role.into(), expected_label: Some(label.into()) }] };
        assert_eq!(invoke_task(Some(&db), "diagnose-other", other, || Ok(()), || Ok(())).await, Err(InvokeError::Forbidden));
    }
    sqlx::query("DELETE FROM local_capability_grants WHERE parameters = ?").bind(&diagnosis).execute(&db).await.unwrap();
    sqlx::query("INSERT INTO local_capability_grants VALUES ('wecom', 'wecom.element.diagnose', 'macos_gui', 'main_or_single_app_window', 'inspect_element', ?)").bind(&coordinate_diagnosis).execute(&db).await.unwrap();
    assert!(app_element_diagnosis_any_grant(&db).await.unwrap());
    assert_eq!(invoke_task(Some(&db), "diagnose-label-denied", input, || Ok(()), || Ok(())).await, Err(InvokeError::Forbidden));
    let diagnostic = match invoke_task(Some(&db), "diagnose-unavailable", coordinate_input, || Ok(()), || Ok(())).await {
        Err(InvokeError::ActivationPreflight(code)) => code,
        other => panic!("expected safe preflight diagnostic, got {other:?}"),
    };
    let decisions: Vec<String> = sqlx::query_scalar("SELECT decision FROM capability_audit WHERE request_id = 'diagnose-unavailable' ORDER BY id").fetch_all(&db).await.unwrap();
    assert_eq!(decisions, ["authorization_check".to_owned(), "grant_allowed".to_owned(), "authorization_check".to_owned(), "dispatch_authorized".to_owned(), format!("not_dispatched_{}", diagnostic.as_str())]);
    let grants: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM local_capability_grants WHERE parameters IN (?, ?)").bind(&activation).bind(&coordinate_diagnosis).fetch_one(&db).await.unwrap();
    assert_eq!(grants, 2);
    let raw_labels: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM capability_audit WHERE parameters LIKE '%private label%' OR decision LIKE '%private label%'").fetch_one(&db).await.unwrap();
    assert_eq!(raw_labels, 0);
}

#[tokio::test]
async fn center_window_is_not_dispatched_without_runtime_authorization() {
    use std::sync::{Arc, atomic::{AtomicBool, Ordering}};
    let database = runtime_database().await;
    let called = Arc::new(AtomicBool::new(false));
    let flag = called.clone();
    let input = TaskInput::ExactInstruction { app: "wecom".into(), steps: vec![TaskInstruction::CenterAppWindow] };
    assert_eq!(invoke_task(Some(&database), "denied", input, || Ok(()), move || { flag.store(true, Ordering::SeqCst); Ok(()) }).await, Err(desk_agent::runtime::InvokeError::Forbidden));
    assert!(!called.load(Ordering::SeqCst));
}

#[tokio::test]
async fn center_grant_is_exact_and_revocable_and_audit_failure_blocks_dispatch() {
    use desk_agent::runtime::{center_authorized, InvokeError};
    use std::sync::{Arc, atomic::{AtomicBool, Ordering}};
    let database = runtime_database().await;
    sqlx::query("INSERT INTO local_capability_grants VALUES ('wecom', 'wecom.window.center', 'other_route', 'main_or_single_wecom_window', 'move_without_resize', 'none:v1')").execute(&database).await.unwrap();
    assert!(!center_authorized(&database).await.unwrap());
    sqlx::query("INSERT INTO local_capability_grants VALUES ('wecom', 'wecom.window.center', 'macos_gui', 'main_or_single_wecom_window', 'move_without_resize', 'none:v1')").execute(&database).await.unwrap();
    assert!(center_authorized(&database).await.unwrap());
    sqlx::query("DELETE FROM local_capability_grants WHERE route = 'macos_gui'").execute(&database).await.unwrap();
    assert!(!center_authorized(&database).await.unwrap());
    let called = Arc::new(AtomicBool::new(false));
    let flag = called.clone();
    let input = TaskInput::ExactInstruction { app: "wecom".into(), steps: vec![TaskInstruction::CenterAppWindow] };
    assert_eq!(invoke_task(Some(&database), "revoked", input.clone(), || Ok(()), move || { flag.store(true, Ordering::SeqCst); Ok(()) }).await, Err(InvokeError::Forbidden));
    let decision: String = sqlx::query_scalar("SELECT decision FROM capability_audit WHERE request_id = 'revoked' ORDER BY id DESC LIMIT 1").fetch_one(&database).await.unwrap();
    assert_eq!(decision, "denied");
    sqlx::query("INSERT INTO local_capability_grants VALUES ('wecom', 'wecom.window.center', 'macos_gui', 'main_or_single_wecom_window', 'move_without_resize', 'none:v1')").execute(&database).await.unwrap();
    sqlx::query("DROP TABLE capability_audit").execute(&database).await.unwrap();
    let flag = called.clone();
    assert_eq!(invoke_task(Some(&database), "no-audit", input, || Ok(()), move || { flag.store(true, Ordering::SeqCst); Ok(()) }).await, Err(InvokeError::AuditUnavailable));
    assert!(!called.load(Ordering::SeqCst));
}
#[test]
fn rejects_old_objects_and_blank_messages() {
    assert!(serde_json::from_str::<TaskRequest>(r#"{"protocol_version":"1","app":"wecom","messages":[{"role":"user","content":"请打开企业微信"}]}"#).is_err());
    for messages in [r#"[]"#, r#"["  "]"#, r#"["ok", "\t"]"#] {
        let request: TaskRequest = serde_json::from_str(&format!(r#"{{"protocol_version":"1","app":"wecom","messages":{messages}}}"#)).unwrap();
        assert!(request.into_task_input().is_err());
    }
}
#[test]
fn rejects_unimplemented_legacy_recovery_shape() {
    let body = r#"{"protocol_version":"1","app":"wecom","messages":["请打开企业微信"],"recovery":{"task_id":"t","action":{"action":"resume","params":{"checkpoint_id":"c","expected_checkpoint_version":1}}}}"#;
    assert!(serde_json::from_str::<TaskRequest>(body).is_err());
}

#[test]
fn requires_supported_protocol_version_before_dispatch() {
    assert!(serde_json::from_str::<TaskRequest>(r#"{"app":"wecom","messages":["请打开企业微信"]}"#).is_err());
    let request: TaskRequest = serde_json::from_str(r#"{"protocol_version":"2","app":"wecom","messages":["请打开企业微信"]}"#).unwrap();
    assert!(request.into_task_input().is_err());
}

#[test]
fn uncertain_dispatch_is_not_reported_as_a_failed_or_verified_operation() {
    let response = desk_agent::kernel::task::TaskResponse::unknown(
        "request".into(),
        "wecom",
        desk_agent::kernel::task::TaskMode::ExactInstruction,
        TaskInstruction::LaunchApp,
    );
    let value = serde_json::to_value(response).unwrap();
    assert_eq!(value["execution"]["status"], "unknown");
    assert_eq!(value["execution"]["steps"][0]["status"], "unknown");
    assert_eq!(value["execution"]["steps"][0]["verification"]["status"], "unknown");
    assert_eq!(value["execution"]["steps"][0]["error"]["outcome"], "unknown");
    assert_eq!(value["execution"]["steps"][0]["error"]["retryable"], false);
}


#[test]
fn instructions_take_precedence_and_do_not_require_messages() {
    for body in [
        r#"{"protocol_version":"1","app":"wecom","instructions":[{"action":"launch_app"}]}"#,
        r#"{"protocol_version":"1","app":"wecom","messages":["  "],"instructions":[{"action":"launch_app"}]}"#,
        r#"{"protocol_version":"1","app":"wecom","messages":["do something else"],"instructions":[{"action":"launch_app"}]}"#,
    ] {
        let request: TaskRequest = serde_json::from_str(body).unwrap();
        assert_eq!(request.into_task_input().unwrap(), TaskInput::ExactInstruction { app: "wecom".into(),
            steps: vec![TaskInstruction::LaunchApp],
        });
    }
    for body in [
        r#"{"protocol_version":"1","app":"wecom"}"#,
        r#"{"protocol_version":"1","app":"wecom","messages":["请打开企业微信"],"instructions":[]}"#,
    ] {
        let request: TaskRequest = serde_json::from_str(body).unwrap();
        assert!(request.into_task_input().is_err());
    }
}

fn search_request(point: [f64; 2], query: &str) -> TaskRequest {
    serde_json::from_value(serde_json::json!({
        "protocol_version": "1",
        "app": "wecom",
        "instructions": [{ "action": "input_app_element", "params": { "point": point, "text": query } }]
    })).unwrap()
}

#[test]
fn search_query_preserves_exact_content_and_rejects_unsafe_request_fields() {
    let point = [-492.5, 203.25];
    for query in ["  张 三  ", "Alice \"研发\" \\", &"界".repeat(85), &format!("{}a", "界".repeat(85))] {
        let input = search_request(point, query).into_task_input().unwrap();
        input.preflight().unwrap();
        assert_eq!(input.resolve_instruction().unwrap(), TaskInstruction::InputAppElement { point, text: query.to_owned() });
    }
    let mut invalid_queries = vec!["".to_owned(), "   ".to_owned(), "\u{3000}".to_owned(), "a".repeat(257), format!("{}ab", "界".repeat(85))];
    for code in (0..=31).chain(127..=159) {
        invalid_queries.push(format!("before{}after", char::from_u32(code).unwrap()));
    }
    for query in invalid_queries {
        let accepted = search_request(point, &query).into_task_input().is_ok_and(|input| input.preflight().is_ok());
        assert!(!accepted, "unsafe query was executable");
    }
    for point in [[0.1, 203.0], [f64::MAX, 203.0]] {
        let accepted = search_request(point, "Alice").into_task_input().is_ok_and(|input| input.preflight().is_ok());
        assert!(!accepted);
    }
    for point in [[f64::NAN, 203.0], [492.0, f64::INFINITY], [f64::NEG_INFINITY, 203.0]] {
        let input = TaskInput::ExactInstruction { app: "wecom".into(), steps: vec![TaskInstruction::InputAppElement { point, text: "Alice".into() }] };
        assert!(input.preflight().is_err());
        assert!(input.resolve_instruction().is_err());
    }
    for params in [
        serde_json::json!({ "point": [492, 203] }),
        serde_json::json!({ "text": "Alice" }),
        serde_json::json!({ "point": [492, 203], "text": "Alice", "append": true }),
        serde_json::json!({ "point": [492, 203], "text": "Alice", "press_enter": true }),
        serde_json::json!({ "point": [492, 203, 0], "text": "Alice" }),
    ] {
        let request = serde_json::json!({ "protocol_version": "1", "app": "wecom", "instructions": [{ "action": "input_app_element", "params": params }] });
        assert!(serde_json::from_value::<TaskRequest>(request).is_err());
    }
    let mut request = serde_json::to_value(search_request(point, "Alice")).unwrap();
    request["app"] = serde_json::json!("other");
    assert!(serde_json::from_value::<TaskRequest>(request).unwrap().into_task_input().is_ok());
    let input = TaskInput::NaturalLanguage { app: "wecom".into(), messages: vec!["搜索企业微信联系人 Alice".into()] };
    assert!(input.preflight().is_err());
}

#[tokio::test]
async fn search_query_digest_binds_exact_point_and_query_and_uses_separate_authorization() {
    use desk_agent::runtime::{wecom_search_query_any_grant, wecom_search_query_parameters, InvokeError};
    use sha2::{Digest, Sha256};
    let db = runtime_database().await;
    let point = [492.0, 203.0];
    let query = "  私密 Alice  ";
    let digest = wecom_search_query_parameters(point, query).unwrap();
    let encoded = "{\"point\":[492.0,203.0],\"text\":\"  私密 Alice  \"}";
    assert_eq!(digest, format!("search-query-sha256:{:x}", Sha256::digest(encoded.as_bytes())));
    assert_ne!(digest, wecom_search_query_parameters([493.0, 203.0], query).unwrap());
    assert_ne!(digest, wecom_search_query_parameters(point, query.trim()).unwrap());
    assert_ne!(digest, wecom_search_query_parameters(point, "  私密 alice  ").unwrap());
    let input = search_request(point, query).into_task_input().unwrap();
    input.preflight().unwrap();
    for (app, capability, route, target, effect, parameters) in [
        ("wecom", "desktop.window.capture", "macos_gui", "main_or_single_app_window", "capture", "none:v1"),
        ("wecom", "wecom.element.activate", "macos_gui", "main_or_single_app_window", "activate_element", digest.as_str()),
        ("other", "wecom.search.query.set", "macos_gui", "wecom_search_field", "set_search_query", digest.as_str()),
        ("wecom", "wecom.search.query.set", "other", "wecom_search_field", "set_search_query", digest.as_str()),
        ("wecom", "wecom.search.query.set", "macos_gui", "other", "set_search_query", digest.as_str()),
        ("wecom", "wecom.search.query.set", "macos_gui", "wecom_search_field", "other", digest.as_str()),
        ("wecom", "wecom.search.query.set", "macos_gui", "wecom_search_field", "set_search_query", "search-query-sha256:wrong"),
    ] {
        sqlx::query("INSERT INTO local_capability_grants VALUES (?, ?, ?, ?, ?, ?)").bind(app).bind(capability).bind(route).bind(target).bind(effect).bind(parameters).execute(&db).await.unwrap();
    }
    assert!(!wecom_search_query_any_grant(&db).await.unwrap());
    assert_eq!(invoke_task(Some(&db), "search-denied", input.clone(), || Ok(()), || Ok(())).await, Err(InvokeError::Forbidden));
    let audit: (String, String, String, String, String, String) = sqlx::query_as("SELECT capability, route, target, effect, parameters, decision FROM capability_audit WHERE request_id = 'search-denied' ORDER BY id DESC LIMIT 1").fetch_one(&db).await.unwrap();
    assert_eq!(audit, ("wecom.search.query.set".into(), "macos_gui".into(), "wecom_search_field".into(), "set_search_query".into(), digest.clone(), "denied".into()));
    sqlx::query("INSERT INTO local_capability_grants VALUES ('wecom', 'wecom.search.query.set', 'macos_gui', 'wecom_search_field', 'set_search_query', ?)").bind(&digest).execute(&db).await.unwrap();
    assert!(wecom_search_query_any_grant(&db).await.unwrap());
    for (point, query) in [([493.0, 203.0], query), (point, query.trim()), (point, "different")] {
        let input = search_request(point, query).into_task_input().unwrap();
        assert_eq!(invoke_task(Some(&db), "search-mismatch", input, || Ok(()), || Ok(())).await, Err(InvokeError::Forbidden));
    }
    let leaked: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM capability_audit WHERE parameters LIKE '%Alice%' OR decision LIKE '%Alice%'").fetch_one(&db).await.unwrap();
    assert_eq!(leaked, 0);
    sqlx::query("DROP TABLE capability_audit").execute(&db).await.unwrap();
    assert_eq!(invoke_task(Some(&db), "search-audit-unavailable", input.clone(), || Ok(()), || Ok(())).await, Err(InvokeError::AuditUnavailable));
    assert!(wecom_search_query_any_grant(&db).await.unwrap());
    assert_eq!(invoke_task(None, "search-storage-unavailable", input, || Ok(()), || Ok(())).await, Err(InvokeError::AuditUnavailable));
}

#[tokio::test]
async fn search_query_catalog_requires_complete_lowercase_digest() {
    use desk_agent::runtime::wecom_search_query_any_grant;
    let db = runtime_database().await;
    for parameters in [
        format!("search-query-sha256:{}", "0".repeat(63)),
        format!("search-query-sha256:{}", "0".repeat(65)),
        format!("search-query-sha256:{}", "A".repeat(64)),
        format!("search-query-sha256:{}", "g".repeat(64)),
        format!("activate-v3-sha256:{}", "0".repeat(64)),
    ] {
        sqlx::query("INSERT INTO local_capability_grants VALUES ('wecom', 'wecom.search.query.set', 'macos_gui', 'wecom_search_field', 'set_search_query', ?)").bind(parameters).execute(&db).await.unwrap();
        assert!(!wecom_search_query_any_grant(&db).await.unwrap());
    }
    let parameters = format!("search-query-sha256:{}", "a".repeat(64));
    sqlx::query("INSERT INTO local_capability_grants VALUES ('wecom', 'wecom.search.query.set', 'macos_gui', 'wecom_search_field', 'set_search_query', ?)").bind(parameters).execute(&db).await.unwrap();
    assert!(wecom_search_query_any_grant(&db).await.unwrap());
}

#[test]
fn search_query_output_does_not_echo_query_or_claim_independent_verification() {
    use desk_agent::kernel::task::{TaskMode, TaskResponse};
    let query = "private-search-value";
    let instruction = search_request([492.0, 203.0], query).into_task_input().unwrap().resolve_instruction().unwrap();
    let response = TaskResponse::completed("request".into(), "wecom", TaskMode::ExactInstruction, instruction.clone(), serde_json::json!({ "text_input": true }));
    let value = serde_json::to_value(response).unwrap();
    assert_eq!(value["execution"]["steps"][0]["output"], serde_json::json!({ "text_input": true }));
    assert_eq!(value["execution"]["steps"][0]["verification"]["status"], "not_performed");
    assert_eq!(value["execution"]["steps"][0]["instruction"]["params"]["text"], query);
    let value = serde_json::to_value(TaskResponse::unknown("request".into(), "wecom", TaskMode::ExactInstruction, instruction)).unwrap();
    assert_eq!(value["execution"]["status"], "unknown");
    assert_eq!(value["execution"]["steps"][0]["error"]["retryable"], false);
    assert_eq!(value["execution"]["steps"][0]["instruction"]["params"]["text"], query);
}
