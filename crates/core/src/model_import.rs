//! Turns providers from the models.dev catalog (the one OpenCode reads)
//! into registry files for `models.d/`. The catalog is read only when the
//! user runs `socium models import`, never at startup: a provider's
//! `base_url` decides where its API key goes, so it must come from a file
//! the user has on disk and can review.
//!
//! The mapping is a pure function of the parsed catalog; `fetch_catalog`
//! is the only part that touches the network.

use crate::models::{ANTHROPIC_BASE_URL, parse_base_url};
use serde::Deserialize;
use std::collections::BTreeMap;
use std::fmt::Write as _;
use std::fs;
use std::io;
use std::path::{Path, PathBuf};
use std::time::SystemTime;

pub const CATALOG_URL: &str = "https://models.dev/api.json";

/// The catalog is a few megabytes; anything far larger is not it.
const CATALOG_MAX_BYTES: usize = 64 << 20;

const ANTHROPIC: &str = "anthropic";
const OPENAI_CHAT: &str = "openai-chat";
const OPENAI_RESPONSES: &str = "openai-responses";
const GOOGLE: &str = "google";

/// An AI SDK package that a socium wire can speak for.
struct Package {
    npm: &'static str,
    wire: &'static str,
    /// The package's own base URL, used when the catalog gives none.
    default_url: Option<&'static str>,
    /// A path the catalog's URL ends with that socium's wire adds itself.
    strip_suffix: Option<&'static str>,
}

const fn package(npm: &'static str, wire: &'static str, url: &'static str) -> Package {
    Package {
        npm,
        wire,
        default_url: Some(url),
        strip_suffix: None,
    }
}

/// Packages socium can import, with the base URL each AI SDK package uses
/// by default (read from the packages' sources on 2026-11-20). Models of
/// any other package are skipped.
const PACKAGES: &[Package] = &[
    Package {
        npm: "@ai-sdk/anthropic",
        wire: ANTHROPIC,
        default_url: Some(ANTHROPIC_BASE_URL),
        // The AI SDK appends /messages; socium's anthropic wire /v1/messages.
        strip_suffix: Some("/v1"),
    },
    package(
        "@ai-sdk/openai",
        OPENAI_RESPONSES,
        "https://api.openai.com/v1",
    ),
    package(
        "@ai-sdk/google",
        GOOGLE,
        "https://generativelanguage.googleapis.com/v1beta",
    ),
    Package {
        npm: "@ai-sdk/openai-compatible",
        wire: OPENAI_CHAT,
        default_url: None,
        strip_suffix: None,
    },
    package(
        "@ai-sdk/cerebras",
        OPENAI_CHAT,
        "https://api.cerebras.ai/v1",
    ),
    package(
        "@ai-sdk/deepinfra",
        OPENAI_CHAT,
        "https://api.deepinfra.com/v1/openai",
    ),
    package("@ai-sdk/deepseek", OPENAI_CHAT, "https://api.deepseek.com"),
    package(
        "@ai-sdk/fireworks",
        OPENAI_CHAT,
        "https://api.fireworks.ai/inference/v1",
    ),
    package(
        "@ai-sdk/groq",
        OPENAI_CHAT,
        "https://api.groq.com/openai/v1",
    ),
    package("@ai-sdk/mistral", OPENAI_CHAT, "https://api.mistral.ai/v1"),
    package(
        "@ai-sdk/perplexity",
        OPENAI_CHAT,
        "https://api.perplexity.ai",
    ),
    package(
        "@ai-sdk/togetherai",
        OPENAI_CHAT,
        "https://api.together.xyz/v1",
    ),
    package("@ai-sdk/xai", OPENAI_CHAT, "https://api.x.ai/v1"),
    package(
        "@openrouter/ai-sdk-provider",
        OPENAI_CHAT,
        "https://openrouter.ai/api/v1",
    ),
];

/// Providers whose key comes from an OAuth sign-in, not from a key the
/// user can put in an environment variable.
const OAUTH_ONLY: &[&str] = &["github-copilot"];

/// Providers that take requests only with a per-conversation session ID
/// in a header, and that header. OpenCode Go refuses clients that send
/// none (https://opencode.ai/docs/go, read on 2026-11-26).
const SESSION_HEADERS: &[(&str, &str)] = &[("opencode-go", "x-opencode-session")];

