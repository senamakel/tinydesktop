//! Tests for deterministic Jev desktop-control policy.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use std::{
    collections::{BTreeMap, VecDeque},
    sync::{Arc, Mutex},
    time::Duration,
};

use serde_json::json;
use tinydesktop_bus::{
    ConfigureJevRequest, DesktopResponse, JevDecisionKind, JevOperation, JevProvider,
    JevStopReason, RunGoalRequest,
};
use tinyjevclient::{Answer, ChoiceAnswer};
use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    net::TcpListener,
};

use super::{
    AgentBackend, JevRuntime, execute_desktop, internal_error,
    policy::{
        ACT, DESTRUCTIVE, FLOOR, action_space, choice, exact_named_match, gate_with_evidence, noul,
        parse_operation, positional_match, request, rerank_request, shortlist, target,
    },
    provider_error, reason, resolve_intent, resolve_intent_with, response as agent_response,
    run_goal, run_goal_with,
    screen::{Candidate, Screen, describe, fingerprint, observe, parse_reply},
    target_payload,
};

#[test]
fn execution_gates_on_selected_probability_not_distribution_concentration() {
    let answer = Answer::Choice(ChoiceAnswer {
        choice: "liked".to_owned(),
        probabilities: BTreeMap::from([("liked".to_owned(), 0.91), ("other".to_owned(), 0.09)]),
        confidence: 0.41,
    });

    assert_eq!(choice(Some(&answer)), Some(("liked", 0.91)));
}

#[test]
fn destructive_actions_always_require_confirmation() {
    assert_eq!(
        gate_with_evidence(JevOperation::Click, 0.99, DESTRUCTIVE, false),
        JevDecisionKind::ConfirmationRequired
    );
}

#[test]
fn only_safe_confident_actions_are_executable() {
    assert_eq!(
        gate_with_evidence(JevOperation::Click, ACT, DESTRUCTIVE - 0.01, false),
        JevDecisionKind::Act
    );
    assert_eq!(
        gate_with_evidence(JevOperation::Click, FLOOR - 0.01, 0.0, false),
        JevDecisionKind::Abstain
    );
}

#[test]
fn an_exact_multiword_accessible_name_is_strong_identity_evidence() {
    let candidate = Candidate {
        name: Some("Liked Songs".to_owned()),
        ..Candidate::default()
    };
    assert!(exact_named_match(
        "open Liked Songs and play the first track",
        Some(&candidate)
    ));
    assert_eq!(
        gate_with_evidence(JevOperation::Click, 0.52, 0.05, true),
        JevDecisionKind::Act
    );
}

#[test]
fn topmost_play_target_is_strong_positional_evidence() {
    let top = Candidate {
        ref_id: "@s:e1".to_owned(),
        name: Some("Play First Song by Artist".to_owned()),
        bounds: Some(serde_json::json!({"x": 10.0, "y": 100.0})),
        ..Candidate::default()
    };
    let lower = Candidate {
        ref_id: "@s:e2".to_owned(),
        name: Some("Play Second Song by Artist".to_owned()),
        bounds: Some(serde_json::json!({"x": 10.0, "y": 160.0})),
        ..Candidate::default()
    };
    let peers = BTreeMap::from([("1".to_owned(), top.clone()), ("2".to_owned(), lower)]);

    assert!(positional_match(
        "play the topmost song",
        Some(&top),
        Some(&peers)
    ));
    assert_eq!(
        gate_with_evidence(JevOperation::Click, 0.49, 0.05, true),
        JevDecisionKind::Act
    );
}

#[test]
fn terminal_operations_are_not_treated_as_actions() {
    assert_eq!(
        gate_with_evidence(JevOperation::Done, 1.0, 1.0, false),
        JevDecisionKind::Done
    );
    assert_eq!(
        gate_with_evidence(JevOperation::Blocked, 1.0, 1.0, false),
        JevDecisionKind::Blocked
    );
}

#[derive(Clone)]
struct FakeBackend {
    screens: Arc<Mutex<VecDeque<Screen>>>,
    operations: Arc<Mutex<Vec<JevOperation>>>,
}

