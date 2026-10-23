//! Feature-gated integration tests for host-provided fixture connectors.
#![cfg(feature = "test-connector")]

use std::{
    future::Future,
    io,
    net::SocketAddr,
    pin::Pin,
    sync::{Arc, Mutex},
};

use base64::Engine as _;
use tinycomputer_browser::{BrowserDialTarget, DialGrant, EgressAuthorizer, EgressProxy};
use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    net::{TcpListener, TcpStream},
};

const SESSION: &str = "fixture-public-dial-session";
const PASSWORD: &str = "fixture-password";

struct ExactPublicGrant {
    address: SocketAddr,
    calls: Arc<Mutex<Vec<String>>>,
}

impl EgressAuthorizer for ExactPublicGrant {
    fn authorize(
        &self,
        target: BrowserDialTarget,
    ) -> Pin<Box<dyn Future<Output = io::Result<DialGrant>> + Send>> {
        let address = self.address;
        let calls = self.calls.clone();
        Box::pin(async move {
            assert_eq!(target.hostname, "8.8.8.8");
            assert_eq!(target.port, address.port());
            calls.lock().unwrap().push(format!("authorize:{address}"));
            Ok(DialGrant {
                lease: "fixture-only-lease".into(),
                addresses: vec![address],
            })
        })
    }

    fn consume(
        &self,
        target: BrowserDialTarget,
        grant: DialGrant,
        address: SocketAddr,
    ) -> Pin<Box<dyn Future<Output = io::Result<()>> + Send>> {
        let expected = self.address;
        let calls = self.calls.clone();
        Box::pin(async move {
            assert_eq!(target.hostname, "8.8.8.8");
            assert_eq!(address, expected);
            assert_eq!(grant.addresses, [expected]);
            calls.lock().unwrap().push(format!("consume:{address}"));
            Ok(())
        })
    }

    fn validate(&self, lease: String) -> Pin<Box<dyn Future<Output = io::Result<bool>> + Send>> {
        Box::pin(async move { Ok(lease == "fixture-only-lease") })
    }
}

#[tokio::test]
async fn proxy_consumes_the_exact_public_grant_before_fixture_connector_mapping() {
    let fixture_listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let fixture_address = fixture_listener.local_addr().unwrap();
    let granted_address = SocketAddr::from(([8, 8, 8, 8], fixture_address.port()));
    let calls = Arc::new(Mutex::new(Vec::new()));
    let connector_calls = calls.clone();
    let auth: Arc<dyn EgressAuthorizer> = Arc::new(ExactPublicGrant {
        address: granted_address,
        calls: calls.clone(),
    });

    let proxy = EgressProxy::bind_with_test_connector(
        auth,
        SESSION.into(),
        PASSWORD.into(),
        move |address| {
            let fixture_address = fixture_address;
            let calls = connector_calls.clone();
            Box::pin(async move {
                // The fixture mapper observes the authorized public socket but
                // deliberately connects only to its local listener.
                assert_eq!(address, granted_address);
                calls.lock().unwrap().push(format!("connect:{address}"));
                TcpStream::connect(fixture_address).await
            })
        },
    )
    .await
    .unwrap();

    let fixture = tokio::spawn(async move {
        let (mut stream, _) = tokio::time::timeout(
            std::time::Duration::from_secs(5),
            fixture_listener.accept(),
        )
        .await
        .expect("fixture connector reaches the local listener")
        .unwrap();
        let mut request = vec![0; 2048];
        let read = stream.read(&mut request).await.unwrap();
        let request = String::from_utf8_lossy(&request[..read]);
        assert!(request.starts_with("GET /control HTTP/1.1\r\n"));
        assert!(request.contains(&format!("Host: 8.8.8.8:{}\r\n", fixture_address.port())));
        stream
            .write_all(b"HTTP/1.1 200 OK\r\nContent-Length: 7\r\nConnection: close\r\n\r\nfixture")
            .await
            .unwrap();
    });

    let mut client = TcpStream::connect(proxy.url().replace("http://", ""))
        .await
        .unwrap();
    let auth = base64::engine::general_purpose::STANDARD.encode(format!("{SESSION}:{PASSWORD}"));
    client
        .write_all(
            format!(
                "GET http://8.8.8.8:{}/control HTTP/1.1\r\nHost: 8.8.8.8:{}\r\nProxy-Authorization: Basic {auth}\r\nConnection: close\r\n\r\n",
                granted_address.port(),
                granted_address.port(),
            )
            .as_bytes(),
        )
        .await
        .unwrap();
    let mut response = Vec::new();
    tokio::time::timeout(
        std::time::Duration::from_secs(5),
        client.read_to_end(&mut response),
    )
    .await
    .expect("the proxy forwards and closes the fixture response")
    .unwrap();
    let response = String::from_utf8_lossy(&response);
    assert!(
        response.starts_with("HTTP/1.1 200 OK\r\n"),
        "unexpected proxy response {response:?}; calls {:?}",
        calls.lock().unwrap()
    );
    assert!(response.ends_with("fixture"));
    fixture.await.unwrap();
    assert_eq!(
        *calls.lock().unwrap(),
        [
            format!("authorize:{granted_address}"),
            format!("consume:{granted_address}"),
            format!("connect:{granted_address}"),
        ],
        "authorization and one-use consumption must precede the fixture-only connector"
    );
}
