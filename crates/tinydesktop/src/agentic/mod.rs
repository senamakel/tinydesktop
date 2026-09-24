//! Native Jev-backed observation, intent resolution, and goal execution.

mod policy;
mod screen;

#[cfg(test)]
mod test;

use std::fmt::Write as _;
use std::{
    collections::HashMap,
    future::Future,
    pin::Pin,
    sync::{Arc, Mutex},
    time::{Duration, Instant},
};

use serde_json::json;
use tinydesktop_bus::{
    DesktopError, DesktopResponse, GoalContinuation, JevConfig, JevConfiguration, JevDecision,
    JevDecisionKind, JevMetrics, JevOperation, JevProvider, JevRunResult, JevStopReason, JevTarget,
    JevTurn, RefRequest, ResolveIntentRequest, RunGoalRequest, ScrollRequest, SetValueRequest,
    WaitRequest,
};
use tinyjevclient::{
    Client, ClientConfig, Error as JevError, EvaluationFailure, EvaluationRequest, EvaluationResult,
};

use crate::Desktop;
use policy::{
    action_space, choice, deterministic_destructive, exact_named_match, gate_with_evidence, noul,
    parse_operation, playing_goal_satisfied, positional_match, shortlist, target,
};
use screen::{Candidate, Screen, fingerprint, observe};

/// Configured Jev transport and non-secret policy metadata.
#[derive(Clone)]
pub(crate) struct JevRuntime {
    client: Arc<dyn Evaluator>,
    configuration: JevConfiguration,
    pending: Arc<Mutex<HashMap<String, PendingRun>>>,
}

#[derive(Debug)]
struct PendingRun {
    created: Instant,
    request: RunGoalRequest,
    decision: JevDecision,
    screen: Screen,
    target: Candidate,
    turns: Vec<JevTurn>,
    metrics: JevMetrics,
}

impl std::fmt::Debug for JevRuntime {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("JevRuntime")
            .field("client", &"[configured]")
            .field("configuration", &self.configuration)
            .field("pending", &"[redacted]")
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
            if !trusted_endpoint(request.provider, endpoint) {
                return Err(Box::new(DesktopError::new(
                    "JEV_INVALID_CONFIG",
                    "endpoint is not an approved Jev provider route",
                )));
            }
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
            client: Arc::new(client),
            configuration: JevConfiguration {
                provider: request.provider,
                model: request
                    .model
                    .clone()
                    .unwrap_or_else(|| "jev-latest".to_owned()),
                endpoint_url: request.endpoint_url.clone(),
            },
            pending: Arc::new(Mutex::new(HashMap::new())),
        })
    }
}

trait Evaluator: Send + Sync {
    fn evaluate<'a>(
        &'a self,
        request: &'a EvaluationRequest,
    ) -> Pin<
        Box<
            dyn Future<Output = std::result::Result<EvaluationResult, EvaluationFailure>>
                + Send
                + 'a,
        >,
    >;
}

impl Evaluator for Client {
    fn evaluate<'a>(
        &'a self,
        request: &'a EvaluationRequest,
    ) -> Pin<
        Box<
            dyn Future<Output = std::result::Result<EvaluationResult, EvaluationFailure>>
                + Send
                + 'a,
        >,
    > {
        Box::pin(Client::evaluate(self, request))
    }
}

fn trusted_endpoint(provider: JevProvider, endpoint: &str) -> bool {
    let approved = match provider {
        JevProvider::TypeSafe => "https://api.typesafe.ai/v1/systemone",
        JevProvider::OpenRouter => "https://openrouter.ai/api/alpha/decisions",
        JevProvider::TinyHumansOpenRouter => {
            "https://api.tinyhumans.ai/agent-integrations/openrouter/systemone"
        }
    };
    if endpoint == approved {
        return true;
    }
    #[cfg(test)]
    return endpoint.starts_with("http://127.0.0.1:");
    #[cfg(not(test))]
    false
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
        true,
    )
    .await;
    match result {
        Ok(outcome) => outcome
            .action_failure
            .unwrap_or_else(|| response("resolve-intent", &outcome.decision)),
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
    if let Some(continuation) = request.continuation.clone() {
        return continue_goal(backend, runtime, continuation).await;
    }
    run_goal_fresh(backend, runtime, request).await
}

