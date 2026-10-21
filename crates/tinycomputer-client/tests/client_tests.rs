//! In-memory `TinyBus` tests for the public `TinyComputer` client.

use std::{
    future::Future,
    pin::Pin,
    sync::{Arc, Mutex as StdMutex, OnceLock},
    time::Duration,
};

use async_trait::async_trait;
use serde_json::{Value, json};
use tinybus::module::ReconfigurableModule;
use tinybus::{
    Connection, Error as TinyBusError, Interface, InterfaceName, MemberName, ObjectPath, Proxy,
    broker::Broker,
    module::{
        AccessFuture, FailureReporter, FailureSink, ModuleAccess, ModuleHost, ProxyFailure,
        ReconfigureFailure, ReleaseRecord,
    },
    transport::memory::MemoryBus,
};
use tinycomputer_bus::JevConfig;
use tinycomputer_bus::{
    DesktopResponse,
    accessibility::{FocusQuery, FocusedTextContext},
    agent::{AgentResponse, Capabilities, names as task_names},
    browser::{
        ClaimProfileRequest, ClaimProfileResponse, NavigateRequest, OutputChunk, OutputId,
        PageState, ProfileClaim, ReadOutputRequest, SessionId, SessionRequest,
    },
    catalogue::{self, Family},
    config::ComputerConfig,
    names,
};
use tinycomputer_client::{ClientError, ComputerClient, RUN_GOAL_TIMEOUT, TASK_CALL_TIMEOUT};

const TEST_MODULE_ID: &str = env!("CARGO_PKG_NAME");

fn static_contract_value<T, E: std::fmt::Debug>(result: Result<T, E>) -> T {
    match result {
        Ok(value) => value,
        Err(error) => {
            eprintln!("client test failed: {error:?}");
            std::process::exit(1);
        }
    }
}

static TEST_RECORD: ReleaseRecord = ReleaseRecord {
    id: TEST_MODULE_ID,
    bus_name: names::INTERFACE,
    object_path: names::OBJECT_PATH,
    version: "0.16.0",
    release_url: "https://example.invalid/test",
    assets: &[],
};

#[derive(Default)]
struct SilentSink;

impl FailureSink for SilentSink {
    fn capture(&self, _report: &tinybus::module::failure::FailureReport<'_>) {}
}

struct TestAccess {
    bus: MemoryBus,
    host: Option<Arc<ModuleHost>>,
    reporter: FailureReporter,
}

impl TestAccess {
    fn new(bus: MemoryBus, host: Option<Arc<ModuleHost>>) -> Self {
        Self {
            bus,
            host,
            reporter: FailureReporter::new(Arc::new(SilentSink)),
        }
    }
}

impl ModuleAccess for TestAccess {
    fn find(&self, id: &str) -> Option<&'static ReleaseRecord> {
        (id == TEST_MODULE_ID).then_some(&TEST_RECORD)
    }

    fn enabled(&self) -> bool {
        true
    }

    fn reporter(&self) -> &FailureReporter {
        &self.reporter
    }

    fn ensure_loaded<'a>(&'a self, _id: &'a str) -> AccessFuture<'a, Result<(), String>> {
        Box::pin(async move {
            if let Some(host) = &self.host {
                host.ensure_ready(names::INTERFACE)
                    .await
                    .map_err(|error| error.to_string())?;
            }
            Ok(())
        })
    }

    fn proxy<'a>(
        &'a self,
        bus_name: &'a str,
        object_path: &'a str,
    ) -> AccessFuture<'a, Result<Proxy, ProxyFailure>> {
        let bus = self.bus.clone();
        let bus_name = bus_name.to_owned();
        let object_path = object_path.to_owned();
        Box::pin(async move {
            let transport = bus.connect().await.map_err(|_| ProxyFailure::Transport)?;
            let connection = Connection::connect(transport)
                .await
                .map_err(|_| ProxyFailure::Transport)?;
            connection
                .proxy(&bus_name, &object_path, &bus_name)
                .map_err(|_| ProxyFailure::Contract)
        })
    }

    fn reinitialize<'a>(
        &'a self,
        id: &'a str,
        config: Value,
    ) -> AccessFuture<'a, Result<(), ReconfigureFailure>> {
        Box::pin(async move {
            let Some(host) = &self.host else {
                return Ok(());
            };
            host.reinitialize(id, config)
                .await
                .map(|_| ())
                .map_err(|_| ReconfigureFailure::Failed)
        })
    }
}

