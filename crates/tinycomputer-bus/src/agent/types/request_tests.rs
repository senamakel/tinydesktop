//! Tests for opaque, host-issued values in Agent requests.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use super::*;

#[test]
fn host_context_is_transparent_on_the_wire_and_redacted_in_debug() {
    let context = HostContext::new("private-host-capability");
    assert_eq!(context.as_str(), "private-host-capability");
    assert_eq!(
        serde_json::to_value(&context).unwrap(),
        "private-host-capability"
    );
    assert_eq!(format!("{context:?}"), "HostContext([opaque])");

    let request = ContinueTaskRequest {
        host_context: Some(context),
        ..ContinueTaskRequest::default()
    };
    assert_eq!(
        serde_json::to_value(request).unwrap()["host_context"],
        "private-host-capability"
    );
}
