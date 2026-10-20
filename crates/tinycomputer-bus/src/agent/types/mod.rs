//! Task payloads for the Agent interface.

mod describe;
mod models;
mod reply;
mod report;
mod request;
mod status;

pub use describe::{Capabilities, Example, MemberDoc, SurfaceAvailability};
pub use models::{LanguageModelConfiguration, LanguageModelProvider};
pub use reply::{AGENT_ENVELOPE_VERSION, AgentError, AgentResponse, AgentResponseError};
pub use report::{Rescue, RescueOutcome, TaskReport};
pub use request::{
    AwaitTaskRequest, ContinueTaskRequest, PaymentMode, PlanTaskRequest, StartTaskRequest,
    SurfaceKind, TaskBudget, TaskConstraints, TaskId, TaskOutput, TaskPlan, TaskRef,
    TaskReportRequest,
};
pub use status::{InputField, InputKind, StepView, TaskStatus, TaskView};
