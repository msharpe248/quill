//! Changing a command's keys from the settings window, from the UI's side:
//! what the window lists, what the question about a key says of the key
//! that was pressed, and what is asked of the service.
//!
//! The service here is the harness: what the UI asked of it is looked at,
//! and its answers are handed in. Writing `keys.toml` is tested where it
//! is done, in `quill-cli`.

// Test helpers outside `#[test]` functions may unwrap too.
#![allow(clippy::unwrap_used, clippy::expect_used)]

mod common;

use common::Harness;
use quill_core::config::KeymapPreset;
use quill_host::bridge::{FormAction, FormField, FormRow, FormShow, FormUsed, SettingsUsed};
use quill_tui::command::{CommandId, Context, Registry};
use quill_tui::keymap::Keymap;
use quill_tui::settings::{KeysChange, Settings, SettingsRead};
use quill_tui::setup::{AccountEvent, AccountRequest};
use quill_tui::state::{Action, SettingsAct};
use quill_tui::theme::Theme;

/// What the settings file says, with these lines of the user's own keys
/// on top of the Vim keys.
fn read_with(own: &str) -> Box<SettingsRead> {
    let config = quill_core::config::Config::parse("").unwrap();
    let keymap = Keymap::preset(KeymapPreset::Vim)
        .with_overrides(own, &Registry::builtin())
        .unwrap();
    Box::new(SettingsRead {
        settings: Settings::of(&config),
        accounts: Vec::new(),
        themes: Theme::builtin_names().map(str::to_owned).collect(),
        theme_light: Theme::builtin("light").unwrap(),
        theme_light_problem: None,
        theme_dark: Theme::builtin("dark").unwrap(),
        theme_dark_problem: None,
        keymap,
        keys_problem: None,
    })
}

/// Quill's app with the settings window open on what `own` comes to.
fn window(own: &str) -> Harness {
    let mut harness = Harness::with_accounts(160, 40);
    harness.env.sheets = true;
    harness.act(Action::Settings(SettingsAct::Opened));
    answer(&mut harness, read_with(own));
    harness
}

fn answer(harness: &mut Harness, read: Box<SettingsRead>) {
    harness.act(Action::Account(AccountEvent::Settings {
        read: Ok(read),
        changed: None,
    }));
}

fn bindings(harness: &Harness) -> (Vec<FormRow>, Vec<FormAction>, bool) {
    let shown = harness.state.settings_shown(&harness.env).unwrap().1;
    match shown.field("keys", "bindings").unwrap().clone() {
        FormField::List {
            rows,
            actions,
            searchable,
            ..
        } => (rows, actions, searchable),
        other => panic!("{other:?}"),
    }
}

fn row(harness: &Harness, id: &str) -> FormRow {
    bindings(harness)
        .0
        .into_iter()
        .find(|row| row.id == id)
        .unwrap_or_else(|| panic!("no row {id}"))
}

fn press(harness: &mut Harness, action: &str, row: Option<&str>) {
    let used = SettingsUsed::action("keys", "bindings", action, row).unwrap();
    harness.act(Action::Settings(SettingsAct::Used(used)));
}

fn sheet(harness: &Harness) -> FormShow {
    harness.state.form(&harness.env).unwrap()
}

/// What the sheet's recorder and choice hold, told to the core as the
/// shell tells it: with a button, or with none when one of them changed.
fn tell(harness: &mut Harness, keys: &str, how: &str, button: Option<&str>) {
    let step = sheet(harness).step().to_owned();
    harness.act(Action::FormUsed(
        FormUsed::new(&step, button, &[("keys", keys), ("how", how)], &[]).unwrap(),
    ));
}

fn asked(harness: &Harness) -> Vec<&KeysChange> {
    harness
        .account_requests
        .iter()
        .filter_map(|request| match request {
            AccountRequest::Keys(change) => Some(change),
            _ => None,
        })
        .collect()
}

fn save_enabled(form: &FormShow) -> bool {
    form.buttons()
        .iter()
        .find(|button| button.id == "confirm")
        .unwrap()
        .enabled
}

#[test]
fn the_window_lists_every_command_with_its_keys_and_says_which_are_the_users_own() {
    let harness = window("\"view.zoom\" = \"Z\"\n\"message.toggle_star\" = []\n");
    let (rows, actions, searchable) = bindings(&harness);
    // Every command there is, each once, in a list long enough to search.
    assert_eq!(rows.len(), Registry::builtin().specs().len());
    assert!(searchable);
    let moving = row(&harness, "message_move");
    assert_eq!(
        (moving.title.as_str(), moving.detail.as_deref()),
        ("Move to a folder", Some("m"))
    );
    // One that has the keys it comes with can be changed, and has nothing
    // to be put back to.
    assert_eq!(moving.actions, ["change"]);
    assert_eq!(row(&harness, "nav_next").detail.as_deref(), Some("j, down"));
    // The user's own are marked, and can be put back.
    let zoom = row(&harness, "view_zoom");
    assert_eq!(zoom.detail.as_deref(), Some("Z \u{b7} yours"));
    assert_eq!(zoom.actions, ["change", "reset"]);
    assert_eq!(
        row(&harness, "message_toggle_star").detail.as_deref(),
        Some("No key \u{b7} yours")
    );
    let titled: Vec<(&str, &str, bool, bool)> = actions
        .iter()
        .map(|action| {
            (
                action.id.as_str(),
                action.title.as_str(),
                action.row,
                action.enabled,
            )
        })
        .collect();
    assert_eq!(
        titled,
        [
            ("change", "Change…", true, true),
            ("reset", "Use the Default", true, true),
            ("reset_all", "Reset All…", false, true),
        ]
    );

    // With nothing of the user's own there is nothing to reset.
    let plain = window("");
    let (rows, actions, _) = bindings(&plain);
    assert!(rows.iter().all(|row| row.actions == ["change"]));
    assert!(!actions[2].enabled);
}

