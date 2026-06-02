//! Provider implementations.
//!
//! This module contains concrete implementations of the Provider trait
//! for various LLM APIs.

use crate::error::{Error, Result};
use crate::http::client::{Client, RequestBuilder};
use crate::models::ModelEntry;
use crate::provider::Provider;
use crate::provider_metadata::{
    PROVIDER_METADATA, canonical_provider_id, provider_routing_defaults,
};
use crate::vcr::{VCR_ENV_MODE, VcrRecorder};
use std::collections::HashMap;
use std::env;
use std::sync::Arc;
use url::Url;

pub mod anthropic;
pub mod azure;
pub mod cohere;
pub mod copilot;
pub mod gemini;
pub mod gitlab;
pub mod model_fetch;
pub mod openai;
pub mod openai_responses;
pub mod vertex;

pub use model_fetch::{
    DISABLE_CACHE_ENV, MODEL_CACHE_TTL, fetch_provider_models, refresh_provider_models,
    static_registry_models,
};

pub(super) fn first_non_empty_header_value_case_insensitive(
    headers: &HashMap<String, String>,
    names: &[&str],
) -> Option<String> {
    headers.iter().find_map(|(key, value)| {
        names
            .iter()
            .any(|name| key.eq_ignore_ascii_case(name))
            .then_some(value.trim())
            .filter(|value| !value.is_empty())
            .map(ToString::to_string)
    })
}

pub(super) fn apply_headers_ignoring_blank_auth_overrides<'a>(
    mut request: RequestBuilder<'a>,
    headers: &HashMap<String, String>,
    auth_names: &[&str],
) -> RequestBuilder<'a> {
    for (key, value) in headers {
        let is_blank_auth_override =
            auth_names.iter().any(|name| key.eq_ignore_ascii_case(name)) && value.trim().is_empty();
        if is_blank_auth_override {
            continue;
        }
        request = request.header(key, value);
    }
    request
}

fn base_url_targets_loopback(base_url: &str) -> bool {
    let Ok(url) = Url::parse(base_url) else {
        return false;
    };
    match url.host() {
        Some(url::Host::Domain("localhost")) => true,
        Some(url::Host::Ipv4(addr)) => addr.is_loopback(),
        Some(url::Host::Ipv6(addr)) => addr.is_loopback(),
        _ => false,
    }
}

fn vcr_client_if_enabled(base_url: &str) -> Result<Option<Client>> {
    if env::var(VCR_ENV_MODE).is_err() {
        return Ok(None);
    }

    if base_url_targets_loopback(base_url) && env::var("PI_VCR_ALLOW_LOOPBACK").is_err() {
        return Ok(None);
    }

    let test_name = env::var("PI_VCR_TEST_NAME").unwrap_or_else(|_| "pi_runtime".to_string());
    let recorder = VcrRecorder::new(&test_name)?;
    Ok(Some(Client::new().with_vcr(recorder)))
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ProviderRouteKind {
    NativeAnthropic,
    NativeOpenAICompletions,
    NativeOpenAIResponses,
    NativeOpenAICodexResponses,
    NativeCohere,
    NativeGoogle,
    NativeGoogleGeminiCli,
    NativeGoogleVertex,
    NativeAzure,
    NativeCopilot,
    NativeGitlab,
    ApiAnthropicMessages,
    ApiOpenAICompletions,
    ApiOpenAIResponses,
    ApiOpenAICodexResponses,
    ApiCohereChat,
    ApiGoogleGenerativeAi,
    ApiGoogleGeminiCli,
}

impl ProviderRouteKind {
    const fn as_str(self) -> &'static str {
        match self {
            Self::NativeAnthropic => "native:anthropic",
            Self::NativeOpenAICompletions => "native:openai-completions",
            Self::NativeOpenAIResponses => "native:openai-responses",
            Self::NativeOpenAICodexResponses => "native:openai-codex-responses",
            Self::NativeCohere => "native:cohere",
            Self::NativeGoogle => "native:google",
            Self::NativeGoogleGeminiCli => "native:google-gemini-cli",
            Self::NativeGoogleVertex => "native:google-vertex",
            Self::NativeAzure => "native:azure-openai",
            Self::NativeCopilot => "native:github-copilot",
            Self::NativeGitlab => "native:gitlab",
            Self::ApiAnthropicMessages => "api:anthropic-messages",
            Self::ApiOpenAICompletions => "api:openai-completions",
            Self::ApiOpenAIResponses => "api:openai-responses",
            Self::ApiOpenAICodexResponses => "api:openai-codex-responses",
            Self::ApiCohereChat => "api:cohere-chat",
            Self::ApiGoogleGenerativeAi => "api:google-generative-ai",
            Self::ApiGoogleGeminiCli => "api:google-gemini-cli",
        }
    }
}

fn resolve_provider_route(entry: &ModelEntry) -> Result<(ProviderRouteKind, String, String)> {
    let canonical_provider =
        canonical_provider_id(&entry.model.provider).unwrap_or(entry.model.provider.as_str());
    let schema_api = provider_routing_defaults(&entry.model.provider).map(|defaults| defaults.api);
    let effective_api = if entry.model.api.is_empty() {
        schema_api.unwrap_or_default().to_string()
    } else {
        entry.model.api.clone()
    };

    let route = match canonical_provider {
        "anthropic" => ProviderRouteKind::NativeAnthropic,
        "openai" => {
            if effective_api == "openai-completions" {
                ProviderRouteKind::NativeOpenAICompletions
            } else {
                ProviderRouteKind::NativeOpenAIResponses
            }
        }
        "openai-codex" => ProviderRouteKind::NativeOpenAICodexResponses,
        "cohere" => ProviderRouteKind::NativeCohere,
        "google" => ProviderRouteKind::NativeGoogle,
        "google-gemini-cli" | "google-antigravity" => ProviderRouteKind::NativeGoogleGeminiCli,
        "google-vertex" | "vertexai" => ProviderRouteKind::NativeGoogleVertex,
        "azure-openai" | "azure" | "azure-cognitive-services" | "azure-openai-responses" => {
            ProviderRouteKind::NativeAzure
        }
        "github-copilot" | "copilot" => ProviderRouteKind::NativeCopilot,
        "gitlab" | "gitlab-duo" => ProviderRouteKind::NativeGitlab,
        _ => match effective_api.as_str() {
            "anthropic-messages" => ProviderRouteKind::ApiAnthropicMessages,
            "openai-completions" => ProviderRouteKind::ApiOpenAICompletions,
            "openai-responses" => ProviderRouteKind::ApiOpenAIResponses,
            "openai-codex-responses" => ProviderRouteKind::ApiOpenAICodexResponses,
            "cohere-chat" => ProviderRouteKind::ApiCohereChat,
            "google-generative-ai" => ProviderRouteKind::ApiGoogleGenerativeAi,
            "google-gemini-cli" => ProviderRouteKind::ApiGoogleGeminiCli,
            "google-vertex" => ProviderRouteKind::NativeGoogleVertex,
            "azure-openai-responses" => ProviderRouteKind::NativeAzure,
            _ => {
                let suggestions = suggest_similar_providers(&entry.model.provider);
                let msg = if suggestions.is_empty() {
                    format!("Provider not implemented (api: {effective_api})")
                } else {
                    format!(
                        "Provider not implemented (api: {effective_api}). Did you mean: {}?",
                        suggestions.join(", ")
                    )
                };
                return Err(Error::provider(&entry.model.provider, msg));
            }
        },
    };

    Ok((route, canonical_provider.to_string(), effective_api))
}

/// Levenshtein edit distance between two byte slices. Uses a single-row
/// buffer so memory is O(min(a,b)).
fn edit_distance(a: &[u8], b: &[u8]) -> usize {
    let (short, long) = if a.len() <= b.len() { (a, b) } else { (b, a) };
    let mut row: Vec<usize> = (0..=short.len()).collect();
    for (i, &lb) in long.iter().enumerate() {
        let mut prev = i;
        row[0] = i + 1;
        for (j, &sb) in short.iter().enumerate() {
            let cost = usize::from(lb != sb);
            let val = (row[j + 1] + 1).min(row[j] + 1).min(prev + cost);
            prev = row[j + 1];
            row[j + 1] = val;
        }
    }
    row[short.len()]
}

/// Maximum edit distance allowed for a fuzzy suggestion, scaled by the
/// length of the input so very short inputs don't produce false positives.
const fn max_edit_distance(input_len: usize) -> usize {
    match input_len {
        0..=2 => 0,
        3..=5 => 1,
        6..=9 => 2,
        _ => 3,
    }
}