fn client_for(bus: &MemoryBus, host: Option<Arc<ModuleHost>>) -> ComputerClient {
    ComputerClient::new(ReconfigurableModule::new(
        TestAccess::new(bus.clone(), host),
        TEST_MODULE_ID,
    ))
}

fn test_capabilities(config: &ComputerConfig) -> Capabilities {
    Capabilities {
        contract_version: tinycomputer_bus::CONTRACT_VERSION,
        surfaces: Vec::new(),
        jev_configured: config.jev.is_some(),
        planner_configured: config.planner.is_some(),
        rescue_configured: false,
        output_configured: false,
        decision_model: None,
        planner_model: None,
        rescue_model: None,
        output_model: None,
        step_kinds: Vec::new(),
        guide: String::new(),
        members: Vec::new(),
        examples: Vec::new(),
        catalogue: catalogue::summaries(),
    }
}

const FLOW_MEMBERS: &[&str] = &[
    names::methods::RESOLVE_INTENT,
    names::methods::RUN_GOAL,
    names::methods::RUN_FLOW,
    names::methods::VALIDATE_FLOW,
    names::methods::FLOW_GUIDE,
];

fn expected_family(member: &str) -> Family {
    if task_names::METHODS.contains(&member) {
        Family::Task
    } else if tinycomputer_bus::browser::names::METHODS.contains(&member) {
        Family::Browser
    } else if FLOW_MEMBERS.contains(&member) {
        Family::Flow
    } else {
        Family::Desktop
    }
}

fn catalogue_request(member: &str) -> Result<Value, serde_json::Error> {
    Ok(match member {
        tinycomputer_bus::browser::names::methods::NAVIGATE => {
            serde_json::to_value(SessionRequest::new(
                SessionId::new("catalogue-session"),
                NavigateRequest::new("https://example.com"),
            ))?
        }
        tinycomputer_bus::browser::names::methods::CLAIM_PROFILE => {
            serde_json::to_value(ClaimProfileRequest::new("/profile/catalogue", 0))?
        }
        tinycomputer_bus::browser::names::methods::READ_OUTPUT => {
            serde_json::to_value(ReadOutputRequest::new(OutputId::new("catalogue-output")))?
        }
        _ => json!({"member": member}),
    })
}

fn assert_catalogue_reply(member: &str, value: &Value) {
    match member {
        tinycomputer_bus::browser::names::methods::NAVIGATE => {
            assert_eq!(value["url"], "https://example.com");
        }
        tinycomputer_bus::browser::names::methods::CLAIM_PROFILE => {
            assert_eq!(value["outcome"], "claimed");
            assert_eq!(value["path"], "/profile/fixture");
        }
        tinycomputer_bus::browser::names::methods::READ_OUTPUT => {
            assert_eq!(value["id"], "catalogue-output");
            assert_eq!(value["offset"], 0);
            assert_eq!(value["data"], "aGVsbG8=");
            assert_eq!(value["eof"], true);
        }
        _ => assert_eq!(*value, json!(member), "member {member} reply"),
    }
}