async fn run_goal_fresh<B: AgentBackend>(
    backend: B,
    runtime: JevRuntime,
    request: RunGoalRequest,
) -> DesktopResponse {
    let max_steps = request.max_steps.clamp(1, 40);
    let max_calls = request.max_model_calls.clamp(1, 80);
    let mut texts = request.text.clone().into_iter();
    let mut next_text = texts.next();
    let mut root = request.root.clone();
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
            metrics.calls.saturating_add(1) < max_calls,
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
        if let Some(_failure) = outcome.action_failure {
            return action_failed_response(turns, decision, metrics);
        }
        let stop = stop_reason(decision.decision);
        if stop == Some(JevStopReason::ConfirmationRequired) {
            let Some(target) = selected_target(&before, &decision) else {
                return run_response(JevStopReason::StaleTarget, turns, Some(decision), metrics);
            };
            let remaining_text = next_text.into_iter().chain(texts).collect();
            let pending_run = PendingRun {
                created: Instant::now(),
                request: RunGoalRequest {
                    text: remaining_text,
                    root,
                    max_steps: max_steps
                        .saturating_sub(u32::try_from(turns.len()).unwrap_or(u32::MAX)),
                    max_model_calls: max_calls.saturating_sub(metrics.calls),
                    ..request.clone()
                },
                decision: decision.clone(),
                screen: before,
                target,
                turns: turns.clone(),
                metrics: metrics.clone(),
            };
            return queue_confirmation(&runtime, pending_run);
        }
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
        let Ok(after) = observe_async(backend.clone(), request.app.clone(), root.clone()).await
        else {
            return action_failed_response(turns, decision, metrics);
        };
        let changed = fingerprint(&after) != before_fingerprint
            || matches!(
                decision.operation,
                JevOperation::Drill | JevOperation::Widen
            );
        unchanged = if changed {
            0
        } else {
            unchanged.saturating_add(1)
        };
        record_turn(&mut turns, &mut history, &decision, changed);
        if unchanged >= 3 {
            return run_response(JevStopReason::Stalled, turns, None, metrics);
        }
    }
}

fn selected_target(screen: &Screen, decision: &JevDecision) -> Option<Candidate> {
    decision
        .target
        .as_ref()
        .and_then(|target| {
            screen
                .candidates
                .iter()
                .find(|candidate| candidate.ref_id == target.ref_id)
        })
        .cloned()
}

fn queue_confirmation(runtime: &JevRuntime, run: PendingRun) -> DesktopResponse {
    let mut token = [0_u8; 16];
    if getrandom::fill(&mut token).is_err() {
        return internal_error("cannot create a confirmation handle");
    }
    let id = token
        .iter()
        .fold(String::with_capacity(32), |mut id, byte| {
            let _ = write!(id, "{byte:02x}");
            id
        });
    let Ok(mut pending) = runtime.pending.lock() else {
        return internal_error("confirmation state is unavailable");
    };
    pending.retain(|_, previous| previous.created.elapsed() < Duration::from_secs(600));
    if pending.len() >= 32 {
        return DesktopResponse::err(
            "run-goal",
            DesktopError::new(
                "CONFIRMATION_LIMIT",
                "too many desktop actions await confirmation",
            ),
        );
    }
    let response = run_response_with_id(
        JevStopReason::ConfirmationRequired,
        run.turns.clone(),
        Some(run.decision.clone()),
        run.metrics.clone(),
        Some(id.clone()),
    );
    pending.insert(id, run);
    response
}

fn record_turn(
    turns: &mut Vec<JevTurn>,
    history: &mut Vec<String>,
    decision: &JevDecision,
    changed: bool,
) {
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
}