/// The features socium's built-in Claude models have.
const CLAUDE_FEATURES: &[&str] = &[
    "adaptive_thinking",
    "effort",
    "eager_input_streaming",
    "anthropic_text_editor",
    "prompt_caching",
];

/// The parsed catalog. Entries are read one at a time, so one malformed
/// provider or model skips only itself.
pub struct Catalog {
    providers: BTreeMap<String, serde_json::Value>,
}

#[derive(Deserialize)]
struct CatalogProvider {
    npm: Option<String>,
    api: Option<String>,
    #[serde(default)]
    env: Vec<String>,
    #[serde(default)]
    models: BTreeMap<String, serde_json::Value>,
}

#[derive(Deserialize)]
struct CatalogModel {
    id: String,
    name: Option<String>,
    #[serde(default)]
    reasoning: bool,
    tool_call: Option<bool>,
    status: Option<String>,
    #[serde(rename = "type")]
    kind: Option<String>,
    modalities: Option<Modalities>,
    limit: Option<Limit>,
    cost: Option<Cost>,
    /// Per-model package and URL, as OpenCode Zen uses for its Claude,
    /// GPT, and Gemini models.
    provider: Option<ModelEndpoint>,
}

#[derive(Deserialize)]
struct Modalities {
    #[serde(default)]
    output: Vec<String>,
}

#[derive(Deserialize)]
struct Limit {
    context: Option<serde_json::Number>,
    output: Option<serde_json::Number>,
}

#[derive(Deserialize)]
struct Cost {
    input: Option<f64>,
    output: Option<f64>,
    cache_read: Option<f64>,
    cache_write: Option<f64>,
}

#[derive(Deserialize, Default)]
struct ModelEndpoint {
    npm: Option<String>,
    api: Option<String>,
    /// `completions` or `responses`, for the OpenAI package.
    shape: Option<String>,
}

/// Reads the text of `api.json`.
pub fn parse_catalog(text: &str) -> Result<Catalog, String> {
    let providers = serde_json::from_str(text)
        .map_err(|err| format!("the catalog is not a JSON object of providers: {err}"))?;
    Ok(Catalog { providers })
}

impl Catalog {
    pub fn provider_ids(&self) -> impl Iterator<Item = &str> {
        self.providers.keys().map(String::as_str)
    }
}

/// One socium provider made from a catalog provider.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Endpoint {
    pub name: String,
    pub wire: &'static str,
    pub base_url: String,
}

/// A registry file for one catalog provider.
#[derive(Debug, Clone, PartialEq)]
pub struct ProviderImport {
    pub provider_id: String,
    /// The text of `models.d/<provider_id>.toml`.
    pub toml: String,
    /// Where the API key is sent, one entry per wire.
    pub endpoints: Vec<Endpoint>,
    pub models: usize,
    /// One line per model left out, with the reason.
    pub skipped: Vec<String>,
}

/// One line of `socium models import` without provider IDs.
#[derive(Debug, Clone, PartialEq)]
pub struct ProviderSummary {
    pub id: String,
    pub catalog_models: usize,
    /// The number of models that map, or why none does.
    pub importable: Result<usize, String>,
}

pub fn summarize(catalog: &Catalog) -> Vec<ProviderSummary> {
    catalog
        .providers
        .iter()
        .map(|(id, value)| ProviderSummary {
            id: id.clone(),
            catalog_models: value
                .get("models")
                .and_then(serde_json::Value::as_object)
                .map_or(0, |models| models.len()),
            importable: import(catalog, id, "", "").map(|import| import.models),
        })
        .collect()
}

/// A mapped model, before it is written out.
struct Mapped {
    key: (&'static str, String),
    alias: String,
    model: CatalogModel,
    features: &'static [&'static str],
}