fn response(member: &str, request: &Value, config: &ComputerConfig) -> tinybus::Result<Value> {
    let malformed_version = request
        .get("malformed_version")
        .and_then(Value::as_bool)
        .unwrap_or(false);
    let wrong_data = request
        .get("wrong_data")
        .and_then(Value::as_bool)
        .unwrap_or(false);

    if member == task_names::methods::DESCRIBE {
        return serde_json::to_value(test_capabilities(config))
            .map_err(|error| TinyBusError::failed(error.to_string()));
    }

    let family = expected_family(member);
    if family == Family::Task {
        if malformed_version {
            return Ok(json!({
                "version": "unsupported",
                "ok": true,
                "data": "unexpected",
                "error": null
            }));
        }
        let data = if wrong_data { json!(17) } else { json!(member) };
        return serde_json::to_value(AgentResponse::ok(data))
            .map_err(|error| TinyBusError::failed(error.to_string()));
    }

    if malformed_version {
        return Ok(json!({
            "version": "unsupported",
            "ok": true,
            "command": member,
            "data": "unexpected",
            "error": null
        }));
    }

    let data = if wrong_data {
        json!(17)
    } else {
        match member {
            "AccessibilityFocus" => serde_json::to_value(FocusedTextContext {
                app_name: Some("fixture".to_owned()),
                role: Some("text".to_owned()),
                text: "typed response".to_owned(),
                selected_text: None,
                raw_error: None,
                bounds: None,
            })
            .map_err(|error| TinyBusError::failed(error.to_string()))?,
            "BrowserClaimProfile" => serde_json::to_value(ClaimProfileResponse {
                outcome: ProfileClaim::Claimed,
                path: Some("/profile/fixture".to_owned()),
            })
            .map_err(|error| TinyBusError::failed(error.to_string()))?,
            "BrowserNavigate" => serde_json::to_value(PageState::new("https://example.com"))
                .map_err(|error| TinyBusError::failed(error.to_string()))?,
            "BrowserReadOutput" => {
                let read: ReadOutputRequest = serde_json::from_value(request.clone())
                    .map_err(|error| TinyBusError::failed(error.to_string()))?;
                serde_json::to_value(OutputChunk {
                    id: read.output,
                    offset: read.offset,
                    data: "aGVsbG8=".to_owned(),
                    eof: true,
                })
                .map_err(|error| TinyBusError::failed(error.to_string()))?
            }
            _ => json!(member),
        }
    };
    serde_json::to_value(DesktopResponse::ok(member, data))
        .map_err(|error| TinyBusError::failed(error.to_string()))
}

struct EchoService {
    config: Arc<tokio::sync::RwLock<ComputerConfig>>,
    calls: Arc<std::sync::atomic::AtomicUsize>,
    seen: Arc<StdMutex<Vec<ObservedCall>>>,
}

#[async_trait]
impl Interface for EchoService {
    fn name(&self) -> InterfaceName {
        static_contract_value(InterfaceName::new(names::INTERFACE))
    }

    fn members(&self) -> Vec<MemberName> {
        names::METHODS
            .iter()
            .map(|member| static_contract_value(MemberName::new(*member)))
            .collect()
    }

    fn requires_confidential(&self, member: &MemberName) -> bool {
        handler_requires_confidential(member.as_str())
    }

    async fn call(&self, member: &MemberName, args: Value) -> tinybus::Result<Value> {
        self.invoke(member, args).await
    }

    async fn call_with_context(
        &self,
        member: &MemberName,
        args: Value,
        context: &tinybus::service::CallContext,
    ) -> tinybus::Result<Value> {
        self.seen
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .push(ObservedCall {
                member: member.as_str().to_owned(),
                confidential: context.is_confidential(),
                args: args.clone(),
            });
        self.invoke(member, args).await
    }
}

impl EchoService {
    async fn invoke(&self, member: &MemberName, args: Value) -> tinybus::Result<Value> {
        self.calls.fetch_add(1, std::sync::atomic::Ordering::AcqRel);
        let request = args
            .as_array()
            .and_then(|items| items.first())
            .cloned()
            .unwrap_or(Value::Null);
        let config = self.config.read().await.clone();
        response(member.as_str(), &request, &config)
    }
}

#[derive(Debug, Clone)]
struct ObservedCall {
    member: String,
    confidential: bool,
    args: Value,
}

fn handler_requires_confidential(member: &str) -> bool {
    task_names::CONFIDENTIAL.contains(&member)
        || [
            names::methods::RESOLVE_INTENT,
            names::methods::RUN_GOAL,
            names::methods::RUN_FLOW,
            names::accessibility::ACCESSIBILITY_FOCUS,
            names::accessibility::ACCESSIBILITY_VALIDATE_TARGET,
            names::accessibility::ACCESSIBILITY_PASTE,
        ]
        .contains(&member)
}

struct UntrustedFixture {
    bus: MemoryBus,
    _broker_task: tokio::task::JoinHandle<tinybus::Result<()>>,
    _service_connection: Connection,
    calls: Arc<std::sync::atomic::AtomicUsize>,
    seen: Arc<StdMutex<Vec<ObservedCall>>>,
}