/// Suggest provider names similar to `input` by checking prefix matching,
/// substring containment, and Levenshtein edit distance against all
/// canonical IDs and aliases.
fn suggest_similar_providers(input: &str) -> Vec<String> {
    let needle = input.to_lowercase();
    let needle_bytes = needle.as_bytes();
    let threshold = max_edit_distance(needle.len());
    let mut matches: Vec<(usize, String)> = Vec::new();

    for meta in PROVIDER_METADATA {
        let names: Vec<&str> = std::iter::once(meta.canonical_id)
            .chain(meta.aliases.iter().copied())
            .collect();
        let mut matched = false;
        for name in &names {
            let haystack = name.to_lowercase();
            // Tier 0: exact prefix match (highest quality)
            if haystack.starts_with(&needle) || needle.starts_with(&haystack) {
                matches.push((0, meta.canonical_id.to_string()));
                matched = true;
                break;
            }
            // Tier 1: substring containment
            if haystack.contains(&needle) || needle.contains(&haystack) {
                matches.push((1, meta.canonical_id.to_string()));
                matched = true;
                break;
            }
        }
        if matched {
            continue;
        }
        // Tier 2: edit distance (typo correction)
        if threshold > 0 {
            let mut best_dist = usize::MAX;
            for name in &names {
                let haystack = name.to_lowercase();
                let dist = edit_distance(needle_bytes, haystack.as_bytes());
                best_dist = best_dist.min(dist);
            }
            if best_dist <= threshold {
                // Encode distance in the sort key so closer matches rank higher
                matches.push((
                    2_usize.wrapping_add(best_dist),
                    meta.canonical_id.to_string(),
                ));
            }
        }
    }

    matches.sort_by(|a, b| a.0.cmp(&b.0).then_with(|| a.1.cmp(&b.1)));
    matches.dedup_by(|a, b| a.1 == b.1);
    matches.truncate(3);
    matches.into_iter().map(|(_, name)| name).collect()
}

const AZURE_OPENAI_RESOURCE_ENV: &str = "AZURE_OPENAI_RESOURCE";
const AZURE_OPENAI_DEPLOYMENT_ENV: &str = "AZURE_OPENAI_DEPLOYMENT";
const AZURE_OPENAI_API_VERSION_ENV: &str = "AZURE_OPENAI_API_VERSION";

#[derive(Debug, Clone, PartialEq, Eq)]
struct AzureProviderRuntime {
    resource: String,
    deployment: String,
    api_version: String,
    endpoint_url: String,
}

fn trim_non_empty(value: Option<String>) -> Option<String> {
    value
        .map(|v| v.trim().to_string())
        .filter(|v| !v.is_empty())
}

fn parse_azure_resource_from_host(host: &str) -> Option<String> {
    host.strip_suffix(".openai.azure.com")
        .or_else(|| host.strip_suffix(".cognitiveservices.azure.com"))
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(ToString::to_string)
}

fn parse_azure_base_url_details(
    base_url: &str,
) -> Result<(String, Option<String>, Option<String>)> {
    let url = Url::parse(base_url)
        .map_err(|err| Error::config(format!("Invalid Azure base_url '{base_url}': {err}")))?;
    let host = url.host_str().map(ToString::to_string).ok_or_else(|| {
        Error::config(format!(
            "Azure base_url is missing host information: '{base_url}'"
        ))
    })?;

    let mut deployment = None;
    if let Some(segments) = url.path_segments() {
        let mut iter = segments;
        while let Some(segment) = iter.next() {
            if segment == "deployments" {
                deployment = iter
                    .next()
                    .map(str::trim)
                    .filter(|value| !value.is_empty())
                    .map(ToString::to_string);
                break;
            }
        }
    }

    let api_version = url
        .query_pairs()
        .find(|(key, _)| key == "api-version")
        .map(|(_, value)| value.into_owned())
        .filter(|value| !value.trim().is_empty());

    Ok((host, deployment, api_version))
}

fn resolve_azure_provider_runtime(entry: &ModelEntry) -> Result<AzureProviderRuntime> {
    resolve_azure_provider_runtime_with_env(entry, |name| env::var(name).ok())
}

fn resolve_azure_provider_runtime_with_env<F>(
    entry: &ModelEntry,
    mut env_lookup: F,
) -> Result<AzureProviderRuntime>
where
    F: FnMut(&str) -> Option<String>,
{
    let base_url = entry.model.base_url.trim();
    if base_url.is_empty() {
        return Err(Error::config(format!(
            "Missing Azure base_url for provider '{}'; expected https://<resource>.openai.azure.com or https://<resource>.cognitiveservices.azure.com",
            entry.model.provider
        )));
    }

    let (host, base_deployment, base_api_version) = parse_azure_base_url_details(base_url)?;
    let host_resource = parse_azure_resource_from_host(&host);
    let env_resource = trim_non_empty(env_lookup(AZURE_OPENAI_RESOURCE_ENV));
    let resource = env_resource.or(host_resource).ok_or_else(|| {
        Error::config(format!(
            "Unable to resolve Azure resource for provider '{}'; set {AZURE_OPENAI_RESOURCE_ENV} or use an Azure host in base_url ('{base_url}')",
            entry.model.provider
        ))
    })?;

    let env_deployment = trim_non_empty(env_lookup(AZURE_OPENAI_DEPLOYMENT_ENV));
    let model_deployment = {
        let model_id = entry.model.id.trim();
        (!model_id.is_empty()).then(|| model_id.to_string())
    };
    let deployment = env_deployment
        .or(base_deployment)
        .or(model_deployment)
        .ok_or_else(|| {
            Error::config(format!(
                "Unable to resolve Azure deployment for provider '{}'; set {AZURE_OPENAI_DEPLOYMENT_ENV}, provide a non-empty model id, or include '/deployments/<name>' in base_url ('{base_url}')",
                entry.model.provider
            ))
        })?;

    let api_version = trim_non_empty(env_lookup(AZURE_OPENAI_API_VERSION_ENV))
        .or(base_api_version)
        .unwrap_or_else(azure::azure_api_version);

    let endpoint_host = if parse_azure_resource_from_host(&host).is_some() {
        host
    } else {
        format!("{resource}.openai.azure.com")
    };
    let endpoint_url = format!(
        "https://{endpoint_host}/openai/deployments/{deployment}/chat/completions?api-version={api_version}"
    );

    Ok(AzureProviderRuntime {
        resource,
        deployment,
        api_version,
        endpoint_url,
    })
}

fn resolve_copilot_token(entry: &ModelEntry) -> Result<String> {
    resolve_copilot_token_with_env(entry, |name| env::var(name).ok())
}

fn resolve_copilot_token_with_env<F>(entry: &ModelEntry, mut env_lookup: F) -> Result<String>
where
    F: FnMut(&str) -> Option<String>,
{
    let inline = entry
        .api_key
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(ToString::to_string);
    let from_env = || {
        env_lookup("GITHUB_COPILOT_API_KEY")
            .or_else(|| env_lookup("GITHUB_TOKEN"))
            .map(|value| value.trim().to_string())
            .filter(|value| !value.is_empty())
    };

    inline.or_else(from_env).ok_or_else(|| {
        Error::auth(
            "GitHub Copilot requires login credentials or GITHUB_COPILOT_API_KEY/GITHUB_TOKEN",
        )
    })
}

