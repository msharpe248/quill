//! Rules, from the window: the switch, trying a rule before it is run,
//! running it, a new rule from a message, and what rules did.

// Test helpers outside `#[test]` functions may unwrap too.
#![allow(clippy::unwrap_used, clippy::expect_used)]

mod common;

use std::sync::Arc;

use common::Harness;
use quill_core::types::FolderRole;
use quill_host::bridge::{FormField, FormUsed};
use quill_store::Mailbox;
use quill_sync::{Rule, RuleSet, SyncEvent, SyncRequest};
use quill_tui::command::{CommandId, Context};
use quill_tui::setup::{AccountEvent, AccountRequest, NewRule, Preference, RulesAsk, RulesNow};
use quill_tui::state::{Action, HostRequest};

/// Far enough back for the demo's mail, whenever the test is run: its
/// dates are fixed, and a rule's look-back is counted from now.
const LONG_AGO: i64 = 200 * 365 * 24 * 60 * 60;

fn rule(name: &str, matching: &str) -> Rule {
    Rule {
        name: name.to_owned(),
        queries: vec![quill_core::search::parse(matching, |_, _, _| None)],
        account: None,
        move_to: None,
        delete: false,
        mark_read: false,
        star: false,
        notify: None,
        enabled: true,
        look_back: Some(LONG_AGO),
    }
}

/// The demo mailbox with these rules read.
fn with_rules(rules: Vec<Rule>) -> Harness {
    let mut harness = Harness::demo(200, 60);
    tell(&mut harness, RuleSet::of(rules), None);
    harness
}

fn tell(harness: &mut Harness, set: RuleSet, problem: Option<&str>) {
    harness.act(Action::Account(AccountEvent::Rules(RulesNow {
        set: Arc::new(set),
        problem: problem.map(str::to_owned),
    })));
}

/// The subjects in the folder of `account` with `role`.
fn in_folder(harness: &Harness, account: &str, role: FolderRole) -> Vec<String> {
    let account = harness
        .store
        .accounts()
        .unwrap()
        .into_iter()
        .find(|row| row.name == account)
        .unwrap()
        .id;
    let folder = harness
        .store
        .folders()
        .unwrap()
        .into_iter()
        .find(|folder| folder.account == account && folder.role == role)
        .unwrap()
        .id;
    harness
        .store
        .messages(Mailbox::Folder(folder))
        .unwrap()
        .into_iter()
        .map(|row| row.headers.subject)
        .collect()
}

fn status(harness: &Harness) -> String {
    harness.screen().lines().last().unwrap().to_owned()
}

fn build_mail() -> Rule {
    Rule {
        move_to: Some("archive".to_owned()),
        star: true,
        ..rule("Build mail", "from:ci@example.com")
    }
}

