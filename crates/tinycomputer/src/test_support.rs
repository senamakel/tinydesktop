//! Feature-gated controls used only by host/module integration tests.

use std::sync::{Mutex, OnceLock};

use tinycomputer_browser::TestConnector;

static BROWSER_EGRESS_CONNECTOR: OnceLock<Mutex<Option<ScopedConnector>>> = OnceLock::new();

struct ScopedConnector {
    policy_id: String,
    generation: u64,
    connector: TestConnector,
}

/// Installs a fixture-only replacement for the browser proxy's final TCP connect.
///
/// The normal TinyComputer module setup still constructs its authenticated
/// `BusAuthorizer` and `EnforcedAgentBrowser`; only the final connector changes.
/// The connector receives the exact address selected and consumed by the
/// authorizer and must map it to a local fixture listener. The returned guard
/// clears the override when dropped.
///
/// # Errors
///
/// Returns an error if another fixture already installed an override or the
/// process-local test hook was poisoned.
pub fn install_browser_egress_connector(
    policy_id: String,
    generation: u64,
    connector: TestConnector,
) -> Result<BrowserEgressConnectorGuard, &'static str> {
    if policy_id.is_empty() {
        return Err("browser egress connector scope requires a policy id");
    }
    let mut installed = connector_slot()
        .lock()
        .map_err(|_| "browser egress connector test hook was poisoned")?;
    if installed.is_some() {
        return Err("a browser egress connector override is already installed");
    }
    *installed = Some(ScopedConnector {
        policy_id,
        generation,
        connector,
    });
    Ok(BrowserEgressConnectorGuard)
}

/// Guard that removes a previously installed fixture connector.
#[derive(Debug)]
pub struct BrowserEgressConnectorGuard;

impl Drop for BrowserEgressConnectorGuard {
    fn drop(&mut self) {
        if let Ok(mut installed) = connector_slot().lock() {
            installed.take();
        }
    }
}

pub(crate) fn browser_egress_connector(policy_id: &str, generation: u64) -> Option<TestConnector> {
    let installed = connector_slot().lock().ok()?;
    installed.as_ref().and_then(|scoped| {
        (scoped.policy_id == policy_id && scoped.generation == generation)
            .then(|| scoped.connector.clone())
    })
}

fn connector_slot() -> &'static Mutex<Option<ScopedConnector>> {
    BROWSER_EGRESS_CONNECTOR.get_or_init(|| Mutex::new(None))
}
