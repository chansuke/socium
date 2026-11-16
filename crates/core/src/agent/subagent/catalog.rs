//! The subagent types the `agent` tool offers: the built-in `explorer` and
//! `worker`, and types the user defines in Markdown files.
//!
//! A definition file is a TOML front matter between `+++` lines, followed by
//! the subagent's instructions, which follow `SUBAGENT_FRAMING` in its
//! system prompt:
//!
//! ```text
//! +++
//! name = "reviewer"
//! description = "Reviews a diff for correctness bugs"
//! tools = ["read_file", "search_text", "git_diff"]  # default: read-only tools
//! model = "haiku"                                   # default: the current model
//! +++
//! You review changes for correctness bugs. ...
//! ```

use crate::constants::{
    AGENT_TOOL_NAME, SUBAGENT_EXPLORER_INSTRUCTIONS, SUBAGENT_WORKER_INSTRUCTIONS,
};
use crate::models::{ModelConfig, ModelRegistry};
use crate::providers::Provider;
use crate::tools::{CustomToolDefinition, ToolDefinition, tools_for_model};
use serde::Deserialize;
use serde_json::json;
use std::fs;
use std::io::ErrorKind;
use std::path::{Path, PathBuf};

pub(crate) const AGENTS_DIR_ENV: &str = "SOCIUM_AGENTS_DIR";
const MAX_DEFINITION_BYTES: u64 = 64 * 1024;
const MAX_DEFINITIONS: usize = 32;
const MAX_NAME_CHARS: usize = 32;
const MAX_DESCRIPTION_CHARS: usize = 300;

/// Tools that only read; the `explorer` set and the default for user types.
const READ_ONLY_TOOLS: &[&str] = &[
    "read_file",
    "list_files",
    "search_text",
    "find_symbol",
    "git_branch",
    "git_status",
    "git_diff",
    "git_log",
];

/// The tools a subagent type gets, as a subset of the tools offered to its
/// model. `agent` is never part of it, so subagents do not nest.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum ToolSet {
    ReadOnly,
    AllButAgent,
    Only(Vec<String>),
}

impl ToolSet {
    pub(crate) fn allows(&self, tool: &str) -> bool {
        match self {
            Self::ReadOnly => READ_ONLY_TOOLS.contains(&tool),
            Self::AllButAgent => tool != AGENT_TOOL_NAME,
            Self::Only(tools) => tools.iter().any(|name| name == tool),
        }
    }

    /// Whether a run that stops halfway may have changed the workspace.
    pub(crate) fn may_change_workspace(&self) -> bool {
        match self {
            Self::ReadOnly => false,
            Self::AllButAgent => true,
            Self::Only(tools) => tools
                .iter()
                .any(|tool| !READ_ONLY_TOOLS.contains(&tool.as_str())),
        }
    }
}

/// A registered model a subagent can run on instead of the current one,
/// built at startup because its API key is set.
pub(crate) struct SubagentModel {
    pub(crate) config: ModelConfig,
    pub(crate) provider: Box<dyn Provider>,
    pub(crate) max_tokens: u32,
}

pub(crate) struct SubagentType {
    pub(crate) name: String,
    /// One line telling the delegating model when to choose this type.
    pub(crate) description: String,
    /// What this type does, put after `SUBAGENT_FRAMING` in its system
    /// prompt.
    pub(crate) instructions: String,
    pub(crate) tools: ToolSet,
    /// Alias of the model the type runs on; `None` runs on the main
    /// agent's current model.
    pub(crate) model: Option<String>,
}

pub(crate) struct SubagentCatalog {
    types: Vec<SubagentType>,
    /// Models a call can choose with `model`, in registry order.
    models: Vec<SubagentModel>,
}

impl SubagentCatalog {
    pub(crate) fn builtin() -> Self {
        Self {
            types: builtin_types(),
            models: Vec::new(),
        }
    }

    /// The built-in types, then `user_types`, with `models` to choose from.
    pub(crate) fn new(user_types: Vec<SubagentType>, models: Vec<SubagentModel>) -> Self {
        let mut types = builtin_types();
        types.extend(user_types);
        Self { types, models }
    }

