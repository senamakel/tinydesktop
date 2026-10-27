//! Tests pinning the `ConsequenceHost` interface identity and wire forms.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use super::*;

#[test]
fn the_interface_is_its_own_not_the_desktop_or_site_memory_one() {
    assert_ne!(INTERFACE, crate::names::INTERFACE);
    assert_ne!(INTERFACE, crate::site_memory::INTERFACE);
    assert_ne!(OBJECT_PATH, crate::site_memory::OBJECT_PATH);
    assert_eq!(METHODS, ["Review"]);
}

#[test]
fn a_review_request_keeps_its_wire_form() {
    let request: ReviewActionRequest = serde_json::from_value(serde_json::json!({
        "task": "t1", "action": "pay now", "target": "Pay", "kind": "payment"
    }))
    .unwrap();
    assert_eq!(request.kind, ConsequenceKind::Payment);
    let json = serde_json::to_value(&request).unwrap();
    assert_eq!(json["kind"], "payment");
    let irreversible = serde_json::to_value(ConsequenceKind::Irreversible).unwrap();
    assert_eq!(irreversible, "irreversible");
}

#[test]
fn review_request_host_context_is_optional_and_round_trips() {
    let legacy: ReviewActionRequest = serde_json::from_value(serde_json::json!({
        "task": "t1", "action": "pay now", "target": "Pay", "kind": "payment"
    }))
    .unwrap();
    assert_eq!(legacy.host_context, None);
    assert!(
        serde_json::to_value(&legacy)
            .unwrap()
            .get("host_context")
            .is_none()
    );

    let with_context = ReviewActionRequest {
        task: "t2".into(),
        action: "submit".into(),
        target: "Submit".into(),
        kind: ConsequenceKind::Irreversible,
        host_context: Some(crate::agent::HostContext::new("opaque-host-context")),
    };
    let encoded = serde_json::to_value(&with_context).unwrap();
    assert_eq!(encoded["host_context"], "opaque-host-context");
    let decoded: ReviewActionRequest = serde_json::from_value(encoded).unwrap();
    assert_eq!(
        decoded
            .host_context
            .as_ref()
            .map(crate::agent::HostContext::as_str),
        Some("opaque-host-context")
    );
}

#[test]
fn a_reason_is_optional_and_left_out_when_absent() {
    let reply: ReviewActionResponse = serde_json::from_str(r#"{"allow":true}"#).unwrap();
    assert_eq!(reply.reason, None);
    let json = serde_json::to_value(&reply).unwrap();
    assert!(json.get("reason").is_none());
}