#[test]
fn a_key_is_pressed_for_a_command_and_the_service_is_asked_to_write_it() {
    let mut harness = window("");
    press(&mut harness, "change", Some("message_move"));
    assert_eq!(harness.state.context(), Context::RecordKeys);
    let form = sheet(&harness);
    assert_eq!(form.step(), "keys_change");
    assert_eq!(form.title(), "Keys for Move to a Folder");
    assert_eq!(form.note(), Some("It has: m."));
    match form.fields() {
        [
            FormField::Keys {
                id, value, focus, ..
            },
            FormField::Choice {
                id: how,
                value: chosen,
                options,
                ..
            },
        ] => {
            assert_eq!((id.as_str(), value.as_str(), *focus), ("keys", "", true));
            assert_eq!((how.as_str(), chosen.as_str()), ("how", "replace"));
            let ids: Vec<&str> = options.iter().map(|option| option.id.as_str()).collect();
            assert_eq!(ids, ["replace", "add", "clear"]);
        }
        other => panic!("{other:?}"),
    }
    // Nothing pressed yet: nothing to save.
    assert!(!save_enabled(&form));
    tell(&mut harness, "", "replace", Some("confirm"));
    assert!(asked(&harness).is_empty());
    assert_eq!(harness.state.context(), Context::RecordKeys);

    // A key nothing else has: it is shown, and can be saved.
    tell(&mut harness, "ctrl-b", "replace", None);
    let form = sheet(&harness);
    assert_eq!(form.fields()[0].value(), Some("ctrl-b"));
    assert_eq!(form.note(), Some("It has: m."));
    assert_eq!(form.error(), None);
    assert!(save_enabled(&form));
    tell(&mut harness, "ctrl-b", "replace", Some("confirm"));
    assert_eq!(
        asked(&harness),
        [&KeysChange::Set(vec![(
            "message.move".to_owned(),
            vec!["ctrl-b".to_owned()]
        )])]
    );
    // The question is over; the window is still there.
    assert!(harness.state.keys.is_none());
    assert!(harness.state.settings_shown(&harness.env).is_some());

    // The service's answer is the keys as they are now: they hold at once,
    // here and in the window.
    harness.act(Action::Account(AccountEvent::KeysChanged {
        read: Ok(read_with("\"message.move\" = \"ctrl-b\"\n")),
        result: Ok(()),
    }));
    let now: Vec<String> = harness
        .env
        .keymap
        .keys_for(CommandId::MessageMove)
        .iter()
        .map(ToString::to_string)
        .collect();
    assert_eq!(now, ["ctrl-b"]);
    assert_eq!(
        row(&harness, "message_move").detail.as_deref(),
        Some("ctrl-b \u{b7} yours")
    );

    // Added to the keys it has, or left with none.
    press(&mut harness, "change", Some("nav_next"));
    tell(&mut harness, "ctrl-n", "add", Some("confirm"));
    assert_eq!(
        asked(&harness).last().unwrap(),
        &&KeysChange::Set(vec![(
            "nav.next".to_owned(),
            vec!["j".to_owned(), "down".to_owned(), "ctrl-n".to_owned()]
        )])
    );
    press(&mut harness, "change", Some("view_zoom"));
    tell(&mut harness, "", "clear", None);
    assert!(save_enabled(&sheet(&harness)));
    tell(&mut harness, "", "clear", Some("confirm"));
    assert_eq!(
        asked(&harness).last().unwrap(),
        &&KeysChange::Set(vec![("view.zoom".to_owned(), Vec::new())])
    );
}

