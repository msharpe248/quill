//! The agents' contract, checked in: every tool as an MCP client is told
//! of it (`schema/agent.json`), and answers to a few requests on the demo
//! mailbox (`schema/examples/agent`). A change to either shows up as a
//! change to those files, to be read before it is committed.
//!
//! When a change is meant, write the files again with
//! `QUILL_UPDATE_SCHEMA=1 cargo test -p quill-tui --test agent_schema`.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use std::path::PathBuf;

use jiff::tz::TimeZone;
use quill_core::config::AgentConfig;
use quill_store::Store;
use quill_tui::agent::{self, Call, Caller, Code, SCHEMA, Settings, tools};
use serde_json::{Value, json};

const NOW: i64 = 1_794_648_600; // 2026-11-14T09:30:00Z

fn schema_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../schema")
        .canonicalize()
        .unwrap()
}

/// Holds `value` to the file at `path`, or writes the file when asked to.
fn held_to(path: &std::path::Path, value: &Value) {
    let text = format!("{}\n", serde_json::to_string_pretty(value).unwrap());
    if std::env::var_os("QUILL_UPDATE_SCHEMA").is_some() {
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, text).unwrap();
        return;
    }
    let on_disk = std::fs::read_to_string(path).unwrap_or_default();
    assert!(
        on_disk == text,
        "{} is not what the code says now. If the change is meant, write it again with \
         QUILL_UPDATE_SCHEMA=1 and read the diff.\n--- now ---\n{text}",
        path.display()
    );
}

#[test]
fn the_tools_are_as_the_schema_says() {
    let codes = [
        Code::SwitchedOff,
        Code::OverLimit,
        Code::NotFound,
        Code::Invalid,
        Code::NotRunning,
        Code::CannotUndo,
        Code::Failed,
    ];
    let value = json!({
        "schema": SCHEMA,
        "about": "What Quill's commands for a shell and its MCP tools take. Every answer is an \
                  object with \"schema\" in it; a refusal is {\"schema\", \"refused\": {\"code\", \
                  \"message\"}}. Examples are in schema/examples/agent.",
        "refusal_codes": codes.iter().map(|code| code.as_str()).collect::<Vec<_>>(),
        "tools": tools::TOOLS
            .iter()
            .map(|tool| {
                let mut described = tool.described();
                // A command for a shell alone: no MCP client is told of it.
                if tool.shell {
                    described["shell_only"] = json!(true);
                }
                described
            })
            .collect::<Vec<_>>(),
    });
    held_to(&schema_dir().join("agent.json"), &value);
}

#[test]
fn the_answers_are_as_the_examples_show() {
    let mut store = Store::open_in_memory().unwrap();
    tokio::runtime::Builder::new_current_thread()
        .build()
        .unwrap()
        .block_on(quill_tui::demo::seed(&mut store, NOW))
        .unwrap();
    let settings = Settings::new(AgentConfig::default(), TimeZone::UTC);
    let mut careful = settings.clone();
    careful.agent.delete = false;
    careful.agent.bulk_limit = 3;

    let mut ask = |name: &str, settings: &Settings, tool: &str, args: Value| -> Value {
        let call = Call::parse(tool, &args).unwrap();
        let answer = match agent::perform(&mut store, settings, &Caller::shell(), &call, NOW).result
        {
            Ok(answer) => answer.value,
            Err(refusal) => refusal.value(),
        };
        held_to(
            &schema_dir()
                .join("examples/agent")
                .join(format!("{name}.json")),
            &json!({ "tool": tool, "arguments": args, "answer": answer }),
        );
        answer
    };

    ask("accounts", &settings, "accounts", json!({}));
    ask(
        "folders",
        &settings,
        "folders",
        json!({ "account": "Personal" }),
    );
    let found = ask(
        "search",
        &settings,
        "search",
        json!({ "query": "from:peter dinner" }),
    );
    let id = found["messages"][0]["id"].as_i64().unwrap();
    ask(
        "read",
        &settings,
        "read",
        json!({ "id": id, "max_chars": 60 }),
    );
    ask("count", &settings, "count", json!({ "query": "is:unread" }));
    ask("status", &settings, "status", json!({}));
    ask(
        "move.dry_run",
        &settings,
        "move",
        json!({ "where": "from:peter dinner", "to": "archive", "dry_run": true }),
    );
    ask("delete", &settings, "delete", json!({ "ids": [id] }));
    ask("undo", &settings, "undo", json!({}));
    ask("log", &settings, "log", json!({ "limit": 2 }));
    ask(
        "folder_create",
        &settings,
        "folder_create",
        json!({ "name": "Receipts", "account": "Work", "on_this_mac": true }),
    );
    // What needs the Quill that is running, asked with none running.
    ask(
        "refused.not_running",
        &settings,
        "open",
        json!({ "id": id }),
    );
    ask(
        "refused.switched_off",
        &careful,
        "delete",
        json!({ "ids": [id] }),
    );
    ask(
        "refused.over_limit",
        &careful,
        "mark",
        json!({ "where": "is:unread", "as": "read" }),
    );
}

#[test]
fn a_request_with_a_mistake_in_it_is_refused_as_data() {
    let refusal = Call::parse("delete", &json!({ "id": 1 })).unwrap_err();
    held_to(
        &schema_dir().join("examples/agent/refused.invalid.json"),
        &json!({ "tool": "delete", "arguments": { "id": 1 }, "answer": refusal.value() }),
    );
}