impl AgentBackend for FakeBackend {
    fn observe(&self, _app: &str, _root: Option<&str>) -> Result<Screen, Box<DesktopResponse>> {
        self.screens
            .lock()
            .expect("screen lock")
            .pop_front()
            .ok_or_else(|| {
                Box::new(DesktopResponse::err(
                    "snapshot",
                    tinydesktop_bus::DesktopError::new("EMPTY", "no screen"),
                ))
            })
    }

    fn execute(
        &self,
        operation: JevOperation,
        _target: Option<Candidate>,
        _text: Option<String>,
    ) -> DesktopResponse {
        self.operations
            .lock()
            .expect("operation lock")
            .push(operation);
        DesktopResponse::ok("fake", json!({"delivery": "delivered_verified"}))
    }
}

fn clickable_screen() -> Screen {
    Screen {
        app: "Spotify".to_owned(),
        window: Some("Liked Songs".to_owned()),
        surface: "window".to_owned(),
        root: None,
        candidates: vec![Candidate {
            ref_id: "@s1:e1".to_owned(),
            role: "button".to_owned(),
            name: Some("Play First Song by Artist".to_owned()),
            available_actions: vec!["Click".to_owned()],
            bounds: Some(json!({"x": 10.0, "y": 100.0})),
            ..Candidate::default()
        }],
    }
}

fn two_candidate_screen() -> Screen {
    let mut screen = clickable_screen();
    screen.candidates.push(Candidate {
        ref_id: "@s1:e2".to_owned(),
        role: "button".to_owned(),
        name: Some("Play Second Song by Artist".to_owned()),
        available_actions: vec!["Click".to_owned()],
        bounds: Some(json!({"x": 10.0, "y": 160.0})),
        ..Candidate::default()
    });
    screen
}

fn response(operation: &str, probability: f64, target: &str) -> String {
    response_with(operation, probability, target, 0.9, 0.05)
}

fn response_with(
    operation: &str,
    probability: f64,
    target: &str,
    selected_target_probability: f64,
    destructive: f64,
) -> String {
    let remainder = (1.0 - probability) / 3.0;
    let target_probability = if target == "1" {
        selected_target_probability
    } else {
        1.0 - selected_target_probability
    };
    json!({
        "model": "typesafe/jev-1.13-20260917",
        "answers": {
            "operation": {
                "type": "choice", "choice": operation, "confidence": 0.4,
                "probabilities": {
                    "CLICK": if operation == "CLICK" { probability } else { remainder },
                    "WAIT": if operation == "WAIT" { probability } else { remainder },
                    "DONE": if operation == "DONE" { probability } else { remainder },
                    "BLOCKED": if operation == "BLOCKED" { probability } else { remainder }
                }
            },
            "click_target": {
                "type": "choice", "choice": target, "confidence": 0.4,
                "probabilities": {"1": target_probability, "none": 1.0 - target_probability}
            },
            "destructive": {"type": "noul", "noul": destructive}
        },
        "usage": {"input_tokens": 10, "output_tokens": 2}
    })
    .to_string()
}

async fn server(bodies: Vec<String>) -> String {
    let listener = TcpListener::bind("127.0.0.1:0")
        .await
        .expect("loopback binds");
    let address = listener.local_addr().expect("listener has address");
    tokio::spawn(async move {
        for body in bodies {
            let (mut stream, _) = listener.accept().await.expect("request connects");
            let mut request = vec![0_u8; 32_768];
            let _ = stream.read(&mut request).await.expect("request reads");
            let reply = format!(
                "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                body.len()
            );
            stream
                .write_all(reply.as_bytes())
                .await
                .expect("reply writes");
        }
    });
    format!("http://{address}/decisions")
}

