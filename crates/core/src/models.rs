//! Model registry: the models `/model` can select and how to reach them.
//! Built-in Anthropic entries are overlaid with the user's `models.toml`
//! (ADR 6). API keys never appear here, only the names of the environment
//! variables that hold them.

use serde::Deserialize;
use std::collections::BTreeMap;
use std::fs;
use std::io::{self, Error, ErrorKind};
use std::path::{Path, PathBuf};

pub(crate) const MODELS_FILE_ENV: &str = "SOCIUM_MODELS_FILE";
pub(crate) const BUILTIN_PROVIDER: &str = "anthropic";
const BUILTIN_DEFAULT_MODEL: &str = "opus";
pub(crate) const ANTHROPIC_BASE_URL: &str = "https://api.anthropic.com";

/// Request format spoken to a provider endpoint.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Wire {
    Anthropic,
    /// OpenAI Chat Completions, also served by Ollama, OpenRouter, and
    /// other OpenAI-compatible endpoints.
    OpenAiChat,
    /// OpenAI Responses, which keeps reasoning across tool calls.
    OpenAiResponses,
    /// Google's Gemini API (`streamGenerateContent`), also served by
    /// OpenCode Zen's Gemini endpoint.
    Google,
}

impl Wire {
    pub(crate) fn name(self) -> &'static str {
        match self {
            Self::Anthropic => "anthropic",
            Self::OpenAiChat => "openai-chat",
            Self::OpenAiResponses => "openai-responses",
            Self::Google => "google",
        }
    }

    fn parse(raw: &str) -> Result<Self, String> {
        match raw {
            "anthropic" => Ok(Self::Anthropic),
            "openai-chat" => Ok(Self::OpenAiChat),
            "openai-responses" => Ok(Self::OpenAiResponses),
            "google" => Ok(Self::Google),
            other => Err(format!(
                "unknown wire '{other}'; expected anthropic, openai-chat, openai-responses, or google"
            )),
        }
    }
}

/// How the API key is sent.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum AuthScheme {
    /// `x-api-key: <key>`
    ApiKeyHeader,
    /// `Authorization: Bearer <key>`
    Bearer,
    /// `x-goog-api-key: <key>`, how the Gemini API takes its keys.
    GoogApiKey,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ProviderConfig {
    pub(crate) name: String,
    pub(crate) wire: Wire,
    /// Normalized without a trailing slash.
    pub(crate) base_url: String,
    /// The variable that holds the key; `None` for an endpoint on this
    /// machine that takes none, which then gets no key header.
    pub(crate) api_key_env: Option<String>,
    pub(crate) auth: AuthScheme,
}

impl ProviderConfig {
    /// The key requests carry: `None` for a provider without one, an error
    /// naming the variable when it is not set.
    pub(crate) fn api_key(
        &self,
        lookup: &dyn Fn(&str) -> Option<String>,
    ) -> Result<Option<String>, String> {
        let Some(variable) = &self.api_key_env else {
            return Ok(None);
        };
        lookup(variable)
            .filter(|key| !key.is_empty())
            .map(Some)
            .ok_or_else(|| format!("{variable} is not set"))
    }
}

/// Anthropic extensions sent only to models that list them, so a
/// compatible endpoint never receives fields it would reject or ignore.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub(crate) struct ModelFeatures {
    pub(crate) adaptive_thinking: bool,
    pub(crate) effort: bool,
    pub(crate) eager_input_streaming: bool,
    pub(crate) anthropic_text_editor: bool,
    /// Top-level `cache_control`: the API caches the request up to its last
    /// block, so each round re-reads the shared history at the cache price.
    pub(crate) prompt_caching: bool,
}

impl ModelFeatures {
    pub(crate) const ALL: Self = Self {
        adaptive_thinking: true,
        effort: true,
        eager_input_streaming: true,
        anthropic_text_editor: true,
        prompt_caching: true,
    };

    fn parse(names: &[String]) -> Result<Self, String> {
        let mut features = Self::default();
        for name in names {
            let flag = match name.as_str() {
                "adaptive_thinking" => &mut features.adaptive_thinking,
                "effort" => &mut features.effort,
                "eager_input_streaming" => &mut features.eager_input_streaming,
                "anthropic_text_editor" => &mut features.anthropic_text_editor,
                "prompt_caching" => &mut features.prompt_caching,
                other => {
                    return Err(format!(
                        "unknown feature '{other}'; expected adaptive_thinking, effort, eager_input_streaming, anthropic_text_editor, or prompt_caching"
                    ));
                }
            };
            *flag = true;
        }
        Ok(features)
    }
}

#[derive(Debug, Clone, PartialEq)]
pub(crate) struct ModelConfig {
    pub(crate) alias: String,
    pub(crate) provider: String,
    pub(crate) id: String,
    /// One line shown in the `/model` picker.
    pub(crate) description: Option<String>,
    pub(crate) features: ModelFeatures,
    pub(crate) context_window: Option<u32>,
    pub(crate) max_output_tokens: Option<u32>,
    /// Alias of the model that summarizes history; `None` means this model.
    pub(crate) compact_model: Option<String>,
    /// Prompt size at which history is compacted; `None` uses the default.
    pub(crate) compact_at_tokens: Option<u32>,
    /// The user's prices, for the session cost; `None` shows no cost.
    pub(crate) price: Option<crate::usage::Price>,
    /// Whether the model takes tool definitions. `tools = false` in
    /// models.toml for one that rejects them: it is sent none, answers in
    /// one round, and cannot read, edit, or run anything.
    pub(crate) tools: bool,
}

