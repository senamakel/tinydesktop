//! Tests for the linked engine that need no browser.
//!
//! agent-browser launches a browser before running any command not on its
//! skip list, so these send only commands that are on it (such as an empty
//! action): the engine answers in its envelope and nothing is launched.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use serde_json::json;
use std::{io, net::SocketAddr, sync::Arc};

use super::{AgentBrowser, ENGINE, EnforcedAgentBrowser, Linked, state_options};
use crate::{
    BrowserDialTarget, DialGrant, EgressAuthorizer,
    engine::{Engine, Launcher},
};
use agent_browser::DaemonState;

#[derive(Debug)]
struct RejectAll;

impl EgressAuthorizer for RejectAll {
    fn authorize(
        &self,
        _: BrowserDialTarget,
    ) -> std::pin::Pin<Box<dyn std::future::Future<Output = io::Result<DialGrant>> + Send>> {
        Box::pin(async { Err(io::Error::new(io::ErrorKind::PermissionDenied, "denied")) })
    }
    fn consume(
        &self,
        _: BrowserDialTarget,
        _: DialGrant,
        _: SocketAddr,
    ) -> std::pin::Pin<Box<dyn std::future::Future<Output = io::Result<()>> + Send>> {
        Box::pin(async { Err(io::Error::new(io::ErrorKind::PermissionDenied, "denied")) })
    }
    fn validate(
        &self,
        _: String,
    ) -> std::pin::Pin<Box<dyn std::future::Future<Output = io::Result<bool>> + Send>> {
        Box::pin(async { Ok(false) })
    }
}

#[tokio::test]
async fn an_empty_command_is_answered_without_launching_a_browser() {
    let mut engine = AgentBrowser.open("unit");
    let reply = engine.execute(json!({"id": "1", "action": ""})).await;
    assert_eq!(reply["success"], false);
    assert!(
        reply["error"]
            .as_str()
            .is_some_and(|error| !error.is_empty())
    );
}

#[tokio::test]
async fn an_invalid_private_session_id_stops_before_binding_a_proxy() {
    let mut engine = Linked {
        state: agent_browser::DaemonState::with_options(super::state_options("invalid-id")),
        egress_authorizer: Some(Arc::new(RejectAll)),
        proxy: None,
        proxy_session_id: "too-short".into(),
        enforced_launch: false,
        #[cfg(feature = "test-connector")]
        test_connector: None,
    };
    let reply = engine
        .execute(json!({"id":"launch","action":"launch"}))
        .await;
    assert_eq!(reply["success"], false);
    assert!(
        reply["error"]
            .as_str()
            .unwrap()
            .contains("private session identity")
    );
    assert!(engine.proxy.is_none());
}

#[tokio::test]
async fn proxy_is_bound_before_caller_launch_options_are_refused() {
    let mut engine = Linked {
        state: agent_browser::DaemonState::with_options(super::state_options("refused-args")),
        egress_authorizer: Some(Arc::new(RejectAll)),
        proxy: None,
        proxy_session_id: "0123456789abcdef0123456789abcdef0123456789abcdef".into(),
        enforced_launch: false,
        #[cfg(feature = "test-connector")]
        test_connector: None,
    };
    let reply = engine
        .execute(json!({"id":"launch","action":"launch","args":["--proxy-server=http://caller"]}))
        .await;
    assert_eq!(reply["success"], false);
    assert!(reply["error"].as_str().unwrap().contains("caller-supplied"));
    assert!(engine.proxy.is_some());
}

#[test]
fn failed_entropy_does_not_create_a_private_session_id() {
    assert_eq!(super::private_session_id_from_entropy([0; 24], false), "");
    let id = super::private_session_id_from_entropy([0xab; 24], true);
    assert_eq!(id, "ab".repeat(24));
}

#[cfg(feature = "test-connector")]
#[tokio::test]
async fn fixture_connector_is_selected_for_the_session_proxy() {
    let launcher =
        EnforcedAgentBrowser::new(Arc::new(RejectAll)).with_test_connector(Arc::new(|_| {
            Box::pin(async { Err(io::Error::other("fixture not contacted")) })
        }));
    let mut engine = launcher.open("fixture-connector");
    let reply = engine
        .execute(json!({"id":"navigate","action":"navigate","url":"https://example.test"}))
        .await;
    assert_eq!(reply["success"], false);
}

