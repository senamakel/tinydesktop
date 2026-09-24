//! Jev configuration, one-step resolution, and goal-running payloads.

mod types;

pub use types::{
    JevConfig, JevConfiguration, JevDecision, JevDecisionKind, JevMetrics, JevOperation,
    JevProvider, JevRunResult, JevStopReason, JevTarget, JevTurn, ResolveIntentRequest,
    RunGoalRequest,
};

#[cfg(test)]
mod test;