#[test]
fn a_rule_is_chosen_shown_in_the_list_and_changes_nothing_until_it_is_run() {
    let mut harness = with_rules(vec![
        build_mail(),
        Rule {
            delete: true,
            enabled: false,
            ..rule("Promotions", "body:unsubscribe")
        },
    ]);
    let inbox_before = harness.state.list.rows.len();

    // The rules are listed to choose from, each with what it does, and
    // one that is off said to be.
    harness.keys("Z r");
    assert_eq!(harness.state.context(), Context::Hooks);
    let screen = harness.screen();
    assert!(screen.contains("Run a rule"), "{screen}");
    assert!(
        harness
            .row_with("Build mail")
            .contains("move to archive, star"),
        "{screen}"
    );
    assert!(
        harness
            .row_with("Promotions")
            .contains("delete (switched off)"),
        "{screen}"
    );

    // Chosen, the list is what the rule would change, and its title what
    // it would do.
    harness.keys("enter");
    assert_eq!(harness.state.context(), Context::List);
    let screen = harness.screen();
    assert!(
        screen.contains("Build mail would move 1 message to Archive, mark 1 message"),
        "{screen}"
    );
    assert_eq!(harness.state.list.rows.len(), 1);
    assert_eq!(harness.selected_subject(), "[quill] PR #42 passed");
    // Nothing has changed, and nothing is queued.
    assert_eq!(harness.store.queued_operations().unwrap(), 0);
    assert!(
        !in_folder(&harness, "Work", FolderRole::Archive)
            .contains(&"[quill] PR #42 passed".to_owned())
    );
    assert!(harness.store.agent_log(10, false).unwrap().is_empty());
    // How to run it is the first thing the bottom bar says.
    assert!(
        status(&harness).contains("Z y run the rule"),
        "{}",
        status(&harness)
    );
    assert!(
        harness
            .state
            .is_enabled(&harness.env, CommandId::RulesApply)
    );

    // Run: the mail is filed, the server is to be told, and the list is
    // the mailbox again, with what was done said.
    harness.keys("Z y");
    assert!(harness.state.search.is_none());
    assert_eq!(harness.state.list.rows.len(), inbox_before - 1);
    assert!(
        in_folder(&harness, "Work", FolderRole::Archive)
            .contains(&"[quill] PR #42 passed".to_owned())
    );
    assert!(harness.sync_requests.contains(&SyncRequest::Queue));
    assert!(
        status(&harness).contains("Rules filed 1."),
        "{}",
        status(&harness)
    );
    assert!(
        !harness
            .state
            .is_enabled(&harness.env, CommandId::RulesApply)
    );
    // It is in the log, under the rule's name.
    let log = harness.store.agent_log(10, false).unwrap();
    assert_eq!(log.len(), 1);
    assert_eq!(
        (log[0].way.as_str(), log[0].agent.as_str()),
        ("rule", "Build mail")
    );

    // The user's undo takes it back as one.
    harness.keys("U");
    assert_eq!(harness.state.list.rows.len(), inbox_before);
    assert!(
        !in_folder(&harness, "Work", FolderRole::Archive)
            .contains(&"[quill] PR #42 passed".to_owned())
    );
}

#[test]
fn leaving_a_rule_that_is_shown_changes_nothing() {
    let mut harness = with_rules(vec![build_mail()]);
    let inbox_before = harness.state.list.rows.len();
    harness.keys("Z r enter");
    assert!(harness.state.rule_trial().is_some());
    harness.keys("esc");
    assert!(harness.state.search.is_none());
    assert_eq!(harness.state.list.rows.len(), inbox_before);
    assert_eq!(harness.store.queued_operations().unwrap(), 0);
    assert!(harness.store.agent_log(10, false).unwrap().is_empty());

    // And a search started from it is a search, not the rule.
    harness.keys("Z r enter /");
    assert!(harness.state.rule_trial().is_none());
    assert_eq!(harness.state.context(), Context::Search);
    assert!(
        !harness
            .state
            .is_enabled(&harness.env, CommandId::RulesApply)
    );
}

#[test]
fn all_the_rules_that_are_on_are_tried_together_and_one_that_is_off_only_by_name() {
    let mut harness = with_rules(vec![
        build_mail(),
        Rule {
            mark_read: true,
            ..rule("Family", "subject:dinner")
        },
        Rule {
            delete: true,
            enabled: false,
            ..rule("Roadmaps", "subject:roadmap")
        },
    ]);
    harness.keys("Z R");
    let screen = harness.screen();
    assert!(
        screen.contains("All rules would change 2 messages"),
        "{screen}"
    );
    let shown: Vec<&str> = harness
        .state
        .list
        .rows
        .iter()
        .map(|row| row.headers.subject.as_str())
        .collect();
    assert!(shown.contains(&"[quill] PR #42 passed") && shown.contains(&"Sunday dinner"));
    assert!(!shown.contains(&"Q4 roadmap"));
    harness.keys("Z y");
    assert!(
        status(&harness).contains("Rules filed 1 and marked 1."),
        "{}",
        status(&harness)
    );

    // The one that is off can still be tried, by choosing it.
    harness.keys("Z r");
    harness.type_text("road");
    harness.keys("enter");
    let screen = harness.screen();
    assert!(
        screen.contains("Roadmaps would delete 1 message"),
        "{screen}"
    );
    harness.keys("Z y");
    assert!(in_folder(&harness, "Work", FolderRole::Trash).contains(&"Q4 roadmap".to_owned()));
    assert!(
        status(&harness).contains("Rules deleted 1."),
        "{}",
        status(&harness)
    );
}

