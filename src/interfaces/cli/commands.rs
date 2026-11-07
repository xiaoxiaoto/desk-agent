use crate::interfaces::ipc::{self, IpcRequest, IpcResponse};
use crate::kernel::task::{TaskInstruction, TaskRequest};
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
        /// launch_app; center_app_window; capture_app_window; detection_app_window_objects=JSON_ARRAY; convert_app_window_coordinates=JSON_OBJECT; click_app_element=JSON_OBJECT; inspect_app_element=JSON_OBJECT; input_app_element=JSON_OBJECT; verify_message_sent; open_chat=CONTACT; send_text=TEXT; search_contact=BOOL=NAME
        #[arg(long)]
        instruction: Vec<String>,
    },
}

fn instruction(value: &str) -> anyhow::Result<TaskInstruction> {
    if value == "launch_app" { return Ok(TaskInstruction::LaunchApp); }
    if value == "center_app_window" { return Ok(TaskInstruction::CenterAppWindow); }
    if value == "capture_app_window" { return Ok(TaskInstruction::CaptureAppWindow); }
    if value == "verify_message_sent" { return Ok(TaskInstruction::VerifyMessageSent); }
    if let Some(objects) = value.strip_prefix("detection_app_window_objects=") {
        return Ok(TaskInstruction::DetectionAppWindowObjects {
            objects: serde_json::from_str(objects).context("detection_app_window_objects requires a JSON array of strings")?,
        });
    }
    if let Some(params) = value.strip_prefix("convert_app_window_coordinates=") {
        #[derive(serde::Deserialize)]
        #[serde(deny_unknown_fields)]
        struct Params {
            capture_id: String,
            boxes: Vec<[f64; 4]>,
        }
        let params: Params = serde_json::from_str(params)
            .context("convert_app_window_coordinates requires a JSON object with capture_id and boxes")?;
        return Ok(TaskInstruction::ConvertAppWindowCoordinates {
            capture_id: params.capture_id,
            boxes: params.boxes,
        });
    }
    if let Some((action, params)) = value.split_once('=').filter(|(action, _)| matches!(*action, "click_app_element" | "inspect_app_element")) {
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
        return Ok(if action == "inspect_app_element" {
            TaskInstruction::InspectAppElement {
                point: params.point,
                expected_role: params.expected_role,
                expected_label: params.expected_label,
            }
        } else {
            TaskInstruction::ClickAppElement {
                point: params.point,
                expected_role: params.expected_role,
                expected_label: params.expected_label,
            }
        });
    }
    if let Some(params) = value.strip_prefix("input_app_element=") {
        #[derive(serde::Deserialize)]
        #[serde(deny_unknown_fields)]
        struct Params {
            point: [f64; 2],
            text: String,
        }
        let params: Params = serde_json::from_str(params)
            .context("input_app_element requires a JSON object with point and text")?;
        return Ok(TaskInstruction::InputAppElement {
            point: params.point,
            text: params.text,
        });
    }
    if let Some(contact) = value.strip_prefix("open_chat=") {
        return Ok(TaskInstruction::OpenChat { contact: contact.to_owned() });
    }
    if let Some(content) = value.strip_prefix("send_text=") {
        return Ok(TaskInstruction::SendText { content: content.to_owned() });
    }
    if let Some(params) = value.strip_prefix("search_contact=") {
        let (exact_match, name) = params.split_once('=').context("search_contact requires BOOL=NAME")?;
        return Ok(TaskInstruction::SearchContact {
            name: name.to_owned(),
            exact_match: exact_match.parse().context("search_contact BOOL must be true or false")?,
        });
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
    let base = std::env::var("DESK_AGENT_URL").unwrap_or_else(|_| "http://127.0.0.1:17890/api/v1".into());
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
    fn instruction_parameters_preserve_equals_and_text() {
        assert_eq!(
            instruction("send_text= a=b\nc ").unwrap(),
            TaskInstruction::SendText { content: " a=b\nc ".into() }
        );
        assert_eq!(
            instruction("search_contact=true=张=三").unwrap(),
            TaskInstruction::SearchContact { name: "张=三".into(), exact_match: true }
        );
        assert!(instruction("search_contact=yes=张三").is_err());
        assert_eq!(
            instruction(r#"detection_app_window_objects=["按钮","a=b"]"#).unwrap(),
            TaskInstruction::DetectionAppWindowObjects { objects: vec!["按钮".into(), "a=b".into()] }
        );
        assert!(instruction("detection_app_window_objects=button").is_err());
    }

    #[test]
    fn element_instruction_preserves_explicit_identity_mode() {
        for action in ["click_app_element", "inspect_app_element"] {
            for label in ["", r#","expected_label":null"#, r#","expected_label":"Search""#] {
                let step = instruction(&format!(
                    r#"{action}={{"point":[100.0,200.0],"expected_role":"AXCell"{label}}}"#
                )).unwrap();
                let expected_label = if label.contains("Search") { Some("Search".to_owned()) } else { None };
                assert_eq!(step, if action == "click_app_element" {
                    TaskInstruction::ClickAppElement { point: [100.0, 200.0], expected_role: "AXCell".into(), expected_label }
                } else {
                    TaskInstruction::InspectAppElement { point: [100.0, 200.0], expected_role: "AXCell".into(), expected_label }
                });
            }
            assert!(instruction(&format!(
                r#"{action}={{"point":[100.0,200.0],"expected_role":"AXCell","extra":true}}"#
            )).is_err());
        }
    }

    #[test]
    fn cli_accepts_either_input_and_rejects_neither() {
        assert!(Cli::try_parse_from(["cli", "rpa", "--app", "wecom", "--instruction", "launch_app"]).is_ok());
        assert!(Cli::try_parse_from(["cli", "rpa", "--app", "wecom", "--message", "打开企业微信"]).is_ok());
        assert!(Cli::try_parse_from(["cli", "rpa", "--app", "wecom"]).is_err());
    }
}