async fn untrusted_fixture() -> Result<UntrustedFixture, Box<dyn std::error::Error>> {
    let bus = MemoryBus::new();
    let broker_task = Broker::new().spawn(bus.clone());
    let transport = bus.connect().await?;
    let service_connection = Connection::connect(transport).await?;
    service_connection.request_name(names::INTERFACE).await?;
    let calls = Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let seen = Arc::new(StdMutex::new(Vec::new()));
    service_connection
        .serve_at(
            ObjectPath::new(names::OBJECT_PATH)?,
            EchoService {
                config: Arc::new(tokio::sync::RwLock::new(ComputerConfig::default())),
                calls: Arc::clone(&calls),
                seen: Arc::clone(&seen),
            },
        )
        .await?;
    Ok(UntrustedFixture {
        bus,
        _broker_task: broker_task,
        _service_connection: service_connection,
        calls,
        seen,
    })
}

#[tokio::test]
async fn every_catalogued_operation_is_routed_by_family_and_confidentiality()
-> Result<(), Box<dyn std::error::Error>> {
    let fixture = untrusted_fixture().await?;
    let client = client_for(&fixture.bus, None);
    let config = ComputerConfig::default();
    client.record_loaded_with(&config)?;
    let mut called = 0;
    let mut refused_confidential = 0;

    for entry in catalogue::MEMBERS {
        assert_eq!(
            entry.family,
            expected_family(entry.name),
            "family for {}",
            entry.name
        );
        assert_eq!(
            entry.confidential,
            handler_requires_confidential(entry.name),
            "confidential flag for {}",
            entry.name
        );
        let request = catalogue_request(entry.name)?;
        let result = if entry.name == task_names::methods::DESCRIBE {
            let described = client.describe(&config).await?;
            assert_eq!(
                described.contract_version,
                tinycomputer_bus::CONTRACT_VERSION
            );
            called += 1;
            continue;
        } else {
            match entry.family {
                Family::Task => {
                    client
                        .call_task::<_, Value>(&config, entry.name, request)
                        .await
                }
                Family::Flow | Family::Desktop | Family::Browser => {
                    client
                        .call_desktop::<_, Value>(&config, entry.name, request)
                        .await
                }
            }
        };

        if entry.confidential {
            assert!(
                matches!(result, Err(ClientError::Bus(_))),
                "unattested recipient must refuse confidential member {}",
                entry.name
            );
            refused_confidential += 1;
        } else {
            let value = result?;
            assert_catalogue_reply(entry.name, &value);
            called += 1;
        }
    }

    assert!(called > 0);
    assert!(refused_confidential > 0);
    assert_eq!(
        fixture.calls.load(std::sync::atomic::Ordering::Acquire),
        called,
        "only public calls and Describe reach an untrusted service"
    );
    let seen = fixture
        .seen
        .lock()
        .map_err(|_| std::io::Error::other("untrusted call log poisoned"))?
        .clone();
    assert!(seen.iter().all(|call| !call.confidential));
    Ok(())
}