#[test]
fn a_key_another_command_has_is_taken_from_it_and_one_that_cannot_be_had_is_refused() {
    let mut harness = window("");
    press(&mut harness, "change", Some("message_move"));
    // Archive has `e`, where a message is moved from: said before saving.
    tell(&mut harness, "e", "replace", None);
    let form = sheet(&harness);
    assert_eq!(
        form.note(),
        Some(
            "It has: m. e is the key for Archive in the same place now. Saving takes it from \
             there."
        )
    );
    assert_eq!(form.error(), None);
    assert!(save_enabled(&form));
    tell(&mut harness, "e", "replace", Some("confirm"));
    assert_eq!(
        asked(&harness),
        [&KeysChange::Set(vec![
            ("message.move".to_owned(), vec!["e".to_owned()]),
            ("message.archive".to_owned(), Vec::new()),
        ])]
    );

    // A key that is how another begins would cut that one short.
    press(&mut harness, "change", Some("message_move"));
    tell(&mut harness, "g", "replace", None);
    let form = sheet(&harness);
    let why = form.error().unwrap();
    assert!(
        why.contains("g cannot be a key by itself here: it is how g g begins"),
        "{why}"
    );
    assert!(!save_enabled(&form));
    // Nor is it saved by asking.
    let before = asked(&harness).len();
    tell(&mut harness, "g", "replace", Some("confirm"));
    assert_eq!(asked(&harness).len(), before);
    assert_eq!(harness.state.context(), Context::RecordKeys);
    // And one that begins with another's key would never be reached.
    tell(&mut harness, "j x", "replace", None);
    let why = sheet(&harness).error().unwrap().to_owned();
    assert!(why.contains("j x would never be reached"), "{why}");
    // What is no key at all is said so.
    tell(&mut harness, "ctrl-", "replace", None);
    assert!(
        sheet(&harness)
            .error()
            .unwrap()
            .contains("not a key Quill can use here")
    );
    // A key used somewhere this command is not is nobody's to give up:
    // `O` opens an attachment where attachments are listed, and a message
    // is not moved from there.
    tell(&mut harness, "O", "replace", None);
    let form = sheet(&harness);
    assert_eq!(form.note(), Some("It has: m."));
    assert!(save_enabled(&form));
    // Cancel changes nothing.
    harness.act(Action::FormUsed(
        FormUsed::new("keys_change", Some("cancel"), &[], &[]).unwrap(),
    ));
    assert!(harness.state.keys.is_none());
    assert_eq!(asked(&harness).len(), before);
}

#[test]
fn a_command_is_put_back_at_once_and_all_of_them_after_asking() {
    let mut harness = window("\"view.zoom\" = \"Z\"\n");
    press(&mut harness, "reset", Some("view_zoom"));
    // One line of a file: not asked about.
    assert!(harness.state.keys.is_none());
    assert_eq!(
        asked(&harness),
        [&KeysChange::Reset(vec!["view.zoom".to_owned()])]
    );

    press(&mut harness, "reset_all", None);
    let form = sheet(&harness);
    assert_eq!(form.step(), "keys_reset");
    assert_eq!(form.title(), "Reset All Keys?");
    assert!(form.fields().is_empty());
    // No button answers to return.
    assert!(
        form.buttons()
            .iter()
            .all(|button| button.role.is_none() || button.id == "cancel")
    );
    assert_eq!(asked(&harness).len(), 1);
    harness.act(Action::FormUsed(
        FormUsed::new("keys_reset", Some("confirm"), &[], &[]).unwrap(),
    ));
    assert_eq!(asked(&harness).last().unwrap(), &&KeysChange::ResetAll);
    assert!(harness.state.keys.is_none());

    // A change the service could not make is said in the window, and the
    // keys are as they were.
    harness.act(Action::Account(AccountEvent::KeysChanged {
        read: Ok(read_with("\"view.zoom\" = \"Z\"\n")),
        result: Err("keys.toml cannot be written: Permission denied".to_owned()),
    }));
    let shown = harness.state.settings_shown(&harness.env).unwrap().1;
    assert_eq!(
        shown.problem(),
        Some("keys.toml cannot be written: Permission denied")
    );
    assert_eq!(
        row(&harness, "view_zoom").detail.as_deref(),
        Some("Z \u{b7} yours")
    );
}

#[test]
fn keys_that_could_not_be_read_are_not_changed_from_here() {
    let mut harness = Harness::with_accounts(160, 40);
    harness.env.sheets = true;
    harness.act(Action::Settings(SettingsAct::Opened));
    let mut read = read_with("");
    read.keys_problem =
        Some("keys.toml: no command is called no.such.command. Using the preset keys".to_owned());
    answer(&mut harness, read);
    let shown = harness.state.settings_shown(&harness.env).unwrap().1;
    let pane = shown.panes().iter().find(|pane| pane.id == "keys").unwrap();
    let note = pane.note.as_deref().unwrap();
    assert!(
        note.starts_with("Your own keys, in keys.toml, have a mistake in them"),
        "{note}"
    );
    assert!(note.contains("no.such.command"), "{note}");
    // The keys in force are listed, and none can be changed.
    let (rows, actions, _) = bindings(&harness);
    assert!(rows.iter().all(|row| row.actions.is_empty()));
    assert!(actions.iter().all(|action| !action.enabled));
    press(&mut harness, "change", Some("message_move"));
    assert!(harness.state.keys.is_none());
    press(&mut harness, "reset_all", None);
    assert!(harness.state.keys.is_none());
    assert!(asked(&harness).is_empty());
}
