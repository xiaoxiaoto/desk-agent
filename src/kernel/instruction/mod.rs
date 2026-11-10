mod atomic;
mod application;
mod perception;

pub use atomic::AtomicInstruction;
pub use application::{ApplicationInstruction, Message};
pub use perception::PerceptionInstruction;

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(untagged)]
pub enum TaskInstruction {
    Atomic(AtomicInstruction),
    Perception(PerceptionInstruction),
    Application(ApplicationInstruction),
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn instruction_categories_preserve_the_flat_wire_contract() {
        let cases = [
            (json!({"action":"process_launch"}), TaskInstruction::Atomic(AtomicInstruction::ProcessLaunch)),
            (json!({"action":"window_center"}), TaskInstruction::Atomic(AtomicInstruction::WindowCenter)),
            (json!({"action":"window_capture"}), TaskInstruction::Atomic(AtomicInstruction::WindowCapture)),
            (json!({"action":"click_button","params":{"point":[1.0,2.0],"expected_role":"AXButton","expected_label":null}}), TaskInstruction::Atomic(AtomicInstruction::ClickButton { point: [1.0, 2.0], expected_role: "AXButton".into(), expected_label: None })),
            (json!({"action":"click_focus","params":{"point":[1.0,2.0],"expected_role":"AXTextField","expected_label":null}}), TaskInstruction::Atomic(AtomicInstruction::ClickFocus { point: [1.0, 2.0], expected_role: "AXTextField".into(), expected_label: None })),
            (json!({"action":"ax_inspect","params":{"point":[1.0,2.0],"expected_role":"AXButton","expected_label":"x"}}), TaskInstruction::Atomic(AtomicInstruction::AxInspect { point: [1.0, 2.0], expected_role: "AXButton".into(), expected_label: Some("x".into()) })),
            (json!({"action":"vision_detect","params":{"objects":["按钮"]}}), TaskInstruction::Perception(PerceptionInstruction::VisionDetect { objects: vec!["按钮".into()] })),
            (json!({"action":"screenshot_coordinates_convert","params":{"capture_id":"id","boxes":[[0.0,1.0,2.0,3.0]]}}), TaskInstruction::Perception(PerceptionInstruction::ScreenshotCoordinatesConvert { capture_id: "id".into(), boxes: vec![[0.0, 1.0, 2.0, 3.0]] })),
            (json!({"action":"keyboard_text_input","params":{"point":[1.0,2.0],"text":"Alice"}}), TaskInstruction::Atomic(AtomicInstruction::KeyboardTextInput { point: [1.0, 2.0], text: "Alice".into() })),
            (json!({"action":"keyboard_key_input","params":{"key":"return","modifiers":[]}}), TaskInstruction::Atomic(AtomicInstruction::KeyboardKeyInput { key: "return".into(), modifiers: vec![] })),
            (json!({"action":"send_message_by_nickname","params":{"nickname":"Alice","messages":[{"type":"text","content":"Hello"}],"interval_ms":0}}), TaskInstruction::Application(ApplicationInstruction::SendMessageByNickname { nickname: "Alice".into(), messages: vec![Message::Text { content: "Hello".into() }], interval_ms: 0 })),
        ];
        for (wire, instruction) in cases {
            assert_eq!(serde_json::from_value::<TaskInstruction>(wire.clone()).unwrap(), instruction);
            assert_eq!(serde_json::to_value(instruction).unwrap(), wire);
        }
    }

    #[test]
    fn rejects_wrappers_unknown_fields_and_raw_input() {
        for wire in [
            json!({"atomic":{"action":"process_launch"}}),
            json!({"action":"process_launch","category":"platform"}),
            json!({"action":"keyboard_input","params":{"point":[1,2],"text":"x","extra":true}}),
            json!({"action":"mouse_down","params":{"point":[1,2]}}),
        ] {
            assert!(serde_json::from_value::<TaskInstruction>(wire).is_err());
        }
    }

    #[test]
    fn rejects_previous_atomic_actions_across_serde_consumers() {
        for wire in [
            json!({"action":"launch_app"}),
            json!({"action":"center_app_window"}),
            json!({"action":"capture_app_window"}),
            json!({"action":"click_app_element","params":{"point":[1,2],"expected_role":"AXButton","expected_label":null}}),
            json!({"action":"mouse_click","params":{"point":[1,2],"expected_role":"AXButton","expected_label":null}}),
            json!({"action":"inspect_app_element","params":{"point":[1,2],"expected_role":"AXButton","expected_label":null}}),
            json!({"action":"input_app_element","params":{"point":[1,2],"text":"Alice"}}),
            json!({"action":"detection_app_window_objects","params":{"objects":["button"]}}),
            json!({"action":"convert_app_window_coordinates","params":{"capture_id":"id","boxes":[[0,1,2,3]]}}),
        ] {
            assert!(serde_json::from_value::<TaskInstruction>(wire.clone()).is_err());
            assert!(serde_json::from_value::<AtomicInstruction>(wire.clone()).is_err());
            assert!(serde_json::from_value::<PerceptionInstruction>(wire.clone()).is_err());
            let request = json!({"protocol_version":"2","app":"wecom","instructions":[wire]});
            assert!(serde_json::from_value::<crate::kernel::task::TaskRequest>(request.clone()).is_err());
            assert!(serde_json::from_value::<crate::interfaces::ipc::IpcRequest>(request).is_err());
        }
    }

    #[test]
    fn removed_application_actions_are_unknown_to_all_request_consumers() {
        for action in ["input_search_query", "search_contact", "open_chat", "write_chat_draft", "send_chat_draft", "send_text", "verify_message_sent"] {
            let wire = json!({"action": action, "params": {}});
            assert!(serde_json::from_value::<ApplicationInstruction>(wire.clone()).is_err());
            assert!(serde_json::from_value::<TaskInstruction>(wire.clone()).is_err());
            let request = json!({"protocol_version":"2","app":"wecom","instructions":[wire]});
            assert!(serde_json::from_value::<crate::kernel::task::TaskRequest>(request.clone()).is_err());
            assert!(serde_json::from_value::<crate::interfaces::ipc::IpcRequest>(request).is_err());
        }
    }
}