async fn runtime(bodies: Vec<String>) -> JevRuntime {
    let endpoint = server(bodies).await;
    let mut config =
        tinyjevclient::ClientConfig::openrouter("test-key").with_endpoint_url(endpoint);
    config.timeout = Duration::from_secs(1);
    config.retry.max_retries = 0;
    JevRuntime {
        client: tinyjevclient::Client::new(config).expect("client config is valid"),
        configuration: tinydesktop_bus::JevConfiguration {
            provider: tinydesktop_bus::JevProvider::OpenRouter,
            model: "jev-latest".to_owned(),
            endpoint_url: None,
        },
    }
}

fn backend(screen_count: usize) -> (FakeBackend, Arc<Mutex<Vec<JevOperation>>>) {
    let operations = Arc::new(Mutex::new(Vec::new()));
    (
        FakeBackend {
            screens: Arc::new(Mutex::new(VecDeque::from(
                (0..screen_count)
                    .map(|_| clickable_screen())
                    .collect::<Vec<_>>(),
            ))),
            operations: Arc::clone(&operations),
        },
        operations,
    )
}

#[tokio::test]
async fn goal_loop_executes_a_safe_choice_then_stops_done() {
    let runtime = runtime(vec![
        response("CLICK", 0.9, "1"),
        response("DONE", 0.9, "none"),
    ])
    .await;
    let (backend, operations) = backend(3);
    let reply = run_goal_with(
        backend,
        runtime,
        RunGoalRequest {
            app: "Spotify".to_owned(),
            goal: "play the topmost song".to_owned(),
            max_steps: 3,
            max_model_calls: 3,
            ..RunGoalRequest::default()
        },
    )
    .await;
    assert!(reply.ok);
    let result: tinydesktop_bus::JevRunResult =
        serde_json::from_value(reply.data.expect("run returns data")).expect("result decodes");
    assert_eq!(result.stop, JevStopReason::Done);
    assert_eq!(result.turns.len(), 1);
    assert_eq!(
        *operations.lock().expect("operation lock"),
        vec![JevOperation::Click]
    );
    assert_eq!((result.metrics.calls, result.metrics.attempts), (2, 2));
}

async fn run_case(
    bodies: Vec<String>,
    screen_count: usize,
    max_steps: u32,
    max_model_calls: u32,
    goal: &str,
) -> tinydesktop_bus::JevRunResult {
    let runtime = runtime(bodies).await;
    let (backend, _) = backend(screen_count);
    let reply = run_goal_with(
        backend,
        runtime,
        RunGoalRequest {
            app: "Spotify".to_owned(),
            goal: goal.to_owned(),
            max_steps,
            max_model_calls,
            ..RunGoalRequest::default()
        },
    )
    .await;
    serde_json::from_value(reply.data.expect("run returns data")).expect("result decodes")
}

#[tokio::test]
async fn goal_loop_reports_terminal_policy_outcomes() {
    let blocked = run_case(
        vec![response("BLOCKED", 0.9, "none")],
        1,
        3,
        3,
        "impossible",
    )
    .await;
    assert_eq!(blocked.stop, JevStopReason::Blocked);

    let confirmation = run_case(
        vec![response_with("CLICK", 0.9, "1", 0.9, 0.9)],
        1,
        3,
        3,
        "play the topmost song",
    )
    .await;
    assert_eq!(confirmation.stop, JevStopReason::ConfirmationRequired);
}

#[tokio::test]
async fn goal_loop_enforces_action_model_and_stall_budgets() {
    let action_budget = run_case(
        vec![response("CLICK", 0.9, "1")],
        2,
        1,
        3,
        "play the topmost song",
    )
    .await;
    assert_eq!(action_budget.stop, JevStopReason::ActionBudget);

    let model_budget = run_case(
        vec![response("CLICK", 0.9, "1")],
        2,
        4,
        1,
        "play the topmost song",
    )
    .await;
    assert_eq!(model_budget.stop, JevStopReason::ModelBudget);

    let stalled = run_case(
        vec![
            response("CLICK", 0.9, "1"),
            response("CLICK", 0.9, "1"),
            response("CLICK", 0.9, "1"),
        ],
        6,
        4,
        4,
        "play the topmost song",
    )
    .await;
    assert_eq!(stalled.stop, JevStopReason::Stalled);
}

