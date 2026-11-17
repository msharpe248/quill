//! Changing a theme's colours and deleting a theme from the settings
//! window, from the UI's side: what the window lists, what the question
//! shows and takes, what Quill is drawn in meanwhile, and what is asked of
//! the service.
//!
//! The service here is the harness: what the UI asked of it is looked at,
//! and its answers are handed in. Writing and deleting the files is tested
//! where it is done, in `quill-cli`.

// Test helpers outside `#[test]` functions may unwrap too.
#![allow(clippy::unwrap_used, clippy::expect_used)]

mod common;

use common::Harness;
use quill_core::config::KeymapPreset;
use quill_host::bridge::{FormAction, FormField, FormRow, FormShow, FormUsed, SettingsUsed};
use quill_tui::command::Context;
use quill_tui::keymap::Keymap;
use quill_tui::settings::{Settings, SettingsRead, ThemeLine, ThemesChange};
use quill_tui::setup::{AccountEvent, AccountRequest};
use quill_tui::state::{Action, HostRequest, SettingsAct};
use quill_tui::theme::{ColorSpec, Theme};

/// One of the user's own, dark, with a colour left to the terminal.
const EVENING: &str = "name = \"Evening\"\n[colors]\nbackground = \"#101018\"\n\
                       text = \"#e0e0f0\"\ndim = \"bright-black\"\n";

fn evening() -> Theme {
    Theme::parse(EVENING).unwrap()
}

/// What the settings file says: the themes Quill comes with, one of the
/// user's own, and one of theirs that cannot be used. `config` is the file.
fn read_with(config: &str) -> Box<SettingsRead> {
    let config = quill_core::config::Config::parse(config).unwrap();
    let settings = Settings::of(&config);
    let named = |name: &str| {
        if name == "Evening" {
            evening()
        } else {
            Theme::builtin(name).unwrap()
        }
    };
    let mut themes = ThemeLine::builtins();
    themes.push(ThemeLine {
        name: "Evening".to_owned(),
        own: true,
        theme: Ok(evening()),
    });
    themes.push(ThemeLine {
        name: "broken".to_owned(),
        own: true,
        theme: Err("themes/broken.toml: theme line 3: unknown colour \"mauve\"".to_owned()),
    });
    Box::new(SettingsRead {
        theme_light: named(&settings.theme_light),
        theme_light_problem: None,
        theme_dark: named(&settings.theme_dark),
        theme_dark_problem: None,
        settings,
        accounts: Vec::new(),
        themes,
        keymap: Keymap::preset(KeymapPreset::Vim),
        keys_problem: None,
    })
}

/// Quill's app with the settings window open on what `config` comes to.
fn window(config: &str) -> Harness {
    let mut harness = Harness::with_accounts(160, 40);
    harness.env.sheets = true;
    harness.act(Action::Settings(SettingsAct::Opened));
    harness.act(Action::Account(AccountEvent::Settings {
        read: Ok(read_with(config)),
        changed: None,
    }));
    harness
}

fn themes(harness: &Harness) -> (Vec<FormRow>, Vec<FormAction>) {
    let shown = harness.state.settings_shown(&harness.env).unwrap().1;
    match shown.field("appearance", "themes").unwrap().clone() {
        FormField::List { rows, actions, .. } => (rows, actions),
        other => panic!("{other:?}"),
    }
}

fn row(harness: &Harness, title: &str) -> FormRow {
    themes(harness)
        .0
        .into_iter()
        .find(|row| row.title == title)
        .unwrap_or_else(|| panic!("no row {title}"))
}

/// Presses one of the list's buttons, with the row of this title picked.
fn press(harness: &mut Harness, action: &str, title: Option<&str>) {
    let id = title.map(|title| row(harness, title).id);
    let used = SettingsUsed::action("appearance", "themes", action, id.as_deref()).unwrap();
    harness.act(Action::Settings(SettingsAct::Used(used)));
}

fn sheet(harness: &Harness) -> FormShow {
    harness.state.form(&harness.env).unwrap()
}

/// What the sheet's fields hold, as the core last said, with these put in
/// their place: told to the core as the shell tells it, with a button, or
/// with none when a colour was picked.
fn tell(harness: &mut Harness, changed: &[(&str, &str)], button: Option<&str>) {
    let form = sheet(harness);
    let mut values: Vec<(String, String)> = form
        .fields()
        .iter()
        .filter_map(|field| Some((field.id().to_owned(), field.value()?.to_owned())))
        .collect();
    for (id, value) in changed {
        let held = values
            .iter_mut()
            .find(|(field, _)| field == id)
            .unwrap_or_else(|| panic!("no field {id}"));
        held.1 = (*value).to_owned();
    }
    let values: Vec<(&str, &str)> = values
        .iter()
        .map(|(id, value)| (id.as_str(), value.as_str()))
        .collect();
    harness.act(Action::FormUsed(
        FormUsed::new(form.step(), button, &values, &[]).unwrap(),
    ));
}

