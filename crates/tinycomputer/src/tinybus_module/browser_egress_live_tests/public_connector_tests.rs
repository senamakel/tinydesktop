//! Live module test for the fixture-only public-address connector override.
#![allow(clippy::expect_used, clippy::panic, clippy::unwrap_used)]

use super::*;
use std::net::SocketAddr;

#[tokio::test]
async fn loaded_module_consumes_public_socket_before_fixture_mapping() -> tinybus::Result<()> {
    let local_listener = tokio::net::TcpListener::bind("127.0.0.1:0").await?;
    let page_port = local_listener.local_addr()?.port();
    let public_socket = SocketAddr::from(([8, 8, 8, 8], page_port));
    let fixture_address = local_listener.local_addr()?;
    let policy_id = "fixture-public-control";
    let generation = 973;

    let body = b"<!doctype html><link rel=\"icon\" href=\"data:,\"><title>public fixture</title>";
    let extra_connections = Arc::new(AtomicUsize::new(0));
    let (served_tx, served_rx) = tokio::sync::oneshot::channel();
    let fixture = tokio::spawn(serve_fixture(
        local_listener,
        body.to_vec(),
        extra_connections,
        served_tx,
        None,
    ));
    let browser = start_browser_with_fixture_hosts_for_policy(
        vec![public_socket.to_string()],
        page_port,
        "/public-control",
        served_rx,
        policy_id,
        generation,
        None,
        Some((public_socket.to_string(), fixture_address.to_string())),
    )
    .await?;

    assert_eq!(browser.navigation["title"], "public fixture");
    let expected_authorize = format!("authorize:{policy_id}:{generation}:fixture.test:{page_port}");
    let expected_consume = format!("consume:{public_socket}");
    let observed = browser.observations.lock().unwrap().clone();
    assert_eq!(
        &observed[..3],
        [
            expected_authorize,
            expected_consume,
            "validate:fixture-lease".into()
        ],
        "TinyComputer's BusAuthorizer consumes exactly the granted public socket before forwarding"
    );
    assert!(
        observed[3..]
            .iter()
            .all(|call| call.starts_with("authorize:") && call.contains(":www.google.com:443")),
        "background public requests must stop at authorization without a grant or connector: {observed:?}"
    );

    let closed: DesktopResponse = browser
        .proxy
        .call(
            browser_names::methods::CLOSE_SESSION,
            (serde_json::json!({"session": browser.session}),),
        )
        .await?;
    assert!(
        closed.ok,
        "browser session did not close: {:?}",
        closed.error
    );
    browser.broker_task.abort();
    fixture.abort();
    Ok(())
}