#[test]
fn a_rule_that_matches_nothing_says_so_and_has_nothing_to_run() {
    let mut harness = with_rules(vec![Rule {
        delete: true,
        ..rule("Nobody", "from:nobody@example.com")
    }]);
    harness.keys("Z r enter");
    let screen = harness.screen();
    assert!(
        screen.contains("Nobody matches nothing in the inbox"),
        "{screen}"
    );
    assert!(
        !harness
            .state
            .is_enabled(&harness.env, CommandId::RulesApply)
    );
    harness.keys("Z y");
    assert!(harness.state.rule_trial().is_some());
    assert!(harness.store.agent_log(10, false).unwrap().is_empty());
}

#[test]
fn with_no_rules_there_is_nothing_to_run_and_the_file_can_still_be_opened() {
    let mut harness = Harness::demo(200, 60);
    assert!(!harness.state.is_enabled(&harness.env, CommandId::RulesRun));
    assert!(
        !harness
            .state
            .is_enabled(&harness.env, CommandId::RulesRunAll)
    );
    harness.keys("Z r");
    assert_eq!(harness.state.context(), Context::List);

    // Edit Rules asks where the file is, and opens what it is told.
    harness.keys("Z e");
    assert_eq!(
        harness.account_requests,
        [AccountRequest::Rules(RulesAsk::Open)]
    );
    // In a terminal it is said where the file is, and nothing is opened:
    // what a file is edited with there is the user's own business.
    let path = std::path::PathBuf::from("/tmp/quill-test/rules.toml");
    harness.act(Action::Account(AccountEvent::RulesFile(Ok(path.clone()))));
    assert!(
        harness
            .screen()
            .contains("Rules are written in /tmp/quill-test/rules.toml"),
        "{}",
        harness.screen()
    );
    assert!(harness.host_requests.is_empty());
    harness.keys("esc");
    // The app opens it, by the folder it knows: no path is handed over.
    harness.env.window = true;
    harness.act(Action::Account(AccountEvent::RulesFile(Ok(path))));
    assert_eq!(harness.host_requests, [HostRequest::EditRules]);
    harness.env.window = false;
    harness.act(Action::Account(AccountEvent::RulesFile(Err(
        "the folder is read-only".to_owned(),
    ))));
    assert!(
        harness
            .screen()
            .contains("The rules' file cannot be opened: the folder is read-only")
    );
}

#[test]
fn the_switch_holds_at_once_and_is_written_for_the_engine() {
    let mut harness = with_rules(vec![build_mail()]);
    assert!(harness.state.is_on(CommandId::RulesToggle));
    harness.keys("Z t");
    assert!(!harness.state.is_on(CommandId::RulesToggle));
    assert_eq!(
        harness.account_requests,
        [AccountRequest::Remember(Preference::Rules(false))]
    );
    assert!(
        status(&harness).contains("Rules are off"),
        "{}",
        status(&harness)
    );
    // Off, a rule can still be run by hand.
    assert!(harness.state.is_enabled(&harness.env, CommandId::RulesRun));

    harness.act(Action::Command(CommandId::RulesToggle));
    assert!(harness.state.is_on(CommandId::RulesToggle));
    assert_eq!(
        harness.account_requests.last(),
        Some(&AccountRequest::Remember(Preference::Rules(true)))
    );
    assert!(
        status(&harness).contains("Rules are on."),
        "{}",
        status(&harness)
    );
}

