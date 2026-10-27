//! Tests for the linked engine that need no browser.
//!
//! agent-browser launches a browser before running any command not on its
//! skip list, so these send only commands that are on it (such as an empty
//! action): the engine answers in its envelope and nothing is launched.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use serde_json::json;
use std::{io, net::SocketAddr, sync::Arc};

use super::{AgentBrowser, EnforcedAgentBrowser};
use crate::{BrowserDialTarget, DialGrant, EgressAuthorizer, engine::Launcher};

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
async fn an_enforced_launcher_refuses_commands_until_a_private_proxy_launch() {
    let mut engine = EnforcedAgentBrowser::new(Arc::new(RejectAll)).open("unit");
    let reply = engine
        .execute(json!({"id":"navigate","action":"navigate","url":"https://example.test"}))
        .await;
    assert_eq!(reply["success"], false);
    assert!(reply["error"].as_str().unwrap().contains("enforced launch"));
}

#[test]
fn the_enforced_launch_replaces_caller_network_arguments() {
    assert!(super::enforce_launch_with_config(
        json!({"action":"launch","args":["--proxy-server=http://attacker","--user-data-dir=/tmp/custom"]}),
        "http://127.0.0.1:43210",
        "private-password",
        false,
    )
    .is_err());
    let command = super::enforce_launch_with_config(
        json!({"action":"launch"}),
        "http://127.0.0.1:43210",
        "private-password",
        false,
    )
    .expect("host controlled launch config");
    assert_eq!(command["proxy"]["server"], "http://127.0.0.1:43210");
    assert_eq!(command["proxy"]["password"], "private-password");
    assert!(
        command["args"]
            .as_array()
            .unwrap()
            .iter()
            .all(|arg| { !arg.as_str().unwrap_or_default().contains("attacker") })
    );
}