    pub(crate) fn model(&self, alias: &str) -> Option<&SubagentModel> {
        self.models.iter().find(|model| model.config.alias == alias)
    }

    /// What the models' providers ignore, such as `EFFORT` on a model
    /// without the feature.
    pub(crate) fn startup_warnings(&self) -> Vec<String> {
        self.models
            .iter()
            .flat_map(|model| model.provider.startup_warnings())
            .collect()
    }

    pub(crate) fn model_aliases(&self) -> Vec<&str> {
        self.models
            .iter()
            .map(|model| model.config.alias.as_str())
            .collect()
    }

    pub(crate) fn find(&self, name: &str) -> Option<&SubagentType> {
        self.types.iter().find(|kind| kind.name == name)
    }

    pub(crate) fn names(&self) -> Vec<&str> {
        self.types.iter().map(|kind| kind.name.as_str()).collect()
    }

    /// The `agent` tool, listing every type and what it is for.
    pub(crate) fn tool_definition(&self) -> ToolDefinition {
        let types = self
            .types
            .iter()
            .map(|kind| match &kind.model {
                Some(model) => format!("- {}: {} (runs on {model})", kind.name, kind.description),
                None => format!("- {}: {}", kind.name, kind.description),
            })
            .collect::<Vec<_>>()
            .join("\n");
        let models = self
            .models
            .iter()
            .map(|model| match &model.config.description {
                Some(description) => format!("- {}: {description}", model.config.alias),
                None => format!("- {}: {}", model.config.alias, model.config.id),
            })
            .collect::<Vec<_>>()
            .join("\n");
        let mut schema = json!({
            "type": "object",
            "properties": {
                "description": {
                    "type": "string",
                    "description": "A short label for the task, 3 to 6 words, shown to the user."
                },
                "prompt": {
                    "type": "string",
                    "description": "The complete task for the subagent, with the context it needs and what to report."
                },
                "agent_type": {
                    "type": "string",
                    "enum": self.names(),
                    "description": "The agent type that fits the task; see the list in this tool's description."
                },
                "resume": {
                    "type": "string",
                    "description": "ID of an earlier subagent, from the end of its report, to give it this prompt as a follow-up with everything it already saw. Use the same agent_type."
                }
            },
            "required": ["description", "prompt", "agent_type"],
            "additionalProperties": false
        });
        let mut description = format!(
            "Delegate a self-contained task to a subagent. The subagent starts with an empty context, works with its own tools until it is done, and returns one written report as this tool's result; nothing else it saw comes back. Use it to keep a broad search or a separable piece of work out of this conversation, not for a single file read or search. The subagent cannot see this conversation, so put everything it needs in prompt: the goal, relevant paths and constraints, and what the report must contain. Commands and commits a subagent runs still need the user's approval. Subagents cannot start subagents. New calls of read-only types such as explorer, placed one after another in one response, run at the same time, so put independent searches in one response; other calls run one at a time. The report ends with the subagent's ID; to follow up on its work without repeating it, call agent again with resume set to that ID. Read the report critically and tell the user what matters; they do not see it.\n\nAgent types:\n{types}"
        );
        if !self.models.is_empty() {
            description.push_str(&format!(
                "\n\nModels (optional model input; by default a subagent runs on its type's model, else on yours). Choose a smaller model for simple searches:\n{models}"
            ));
            schema["properties"]["model"] = json!({
                "type": "string",
                "enum": self.model_aliases(),
                "description": "Model alias for this run; see the list in this tool's description."
            });
        }
        ToolDefinition::Custom(CustomToolDefinition {
            name: AGENT_TOOL_NAME.to_string(),
            description,
            input_schema: schema,
        })
    }
}

