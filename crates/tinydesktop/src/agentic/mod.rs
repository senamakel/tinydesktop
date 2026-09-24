//! Native Jev-backed observation, intent resolution, and goal execution.

mod policy;
mod screen;

#[cfg(test)]
mod test;

use std::time::Duration;

use serde_json::json;
use tinydesktop_bus::{
    DesktopError, DesktopResponse, JevConfig, JevConfiguration, JevDecision, JevDecisionKind,
    JevMetrics, JevOperation, JevProvider, JevRunResult, JevStopReason, JevTarget, JevTurn,
    RefRequest, ResolveIntentRequest, RunGoalRequest, ScrollRequest, SetValueRequest, WaitRequest,
};
use tinyjevclient::{Client, ClientConfig, Error as JevError, EvaluationResult};

use crate::Desktop;
use policy::{
    action_space, choice, exact_named_match, gate_with_evidence, noul, parse_operation,
    playing_goal_satisfied, positional_match, shortlist, target,
};
use screen::{Candidate, Screen, fingerprint, observe};

/// Configured Jev transport and non-secret policy metadata.
#[derive(Clone)]
pub(crate) struct JevRuntime {
    client: Client,
    configuration: JevConfiguration,
}

impl std::fmt::Debug for JevRuntime {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("JevRuntime")
            .field("client", &self.client)
            .field("configuration", &self.configuration)
            .finish()
    }
}

impl JevRuntime {
    pub(crate) fn configure(request: &JevConfig) -> Result<Self, Box<DesktopError>> {
        let mut config = match request.provider {
            JevProvider::TypeSafe => ClientConfig::new(request.api_key()),
            JevProvider::OpenRouter => ClientConfig::openrouter(request.api_key()),
            JevProvider::TinyHumansOpenRouter => {
                ClientConfig::tinyhumans_openrouter(request.api_key())
            }
        };
        if let Some(endpoint) = &request.endpoint_url {
            config = config.with_endpoint_url(endpoint);
        }
        if let Some(timeout_ms) = request.timeout_ms {
            config.timeout = Duration::from_millis(timeout_ms);
        }
        if let Some(max_retries) = request.max_retries {
            config.retry.max_retries = max_retries;
        }
        let client = Client::new(config).map_err(|error| config_error(&error))?;
        Ok(Self {
            client,
            configuration: JevConfiguration {
                provider: request.provider,
                model: request
                    .model
                    .clone()
                    .unwrap_or_else(|| "jev-latest".to_owned()),
                endpoint_url: request.endpoint_url.clone(),
            },
        })
    }
}

pub(crate) async fn resolve_intent(
    desktop: Desktop,
    runtime: JevRuntime,
    request: ResolveIntentRequest,
) -> DesktopResponse {
    resolve_intent_with(desktop, runtime, request).await
}

async fn resolve_intent_with<B: AgentBackend>(
    backend: B,
    runtime: JevRuntime,
    request: ResolveIntentRequest,
) -> DesktopResponse {
    let result = resolve(
        &backend,
        &runtime,
        &request.intent,
        &request.app,
        request.root.as_deref(),
        request.text.as_deref(),
        request.include_values,
        request.execute,
        &[],
    )
    .await;
    match result {
        Ok(outcome) => response("resolve-intent", &outcome.decision),
        Err(error) => *error,
    }
}

pub(crate) async fn run_goal(
    desktop: Desktop,
    runtime: JevRuntime,
    request: RunGoalRequest,
) -> DesktopResponse {
    run_goal_with(desktop, runtime, request).await
}

