//! Wire-form tests for Jev desktop-control payloads.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use super::{ConfigureJevRequest, JevProvider, ResolveIntentRequest, RunGoalRequest};
use serde_json::json;

#[test]
fn configuration_serializes_the_key_but_never_debug_prints_it() {
    let mut request = ConfigureJevRequest::new("openrouter-secret");
    request.provider = JevProvider::OpenRouter;
    request.endpoint_url = Some("https://openrouter.ai/api/alpha/decisions".into());
    let value = serde_json::to_value(&request).expect("configuration serializes");

    assert_eq!(value["api_key"], json!("openrouter-secret"));
    assert!(!format!("{request:?}").contains("openrouter-secret"));
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
}
