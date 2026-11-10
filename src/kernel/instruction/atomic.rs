use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "action", content = "params", rename_all = "snake_case", deny_unknown_fields)]
pub enum AtomicInstruction {
    ProcessLaunch,
    WindowCenter,
    WindowCapture,
    ClickButton { point: [f64; 2], expected_role: String, #[serde(default)] expected_label: Option<String> },
    AxInspect { point: [f64; 2], expected_role: String, #[serde(default)] expected_label: Option<String> },
    ClickFocus { point: [f64; 2], expected_role: String, #[serde(default)] expected_label: Option<String> },
    KeyboardTextInput { point: [f64; 2], text: String },
    KeyboardKeyInput { key: String, modifiers: Vec<String> },
}
