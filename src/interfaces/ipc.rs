use crate::kernel::task::{TaskErrorResponse, TaskInstruction, TaskRequest, TaskResponse};
use serde::{Deserialize, Serialize};
use std::io::{Read, Write};

pub const MAX_FRAME_BYTES: usize = 1024 * 1024;

#[derive(Debug, Serialize, Deserialize)]
#[serde(untagged)]
pub enum IpcRequest {
    Status { method: StatusMethod },
    Rpa(TaskRequest),
}

#[derive(Debug, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum StatusMethod {
    Status,
}

#[derive(Debug, Serialize, Deserialize)]
#[serde(untagged)]
pub enum IpcResponse {
    Status { available: bool, version: String },
    Task(TaskResponse),
    Error(TaskErrorResponse),
}

pub fn socket_path() -> std::path::PathBuf {
    std::env::var_os("DESK_AGENT_SOCKET")
        .map(std::path::PathBuf::from)
        .unwrap_or_else(|| {
            let user = std::env::var("USER").unwrap_or_else(|_| "local".to_owned());
            std::env::temp_dir().join(format!("desk-agent-{user}.sock"))
        })
}

pub fn encode<T: Serialize>(value: &T) -> anyhow::Result<Vec<u8>> {
    let data = serde_json::to_vec(value)?;
    anyhow::ensure!(!data.is_empty() && data.len() <= MAX_FRAME_BYTES, "IPC frame size is invalid");
    Ok(data)
}

pub fn request(request: &IpcRequest) -> anyhow::Result<IpcResponse> {
    let data = encode(request)?;
    let mut stream = std::os::unix::net::UnixStream::connect(socket_path())?;
    // Vision uses up to 16 sequential local inference calls, each bounded to
    // 90 seconds. A client timeout does not cancel an already dispatched task.
    let read_timeout = match request {
        IpcRequest::Rpa(task) if task.instructions.as_ref().is_some_and(|steps| {
            steps.iter().any(|step| matches!(step, TaskInstruction::DetectionAppWindowObjects { .. }))
        }) => 1500,
        _ => 10,
    };
    stream.set_read_timeout(Some(std::time::Duration::from_secs(read_timeout)))?;
    stream.set_write_timeout(Some(std::time::Duration::from_secs(2)))?;
    stream.write_all(&(data.len() as u32).to_be_bytes())?;
    stream.write_all(&data)?;
    let mut length = [0; 4];
    stream.read_exact(&mut length)?;
    let length = u32::from_be_bytes(length) as usize;
    anyhow::ensure!(length > 0 && length <= MAX_FRAME_BYTES, "IPC response frame size is invalid");
    let mut data = vec![0; length];
    stream.read_exact(&mut data)?;
    Ok(serde_json::from_slice(&data)?)
}