#[test]
fn screen_parsing_filters_disabled_nodes_and_builds_descriptions() {
    let reply = DesktopResponse::ok(
        "snapshot",
        json!({
            "app": "Spotify", "window": {"title": "Liked Songs"},
            "tree": {"role": "window", "children": [
                {"ref_id": "@s:e1", "role": "button", "name": "Play First Song by Artist", "available_actions": ["Click"], "children_count": 4},
                {"ref_id": "@s:e2", "role": "button", "name": "Disabled", "available_actions": ["Click"], "states": ["disabled"]}
            ]}
        }),
    );
    let screen = parse_reply(&crate::Desktop::new(), "Spotify", Some("@s:root"), reply)
        .expect("synthetic snapshot parses");
    assert_eq!(screen.candidates.len(), 1);
    assert_eq!(describe(&screen.candidates[0], false)["contains"], json!(4));
    assert!(fingerprint(&screen).contains("Play First Song"));
}

#[test]
fn action_space_and_requests_cover_every_supported_capability() {
    let screen = Screen {
        app: "App".to_owned(),
        window: Some("Window".to_owned()),
        surface: "window".to_owned(),
        root: None,
        candidates: vec![Candidate {
            ref_id: "@s:e1".to_owned(),
            role: "control".to_owned(),
            name: Some("Everything".to_owned()),
            value: Some(json!("held")),
            states: vec!["checked".to_owned()],
            available_actions: vec![
                "Click".to_owned(),
                "SetValue".to_owned(),
                "Toggle".to_owned(),
                "Expand".to_owned(),
                "Collapse".to_owned(),
                "Scroll".to_owned(),
            ],
            children_count: Some(3),
            ..Candidate::default()
        }],
    };
    let space = action_space(&screen, true);
    for operation in [
        "CLICK",
        "TYPE_TEXT",
        "CHECK",
        "UNCHECK",
        "EXPAND",
        "COLLAPSE",
        "SCROLL",
        "DRILL",
    ] {
        assert!(space.targets.contains_key(operation), "missing {operation}");
    }
    let evaluation = request("jev-latest", "change it", &screen, &space, &[], true);
    assert!(evaluation.questions.contains_key("type_text_target"));
    let rerank = rerank_request(
        "jev-latest",
        "change it",
        &screen,
        "CLICK",
        space.targets.get("CLICK").expect("click targets"),
        true,
    );
    assert_eq!(rerank.questions.len(), 1);
}

#[test]
fn answer_helpers_cover_terminal_missing_and_shortlist_paths() {
    let screen = clickable_screen();
    let space = action_space(&screen, false);
    let answer = Answer::Choice(ChoiceAnswer {
        choice: "1".to_owned(),
        probabilities: BTreeMap::from([("1".to_owned(), 0.8), ("none".to_owned(), 0.2)]),
        confidence: 0.2,
    });
    assert!(target(&space, "CLICK", Some(&answer)).is_some());
    assert_eq!(shortlist(&space, "CLICK", Some(&answer)).len(), 1);
    assert!(shortlist(&space, "MISSING", Some(&answer)).is_empty());
    assert!(choice(None).is_none());
    assert!((noul(None) - 1.0).abs() < f64::EPSILON);
    for (wire, operation) in [
        ("CLICK", JevOperation::Click),
        ("TYPE_TEXT", JevOperation::TypeText),
        ("CHECK", JevOperation::Check),
        ("UNCHECK", JevOperation::Uncheck),
        ("EXPAND", JevOperation::Expand),
        ("COLLAPSE", JevOperation::Collapse),
        ("SCROLL", JevOperation::Scroll),
        ("DRILL", JevOperation::Drill),
        ("WIDEN", JevOperation::Widen),
        ("WAIT", JevOperation::Wait),
        ("DONE", JevOperation::Done),
        ("BLOCKED", JevOperation::Blocked),
    ] {
        assert_eq!(parse_operation(wire), Some(operation));
    }
    assert_eq!(parse_operation("NOPE"), None);
}