impl ModelConfig {
    /// A model named only by the `MODEL` environment variable: a Claude
    /// model ID on the built-in provider, as before the registry existed.
    pub(crate) fn ad_hoc(id: &str) -> Self {
        Self {
            alias: id.to_string(),
            provider: BUILTIN_PROVIDER.to_string(),
            id: id.to_string(),
            description: None,
            features: ModelFeatures::ALL,
            context_window: None,
            max_output_tokens: None,
            compact_model: Some("haiku".to_string()),
            compact_at_tokens: None,
            price: None,
            tools: true,
        }
    }

    /// Alias of another model that summarizes this one's history; `None`
    /// means this model does.
    pub(crate) fn compaction_alias(&self) -> Option<&str> {
        self.compact_model
            .as_deref()
            .filter(|alias| *alias != self.alias)
    }
}

#[derive(Debug, Clone, PartialEq)]
pub(crate) struct ModelRegistry {
    pub(crate) default: String,
    pub(crate) providers: BTreeMap<String, ProviderConfig>,
    pub(crate) models: Vec<ModelConfig>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RawFile {
    default: Option<String>,
    #[serde(default)]
    providers: BTreeMap<String, RawProvider>,
    #[serde(default)]
    models: Vec<RawModel>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RawProvider {
    wire: String,
    base_url: String,
    #[serde(default)]
    api_key_env: Option<String>,
    auth: Option<String>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RawModel {
    alias: String,
    provider: String,
    id: String,
    description: Option<String>,
    #[serde(default)]
    features: Vec<String>,
    context_window: Option<u32>,
    max_output_tokens: Option<u32>,
    compact_model: Option<String>,
    compact_at_tokens: Option<u32>,
    price: Option<crate::usage::Price>,
    #[serde(default = "takes_tools")]
    tools: bool,
}

fn takes_tools() -> bool {
    true
}

/// The window of the built-in Claude models, from Anthropic's model table
/// (https://platform.claude.com/docs/en/about-claude/models/overview).
const BUILTIN_CONTEXT_WINDOW: u32 = 1_000_000;

impl ModelRegistry {
    pub(crate) fn builtin() -> Self {
        let provider = ProviderConfig {
            name: BUILTIN_PROVIDER.to_string(),
            wire: Wire::Anthropic,
            base_url: ANTHROPIC_BASE_URL.to_string(),
            api_key_env: Some("ANTHROPIC_API_KEY".to_string()),
            auth: AuthScheme::ApiKeyHeader,
        };
        let claude =
            |alias: &str, id: &str, description: &str, compact_model: Option<&str>| ModelConfig {
                alias: alias.to_string(),
                provider: BUILTIN_PROVIDER.to_string(),
                id: id.to_string(),
                description: Some(description.to_string()),
                features: ModelFeatures::ALL,
                context_window: Some(BUILTIN_CONTEXT_WINDOW),
                max_output_tokens: None,
                compact_model: compact_model.map(str::to_string),
                compact_at_tokens: None,
                price: None,
                tools: true,
            };
        Self {
            default: BUILTIN_DEFAULT_MODEL.to_string(),
            providers: BTreeMap::from([(BUILTIN_PROVIDER.to_string(), provider)]),
            models: vec![
                claude(
                    "opus",
                    "claude-opus-5-5",
                    "Opus 5.5 for complex work",
                    Some("haiku"),
                ),
                claude(
                    "sonnet",
                    "claude-sonnet-5-5",
                    "Sonnet 5.5 for everyday tasks",
                    Some("haiku"),
                ),
                claude(
                    "haiku",
                    "claude-haiku-5-5",
                    "Haiku 5.5 for quick answers",
                    None,
                ),
            ],
        }
    }

    /// Reads the registry: the built-in entries, then every `*.toml` in
    /// `models.d/` beside the models file in file-name order, then the
    /// models file named by `SOCIUM_MODELS_FILE` or the XDG default. A
    /// missing default file or directory adds nothing; a missing
    /// explicitly named file is an error.
    pub(crate) fn load(lookup: &dyn Fn(&str) -> Option<String>) -> io::Result<Self> {
        let Some((path, explicit)) = models_file(lookup) else {
            return Ok(Self::builtin());
        };
        let mut layers = read_models_dir(&models_dir_beside(&path))?;
        match fs::read_to_string(&path) {
            Ok(text) => layers.push(Layer {
                source: Some(path.display().to_string()),
                text,
                kind: LayerKind::ModelsFile,
            }),
            Err(err) if err.kind() == ErrorKind::NotFound && !explicit => {}
            Err(err) => {
                return Err(Error::new(
                    err.kind(),
                    format!("Cannot read {}: {err}", path.display()),
                ));
            }
        }
        Self::from_layers(&layers).map_err(|message| Error::new(ErrorKind::InvalidInput, message))
    }

    /// Overlays a `models.toml` on the built-in registry. Entries with a
    /// built-in name replace it in place.
    #[cfg(test)]
    pub(crate) fn from_toml(text: &str) -> Result<Self, String> {
        Self::from_layers(&[Layer {
            source: None,
            text: text.to_string(),
            kind: LayerKind::ModelsFile,
        }])
    }

    /// Overlays each layer on the built-in registry in order: a later
    /// provider or alias replaces an earlier one of the same name.
    /// Validation runs once, on the merged result, and an error names the
    /// file the offending entry came from.
    fn from_layers(layers: &[Layer]) -> Result<Self, String> {
        let mut registry = Self::builtin();
        let mut origins = BTreeMap::<String, Option<&str>>::new();
        let mut dir_providers = BTreeMap::<String, &str>::new();
        let mut default_origin = None;
        for layer in layers {
            let source = layer.source.as_deref();
            let at = |message: String| match source {
                Some(source) => format!("{source}: {message}"),
                None => message,
            };
            let raw: RawFile = toml::from_str(&layer.text).map_err(|err| at(err.to_string()))?;
            if let Some(default) = raw.default {
                if layer.kind == LayerKind::ModelsDir {
                    return Err(at("default is only allowed in models.toml".into()));
                }
                registry.default = default;
                default_origin = source;
            }
            for (name, provider) in raw.providers {
                let config = parse_provider(&name, provider)
                    .map_err(|message| at(format!("provider '{name}': {message}")))?;
                if layer.kind == LayerKind::ModelsDir {
                    let this = source.unwrap_or_default();
                    if let Some(first) = dir_providers.insert(name.clone(), this) {
                        return Err(format!(
                            "provider '{name}' is defined in both {first} and {this}"
                        ));
                    }
                }
                registry.providers.insert(name, config);
            }
            let mut seen = Vec::new();
            for model in raw.models {
                let alias = model.alias.clone();
                if seen.contains(&alias) {
                    return Err(at(format!(
                        "model alias '{alias}' is defined more than once"
                    )));
                }
                let config = parse_model(model)
                    .map_err(|message| at(format!("model '{alias}': {message}")))?;
                match registry.models.iter_mut().find(|m| m.alias == alias) {
                    Some(existing) => *existing = config,
                    None => registry.models.push(config),
                }
                origins.insert(alias.clone(), source);
                seen.push(alias);
            }
        }
        let at = |source: Option<&str>, message: String| match source {
            Some(source) => format!("{source}: {message}"),
            None => message,
        };
        for model in &registry.models {
            registry
                .validate_model(model)
                .map_err(|message| at(origins.get(&model.alias).copied().flatten(), message))?;
        }
        if registry.find(&registry.default).is_none() {
            return Err(at(
                default_origin,
                format!("default '{}' is not a registered alias", registry.default),
            ));
        }
        Ok(registry)
    }

    fn validate_model(&self, model: &ModelConfig) -> Result<(), String> {
        let Some(provider) = self.providers.get(&model.provider) else {
            return Err(format!(
                "model '{}': unknown provider '{}'",
                model.alias, model.provider
            ));
        };
        let features = model.features;
        if provider.wire != Wire::Anthropic
            && (features.adaptive_thinking
                || features.eager_input_streaming
                || features.anthropic_text_editor)
        {
            return Err(format!(
                "model '{}': only the effort feature applies to the {} wire",
                model.alias,
                provider.wire.name()
            ));
        }
        if let Some(compact) = &model.compact_model
            && self.find(compact).is_none()
        {
            return Err(format!(
                "model '{}': compact_model '{compact}' is not a registered alias",
                model.alias
            ));
        }
        Ok(())
    }

    pub(crate) fn find(&self, alias: &str) -> Option<&ModelConfig> {
        self.models.iter().find(|model| model.alias == alias)
    }

    /// Resolves the `MODEL` environment variable: an alias, then a
    /// registered model ID, then an ad hoc Claude model ID.
    pub(crate) fn resolve_env_model(&self, value: &str) -> ModelConfig {
        self.find(value)
            .or_else(|| self.models.iter().find(|model| model.id == value))
            .cloned()
            .unwrap_or_else(|| ModelConfig::ad_hoc(value))
    }

    pub(crate) fn default_model(&self) -> &ModelConfig {
        self.find(&self.default)
            .expect("validated registry has its default model")
    }

    pub(crate) fn provider(&self, name: &str) -> Option<&ProviderConfig> {
        self.providers.get(name)
    }

    /// Whether `model` is the registered entry of the same alias, rather
    /// than an ad hoc model from `MODEL`.
    pub(crate) fn is_registered(&self, model: &ModelConfig) -> bool {
        self.find(&model.alias) == Some(model)
    }
}

/// Directory of extra registry files, read before `models.toml`.
pub(crate) const MODELS_DIR_NAME: &str = "models.d";

/// One registry file to overlay.
struct Layer {
    /// The path shown in errors; `None` for text given directly.
    source: Option<String>,
    text: String,
    kind: LayerKind,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum LayerKind {
    /// `models.toml`, the only file that may set `default`.
    ModelsFile,
    /// A file in `models.d/`.
    ModelsDir,
}

/// The models file and whether `SOCIUM_MODELS_FILE` named it.
fn models_file(lookup: &dyn Fn(&str) -> Option<String>) -> Option<(PathBuf, bool)> {
    if let Some(explicit) = lookup(MODELS_FILE_ENV).filter(|value| !value.is_empty()) {
        return Some((PathBuf::from(explicit), true));
    }
    Some((
        crate::settings::config_dir(lookup)?.join("models.toml"),
        false,
    ))
}

/// `models.d/` beside a models file.
pub(crate) fn models_dir_beside(models_file: &Path) -> PathBuf {
    models_file.with_file_name(MODELS_DIR_NAME)
}

/// `models.d/` beside the models file `load` reads.
pub(crate) fn models_dir(lookup: &dyn Fn(&str) -> Option<String>) -> Option<PathBuf> {
    models_file(lookup).map(|(path, _)| models_dir_beside(&path))
}

/// The `*.toml` files of `dir` in file-name order; none when it is missing.
fn read_models_dir(dir: &Path) -> io::Result<Vec<Layer>> {
    let cannot_read = |path: &Path, err: Error| {
        Error::new(err.kind(), format!("Cannot read {}: {err}", path.display()))
    };
    let entries = match fs::read_dir(dir) {
        Ok(entries) => entries,
        Err(err) if err.kind() == ErrorKind::NotFound => return Ok(Vec::new()),
        Err(err) => return Err(cannot_read(dir, err)),
    };
    let mut paths = Vec::new();
    for entry in entries {
        let path = entry.map_err(|err| cannot_read(dir, err))?.path();
        if path.extension().is_some_and(|ext| ext == "toml") && path.is_file() {
            paths.push(path);
        }
    }
    paths.sort();
    paths
        .into_iter()
        .map(|path| {
            let text = fs::read_to_string(&path).map_err(|err| cannot_read(&path, err))?;
            Ok(Layer {
                source: Some(path.display().to_string()),
                text,
                kind: LayerKind::ModelsDir,
            })
        })
        .collect()
}

fn parse_provider(name: &str, raw: RawProvider) -> Result<ProviderConfig, String> {
    if name.is_empty() {
        return Err("name must not be empty".into());
    }
    let wire = Wire::parse(&raw.wire)?;
    let (base_url, loopback) = parse_base_url(&raw.base_url)?;
    match &raw.api_key_env {
        // A local server such as Ollama or llama.cpp often takes no key; a
        // remote one left without it is far more likely a mistake.
        None if !loopback => {
            return Err(
                "api_key_env is required unless base_url is on this machine (localhost)".into(),
            );
        }
        None if raw.auth.is_some() => return Err("auth needs api_key_env".into()),
        None => {}
        Some(variable) => check_key_variable(variable)?,
    }
    let auth = match (wire, raw.auth.as_deref()) {
        (Wire::Anthropic, None | Some("x-api-key")) => AuthScheme::ApiKeyHeader,
        (Wire::Google, None | Some("x-goog-api-key")) => AuthScheme::GoogApiKey,
        (_, Some("bearer")) | (Wire::OpenAiChat | Wire::OpenAiResponses, None) => {
            AuthScheme::Bearer
        }
        (Wire::OpenAiChat | Wire::OpenAiResponses, Some("x-api-key" | "x-goog-api-key")) => {
            return Err(format!("auth must be bearer for the {} wire", wire.name()));
        }
        (Wire::Anthropic, Some("x-goog-api-key")) => {
            return Err("auth must be x-api-key or bearer for the anthropic wire".into());
        }
        (Wire::Google, Some("x-api-key")) => {
            return Err("auth must be x-goog-api-key or bearer for the google wire".into());
        }
        (_, Some(other)) => {
            return Err(format!(
                "auth must be x-api-key, x-goog-api-key, or bearer; got '{other}'"
            ));
        }
    };
    Ok(ProviderConfig {
        name: name.to_string(),
        wire,
        base_url,
        api_key_env: raw.api_key_env,
        auth,
    })
}

fn check_key_variable(variable: &str) -> Result<(), String> {
    if variable.is_empty()
        || !variable
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '_')
    {
        // Never echo the value: it is often the key itself, pasted by mistake.
        return Err(
            "api_key_env must be the name of the environment variable that holds the key (letters, digits, and _), not the key itself"
                .into(),
        );
    }
    Ok(())
}

/// The normalized URL and whether it is on this machine. Keys travel to
/// this URL, so plain HTTP is allowed only on loopback.
pub(crate) fn parse_base_url(raw: &str) -> Result<(String, bool), String> {
    let url = reqwest::Url::parse(raw).map_err(|err| format!("invalid base_url '{raw}': {err}"))?;
    let loopback = url.host_str().is_some_and(|host| {
        host == "localhost"
            || host
                .trim_start_matches('[')
                .trim_end_matches(']')
                .parse::<std::net::IpAddr>()
                .is_ok_and(|ip| ip.is_loopback())
    });
    match url.scheme() {
        "https" => {}
        "http" if loopback => {}
        _ => {
            return Err(format!(
                "base_url must use https (http only for localhost); got '{raw}'"
            ));
        }
    }
    if !url.username().is_empty() || url.password().is_some() {
        return Err("base_url must not contain credentials".into());
    }
    if url.query().is_some() || url.fragment().is_some() {
        return Err("base_url must not contain a query or fragment".into());
    }
    Ok((url.as_str().trim_end_matches('/').to_string(), loopback))
}

fn parse_model(raw: RawModel) -> Result<ModelConfig, String> {
    if raw.alias.is_empty() || raw.alias.chars().any(char::is_whitespace) {
        return Err("alias must be non-empty and contain no whitespace".into());
    }
    if raw.id.trim().is_empty() {
        return Err("id must not be empty".into());
    }
    if raw
        .description
        .as_deref()
        .is_some_and(|text| text.contains(char::is_control))
    {
        return Err("description must be a single line".into());
    }
    if raw.context_window == Some(0)
        || raw.max_output_tokens == Some(0)
        || raw.compact_at_tokens == Some(0)
    {
        return Err(
            "context_window, max_output_tokens, and compact_at_tokens must be positive".into(),
        );
    }
    if let (Some(window), Some(compact_at)) = (raw.context_window, raw.compact_at_tokens)
        && compact_at >= window
    {
        return Err(format!(
            "compact_at_tokens ({compact_at}) must be smaller than context_window ({window})"
        ));
    }
    if let (Some(window), Some(output)) = (raw.context_window, raw.max_output_tokens)
        && output >= window
    {
        return Err(format!(
            "max_output_tokens ({output}) must be smaller than context_window ({window})"
        ));
    }
    Ok(ModelConfig {
        alias: raw.alias,
        provider: raw.provider,
        id: raw.id,
        description: raw.description,
        features: ModelFeatures::parse(&raw.features)?,
        context_window: raw.context_window,
        max_output_tokens: raw.max_output_tokens,
        compact_model: raw.compact_model,
        compact_at_tokens: raw.compact_at_tokens,
        price: raw.price,
        tools: raw.tools,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    const DEEPSEEK: &str = r#"
default = "ds-flash"

[providers.deepseek]
wire = "anthropic"
base_url = "https://api.deepseek.com/anthropic/"
api_key_env = "DEEPSEEK_API_KEY"
auth = "bearer"

[[models]]
alias = "ds-flash"
provider = "deepseek"
id = "deepseek-flash"
description = "DeepSeek-V4.1-Flash"
context_window = 1000000
max_output_tokens = 32000
"#;

    #[test]
    fn builtin_registry_defaults_to_opus_with_every_feature() {
        let registry = ModelRegistry::builtin();
        let opus = registry.default_model();
        assert_eq!(opus.id, "claude-opus-5-5");
        assert_eq!(opus.features, ModelFeatures::ALL);
        assert_eq!(opus.compact_model.as_deref(), Some("haiku"));
        // Review N3: a missing window fell back to 200k, so the input
        // budget was about an eighth of the real one.
        for alias in ["opus", "sonnet", "haiku"] {
            assert_eq!(
                registry.find(alias).unwrap().context_window,
                Some(1_000_000)
            );
        }
        let provider = registry.provider(&opus.provider).unwrap();
        assert_eq!(provider.base_url, "https://api.anthropic.com");
        assert_eq!(provider.auth, AuthScheme::ApiKeyHeader);
    }

    #[test]
    fn compact_at_tokens_is_read_and_must_fit_the_window() {
        let model = |extra: &str| {
            ModelRegistry::from_toml(&format!(
                "[[models]]\nalias = \"m\"\nprovider = \"anthropic\"\nid = \"x\"\ncontext_window = 100000\n{extra}"
            ))
        };
        let registry = model("compact_at_tokens = 50000").unwrap();
        assert_eq!(registry.find("m").unwrap().compact_at_tokens, Some(50_000));
        let registry = model("price = { input = 4.0, output = 20.0, cache_read = 0.2 }").unwrap();
        assert_eq!(
            registry.find("m").unwrap().price.unwrap().cache_read,
            Some(0.2)
        );
        for bad in ["compact_at_tokens = 0", "compact_at_tokens = 100000"] {
            assert!(model(bad).is_err(), "{bad}");
        }
    }

    #[test]
    fn file_overlays_providers_and_models_on_the_builtins() {
        let registry = ModelRegistry::from_toml(DEEPSEEK).unwrap();
        let model = registry.default_model();
        assert_eq!(model.alias, "ds-flash");
        assert_eq!(model.features, ModelFeatures::default());
        assert_eq!(model.context_window, Some(1_000_000));
        assert_eq!(model.description.as_deref(), Some("DeepSeek-V4.1-Flash"));
        let provider = registry.provider("deepseek").unwrap();
        assert_eq!(provider.base_url, "https://api.deepseek.com/anthropic");
        assert_eq!(provider.auth, AuthScheme::Bearer);
        assert!(registry.find("opus").is_some());
    }

    #[test]
    fn a_file_entry_replaces_the_builtin_of_the_same_alias() {
        let registry = ModelRegistry::from_toml(
            r#"
[[models]]
alias = "opus"
provider = "anthropic"
id = "claude-opus-5-5"
features = ["adaptive_thinking"]
context_window = 1000000
"#,
        )
        .unwrap();
        let opus = registry.find("opus").unwrap();
        assert_eq!(opus.context_window, Some(1_000_000));
        assert!(opus.features.adaptive_thinking && !opus.features.effort);
        assert_eq!(
            registry.models.iter().filter(|m| m.alias == "opus").count(),
            1
        );
    }

    #[test]
    fn invalid_files_are_rejected_with_the_offending_entry() {
        let cases = [
            (
                "[providers.x]\nwire = \"openai-responses\"\nbase_url = \"https://a.example\"\napi_key_env = \"K\"\nauth = \"x-api-key\"",
                "auth must be bearer for the openai-responses wire",
            ),
            (
                "[providers.x]\nwire = \"openai-chat\"\nbase_url = \"https://a.example\"\napi_key_env = \"K\"\nauth = \"x-api-key\"",
                "auth must be bearer for the openai-chat wire",
            ),
            (
                "[providers.x]\nwire = \"openai-chat\"\nbase_url = \"https://a.example\"\napi_key_env = \"K\"\n[[models]]\nalias = \"m\"\nprovider = \"x\"\nid = \"y\"\nfeatures = [\"adaptive_thinking\"]",
                "only the effort feature applies to the openai-chat wire",
            ),
            (
                "[providers.x]\nwire = \"grpc\"\nbase_url = \"https://a.example\"\napi_key_env = \"K\"",
                "unknown wire 'grpc'",
            ),
            (
                "[providers.x]\nwire = \"anthropic\"\nbase_url = \"http://a.example\"\napi_key_env = \"K\"",
                "must use https",
            ),
            (
                "[providers.x]\nwire = \"anthropic\"\nbase_url = \"https://u:p@a.example\"\napi_key_env = \"K\"",
                "must not contain credentials",
            ),
            (
                "[providers.x]\nwire = \"anthropic\"\nbase_url = \"https://a.example\"\napi_key_env = \"sk-live-123\"",
                "not the key itself",
            ),
            (
                "[providers.x]\nwire = \"anthropic\"\nbase_url = \"https://a.example\"\napi_key_env = \"K\"\nauth = \"basic\"",
                "auth must be x-api-key, x-goog-api-key, or bearer",
            ),
            (
                "[providers.x]\nwire = \"google\"\nbase_url = \"https://a.example\"\napi_key_env = \"K\"\nauth = \"x-api-key\"",
                "auth must be x-goog-api-key or bearer for the google wire",
            ),
            (
                "[providers.x]\nwire = \"anthropic\"\nbase_url = \"https://a.example\"\napi_key_env = \"K\"\nauth = \"x-goog-api-key\"",
                "auth must be x-api-key or bearer for the anthropic wire",
            ),
            (
                "[providers.x]\nwire = \"openai-chat\"\nbase_url = \"https://a.example\"\napi_key_env = \"K\"\nauth = \"x-goog-api-key\"",
                "auth must be bearer for the openai-chat wire",
            ),
            (
                "[providers.x]\nwire = \"google\"\nbase_url = \"https://a.example\"\napi_key_env = \"K\"\n[[models]]\nalias = \"m\"\nprovider = \"x\"\nid = \"y\"\nfeatures = [\"anthropic_text_editor\"]",
                "only the effort feature applies to the google wire",
            ),
            (
                "[[models]]\nalias = \"m\"\nprovider = \"nowhere\"\nid = \"x\"",
                "unknown provider 'nowhere'",
            ),
            (
                "[providers.x]\nwire = \"openai-chat\"\nbase_url = \"https://a.example\"",
                "api_key_env is required unless base_url is on this machine",
            ),
            (
                "[providers.x]\nwire = \"openai-chat\"\nbase_url = \"http://localhost:8080/v1\"\nauth = \"bearer\"",
                "auth needs api_key_env",
            ),
            (
                "[[models]]\nalias = \"m\"\nprovider = \"anthropic\"\nid = \"x\"\ncompact_model = \"tiny\"",
                "compact_model 'tiny' is not a registered alias",
            ),
            (
                "[[models]]\nalias = \"m\"\nprovider = \"anthropic\"\nid = \"x\"\nfeatures = [\"vision\"]",
                "unknown feature 'vision'",
            ),
            (
                "[[models]]\nalias = \"m\"\nprovider = \"anthropic\"\nid = \"x\"\ncontext_window = 1000\nmax_output_tokens = 1000",
                "must be smaller than context_window",
            ),
            (
                "[[models]]\nalias = \"two words\"\nprovider = \"anthropic\"\nid = \"x\"",
                "contain no whitespace",
            ),
            (
                "[[models]]\nalias = \"m\"\nprovider = \"anthropic\"\nid = \"x\"\n[[models]]\nalias = \"m\"\nprovider = \"anthropic\"\nid = \"y\"",
                "defined more than once",
            ),
            (
                "[[models]]\nalias = \"m\"\nprovider = \"anthropic\"\nid = \"x\"\ndescription = \"a\\nb\"",
                "description must be a single line",
            ),
            (
                "default = \"missing\"",
                "default 'missing' is not a registered alias",
            ),
            ("api_key = \"sk-live\"", "unknown field"),
            (
                "[providers.x]\nwire = \"openai-responses\"\nbase_url = \"https://a.example\"\napi_key_env = \"K\"\n[[models]]\nalias = \"m\"\nprovider = \"x\"\nid = \"y\"\nfeatures = [\"eager_input_streaming\"]",
                "only the effort feature applies to the openai-responses wire",
            ),
        ];
        for (text, expected) in cases {
            let err = ModelRegistry::from_toml(text).unwrap_err();
            assert!(err.contains(expected), "{text:?}: {err}");
        }
        let err = ModelRegistry::from_toml(cases[4].0).unwrap_err();
        assert!(!err.contains("sk-live-123"), "{err}");
    }

    #[test]
    fn openai_providers_default_to_bearer_auth() {
        for (wire, expected) in [
            ("openai-chat", Wire::OpenAiChat),
            ("openai-responses", Wire::OpenAiResponses),
        ] {
            let registry = ModelRegistry::from_toml(&format!(
                "[providers.openai]\nwire = \"{wire}\"\nbase_url = \"https://api.openai.com/v1\"\napi_key_env = \"OPENAI_API_KEY\"\n[[models]]\nalias = \"gpt\"\nprovider = \"openai\"\nid = \"gpt-6-astra\"\nfeatures = [\"effort\"]"
            ))
            .unwrap();
            let provider = registry.provider("openai").unwrap();
            assert_eq!(provider.wire, expected);
            assert_eq!(provider.auth, AuthScheme::Bearer);
            assert!(registry.find("gpt").unwrap().features.effort);
        }
    }

    #[test]
    fn google_providers_default_to_the_goog_api_key_header() {
        let google = |auth: &str| {
            ModelRegistry::from_toml(&format!(
                "[providers.gemini]\nwire = \"google\"\nbase_url = \"https://generativelanguage.googleapis.com/v1beta/\"\napi_key_env = \"GEMINI_API_KEY\"\n{auth}\n[[models]]\nalias = \"gemini\"\nprovider = \"gemini\"\nid = \"gemini-4-carbon\"\nfeatures = [\"effort\"]"
            ))
            .unwrap()
        };
        let registry = google("");
        let provider = registry.provider("gemini").unwrap();
        assert_eq!(provider.wire, Wire::Google);
        assert_eq!(
            provider.base_url,
            "https://generativelanguage.googleapis.com/v1beta"
        );
        assert_eq!(provider.auth, AuthScheme::GoogApiKey);
        assert!(registry.find("gemini").unwrap().features.effort);
        let named = google("auth = \"x-goog-api-key\"");
        assert_eq!(
            named.provider("gemini").unwrap().auth,
            AuthScheme::GoogApiKey
        );
        // OpenCode Zen takes the key as a bearer token.
        let bearer = google("auth = \"bearer\"");
        assert_eq!(bearer.provider("gemini").unwrap().auth, AuthScheme::Bearer);
    }

    #[test]
    fn the_example_registry_parses() {
        let registry =
            ModelRegistry::from_toml(include_str!("../../../models.toml.example")).unwrap();
        for alias in ["ds-flash", "ds-pro", "glm", "gpt"] {
            assert!(registry.find(alias).is_some(), "{alias}");
        }
        assert_eq!(
            registry.provider("openai").unwrap().wire,
            Wire::OpenAiResponses
        );
    }

    #[test]
    fn the_commented_gemini_example_parses_once_uncommented() {
        let example = include_str!("../../../models.toml.example");
        let block = |start: &str| {
            let from = example.find(start).unwrap();
            example[from..]
                .lines()
                .take_while(|line| line.starts_with("# "))
                .map(|line| &line[2..])
                .collect::<Vec<_>>()
                .join("\n")
        };
        let text = format!(
            "{}\n{}",
            block("# [providers.gemini]"),
            block("# [[models]]\n# alias = \"gemini\"")
        );
        let registry = ModelRegistry::from_toml(&text).unwrap();
        assert_eq!(registry.provider("gemini").unwrap().wire, Wire::Google);
        assert_eq!(registry.find("gemini").unwrap().id, "gemini-4-carbon");
    }

    #[test]
    fn plain_http_is_allowed_only_on_loopback() {
        for url in [
            "http://localhost:11434",
            "http://127.0.0.1:8080/anthropic",
            "http://[::1]:9000",
        ] {
            assert!(parse_base_url(url).is_ok(), "{url}");
        }
        assert!(parse_base_url("http://192.168.1.10:11434").is_err());
    }

    #[test]
    fn env_model_resolves_alias_then_id_then_ad_hoc_claude() {
        let registry = ModelRegistry::from_toml(DEEPSEEK).unwrap();
        assert_eq!(registry.resolve_env_model("opus").id, "claude-opus-5-5");
        assert_eq!(
            registry.resolve_env_model("deepseek-flash").alias,
            "ds-flash"
        );
        let ad_hoc = registry.resolve_env_model("claude-future-1");
        assert_eq!(ad_hoc.provider, BUILTIN_PROVIDER);
        assert!(!registry.is_registered(&ad_hoc));
    }

    #[test]
    fn load_reads_the_xdg_file_and_falls_back_to_builtins_when_absent() {
        let dir = tempfile::tempdir().unwrap();
        let config_home = dir.path().to_str().unwrap().to_string();
        let lookup = |key: &str| (key == "XDG_CONFIG_HOME").then(|| config_home.clone());
        assert_eq!(
            ModelRegistry::load(&lookup).unwrap(),
            ModelRegistry::builtin()
        );

        fs::create_dir_all(dir.path().join("socium")).unwrap();
        fs::write(dir.path().join("socium/models.toml"), DEEPSEEK).unwrap();
        assert_eq!(ModelRegistry::load(&lookup).unwrap().default, "ds-flash");
    }

    /// A config home holding `files` (relative path, text) under `socium/`.
    fn config_home(files: &[(&str, &str)]) -> (tempfile::TempDir, String) {
        let dir = tempfile::tempdir().unwrap();
        for (path, text) in files {
            let path = dir.path().join("socium").join(path);
            fs::create_dir_all(path.parent().unwrap()).unwrap();
            fs::write(path, text).unwrap();
        }
        let home = dir.path().to_str().unwrap().to_string();
        (dir, home)
    }

    fn load_from(home: &str) -> io::Result<ModelRegistry> {
        ModelRegistry::load(&|key: &str| (key == "XDG_CONFIG_HOME").then(|| home.to_string()))
    }

    const LOCAL_A: &str = "[providers.a]\nwire = \"openai-chat\"\nbase_url = \"http://localhost:1/v1\"\n[[models]]\nalias = \"m\"\nprovider = \"a\"\nid = \"from-a\"\n[[models]]\nalias = \"only-a\"\nprovider = \"a\"\nid = \"a\"";
    const LOCAL_B: &str = "[providers.b]\nwire = \"openai-chat\"\nbase_url = \"http://localhost:2/v1\"\n[[models]]\nalias = \"m\"\nprovider = \"b\"\nid = \"from-b\"";

    #[test]
    fn models_dir_files_load_in_name_order_under_models_toml() {
        let (_dir, home) = config_home(&[
            ("models.d/20-b.toml", LOCAL_B),
            ("models.d/10-a.toml", LOCAL_A),
            ("models.d/notes.txt", "not toml"),
        ]);
        let registry = load_from(&home).unwrap();
        assert_eq!(registry.find("m").unwrap().id, "from-b");
        assert_eq!(registry.find("only-a").unwrap().provider, "a");
        assert!(registry.find("opus").is_some());

        let (_dir, home) = config_home(&[
            ("models.d/10-a.toml", LOCAL_A),
            (
                "models.toml",
                "default = \"m\"\n[[models]]\nalias = \"m\"\nprovider = \"a\"\nid = \"from-file\"",
            ),
        ]);
        let registry = load_from(&home).unwrap();
        assert_eq!(registry.default_model().id, "from-file");
        assert_eq!(registry.models.iter().filter(|m| m.alias == "m").count(), 1);
    }

    #[test]
    fn models_toml_may_replace_a_provider_from_models_dir() {
        let (_dir, home) = config_home(&[
            ("models.d/a.toml", LOCAL_A),
            (
                "models.toml",
                "[providers.a]\nwire = \"openai-chat\"\nbase_url = \"http://localhost:9/v1\"",
            ),
        ]);
        let registry = load_from(&home).unwrap();
        assert_eq!(
            registry.provider("a").unwrap().base_url,
            "http://localhost:9/v1"
        );
    }

    #[test]
    fn models_dir_errors_name_the_file() {
        type Case<'a> = (&'a [(&'a str, &'a str)], &'a [&'a str]);
        let cases: [Case; 5] = [
            (
                &[("models.d/x.toml", LOCAL_A), ("models.d/y.toml", LOCAL_A)],
                &["provider 'a' is defined in both", "x.toml", "y.toml"],
            ),
            (
                &[("models.d/x.toml", "default = \"opus\"")],
                &["x.toml: default is only allowed in models.toml"],
            ),
            (
                &[(
                    "models.d/x.toml",
                    "[[models]]\nalias = \"m\"\nprovider = \"anthropic\"\nid = \"1\"\n[[models]]\nalias = \"m\"\nprovider = \"anthropic\"\nid = \"2\"",
                )],
                &["x.toml: model alias 'm' is defined more than once"],
            ),
            (
                &[(
                    "models.d/x.toml",
                    "[[models]]\nalias = \"m\"\nprovider = \"nowhere\"\nid = \"1\"",
                )],
                &["x.toml: model 'm': unknown provider 'nowhere'"],
            ),
            (
                &[
                    ("models.d/x.toml", LOCAL_A),
                    ("models.toml", "default = \"missing\""),
                ],
                &["models.toml: default 'missing' is not a registered alias"],
            ),
        ];
        for (files, expected) in cases {
            let (_dir, home) = config_home(files);
            let err = load_from(&home).unwrap_err();
            assert_eq!(err.kind(), ErrorKind::InvalidInput);
            for part in expected {
                assert!(err.to_string().contains(part), "{files:?}: {err}");
            }
        }
    }

    #[test]
    fn models_dir_sits_beside_an_explicit_models_file() {
        let dir = tempfile::tempdir().unwrap();
        fs::create_dir(dir.path().join("models.d")).unwrap();
        fs::write(dir.path().join("models.d/a.toml"), LOCAL_A).unwrap();
        let file = dir.path().join("custom.toml");
        fs::write(&file, "").unwrap();
        let file = file.to_str().unwrap().to_string();
        let lookup = |key: &str| (key == MODELS_FILE_ENV).then(|| file.clone());
        assert!(
            ModelRegistry::load(&lookup)
                .unwrap()
                .find("only-a")
                .is_some()
        );
    }

    #[test]
    fn a_missing_explicit_file_is_an_error() {
        let lookup =
            |key: &str| (key == MODELS_FILE_ENV).then(|| "/nonexistent/models.toml".to_string());
        let err = ModelRegistry::load(&lookup).unwrap_err();
        assert!(err.to_string().contains("/nonexistent/models.toml"));
    }

    #[test]
    fn parse_errors_name_the_file() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("models.toml");
        fs::write(&path, "default = \"missing\"").unwrap();
        let path_text = path.to_str().unwrap().to_string();
        let lookup = |key: &str| (key == MODELS_FILE_ENV).then(|| path_text.clone());
        let err = ModelRegistry::load(&lookup).unwrap_err();
        assert_eq!(err.kind(), ErrorKind::InvalidInput);
        assert!(err.to_string().starts_with(&path_text));
    }

    #[test]
    fn a_provider_on_this_machine_may_go_without_a_key() {
        let registry = ModelRegistry::from_toml(
            "[providers.ollama]\nwire = \"openai-chat\"\nbase_url = \"http://127.0.0.1:11434/v1\"\n[[models]]\nalias = \"qwen\"\nprovider = \"ollama\"\nid = \"qwen3\"",
        )
        .unwrap();
        let provider = registry.provider("ollama").unwrap();
        assert_eq!(provider.api_key(&|_| None), Ok(None));

        let anthropic = ModelRegistry::builtin();
        let anthropic = anthropic.provider("anthropic").unwrap();
        assert_eq!(
            anthropic.api_key(&|_| None),
            Err("ANTHROPIC_API_KEY is not set".to_string())
        );
        assert_eq!(
            anthropic.api_key(&|_| Some("k".into())),
            Ok(Some("k".to_string()))
        );
    }

    #[test]
    fn a_model_can_be_marked_as_taking_no_tools() {
        let registry = ModelRegistry::from_toml(
            "[providers.local]\nwire = \"openai-chat\"\nbase_url = \"http://localhost:8080/v1\"\napi_key_env = \"K\"\n[[models]]\nalias = \"plain\"\nprovider = \"local\"\nid = \"tiny\"\ntools = false\n[[models]]\nalias = \"agent\"\nprovider = \"local\"\nid = \"big\"",
        )
        .unwrap();
        assert!(!registry.find("plain").unwrap().tools);
        assert!(registry.find("agent").unwrap().tools);
        assert!(registry.find("opus").unwrap().tools);
    }
}
