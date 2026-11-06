use crate::kernel::task::{TaskInstruction, TaskRequest, WECOM_APP_ID};
use crate::interfaces::ipc::{self, IpcRequest, IpcResponse};
use eframe::egui;

struct DeskAgentApp {
    status: String,
    message: String,
}

impl eframe::App for DeskAgentApp {
    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        egui::CentralPanel::default().show(ctx, |ui| {
            ui.heading("DeskAgent");
            ui.label("Local Runtime status");
            if ui.button("Check Runtime").clicked() {
                self.status = runtime_command(IpcRequest::Status { method: ipc::StatusMethod::Status });
            }
            ui.text_edit_singleline(&mut self.message);
            if ui.button("Run task").clicked() {
                self.status = runtime_command(IpcRequest::Rpa(TaskRequest {
                    protocol_version: crate::kernel::task::PROTOCOL_VERSION.to_owned(),
                    app: WECOM_APP_ID.to_owned(),
                    messages: vec![self.message.clone()],
                    instructions: None,
                }));
            }
            if ui.button("Open WeCom").clicked() {
                self.status = runtime_command(IpcRequest::Rpa(TaskRequest {
                    protocol_version: crate::kernel::task::PROTOCOL_VERSION.to_owned(),
                    app: WECOM_APP_ID.to_owned(),
                    messages: Vec::new(),
                    instructions: Some(vec![TaskInstruction::LaunchWecom]),
                }));
            }
            ui.label(&self.status);
        });
    }
}

fn runtime_command(command: IpcRequest) -> String {
    match ipc::request(&command) {
        Ok(IpcResponse::Status { available, version }) => {
            format!("Runtime: {} (version {version})", if available { "available" } else { "unavailable" })
        }
        Ok(IpcResponse::Task(response)) => format!("{response:#?}"),
        Ok(IpcResponse::Error(response)) => format!("{response:#?}"),
        Err(error) => format!("Runtime unavailable: {error}"),
    }
}

pub fn run() -> eframe::Result<()> {
    eframe::run_native(
        "DeskAgent",
        eframe::NativeOptions::default(),
        Box::new(|_| {
            Ok(Box::new(DeskAgentApp {
                status: "Not checked".into(),
                message: String::new(),
            }))
        }),
    )
}
