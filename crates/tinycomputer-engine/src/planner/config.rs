//! The module's private `planner` configuration: which models plan, rescue,
//! and shape a task, and the route each is called through.
//!
//! A route is a provider ([`LanguageModelProvider`]), its credential, and at
//! most an exact repeat of that provider's one approved base URL — the same
//! allow-list principle as Jev's endpoint, so private configuration can never
//! point a model's key at an arbitrary host.

use tinycomputer_bus::agent::{LanguageModelConfiguration, LanguageModelProvider};
pub use tinycomputer_bus::config::{ModelRoute, PlannerConfig};

/// The model used when the configuration names none.
pub const PLANNER_MODEL: &str = "anthropic/claude-sonnet-5";

/// The reasoning model a failed step is rescued with when the configuration
/// names none.
pub const RESCUE_MODEL: &str = "openai/gpt-6-luna";

/// The reasoning model a finished task's answer is shaped with when the
/// configuration names none.
pub const OUTPUT_MODEL: &str = "openai/gpt-6-luna";

/// `OpenRouter`'s OpenAI-compatible base URL, the only one the
/// `open_router` route accepts.
pub const OPEN_ROUTER_BASE_URL: &str = "https://openrouter.ai/api/v1";

/// Tiny Humans' OpenAI-compatible gateway, the only base URL the
/// `tiny_humans` route accepts.
pub const TINYHUMANS_BASE_URL: &str = "https://api.tinyhumans.ai/openai/v1";

/// A route checked and ready to build a model on.
pub(super) struct ResolvedRoute {
    pub(super) provider: LanguageModelProvider,
    pub(super) base_url: String,
    pub(super) sdk_name: Option<String>,
}

/// Validates a configured route and resolves its approved base URL.
pub(super) fn resolve_route(route: &ModelRoute) -> Result<ResolvedRoute, String> {
    if route.api_key.trim().is_empty() {
        return Err("the planner configuration needs an api_key".to_owned());
    }
    let base_url = match &route.endpoint_url {
        Some(endpoint) if trusted_base_url(route.provider, endpoint) => {
            endpoint.trim_end_matches('/').to_owned()
        }
        Some(_) => {
            return Err("endpoint_url is not an approved language model route".to_owned());
        }
        None => approved_base_url(route.provider).to_owned(),
    };
    let sdk_name = match route.provider {
        LanguageModelProvider::TinyHumans => route.sdk_name.as_deref().and_then(sanitized_sdk_name),
        LanguageModelProvider::OpenRouter => None,
    };
    Ok(ResolvedRoute {
        provider: route.provider,
        base_url,
        sdk_name,
    })
}

/// Returns the non-secret model summary stored on the planner, rescuer, or
/// shaper.
pub(super) fn describe_route(route: &ModelRoute, model: &str) -> LanguageModelConfiguration {
    LanguageModelConfiguration {
        provider: route.provider,
        model: model.to_owned(),
        endpoint_url: route.endpoint_url.clone(),
    }
}

/// The one base URL each route may be configured at.
#[must_use]
pub(super) const fn approved_base_url(provider: LanguageModelProvider) -> &'static str {
    match provider {
        LanguageModelProvider::OpenRouter => OPEN_ROUTER_BASE_URL,
        LanguageModelProvider::TinyHumans => TINYHUMANS_BASE_URL,
    }
}

/// Whether `endpoint` is `provider`'s approved base URL, with or without a
/// trailing slash.
pub(super) fn trusted_base_url(provider: LanguageModelProvider, endpoint: &str) -> bool {
    if endpoint.trim_end_matches('/') == approved_base_url(provider) {
        return true;
    }
    #[cfg(test)]
    return endpoint.starts_with("http://127.0.0.1:");
    #[cfg(not(test))]
    false
}

/// `raw` as a header-safe product name, as the Jev client sanitizes its own:
/// ASCII alphanumerics, `.`, `_`, and `-`, lowercased, at most 64 bytes.
fn sanitized_sdk_name(raw: &str) -> Option<String> {
    let name = raw
        .trim()
        .chars()
        .filter(|character| {
            character.is_ascii_alphanumeric() || matches!(character, '.' | '_' | '-')
        })
        .take(64)
        .map(|character| character.to_ascii_lowercase())
        .collect::<String>();
    (!name.is_empty()).then_some(name)
}
