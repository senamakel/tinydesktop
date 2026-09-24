//! Wire-form tests for Jev desktop-control payloads.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use super::{
    GoalContinuation, JevConfig, JevDecisionKind, JevOperation, JevProvider, JevStopReason,
    ResolveIntentRequest, RunGoalRequest,
};
use serde_json::json;

#[test]
fn configuration_serializes_the_key_but_never_debug_prints_it() {
    let mut request = JevConfig::new("openrouter-secret");
    request.provider = JevProvider::OpenRouter;
    request.endpoint_url = Some("https://openrouter.ai/api/alpha/decisions".into());
    let value = serde_json::to_value(&request).expect("configuration serializes");

    assert_eq!(value["api_key"], json!("openrouter-secret"));
    assert!(!format!("{request:?}").contains("openrouter-secret"));
}

#[test]
fn every_agentic_enum_pins_its_wire_spelling() {
    assert_eq!(
        serde_json::to_value(JevProvider::TypeSafe).unwrap(),
        json!("type_safe")
    );
    assert_eq!(
        serde_json::to_value(JevProvider::OpenRouter).unwrap(),
        json!("open_router")
    );
    assert_eq!(
        serde_json::to_value(JevProvider::TinyHumansOpenRouter).unwrap(),
        json!("tiny_humans_open_router")
    );
    for (operation, wire) in [
        (JevOperation::Click, "CLICK"),
        (JevOperation::TypeText, "TYPE_TEXT"),
        (JevOperation::Check, "CHECK"),
        (JevOperation::Uncheck, "UNCHECK"),
        (JevOperation::Expand, "EXPAND"),
        (JevOperation::Collapse, "COLLAPSE"),
        (JevOperation::Scroll, "SCROLL"),
        (JevOperation::Drill, "DRILL"),
        (JevOperation::Widen, "WIDEN"),
        (JevOperation::Wait, "WAIT"),
        (JevOperation::Done, "DONE"),
        (JevOperation::Blocked, "BLOCKED"),
    ] {
        assert_eq!(serde_json::to_value(operation).unwrap(), json!(wire));
    }
    assert_eq!(
        serde_json::to_value(JevDecisionKind::ConfirmationRequired).unwrap(),
        json!("confirmation_required")
    );
    assert_eq!(
        serde_json::to_value(JevStopReason::ActionFailed).unwrap(),
        json!("action_failed")
    );
}

#[test]
fn agentic_requests_default_to_not_sharing_field_values() {
    let resolve: ResolveIntentRequest = serde_json::from_value(json!({
        "app": "Spotify",
        "intent": "open Search"
    }))
    .expect("resolve request decodes");
    let run: RunGoalRequest = serde_json::from_value(json!({
        "app": "Spotify",
        "goal": "open Search"
    }))
    .expect("run request decodes");

    assert!(!resolve.include_values);
    assert!(!run.include_values);
    assert_eq!((run.max_steps, run.max_model_calls), (40, 80));
    assert!(run.continuation.is_none());
}

#[test]
fn confirmation_payload_has_explicit_approval_and_one_use_handle() {
    let request: RunGoalRequest = serde_json::from_value(json!({
        "continuation": {"id": "opaque-handle", "approve": false}
    }))
    .expect("continuation decodes");
    assert_eq!(
        request.continuation,
        Some(GoalContinuation {
            id: "opaque-handle".to_owned(),
            approve: false,
        })
    );
    assert_eq!(
        serde_json::to_value(JevStopReason::Cancelled).unwrap(),
        json!("cancelled")
    );
    assert_eq!(
        serde_json::to_value(JevStopReason::StaleTarget).unwrap(),
        json!("stale_target")
    );
}