async fn run_goal_with<B: AgentBackend>(
    backend: B,
    runtime: JevRuntime,
    request: RunGoalRequest,
) -> DesktopResponse {
    let max_steps = request.max_steps.clamp(1, 40);
    let max_calls = request.max_model_calls.clamp(1, 80);
    let mut texts = request.text.into_iter();
    let mut next_text = texts.next();
    let mut root = request.root;
    let mut turns = Vec::new();
    let mut history = Vec::new();
    let mut metrics = JevMetrics::default();
    let mut unchanged = 0_u32;

    loop {
        if u32::try_from(turns.len()).unwrap_or(u32::MAX) >= max_steps {
            return run_response(JevStopReason::ActionBudget, turns, None, metrics);
        }
        if metrics.calls >= max_calls {
            return run_response(JevStopReason::ModelBudget, turns, None, metrics);
        }
        let before = match observe_async(backend.clone(), request.app.clone(), root.clone()).await {
            Ok(screen) => screen,
            Err(error) => return *error,
        };
        let before_fingerprint = fingerprint(&before);
        let outcome = resolve_on_screen(
            &backend,
            &runtime,
            &request.goal,
            &before,
            next_text.as_deref(),
            request.include_values,
            true,
            &history,
        )
        .await;
        let outcome = match outcome {
            Ok(outcome) => outcome,
            Err(error) => return *error,
        };
        for evaluation in &outcome.evaluations {
            merge_metrics(&mut metrics, evaluation);
        }
        let decision = outcome.decision;
        let stop = match decision.decision {
            JevDecisionKind::Done => Some(JevStopReason::Done),
            JevDecisionKind::Blocked => Some(JevStopReason::Blocked),
            JevDecisionKind::ConfirmationRequired => Some(JevStopReason::ConfirmationRequired),
            JevDecisionKind::Abstain => Some(JevStopReason::LowConfidence),
            JevDecisionKind::NeedsText => Some(JevStopReason::NeedsText),
            JevDecisionKind::Act => None,
        };
        if let Some(stop) = stop {
            return run_response(stop, turns, Some(decision), metrics);
        }
        if decision.operation == JevOperation::TypeText {
            next_text = texts.next();
        }
        if decision.operation == JevOperation::Drill {
            root = decision.target.as_ref().map(|target| target.ref_id.clone());
        } else if decision.operation == JevOperation::Widen {
            root = None;
        }
        let after = observe_async(backend.clone(), request.app.clone(), root.clone()).await;
        let changed = after
            .as_ref()
            .is_ok_and(|screen| fingerprint(screen) != before_fingerprint)
            || matches!(
                decision.operation,
                JevOperation::Drill | JevOperation::Widen
            );
        unchanged = if changed {
            0
        } else {
            unchanged.saturating_add(1)
        };
        let turn = JevTurn {
            step: u32::try_from(turns.len())
                .unwrap_or(u32::MAX)
                .saturating_add(1),
            operation: decision.operation,
            target: decision.target.clone(),
            confidence: decision.confidence,
            ok: decision.executed,
            changed,
        };
        history.push(format!(
            "step {}: {:?} {} and changed={changed}",
            turn.step,
            turn.operation,
            turn.target
                .as_ref()
                .and_then(|target| target.name.as_deref())
                .unwrap_or("the selected element")
        ));
        turns.push(turn);
        if unchanged >= 3 {
            return run_response(JevStopReason::Stalled, turns, None, metrics);
        }
    }
}

fn run_response(
    stop: JevStopReason,
    turns: Vec<JevTurn>,
    pending: Option<JevDecision>,
    metrics: JevMetrics,
) -> DesktopResponse {
    response(
        "run-goal",
        &JevRunResult {
            stop,
            turns,
            pending,
            metrics,
        },
    )
}

struct ResolveOutcome {
    decision: JevDecision,
    evaluations: Vec<EvaluationResult>,
}

#[allow(clippy::too_many_arguments)]
async fn resolve<B: AgentBackend>(
    backend: &B,
    runtime: &JevRuntime,
    intent: &str,
    app: &str,
    root: Option<&str>,
    text: Option<&str>,
    include_values: bool,
    execute: bool,
    history: &[String],
) -> Result<ResolveOutcome, Box<DesktopResponse>> {
    let screen = observe_async(backend.clone(), app.to_owned(), root.map(str::to_owned)).await?;
    resolve_on_screen(
        backend,
        runtime,
        intent,
        &screen,
        text,
        include_values,
        execute,
        history,
    )
    .await
}