#[test]
fn a_new_rule_starts_from_the_message_and_is_tried_once_saved() {
    let mut harness = Harness::demo(200, 60);
    assert_eq!(harness.selected_subject(), "Q4 roadmap");
    harness.keys("Z n");
    assert_eq!(harness.state.context(), Context::NewRule);
    let screen = harness.screen();
    assert!(screen.contains("New rule"), "{screen}");
    assert!(
        harness.row_with("Name:").contains("From Ana Ruiz"),
        "{screen}"
    );
    assert!(
        harness
            .row_with("Matching:")
            .contains("from:ana.ruiz@example.com"),
        "{screen}"
    );
    assert!(
        harness.row_with("Do:").contains("Move to a folder"),
        "{screen}"
    );
    // The demo has two accounts: the rule is for the one the message is
    // in, whose folder it will name.
    assert!(
        harness.row_with("For:").contains("mail in Work"),
        "{screen}"
    );

    // Saved with no folder named, it says what is missing and stays.
    harness.keys("enter");
    assert!(
        harness
            .screen()
            .contains("Name the folder to move the mail to.")
    );
    assert!(harness.account_requests.is_empty());

    // tab goes on through the fields; the folder is typed.
    harness.keys("tab tab tab");
    harness.type_text("Plans");
    harness.keys("enter");
    assert_eq!(
        harness.account_requests,
        [AccountRequest::Rules(RulesAsk::Add(NewRule {
            name: "From Ana Ruiz".to_owned(),
            matching: "from:ana.ruiz@example.com".to_owned(),
            account: Some("Work".to_owned()),
            move_to: Some("Plans".to_owned()),
            delete: false,
            mark_read: false,
        }))]
    );
    assert!(harness.screen().contains("Saving"));
    // While it is being written the form takes nothing more.
    harness.keys("enter");
    assert_eq!(harness.account_requests.len(), 1);

    // It could not be written: the form is still there, and says why.
    harness.act(Action::Account(AccountEvent::RuleAdded(Err(
        "There is a rule called \"From Ana Ruiz\" already.".to_owned(),
    ))));
    assert_eq!(harness.state.context(), Context::NewRule);
    assert!(harness.screen().contains("already"));

    // Written: the rules are read again, and the new one is shown with
    // what it would match. Nothing is changed until it is run.
    harness.keys("enter");
    tell(
        &mut harness,
        RuleSet::of(vec![Rule {
            account: Some("Work".to_owned()),
            move_to: Some("archive".to_owned()),
            ..rule("From Ana Ruiz", "from:ana.ruiz@example.com")
        }]),
        None,
    );
    harness.act(Action::Account(AccountEvent::RuleAdded(Ok(
        "From Ana Ruiz".to_owned(),
    ))));
    assert!(harness.state.new_rule.is_none());
    let screen = harness.screen();
    assert!(screen.contains("From Ana Ruiz would move"), "{screen}");
    assert!(harness.state.list.rows.iter().all(|row| {
        row.headers
            .from
            .first()
            .is_some_and(|from| from.email == "ana.ruiz@example.com")
    }));
    assert_eq!(harness.store.queued_operations().unwrap(), 0);
}

#[test]
fn a_new_rule_can_delete_or_mark_and_then_asks_for_no_folder() {
    let mut harness = Harness::demo(200, 60);
    harness.keys("Z n tab tab");
    // On what it does, the side arrows and space change it.
    harness.keys("right");
    assert!(
        harness
            .row_with("Do:")
            .contains("Delete (move to the Trash)")
    );
    assert!(!harness.screen().contains("Folder:"));
    harness.keys("space");
    assert!(harness.row_with("Do:").contains("Mark as read"));
    harness.keys("left");
    assert!(harness.row_with("Do:").contains("Delete"));
    harness.keys("enter");
    match harness.account_requests.as_slice() {
        [AccountRequest::Rules(RulesAsk::Add(new))] => {
            assert!(new.delete && new.move_to.is_none() && !new.mark_read);
        }
        other => panic!("{other:?}"),
    }

    // esc puts the form away, and nothing more is asked.
    let mut harness = Harness::demo(200, 60);
    harness.keys("Z n esc");
    assert_eq!(harness.state.context(), Context::List);
    assert!(harness.account_requests.is_empty());
}