#[allow(clippy::too_many_lines)]
pub fn create_provider(entry: &ModelEntry) -> Result<Arc<dyn Provider>> {
    let (route, canonical_provider, effective_api) = resolve_provider_route(entry)?;
    let vcr_client = vcr_client_if_enabled(&entry.model.base_url)?;
    let client = vcr_client.unwrap_or_else(Client::new);
    tracing::debug!(
        event = "pi.provider.factory.select",
        provider = %entry.model.provider,
        canonical_provider = %canonical_provider,
        api = %effective_api,
        base_url = %entry.model.base_url,
        route = %route.as_str(),
        "Selecting provider implementation"
    );

    match route {
        ProviderRouteKind::NativeAnthropic | ProviderRouteKind::ApiAnthropicMessages => {
            Ok(Arc::new(
                anthropic::AnthropicProvider::new(entry.model.id.clone())
                    .with_provider_name(entry.model.provider.clone())
                    .with_base_url(normalize_anthropic_base(&entry.model.base_url))
                    .with_compat(entry.compat.clone())
                    .with_client(client),
            ))
        }
        ProviderRouteKind::NativeOpenAICompletions | ProviderRouteKind::ApiOpenAICompletions => {
            Ok(Arc::new(
                openai::OpenAIProvider::new(entry.model.id.clone())
                    .with_provider_name(entry.model.provider.clone())
                    .with_base_url(normalize_openai_base(&entry.model.base_url))
                    .with_compat(entry.compat.clone())
                    .with_client(client),
            ))
        }
        ProviderRouteKind::NativeOpenAIResponses | ProviderRouteKind::ApiOpenAIResponses => {
            Ok(Arc::new(
                openai_responses::OpenAIResponsesProvider::new(entry.model.id.clone())
                    .with_provider_name(entry.model.provider.clone())
                    .with_base_url(normalize_openai_responses_base(&entry.model.base_url))
                    .with_compat(entry.compat.clone())
                    .with_client(client),
            ))
        }
        ProviderRouteKind::NativeOpenAICodexResponses
        | ProviderRouteKind::ApiOpenAICodexResponses => Ok(Arc::new(
            openai_responses::OpenAIResponsesProvider::new(entry.model.id.clone())
                .with_provider_name(entry.model.provider.clone())
                .with_api_name("openai-codex-responses")
                .with_codex_mode(true)
                .with_base_url(normalize_openai_codex_responses_base(&entry.model.base_url))
                .with_compat(entry.compat.clone())
                .with_client(client),
        )),
        ProviderRouteKind::NativeCohere | ProviderRouteKind::ApiCohereChat => Ok(Arc::new(
            cohere::CohereProvider::new(entry.model.id.clone())
                .with_provider_name(entry.model.provider.clone())
                .with_base_url(normalize_cohere_base(&entry.model.base_url))
                .with_compat(entry.compat.clone())
                .with_client(client),
        )),
        ProviderRouteKind::NativeGoogle | ProviderRouteKind::ApiGoogleGenerativeAi => Ok(Arc::new(
            gemini::GeminiProvider::new(entry.model.id.clone())
                .with_provider_name(entry.model.provider.clone())
                .with_api_name("google-generative-ai")
                .with_base_url(entry.model.base_url.clone())
                .with_compat(entry.compat.clone())
                .with_client(client),
        )),
        ProviderRouteKind::NativeGoogleGeminiCli | ProviderRouteKind::ApiGoogleGeminiCli => {
            Ok(Arc::new(
                gemini::GeminiProvider::new(entry.model.id.clone())
                    .with_provider_name(entry.model.provider.clone())
                    .with_api_name("google-gemini-cli")
                    .with_google_cli_mode(true)
                    .with_base_url(entry.model.base_url.clone())
                    .with_compat(entry.compat.clone())
                    .with_client(client),
            ))
        }
        ProviderRouteKind::NativeGoogleVertex => {
            let runtime = vertex::resolve_vertex_provider_runtime(entry)?;
            Ok(Arc::new(
                vertex::VertexProvider::new(runtime.model)
                    .with_project(runtime.project)
                    .with_location(runtime.location)
                    .with_publisher(runtime.publisher)
                    .with_compat(entry.compat.clone())
                    .with_client(client),
            ))
        }
        ProviderRouteKind::NativeAzure => {
            let runtime = resolve_azure_provider_runtime(entry)?;
            Ok(Arc::new(
                azure::AzureOpenAIProvider::new(runtime.resource, runtime.deployment)
                    .with_provider_name(&entry.model.provider)
                    .with_api_version(runtime.api_version)
                    .with_endpoint_url(runtime.endpoint_url)
                    .with_compat(entry.compat.clone())
                    .with_client(client),
            ))
        }
        ProviderRouteKind::NativeCopilot => {
            let github_token = resolve_copilot_token(entry)?;
            let mut provider = copilot::CopilotProvider::new(&entry.model.id, github_token)
                .with_provider_name(&entry.model.provider)
                .with_compat(entry.compat.clone())
                .with_client(client);
            if !entry.model.base_url.is_empty() {
                provider = provider.with_github_api_base(&entry.model.base_url);
            }
            Ok(Arc::new(provider))
        }
        ProviderRouteKind::NativeGitlab => Ok(Arc::new(
            gitlab::GitLabProvider::new(&entry.model.id)
                .with_provider_name(&entry.model.provider)
                .with_base_url(&entry.model.base_url)
                .with_compat(entry.compat.clone())
                .with_client(client),
        )),
    }
}

pub fn normalize_anthropic_base(base_url: &str) -> String {
    let trimmed = base_url.trim();
    if trimmed.is_empty() {
        return "https://api.anthropic.com/v1/messages".to_string();
    }

    let mut base_for_fallback = trimmed.trim_end_matches('/').to_string();

    if let Ok(url) = Url::parse(trimmed) {
        if url.cannot_be_a_base() {
            base_for_fallback = url.as_str().trim_end_matches('/').to_string();
        } else {
            if trimmed_url_path(&url).ends_with("/v1/messages") {
                return canonicalize_url_path(&url);
            }
            return append_url_path(&url, "v1/messages");
        }
    }

    let base_url = base_for_fallback;
    if base_url.ends_with("/v1/messages") {
        return base_url;
    }
    format!("{base_url}/v1/messages")
}

fn trimmed_url_path(url: &Url) -> &str {
    match url.path().trim_end_matches('/') {
        "" => "/",
        trimmed => trimmed,
    }
}

fn canonicalize_url_path(url: &Url) -> String {
    let mut canonical = url.clone();
    canonical.set_path(trimmed_url_path(url));
    canonical.to_string()
}

fn replace_url_path(url: &Url, path: &str) -> String {
    let mut updated = url.clone();
    updated.set_path(path);
    updated.to_string()
}

fn append_url_path(url: &Url, suffix: &str) -> String {
    let base_path = trimmed_url_path(url);
    let path = if base_path == "/" {
        format!("/{suffix}")
    } else {
        format!("{base_path}/{suffix}")
    };
    replace_url_path(url, &path)
}

fn strip_url_path_suffix(url: &Url, suffix: &str) -> Option<Url> {
    let base_path = trimmed_url_path(url);
    let prefix = base_path.strip_suffix(suffix)?;
    let mut stripped = url.clone();
    stripped.set_path(if prefix.is_empty() { "/" } else { prefix });
    Some(stripped)
}

fn is_official_https_origin(url: &Url, host: &str, default_port: u16) -> bool {
    url.scheme().eq_ignore_ascii_case("https")
        && url
            .host_str()
            .is_some_and(|candidate| candidate.eq_ignore_ascii_case(host))
        && url.port_or_known_default() == Some(default_port)
        && trimmed_url_path(url) == "/"
}

pub fn normalize_openai_base(base_url: &str) -> String {
    let trimmed = base_url.trim();
    if trimmed.is_empty() {
        return "https://api.openai.com/v1/chat/completions".to_string();
    }

    let mut base_for_fallback = trimmed.trim_end_matches('/').to_string();

    if let Ok(url) = Url::parse(trimmed) {
        if url.cannot_be_a_base() {
            base_for_fallback = url.as_str().trim_end_matches('/').to_string();
        } else {
            if trimmed_url_path(&url).ends_with("/chat/completions") {
                return canonicalize_url_path(&url);
            }
            let url = strip_url_path_suffix(&url, "/responses").unwrap_or(url);
            if is_official_https_origin(&url, "api.openai.com", 443) {
                return replace_url_path(&url, "/v1/chat/completions");
            }
            return append_url_path(&url, "chat/completions");
        }
    }

    let base_url = base_for_fallback;
    if base_url.ends_with("/chat/completions") {
        return base_url;
    }
    let base_url = base_url
        .strip_suffix("/responses")
        .unwrap_or(base_url.as_str());
    format!("{base_url}/chat/completions")
}

pub fn normalize_openai_responses_base(base_url: &str) -> String {
    let trimmed = base_url.trim();
    if trimmed.is_empty() {
        return "https://api.openai.com/v1/responses".to_string();
    }

    let mut base_for_fallback = trimmed.trim_end_matches('/').to_string();

    if let Ok(url) = Url::parse(trimmed) {
        if url.cannot_be_a_base() {
            base_for_fallback = url.as_str().trim_end_matches('/').to_string();
        } else {
            if trimmed_url_path(&url).ends_with("/responses") {
                return canonicalize_url_path(&url);
            }
            let url = strip_url_path_suffix(&url, "/chat/completions").unwrap_or(url);
            if is_official_https_origin(&url, "api.openai.com", 443) {
                return replace_url_path(&url, "/v1/responses");
            }
            return append_url_path(&url, "responses");
        }
    }

    let base_url = base_for_fallback;
    if base_url.ends_with("/responses") {
        return base_url;
    }
    let base_url = base_url
        .strip_suffix("/chat/completions")
        .unwrap_or(base_url.as_str());
    format!("{base_url}/responses")
}

pub fn normalize_openai_codex_responses_base(base_url: &str) -> String {
    let trimmed = base_url.trim();
    if trimmed.is_empty() {
        return openai_responses::CODEX_RESPONSES_API_URL.to_string();
    }

    let mut base_for_fallback = trimmed.trim_end_matches('/').to_string();

    if let Ok(url) = Url::parse(trimmed) {
        if url.cannot_be_a_base() {
            base_for_fallback = url.as_str().trim_end_matches('/').to_string();
        } else {
            let path = trimmed_url_path(&url);
            if path.ends_with("/backend-api/codex/responses") || path.ends_with("/responses") {
                return canonicalize_url_path(&url);
            }
            if path.ends_with("/backend-api") {
                return append_url_path(&url, "codex/responses");
            }
            return append_url_path(&url, "backend-api/codex/responses");
        }
    }

    let base = base_for_fallback;
    if base.ends_with("/backend-api/codex/responses") {
        return base;
    }
    // Some registries (including legacy Pi) store the ChatGPT base as
    // `https://chatgpt.com/backend-api`. In that case we only want to append
    // `/codex/responses`, not `/backend-api/codex/responses` again.
    if base.ends_with("/backend-api") {
        return format!("{base}/codex/responses");
    }
    if base.ends_with("/responses") {
        return base;
    }
    format!("{base}/backend-api/codex/responses")
}

