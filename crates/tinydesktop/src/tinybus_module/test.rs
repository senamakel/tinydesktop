//! Tests for the `TinyBus` module adapter and its declared surface.
//!
//! These run against the in-memory transport rather than a loaded `cdylib`, so
//! they exercise the dispatch table and the envelope without needing a display
//! server or a granted permission. `examples/verify_module.rs` covers the real
//! dynamic loader.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use super::{DesktopService, setup};
use serde_json::json;
use tinybus::broker::Broker;
use tinybus::transport::memory::MemoryBus;
use tinybus::{Connection, Interface};
use tinydesktop_bus::{DesktopResponse, PermissionsRequest, names};

/// The declared manifest method list, parsed back out of the descriptor the
/// module exports.
fn manifest_methods() -> Vec<String> {
    super::MODULE_MANIFEST
        .methods
        .iter()
        .map(|method| (*method).to_owned())
        .collect()
}

fn service() -> DesktopService {
    DesktopService::from_config(&json!({})).expect("an empty configuration is valid")
}

#[test]
fn declared_methods_match_the_dispatch_table() {
    let methods = service()
        .members()
        .into_iter()
        .map(|member| member.to_string())
        .collect::<Vec<_>>();

    assert_eq!(methods, names::METHODS.to_vec());
}

#[test]
fn the_embedded_manifest_matches_the_contract() {
    assert_eq!(manifest_methods(), names::METHODS.to_vec());
}

#[test]
fn the_served_interface_name_matches_the_contract() {
    assert_eq!(service().name().to_string(), names::INTERFACE);
}

#[test]
fn the_type_member_keeps_the_engines_spelling_rather_than_the_rust_one() {
    // The Rust method is `type_text` because `type` is a keyword; the wire name
    // has to stay `Type` to match the contract a host spells.
    let members = service()
        .members()
        .into_iter()
        .map(|member| member.to_string())
        .collect::<Vec<_>>();

    assert!(members.contains(&"Type".to_owned()));
    assert!(!members.contains(&"TypeText".to_owned()));
}

#[test]
fn a_malformed_configuration_is_rejected_rather_than_defaulted() {
    let error = DesktopService::from_config(&json!({ "session_id": 7 }))
        .expect_err("a numeric session id is not a session id");

    assert!(error.to_string().contains("session_id"));
}

#[tokio::test]
async fn the_module_answers_a_command_over_a_real_bus() -> tinybus::Result<()> {
    let bus = MemoryBus::new();
    Broker::new().spawn(bus.clone());

    let service = Connection::connect(bus.connect().await?).await?;
    setup(service.clone(), json!({})).await?;

    let client = Connection::connect(bus.connect().await?).await?;
    let proxy = client.proxy(names::INTERFACE, names::OBJECT_PATH, names::INTERFACE)?;
    let reply: DesktopResponse = proxy.call(names::methods::VERSION, ()).await?;

    assert!(reply.ok, "version needs no permission and no display");
    assert_eq!(reply.command, "version");
    assert!(reply.data.is_some());
    Ok(())
}

#[tokio::test]
async fn a_member_taking_a_payload_round_trips_it() -> tinybus::Result<()> {
    let bus = MemoryBus::new();
    Broker::new().spawn(bus.clone());

    let service = Connection::connect(bus.connect().await?).await?;
    setup(service.clone(), json!({})).await?;

    let client = Connection::connect(bus.connect().await?).await?;
    let proxy = client.proxy(names::INTERFACE, names::OBJECT_PATH, names::INTERFACE)?;
    let reply: DesktopResponse = proxy
        .call(
            names::methods::PERMISSIONS,
            (PermissionsRequest { request: false },),
        )
        .await?;

    // Reporting permissions works everywhere; whether they are granted is the
    // machine's business, not this test's.
    assert_eq!(reply.command, "permissions");
    Ok(())
}

#[tokio::test]
async fn a_reserved_hold_member_fails_closed_over_the_bus() -> tinybus::Result<()> {
    let bus = MemoryBus::new();
    Broker::new().spawn(bus.clone());

    let service = Connection::connect(bus.connect().await?).await?;
    setup(service.clone(), json!({})).await?;

    let client = Connection::connect(bus.connect().await?).await?;
    let proxy = client.proxy(names::INTERFACE, names::OBJECT_PATH, names::INTERFACE)?;
    let reply: DesktopResponse = proxy
        .call(
            names::methods::KEY_DOWN,
            (json!({ "combo": "shift", "force": false }),),
        )
        .await?;

    // It answers rather than erroring at the transport, and it says no.
    assert!(!reply.ok);
    assert!(reply.error.is_some());
    Ok(())
}

#[tokio::test]
async fn an_unknown_member_is_a_transport_error_not_an_envelope() -> tinybus::Result<()> {
    let bus = MemoryBus::new();
    Broker::new().spawn(bus.clone());

    let service = Connection::connect(bus.connect().await?).await?;
    setup(service.clone(), json!({})).await?;

    let client = Connection::connect(bus.connect().await?).await?;
    let proxy = client.proxy(names::INTERFACE, names::OBJECT_PATH, names::INTERFACE)?;
    let result = proxy.call::<DesktopResponse>("NoSuchMember", ()).await;

    // The envelope carries command failures; a member that does not exist is a
    // different kind of problem and belongs on the other channel.
    assert!(result.is_err());
    Ok(())
}
