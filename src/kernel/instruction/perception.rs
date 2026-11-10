use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "action", content = "params", rename_all = "snake_case", deny_unknown_fields)]
pub enum PerceptionInstruction {
    VisionDetect { objects: Vec<String> },
    ScreenshotCoordinatesConvert { capture_id: String, boxes: Vec<[f64; 4]> },
}
