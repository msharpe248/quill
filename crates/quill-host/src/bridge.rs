//! The messages of the shell-core bridge, as Rust types.
//!
//! `schema/bridge.json` is the definition. These types, the Swift types in
//! `app/Sources/Bridge` and the examples in `schema/examples` are all tested
//! against it; change them together.

use serde::de::DeserializeOwned;
use serde::{Deserialize, Serialize};

/// A message the core sends to the shell.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "method", content = "params")]
pub enum CoreToShell {
    /// Every command, once at startup, in menu order.
    #[serde(rename = "commands.list")]
    CommandsList(CommandsList),
    /// The whole of what the shell mirrors, whenever any of it changes.
    #[serde(rename = "state.update")]
    StateUpdate(StateUpdate),
    /// Show a file in a Quick Look pane beside the terminal.
    #[serde(rename = "pane.preview")]
    PanePreview(PanePreview),
    /// Close the second pane and return focus to the terminal.
    #[serde(rename = "pane.close")]
    PaneClose(NoParams),
}

/// A message the shell sends to the core.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "method", content = "params")]
pub enum ShellToCore {
    /// A menu item, toolbar button or notification action was used.
    #[serde(rename = "command.run")]
    CommandRun(CommandRun),
    /// The app is quitting: flush and exit cleanly.
    #[serde(rename = "app.will_quit")]
    AppWillQuit(NoParams),
}

