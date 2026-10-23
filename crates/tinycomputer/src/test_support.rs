//! Feature-gated controls used only by host/module integration tests.

use std::sync::{Mutex, OnceLock};

use tinycomputer_browser::TestConnector;

static BROWSER_EGRESS_CONNECTOR: OnceLock<Mutex<Option<TestConnector>>> = OnceLock::new();

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
    connector: TestConnector,
) -> Result<BrowserEgressConnectorGuard, &'static str> {
    let mut installed = connector_slot()
        .lock()
        .map_err(|_| "browser egress connector test hook was poisoned")?;
    if installed.is_some() {
        return Err("a browser egress connector override is already installed");
    }
    *installed = Some(connector);
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

pub(crate) fn browser_egress_connector() -> Option<TestConnector> {
    connector_slot().lock().ok()?.clone()
}

fn connector_slot() -> &'static Mutex<Option<TestConnector>> {
    BROWSER_EGRESS_CONNECTOR.get_or_init(|| Mutex::new(None))
}