#[tokio::test]
async fn enforced_launcher_creates_a_private_session_without_implicit_launch() {
    let launcher = super::EnforcedAgentBrowser::new(Arc::new(RejectAll));
    assert_eq!(
        format!("{launcher:?}"),
        "EnforcedAgentBrowser([host authorizer])"
    );

    let mut engine = launcher.open("private-launcher");
    let reply = engine
        .execute(json!({"id": "read", "action": "read"}))
        .await;

    assert_eq!(reply["success"], false);
    assert!(
        reply["error"]
            .as_str()
            .is_some_and(|error| error.contains("enforced launch with a live browser session"))
    );
}

#[tokio::test]
async fn enforced_launcher_binds_its_proxy_on_loopback_before_launch() {
    let linked = Linked {
        state: DaemonState::with_options(state_options("proxy-bind")),
        egress_authorizer: Some(Arc::new(RejectAll)),
        proxy: None,
        proxy_session_id: "private-session-id".into(),
        enforced_launch: false,
        #[cfg(feature = "test-connector")]
        test_connector: None,
    };

    let proxy = linked
        .bind_egress_proxy(Arc::new(RejectAll))
        .await
        .expect("the local egress proxy binds without opening a browser");

    assert!(proxy.url().starts_with("http://127.0.0.1:"));
}

#[tokio::test]
async fn enforced_browser_refuses_implicit_launch_before_any_request() {
    let mut engine = EnforcedAgentBrowser::new(Arc::new(RejectAll)).open("unit");
    let reply = engine
        .execute(json!({"id":"navigate","action":"navigate","url":"http://127.0.0.1/"}))
        .await;
    assert_eq!(reply["success"], false);
    assert!(reply["error"].as_str().unwrap().contains("enforced launch"));
}

#[tokio::test]
async fn a_closed_or_lost_browser_cannot_implicitly_launch_without_the_proxy() {
    let mut engine = Linked {
        state: DaemonState::with_options(state_options("closed")),
        egress_authorizer: Some(Arc::new(RejectAll)),
        proxy: None,
        proxy_session_id: "private-session-id".into(),
        enforced_launch: true,
        #[cfg(feature = "test-connector")]
        test_connector: None,
    };
    let reply = engine
        .execute(json!({"id":"navigate","action":"navigate","url":"http://public.example/"}))
        .await;
    assert_eq!(reply["success"], false);
    assert!(
        reply["error"]
            .as_str()
            .unwrap()
            .contains("live browser session")
    );
}

#[tokio::test]
async fn an_exited_enforced_browser_can_still_be_closed() {
    let mut engine = Linked {
        state: DaemonState::with_options(state_options("close-exited")),
        egress_authorizer: Some(Arc::new(RejectAll)),
        proxy: None,
        proxy_session_id: "private-session-id".into(),
        enforced_launch: true,
        #[cfg(feature = "test-connector")]
        test_connector: None,
    };

    let reply = engine.execute(json!({"id":"close","action":"close"})).await;

    assert_eq!(reply["success"], true);
    assert!(!engine.enforced_launch);
    assert!(engine.proxy.is_none());
}

#[tokio::test]
async fn enforced_browser_refuses_process_side_reads_with_a_url() {
    let mut engine = Linked {
        state: DaemonState::with_options(state_options("process-read")),
        egress_authorizer: Some(Arc::new(RejectAll)),
        proxy: None,
        proxy_session_id: "private-session-id".into(),
        enforced_launch: true,
        #[cfg(feature = "test-connector")]
        test_connector: None,
    };
    let reply = engine
        .execute(json!({
            "id":"read",
            "action":"read",
            "url":"http://public.example/"
        }))
        .await;
    assert_eq!(reply["success"], false);
    assert!(
        reply["error"]
            .as_str()
            .unwrap()
            .contains("process-side reads with a URL")
    );
}

#[tokio::test]
async fn enforced_browser_rechecks_liveness_after_waiting_for_engine_lock() {
    let engine = Linked {
        state: DaemonState::with_options(state_options("closed-after-lock")),
        egress_authorizer: Some(Arc::new(RejectAll)),
        proxy: None,
        proxy_session_id: "private-session-id".into(),
        enforced_launch: true,
        #[cfg(feature = "test-connector")]
        test_connector: None,
    };
    let serialized = ENGINE.lock().await;
    let pending = tokio::spawn(async move {
        let mut engine = engine;
        engine
            .execute(json!({
                "id":"navigate",
                "action":"navigate",
                "url":"http://public.example/"
            }))
            .await
    });
    tokio::task::yield_now().await;
    assert!(!pending.is_finished(), "the call should wait for ENGINE");
    drop(serialized);

    let reply = pending.await.unwrap();
    assert_eq!(reply["success"], false);
    assert!(
        reply["error"]
            .as_str()
            .unwrap()
            .contains("live browser session")
    );
}

mod launch_tests;