#[test]
fn in_the_app_the_new_rule_is_a_sheet_with_the_same_fields() {
    let mut harness = Harness::demo(200, 60);
    harness.env.sheets = true;
    harness.keys("Z n");
    let form = harness.state.form(&harness.env).unwrap();
    assert_eq!((form.step(), form.title()), ("new_rule", "New Rule"));
    let ids: Vec<&str> = form
        .fields()
        .iter()
        .map(|field| match field {
            FormField::Text { id, .. }
            | FormField::Choice { id, .. }
            | FormField::Static { id, .. } => id.as_str(),
            other => panic!("{other:?}"),
        })
        .collect();
    assert_eq!(ids, ["name", "match", "does", "folder", "account"]);
    let buttons: Vec<(&str, &str)> = form
        .buttons()
        .iter()
        .map(|button| (button.id.as_str(), button.title.as_str()))
        .collect();
    assert_eq!(buttons, [("cancel", "Cancel"), ("confirm", "Save")]);
    // The terminal's own keys are off while the sheet has the form.
    assert!(!harness.state.is_enabled(&harness.env, CommandId::NavOpen));

    // What the rule does is changed: the folder goes with it.
    harness.act(Action::FormUsed(
        FormUsed::new(
            "new_rule",
            None,
            &[
                ("name", "Ana"),
                ("match", "from:ana.ruiz@example.com subject:roadmap"),
                ("does", "mark_read"),
                ("folder", ""),
            ],
            &[],
        )
        .unwrap(),
    ));
    let form = harness.state.form(&harness.env).unwrap();
    assert!(
        !form
            .fields()
            .iter()
            .any(|field| matches!(field, FormField::Text { id, .. } if id == "folder"))
    );
    assert!(harness.account_requests.is_empty());

    // Save sends what the sheet's fields held.
    harness.act(Action::FormUsed(
        FormUsed::new(
            "new_rule",
            Some("confirm"),
            &[
                ("name", "Ana"),
                ("match", "from:ana.ruiz@example.com subject:roadmap"),
                ("does", "mark_read"),
            ],
            &[],
        )
        .unwrap(),
    ));
    assert_eq!(
        harness.account_requests,
        [AccountRequest::Rules(RulesAsk::Add(NewRule {
            name: "Ana".to_owned(),
            matching: "from:ana.ruiz@example.com subject:roadmap".to_owned(),
            account: Some("Work".to_owned()),
            move_to: None,
            delete: false,
            mark_read: true,
        }))]
    );
    let form = harness.state.form(&harness.env).unwrap();
    assert!(form.busy().is_some());

    // Cancel, once it has failed, puts it away.
    harness.act(Action::Account(AccountEvent::RuleAdded(Err(
        "rules.toml cannot be written".to_owned(),
    ))));
    let form = harness.state.form(&harness.env).unwrap();
    assert_eq!(form.error(), Some("rules.toml cannot be written"));
    harness.act(Action::FormUsed(
        FormUsed::new("new_rule", Some("cancel"), &[], &[]).unwrap(),
    ));
    assert!(harness.state.new_rule.is_none());
}