fn builtin_types() -> Vec<SubagentType> {
    vec![
        SubagentType {
            name: "explorer".into(),
            description: "read-only investigation; reads, lists, and searches files and inspects Git, never changes anything.".into(),
            instructions: SUBAGENT_EXPLORER_INSTRUCTIONS.into(),
            tools: ToolSet::ReadOnly,
            model: None,
        },
        SubagentType {
            name: "worker".into(),
            description: "tasks that need edits or commands; has every tool but agent.".into(),
            instructions: SUBAGENT_WORKER_INSTRUCTIONS.into(),
            tools: ToolSet::AllButAgent,
            model: None,
        },
    ]
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct FrontMatter {
    name: String,
    description: String,
    tools: Option<Vec<String>>,
    model: Option<String>,
}

/// A definition file as written, before its model is resolved.
#[derive(Debug, PartialEq, Eq)]
pub(crate) struct Definition {
    pub(crate) name: String,
    pub(crate) description: String,
    pub(crate) instructions: String,
    pub(crate) tools: ToolSet,
    pub(crate) model: Option<String>,
}

pub(crate) fn parse_definition(text: &str) -> Result<Definition, String> {
    let (front, body) = split_front_matter(text)?;
    let front: FrontMatter = toml::from_str(front).map_err(|err| err.to_string())?;

    let name = front.name;
    if name.is_empty()
        || name.chars().count() > MAX_NAME_CHARS
        || !name.starts_with(|c: char| c.is_ascii_lowercase())
        || !name
            .chars()
            .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-' || c == '_')
    {
        return Err(format!(
            "name '{name}' must start with a lowercase letter and use only a-z, 0-9, '-' and '_', up to {MAX_NAME_CHARS} characters"
        ));
    }
    if builtin_types().iter().any(|kind| kind.name == name) {
        return Err(format!("name '{name}' is a built-in agent type"));
    }

    let description = front.description.trim().to_string();
    if description.is_empty()
        || description.contains('\n')
        || description.chars().count() > MAX_DESCRIPTION_CHARS
    {
        return Err(format!(
            "description must be one non-empty line of at most {MAX_DESCRIPTION_CHARS} characters"
        ));
    }

    let tools = match front.tools {
        None => ToolSet::ReadOnly,
        Some(names) => {
            let known = tools_for_model(true);
            let mut chosen = Vec::new();
            for tool in names {
                if tool == AGENT_TOOL_NAME {
                    return Err("tools cannot include agent; subagents do not nest".into());
                }
                if !known.iter().any(|known| known.name() == tool) {
                    return Err(format!("unknown tool '{tool}'"));
                }
                if !chosen.contains(&tool) {
                    chosen.push(tool);
                }
            }
            if chosen.is_empty() {
                return Err(
                    "tools must name at least one tool; omit it for read-only tools".into(),
                );
            }
            ToolSet::Only(chosen)
        }
    };

    let instructions = body.trim().to_string();
    if instructions.is_empty() {
        return Err("the instructions after the front matter are empty".into());
    }
    Ok(Definition {
        name,
        description,
        instructions,
        tools,
        model: front.model,
    })
}

fn split_front_matter(text: &str) -> Result<(&str, &str), String> {
    let text = text.strip_prefix('\u{feff}').unwrap_or(text);
    let mut lines = text.split_inclusive('\n');
    if lines.next().map(|line| line.trim_end()) != Some("+++") {
        return Err("must start with a front matter between +++ lines".into());
    }
    let start = text.find('\n').map_or(text.len(), |end| end + 1);
    let mut offset = start;
    for line in lines {
        if line.trim_end() == "+++" {
            return Ok((&text[start..offset], &text[offset + line.len()..]));
        }
        offset += line.len();
    }
    Err("the front matter is not closed with a +++ line".into())
}

/// Says why a registered model cannot run, or `Ok` when it can.
pub(crate) type ModelCheck<'a> = dyn Fn(&ModelConfig) -> Result<(), String> + 'a;

/// Reads the definitions in the directory named by `SOCIUM_AGENTS_DIR`, or
/// else `socium/agents` in the XDG config directory, in file name order. A
/// file that cannot be used is skipped with a warning naming it; it never
/// stops startup. A missing default directory means no user types.
pub(crate) fn load_user_types(
    lookup: &dyn Fn(&str) -> Option<String>,
    registry: &ModelRegistry,
    check_model: &ModelCheck<'_>,
) -> (Vec<SubagentType>, Vec<String>) {
    let mut warnings = Vec::new();
    let explicit = lookup(AGENTS_DIR_ENV).filter(|value| !value.is_empty());
    let Some(dir) = explicit
        .as_ref()
        .map(PathBuf::from)
        .or_else(|| default_agents_dir(lookup))
    else {
        return (Vec::new(), warnings);
    };
    let entries = match fs::read_dir(&dir) {
        Ok(entries) => entries,
        Err(err) if err.kind() == ErrorKind::NotFound && explicit.is_none() => {
            return (Vec::new(), warnings);
        }
        Err(err) => {
            warnings.push(format!(
                "Cannot read agent definitions in {}: {err}",
                dir.display()
            ));
            return (Vec::new(), warnings);
        }
    };
    let mut paths = entries
        .filter_map(|entry| entry.ok().map(|entry| entry.path()))
        .filter(|path| path.extension().is_some_and(|ext| ext == "md"))
        .collect::<Vec<_>>();
    paths.sort();

    let mut types: Vec<SubagentType> = Vec::new();
    for path in paths {
        let skip =
            |reason: String| format!("Agent definition {} skipped: {reason}", path.display());
        if types.len() == MAX_DEFINITIONS {
            warnings.push(skip(format!(
                "at most {MAX_DEFINITIONS} agent types are loaded"
            )));
            continue;
        }
        let definition = match read_definition(&path).and_then(|text| parse_definition(&text)) {
            Ok(definition) => definition,
            Err(reason) => {
                warnings.push(skip(reason));
                continue;
            }
        };
        if types.iter().any(|kind| kind.name == definition.name) {
            warnings.push(skip(format!(
                "another file already defines '{}'",
                definition.name
            )));
            continue;
        }
        // A type that names a model never silently runs on another one.
        let model = match &definition.model {
            None => None,
            Some(alias) => match registry.find(alias) {
                None => {
                    warnings.push(skip(format!("model '{alias}' is not registered")));
                    continue;
                }
                Some(config) => match check_model(config) {
                    Ok(()) => Some(alias.clone()),
                    Err(reason) => {
                        warnings.push(skip(format!("model '{alias}' is unavailable: {reason}")));
                        continue;
                    }
                },
            },
        };
        types.push(SubagentType {
            name: definition.name,
            description: definition.description,
            instructions: definition.instructions,
            tools: definition.tools,
            model,
        });
    }
    (types, warnings)
}

fn read_definition(path: &Path) -> Result<String, String> {
    let metadata = fs::metadata(path).map_err(|err| err.to_string())?;
    if !metadata.is_file() {
        return Err("not a regular file".into());
    }
    if metadata.len() > MAX_DEFINITION_BYTES {
        return Err(format!("larger than {} KiB", MAX_DEFINITION_BYTES / 1024));
    }
    fs::read_to_string(path).map_err(|err| err.to_string())
}

/// `socium/agents` in the XDG config directory.
fn default_agents_dir(lookup: &dyn Fn(&str) -> Option<String>) -> Option<PathBuf> {
    let config_home = lookup("XDG_CONFIG_HOME")
        .map(PathBuf::from)
        .filter(|path| path.is_absolute())
        .or_else(|| {
            lookup("HOME")
                .filter(|home| !home.is_empty())
                .map(|home| Path::new(&home).join(".config"))
        })?;
    Some(config_home.join("socium").join("agents"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_support::{ScriptedProvider, make_temp_dir};

    const REVIEWER: &str = "+++\nname = \"reviewer\"\ndescription = \"Reviews a diff for correctness bugs\"\ntools = [\"read_file\", \"git_diff\", \"read_file\"]\nmodel = \"haiku\"\n+++\n\nYou review changes.\n";

    fn no_model(_: &ModelConfig) -> Result<(), String> {
        Err("no models in this test".into())
    }

    fn any_model(_: &ModelConfig) -> Result<(), String> {
        Ok(())
    }

    fn load(dir: &Path, build: &ModelCheck<'_>) -> (Vec<SubagentType>, Vec<String>) {
        let dir = dir.to_string_lossy().into_owned();
        let lookup = move |key: &str| (key == AGENTS_DIR_ENV).then(|| dir.clone());
        load_user_types(&lookup, &ModelRegistry::builtin(), build)
    }

    #[test]
    fn a_definition_is_front_matter_then_the_instructions() {
        let definition = parse_definition(REVIEWER).unwrap();

        assert_eq!(
            definition,
            Definition {
                name: "reviewer".into(),
                description: "Reviews a diff for correctness bugs".into(),
                instructions: "You review changes.".into(),
                tools: ToolSet::Only(vec!["read_file".into(), "git_diff".into()]),
                model: Some("haiku".into()),
            }
        );
        assert!(!definition.tools.may_change_workspace());
    }

    #[test]
    fn tools_default_to_read_only_and_writing_tools_are_marked() {
        let minimal =
            "+++\r\nname = \"scout\"\ndescription = \"Finds things\"\n+++\r\nLook around.";
        assert_eq!(parse_definition(minimal).unwrap().tools, ToolSet::ReadOnly);

        let fixer = "+++\nname = \"fixer\"\ndescription = \"Fixes\"\ntools = [\"read_file\", \"apply_patch\"]\n+++\nFix it.";
        let tools = parse_definition(fixer).unwrap().tools;
        assert!(tools.may_change_workspace());
        assert!(tools.allows("apply_patch"));
        assert!(!tools.allows("run_command"));
    }

    #[test]
    fn invalid_definitions_say_what_is_wrong() {
        let with = |front: &str, body: &str| format!("+++\n{front}\n+++\n{body}");
        let base = "description = \"d\"";
        let cases = [
            (
                "no front matter".to_string(),
                "must start with a front matter",
            ),
            (format!("+++\nname = \"a\"\n{base}\nprompt"), "not closed"),
            (
                with(&format!("name = \"Bad\"\n{base}"), "p"),
                "name 'Bad' must start",
            ),
            (
                with(&format!("name = \"worker\"\n{base}"), "p"),
                "built-in agent type",
            ),
            (
                with("name = \"a\"\ndescription = \" \"", "p"),
                "description must be",
            ),
            (
                with(&format!("name = \"a\"\n{base}\ntools = [\"agent\"]"), "p"),
                "cannot include agent",
            ),
            (
                with(&format!("name = \"a\"\n{base}\ntools = [\"rm\"]"), "p"),
                "unknown tool 'rm'",
            ),
            (
                with(&format!("name = \"a\"\n{base}\ntools = []"), "p"),
                "at least one tool",
            ),
            (
                with(&format!("name = \"a\"\n{base}\ncolor = \"red\""), "p"),
                "unknown field",
            ),
            (
                with(&format!("name = \"a\"\n{base}"), "  \n"),
                "instructions after the front matter",
            ),
        ];
        for (text, expected) in cases {
            let err = parse_definition(&text).unwrap_err();
            assert!(err.contains(expected), "{text:?}: {err}");
        }
    }

    #[test]
    fn user_types_load_in_file_order_and_bad_files_are_skipped_with_a_warning() {
        let dir = make_temp_dir("agents");
        let write = |name: &str, text: &str| fs::write(dir.path().join(name), text).unwrap();
        write("b-reviewer.md", REVIEWER);
        write(
            "a-scout.md",
            "+++\nname = \"scout\"\ndescription = \"Finds things\"\n+++\nLook.",
        );
        write(
            "c-again.md",
            "+++\nname = \"scout\"\ndescription = \"Again\"\n+++\nLook.",
        );
        write("d-broken.md", "no front matter");
        write(
            "e-ghost.md",
            "+++\nname = \"ghost\"\ndescription = \"d\"\nmodel = \"nope\"\n+++\np",
        );
        write("notes.txt", "ignored");

        let (types, warnings) = load(dir.path(), &any_model);

        let names = types
            .iter()
            .map(|kind| kind.name.as_str())
            .collect::<Vec<_>>();
        assert_eq!(names, ["scout", "reviewer"]);
        assert!(types[0].model.is_none());
        assert_eq!(types[1].model.as_deref(), Some("haiku"));
        assert_eq!(warnings.len(), 3, "{warnings:#?}");
        assert!(
            warnings[0].contains("c-again.md") && warnings[0].contains("already defines 'scout'")
        );
        assert!(warnings[1].contains("d-broken.md"));
        assert!(
            warnings[2].contains("e-ghost.md")
                && warnings[2].contains("model 'nope' is not registered")
        );
    }

    #[test]
    fn a_type_whose_model_cannot_start_is_skipped_not_moved_to_another_model() {
        let dir = make_temp_dir("agents_model");
        fs::write(dir.path().join("reviewer.md"), REVIEWER).unwrap();

        let (types, warnings) = load(dir.path(), &no_model);

        assert!(types.is_empty());
        assert_eq!(warnings.len(), 1);
        assert!(warnings[0].contains("model 'haiku' is unavailable: no models in this test"));
    }

    #[test]
    fn a_missing_directory_is_silent_only_when_it_is_the_default() {
        let dir = make_temp_dir("agents_missing");
        let missing = dir.path().join("none");

        let (types, warnings) = load(&missing, &no_model);
        assert!(types.is_empty());
        assert_eq!(warnings.len(), 1);
        assert!(warnings[0].starts_with("Cannot read agent definitions"));

        let home = dir.path().to_string_lossy().into_owned();
        let lookup = move |key: &str| (key == "HOME").then(|| home.clone());
        let (types, warnings) = load_user_types(&lookup, &ModelRegistry::builtin(), &no_model);
        assert!(types.is_empty() && warnings.is_empty());
    }

    #[test]
    fn oversized_definitions_are_not_read() {
        let dir = make_temp_dir("agents_large");
        let big = format!(
            "+++\nname = \"big\"\ndescription = \"d\"\n+++\n{}",
            "x".repeat(MAX_DEFINITION_BYTES as usize)
        );
        fs::write(dir.path().join("big.md"), big).unwrap();

        let (types, warnings) = load(dir.path(), &no_model);

        assert!(types.is_empty());
        assert!(warnings[0].contains("larger than 64 KiB"), "{warnings:?}");
    }

    #[test]
    fn the_agent_tool_lists_every_type() {
        let user = parse_definition(REVIEWER).unwrap();
        let haiku = ModelRegistry::builtin().find("haiku").unwrap().clone();
        let catalog = SubagentCatalog::new(
            vec![SubagentType {
                name: user.name,
                description: user.description,
                instructions: user.instructions,
                tools: user.tools,
                model: user.model,
            }],
            vec![SubagentModel {
                config: haiku,
                provider: Box::new(ScriptedProvider::new(Vec::new())),
                max_tokens: 100,
            }],
        );

        let ToolDefinition::Custom(tool) = catalog.tool_definition() else {
            panic!("agent is a schema tool");
        };

        assert_eq!(
            tool.input_schema["properties"]["agent_type"]["enum"],
            json!(["explorer", "worker", "reviewer"])
        );
        assert!(
            tool.description
                .contains("- reviewer: Reviews a diff for correctness bugs (runs on haiku)\n"),
            "{}",
            tool.description
        );
        assert!(
            tool.description
                .ends_with("- haiku: Haiku 5.5 for quick answers"),
            "{}",
            tool.description
        );
        assert_eq!(
            tool.input_schema["properties"]["model"]["enum"],
            json!(["haiku"])
        );
        // Without models to choose from, there is no model input.
        let ToolDefinition::Custom(builtin) = SubagentCatalog::builtin().tool_definition() else {
            panic!("agent is a schema tool");
        };
        assert!(builtin.input_schema["properties"].get("model").is_none());
    }

    #[test]
    fn the_example_definition_parses() {
        let text = include_str!("../../../../../agent.md.example");
        let definition = parse_definition(text).unwrap();
        assert_eq!(definition.name, "reviewer");
    }
}