/// Maps the catalog provider `provider_id` to a registry file. `source`
/// and `date` go into the header comment. Fails when the provider is not
/// in the catalog or none of its models can be imported.
pub fn import(
    catalog: &Catalog,
    provider_id: &str,
    source: &str,
    date: &str,
) -> Result<ProviderImport, String> {
    let value = catalog
        .providers
        .get(provider_id)
        .ok_or_else(|| format!("'{provider_id}' is not a models.dev provider"))?;
    if !is_file_stem(provider_id) {
        return Err(format!(
            "provider id '{provider_id}' cannot name a file (letters, digits, '.', '-' and '_' only)"
        ));
    }
    if OAUTH_ONLY.contains(&provider_id) {
        return Err(format!(
            "{provider_id} signs in with OAuth; socium needs an API key"
        ));
    }
    let provider: CatalogProvider = CatalogProvider::deserialize(value)
        .map_err(|err| format!("cannot read the catalog entry: {err}"))?;
    let api_key_env = provider
        .env
        .first()
        .filter(|name| is_env_name(name))
        .ok_or("the catalog names no API key variable")?
        .clone();
    let own = endpoint(&provider, &ModelEndpoint::default()).ok();

    let mut skipped = Vec::new();
    let mut mapped = Vec::new();
    for (key, value) in &provider.models {
        let alias = format!("{provider_id}/{key}");
        let model = match CatalogModel::deserialize(value) {
            Ok(model) => model,
            Err(err) => {
                skipped.push(format!("{alias}: cannot read the catalog entry: {err}"));
                continue;
            }
        };
        let alias = format!("{provider_id}/{}", model.id);
        if mapped.iter().any(|item: &Mapped| item.alias == alias) {
            skipped.push(format!("{alias}: listed twice"));
            continue;
        }
        match map_model(&provider, &model) {
            Ok((key, features)) => mapped.push(Mapped {
                key,
                alias,
                model,
                features,
            }),
            Err(reason) => skipped.push(format!("{alias}: {reason}")),
        }
    }
    if mapped.is_empty() {
        let mut reasons = skipped
            .iter()
            .filter_map(|line| line.split_once(": ").map(|(_, reason)| reason))
            .collect::<Vec<_>>();
        reasons.sort_unstable();
        reasons.dedup();
        if reasons.is_empty() {
            return Err("the catalog lists no models".into());
        }
        return Err(format!("no model can be imported ({})", reasons.join("; ")));
    }

    // One socium provider per (wire, base_url): the catalog provider's own
    // package keeps its ID, the others get the wire as a suffix.
    let mut names = BTreeMap::<(&'static str, String), String>::new();
    for item in &mapped {
        if names.contains_key(&item.key) {
            continue;
        }
        let name = if own.as_ref() == Some(&item.key) {
            provider_id.to_string()
        } else {
            let base = format!("{provider_id}-{}", item.key.0);
            let mut name = base.clone();
            let mut n = 2;
            while names.values().any(|taken| *taken == name) || name == provider_id {
                name = format!("{base}-{n}");
                n += 1;
            }
            name
        };
        names.insert(item.key.clone(), name);
    }
    let mut endpoints = names
        .iter()
        .map(|((wire, base_url), name)| Endpoint {
            name: name.clone(),
            wire,
            base_url: base_url.clone(),
        })
        .collect::<Vec<_>>();
    endpoints.sort_by(|a, b| a.name.cmp(&b.name));

    let mut text = String::new();
    let _ = writeln!(
        text,
        "# Generated by `socium models import {provider_id}` from {source} on {date}."
    );
    text.push_str(
        "# Review each base_url: the key in api_key_env is sent there.\n\
         # Rerunning the command overwrites this file; put changes in models.toml.\n",
    );
    for endpoint in &endpoints {
        let _ = write!(
            text,
            "\n[providers.{}]\nwire = {}\nbase_url = {}\napi_key_env = {}\n",
            key(&endpoint.name),
            string(endpoint.wire),
            string(&endpoint.base_url),
            string(&api_key_env),
        );
        if let Some((_, header)) = SESSION_HEADERS.iter().find(|(id, _)| *id == provider_id) {
            let _ = writeln!(text, "session_header = {}", string(header));
        }
    }
    for item in &mapped {
        write_model(&mut text, item, &names[&item.key]);
    }
    Ok(ProviderImport {
        provider_id: provider_id.to_string(),
        toml: text,
        endpoints,
        models: mapped.len(),
        skipped,
    })
}

/// The socium endpoint and features of a model, or why it is skipped.
fn map_model(
    provider: &CatalogProvider,
    model: &CatalogModel,
) -> Result<((&'static str, String), &'static [&'static str]), String> {
    if model.status.as_deref() == Some("deprecated") {
        return Err("deprecated".into());
    }
    if let Some(kind) = model.kind.as_deref().filter(|kind| *kind != "chat") {
        return Err(format!("not a chat model ({kind})"));
    }
    if model
        .modalities
        .as_ref()
        .is_some_and(|modalities| !modalities.output.iter().any(|m| m == "text"))
    {
        return Err("does not answer in text".into());
    }
    if model.id.trim().is_empty()
        || model
            .id
            .chars()
            .any(|c| c.is_whitespace() || c.is_control())
    {
        return Err("the model id is empty or has whitespace".into());
    }
    let key = endpoint(
        provider,
        model.provider.as_ref().unwrap_or(&ModelEndpoint::default()),
    )?;
    let features: &'static [&'static str] =
        if key.0 == ANTHROPIC && key.1 == ANTHROPIC_BASE_URL && model.id.starts_with("claude-") {
            CLAUDE_FEATURES
        } else if model.reasoning {
            &["effort"]
        } else {
            &[]
        };
    Ok((key, features))
}

/// The (wire, base_url) a model is reached at. A model's own package and
/// URL win over the provider's, and the package's default URL comes last,
/// as in OpenCode.
fn endpoint(
    provider: &CatalogProvider,
    model: &ModelEndpoint,
) -> Result<(&'static str, String), String> {
    let npm = model
        .npm
        .as_deref()
        .or(provider.npm.as_deref())
        .ok_or("the catalog names no SDK package")?;
    let package = PACKAGES
        .iter()
        .find(|package| package.npm == npm)
        .ok_or_else(|| format!("package {npm} has no socium wire"))?;
    let wire = match (package.wire, model.shape.as_deref()) {
        (OPENAI_RESPONSES, Some("completions")) => OPENAI_CHAT,
        (wire, _) => wire,
    };
    let url = model
        .api
        .as_deref()
        .or(provider.api.as_deref())
        .or(package.default_url)
        .ok_or_else(|| format!("no base URL for package {npm}"))?;
    if url.contains("${") {
        return Err(format!("base URL {url} needs values filled in"));
    }
    let mut url = url.trim_end_matches('/');
    if let Some(suffix) = package.strip_suffix
        && Some(url) != package.default_url
    {
        url = url
            .strip_suffix(suffix)
            .ok_or_else(|| format!("{wire} base URL {url} does not end in {suffix}"))?;
    }
    let (url, _) = parse_base_url(url)?;
    Ok((wire, url))
}

fn write_model(text: &mut String, item: &Mapped, provider: &str) {
    let model = &item.model;
    let _ = write!(
        text,
        "\n[[models]]\nalias = {}\nprovider = {}\nid = {}\n",
        string(&item.alias),
        string(provider),
        string(&model.id),
    );
    if let Some(name) = model
        .name
        .as_deref()
        .map(str::trim)
        .filter(|name| !name.is_empty() && !name.contains(char::is_control))
    {
        let _ = writeln!(text, "description = {}", string(name));
    }
    if !item.features.is_empty() {
        let list = item
            .features
            .iter()
            .map(|feature| string(feature))
            .collect::<Vec<_>>()
            .join(", ");
        let _ = writeln!(text, "features = [{list}]");
    }
    let tokens = |number: Option<&serde_json::Number>| {
        number
            .and_then(serde_json::Number::as_u64)
            .and_then(|n| u32::try_from(n).ok())
            .filter(|n| *n > 0)
    };
    let limit = model.limit.as_ref();
    let context = tokens(limit.and_then(|limit| limit.context.as_ref()));
    let output = tokens(limit.and_then(|limit| limit.output.as_ref()))
        .filter(|output| context.is_none_or(|context| *output < context));
    if let Some(context) = context {
        let _ = writeln!(text, "context_window = {context}");
    }
    if let Some(output) = output {
        let _ = writeln!(text, "max_output_tokens = {output}");
    }
    if let Some(cost) = &model.cost
        && let (Some(input), Some(output)) = (price(cost.input), price(cost.output))
    {
        let mut fields = vec![format!("input = {input:?}"), format!("output = {output:?}")];
        if let Some(read) = price(cost.cache_read) {
            fields.push(format!("cache_read = {read:?}"));
        }
        if let Some(write) = price(cost.cache_write) {
            fields.push(format!("cache_write = {write:?}"));
        }
        let _ = writeln!(text, "price = {{ {} }}", fields.join(", "));
    }
    if model.tool_call == Some(false) {
        text.push_str("tools = false\n");
    }
}

fn price(value: Option<f64>) -> Option<f64> {
    value.filter(|value| value.is_finite() && *value >= 0.0)
}

fn is_bare_key(text: &str) -> bool {
    !text.is_empty()
        && text
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_')
}

/// Safe as `<text>.toml` inside `models.d/`: no separator, and not hidden.
fn is_file_stem(text: &str) -> bool {
    !text.starts_with('.')
        && text
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '-' | '_'))
        && !text.is_empty()
}