#[test]
fn screen_helpers_cover_overlay_values_bounds_and_failed_observation() {
    let reply = DesktopResponse::ok(
        "snapshot",
        json!({
            "app": "App",
            "tree": {"role": "sheet", "children": [{
                "ref_id": "@s:e1", "role": "textfield", "value": "private",
                "available_actions": ["SetValue"], "states": ["focused"],
                "bounds": {"x": 1.0, "y": 2.0}
            }]}
        }),
    );
    let screen = parse_reply(&crate::Desktop::new(), "App", None, reply)
        .expect("original synthetic overlay remains usable");
    let with_values = describe(&screen.candidates[0], true);
    assert!(with_values.get("holds").is_some());
    assert!(with_values.get("state").is_some());
    let unnamed = Candidate {
        role: "button".to_owned(),
        bounds: Some(json!({"x": 1.0, "y": 2.0})),
        ..Candidate::default()
    };
    assert!(describe(&unnamed, false).get("bounds").is_some());

    let failed = observe(&crate::Desktop::new(), "__tinydesktop_missing__", None)
        .expect_err("missing app fails");
    assert!(!failed.ok);

    let failed_reply = DesktopResponse::err(
        "snapshot",
        tinydesktop_bus::DesktopError::new("FAIL", "failed"),
    );
    assert!(parse_reply(&crate::Desktop::new(), "App", Some("@s:root"), failed_reply).is_err());
    let no_data = DesktopResponse {
        version: tinydesktop_bus::ENVELOPE_VERSION.to_owned(),
        ok: true,
        command: "snapshot".to_owned(),
        data: None,
        error: None,
    };
    assert!(parse_reply(&crate::Desktop::new(), "App", Some("@s:root"), no_data).is_err());
}

#[test]
fn runtime_configuration_covers_all_providers_and_rejects_empty_keys() {
    for provider in [
        JevProvider::TypeSafe,
        JevProvider::OpenRouter,
        JevProvider::TinyHumansOpenRouter,
    ] {
        let mut request = ConfigureJevRequest::new("key");
        request.provider = provider;
        request.model = Some("jev-test".to_owned());
        request.timeout_ms = Some(500);
        request.max_retries = Some(0);
        request.endpoint_url = Some("http://127.0.0.1:1/decisions".to_owned());
        let runtime = JevRuntime::configure(&request).expect("configuration is valid");
        assert_eq!(runtime.configuration().provider, provider);
    }
    assert!(JevRuntime::configure(&ConfigureJevRequest::default()).is_err());
}

#[test]
fn desktop_execution_dispatches_every_closed_operation_without_panicking() {
    let desktop = crate::Desktop::new();
    let candidate = Candidate {
        ref_id: String::new(),
        ..Candidate::default()
    };
    for operation in [
        JevOperation::Click,
        JevOperation::TypeText,
        JevOperation::Check,
        JevOperation::Uncheck,
        JevOperation::Expand,
        JevOperation::Collapse,
        JevOperation::Scroll,
        JevOperation::Wait,
        JevOperation::Drill,
        JevOperation::Widen,
        JevOperation::Done,
        JevOperation::Blocked,
    ] {
        let reply = execute_desktop(
            &desktop,
            operation,
            Some(&candidate),
            Some("text".to_owned()),
        );
        assert!(!reply.command.is_empty());
    }
}

#[tokio::test]
async fn one_step_resolution_and_public_wrappers_cover_success_and_observation_failure() {
    let runtime = runtime(vec![response("CLICK", 0.9, "1")]).await;
    assert!(format!("{runtime:?}").contains("JevRuntime"));
    let (backend, _) = backend(1);
    let reply = resolve_intent_with(
        backend,
        runtime.clone(),
        tinydesktop_bus::ResolveIntentRequest {
            app: "Spotify".to_owned(),
            intent: "play the topmost song".to_owned(),
            execute: false,
            ..tinydesktop_bus::ResolveIntentRequest::default()
        },
    )
    .await;
    assert!(reply.ok);

    let missing = tinydesktop_bus::ResolveIntentRequest {
        app: "__tinydesktop_missing__".to_owned(),
        intent: "click".to_owned(),
        ..tinydesktop_bus::ResolveIntentRequest::default()
    };
    assert!(
        !resolve_intent(crate::Desktop::new(), runtime.clone(), missing)
            .await
            .ok
    );
    assert!(
        !run_goal(
            crate::Desktop::new(),
            runtime,
            RunGoalRequest {
                app: "__tinydesktop_missing__".to_owned(),
                goal: "finish".to_owned(),
                ..RunGoalRequest::default()
            }
        )
        .await
        .ok
    );
}

