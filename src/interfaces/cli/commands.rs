use crate::interfaces::ipc::{self, IpcRequest, IpcResponse};
use crate::kernel::instruction::{AtomicInstruction, PerceptionInstruction, TaskInstruction};
#[cfg(test)]
use crate::kernel::instruction::ApplicationInstruction;
use crate::kernel::task::TaskRequest;
use anyhow::{bail, Context};
use clap::{Parser, Subcommand};

#[derive(Parser)]
#[command(name = "desk-agent-cli", about = "DeskAgent local runtime client")]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    Status,
    Capabilities,
    Serve,
    /// Submit a task; repeat --message and --instruction to preserve order.
    Rpa {
        #[arg(long)]
        app: String,
        #[arg(long, required_unless_present = "instruction")]
        message: Vec<String>,
        /// process_launch; window_center; window_capture; vision_detect=JSON_ARRAY; screenshot_coordinates_convert=JSON_OBJECT; click_button=JSON_OBJECT; click_focus=JSON_OBJECT; ax_inspect=JSON_OBJECT; keyboard_text_input=JSON_OBJECT; keyboard_key_input=JSON_OBJECT; send_message_by_nickname=JSON_OBJECT
        #[arg(long)]
        instruction: Vec<String>,
    },
}

fn instruction(value: &str) -> anyhow::Result<TaskInstruction> {
    if value == "process_launch" { return Ok(TaskInstruction::Atomic(AtomicInstruction::ProcessLaunch)); }
    if value == "window_center" { return Ok(TaskInstruction::Atomic(AtomicInstruction::WindowCenter)); }
    if value == "window_capture" { return Ok(TaskInstruction::Atomic(AtomicInstruction::WindowCapture)); }
    if let Some(params) = value.strip_prefix("send_message_by_nickname=") {
        return serde_json::from_str(&format!(r#"{{"action":"send_message_by_nickname","params":{params}}}"#))
            .context("send_message_by_nickname requires nickname, typed messages and interval_ms");
    }
    if let Some(objects) = value.strip_prefix("vision_detect=") {
        return Ok(TaskInstruction::Perception(PerceptionInstruction::VisionDetect { objects: serde_json::from_str(objects).context("vision_detect requires a JSON array of strings")?, }));
    }
    if let Some(params) = value.strip_prefix("screenshot_coordinates_convert=") {
        #[derive(serde::Deserialize)]
        #[serde(deny_unknown_fields)]
        struct Params {
            capture_id: String,
            boxes: Vec<[f64; 4]>,
        }
        let params: Params = serde_json::from_str(params)
            .context("screenshot_coordinates_convert requires a JSON object with capture_id and boxes")?;
        return Ok(TaskInstruction::Perception(PerceptionInstruction::ScreenshotCoordinatesConvert { capture_id: params.capture_id,
        boxes: params.boxes, }));
    }
    if let Some((action, params)) = value.split_once('=').filter(|(action, _)| matches!(*action, "click_button" | "click_focus" | "ax_inspect")) {
        #[derive(serde::Deserialize)]
        #[serde(deny_unknown_fields)]
        struct Params {
            point: [f64; 2],
            expected_role: String,
            #[serde(default)]
            expected_label: Option<String>,
        }
        let params: Params = serde_json::from_str(params)
            .context("element instruction requires a JSON object with point, expected_role and optional expected_label (null or omitted selects coordinate-only identity)")?;
        return Ok(match action {
            "ax_inspect" => TaskInstruction::Atomic(AtomicInstruction::AxInspect { point: params.point,
                expected_role: params.expected_role,
                expected_label: params.expected_label, }),
            "click_focus" => TaskInstruction::Atomic(AtomicInstruction::ClickFocus { point: params.point,
                expected_role: params.expected_role,
                expected_label: params.expected_label, }),
            _ => TaskInstruction::Atomic(AtomicInstruction::ClickButton { point: params.point,
                expected_role: params.expected_role,
                expected_label: params.expected_label, }),
        });
    }
    if let Some(params) = value.strip_prefix("keyboard_text_input=") {
        #[derive(serde::Deserialize)]
        #[serde(deny_unknown_fields)]
        struct Params {
            point: [f64; 2],
            text: String,
        }
        let params: Params = serde_json::from_str(params)
            .context("input instruction requires a JSON object with point and text")?;
        return Ok(TaskInstruction::Atomic(AtomicInstruction::KeyboardTextInput { point: params.point, text: params.text }));
    }
    if let Some(params) = value.strip_prefix("keyboard_key_input=") {
        #[derive(serde::Deserialize)]
        #[serde(deny_unknown_fields)]
        struct Params {
            key: String,
            modifiers: Vec<String>,
        }
        let params: Params = serde_json::from_str(params)
            .context("keyboard_key_input requires a JSON object with key and modifiers")?;
        return Ok(TaskInstruction::Atomic(AtomicInstruction::KeyboardKeyInput { key: params.key,
        modifiers: params.modifiers, }));
    }
    bail!("Unknown instruction syntax")
}

pub async fn run() -> anyhow::Result<()> {
    match Cli::parse().command {
        Command::Serve => {
            let runtime = std::env::current_exe()?.with_file_name("desk-agent");
            let status = tokio::process::Command::new(runtime).status().await?;
            if !status.success() { bail!("runtime exited with {status}"); }
        }
        Command::Status => show_ipc(IpcRequest::Status { method: ipc::StatusMethod::Status }).await?,
        Command::Capabilities => capabilities().await?,
        Command::Rpa { app, message, instruction: instructions } => {
            let instructions = instructions.iter().map(|value| instruction(value)).collect::<anyhow::Result<Vec<_>>>()?;
            let request = TaskRequest {
                protocol_version: crate::kernel::task::PROTOCOL_VERSION.to_owned(),
                app,
                messages: message,
                instructions: (!instructions.is_empty()).then_some(instructions),
            };
            show_ipc(IpcRequest::Rpa(request)).await?;
        }
    }
    Ok(())
}

async fn capabilities() -> anyhow::Result<()> {
    let base = std::env::var("DESK_AGENT_URL").unwrap_or_else(|_| "http://127.0.0.1:17890/api/v2".into());
    let response = tokio::task::spawn_blocking(move || {
        std::process::Command::new("curl").args(["--fail", "--silent", "--show-error", &format!("{base}/capabilities")]).output()
    }).await??;
    if !response.status.success() { bail!("API request failed: {}", String::from_utf8_lossy(&response.stderr)); }
    let value: serde_json::Value = serde_json::from_slice(&response.stdout)?;
    println!("{value:#?}");
    Ok(())
}

async fn show_ipc(request: IpcRequest) -> anyhow::Result<()> {
    let response = tokio::task::spawn_blocking(move || ipc::request(&request)).await??;
    match response {
        IpcResponse::Status { available, version } => println!("Runtime: {} (version {version})", if available { "available" } else { "unavailable" }),
        IpcResponse::Task(response) => println!("{response:#?}"),
        IpcResponse::Error(response) => {
            eprintln!("{response:#?}");
            bail!("Runtime rejected the request");
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn atomic_and_perception_names_have_no_parser_aliases() {
        for (value, action) in [
            ("process_launch", "process_launch"),
            ("window_center", "window_center"),
            ("window_capture", "window_capture"),
            (r#"ax_inspect={"point":[1,2],"expected_role":"AXButton","expected_label":null}"#, "ax_inspect"),
            (r#"click_button={"point":[1,2],"expected_role":"AXButton","expected_label":null}"#, "click_button"),
            (r#"keyboard_text_input={"point":[1,2],"text":"Alice"}"#, "keyboard_text_input"),
            (r#"keyboard_key_input={"key":"return","modifiers":[]}"#, "keyboard_key_input"),
            (r#"vision_detect=["button"]"#, "vision_detect"),
            (r#"screenshot_coordinates_convert={"capture_id":"id","boxes":[[0,1,2,3]]}"#, "screenshot_coordinates_convert"),
        ] {
            let parsed = instruction(value).unwrap();
            assert_eq!(serde_json::to_value(parsed).unwrap()["action"], action);
        }
        for value in [
            "launch_app",
            "center_app_window",
            "capture_app_window",
            r#"inspect_app_element={"point":[1,2],"expected_role":"AXButton","expected_label":null}"#,
            r#"click_app_element={"point":[1,2],"expected_role":"AXButton","expected_label":null}"#,
            r#"mouse_click={"point":[1,2],"expected_role":"AXButton","expected_label":null}"#,
            r#"input_app_element={"point":[1,2],"text":"Alice"}"#,
            r#"keyboard_input={"point":[1,2],"text":"Alice"}"#,
            r#"detection_app_window_objects=["button"]"#,
            r#"convert_app_window_coordinates={"capture_id":"id","boxes":[[0,1,2,3]]}"#,
        ] {
            assert!(instruction(value).is_err(), "{value}");
        }
    }

    #[test]
    fn removed_application_actions_are_unknown_cli_syntax() {
        for value in [
            r#"input_search_query={"point":[1,2],"text":"Alice"}"#,
            "search_contact=true=Alice",
            "open_chat=Alice",
            r#"write_chat_draft={"contact":"Alice","text":"Hello"}"#,
            r#"send_chat_draft={"contact":"Alice","content":"Hello"}"#,
            "send_text=Hello",
            "verify_message_sent",
        ] {
            assert!(instruction(value).is_err(), "{value}");
        }
    }
    #[test]
    fn input_actions_construct_atomic_instructions() {
        assert_eq!(
            instruction(r#"keyboard_text_input={"point":[1.0,2.0],"text":"Alice"}"#).unwrap(),
            TaskInstruction::Atomic(AtomicInstruction::KeyboardTextInput { point: [1.0, 2.0], text: "Alice".into() })
        );
        assert_eq!(
            instruction(r#"keyboard_key_input={"key":"return","modifiers":[]}"#).unwrap(),
            TaskInstruction::Atomic(AtomicInstruction::KeyboardKeyInput { key: "return".into(), modifiers: vec![] })
        );
        for value in [
            r#"keyboard_key_input={"key":"return"}"#,
            r#"keyboard_key_input={"key":"return","modifiers":[],"extra":1}"#,
        ] {
            assert!(instruction(value).is_err(), "{value}");
        }
        assert!(instruction(r#"keyboard_key_input={"key":"enter","modifiers":[]}"#).is_ok());
    }


    #[test]
    fn instruction_parameters_preserve_equals_and_text() {
        assert_eq!(
            instruction(r#"vision_detect=["按钮","a=b"]"#).unwrap(),
            TaskInstruction::Perception(PerceptionInstruction::VisionDetect{ objects: vec!["按钮".into(), "a=b".into()] })
        );
        assert!(instruction("vision_detect=button").is_err());
    }

    #[test]
    fn element_instruction_preserves_explicit_identity_mode() {
        for action in ["click_button", "ax_inspect"] {
            for label in ["", r#","expected_label":null"#, r#","expected_label":"Search""#] {
                let step = instruction(&format!(
                    r#"{action}={{"point":[100.0,200.0],"expected_role":"AXCell"{label}}}"#
                )).unwrap();
                let expected_label = if label.contains("Search") { Some("Search".to_owned()) } else { None };
                assert_eq!(step, if action == "click_button" {
                    TaskInstruction::Atomic(AtomicInstruction::ClickButton{ point: [100.0, 200.0], expected_role: "AXCell".into(), expected_label })
                } else {
                    TaskInstruction::Atomic(AtomicInstruction::AxInspect{ point: [100.0, 200.0], expected_role: "AXCell".into(), expected_label })
                });
            }
            assert!(instruction(&format!(
                r#"{action}={{"point":[100.0,200.0],"expected_role":"AXCell","extra":true}}"#
            )).is_err());
        }
    }

    #[test]
    fn nickname_send_uses_shared_strict_typed_serde_parser() {
        let raw = r#"send_message_by_nickname={"nickname":" 张=三 ","messages":[{"type":"text","content":" e\u0301 😀 = "},{"type":"text","content":"second"}],"interval_ms":1000}"#;
        assert_eq!(
            instruction(raw).unwrap(),
            TaskInstruction::Application(ApplicationInstruction::SendMessageByNickname {
                nickname: " 张=三 ".into(),
                messages: vec![
                    crate::kernel::instruction::Message::Text { content: " e\u{301} 😀 = ".into() },
                    crate::kernel::instruction::Message::Text { content: "second".into() },
                ],
                interval_ms: 1000,
            })
        );
        for params in [
            r#"{"nickname":"Alice","messages":[{"type":"text","content":"hi"}]}"#,
            r#"{"nickname":"Alice","messages":["hi"],"interval_ms":0}"#,
            r#"{"nickname":"Alice","messages":[{"type":"text","content":"hi","extra":0}],"interval_ms":0}"#,
            r#"{"nickname":"Alice","messages":[{"type":"text","content":"hi"}],"interval_ms":0,"extra":0}"#,
            r#"{"nickname":"Alice","nickname":"Bob","messages":[{"type":"text","content":"hi"}],"interval_ms":0}"#,
            r#"{"nickname":"Alice","messages":[{"type":"text","content":"hi"}],"interval_ms":-1}"#,
        ] {
            assert!(instruction(&format!("send_message_by_nickname={params}")).is_err());
        }
    }

    #[test]
    fn cli_accepts_either_input_and_rejects_neither() {
        assert!(Cli::try_parse_from(["cli", "rpa", "--app", "wecom", "--instruction", "process_launch"]).is_ok());
        assert!(Cli::try_parse_from(["cli", "rpa", "--app", "wecom", "--message", "打开企业微信"]).is_ok());
        assert!(Cli::try_parse_from(["cli", "rpa", "--app", "wecom"]).is_err());
    }
}