pub fn normalize_cohere_base(base_url: &str) -> String {
    let trimmed = base_url.trim();
    if trimmed.is_empty() {
        return "https://api.cohere.com/v2/chat".to_string();
    }

    let mut base_for_fallback = trimmed.trim_end_matches('/').to_string();

    if let Ok(url) = Url::parse(trimmed) {
        if url.cannot_be_a_base() {
            base_for_fallback = url.as_str().trim_end_matches('/').to_string();
        } else {
            if trimmed_url_path(&url).ends_with("/chat") {
                return canonicalize_url_path(&url);
            }
            if is_official_https_origin(&url, "api.cohere.com", 443) {
                return replace_url_path(&url, "/v2/chat");
            }
            return append_url_path(&url, "chat");
        }
    }

    let base_url = base_for_fallback;
    if base_url.ends_with("/chat") {
        return base_url;
    }
    format!("{base_url}/chat")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn vcr_loopback_detection_covers_provider_factory_mock_hosts() {
        assert!(base_url_targets_loopback("http://127.0.0.1:45713/v1"));
        assert!(base_url_targets_loopback("http://localhost:45713/v1"));
        assert!(base_url_targets_loopback("http://[::1]:45713/v1"));
        assert!(base_url_targets_loopback("http://127.12.34.56:45713/v1"));
        assert!(!base_url_targets_loopback("https://api.openai.com/v1"));
        assert!(!base_url_targets_loopback("http://127.example.com/v1"));
        assert!(!base_url_targets_loopback("not a url"));
    }

    // ========================================================================
    // bd-g1nx: Provider factory + URL normalization tests
    // ========================================================================

    use crate::models::ModelEntry;
    use crate::provider::{InputType, Model, ModelCost};
    use std::collections::HashMap;

    fn model_entry(provider: &str, api: &str, model_id: &str, base_url: &str) -> ModelEntry {
        ModelEntry {
            model: Model {
                id: model_id.to_string(),
                name: model_id.to_string(),
                api: api.to_string(),
                provider: provider.to_string(),
                base_url: base_url.to_string(),
                reasoning: false,
                input: vec![InputType::Text],
                cost: ModelCost {
                    input: 3.0,
                    output: 15.0,
                    cache_read: 0.3,
                    cache_write: 3.75,
                },
                context_window: 200_000,
                max_tokens: 8192,
                headers: HashMap::new(),
            },
            api_key: Some("sk-test-key".to_string()),
            headers: HashMap::new(),
            auth_header: true,
            compat: None,
        }
    }

    #[test]
    fn resolve_provider_route_uses_metadata_for_alias_provider() {
        let entry = model_entry(
            "kimi",
            "openai-completions",
            "kimi-k2-instruct",
            "https://api.moonshot.ai/v1",
        );
        let (route, canonical_provider, effective_api) =
            resolve_provider_route(&entry).expect("resolve alias route");
        assert_eq!(route, ProviderRouteKind::ApiOpenAICompletions);
        assert_eq!(canonical_provider, "moonshotai");
        assert_eq!(effective_api, "openai-completions");
    }

    #[test]
    fn resolve_provider_route_openai_unknown_api_defaults_to_native_responses() {
        let entry = model_entry("openai", "openai", "gpt-4o", "https://api.openai.com/v1");
        let (route, canonical_provider, effective_api) =
            resolve_provider_route(&entry).expect("resolve openai route");
        assert_eq!(route, ProviderRouteKind::NativeOpenAIResponses);
        assert_eq!(canonical_provider, "openai");
        assert_eq!(effective_api, "openai");
    }

    #[test]
    fn resolve_provider_route_cloudflare_workers_defaults_to_openai_completions() {
        let entry = model_entry(
            "cloudflare-workers-ai",
            "",
            "@cf/meta/llama-3.1-8b-instruct",
            "https://api.cloudflare.com/client/v4/accounts/test-account/ai/v1",
        );
        let (route, canonical_provider, effective_api) =
            resolve_provider_route(&entry).expect("resolve cloudflare workers route");
        assert_eq!(route, ProviderRouteKind::ApiOpenAICompletions);
        assert_eq!(canonical_provider, "cloudflare-workers-ai");
        assert_eq!(effective_api, "openai-completions");
    }

    #[test]
    fn resolve_provider_route_cloudflare_gateway_defaults_to_openai_completions() {
        let entry = model_entry(
            "cloudflare-ai-gateway",
            "",
            "gpt-4o-mini",
            "https://gateway.ai.cloudflare.com/v1/account-id/gateway-id/openai",
        );
        let (route, canonical_provider, effective_api) =
            resolve_provider_route(&entry).expect("resolve cloudflare gateway route");
        assert_eq!(route, ProviderRouteKind::ApiOpenAICompletions);
        assert_eq!(canonical_provider, "cloudflare-ai-gateway");
        assert_eq!(effective_api, "openai-completions");
    }

    #[test]
    fn resolve_provider_route_uses_native_azure_route_for_cognitive_alias() {
        let entry = model_entry(
            "azure-cognitive-services",
            "openai-completions",
            "gpt-4o-mini",
            "https://myresource.cognitiveservices.azure.com",
        );
        let (route, canonical_provider, effective_api) =
            resolve_provider_route(&entry).expect("resolve azure cognitive route");
        assert_eq!(route, ProviderRouteKind::NativeAzure);
        assert_eq!(canonical_provider, "azure-openai");
        assert_eq!(effective_api, "openai-completions");
    }

    #[test]
    fn resolve_provider_route_uses_native_azure_route_for_legacy_provider_alias() {
        let entry = model_entry(
            "azure-openai-responses",
            "azure-openai-responses",
            "gpt-4o-mini",
            "https://myresource.openai.azure.com",
        );
        let (route, canonical_provider, effective_api) =
            resolve_provider_route(&entry).expect("resolve azure legacy alias route");
        assert_eq!(route, ProviderRouteKind::NativeAzure);
        assert_eq!(canonical_provider, "azure-openai");
        assert_eq!(effective_api, "azure-openai-responses");
    }

    #[test]
    fn resolve_provider_route_accepts_azure_legacy_api_for_custom_provider_id() {
        let entry = model_entry(
            "my-azure",
            "azure-openai-responses",
            "gpt-4o-mini",
            "https://example.invalid",
        );
        let (route, canonical_provider, effective_api) =
            resolve_provider_route(&entry).expect("resolve azure legacy api fallback");
        assert_eq!(route, ProviderRouteKind::NativeAzure);
        assert_eq!(canonical_provider, "my-azure");
        assert_eq!(effective_api, "azure-openai-responses");
    }

    #[test]
    fn resolve_copilot_token_prefers_inline_model_api_key() {
        let mut entry = model_entry("github-copilot", "", "gpt-4o", "");
        entry.api_key = Some("inline-copilot-token".to_string());

        let token = resolve_copilot_token_with_env(&entry, |_| None)
            .expect("inline token should be accepted");
        assert_eq!(token, "inline-copilot-token");
    }

    #[test]
    fn resolve_copilot_token_falls_back_to_env() {
        let mut entry = model_entry("github-copilot", "", "gpt-4o", "");
        entry.api_key = None;

        let token = resolve_copilot_token_with_env(&entry, |name| match name {
            "GITHUB_COPILOT_API_KEY" => Some("env-copilot-token".to_string()),
            _ => None,
        })
        .expect("env token should be accepted");
        assert_eq!(token, "env-copilot-token");
    }

    #[test]
    fn resolve_copilot_token_errors_when_missing_everywhere() {
        let mut entry = model_entry("github-copilot", "", "gpt-4o", "");
        entry.api_key = None;

        let err = resolve_copilot_token_with_env(&entry, |_| None).expect_err("expected error");
        assert!(
            err.to_string().contains("GitHub Copilot requires"),
            "unexpected error: {err}"
        );
    }

    #[test]
    fn suggest_similar_providers_finds_prefix_match() {
        let suggestions = suggest_similar_providers("deep");
        assert!(
            suggestions.contains(&"deepinfra".to_string())
                || suggestions.contains(&"deepseek".to_string()),
            "expected deepinfra or deepseek in suggestions: {suggestions:?}"
        );
    }

    #[test]
    fn suggest_similar_providers_finds_substring_match() {
        let suggestions = suggest_similar_providers("flow");
        assert!(
            suggestions.contains(&"siliconflow".to_string()),
            "expected siliconflow in suggestions: {suggestions:?}"
        );
    }

    #[test]
    fn suggest_similar_providers_returns_empty_for_gibberish() {
        let suggestions = suggest_similar_providers("xyzzzabc123");
        assert!(
            suggestions.is_empty(),
            "expected no suggestions for gibberish: {suggestions:?}"
        );
    }

    #[test]
    fn suggest_similar_providers_caps_at_three() {
        let suggestions = suggest_similar_providers("a");
        assert!(
            suggestions.len() <= 3,
            "expected at most 3 suggestions: {suggestions:?}"
        );
    }

    #[test]
    fn edit_distance_basic_cases() {
        assert_eq!(edit_distance(b"", b""), 0);
        assert_eq!(edit_distance(b"abc", b"abc"), 0);
        assert_eq!(edit_distance(b"abc", b"ab"), 1);
        assert_eq!(edit_distance(b"abc", b"axc"), 1);
        assert_eq!(edit_distance(b"abc", b"abcd"), 1);
        assert_eq!(edit_distance(b"kitten", b"sitting"), 3);
        assert_eq!(edit_distance(b"", b"hello"), 5);
    }

    #[test]
    fn suggest_similar_providers_finds_typo_with_edit_distance() {
        // "anthropick" is edit distance 1 from "anthropic"
        let suggestions = suggest_similar_providers("anthropick");
        assert!(
            suggestions.contains(&"anthropic".to_string()),
            "expected anthropic for typo 'anthropick': {suggestions:?}"
        );
    }

    #[test]
    fn suggest_similar_providers_finds_typo_missing_char() {
        // "openai" with missing letter: "opnai" → edit distance 1
        let suggestions = suggest_similar_providers("opnai");
        assert!(
            suggestions.contains(&"openai".to_string()),
            "expected openai for typo 'opnai': {suggestions:?}"
        );
    }

    #[test]
    fn suggest_similar_providers_finds_transposed_chars() {
        // "gogle" → "google" edit distance 1 (missing 'o')
        let suggestions = suggest_similar_providers("gogle");
        assert!(
            suggestions.contains(&"google".to_string()),
            "expected google for typo 'gogle': {suggestions:?}"
        );
    }

    #[test]
    fn suggest_similar_providers_no_false_positives_for_short_input() {
        // Very short input should not match via edit distance (threshold=0)
        let suggestions = suggest_similar_providers("xy");
        assert!(
            suggestions.is_empty(),
            "expected no suggestions for 'xy': {suggestions:?}"
        );
    }

    #[test]
    fn resolve_azure_provider_runtime_supports_openai_host() {
        let entry = model_entry(
            "azure-openai",
            "openai-completions",
            "gpt-4o",
            "https://myresource.openai.azure.com",
        );
        let runtime =
            resolve_azure_provider_runtime_with_env(&entry, |_| None).expect("resolve runtime");
        assert_eq!(runtime.resource, "myresource");
        assert_eq!(runtime.deployment, "gpt-4o");
        assert_eq!(runtime.api_version, "2024-12-01-preview");
        assert_eq!(
            runtime.endpoint_url,
            "https://myresource.openai.azure.com/openai/deployments/gpt-4o/chat/completions?api-version=2024-12-01-preview"
        );
    }

    #[test]
    fn resolve_azure_provider_runtime_supports_cognitive_services_host() {
        let entry = model_entry(
            "azure-cognitive-services",
            "openai-completions",
            "gpt-4o-mini",
            "https://myresource.cognitiveservices.azure.com/openai/deployments/custom/chat/completions?api-version=2024-10-21",
        );
        let runtime =
            resolve_azure_provider_runtime_with_env(&entry, |_| None).expect("resolve runtime");
        assert_eq!(runtime.resource, "myresource");
        assert_eq!(runtime.deployment, "custom");
        assert_eq!(runtime.api_version, "2024-10-21");
        assert_eq!(
            runtime.endpoint_url,
            "https://myresource.cognitiveservices.azure.com/openai/deployments/custom/chat/completions?api-version=2024-10-21"
        );
    }

    #[test]
    fn resolve_azure_provider_runtime_prefers_base_url_deployment_over_model_id() {
        let entry = model_entry(
            "azure-openai",
            "openai-completions",
            "model-fallback",
            "https://myresource.openai.azure.com/openai/deployments/base-deploy/chat/completions?api-version=2024-10-21",
        );
        let runtime =
            resolve_azure_provider_runtime_with_env(&entry, |_| None).expect("resolve runtime");
        assert_eq!(runtime.resource, "myresource");
        assert_eq!(runtime.deployment, "base-deploy");
        assert_eq!(runtime.api_version, "2024-10-21");
        assert_eq!(
            runtime.endpoint_url,
            "https://myresource.openai.azure.com/openai/deployments/base-deploy/chat/completions?api-version=2024-10-21"
        );
    }

    #[test]
    fn resolve_azure_provider_runtime_env_deployment_overrides_base_url_and_model_id() {
        let entry = model_entry(
            "azure-openai",
            "openai-completions",
            "model-fallback",
            "https://myresource.openai.azure.com/openai/deployments/base-deploy/chat/completions?api-version=2024-10-21",
        );
        let runtime = resolve_azure_provider_runtime_with_env(&entry, |name| match name {
            AZURE_OPENAI_DEPLOYMENT_ENV => Some("env-deploy".to_string()),
            _ => None,
        })
        .expect("resolve runtime");
        assert_eq!(runtime.resource, "myresource");
        assert_eq!(runtime.deployment, "env-deploy");
        assert_eq!(runtime.api_version, "2024-10-21");
        assert_eq!(
            runtime.endpoint_url,
            "https://myresource.openai.azure.com/openai/deployments/env-deploy/chat/completions?api-version=2024-10-21"
        );
    }

    // ── create_provider: built-in provider selection ─────────────────

    #[test]
    fn create_provider_anthropic_by_name() {
        let entry = model_entry(
            "anthropic",
            "anthropic-messages",
            "claude-sonnet-4-5",
            "https://api.anthropic.com",
        );
        let provider = create_provider(&entry).expect("anthropic provider");
        assert_eq!(provider.name(), "anthropic");
        assert_eq!(provider.model_id(), "claude-sonnet-4-5");
        assert_eq!(provider.api(), "anthropic-messages");
    }

    #[test]
    fn create_provider_openai_completions_by_name() {
        let entry = model_entry(
            "openai",
            "openai-completions",
            "gpt-4o",
            "https://api.openai.com/v1",
        );
        let provider = create_provider(&entry).expect("openai completions provider");
        assert_eq!(provider.name(), "openai");
        assert_eq!(provider.model_id(), "gpt-4o");
    }

    #[test]
    fn create_provider_openai_responses_by_name() {
        let entry = model_entry(
            "openai",
            "openai-responses",
            "gpt-4o",
            "https://api.openai.com/v1",
        );
        let provider = create_provider(&entry).expect("openai responses provider");
        assert_eq!(provider.name(), "openai");
        assert_eq!(provider.model_id(), "gpt-4o");
    }

    #[test]
    fn create_provider_openai_defaults_to_responses() {
        // When api is not "openai-completions", OpenAI defaults to Responses API
        let entry = model_entry("openai", "openai", "gpt-4o", "https://api.openai.com/v1");
        let provider = create_provider(&entry).expect("openai default responses provider");
        assert_eq!(provider.name(), "openai");
    }

    #[test]
    fn create_provider_google_by_name() {
        let entry = model_entry(
            "google",
            "google-generative-ai",
            "gemini-2.0-flash",
            "https://generativelanguage.googleapis.com",
        );
        let provider = create_provider(&entry).expect("google provider");
        assert_eq!(provider.name(), "google");
        assert_eq!(provider.model_id(), "gemini-2.0-flash");
    }

    #[test]
    fn create_provider_cohere_by_name() {
        let entry = model_entry(
            "cohere",
            "cohere-chat",
            "command-r-plus",
            "https://api.cohere.com/v2",
        );
        let provider = create_provider(&entry).expect("cohere provider");
        assert_eq!(provider.name(), "cohere");
        assert_eq!(provider.model_id(), "command-r-plus");
    }

    #[test]
    fn create_provider_azure_openai_by_name() {
        let entry = model_entry(
            "azure-openai",
            "openai-completions",
            "gpt-4o",
            "https://myresource.openai.azure.com",
        );
        let provider = create_provider(&entry).expect("azure provider");
        assert_eq!(provider.name(), "azure-openai");
        assert_eq!(provider.api(), "azure-openai");
        assert!(!provider.model_id().is_empty());
    }

    #[test]
    fn create_provider_azure_cognitive_services_alias_by_name() {
        let entry = model_entry(
            "azure-cognitive-services",
            "openai-completions",
            "gpt-4o-mini",
            "https://myresource.cognitiveservices.azure.com",
        );
        let provider = create_provider(&entry).expect("azure cognitive provider");
        assert_eq!(provider.name(), "azure-cognitive-services");
        assert_eq!(provider.api(), "azure-openai");
        assert!(!provider.model_id().is_empty());
    }

    #[test]
    fn create_provider_cloudflare_workers_ai_by_name() {
        let entry = model_entry(
            "cloudflare-workers-ai",
            "",
            "@cf/meta/llama-3.1-8b-instruct",
            "https://api.cloudflare.com/client/v4/accounts/test-account/ai/v1",
        );
        let provider = create_provider(&entry).expect("cloudflare workers provider");
        assert_eq!(provider.name(), "cloudflare-workers-ai");
        assert_eq!(provider.api(), "openai-completions");
        assert_eq!(provider.model_id(), "@cf/meta/llama-3.1-8b-instruct");
    }

    #[test]
    fn create_provider_cloudflare_ai_gateway_by_name() {
        let entry = model_entry(
            "cloudflare-ai-gateway",
            "",
            "gpt-4o-mini",
            "https://gateway.ai.cloudflare.com/v1/account-id/gateway-id/openai",
        );
        let provider = create_provider(&entry).expect("cloudflare gateway provider");
        assert_eq!(provider.name(), "cloudflare-ai-gateway");
        assert_eq!(provider.api(), "openai-completions");
        assert_eq!(provider.model_id(), "gpt-4o-mini");
    }

    // ── create_provider: API fallback path ──────────────────────────

    #[test]
    fn create_provider_falls_back_to_api_anthropic_messages() {
        let entry = model_entry(
            "custom-anthropic",
            "anthropic-messages",
            "my-model",
            "https://custom.api.com",
        );
        let provider = create_provider(&entry).expect("fallback anthropic provider");
        // Anthropic fallback uses the standard anthropic provider
        assert_eq!(provider.model_id(), "my-model");
    }

    #[test]
    fn create_provider_falls_back_to_api_openai_completions() {
        let entry = model_entry(
            "my-openai-compat",
            "openai-completions",
            "local-model",
            "http://localhost:8080/v1",
        );
        let provider = create_provider(&entry).expect("fallback openai completions");
        assert_eq!(provider.model_id(), "local-model");
    }

    #[test]
    fn create_provider_falls_back_to_api_openai_responses() {
        let entry = model_entry(
            "my-openai-compat",
            "openai-responses",
            "local-model",
            "http://localhost:8080/v1",
        );
        let provider = create_provider(&entry).expect("fallback openai responses");
        assert_eq!(provider.model_id(), "local-model");
    }

    #[test]
    fn create_provider_falls_back_to_api_cohere_chat() {
        let entry = model_entry(
            "custom-cohere",
            "cohere-chat",
            "custom-r",
            "https://custom-cohere.api.com/v2",
        );
        let provider = create_provider(&entry).expect("fallback cohere provider");
        assert_eq!(provider.model_id(), "custom-r");
    }

    #[test]
    fn create_provider_falls_back_to_api_google() {
        let entry = model_entry(
            "custom-google",
            "google-generative-ai",
            "custom-gemini",
            "https://custom.google.com",
        );
        let provider = create_provider(&entry).expect("fallback google provider");
        assert_eq!(provider.model_id(), "custom-gemini");
    }

    #[test]
    fn resolve_provider_route_copilot_routes_correctly() {
        let entry = model_entry("github-copilot", "", "gpt-4o", "");
        let (route, canonical, _api) = resolve_provider_route(&entry).expect("copilot route");
        assert_eq!(route, ProviderRouteKind::NativeCopilot);
        assert_eq!(canonical, "github-copilot");
    }

    #[test]
    fn resolve_provider_route_copilot_alias_routes_correctly() {
        let entry = model_entry("copilot", "", "gpt-4o", "");
        let (route, canonical, _api) = resolve_provider_route(&entry).expect("copilot alias route");
        assert_eq!(route, ProviderRouteKind::NativeCopilot);
        assert_eq!(canonical, "github-copilot");
    }

    #[test]
    fn create_provider_unknown_provider_and_api_returns_error() {
        let entry = model_entry(
            "totally-unknown",
            "unknown-api",
            "some-model",
            "https://example.com",
        );
        let Err(err) = create_provider(&entry) else {
            panic!();
        };
        let msg = err.to_string();
        assert!(
            msg.contains("not implemented"),
            "expected 'not implemented' message, got: {msg}"
        );
    }

    // ── normalize_anthropic_base ───────────────────────────────────

    #[test]
    fn normalize_anthropic_base_appends_v1_messages() {
        assert_eq!(
            normalize_anthropic_base("https://api.anthropic.com"),
            "https://api.anthropic.com/v1/messages"
        );
    }

    #[test]
    fn normalize_anthropic_base_keeps_existing_v1_messages() {
        assert_eq!(
            normalize_anthropic_base("https://api.anthropic.com/v1/messages"),
            "https://api.anthropic.com/v1/messages"
        );
    }

    #[test]
    fn normalize_anthropic_base_strips_trailing_slash() {
        assert_eq!(
            normalize_anthropic_base("https://api.anthropic.com/"),
            "https://api.anthropic.com/v1/messages"
        );
    }

    #[test]
    fn normalize_anthropic_base_empty_uses_default() {
        assert_eq!(
            normalize_anthropic_base("   "),
            "https://api.anthropic.com/v1/messages"
        );
    }

    #[test]
    fn normalize_anthropic_base_preserves_query_and_fragment() {
        assert_eq!(
            normalize_anthropic_base("https://api.anthropic.com/?via=proxy#frag"),
            "https://api.anthropic.com/v1/messages?via=proxy#frag"
        );
    }

    #[test]
    fn normalize_anthropic_base_handles_opaque_url_fallback() {
        assert_eq!(
            normalize_anthropic_base("data:text/plain,hello"),
            "data:text/plain,hello/v1/messages"
        );
    }

    // ── normalize_openai_base ───────────────────────────────────────

    #[test]
    fn normalize_openai_base_appends_chat_completions_to_v1() {
        assert_eq!(
            normalize_openai_base("https://api.openai.com/v1"),
            "https://api.openai.com/v1/chat/completions"
        );
    }

    #[test]
    fn normalize_openai_base_keeps_existing_chat_completions() {
        assert_eq!(
            normalize_openai_base("https://api.openai.com/v1/chat/completions"),
            "https://api.openai.com/v1/chat/completions"
        );
    }

    #[test]
    fn normalize_openai_base_strips_trailing_slash() {
        assert_eq!(
            normalize_openai_base("https://api.openai.com/v1/"),
            "https://api.openai.com/v1/chat/completions"
        );
    }

    #[test]
    fn normalize_openai_base_strips_responses_suffix() {
        assert_eq!(
            normalize_openai_base("https://api.openai.com/v1/responses"),
            "https://api.openai.com/v1/chat/completions"
        );
    }

    #[test]
    fn normalize_openai_base_official_bare_url_gets_v1_chat_completions() {
        assert_eq!(
            normalize_openai_base("https://api.openai.com"),
            "https://api.openai.com/v1/chat/completions"
        );
    }

    #[test]
    fn normalize_openai_base_official_default_port_gets_v1_chat_completions() {
        assert_eq!(
            normalize_openai_base("https://api.openai.com:443"),
            "https://api.openai.com/v1/chat/completions"
        );
    }

    #[test]
    fn normalize_openai_base_strips_non_v1_official_responses_suffix() {
        assert_eq!(
            normalize_openai_base("https://api.openai.com/responses"),
            "https://api.openai.com/v1/chat/completions"
        );
    }

    #[test]
    fn normalize_openai_base_custom_bare_url_gets_chat_completions() {
        assert_eq!(
            normalize_openai_base("https://my-llm-proxy.com"),
            "https://my-llm-proxy.com/chat/completions"
        );
    }

    #[test]
    fn normalize_openai_base_preserves_query_and_fragment_on_official_origin() {
        assert_eq!(
            normalize_openai_base("https://api.openai.com:443/?via=proxy#frag"),
            "https://api.openai.com/v1/chat/completions?via=proxy#frag"
        );
    }

    #[test]
    fn normalize_openai_base_empty_uses_default() {
        assert_eq!(
            normalize_openai_base(""),
            "https://api.openai.com/v1/chat/completions"
        );
    }

    #[test]
    fn normalize_openai_base_handles_opaque_url_fallback() {
        assert_eq!(
            normalize_openai_base("data:text/plain,hello"),
            "data:text/plain,hello/chat/completions"
        );
    }

    // ── normalize_openai_responses_base ─────────────────────────────

    #[test]
    fn normalize_responses_appends_responses_to_v1() {
        assert_eq!(
            normalize_openai_responses_base("https://api.openai.com/v1"),
            "https://api.openai.com/v1/responses"
        );
    }

    #[test]
    fn normalize_responses_keeps_existing_responses() {
        assert_eq!(
            normalize_openai_responses_base("https://api.openai.com/v1/responses"),
            "https://api.openai.com/v1/responses"
        );
    }

    #[test]
    fn normalize_responses_strips_trailing_slash() {
        assert_eq!(
            normalize_openai_responses_base("https://api.openai.com/v1/"),
            "https://api.openai.com/v1/responses"
        );
    }

    #[test]
    fn normalize_responses_strips_chat_completions_suffix() {
        assert_eq!(
            normalize_openai_responses_base("https://api.openai.com/v1/chat/completions"),
            "https://api.openai.com/v1/responses"
        );
    }

    #[test]
    fn normalize_responses_official_bare_url_gets_v1_responses() {
        assert_eq!(
            normalize_openai_responses_base("https://api.openai.com"),
            "https://api.openai.com/v1/responses"
        );
    }

    #[test]
    fn normalize_responses_official_default_port_gets_v1_responses() {
        assert_eq!(
            normalize_openai_responses_base("https://api.openai.com:443"),
            "https://api.openai.com/v1/responses"
        );
    }

    #[test]
    fn normalize_responses_strips_non_v1_official_chat_completions_suffix() {
        assert_eq!(
            normalize_openai_responses_base("https://api.openai.com/chat/completions"),
            "https://api.openai.com/v1/responses"
        );
    }

    #[test]
    fn normalize_responses_custom_bare_url_gets_responses() {
        assert_eq!(
            normalize_openai_responses_base("https://my-llm-proxy.com"),
            "https://my-llm-proxy.com/responses"
        );
    }

    #[test]
    fn normalize_responses_preserves_query_and_fragment() {
        assert_eq!(
            normalize_openai_responses_base("https://my-llm-proxy.com/api?via=proxy#frag"),
            "https://my-llm-proxy.com/api/responses?via=proxy#frag"
        );
    }

    #[test]
    fn normalize_responses_preserves_query_and_fragment_on_official_origin() {
        assert_eq!(
            normalize_openai_responses_base("https://api.openai.com:443/?via=proxy#frag"),
            "https://api.openai.com/v1/responses?via=proxy#frag"
        );
    }

    #[test]
    fn normalize_responses_base_empty_uses_default() {
        assert_eq!(
            normalize_openai_responses_base("  "),
            "https://api.openai.com/v1/responses"
        );
    }

    #[test]
    fn normalize_responses_base_handles_opaque_url_fallback() {
        assert_eq!(
            normalize_openai_responses_base("data:text/plain,hello"),
            "data:text/plain,hello/responses"
        );
    }

    // ── normalize_openai_codex_responses_base ──────────────────────

    #[test]
    fn normalize_codex_responses_base_empty_uses_default() {
        assert_eq!(
            normalize_openai_codex_responses_base(""),
            openai_responses::CODEX_RESPONSES_API_URL
        );
    }

    #[test]
    fn normalize_codex_responses_base_keeps_existing_suffix() {
        assert_eq!(
            normalize_openai_codex_responses_base(
                "https://chatgpt.com/backend-api/codex/responses"
            ),
            "https://chatgpt.com/backend-api/codex/responses"
        );
    }

    #[test]
    fn normalize_codex_responses_base_appends_suffix_from_backend_api() {
        assert_eq!(
            normalize_openai_codex_responses_base("https://chatgpt.com/backend-api"),
            "https://chatgpt.com/backend-api/codex/responses"
        );
    }

    #[test]
    fn normalize_codex_responses_base_preserves_query_and_fragment() {
        assert_eq!(
            normalize_openai_codex_responses_base("https://chatgpt.com/backend-api?via=proxy#frag"),
            "https://chatgpt.com/backend-api/codex/responses?via=proxy#frag"
        );
    }

    #[test]
    fn normalize_codex_responses_base_handles_opaque_url_fallback() {
        assert_eq!(
            normalize_openai_codex_responses_base("data:text/plain,hello"),
            "data:text/plain,hello/backend-api/codex/responses"
        );
    }

    // ── normalize_cohere_base ───────────────────────────────────────

    #[test]
    fn normalize_cohere_appends_chat_to_v2() {
        assert_eq!(
            normalize_cohere_base("https://api.cohere.com/v2"),
            "https://api.cohere.com/v2/chat"
        );
    }

    #[test]
    fn normalize_cohere_keeps_existing_chat() {
        assert_eq!(
            normalize_cohere_base("https://api.cohere.com/v2/chat"),
            "https://api.cohere.com/v2/chat"
        );
    }

    #[test]
    fn normalize_cohere_strips_trailing_slash() {
        assert_eq!(
            normalize_cohere_base("https://api.cohere.com/v2/"),
            "https://api.cohere.com/v2/chat"
        );
    }

    #[test]
    fn normalize_cohere_official_bare_url_gets_v2_chat() {
        assert_eq!(
            normalize_cohere_base("https://api.cohere.com"),
            "https://api.cohere.com/v2/chat"
        );
    }

    #[test]
    fn normalize_cohere_official_default_port_gets_v2_chat() {
        assert_eq!(
            normalize_cohere_base("https://api.cohere.com:443"),
            "https://api.cohere.com/v2/chat"
        );
    }

    #[test]
    fn normalize_cohere_custom_bare_url_gets_chat() {
        assert_eq!(
            normalize_cohere_base("https://custom-cohere.example.com"),
            "https://custom-cohere.example.com/chat"
        );
    }

    #[test]
    fn normalize_cohere_preserves_query_and_fragment() {
        assert_eq!(
            normalize_cohere_base("https://custom-cohere.example.com/v2?tenant=test#frag"),
            "https://custom-cohere.example.com/v2/chat?tenant=test#frag"
        );
    }

    #[test]
    fn normalize_cohere_preserves_query_and_fragment_on_official_origin() {
        assert_eq!(
            normalize_cohere_base("https://api.cohere.com:443/?tenant=test#frag"),
            "https://api.cohere.com/v2/chat?tenant=test#frag"
        );
    }

    #[test]
    fn normalize_cohere_base_empty_uses_default() {
        assert_eq!(normalize_cohere_base(""), "https://api.cohere.com/v2/chat");
    }

    #[test]
    fn normalize_cohere_base_handles_opaque_url_fallback() {
        assert_eq!(
            normalize_cohere_base("data:text/plain,hello"),
            "data:text/plain,hello/chat"
        );
    }

    mod proptests {
        use super::*;
        use proptest::prelude::*;

        proptest! {
            #[test]
            fn normalize_anthropic_base_is_idempotent_and_targets_v1_messages(
                base in "[A-Za-z0-9:/._-]{1,96}"
            ) {
                let normalized = normalize_anthropic_base(&base);
                prop_assert!(normalized.ends_with("/v1/messages"));
                prop_assert_eq!(normalize_anthropic_base(&normalized), normalized);
            }

            #[test]
            fn normalize_openai_base_is_idempotent_and_targets_chat_completions(
                base in "[A-Za-z0-9:/._-]{1,96}"
            ) {
                let normalized = normalize_openai_base(&base);
                prop_assert!(normalized.ends_with("/chat/completions"));
                prop_assert_eq!(normalize_openai_base(&normalized), normalized);
            }

            #[test]
            fn normalize_openai_responses_base_is_idempotent_and_targets_responses(
                base in "[A-Za-z0-9:/._-]{1,96}"
            ) {
                let normalized = normalize_openai_responses_base(&base);
                prop_assert!(normalized.ends_with("/responses"));
                prop_assert_eq!(normalize_openai_responses_base(&normalized), normalized);
            }

            #[test]
            fn normalize_cohere_base_is_idempotent_and_targets_chat(
                base in "[A-Za-z0-9:/._-]{1,96}"
            ) {
                let normalized = normalize_cohere_base(&base);
                prop_assert!(normalized.ends_with("/chat"));
                prop_assert_eq!(normalize_cohere_base(&normalized), normalized);
            }

            #[test]
            fn normalize_openai_base_rewrites_responses_suffix(
                host in "[a-z0-9-]{1,32}",
                trailing_slashes in 0usize..4
            ) {
                let base = format!(
                    "https://{host}.example/v1/responses{}",
                    "/".repeat(trailing_slashes)
                );
                prop_assert_eq!(
                    normalize_openai_base(&base),
                    format!("https://{host}.example/v1/chat/completions")
                );
            }

            #[test]
            fn normalize_openai_responses_base_rewrites_chat_completions_suffix(
                host in "[a-z0-9-]{1,32}",
                trailing_slashes in 0usize..4
            ) {
                let base = format!(
                    "https://{host}.example/v1/chat/completions{}",
                    "/".repeat(trailing_slashes)
                );
                prop_assert_eq!(
                    normalize_openai_responses_base(&base),
                    format!("https://{host}.example/v1/responses")
                );
            }
        }
    }

    // ── bd-3uqg.2.4: Compat override propagation ─────────────────────

    use crate::models::CompatConfig;

    fn compat_with_custom_headers() -> CompatConfig {
        let mut custom = HashMap::new();
        custom.insert("X-Custom-Header".to_string(), "test-value".to_string());
        custom.insert("X-Provider-Tag".to_string(), "override".to_string());
        CompatConfig {
            custom_headers: Some(custom),
            ..Default::default()
        }
    }

    fn model_entry_with_compat(
        provider: &str,
        api: &str,
        model_id: &str,
        base_url: &str,
        compat: CompatConfig,
    ) -> ModelEntry {
        let mut entry = model_entry(provider, api, model_id, base_url);
        entry.compat = Some(compat);
        entry
    }

    #[test]
    fn create_provider_anthropic_accepts_compat_config() {
        let entry = model_entry_with_compat(
            "anthropic",
            "anthropic-messages",
            "claude-sonnet-4-5",
            "https://api.anthropic.com",
            compat_with_custom_headers(),
        );
        let provider = create_provider(&entry).expect("anthropic with compat");
        assert_eq!(provider.name(), "anthropic");
    }

    #[test]
    fn create_provider_openai_completions_accepts_compat_config() {
        let entry = model_entry_with_compat(
            "openai",
            "openai-completions",
            "gpt-4o",
            "https://api.openai.com/v1",
            CompatConfig {
                max_tokens_field: Some("max_completion_tokens".to_string()),
                system_role_name: Some("developer".to_string()),
                supports_tools: Some(false),
                ..Default::default()
            },
        );
        let provider = create_provider(&entry).expect("openai completions with compat");
        assert_eq!(provider.name(), "openai");
    }

    #[test]
    fn create_provider_openai_responses_accepts_compat_config() {
        let entry = model_entry_with_compat(
            "openai",
            "openai-responses",
            "gpt-4o",
            "https://api.openai.com/v1",
            compat_with_custom_headers(),
        );
        let provider = create_provider(&entry).expect("openai responses with compat");
        assert_eq!(provider.name(), "openai");
    }

    #[test]
    fn create_provider_cohere_accepts_compat_config() {
        let entry = model_entry_with_compat(
            "cohere",
            "cohere-chat",
            "command-r-plus",
            "https://api.cohere.com/v2",
            compat_with_custom_headers(),
        );
        let provider = create_provider(&entry).expect("cohere with compat");
        assert_eq!(provider.name(), "cohere");
    }

    #[test]
    fn create_provider_google_accepts_compat_config() {
        let entry = model_entry_with_compat(
            "google",
            "google-generative-ai",
            "gemini-2.0-flash",
            "https://generativelanguage.googleapis.com",
            compat_with_custom_headers(),
        );
        let provider = create_provider(&entry).expect("google with compat");
        assert_eq!(provider.name(), "google");
    }

    #[test]
    fn create_provider_fallback_api_routes_accept_compat_config() {
        // Custom provider using anthropic-messages API fallback
        let entry = model_entry_with_compat(
            "custom-anthropic",
            "anthropic-messages",
            "my-model",
            "https://custom.api.com",
            compat_with_custom_headers(),
        );
        let provider = create_provider(&entry).expect("fallback anthropic with compat");
        assert_eq!(provider.model_id(), "my-model");

        // Custom provider using openai-completions API fallback
        let entry = model_entry_with_compat(
            "my-groq-clone",
            "openai-completions",
            "llama-3.1",
            "http://localhost:8080/v1",
            compat_with_custom_headers(),
        );
        let provider = create_provider(&entry).expect("fallback openai with compat");
        assert_eq!(provider.model_id(), "llama-3.1");

        // Custom provider using cohere-chat API fallback
        let entry = model_entry_with_compat(
            "custom-cohere",
            "cohere-chat",
            "custom-r",
            "https://custom-cohere.api.com/v2",
            compat_with_custom_headers(),
        );
        let provider = create_provider(&entry).expect("fallback cohere with compat");
        assert_eq!(provider.model_id(), "custom-r");

        // Custom provider using google-generative-ai API fallback
        let entry = model_entry_with_compat(
            "custom-google",
            "google-generative-ai",
            "custom-gemini",
            "https://custom.google.com",
            compat_with_custom_headers(),
        );
        let provider = create_provider(&entry).expect("fallback google with compat");
        assert_eq!(provider.model_id(), "custom-gemini");
    }

    // ── bd-3uqg.3.1: Google Vertex AI provider routing ──────────────

    #[test]
    fn resolve_provider_route_google_vertex_routes_to_native() {
        let entry = model_entry(
            "google-vertex",
            "google-vertex",
            "gemini-2.0-flash",
            "https://us-central1-aiplatform.googleapis.com/v1/projects/my-project/locations/us-central1/publishers/google/models/gemini-2.0-flash",
        );
        let (route, canonical_provider, effective_api) =
            resolve_provider_route(&entry).expect("resolve google-vertex route");
        assert_eq!(route, ProviderRouteKind::NativeGoogleVertex);
        assert_eq!(canonical_provider, "google-vertex");
        assert_eq!(effective_api, "google-vertex");
    }

    #[test]
    fn resolve_provider_route_vertexai_alias_routes_to_native() {
        let entry = model_entry(
            "vertexai",
            "google-vertex",
            "gemini-2.0-flash",
            "https://us-central1-aiplatform.googleapis.com/v1/projects/my-project/locations/us-central1/publishers/google/models/gemini-2.0-flash",
        );
        let (route, canonical_provider, effective_api) =
            resolve_provider_route(&entry).expect("resolve vertexai alias route");
        assert_eq!(route, ProviderRouteKind::NativeGoogleVertex);
        assert_eq!(canonical_provider, "google-vertex");
        assert_eq!(effective_api, "google-vertex");
    }

    #[test]
    fn resolve_provider_route_google_vertex_api_fallback() {
        // Unknown provider but google-vertex API should still route correctly
        let entry = model_entry(
            "custom-vertex",
            "google-vertex",
            "gemini-2.0-flash",
            "https://us-central1-aiplatform.googleapis.com/v1/projects/my-project/locations/us-central1/publishers/google/models/gemini-2.0-flash",
        );
        let (route, _canonical_provider, effective_api) =
            resolve_provider_route(&entry).expect("resolve google-vertex fallback");
        assert_eq!(route, ProviderRouteKind::NativeGoogleVertex);
        assert_eq!(effective_api, "google-vertex");
    }

    #[test]
    fn create_provider_google_vertex_from_full_url() {
        let entry = model_entry(
            "google-vertex",
            "google-vertex",
            "gemini-2.0-flash",
            "https://us-central1-aiplatform.googleapis.com/v1/projects/my-project/locations/us-central1/publishers/google/models/gemini-2.0-flash",
        );
        let provider = create_provider(&entry).expect("google-vertex from full URL");
        assert_eq!(provider.name(), "google-vertex");
        assert_eq!(provider.api(), "google-vertex");
        assert_eq!(provider.model_id(), "gemini-2.0-flash");
    }

    #[test]
    fn create_provider_google_vertex_anthropic_publisher() {
        let entry = model_entry(
            "google-vertex",
            "google-vertex",
            "claude-sonnet-4-5",
            "https://us-east5-aiplatform.googleapis.com/v1/projects/my-project/locations/us-east5/publishers/anthropic/models/claude-sonnet-4-5",
        );
        let provider = create_provider(&entry).expect("google-vertex with anthropic publisher");
        assert_eq!(provider.name(), "google-vertex");
        assert_eq!(provider.model_id(), "claude-sonnet-4-5");
    }

    #[test]
    fn create_provider_google_vertex_accepts_compat_config() {
        let entry = model_entry_with_compat(
            "google-vertex",
            "google-vertex",
            "gemini-2.0-flash",
            "https://us-central1-aiplatform.googleapis.com/v1/projects/my-project/locations/us-central1/publishers/google/models/gemini-2.0-flash",
            compat_with_custom_headers(),
        );
        let provider = create_provider(&entry).expect("google-vertex with compat");
        assert_eq!(provider.name(), "google-vertex");
    }

    #[test]
    fn create_provider_compat_none_accepted_by_all_routes() {
        // Verify None compat doesn't break anything (regression guard)
        let routes = [
            (
                "anthropic",
                "anthropic-messages",
                "https://api.anthropic.com",
            ),
            ("openai", "openai-completions", "https://api.openai.com/v1"),
            ("openai", "openai-responses", "https://api.openai.com/v1"),
            ("cohere", "cohere-chat", "https://api.cohere.com/v2"),
            (
                "google",
                "google-generative-ai",
                "https://generativelanguage.googleapis.com",
            ),
            (
                "google-vertex",
                "google-vertex",
                "https://us-central1-aiplatform.googleapis.com/v1/projects/my-project/locations/us-central1/publishers/google/models/test-model",
            ),
        ];
        for (provider, api, base_url) in routes {
            let entry = model_entry(provider, api, "test-model", base_url);
            assert!(
                entry.compat.is_none(),
                "expected None compat for {provider}"
            );
            let result = create_provider(&entry);
            assert!(
                result.is_ok(),
                "create_provider failed for {provider} with None compat: {:?}",
                result.err()
            );
        }
    }
}