#[allow(clippy::too_many_arguments)]
async fn resolve_on_screen<B: AgentBackend>(
    backend: &B,
    runtime: &JevRuntime,
    intent: &str,
    screen: &Screen,
    text: Option<&str>,
    include_values: bool,
    execute: bool,
    history: &[String],
) -> Result<ResolveOutcome, Box<DesktopResponse>> {
    if let Some(done) = visible_completion(intent, screen) {
        return Ok(done);
    }
    let space = action_space(screen, text.is_some());
    let evaluation = runtime
        .client
        .evaluate(&policy::request(
            &runtime.configuration.model,
            intent,
            screen,
            &space,
            history,
            include_values,
        ))
        .await
        .map_err(|error| provider_error(&error))?;
    let answers = &evaluation.response.answers;
    let (operation_name, operation_confidence) = choice(answers.get("operation"))
        .ok_or_else(|| invalid_response("operation answer was absent"))?;
    let operation_name = operation_name.to_owned();
    let operation = parse_operation(&operation_name)
        .ok_or_else(|| invalid_response("operation answer was unknown"))?;
    let destructive = noul(answers.get("destructive"));
    let target_answer_name = format!("{}_target", operation_name.to_ascii_lowercase());
    let mut selected = target(&space, &operation_name, answers.get(&target_answer_name))
        .map(|(candidate, confidence)| (candidate.clone(), confidence));
    let mut evaluations = vec![evaluation];
    if let Some((reranked_target, reranked)) = rerank(RerankInput {
        runtime,
        intent,
        screen,
        space: &space,
        operation: &operation_name,
        target_answer: &target_answer_name,
        selected: selected.as_ref(),
        include_values,
        first: &evaluations[0],
    })
    .await?
    {
        if let Some(reranked_target) = reranked_target {
            selected = Some(reranked_target);
        }
        evaluations.push(reranked);
    }
    let confidence = selected
        .as_ref()
        .map_or(operation_confidence, |(_, confidence)| *confidence);
    let mut decision = gate_with_evidence(
        operation,
        confidence,
        destructive,
        exact_named_match(intent, selected.as_ref().map(|(candidate, _)| candidate))
            || positional_match(
                intent,
                selected.as_ref().map(|(candidate, _)| candidate),
                space.targets.get(&operation_name),
            ),
    );
    if space.targets.contains_key(&operation_name) && selected.is_none() {
        decision = JevDecisionKind::Abstain;
    }
    if operation == JevOperation::TypeText && text.is_none() {
        decision = JevDecisionKind::NeedsText;
    }
    let target = selected
        .as_ref()
        .map(|(candidate, _)| target_payload(candidate));
    let mut out = JevDecision {
        decision,
        operation,
        target,
        confidence,
        destructive,
        reason: reason(decision, confidence, destructive),
        executed: false,
    };
    if execute && decision == JevDecisionKind::Act {
        let response = execute_operation(
            backend.clone(),
            operation,
            selected.map(|(node, _)| node),
            text.map(str::to_owned),
        )
        .await;
        if !response.ok {
            return Err(Box::new(response));
        }
        out.executed = true;
    }
    Ok(ResolveOutcome {
        decision: out,
        evaluations,
    })
}

fn visible_completion(intent: &str, screen: &Screen) -> Option<ResolveOutcome> {
    playing_goal_satisfied(intent, screen).then(|| ResolveOutcome {
        decision: JevDecision {
            decision: JevDecisionKind::Done,
            operation: JevOperation::Done,
            target: None,
            confidence: 1.0,
            destructive: 0.0,
            reason: "the requested playback state is visibly satisfied".to_owned(),
            executed: false,
        },
        evaluations: Vec::new(),
    })
}