#[tokio::test]
async fn typed_accessibility_browser_profile_task_and_held_output_calls_round_trip()
-> Result<(), Box<dyn std::error::Error>> {
    let fixture = untrusted_fixture().await?;
    let client = client_for(&fixture.bus, None);
    let config = ComputerConfig::default();
    client.record_loaded_with(&config)?;

    let focus = client
        .call_desktop::<_, FocusedTextContext>(
            &config,
            names::accessibility::ACCESSIBILITY_FOCUS,
            FocusQuery::default(),
        )
        .await;
    assert!(matches!(focus, Err(ClientError::Bus(_))));

    let claimed = client
        .call_desktop::<_, ClaimProfileResponse>(
            &config,
            tinycomputer_bus::browser::names::methods::CLAIM_PROFILE,
            ClaimProfileRequest::new("/profile/fixture", 0),
        )
        .await?;
    assert_eq!(claimed.outcome, ProfileClaim::Claimed);
    assert_eq!(claimed.path.as_deref(), Some("/profile/fixture"));

    let navigated = client
        .call_desktop::<_, PageState>(
            &config,
            tinycomputer_bus::browser::names::methods::NAVIGATE,
            SessionRequest::new(
                SessionId::new("session-1"),
                NavigateRequest::new("https://example.com"),
            ),
        )
        .await?;
    assert_eq!(navigated.url, "https://example.com");

    let chunk = client
        .call_desktop::<_, OutputChunk>(
            &config,
            tinycomputer_bus::browser::names::methods::READ_OUTPUT,
            ReadOutputRequest::new(OutputId::new("held-image-1")),
        )
        .await?;
    assert_eq!(chunk.id.as_str(), "held-image-1");
    assert_eq!(chunk.offset, 0);
    assert_eq!(chunk.data, "aGVsbG8=");
    assert!(chunk.eof);

    let planned = client
        .call_task::<_, Value>(
            &config,
            task_names::methods::PLAN_TASK,
            tinycomputer_bus::agent::PlanTaskRequest::default(),
        )
        .await?;
    assert_eq!(planned, json!(task_names::methods::PLAN_TASK));

    let seen = fixture
        .seen
        .lock()
        .map_err(|_| std::io::Error::other("untrusted call log poisoned"))?
        .clone();
    for member in [
        tinycomputer_bus::browser::names::methods::CLAIM_PROFILE,
        tinycomputer_bus::browser::names::methods::NAVIGATE,
        tinycomputer_bus::browser::names::methods::READ_OUTPUT,
        task_names::methods::PLAN_TASK,
    ] {
        let observed = seen
            .iter()
            .find(|call| call.member == member)
            .ok_or_else(|| std::io::Error::other("typed member was not observed"))?;
        assert!(!observed.confidential, "{member} is public");
        assert_eq!(observed.args.as_array().map(Vec::len), Some(1));
    }
    let claim = seen
        .iter()
        .find(|call| call.member == tinycomputer_bus::browser::names::methods::CLAIM_PROFILE)
        .ok_or_else(|| std::io::Error::other("claim request was not observed"))?;
    assert_eq!(
        claim.args[0],
        json!({
            "path": "/profile/fixture",
            "wait_ms": 0,
            "fall_back_to_fresh": true
        })
    );
    let navigation = seen
        .iter()
        .find(|call| call.member == tinycomputer_bus::browser::names::methods::NAVIGATE)
        .ok_or_else(|| std::io::Error::other("navigation request was not observed"))?;
    assert_eq!(navigation.args[0]["session"], "session-1");
    assert_eq!(navigation.args[0]["url"], "https://example.com");
    let held_output = seen
        .iter()
        .find(|call| call.member == tinycomputer_bus::browser::names::methods::READ_OUTPUT)
        .ok_or_else(|| std::io::Error::other("held-output request was not observed"))?;
    assert_eq!(held_output.args[0]["output"], "held-image-1");
    assert_eq!(held_output.args[0]["offset"], 0);
    assert_eq!(
        held_output.args[0]["max_len"],
        ReadOutputRequest::DEFAULT_LEN
    );

    assert_eq!(RUN_GOAL_TIMEOUT, Duration::from_secs(330));
    assert_eq!(TASK_CALL_TIMEOUT, Duration::from_secs(75));
    Ok(())
}

#[tokio::test]
async fn unknown_members_wrong_families_and_bad_envelopes_fail_closed()
-> Result<(), Box<dyn std::error::Error>> {
    let fixture = untrusted_fixture().await?;
    let client = client_for(&fixture.bus, None);
    let config = ComputerConfig::default();
    client.record_loaded_with(&config)?;

    assert!(matches!(
        client
            .call_desktop::<_, Value>(&config, "AccessibilityFutureMember", Value::Null)
            .await,
        Err(ClientError::UnknownMember(_))
    ));
    assert!(matches!(
        client
            .call_task::<_, Value>(
                &config,
                tinycomputer_bus::browser::names::methods::NAVIGATE,
                Value::Null,
            )
            .await,
        Err(ClientError::WrongFamily { .. })
    ));
    assert!(matches!(
        client
            .call_desktop::<_, Value>(&config, task_names::methods::PLAN_TASK, Value::Null,)
            .await,
        Err(ClientError::WrongFamily { .. })
    ));
    assert!(matches!(
        client
            .call_desktop::<_, Value>(
                &config,
                names::methods::PERMISSIONS,
                json!({"malformed_version": true}),
            )
            .await,
        Err(ClientError::DesktopResponse(
            tinycomputer_bus::DesktopResponseError::Version { .. }
        ))
    ));
    assert!(matches!(
        client
            .call_desktop::<_, String>(
                &config,
                names::methods::PERMISSIONS,
                json!({"wrong_data": true}),
            )
            .await,
        Err(ClientError::DesktopResponse(
            tinycomputer_bus::DesktopResponseError::Decode(_)
        ))
    ));
    assert!(matches!(
        client
            .call_task::<_, Value>(
                &config,
                task_names::methods::PLAN_TASK,
                json!({"malformed_version": true}),
            )
            .await,
        Err(ClientError::AgentResponse(
            tinycomputer_bus::agent::AgentResponseError::Version { .. }
        ))
    ));
    Ok(())
}