async fn continue_goal<B: AgentBackend>(
    backend: B,
    runtime: JevRuntime,
    continuation: GoalContinuation,
) -> DesktopResponse {
    let pending = match runtime.pending.lock() {
        Ok(mut pending) => pending.remove(&continuation.id),
        Err(_) => return internal_error("confirmation state is unavailable"),
    };
    let Some(pending) = pending else {
        return DesktopResponse::err(
            "run-goal",
            DesktopError::new(
                "CONFIRMATION_EXPIRED",
                "confirmation handle is absent or already consumed",
            ),
        );
    };
    if pending.created.elapsed() >= Duration::from_secs(600) {
        return DesktopResponse::err(
            "run-goal",
            DesktopError::new("CONFIRMATION_EXPIRED", "confirmation handle expired"),
        );
    }
    if !continuation.approve {
        return run_response(
            JevStopReason::Cancelled,
            pending.turns,
            Some(pending.decision),
            pending.metrics,
        );
    }
    let fresh = match observe_async(
        backend.clone(),
        pending.request.app.clone(),
        pending.request.root.clone(),
    )
    .await
    {
        Ok(screen) => screen,
        Err(error) => return *error,
    };
    let Some(target) = current_target(&pending, &fresh) else {
        return run_response(
            JevStopReason::StaleTarget,
            pending.turns,
            Some(pending.decision),
            pending.metrics,
        );
    };
    let text = (pending.decision.operation == JevOperation::TypeText)
        .then(|| pending.request.text.first().cloned())
        .flatten();
    let reply = execute_operation(
        backend.clone(),
        pending.decision.operation,
        Some(target),
        text,
    )
    .await;
    if !reply.ok {
        return action_failed_response(pending.turns, pending.decision, pending.metrics);
    }
    let mut turns = pending.turns;
    turns.push(JevTurn {
        step: u32::try_from(turns.len())
            .unwrap_or(u32::MAX)
            .saturating_add(1),
        operation: pending.decision.operation,
        target: pending.decision.target.clone(),
        confidence: pending.decision.confidence,
        ok: true,
        changed: false,
    });
    let mut request = pending.request;
    request.continuation = None;
    request.max_steps = request.max_steps.saturating_sub(1);
    if pending.decision.operation == JevOperation::TypeText && !request.text.is_empty() {
        request.text.remove(0);
    }
    if request.max_steps == 0 {
        return run_response(JevStopReason::ActionBudget, turns, None, pending.metrics);
    }
    if request.max_model_calls == 0 {
        return run_response(JevStopReason::ModelBudget, turns, None, pending.metrics);
    }
    merge_continuation(
        run_goal_fresh(backend, runtime, request).await,
        turns,
        &pending.metrics,
    )
}

fn current_target(pending: &PendingRun, fresh: &Screen) -> Option<Candidate> {
    let mut matching = fresh.candidates.iter().filter(|candidate| {
        same_target(
            &pending.screen,
            fresh,
            &pending.target,
            candidate,
            pending.decision.operation,
        )
    });
    let target = matching.next()?;
    matching.next().is_none().then(|| target.clone())
}

fn merge_continuation(
    mut result: DesktopResponse,
    mut turns: Vec<JevTurn>,
    metrics: &JevMetrics,
) -> DesktopResponse {
    if let Some(data) = result.data.take() {
        if let Ok(mut continuation_result) = serde_json::from_value::<JevRunResult>(data.clone()) {
            turns.append(&mut continuation_result.turns);
            for (index, turn) in turns.iter_mut().enumerate() {
                turn.step = u32::try_from(index).unwrap_or(u32::MAX).saturating_add(1);
            }
            continuation_result.turns = turns;
            continuation_result.metrics.calls = continuation_result
                .metrics
                .calls
                .saturating_add(metrics.calls);
            continuation_result.metrics.attempts = continuation_result
                .metrics
                .attempts
                .saturating_add(metrics.attempts);
            continuation_result.metrics.latency_ms = continuation_result
                .metrics
                .latency_ms
                .saturating_add(metrics.latency_ms);
            continuation_result.metrics.input_tokens = continuation_result
                .metrics
                .input_tokens
                .saturating_add(metrics.input_tokens);
            continuation_result.metrics.output_tokens = continuation_result
                .metrics
                .output_tokens
                .saturating_add(metrics.output_tokens);
            result.data = serde_json::to_value(continuation_result).ok();
        } else {
            result.data = Some(data);
        }
    }
    result
}