#[tokio::test]
async fn one_step_resolution_reranks_a_close_target_shortlist() {
    let first = json!({
        "model": "typesafe/jev-1.13-20260917",
        "answers": {
            "operation": {"type": "choice", "choice": "CLICK", "confidence": 0.4,
                "probabilities": {"CLICK": 0.9, "WAIT": 0.033_333_333_333, "DONE": 0.033_333_333_333, "BLOCKED": 0.033_333_333_334}},
            "click_target": {"type": "choice", "choice": "1", "confidence": 0.3,
                "probabilities": {"1": 0.5, "2": 0.4, "none": 0.1}},
            "destructive": {"type": "noul", "noul": 0.05}
        },
        "usage": {}
    })
    .to_string();
    let reranked = json!({
        "model": "typesafe/jev-1.13-20260917",
        "answers": {
            "target": {"type": "choice", "choice": "2", "confidence": 0.6,
                "probabilities": {"1": 0.1, "2": 0.85, "none": 0.05}}
        },
        "usage": {}
    })
    .to_string();
    let runtime = runtime(vec![first, reranked]).await;
    let backend = FakeBackend {
        screens: Arc::new(Mutex::new(VecDeque::from([two_candidate_screen()]))),
        operations: Arc::new(Mutex::new(Vec::new())),
    };
    let reply = resolve_intent_with(
        backend,
        runtime,
        tinydesktop_bus::ResolveIntentRequest {
            app: "Spotify".to_owned(),
            intent: "activate the second song".to_owned(),
            ..tinydesktop_bus::ResolveIntentRequest::default()
        },
    )
    .await;
    assert!(reply.ok);
    let decision: tinydesktop_bus::JevDecision =
        serde_json::from_value(reply.data.expect("decision data")).expect("decision decodes");
    assert_eq!(decision.target.expect("target").ref_id, "@s1:e2");
}

#[test]
fn response_helpers_classify_provider_failures_and_policy_reasons() {
    for (error, code) in [
        (tinyjevclient::Error::Authentication, "JEV_AUTHENTICATION"),
        (tinyjevclient::Error::RateLimited, "JEV_RATE_LIMITED"),
        (tinyjevclient::Error::Timeout, "JEV_TIMEOUT"),
        (
            tinyjevclient::Error::InvalidResponse {
                reason: "bad".to_owned(),
            },
            "JEV_INVALID_RESPONSE",
        ),
        (
            tinyjevclient::Error::HttpStatus { status: 500 },
            "JEV_PROVIDER_FAILED",
        ),
    ] {
        let failure = tinyjevclient::EvaluationFailure {
            error,
            attempts: 1,
            latency: Duration::ZERO,
        };
        assert_eq!(
            provider_error(&failure)
                .error
                .as_ref()
                .expect("provider error payload")
                .code,
            code
        );
    }
    for decision in [
        JevDecisionKind::Act,
        JevDecisionKind::ConfirmationRequired,
        JevDecisionKind::Abstain,
        JevDecisionKind::NeedsText,
        JevDecisionKind::Done,
        JevDecisionKind::Blocked,
    ] {
        assert!(!reason(decision, 0.5, 0.6).is_empty());
    }
    let target = target_payload(&Candidate {
        ref_id: "@s:e1".to_owned(),
        role: "button".to_owned(),
        description: Some("described".to_owned()),
        ..Candidate::default()
    });
    assert_eq!(target.name.as_deref(), Some("described"));
    assert!(!internal_error("broken").ok);
    assert!(agent_response("test", &json!({"ok": true})).ok);
}