fn well(form: &FormShow, id: &str) -> String {
    match form.fields().iter().find(|field| field.id() == id) {
        Some(FormField::Color { value, .. }) => value.as_str().to_owned(),
        other => panic!("{id}: {other:?}"),
    }
}

fn asked(harness: &Harness) -> Vec<&ThemesChange> {
    harness
        .account_requests
        .iter()
        .filter_map(|request| match request {
            AccountRequest::Themes(change) => Some(change),
            _ => None,
        })
        .collect()
}

#[test]
fn the_window_lists_the_themes_with_what_each_is_and_what_can_be_done_with_it() {
    let harness = window("[ui]\ntheme_dark = \"Evening\"\ntheme_light = \"light\"\n");
    let (rows, actions) = themes(&harness);
    let said: Vec<(&str, &str, Vec<&str>)> = rows
        .iter()
        .map(|row| {
            (
                row.title.as_str(),
                row.detail.as_deref().unwrap(),
                row.actions.iter().map(String::as_str).collect(),
            )
        })
        .collect();
    assert_eq!(
        said,
        [
            ("Dark", "Dark \u{b7} comes with Quill", vec!["edit"]),
            (
                "Light",
                "Light \u{b7} comes with Quill \u{b7} the light theme",
                vec!["edit"]
            ),
            (
                "Terminal",
                "The terminal\u{2019}s own colours \u{b7} comes with Quill",
                vec!["edit"]
            ),
            (
                "Evening",
                "Dark \u{b7} the dark theme",
                vec!["edit", "delete"]
            ),
            // One that cannot be read can only go.
            (
                "broken",
                "Cannot be used. Themes/broken.toml: theme line 3: unknown colour \"mauve\".",
                vec!["delete"]
            ),
        ]
    );
    let buttons: Vec<(&str, &str, bool, bool)> = actions
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
        buttons,
        [
            ("edit", "Edit\u{2026}", true, true),
            ("delete", "Delete\u{2026}", true, true),
            ("import", "Import\u{2026}", false, true),
        ]
    );

    // Import is the host's chooser, as it was.
    let mut harness = harness;
    press(&mut harness, "import", None);
    assert!(harness.host_requests.contains(&HostRequest::PickTheme));
    assert_eq!(harness.state.context(), Context::List);

    // One Quill comes with has no file to delete: asking changes nothing.
    press(&mut harness, "delete", Some("Dark"));
    assert!(harness.state.theme_prompt.is_none());
    // And one that cannot be read has no colours to change.
    press(&mut harness, "edit", Some("broken"));
    assert!(harness.state.theme_prompt.is_none());
}

