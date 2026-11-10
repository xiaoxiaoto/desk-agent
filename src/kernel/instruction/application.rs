use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", deny_unknown_fields, rename_all = "snake_case")]
pub enum Message {
    Text { content: String },
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(
    tag = "action",
    content = "params",
    rename_all = "snake_case",
    deny_unknown_fields
)]
pub enum ApplicationInstruction {
    SendMessageByNickname {
        nickname: String,
        messages: Vec<Message>,
        interval_ms: u64,
    },
}
