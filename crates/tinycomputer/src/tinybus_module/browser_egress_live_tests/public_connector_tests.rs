//! Live module test for the fixture-only public-address connector override.
#![allow(clippy::expect_used, clippy::panic, clippy::unwrap_used)]

use super::*;
use std::{future::Future, io, net::SocketAddr, pin::Pin};
use tinycomputer_browser::TestConnector;
use tokio::net::TcpStream;

#[tokio::test]
async fn loaded_module_consumes_public_socket_before_fixture_mapping() -> tinybus::Result<()> {
    let local_listener = tokio::net::TcpListener::bind("127.0.0.1:0").await?;
    let page_port = local_listener.local_addr()?.port();
    let public_socket = SocketAddr::from(([8, 8, 8, 8], page_port));
    let calls = Arc::new(Mutex::new(Vec::<String>::new()));
    let connector_calls = calls.clone();
    let fixture_address = local_listener.local_addr()?;
    let connector: TestConnector = Arc::new(move |address| {
        let calls = connector_calls.clone();
        Box::pin(async move {
            assert_eq!(address, public_socket);
            calls.lock().unwrap().push(format!("connect:{address}"));
            // Test-only mapping: the proxy authorized 8.8.8.8, but this closure
            // makes no public connection and reaches only the local fixture.
            TcpStream::connect(fixture_address).await
        }) as Pin<Box<dyn Future<Output = io::Result<TcpStream>> + Send>>
    });
    let policy_id = "fixture-public-control";
    let generation = 973;
    let _connector_guard = crate::test_support::install_browser_egress_connector(
        policy_id.into(),
        generation,
        connector,
    )
    .expect("fixture connector installs once");

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
        Some(calls.clone()),
    )
    .await?;

    assert_eq!(browser.navigation["title"], "public fixture");
    let expected_authorize = format!("authorize:{policy_id}:{generation}:fixture.test:{page_port}");
    let expected_consume = format!("consume:{public_socket}");
    let observed = browser.observations.lock().unwrap().clone();
    assert!(observed.iter().any(|call| call == &expected_authorize));
    assert!(observed.iter().any(|call| call == &expected_consume));
    let observed_calls = calls.lock().unwrap().clone();
    assert_eq!(
        &observed_calls[..4],
        [
            expected_authorize,
            expected_consume,
            format!("connect:{public_socket}"),
            "validate:fixture-lease".into(),
        ],
        "the exact TinyBus-consumed socket is passed to the fixture connector before lease validation"
    );
    assert!(
        observed_calls[4..]
            .iter()
            .all(|call| call.starts_with("authorize:") && call.contains(":www.google.com:443")),
        "background public requests must stop at authorization without consume or connector: {observed_calls:?}"
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