#[test]
fn what_rules_did_is_listed_and_one_can_be_taken_back_from_the_list() {
    let mut harness = with_rules(vec![build_mail()]);
    // Nothing yet.
    harness.keys("Z l");
    assert!(status(&harness).contains("Rules have done nothing yet."));
    assert_eq!(harness.state.context(), Context::List);

    let inbox_before = harness.state.list.rows.len();
    harness.keys("Z r enter Z y");
    assert_eq!(harness.state.list.rows.len(), inbox_before - 1);

    harness.keys("Z l");
    assert_eq!(harness.state.context(), Context::Hooks);
    let screen = harness.screen();
    assert!(screen.contains("What rules did"), "{screen}");
    assert!(
        harness
            .row_with("Build mail")
            .contains("moved 1 message to Archive, marked 1 message"),
        "{screen}"
    );

    // enter takes that change back: the mail is in the inbox again, as
    // it was.
    harness.keys("enter");
    assert_eq!(harness.state.context(), Context::List);
    assert_eq!(harness.state.list.rows.len(), inbox_before);
    assert!(
        status(&harness).contains("Took back change"),
        "{}",
        status(&harness)
    );
    // And the list says so the next time.
    harness.keys("Z l");
    assert!(harness.row_with("Build mail").contains("(taken back)"));
    // Taken back twice, it says that it was already.
    harness.keys("enter");
    assert!(
        status(&harness).contains("was taken back already"),
        "{}",
        status(&harness)
    );
}

#[test]
fn what_the_engines_rules_do_is_said_and_a_mistake_in_the_file_once() {
    let mut harness = with_rules(vec![build_mail()]);
    harness.act(Action::Sync(SyncEvent::Ruled {
        said: "Rules filed 3".to_owned(),
    }));
    assert!(
        status(&harness).contains("Rules filed 3."),
        "{}",
        status(&harness)
    );

    harness.act(Action::Sync(SyncEvent::RulesWaiting {
        account: "Work".to_owned(),
        count: 212,
    }));
    let screen = harness.screen();
    assert!(
        screen.contains("Rules would change 212 messages in Work's inbox"),
        "{screen}"
    );
    harness.keys("esc");

    // A mistake in the file is a notice, said when it appears and not
    // again while it stays the same.
    let problem = "rules.toml: line 4: unknown field `forward`. None of the rules in it run \
                   until it is put right.";
    tell(&mut harness, RuleSet::default(), Some(problem));
    assert!(harness.screen().contains("unknown field `forward`"));
    harness.keys("esc");
    tell(&mut harness, RuleSet::default(), Some(problem));
    assert!(!harness.screen().contains("unknown field `forward`"));
    // With the rules gone there is nothing to choose.
    assert!(!harness.state.is_enabled(&harness.env, CommandId::RulesRun));
}

#[test]
fn a_rule_on_show_follows_the_rules_as_they_are_read_again() {
    let mut harness = with_rules(vec![build_mail()]);
    harness.keys("Z r enter");
    assert_eq!(harness.state.list.rows.len(), 1);
    // The file is saved with the rule matching something else.
    tell(
        &mut harness,
        RuleSet::of(vec![Rule {
            move_to: Some("archive".to_owned()),
            ..rule("Build mail", "subject:invoice")
        }]),
        None,
    );
    assert_eq!(harness.selected_subject(), "Invoice 2026-114");
    assert!(
        harness
            .screen()
            .contains("Build mail would move 1 message to Archive")
    );
}

#[test]
fn the_rules_commands_are_in_a_menu_of_their_own_with_keys_in_both_presets() {
    use quill_core::config::KeymapPreset;
    use quill_tui::command::{Menu, Registry};
    use quill_tui::keymap::Keymap;
    let registry = Registry::builtin();
    let rules: Vec<&str> = registry
        .specs()
        .iter()
        .filter(|spec| spec.menu == Some(Menu::Rules))
        .map(|spec| spec.title)
        .collect();
    assert_eq!(
        rules,
        [
            "Rules On",
            "Run a Rule\u{2026}",
            "Run All Rules\u{2026}",
            "Run the Rule Shown",
            "New Rule from This Message\u{2026}",
            "Edit Rules",
            "What Rules Did",
        ]
    );
    for preset in [KeymapPreset::Vim, KeymapPreset::Mutt] {
        let keymap = Keymap::preset(preset);
        for spec in registry
            .specs()
            .iter()
            .filter(|spec| spec.menu == Some(Menu::Rules))
        {
            assert!(
                !keymap.keys_for(spec.id).is_empty(),
                "{} has no key in {preset:?}",
                spec.name
            );
        }
    }
}