fn is_env_name(text: &str) -> bool {
    !text.is_empty() && text.chars().all(|c| c.is_ascii_alphanumeric() || c == '_')
}

fn key(text: &str) -> String {
    if is_bare_key(text) {
        text.to_string()
    } else {
        string(text)
    }
}

/// A TOML basic string.
fn string(text: &str) -> String {
    let mut out = String::with_capacity(text.len() + 2);
    out.push('"');
    for c in text.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            c if c.is_control() => {
                let _ = write!(out, "\\u{:04X}", c as u32);
            }
            c => out.push(c),
        }
    }
    out.push('"');
    out
}

/// `models.d/` beside the models file socium reads, from the environment.
pub fn models_dir() -> Option<PathBuf> {
    crate::models::models_dir(&crate::host::process_env)
}

/// Writes `<dir>/<provider_id>.toml` in place of any earlier file,
/// creating the directory, and returns its path.
pub fn write(dir: &Path, import: &ProviderImport) -> io::Result<PathBuf> {
    fs::create_dir_all(dir)?;
    let path = dir.join(format!("{}.toml", import.provider_id));
    // A file left half-written would stop socium from starting.
    let partial = dir.join(format!(".{}.toml.partial", import.provider_id));
    fs::write(&partial, &import.toml)?;
    fs::rename(&partial, &path)?;
    Ok(path)
}