fn same_target(
    before: &Screen,
    after: &Screen,
    old: &Candidate,
    current: &Candidate,
    operation: JevOperation,
) -> bool {
    let action = match operation {
        JevOperation::Click => "Click",
        JevOperation::TypeText => "SetValue",
        JevOperation::Check | JevOperation::Uncheck => "Toggle",
        JevOperation::Expand => "Expand",
        JevOperation::Collapse => "Collapse",
        JevOperation::Scroll => "Scroll",
        _ => return false,
    };
    before.app == after.app
        && before.window == after.window
        && before.surface == after.surface
        && old.role == current.role
        && old.name == current.name
        && old.description == current.description
        && old.path == current.path
        && old.bounds == current.bounds
        && old.states == current.states
        && (old.name.is_some() || old.description.is_some() || old.bounds.is_some())
        && current.available_actions.iter().any(|available| {
            available == action || (operation == JevOperation::TypeText && available == "TypeText")
        })
}

fn stop_reason(decision: JevDecisionKind) -> Option<JevStopReason> {
    match decision {
        JevDecisionKind::Done => Some(JevStopReason::Done),
        JevDecisionKind::Blocked => Some(JevStopReason::Blocked),
        JevDecisionKind::ConfirmationRequired => Some(JevStopReason::ConfirmationRequired),
        JevDecisionKind::Abstain => Some(JevStopReason::LowConfidence),
        JevDecisionKind::NeedsText => Some(JevStopReason::NeedsText),
        JevDecisionKind::Act => None,
    }
}

fn action_failed_response(
    mut turns: Vec<JevTurn>,
    decision: JevDecision,
    metrics: JevMetrics,
) -> DesktopResponse {
    turns.push(failed_turn(&turns, &decision));
    run_response(JevStopReason::ActionFailed, turns, Some(decision), metrics)
}

fn failed_turn(turns: &[JevTurn], decision: &JevDecision) -> JevTurn {
    JevTurn {
        step: u32::try_from(turns.len())
            .unwrap_or(u32::MAX)
            .saturating_add(1),
        operation: decision.operation,
        target: decision.target.clone(),
        confidence: decision.confidence,
        ok: false,
        changed: false,
    }
}

fn run_response(
    stop: JevStopReason,
    turns: Vec<JevTurn>,
    pending: Option<JevDecision>,
    metrics: JevMetrics,
) -> DesktopResponse {
    run_response_with_id(stop, turns, pending, metrics, None)
}

fn run_response_with_id(
    stop: JevStopReason,
    turns: Vec<JevTurn>,
    pending: Option<JevDecision>,
    metrics: JevMetrics,
    confirmation_id: Option<String>,
) -> DesktopResponse {
    response(
        "run-goal",
        &JevRunResult {
            stop,
            turns,
            pending,
            confirmation_id,
            metrics,
        },
    )
}

struct ResolveOutcome {
    decision: JevDecision,
    evaluations: Vec<EvaluationResult>,
    action_failure: Option<DesktopResponse>,
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
    allow_rerank: bool,
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
        allow_rerank,
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
    allow_rerank: bool,
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
    let mut destructive = noul(answers.get("destructive"));
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
        allow: allow_rerank,
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
    destructive = destructive.max(local_destructive_score(
        intent,
        operation,
        selected.as_ref(),
    ));
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
    let (executed, action_failure) = execute_if_requested(
        backend,
        execute && decision == JevDecisionKind::Act,
        operation,
        selected.map(|(node, _)| node),
        text,
    )
    .await;
    out.executed = executed;
    Ok(ResolveOutcome {
        decision: out,
        evaluations,
        action_failure,
    })
}

fn local_destructive_score(
    intent: &str,
    operation: JevOperation,
    selected: Option<&(Candidate, f64)>,
) -> f64 {
    if deterministic_destructive(intent, operation, selected.map(|(candidate, _)| candidate)) {
        1.0
    } else {
        0.0
    }
}

async fn execute_if_requested<B: AgentBackend>(
    backend: &B,
    execute: bool,
    operation: JevOperation,
    target: Option<Candidate>,
    text: Option<&str>,
) -> (bool, Option<DesktopResponse>) {
    if !execute {
        return (false, None);
    }
    let response =
        execute_operation(backend.clone(), operation, target, text.map(str::to_owned)).await;
    if response.ok {
        (true, None)
    } else {
        (false, Some(response))
    }
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
        action_failure: None,
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
    allow: bool,
}

async fn rerank(
    input: RerankInput<'_>,
) -> Result<Option<(Option<(Candidate, f64)>, EvaluationResult)>, Box<DesktopResponse>> {
    if !input.allow
        || !input
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