#[test]
fn a_theme_is_drawn_with_as_its_colours_are_chosen_and_put_back_when_given_up() {
    let mut harness = window("");
    assert_eq!(harness.env.styles.theme_name(), "dark");
    press(&mut harness, "edit", Some("Evening"));
    assert_eq!(harness.state.context(), Context::EditTheme);
    let form = sheet(&harness);
    assert_eq!(form.step(), "theme_edit");
    assert_eq!(form.title(), "The Theme \u{201c}Evening\u{201d}");
    // A name, and a well for every colour the theme has.
    let kinds: Vec<(&str, &str)> = form
        .fields()
        .iter()
        .map(|field| {
            (
                field.id(),
                match field {
                    FormField::Text { .. } => "text",
                    FormField::Color { .. } => "color",
                    _ => "other",
                },
            )
        })
        .collect();
    assert_eq!(kinds[0], ("name", "text"));
    assert_eq!(kinds.len(), 1 + 13 + 4);
    assert!(kinds[1..].iter().all(|(_, kind)| *kind == "color"));
    assert_eq!(kinds[1].0, "background");
    assert_eq!(kinds[17].0, "account_4");
    assert_eq!(form.fields()[0].value(), Some("Evening"));
    assert!(form.fields()[0].focus());
    assert_eq!(well(&form, "background"), "#101018");
    // What the theme leaves to the terminal is shown as terminals have it,
    // and its own text as what is read on the theme's background.
    assert_eq!(well(&form, "dim"), "#7f7f7f");
    assert_eq!(well(&form, "unread"), "#ffffff");
    // The theme is drawn with from the moment the question is up.
    assert_eq!(harness.env.styles.theme_name(), "Evening");
    assert_eq!(
        harness.env.styles.background_rgb(),
        Some((0x10, 0x10, 0x18))
    );

    // A colour picked is drawn with at once, and the well has it.
    tell(&mut harness, &[("background", "#203040")], None);
    assert_eq!(
        harness.env.styles.background_rgb(),
        Some((0x20, 0x30, 0x40))
    );
    assert_eq!(well(&sheet(&harness), "background"), "#203040");
    // The wells that were not touched keep what the theme wrote.
    let draft = &harness.state.theme_prompt.as_ref().unwrap().draft;
    assert_eq!(draft.colors.dim, ColorSpec::Indexed(8));
    assert_eq!(draft.colors.text, ColorSpec::Rgb(0xe0, 0xe0, 0xf0));
    assert_eq!(draft.colors.selection_inactive_bg, None);

    // Text that cannot be read on its background is said to be so.
    assert!(!sheet(&harness).note().unwrap().contains("hard to read"));
    tell(&mut harness, &[("text", "#304050")], None);
    assert!(sheet(&harness).note().unwrap().contains("hard to read"));

    // Given up: nothing is asked of the service, and Quill is as it was.
    tell(&mut harness, &[], Some("cancel"));
    assert!(harness.state.theme_prompt.is_none());
    assert_eq!(harness.env.styles.theme_name(), "dark");
    assert_eq!(harness.env.look.trying, None);
    assert!(asked(&harness).is_empty());
}

#[test]
fn saved_under_its_name_a_theme_is_changed_and_under_another_it_is_a_new_one() {
    let mut harness = window("");
    press(&mut harness, "edit", Some("Evening"));
    tell(&mut harness, &[("accent", "#ff8800")], None);
    tell(&mut harness, &[], Some("confirm"));
    let mut changed = evening();
    changed.colors.accent = ColorSpec::Rgb(0xff, 0x88, 0);
    assert_eq!(
        asked(&harness),
        [&ThemesChange::Save {
            from: "Evening".to_owned(),
            theme: changed.clone(),
            select: None,
        }]
    );
    // The question is down, and the theme in force is drawn with again
    // until the service answers.
    assert!(harness.state.theme_prompt.is_none());
    assert_eq!(harness.env.styles.theme_name(), "dark");

    // Under another name: a new theme, to be the one in use, with the
    // colour picked at the moment the button was pressed.
    press(&mut harness, "edit", Some("Evening"));
    tell(
        &mut harness,
        &[("name", " Morning "), ("error", "#ff0000")],
        Some("confirm"),
    );
    let mut morning = evening();
    morning.name = "Morning".to_owned();
    morning.colors.error = ColorSpec::Rgb(0xff, 0, 0);
    assert_eq!(
        asked(&harness)[1],
        &ThemesChange::Save {
            from: "Evening".to_owned(),
            theme: morning,
            select: Some(true),
        }
    );

    // One Quill comes with is saved as a copy, under a name no theme has.
    press(&mut harness, "edit", Some("Terminal"));
    let form = sheet(&harness);
    assert_eq!(form.title(), "The Theme \u{201c}Terminal\u{201d}");
    assert_eq!(form.fields()[0].value(), Some("Terminal copy"));
    // Its colours are the terminal's to say: black and white stand in.
    assert_eq!(well(&form, "background"), "#000000");
    assert_eq!(well(&form, "text"), "#ffffff");
    tell(&mut harness, &[], Some("confirm"));
    let mut copy = Theme::terminal();
    copy.name = "Terminal copy".to_owned();
    // Nothing was picked, so nothing the theme left to the terminal was
    // fixed.
    assert_eq!(
        asked(&harness)[2],
        &ThemesChange::Save {
            from: "terminal".to_owned(),
            theme: copy,
            select: Some(true),
        }
    );

    // What the service could not do is said in the window.
    harness.act(Action::Account(AccountEvent::ThemesChanged {
        read: Ok(read_with("")),
        result: Err("themes/Morning.toml cannot be written: the disk is full.".to_owned()),
    }));
    let shown = harness.state.settings_shown(&harness.env).unwrap().1;
    assert!(shown.problem().unwrap().contains("the disk is full"));
}