/// Today's date in UTC as `YYYY-MM-DD`.
pub fn utc_date(time: SystemTime) -> String {
    crate::session::utc_date(time)
}

/// Downloads the catalog from models.dev through `client`, which
/// [`crate::fetch_models_catalog`] makes as socium's provider client, so it
/// follows no redirects.
pub(crate) async fn fetch_catalog(client: &reqwest::Client) -> Result<String, String> {
    let mut response = client
        .get(CATALOG_URL)
        .send()
        .await
        .map_err(|err| format!("cannot reach {CATALOG_URL}: {err}"))?;
    if !response.status().is_success() {
        return Err(format!("{CATALOG_URL} answered {}", response.status()));
    }
    let mut body = Vec::new();
    while let Some(chunk) = response
        .chunk()
        .await
        .map_err(|err| format!("reading {CATALOG_URL}: {err}"))?
    {
        body.extend_from_slice(&chunk);
        if body.len() > CATALOG_MAX_BYTES {
            return Err(format!(
                "{CATALOG_URL} sent more than {} MiB",
                CATALOG_MAX_BYTES >> 20
            ));
        }
    }
    String::from_utf8(body).map_err(|_| format!("{CATALOG_URL} did not send UTF-8 text"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::models::{ModelFeatures, ModelRegistry, Wire};

    /// A trimmed copy of models.dev's api.json from 2026-11-20.
    const FIXTURE: &str = include_str!("../tests/fixtures/models-dev-api.json");

    fn catalog() -> Catalog {
        parse_catalog(FIXTURE).unwrap()
    }

    fn imported(id: &str) -> ProviderImport {
        import(&catalog(), id, CATALOG_URL, "2026-11-20").unwrap()
    }

    fn endpoint(name: &str, wire: &'static str, base_url: &str) -> Endpoint {
        Endpoint {
            name: name.into(),
            wire,
            base_url: base_url.into(),
        }
    }

    /// The `[[models]]` block of `alias` in generated text.
    fn model_block<'a>(text: &'a str, alias: &str) -> &'a str {
        let start = text
            .find(&format!("alias = \"{alias}\""))
            .unwrap_or_else(|| panic!("{alias} missing from\n{text}"));
        let rest = &text[start..];
        &rest[..rest.find("\n\n").unwrap_or(rest.len())]
    }

    #[test]
    fn a_provider_with_several_packages_becomes_one_provider_per_wire_sharing_its_key() {
        let opencode = imported("opencode");
        assert_eq!(
            opencode.endpoints,
            [
                endpoint(
                    "opencode",
                    OPENAI_CHAT,
                    "https://opencode.ai/inference/openai/v1"
                ),
                endpoint(
                    "opencode-anthropic",
                    ANTHROPIC,
                    "https://opencode.ai/inference/anthropic"
                ),
                endpoint(
                    "opencode-google",
                    GOOGLE,
                    "https://opencode.ai/inference/google/v1beta"
                ),
                endpoint(
                    "opencode-openai-responses",
                    OPENAI_RESPONSES,
                    "https://opencode.ai/inference/openai/v1"
                ),
            ]
        );
        assert_eq!(
            opencode
                .toml
                .matches("api_key_env = \"OPENCODE_API_KEY\"")
                .count(),
            4
        );
        assert!(opencode.toml.contains(
            "[providers.opencode-google]\nwire = \"google\"\nbase_url = \"https://opencode.ai/inference/google/v1beta\"\n"
        ));
        for (alias, provider) in [
            ("opencode/big-pickle", "opencode"),
            // @ai-sdk/mistral at the provider's own URL speaks the same wire.
            ("opencode/mistral-large-4", "opencode"),
            ("opencode/claude-haiku-5-5", "opencode-anthropic"),
            ("opencode/gemini-3.5-flash-lite", "opencode-google"),
            ("opencode/gpt-5-nano", "opencode-openai-responses"),
        ] {
            assert!(
                model_block(&opencode.toml, alias).contains(&format!("provider = \"{provider}\"")),
                "{alias}"
            );
        }
        // A Claude model off Anthropic's own API gets only effort.
        assert!(
            model_block(&opencode.toml, "opencode/claude-haiku-5-5")
                .contains("features = [\"effort\"]\n")
        );
        assert_eq!(opencode.models, 5);
        assert_eq!(opencode.skipped, ["opencode/kimi-k2: deprecated"]);
    }

    #[test]
    fn first_party_claude_models_get_every_anthropic_feature() {
        let anthropic = imported("anthropic");
        assert_eq!(
            anthropic.skipped,
            ["anthropic/claude-sonnet-4-5: deprecated"]
        );
        let registry = ModelRegistry::from_toml(&anthropic.toml).unwrap();
        let opus = registry.find("anthropic/claude-opus-5-5").unwrap();
        assert_eq!(opus.id, "claude-opus-5-5");
        assert_eq!(opus.provider, "anthropic");
        assert_eq!(opus.features, ModelFeatures::ALL);
        assert_eq!(opus.description.as_deref(), Some("Claude Opus 5.5"));
        assert_eq!(
            (opus.context_window, opus.max_output_tokens),
            (Some(1_000_000), Some(128_000))
        );
        let price = opus.price.unwrap();
        assert_eq!(
            (
                price.input,
                price.output,
                price.cache_read,
                price.cache_write
            ),
            (4.0, 20.0, Some(0.2), Some(5.0))
        );
        let provider = registry.provider("anthropic").unwrap();
        assert_eq!(provider.base_url, ANTHROPIC_BASE_URL);
        assert_eq!(provider.api_key_env.as_deref(), Some("ANTHROPIC_API_KEY"));
    }

    #[test]
    fn an_anthropic_compatible_url_loses_the_v1_the_wire_adds() {
        let minimax = imported("minimax");
        let registry = ModelRegistry::from_toml(&minimax.toml).unwrap();
        let provider = registry.provider("minimax").unwrap();
        assert_eq!(provider.wire, Wire::Anthropic);
        assert_eq!(provider.base_url, "https://api.minimax.io/anthropic");
        let model = registry.find("minimax/MiniMax-M3").unwrap();
        assert!(model.features.effort && !model.features.adaptive_thinking);
    }

    #[test]
    fn models_that_cannot_chat_or_take_tools_are_marked_or_skipped() {
        let groq = imported("groq");
        assert_eq!(
            groq.skipped,
            [
                "groq/canopylabs/orpheus-v1-english: not a chat model (speech)",
                "groq/llama-3.1-8b-instant: deprecated",
                "groq/llama-3.3-70b-versatile: deprecated",
                "groq/whisper-large-v3: not a chat model (transcription)",
            ]
        );
        let registry = ModelRegistry::from_toml(&groq.toml).unwrap();
        assert_eq!(
            registry.provider("groq").unwrap().base_url,
            "https://api.groq.com/openai/v1"
        );
        let allam = registry.find("groq/allam-2-7b").unwrap();
        assert!(!allam.tools);
        assert_eq!(allam.features, ModelFeatures::default());
        // The catalog's output limit equals the window, which socium refuses.
        assert_eq!(
            (allam.context_window, allam.max_output_tokens),
            (Some(4096), None)
        );
    }

    #[test]
    fn providers_socium_cannot_reach_are_refused_with_the_reason() {
        let catalog = catalog();
        for (id, reason) in [
            (
                "amazon-bedrock",
                "package @ai-sdk/amazon-bedrock has no socium wire",
            ),
            ("github-copilot", "signs in with OAuth"),
            ("azure", "needs values filled in"),
            ("nowhere", "'nowhere' is not a models.dev provider"),
        ] {
            let err = import(&catalog, id, CATALOG_URL, "2026-11-20").unwrap_err();
            assert!(err.contains(reason), "{id}: {err}");
        }
        let summary = summarize(&catalog);
        assert_eq!(summary.len(), 9);
        let line = |id: &str| summary.iter().find(|line| line.id == id).unwrap();
        assert_eq!(line("opencode").importable, Ok(5));
        assert_eq!(line("opencode").catalog_models, 6);
        assert_eq!(line("opencode-go").importable, Ok(4));
        assert_eq!(line("opencode-go").catalog_models, 5);
        assert!(line("azure").importable.is_err());
    }

    #[test]
    fn imported_files_layer_into_the_registry() {
        let dir = tempfile::tempdir().unwrap();
        let models_dir = dir.path().join("socium/models.d");
        for id in ["anthropic", "groq", "minimax"] {
            write(&models_dir, &imported(id)).unwrap();
        }
        let home = dir.path().to_str().unwrap().to_string();
        let registry =
            ModelRegistry::load(&|key: &str| (key == "XDG_CONFIG_HOME").then(|| home.clone()))
                .unwrap();
        for alias in [
            "opus",
            "anthropic/claude-haiku-4-5",
            "groq/allam-2-7b",
            "minimax/MiniMax-M3",
        ] {
            assert!(registry.find(alias).is_some(), "{alias}");
        }
    }

    #[test]
    fn opencode_go_providers_send_the_session_header_and_zen_ones_do_not() {
        let go = imported("opencode-go");
        let registry = ModelRegistry::from_toml(&go.toml).unwrap();
        let mut wires = registry
            .providers
            .values()
            .filter(|provider| provider.name.starts_with("opencode-go"))
            .map(|provider| {
                assert_eq!(
                    provider.session_header.as_ref().map(|name| name.as_str()),
                    Some("x-opencode-session"),
                    "{}",
                    provider.name
                );
                assert_eq!(provider.api_key_env.as_deref(), Some("OPENCODE_API_KEY"));
                provider.wire.name()
            })
            .collect::<Vec<_>>();
        wires.sort_unstable();
        assert_eq!(
            wires,
            ["anthropic", "google", "openai-chat", "openai-responses"]
        );
        assert_eq!(go.models, 4);
        assert!(
            go.skipped
                .iter()
                .any(|reason| reason == "opencode-go/glm-5.2: deprecated")
        );
        let haiku = registry.find("opencode-go/claude-haiku-5-5").unwrap();
        assert_eq!(
            registry.provider(&haiku.provider).unwrap().base_url,
            "https://opencode.ai/inference/go/anthropic"
        );

        let zen = ModelRegistry::from_toml(&imported("opencode").toml).unwrap();
        assert!(
            zen.providers
                .values()
                .all(|provider| provider.session_header.is_none())
        );
    }

    #[test]
    fn gemini_models_import_onto_the_google_wire() {
        let opencode = ModelRegistry::from_toml(&imported("opencode").toml).unwrap();
        let zen = opencode.provider("opencode-google").unwrap();
        assert_eq!(zen.wire, Wire::Google);
        assert_eq!(zen.api_key_env.as_deref(), Some("OPENCODE_API_KEY"));
        let flash = opencode.find("opencode/gemini-3.5-flash-lite").unwrap();
        assert_eq!(flash.provider, "opencode-google");

        let google = ModelRegistry::from_toml(&imported("google").toml).unwrap();
        let provider = google.provider("google").unwrap();
        assert_eq!(provider.wire, Wire::Google);
        assert_eq!(
            provider.base_url,
            "https://generativelanguage.googleapis.com/v1beta"
        );
        assert!(google.models.iter().any(|model| model.provider == "google"));
    }

    #[test]
    fn rewriting_an_import_converges_to_the_same_file() {
        let dir = tempfile::tempdir().unwrap();
        let first = imported("opencode");
        let path = write(dir.path(), &first).unwrap();
        assert_eq!(path, dir.path().join("opencode.toml"));
        fs::write(&path, "edited by hand").unwrap();
        write(dir.path(), &imported("opencode")).unwrap();
        assert_eq!(fs::read_to_string(&path).unwrap(), first.toml);
        assert_eq!(fs::read_dir(dir.path()).unwrap().count(), 1);
        assert!(first.toml.starts_with(
            "# Generated by `socium models import opencode` from https://models.dev/api.json on 2026-11-20.\n"
        ));
    }

    #[test]
    fn a_model_endpoint_falls_back_to_the_provider_then_the_package() {
        let catalog = parse_catalog(
            r#"{"p": {"npm": "@ai-sdk/openai-compatible", "api": "https://p.example/v1", "env": ["P_KEY"], "models": {
                "chat": {"id": "chat", "name": "Say \"hi\" \\ there", "provider": {"npm": "@ai-sdk/openai", "shape": "completions"}},
                "resp": {"id": "resp", "provider": {"npm": "@ai-sdk/openai"}},
                "mistral": {"id": "mistral", "provider": {"npm": "@ai-sdk/mistral", "api": null}},
                "plain": {"id": "plain", "provider": {"npm": "@ai-sdk/anthropic", "api": "https://p.example/anthropic"}}
            }},
            "q": {"npm": "@ai-sdk/xai", "env": ["XAI_API_KEY"], "models": {"grok": {"id": "grok"}}}}"#,
        )
        .unwrap();
        let q = import(&catalog, "q", "saved.json", "2026-11-20").unwrap();
        assert_eq!(
            q.endpoints,
            [endpoint("q", OPENAI_CHAT, "https://api.x.ai/v1")]
        );
        let p = import(&catalog, "p", "saved.json", "2026-11-20").unwrap();
        assert_eq!(
            p.endpoints,
            [
                endpoint("p", OPENAI_CHAT, "https://p.example/v1"),
                endpoint(
                    "p-openai-responses",
                    OPENAI_RESPONSES,
                    "https://p.example/v1"
                ),
            ]
        );
        assert_eq!(
            p.skipped,
            ["p/plain: anthropic base URL https://p.example/anthropic does not end in /v1"]
        );
        let registry = ModelRegistry::from_toml(&p.toml).unwrap();
        let chat = registry.find("p/chat").unwrap();
        assert_eq!(chat.description.as_deref(), Some("Say \"hi\" \\ there"));
        assert_eq!(chat.provider, "p");
        assert_eq!(registry.find("p/mistral").unwrap().provider, "p");
    }

    #[test]
    fn a_provider_id_names_a_file_inside_models_d_only() {
        let catalog = parse_catalog(
            r#"{"w.ai": {"npm": "@ai-sdk/xai", "env": ["W_KEY"], "models": {"m": {"id": "m"}}},
                "../up": {"npm": "@ai-sdk/xai", "env": ["W_KEY"], "models": {"m": {"id": "m"}}},
                ".hidden": {"npm": "@ai-sdk/xai", "env": ["W_KEY"], "models": {"m": {"id": "m"}}}}"#,
        )
        .unwrap();
        let w = import(&catalog, "w.ai", "saved.json", "2026-11-20").unwrap();
        assert!(w.toml.contains("[providers.\"w.ai\"]\n"), "{}", w.toml);
        let registry = ModelRegistry::from_toml(&w.toml).unwrap();
        assert_eq!(registry.find("w.ai/m").unwrap().provider, "w.ai");
        for id in ["../up", ".hidden"] {
            let err = import(&catalog, id, "saved.json", "2026-11-20").unwrap_err();
            assert!(err.contains("cannot name a file"), "{id}: {err}");
        }
    }

    #[test]
    fn utc_date_is_the_calendar_day() {
        let time = SystemTime::UNIX_EPOCH + std::time::Duration::from_secs(1_795_132_800);
        assert_eq!(utc_date(time), "2026-11-20");
    }
}