struct RerankInput<'a> {
    runtime: &'a JevRuntime,
    intent: &'a str,
    screen: &'a Screen,
    space: &'a policy::ActionSpace,
    operation: &'a str,
    target_answer: &'a str,
    selected: Option<&'a (Candidate, f64)>,
    include_values: bool,
    first: &'a EvaluationResult,
}

async fn rerank(
    input: RerankInput<'_>,
) -> Result<Option<(Option<(Candidate, f64)>, EvaluationResult)>, Box<DesktopResponse>> {
    if !input
        .selected
        .is_some_and(|(_, confidence)| *confidence < policy::ACT)
    {
        return Ok(None);
    }
    let candidates = shortlist(
        input.space,
        input.operation,
        input.first.response.answers.get(input.target_answer),
    );
    if candidates.len() <= 1 {
        return Ok(None);
    }
    let evaluation = input
        .runtime
        .client
        .evaluate(&policy::rerank_request(
            &input.runtime.configuration.model,
            input.intent,
            input.screen,
            input.operation,
            &candidates,
            input.include_values,
        ))
        .await
        .map_err(|error| provider_error(&error))?;
    let selected =
        choice(evaluation.response.answers.get("target")).and_then(|(choice, confidence)| {
            candidates
                .get(choice)
                .cloned()
                .map(|candidate| (candidate, confidence))
        });
    Ok(Some((selected, evaluation)))
}

trait AgentBackend: Clone + Send + 'static {
    fn observe(&self, app: &str, root: Option<&str>) -> Result<Screen, Box<DesktopResponse>>;
    fn execute(
        &self,
        operation: JevOperation,
        target: Option<Candidate>,
        text: Option<String>,
    ) -> DesktopResponse;
}

impl AgentBackend for Desktop {
    fn observe(&self, app: &str, root: Option<&str>) -> Result<Screen, Box<DesktopResponse>> {
        observe(self, app, root)
    }

    fn execute(
        &self,
        operation: JevOperation,
        target: Option<Candidate>,
        text: Option<String>,
    ) -> DesktopResponse {
        execute_desktop(self, operation, target.as_ref(), text)
    }
}

async fn observe_async<B: AgentBackend>(
    backend: B,
    app: String,
    root: Option<String>,
) -> Result<Screen, Box<DesktopResponse>> {
    tokio::task::spawn_blocking(move || backend.observe(&app, root.as_deref()))
        .await
        .map_err(|error| {
            Box::new(internal_error(&format!(
                "desktop observation task failed: {error}"
            )))
        })?
}

async fn execute_operation<B: AgentBackend>(
    backend: B,
    operation: JevOperation,
    target: Option<Candidate>,
    text: Option<String>,
) -> DesktopResponse {
    tokio::task::spawn_blocking(move || backend.execute(operation, target, text))
        .await
        .unwrap_or_else(|error| internal_error(&format!("desktop action task failed: {error}")))
}

fn execute_desktop(
    desktop: &Desktop,
    operation: JevOperation,
    target: Option<&Candidate>,
    text: Option<String>,
) -> DesktopResponse {
    let ref_id = target.map(|node| node.ref_id.clone());
    match operation {
        JevOperation::Click => desktop.click(RefRequest::new(ref_id.unwrap_or_default())),
        JevOperation::TypeText => desktop.set_value(SetValueRequest {
            ref_id: ref_id.unwrap_or_default(),
            value: text.unwrap_or_default(),
            ..SetValueRequest::default()
        }),
        JevOperation::Check => desktop.check(RefRequest::new(ref_id.unwrap_or_default())),
        JevOperation::Uncheck => desktop.uncheck(RefRequest::new(ref_id.unwrap_or_default())),
        JevOperation::Expand => desktop.expand(RefRequest::new(ref_id.unwrap_or_default())),
        JevOperation::Collapse => desktop.collapse(RefRequest::new(ref_id.unwrap_or_default())),
        JevOperation::Scroll => desktop.scroll(ScrollRequest::new(
            ref_id.unwrap_or_default(),
            tinydesktop_bus::Direction::Down,
            3,
        )),
        JevOperation::Wait => desktop.wait(WaitRequest::sleep(500)),
        JevOperation::Drill | JevOperation::Widen => {
            DesktopResponse::ok("look", json!({"root": ref_id}))
        }
        JevOperation::Done | JevOperation::Blocked => {
            DesktopResponse::ok("resolve-intent", json!({}))
        }
    }
}

