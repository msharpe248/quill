//! The question asked before agents are let in over MCP.
//!
//! MCP is off as Quill comes. Switching it on in the settings window lets
//! any program the user gives Quill's command to read their mail and do
//! what the other switches allow, for as long as Quill runs: so the
//! question says that first, with what is switched on for them now, and
//! nothing is changed until it is answered ([`McpPrompt`]). It is asked
//! each time MCP is switched on there, and only there: in a terminal the
//! settings are the file, and whoever writes `mcp = true` in it has said
//! so themselves.

use quill_core::config::AgentConfig;

use super::{Effects, State};
use crate::settings::Change;
use crate::setup::AccountRequest;

/// The question: are agents to be let in over MCP.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct McpPrompt {
    /// What agents may do as the settings stand, in a sentence.
    pub may: String,
}

impl McpPrompt {
    pub fn title(&self) -> &'static str {
        "Let agents work your mail over MCP?"
    }

    /// What saying yes lets in.
    pub fn does(&self) -> &'static str {
        "An agent\u{2019}s program that you give Quill\u{2019}s MCP command to can then read \
         the mail on this Mac and work it, for as long as Quill is running. Nothing else can \
         reach it: only programs run by you, on this Mac."
    }

    /// What to keep in mind, and what there is to keep an eye on it with.
    pub fn mind(&self) -> &'static str {
        "Mail is written by strangers, and an agent reads it: a message can tell an agent to \
         do something you never asked for. Each program that connects is said in a \
         notification, what agents change is said in Quill and kept in the log, a change can \
         be taken back, and Stop Agents shuts them all out at once."
    }

    fn of(agent: &AgentConfig) -> Self {
        let mut can = vec!["read mail"];
        if agent.organize {
            can.push("organize it");
        }
        if agent.delete {
            can.push("delete it to the Trash");
        }
        if agent.draft {
            can.push("write drafts");
        }
        let can = match can.as_slice() {
            [only] => (*only).to_owned(),
            [first @ .., last] => format!("{} and {last}", first.join(", ")),
            [] => String::new(),
        };
        let sending = if agent.send && agent.hold_sends {
            "They may send mail too, and what they send waits in the Outbox for your OK."
        } else if agent.send {
            "They may send mail too, without asking you."
        } else if agent.draft {
            "They may not send: what they write stays in Drafts until you send it."
        } else {
            "They may not send."
        };
        Self {
            may: format!("As the switches in this pane stand, agents may {can}. {sending}"),
        }
    }
}

impl State {
    /// Asks before MCP is switched on, with what the settings let agents
    /// do said.
    pub(super) fn ask_mcp(&mut self, agent: &AgentConfig) {
        self.mcp_ask = Some(McpPrompt::of(agent));
    }

    /// The user said yes: MCP is switched on, by the service that writes
    /// the settings.
    pub(super) fn answer_mcp(&mut self, out: &mut Effects) {
        if self.mcp_ask.take().is_some() {
            out.accounts
                .push(AccountRequest::Settings(Some(Change::AgentMcp(true))));
        }
    }

    /// The user said no: MCP stays off.
    pub(super) fn cancel_mcp(&mut self) {
        self.mcp_ask = None;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_question_says_what_agents_may_do_as_the_settings_stand() {
        // As Quill comes.
        let said = McpPrompt::of(&AgentConfig::default()).may;
        assert!(said.contains(
            "agents may read mail, organize it, delete it to the Trash and write drafts."
        ));
        assert!(said.contains("stays in Drafts until you send it"));
        // Sending switched on, held or not.
        let held = McpPrompt::of(&AgentConfig {
            send: true,
            hold_sends: true,
            ..AgentConfig::default()
        });
        assert!(held.may.contains("waits in the Outbox for your OK"));
        let free = McpPrompt::of(&AgentConfig {
            send: true,
            ..AgentConfig::default()
        });
        assert!(free.may.contains("without asking you"));
        // Everything switched off: they read, and that is all.
        let least = McpPrompt::of(&AgentConfig {
            organize: false,
            delete: false,
            draft: false,
            ..AgentConfig::default()
        });
        assert!(
            least
                .may
                .contains("agents may read mail. They may not send.")
        );
    }
}
