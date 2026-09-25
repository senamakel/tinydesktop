use super::{Candidate, Screen, fingerprint};
use serde_json::json;

fn screen(node: Candidate) -> Screen {
    Screen {
        app: "Example".to_owned(),
        window: None,
        window_id: None,
        surface: "window".to_owned(),
        root: None,
        candidates: Vec::new(),
        observed: vec![node],
    }
}

#[test]
fn actionability_and_child_count_change_fingerprint() {
    let mut node = Candidate {
        role: "button".to_owned(),
        name: Some("Play".to_owned()),
        ..Candidate::default()
    };
    let original = fingerprint(&screen(node.clone()));
    node.available_actions.push("click".to_owned());
    let actionable = fingerprint(&screen(node.clone()));
    assert_ne!(original, actionable);
    node.children_count = Some(2);
    assert_ne!(actionable, fingerprint(&screen(node)));
}

#[test]
fn large_accessibility_fields_produce_fixed_size_fingerprint() {
    let node = Candidate {
        role: "text".to_owned(),
        name: Some("é".repeat(100_000)),
        path: vec!["ancestor".repeat(100_000); 100],
        value: Some(json!({"text": "v".repeat(100_000)})),
        ..Candidate::default()
    };
    let digest = fingerprint(&screen(node));
    assert_eq!(digest.len(), 16);
    assert!(digest.bytes().all(|byte| byte.is_ascii_hexdigit()));
}