fn target_payload(candidate: &Candidate) -> JevTarget {
    JevTarget {
        ref_id: candidate.ref_id.clone(),
        role: candidate.role.clone(),
        name: candidate
            .name
            .clone()
            .or_else(|| candidate.description.clone()),
    }
}

fn reason(decision: JevDecisionKind, confidence: f64, destructive: f64) -> String {
    match decision {
        JevDecisionKind::Act => "the target cleared the safe-action threshold".to_owned(),
        JevDecisionKind::ConfirmationRequired => {
            format!("the action is hard to undo ({destructive:.2})")
        }
        JevDecisionKind::Abstain => format!("target confidence {confidence:.2} is too low"),
        JevDecisionKind::NeedsText => {
            "the selected operation needs caller-supplied text".to_owned()
        }
        JevDecisionKind::Done => "the goal is visibly satisfied".to_owned(),
        JevDecisionKind::Blocked => "no offered operation can advance the goal".to_owned(),
    }
}

fn merge_metrics(metrics: &mut JevMetrics, evaluation: &EvaluationResult) {
    metrics.calls = metrics.calls.saturating_add(1);
    metrics.attempts = metrics.attempts.saturating_add(evaluation.attempts);
    metrics.latency_ms = metrics.latency_ms.saturating_add(
        evaluation
            .latency
            .as_millis()
            .try_into()
            .unwrap_or(u64::MAX),
    );
    metrics.input_tokens = metrics
        .input_tokens
        .saturating_add(evaluation.response.usage.input_tokens.unwrap_or_default());
    metrics.output_tokens = metrics
        .output_tokens
        .saturating_add(evaluation.response.usage.output_tokens.unwrap_or_default());
    metrics.model = Some(evaluation.response.model.clone());
}

fn response<T: serde::Serialize>(command: &str, value: &T) -> DesktopResponse {
    match serde_json::to_value(value) {
        Ok(data) => DesktopResponse::ok(command, data),
        Err(error) => internal_error(&format!("cannot encode Jev result: {error}")),
    }
}

fn config_error(error: &JevError) -> Box<DesktopError> {
    Box::new(DesktopError::new("JEV_INVALID_CONFIG", error.to_string()))
}

fn provider_error(error: &tinyjevclient::EvaluationFailure) -> Box<DesktopResponse> {
    let code = match &error.error {
        JevError::Authentication => "JEV_AUTHENTICATION",
        JevError::RateLimited => "JEV_RATE_LIMITED",
        JevError::Timeout => "JEV_TIMEOUT",
        JevError::InvalidResponse { .. } | JevError::Decode { .. } => "JEV_INVALID_RESPONSE",
        _ => "JEV_PROVIDER_FAILED",
    };
    Box::new(DesktopResponse::err(
        "jev-evaluate",
        DesktopError::new(code, error.to_string()),
    ))
}

fn invalid_response(message: &str) -> Box<DesktopResponse> {
    Box::new(DesktopResponse::err(
        "jev-evaluate",
        DesktopError::new("JEV_INVALID_RESPONSE", message),
    ))
}

fn internal_error(message: &str) -> DesktopResponse {
    DesktopResponse::err("jev-desktop", DesktopError::new("INTERNAL", message))
}