/// `"params": {}`.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct NoParams {}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CommandsList {
    pub commands: Vec<CommandInfo>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CommandInfo {
    /// The stable name, as used in `keys.toml` and `command.run`.
    pub name: String,
    pub title: String,
    /// Key sequences in Quill's notation, the preferred one first.
    pub keys: Vec<String>,
    /// The menu the command appears in, if any.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub menu: Option<Menu>,
    /// An SF Symbol name. A command with one may be shown in the toolbar.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub symbol: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Menu {
    App,
    File,
    Edit,
    View,
    Mailbox,
    Message,
    Window,
    Help,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct StateUpdate {
    /// Where the keyboard focus is inside the terminal UI.
    pub context: Context,
    /// Names of the commands that would do something right now.
    pub enabled: Vec<String>,
    /// Unread messages across all inboxes, for the Dock badge.
    pub unread: u32,
    /// The window title.
    pub title: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Context {
    Sidebar,
    List,
    Reader,
    Help,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PanePreview {
    /// Absolute path of a file the shell can read.
    pub path: String,
    pub title: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CommandRun {
    pub name: String,
}

#[derive(Debug, thiserror::Error)]
pub enum BridgeError {
    #[error("not a bridge message: {0}")]
    Malformed(#[from] serde_json::Error),
    #[error("unsupported JSON-RPC version \"{0}\"")]
    Version(String),
}

const VERSION: &str = "2.0";

#[derive(Serialize)]
struct Outgoing<'a, M> {
    jsonrpc: &'static str,
    #[serde(flatten)]
    message: &'a M,
}

#[derive(Deserialize)]
struct Incoming<M> {
    jsonrpc: String,
    #[serde(flatten)]
    message: M,
}

/// One message as one line of JSON, without the newline.
pub fn encode<M: Serialize>(message: &M) -> Result<String, BridgeError> {
    Ok(serde_json::to_string(&Outgoing {
        jsonrpc: VERSION,
        message,
    })?)
}

/// Parses one line. Unknown methods, missing fields and extra fields are all
/// errors: both ends ship together, so a mismatch is a bug to surface.
pub fn decode<M: DeserializeOwned>(line: &str) -> Result<M, BridgeError> {
    let incoming: Incoming<M> = serde_json::from_str(line)?;
    if incoming.jsonrpc != VERSION {
        return Err(BridgeError::Version(incoming.jsonrpc));
    }
    Ok(incoming.message)
}

#[cfg(test)]
mod tests {
    use std::fs;
    use std::path::{Path, PathBuf};

    use serde_json::Value;

    use super::*;

    fn schema_dir() -> PathBuf {
        Path::new(env!("CARGO_MANIFEST_DIR")).join("../../schema")
    }

    fn read(path: &Path) -> String {
        fs::read_to_string(path).unwrap_or_else(|err| panic!("{}: {err}", path.display()))
    }

    fn examples(directory: &str) -> Vec<(String, String)> {
        let mut found: Vec<_> = fs::read_dir(schema_dir().join("examples").join(directory))
            .unwrap()
            .map(|entry| entry.unwrap().path())
            .filter(|path| path.extension().is_some_and(|ext| ext == "json"))
            .map(|path| {
                let name = path.file_name().unwrap().to_string_lossy().into_owned();
                (name, read(&path))
            })
            .collect();
        found.sort();
        assert!(!found.is_empty(), "no examples in {directory}");
        found
    }

    /// The schema restricted to one direction, with every definition in scope.
    fn validator(direction: &str) -> jsonschema::Validator {
        let schema: Value = serde_json::from_str(&read(&schema_dir().join("bridge.json"))).unwrap();
        let one_direction = serde_json::json!({
            "$schema": schema["$schema"],
            "$ref": format!("#/$defs/{direction}"),
            "$defs": schema["$defs"],
        });
        jsonschema::validator_for(&one_direction).unwrap()
    }

    /// Every example is valid by the schema, parses into the Rust type, and
    /// comes back out as the same JSON.
    fn round_trip<M>(directory: &str, direction: &str)
    where
        M: Serialize + DeserializeOwned,
    {
        let validator = validator(direction);
        for (name, text) in examples(directory) {
            let value: Value = serde_json::from_str(&text).unwrap();
            let problems: Vec<String> = validator
                .iter_errors(&value)
                .map(|err| err.to_string())
                .collect();
            assert!(
                problems.is_empty(),
                "{name} breaks the schema: {problems:?}"
            );

            let message: M = decode(&text).unwrap_or_else(|err| panic!("{name}: {err}"));
            let line = encode(&message).unwrap();
            assert!(!line.contains('\n'), "{name} does not encode to one line");
            let again: Value = serde_json::from_str(&line).unwrap();
            assert_eq!(again, value, "{name} does not round-trip");
        }
    }

    #[test]
    fn core_to_shell_examples_match_the_schema_and_the_types() {
        round_trip::<CoreToShell>("core-to-shell", "coreToShell");
    }

    #[test]
    fn shell_to_core_examples_match_the_schema_and_the_types() {
        round_trip::<ShellToCore>("shell-to-core", "shellToCore");
    }

    #[test]
    fn every_method_in_the_schema_has_an_example() {
        let schema: Value = serde_json::from_str(&read(&schema_dir().join("bridge.json"))).unwrap();
        for (directory, direction) in [
            ("core-to-shell", "coreToShell"),
            ("shell-to-core", "shellToCore"),
        ] {
            let mut declared: Vec<String> = schema["$defs"][direction]["oneOf"]
                .as_array()
                .unwrap()
                .iter()
                .map(|entry| {
                    let reference = entry["$ref"].as_str().unwrap();
                    reference.trim_start_matches("#/$defs/").to_owned()
                })
                .collect();
            declared.sort();
            let exemplified: Vec<String> = examples(directory)
                .into_iter()
                .map(|(name, _)| name.trim_end_matches(".json").to_owned())
                .collect();
            assert_eq!(declared, exemplified, "{directory}");
        }
    }

    #[test]
    fn a_direction_does_not_accept_the_other_directions_messages() {
        let core_to_shell = validator("coreToShell");
        for (name, text) in examples("shell-to-core") {
            let value: Value = serde_json::from_str(&text).unwrap();
            assert!(!core_to_shell.is_valid(&value), "{name}");
            assert!(decode::<CoreToShell>(&text).is_err(), "{name}");
        }
        let shell_to_core = validator("shellToCore");
        for (name, text) in examples("core-to-shell") {
            let value: Value = serde_json::from_str(&text).unwrap();
            assert!(!shell_to_core.is_valid(&value), "{name}");
            assert!(decode::<ShellToCore>(&text).is_err(), "{name}");
        }
    }

    #[test]
    fn invalid_examples_are_rejected_by_the_schema_and_by_both_types() {
        let schema: Value = serde_json::from_str(&read(&schema_dir().join("bridge.json"))).unwrap();
        let whole = jsonschema::validator_for(&schema).unwrap();
        for (name, text) in examples("invalid") {
            let value: Value = serde_json::from_str(&text).unwrap();
            assert!(!whole.is_valid(&value), "the schema accepts {name}");
            assert!(
                decode::<CoreToShell>(&text).is_err(),
                "CoreToShell accepts {name}"
            );
            assert!(
                decode::<ShellToCore>(&text).is_err(),
                "ShellToCore accepts {name}"
            );
        }
    }

    #[test]
    fn the_version_is_checked() {
        let line = r#"{"jsonrpc":"1.0","method":"pane.close","params":{}}"#;
        assert!(matches!(
            decode::<CoreToShell>(line),
            Err(BridgeError::Version(version)) if version == "1.0"
        ));
        assert!(matches!(
            decode::<CoreToShell>("not json"),
            Err(BridgeError::Malformed(_))
        ));
    }

    #[test]
    fn optional_fields_are_left_out_rather_than_null() {
        let line = encode(&CoreToShell::CommandsList(CommandsList {
            commands: vec![CommandInfo {
                name: "nav.next".to_owned(),
                title: "Next".to_owned(),
                keys: vec!["j".to_owned()],
                menu: None,
                symbol: None,
            }],
        }))
        .unwrap();
        assert!(!line.contains("null"), "{line}");
        assert!(!line.contains("menu"), "{line}");
    }
}