#[derive(Default)]
struct TestModuleState {
    config: tokio::sync::RwLock<ComputerConfig>,
    applied: StdMutex<Vec<ComputerConfig>>,
    observed: StdMutex<Vec<ObservedCall>>,
    focus_gate: tokio::sync::Mutex<Option<FocusGate>>,
    run_goal_gate: tokio::sync::Mutex<Option<FocusGate>>,
    name_acquired: std::sync::atomic::AtomicBool,
}

struct FocusGate {
    entered: tokio::sync::oneshot::Sender<()>,
    release: tokio::sync::oneshot::Receiver<()>,
}

static LINKED_STATE: OnceLock<StdMutex<Option<Arc<TestModuleState>>>> = OnceLock::new();
static LINKED_TEST_LOCK: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());

fn linked_state_slot() -> &'static StdMutex<Option<Arc<TestModuleState>>> {
    LINKED_STATE.get_or_init(|| StdMutex::new(None))
}

fn linked_setup_factory()
-> impl Fn(Connection, ComputerConfig) -> Pin<Box<dyn Future<Output = tinybus::Result<()>> + Send>>
+ Clone
+ Send
+ Sync
+ 'static {
    let state = Arc::new(TestModuleState::default());
    *linked_state_slot()
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner) = Some(Arc::clone(&state));
    move |connection, config| {
        let state = Arc::clone(&state);
        Box::pin(async move {
            state
                .applied
                .lock()
                .map_err(|_| TinyBusError::failed("test configuration state poisoned"))?
                .push(config.clone());
            *state.config.write().await = config;
            connection
                .serve_at(
                    ObjectPath::new(names::OBJECT_PATH)?,
                    LinkedService(Arc::clone(&state)),
                )
                .await?;
            if !state
                .name_acquired
                .swap(true, std::sync::atomic::Ordering::AcqRel)
            {
                connection.request_name(names::INTERFACE).await?;
            }
            Ok(())
        })
    }
}

mod linked_service {
    fn setup_factory() -> impl Fn(
        tinybus::Connection,
        tinycomputer_bus::config::ComputerConfig,
    ) -> std::pin::Pin<
        Box<dyn std::future::Future<Output = tinybus::Result<()>> + Send>,
    > + Clone
    + Send
    + Sync {
        super::linked_setup_factory()
    }

    tinybus_module::module_export_static! {
        setup_factory = setup_factory,
        config = tinycomputer_bus::config::ComputerConfig,
        worker_threads = 1,
        provides = ["ai.tinyhumans.tinycomputer.Desktop"],
        methods = ["Describe", "AccessibilityFocus", "RunGoal"],
        signals = [],
        requires = [],
        optional = [],
        lazy = false,
    }
}

struct LinkedService(Arc<TestModuleState>);

#[async_trait]
impl Interface for LinkedService {
    fn name(&self) -> InterfaceName {
        static_contract_value(InterfaceName::new(names::INTERFACE))
    }

    fn members(&self) -> Vec<MemberName> {
        [
            task_names::methods::DESCRIBE,
            names::accessibility::ACCESSIBILITY_FOCUS,
            names::methods::RUN_GOAL,
        ]
        .iter()
        .map(|member| static_contract_value(MemberName::new(*member)))
        .collect()
    }

    fn requires_confidential(&self, member: &MemberName) -> bool {
        handler_requires_confidential(member.as_str())
    }

    async fn call(&self, member: &MemberName, args: Value) -> tinybus::Result<Value> {
        self.reply(member, &args).await
    }

    async fn call_with_context(
        &self,
        member: &MemberName,
        args: Value,
        context: &tinybus::service::CallContext,
    ) -> tinybus::Result<Value> {
        self.0
            .observed
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .push(ObservedCall {
                member: member.as_str().to_owned(),
                confidential: context.is_confidential(),
                args: args.clone(),
            });
        self.reply(member, &args).await
    }
}

