//! Private callback contract for the module's authenticated browser egress.

mod types;
pub use types::{
    BrowserEgressConsumeRequest, BrowserEgressDecision, BrowserEgressGrant, BrowserEgressTarget,
    BrowserEgressValidateRequest,
};

/// Host interface for TinySecurity-backed browser egress decisions.
pub const INTERFACE: &str = "ai.tinyhumans.tinycomputer.BrowserEgressHost";
/// Object path for [`INTERFACE`].
pub const OBJECT_PATH: &str = "/ai/tinyhumans/tinycomputer/BrowserEgressHost";

/// Callback members in dispatch order.
pub mod methods {
    /// Authorize one actual browser destination.
    pub const AUTHORIZE: &str = "Authorize";
    /// Consume a one-use grant for an exact socket address.
    pub const CONSUME: &str = "Consume";
    /// Validate a live tunnel lease.
    pub const VALIDATE: &str = "Validate";
}

/// Callback names in dispatch order.
pub const METHODS: [&str; 3] = [methods::AUTHORIZE, methods::CONSUME, methods::VALIDATE];

#[cfg(test)]
mod browser_egress_tests;
