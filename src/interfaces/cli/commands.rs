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
        /// launch_wecom; center_wecom_window; capture_app_window; locate_app_window_objects=JSON_ARRAY; convert_app_window_coordinates=JSON_OBJECT; activate_app_element=JSON_OBJECT; diagnose_app_element=JSON_OBJECT; verify_message_sent; open_chat=CONTACT; send_text=TEXT; search_contact=BOOL=NAME
        #[arg(long)]
        instruction: Vec<String>,
    },
}

fn instruction(value: &str) -> anyhow::Result<TaskInstruction> {
    if value == "launch_wecom" { return Ok(TaskInstruction::LaunchWecom); }
    if value == "center_wecom_window" { return Ok(TaskInstruction::CenterWecomWindow); }
    if value == "capture_app_window" { return Ok(TaskInstruction::CaptureAppWindow); }
    if value == "verify_message_sent" { return Ok(TaskInstruction::VerifyMessageSent); }
    if let Some(objects) = value.strip_prefix("locate_app_window_objects=") {
        return Ok(TaskInstruction::LocateAppWindowObjects {
            objects: serde_json::from_str(objects).context("locate_app_window_objects requires a JSON array of strings")?,
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
    if let Some((action, params)) = value.split_once('=').filter(|(action, _)| matches!(*action, "activate_app_element" | "diagnose_app_element")) {
        #[derive(serde::Deserialize)]
        #[serde(deny_unknown_fields)]
        struct Params {
            point: [f64; 2],
            expected_role: String,
            expected_label: String,
        }
        let params: Params = serde_json::from_str(params)
            .context("element instruction requires a JSON object with point, expected_role and expected_label")?;
        return Ok(if action == "diagnose_app_element" {
            TaskInstruction::DiagnoseAppElement {
                point: params.point,
                expected_role: params.expected_role,
                expected_label: params.expected_label,
            }
        } else {
            TaskInstruction::ActivateAppElement {
                point: params.point,
                expected_role: params.expected_role,
                expected_label: params.expected_label,
            }
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
            instruction(r#"locate_app_window_objects=["按钮","a=b"]"#).unwrap(),
            TaskInstruction::LocateAppWindowObjects { objects: vec!["按钮".into(), "a=b".into()] }
        );
        assert!(instruction("locate_app_window_objects=button").is_err());
    }

    #[test]
    fn cli_accepts_either_input_and_rejects_neither() {
        assert!(Cli::try_parse_from(["cli", "rpa", "--app", "wecom", "--instruction", "launch_wecom"]).is_ok());
        assert!(Cli::try_parse_from(["cli", "rpa", "--app", "wecom", "--message", "打开企业微信"]).is_ok());
        assert!(Cli::try_parse_from(["cli", "rpa", "--app", "wecom"]).is_err());
    }
}