impl LinkedService {
    async fn reply(&self, member: &MemberName, args: &Value) -> tinybus::Result<Value> {
        if member.as_str() == task_names::methods::DESCRIBE
            && !args.as_array().is_some_and(Vec::is_empty)
        {
            return Err(TinyBusError::failed("Describe takes no arguments"));
        }

        if member.as_str() == names::accessibility::ACCESSIBILITY_FOCUS {
            if let Some(gate) = self.0.focus_gate.lock().await.take() {
                let _ = gate.entered.send(());
                let _ = gate.release.await;
            }
            return serde_json::to_value(DesktopResponse::ok(
                member.as_str(),
                serde_json::to_value(FocusedTextContext {
                    app_name: Some("linked fixture".to_owned()),
                    role: Some("text".to_owned()),
                    text: "attested result".to_owned(),
                    selected_text: None,
                    raw_error: None,
                    bounds: None,
                })
                .map_err(|error| TinyBusError::failed(error.to_string()))?,
            ))
            .map_err(|error| TinyBusError::failed(error.to_string()));
        }

        let config = self.0.config.read().await.clone();
        if member.as_str() == names::methods::RUN_GOAL {
            if let Some(gate) = self.0.run_goal_gate.lock().await.take() {
                let _ = gate.entered.send(());
                let _ = gate.release.await;
            }
            let request = args
                .as_array()
                .and_then(|items| items.first())
                .cloned()
                .unwrap_or(Value::Null);
            return response(member.as_str(), &request, &config);
        }

        serde_json::to_value(test_capabilities(&config))
            .map_err(|error| TinyBusError::failed(error.to_string()))
    }
}

struct LinkedFixture {
    client: ComputerClient,
    state: Arc<TestModuleState>,
    _host: Arc<ModuleHost>,
    _broker_task: tokio::task::JoinHandle<tinybus::Result<()>>,
}

fn linked_fixture(initial: &ComputerConfig) -> Result<LinkedFixture, Box<dyn std::error::Error>> {
    let bus = MemoryBus::new();
    let broker = Broker::new();
    let broker_task = broker.spawn(bus.clone());
    let host = Arc::new(ModuleHost::new(broker));
    let initial_value = serde_json::to_value(initial)?;
    host.attach_linked_with_config(linked_service::linked_module()?, initial_value)?;
    let state_guard = linked_state_slot()
        .lock()
        .map_err(|_| std::io::Error::other("linked test state poisoned"))?;
    let state = Arc::clone(
        state_guard
            .as_ref()
            .ok_or_else(|| std::io::Error::other("linked setup did not publish its test state"))?,
    );
    let client = client_for(&bus, Some(Arc::clone(&host)));
    client.record_loaded_with(initial)?;
    Ok(LinkedFixture {
        client,
        state,
        _host: host,
        _broker_task: broker_task,
    })
}

#[tokio::test(flavor = "current_thread")]
async fn describe_uses_the_zero_argument_wire_shape() -> Result<(), Box<dyn std::error::Error>> {
    let _serial = LINKED_TEST_LOCK.lock().await;
    let config = ComputerConfig::default();
    let fixture = linked_fixture(&config)?;

    let described = fixture.client.describe(&config).await?;

    assert_eq!(
        described.contract_version,
        tinycomputer_bus::CONTRACT_VERSION
    );
    let describe_call = fixture
        .state
        .observed
        .lock()
        .map_err(|_| std::io::Error::other("observed calls poisoned"))?
        .iter()
        .find(|call| call.member == task_names::methods::DESCRIBE)
        .cloned()
        .ok_or_else(|| std::io::Error::other("Describe call was not observed"))?;
    assert_eq!(describe_call.args, Value::Array(Vec::new()));
    Ok(())
}