#[test]
fn a_name_that_cannot_be_had_leaves_the_question_up_saying_why() {
    let mut harness = window("");
    press(&mut harness, "edit", Some("Evening"));
    for (name, why) in [
        ("", "A theme needs a name."),
        ("dark", "is a theme Quill comes with"),
        ("Light", "is a theme Quill comes with"),
        (
            "broken",
            "There is already a theme called \u{201c}broken\u{201d}",
        ),
        (
            "EVENING",
            "There is already a theme called \u{201c}Evening\u{201d}",
        ),
        ("a/b", "cannot have /"),
        ("mine.toml", "cannot have /"),
    ] {
        tell(&mut harness, &[("name", name)], Some("confirm"));
        let form = sheet(&harness);
        assert!(form.error().unwrap().contains(why), "{name}: {form:?}");
        // What was typed is still there to put right.
        assert_eq!(form.fields()[0].value(), Some(name));
    }
    assert!(asked(&harness).is_empty());
    // Put right, it goes.
    tell(&mut harness, &[("name", "Evening 2")], Some("confirm"));
    assert_eq!(asked(&harness).len(), 1);
    assert!(harness.state.theme_prompt.is_none());

    // One Quill comes with cannot be saved over, under its own name.
    press(&mut harness, "edit", Some("Dark"));
    tell(&mut harness, &[("name", "dark")], Some("confirm"));
    assert!(
        sheet(&harness)
            .error()
            .unwrap()
            .contains("is a theme Quill comes with")
    );
    // `esc` puts the question away, as its button does.
    harness.keys("esc");
    assert!(harness.state.theme_prompt.is_none());
    assert_eq!(asked(&harness).len(), 1);
}

#[test]
fn a_theme_is_deleted_only_after_asking_with_what_goes_with_it_said() {
    let mut harness = window("[ui]\ntheme_dark = \"Evening\"\n");
    press(&mut harness, "delete", Some("Evening"));
    let form = sheet(&harness);
    assert_eq!(form.step(), "theme_delete");
    assert_eq!(form.title(), "Delete the Theme \u{201c}Evening\u{201d}?");
    let note = form.note().unwrap();
    assert!(note.contains("cannot be undone"), "{note}");
    assert!(note.contains("It is the dark theme"), "{note}");
    assert!(form.fields().is_empty());
    // No key deletes: only the button.
    let buttons: Vec<(&str, bool)> = form
        .buttons()
        .iter()
        .map(|button| (button.title.as_str(), button.role.is_some()))
        .collect();
    assert_eq!(buttons, [("Cancel", true), ("Delete", false)]);
    // Nothing is drawn differently for this question.
    assert_eq!(harness.env.look.trying, None);

    tell(&mut harness, &[], Some("cancel"));
    assert!(asked(&harness).is_empty());

    press(&mut harness, "delete", Some("broken"));
    assert!(!sheet(&harness).note().unwrap().contains("It is the"));
    tell(&mut harness, &[], Some("confirm"));
    assert_eq!(
        asked(&harness),
        [&ThemesChange::Delete {
            name: "broken".to_owned()
        }]
    );
    assert!(harness.state.theme_prompt.is_none());
}

#[test]
fn only_a_host_with_sheets_has_themes_to_change() {
    let mut harness = Harness::with_accounts(160, 40);
    harness.act(Action::Settings(SettingsAct::Opened));
    harness.act(Action::Account(AccountEvent::Settings {
        read: Ok(read_with("")),
        changed: None,
    }));
    let (_, actions) = themes(&harness);
    assert!(!actions[0].enabled && !actions[1].enabled);
    press(&mut harness, "edit", Some("Evening"));
    assert!(harness.state.theme_prompt.is_none());
    assert_eq!(harness.env.look.trying, None);
}

#[test]
fn a_long_list_of_themes_is_one_to_search() {
    let mut harness = window("");
    let searchable = |harness: &Harness| {
        let shown = harness.state.settings_shown(&harness.env).unwrap().1;
        matches!(
            shown.field("appearance", "themes"),
            Some(FormField::List {
                searchable: true,
                ..
            })
        )
    };
    assert!(!searchable(&harness));
    let mut read = read_with("");
    for count in 0..6 {
        let mut theme = evening();
        theme.name = format!("Evening {count}");
        read.themes.push(ThemeLine {
            name: theme.name.clone(),
            own: true,
            theme: Ok(theme),
        });
    }
    harness.act(Action::Account(AccountEvent::Settings {
        read: Ok(read),
        changed: None,
    }));
    assert!(searchable(&harness));
    assert_eq!(themes(&harness).0.len(), 11);
}