#[tokio::test(flavor = "current_thread", start_paused = true)]
async fn run_goal_remains_pending_past_the_default_bus_timeout()
-> Result<(), Box<dyn std::error::Error>> {
    let _serial = LINKED_TEST_LOCK.lock().await;
    let config = ComputerConfig::default();
    let fixture = linked_fixture(&config)?;
    let (entered_tx, mut entered_rx) = tokio::sync::oneshot::channel();
    let (release_tx, release_rx) = tokio::sync::oneshot::channel();
    *fixture.state.run_goal_gate.lock().await = Some(FocusGate {
        entered: entered_tx,
        release: release_rx,
    });
    let keep_clock_scheduled = tokio::spawn(async {
        loop {
            tokio::task::yield_now().await;
        }
    });

    let client = fixture.client.clone();
    let call_config = config.clone();
    let mut call = tokio::spawn(async move {
        client
            .call_desktop::<_, String>(
                &call_config,
                names::methods::RUN_GOAL,
                json!({"goal": "wait for the fixture"}),
            )
            .await
    });
    tokio::select! {
        biased;
        result = &mut call => {
            keep_clock_scheduled.abort();
            return Err(std::io::Error::other(format!(
                "RunGoal call finished before reaching its service gate: {result:?}"
            )).into());
        }
        result = &mut entered_rx => {
            result.map_err(|_| std::io::Error::other("RunGoal service exited before the gate"))?;
        }
    }

    tokio::time::advance(Duration::from_secs(31)).await;
    tokio::task::yield_now().await;
    assert!(
        !call.is_finished(),
        "RunGoal must use its extended deadline instead of TinyBus' default"
    );

    release_tx
        .send(())
        .map_err(|()| std::io::Error::other("RunGoal gate closed before release"))?;
    let result = call.await?;
    keep_clock_scheduled.abort();
    let _ = keep_clock_scheduled.await;
    assert_eq!(result?, names::methods::RUN_GOAL);
    Ok(())
}

#[tokio::test(flavor = "current_thread")]
async fn attested_client_keeps_configuration_exclusive_through_member_completion()
-> Result<(), Box<dyn std::error::Error>> {
    let _serial = LINKED_TEST_LOCK.lock().await;
    let first_config = ComputerConfig {
        jev: Some(JevConfig::new("fixture-secret")),
        ..ComputerConfig::default()
    };
    let second_config = ComputerConfig::default();
    let fixture = linked_fixture(&first_config)?;
    assert!(format!("{:?}", fixture.client).contains("ComputerClient"));

    let described = fixture.client.describe(&first_config).await?;
    assert!(described.jev_configured);
    assert_eq!(
        fixture
            .state
            .applied
            .lock()
            .map_err(|_| std::io::Error::other("applied config log poisoned"))?
            .len(),
        1
    );

    let (entered_tx, entered_rx) = tokio::sync::oneshot::channel();
    let (release_tx, release_rx) = tokio::sync::oneshot::channel();
    *fixture.state.focus_gate.lock().await = Some(FocusGate {
        entered: entered_tx,
        release: release_rx,
    });
    let first_client = fixture.client.clone();
    let call_config = first_config.clone();
    let focus = tokio::spawn(async move {
        first_client
            .call_desktop::<_, FocusedTextContext>(
                &call_config,
                names::accessibility::ACCESSIBILITY_FOCUS,
                FocusQuery::default(),
            )
            .await
    });
    entered_rx.await?;

    let second_client = fixture.client.clone();
    let next_config = second_config.clone();
    let (started_tx, started_rx) = tokio::sync::oneshot::channel();
    let next = tokio::spawn(async move {
        let _ = started_tx.send(());
        second_client.describe(&next_config).await
    });
    started_rx.await?;
    tokio::task::yield_now().await;
    assert!(
        !next.is_finished(),
        "second call waits for the active guard"
    );
    assert_eq!(
        fixture
            .state
            .applied
            .lock()
            .map_err(|_| std::io::Error::other("applied config log poisoned"))?
            .len(),
        1
    );

    release_tx
        .send(())
        .map_err(|()| std::io::Error::other("focus gate closed before release"))?;
    let focused = focus.await??;
    assert_eq!(focused.text, "attested result");
    let focus_call = fixture
        .state
        .observed
        .lock()
        .map_err(|_| std::io::Error::other("observed calls poisoned"))?
        .iter()
        .find(|call| call.member == names::accessibility::ACCESSIBILITY_FOCUS)
        .cloned()
        .ok_or_else(|| std::io::Error::other("linked focus call was not observed"))?;
    assert!(focus_call.confidential);
    assert_eq!(focus_call.args[0]["verbose"], false);
    let reconfigured = next.await??;
    assert!(!reconfigured.jev_configured);
    assert_eq!(
        fixture
            .state
            .applied
            .lock()
            .map_err(|_| std::io::Error::other("applied config log poisoned"))?
            .len(),
        2
    );
    Ok(())
}
