//! Changing `config.toml` for the user: adding an account, pinning a
//! certificate, a setting changed in the app's settings window. Everything
//! else in the file, comments and layout included, is left exactly as they
//! wrote it.

use std::fs;
use std::io::{self, Write};
use std::os::unix::fs::{OpenOptionsExt, PermissionsExt};
use std::path::Path;

use quill_core::config::{Config, TlsMode};
use quill_tui::settings::{Change, Written};
use quill_tui::setup::{AccountDraft, ServerDraft, SignInDraft};
use toml_edit::{ArrayOfTables, DocumentMut, InlineTable, Item, Table, Value, value};

/// The permissions of a config Quill makes: the user's alone.
const PRIVATE: u32 = 0o600;

/// `text` with one more `[[account]]`. `fingerprint`, when given, pins the
/// IMAP server's certificate.
pub fn add_account(
    text: &str,
    draft: &AccountDraft,
    fingerprint: Option<&str>,
) -> Result<String, String> {
    let mut account = Table::new();
    account["name"] = value(&draft.name);
    account["type"] = value("imap");
    account["address"] = value(&draft.address);
    account["imap"] = Item::Value(Value::InlineTable(server(
        &draft.imap,
        &draft.address,
        fingerprint,
    )));
    if let Some(smtp) = &draft.smtp {
        account["smtp"] = Item::Value(Value::InlineTable(server(smtp, &draft.address, None)));
    }
    with_account(text, account)
}

/// `text` with one more `[[account]]` that signs in with Microsoft: a
/// `graph` account. The application it signs in as is written only when it
/// is not Quill's own, which an account that names none uses anyway; and
/// `oauth_tenant` only when the draft names one.
pub fn add_sign_in_account(text: &str, draft: &SignInDraft) -> Result<String, String> {
    let mut account = Table::new();
    account["name"] = value(&draft.name);
    account["type"] = value("graph");
    account["address"] = value(&draft.address);
    let application = draft.application_id.trim();
    if !application.is_empty() && application != crate::oauth::MICROSOFT_APPLICATION {
        account["oauth_client_id"] = value(application);
    }
    if let Some(tenant) = &draft.tenant {
        account["oauth_tenant"] = value(tenant);
    }
    with_account(text, account)
}

/// `text` with `account` after the accounts it already has.
fn with_account(text: &str, account: Table) -> Result<String, String> {
    let mut document: DocumentMut = text
        .parse()
        .map_err(|err| format!("config.toml cannot be read: {err}"))?;
    document
        .entry("account")
        .or_insert(Item::ArrayOfTables(ArrayOfTables::new()))
        .as_array_of_tables_mut()
        .ok_or("`account` in config.toml is not a list of accounts")?
        .push(account);
    checked(document.to_string())
}

/// `text` with the certificate of one of `account`'s servers pinned:
/// `server` is the key it is configured under, `imap` or `smtp`.
pub fn pin_fingerprint(
    text: &str,
    account: &str,
    server: &str,
    fingerprint: &str,
) -> Result<String, String> {
    let mut document: DocumentMut = text
        .parse()
        .map_err(|err| format!("config.toml cannot be read: {err}"))?;
    let accounts = document
        .get_mut("account")
        .and_then(Item::as_array_of_tables_mut)
        .ok_or("config.toml has no accounts")?;
    let table = accounts
        .iter_mut()
        .find(|table| table.get("name").and_then(Item::as_str) == Some(account))
        .ok_or_else(|| format!("config.toml has no account called {account}"))?;
    let entry = table
        .get_mut(server)
        .ok_or_else(|| format!("{account} has no `{server}` server in config.toml"))?;
    // Written either as `imap = { ... }` or as a `[account.imap]` table.
    if let Some(inline) = entry.as_inline_table_mut() {
        inline.insert("fingerprint", fingerprint.into());
    } else if let Some(table) = entry.as_table_mut() {
        table["fingerprint"] = value(fingerprint);
    } else {
        return Err(format!(
            "`{server}` of {account} in config.toml is not a table"
        ));
    }
    checked(document.to_string())
}

/// `text` with `web_reading` under `[ui]` set: whether mail is read in the
/// app's web view pane. Written when the user switches, so that Quill
/// starts the way they left it. A comment on the line is kept, like every
/// other.
pub fn set_web_reading(text: &str, on: bool) -> Result<String, String> {
    set_value(text, "ui", "web_reading", Value::from(on))
}

/// `text` with `sidebar` under `[ui]` set: whether the accounts pane is
/// on show. Written when the user hides it or puts it back, like
/// [`set_web_reading`].
pub fn set_sidebar(text: &str, on: bool) -> Result<String, String> {
    set_value(text, "ui", "sidebar", Value::from(on))
}

/// `text` with `editor_tips` under `[compose]` set: whether what an
/// editor's keys do is said above it. Written once they have been shown
/// for one message, and when the user puts them away.
pub fn set_editor_tips(text: &str, on: bool) -> Result<String, String> {
    set_value(text, "compose", "editor_tips", Value::from(on))
}

/// `text` with one setting given another value: what was changed in the
/// app's settings window. The line is changed where it stands, with the
/// comment on it kept; a setting the file does not have yet is added to
/// its section, and the section made if there is none.
///
/// A file Quill cannot read for what it says is not changed, whatever is
/// wrong with it and wherever: the window does not edit a file it cannot
/// show. Nor is a file left unreadable by the change.
pub fn set(text: &str, change: &Change) -> Result<String, String> {
    let config = Config::parse(text)
        .map_err(|err| format!("config.toml has a mistake to fix first: {err}"))?;
    let setting = change.setting();
    // A theme to draw with from now on. In a file that has one theme for
    // both appearances, as every file had before there were two, that is
    // `theme`, and it goes on being the one theme. In a file that names a
    // theme for either appearance, it is the theme of the one in force.
    let key = match change {
        Change::ThemeInForce { .. }
            if config.ui.theme_light.is_none() && config.ui.theme_dark.is_none() =>
        {
            "theme"
        }
        _ => setting.key(),
    };
    let written = match change.written() {
        Written::Gone => return remove_value(text, setting.section(), key),
        Written::Word(word) => Value::from(word),
        Written::Flag(on) => Value::from(on),
        Written::Whole(number) => Value::from(number),
        Written::Decimal(number) => Value::from(number),
    };
    set_value(text, setting.section(), key, written)
}

/// `text` without `key` in `section`: the setting is as it is when the
/// file does not say. A file that never said is left exactly as it is.
fn remove_value(text: &str, section: &str, key: &str) -> Result<String, String> {
    let mut document: DocumentMut = text
        .parse()
        .map_err(|err| format!("config.toml cannot be read: {err}"))?;
    let removed = document
        .get_mut(section)
        .and_then(Item::as_table_like_mut)
        .and_then(|table| table.remove(key));
    if removed.is_none() {
        return Ok(text.to_owned());
    }
    checked(document.to_string())
}

/// `text` with `key` in `section` set to `new`.
fn set_value(text: &str, section: &str, key: &str, new: Value) -> Result<String, String> {
    let mut document: DocumentMut = text
        .parse()
        .map_err(|err| format!("config.toml cannot be read: {err}"))?;
    let had_section = document.contains_key(section);
    let table = document
        .entry(section)
        .or_insert(Item::Table(Table::new()))
        .as_table_like_mut()
        .ok_or_else(|| format!("`{section}` in config.toml is not a table"))?;
    match table.get_mut(key).and_then(Item::as_value_mut) {
        // In place, with whatever is written around it.
        Some(existing) => {
            let decor = existing.decor().clone();
            *existing = new;
            *existing.decor_mut() = decor;
        }
        None => {
            table.insert(key, Item::Value(new));
        }
    }
    // A section of Quill's own making is set apart from what is above it.
    if !had_section
        && !text.trim().is_empty()
        && let Some(table) = document.get_mut(section).and_then(Item::as_table_mut)
    {
        table.decor_mut().set_prefix("\n");
    }
    checked(document.to_string())
}

fn server(server: &ServerDraft, address: &str, fingerprint: Option<&str>) -> InlineTable {
    let mut table = InlineTable::new();
    table.insert("host", server.host.as_str().into());
    table.insert("port", i64::from(server.port).into());
    table.insert(
        "tls",
        match server.tls {
            TlsMode::Implicit => "implicit",
            TlsMode::Starttls => "starttls",
            TlsMode::None => "none",
        }
        .into(),
    );
    // The address is the default sign-in name, so it is not repeated.
    if server.username != address {
        table.insert("username", server.username.as_str().into());
    }
    if let Some(fingerprint) = fingerprint {
        table.insert("fingerprint", fingerprint.into());
    }
    table
}

/// Refuses to hand back a config Quill itself could not read: better to
/// change nothing than to write a file that will not load.
fn checked(text: String) -> Result<String, String> {
    Config::parse(&text)
        .map(|_| text)
        .map_err(|err| format!("the change would leave config.toml unreadable: {err}"))
}

/// Replaces the file at `path` with `text`, all or nothing: it is written
/// beside the old one and moved into place.
///
/// A file that was there keeps the permissions it had. One made here is
/// the user's alone to read and write, whatever the umask: it says which
/// servers they use and under which names.
pub fn write(path: &Path, text: &str) -> io::Result<()> {
    let dir = path
        .parent()
        .ok_or_else(|| io::Error::other("the config file has no directory"))?;
    fs::create_dir_all(dir)?;
    let mode = match fs::metadata(path) {
        Ok(existing) => existing.permissions().mode() & 0o7777,
        Err(err) if err.kind() == io::ErrorKind::NotFound => PRIVATE,
        Err(err) => return Err(err),
    };
    let partial = dir.join(format!(".config.toml.{}", std::process::id()));
    let result = (|| {
        let mut file = fs::OpenOptions::new()
            .write(true)
            .create(true)
            .truncate(true)
            .mode(PRIVATE)
            .open(&partial)?;
        // Said outright as well: the mode asked for above is cut down by
        // the umask, and is not applied at all to a file left behind by an
        // earlier attempt.
        file.set_permissions(fs::Permissions::from_mode(mode))?;
        file.write_all(text.as_bytes())?;
        file.sync_all()?;
        fs::rename(&partial, path)
    })();
    if result.is_err() {
        let _ = fs::remove_file(&partial);
    }
    result
}

#[cfg(test)]
mod tests {
    use super::*;

    fn draft() -> AccountDraft {
        AccountDraft {
            name: "Personal".to_owned(),
            address: "robin@example.com".to_owned(),
            imap: ServerDraft {
                host: "imap.example.com".to_owned(),
                port: 993,
                tls: TlsMode::Implicit,
                username: "robin@example.com".to_owned(),
            },
            smtp: Some(ServerDraft {
                host: "smtp.example.com".to_owned(),
                port: 587,
                tls: TlsMode::Starttls,
                username: "robin@example.com".to_owned(),
            }),
        }
    }

    #[test]
    fn an_account_is_added_to_an_empty_config() {
        let text = add_account("", &draft(), None).unwrap();
        let config = Config::parse(&text).unwrap();
        assert_eq!(config.accounts.len(), 1);
        let account = &config.accounts[0];
        assert_eq!(
            (account.name.as_str(), account.address.as_str()),
            ("Personal", "robin@example.com")
        );
        let imap = account.imap.as_ref().unwrap();
        assert_eq!(
            (imap.host.as_str(), imap.port, imap.tls),
            ("imap.example.com", Some(993), TlsMode::Implicit)
        );
        // The address is the default sign-in name and is not written twice.
        assert_eq!(imap.username, None);
        assert_eq!(imap.fingerprint, None);
        let smtp = account.smtp.as_ref().unwrap();
        assert_eq!(
            (smtp.host.as_str(), smtp.port, smtp.tls),
            ("smtp.example.com", Some(587), TlsMode::Starttls)
        );
    }

    #[test]
    fn what_the_user_wrote_is_left_as_it_was() {
        let before = "# My settings\n[ui]\ntheme = \"light\"   # easier on the eyes\n\n\
                      [[account]]\nname = \"Work\"\ntype = \"imap\"\naddress = \"robin@work.example.com\"\n\
                      imap = { host = \"mail.work.example.com\" }\n";
        let mut other = draft();
        other.imap.username = "robin".to_owned();
        let after = add_account(before, &other, Some(&"ab".repeat(32))).unwrap();
        assert!(after.starts_with(before), "{after}");
        let config = Config::parse(&after).unwrap();
        assert_eq!(config.accounts.len(), 2);
        let imap = config.accounts[1].imap.as_ref().unwrap();
        assert_eq!(imap.username.as_deref(), Some("robin"));
        assert_eq!(imap.fingerprint.as_deref(), Some("ab".repeat(32).as_str()));
    }

    #[test]
    fn web_reading_is_written_under_ui_and_everything_else_is_left_as_it_was() {
        // Into a section that is there, after what it already says.
        let before = "# My settings\n[ui]\ntheme = \"light\"   # easier on the eyes\n\n\
                      [[account]]\nname = \"Work\"   # the office\ntype = \"imap\"\n\
                      address = \"robin@work.example.com\"\nimap = { host = \"mail.work.example.com\" }\n";
        let on = set_web_reading(before, true).unwrap();
        assert_eq!(
            on,
            before.replace(
                "# easier on the eyes\n",
                "# easier on the eyes\nweb_reading = true\n"
            )
        );
        assert!(Config::parse(&on).unwrap().ui.web_reading);

        // Switched back, the line is changed where it stands.
        let off = set_web_reading(&on, false).unwrap();
        assert_eq!(off, on.replace("web_reading = true", "web_reading = false"));
        assert!(!Config::parse(&off).unwrap().ui.web_reading);
        // And again: nothing is added twice.
        assert_eq!(set_web_reading(&on, true).unwrap(), on);

        // A comment the user put on the line itself stays on it.
        let commented = "[ui]\nweb_reading = false   # I like the text reader\ntheme = \"dark\"\n";
        assert_eq!(
            set_web_reading(commented, true).unwrap(),
            "[ui]\nweb_reading = true   # I like the text reader\ntheme = \"dark\"\n"
        );
    }

    #[test]
    fn web_reading_makes_the_ui_section_when_there_is_none() {
        let text = set_web_reading("", true).unwrap();
        assert_eq!(text, "[ui]\nweb_reading = true\n");
        assert!(Config::parse(&text).unwrap().ui.web_reading);

        // After what the user wrote, which stays as it was.
        let before = "# Mine\n[[account]]\nname = \"Home\"   # the old one\ntype = \"imap\"\n\
                      address = \"robin@example.org\"\nimap = { host = \"mail.example.org\" }\n";
        let after = set_web_reading(before, true).unwrap();
        assert_eq!(after, format!("{before}\n[ui]\nweb_reading = true\n"));
        let config = Config::parse(&after).unwrap();
        assert!(config.ui.web_reading);
        assert_eq!(config.accounts.len(), 1);

        // Written on one line, the section is still the one changed.
        let inline = "ui = { theme = \"light\" }\n";
        let after = set_web_reading(inline, true).unwrap();
        let config = Config::parse(&after).unwrap();
        assert!(config.ui.web_reading);
        assert_eq!(config.ui.theme, "light");
    }

    /// A config a person has written, with their comments.
    const WRITTEN: &str = "# My settings\n\
        [ui]\n\
        theme = \"light\"   # easier on the eyes\n\
        keymap = \"vim\"\n\
        \n\
        [compose]\n\
        # ten is too short for me\n\
        undo_send_seconds = 30\n\
        \n\
        [[account]]\n\
        name = \"Work\"   # the office\n\
        type = \"imap\"\n\
        address = \"robin@work.example.com\"\n\
        imap = { host = \"mail.work.example.com\" }\n\
        \n\
        [[rule]]\n\
        match = \"from:buildbot\"   # noisy\n\
        mark_read = true\n";

    fn gigabytes(value: f64) -> Change {
        Change::CacheGb(quill_host::bridge::Number::new(value).unwrap())
    }

    #[test]
    fn a_setting_is_changed_where_it_stands_and_everything_else_is_left_as_it_was() {
        use quill_core::config::{
            Appearance, ComposeEditor, ComposeFormat, KeymapPreset, RemoteImages, ReplyPosition,
        };

        // On its own line, with the comment that was on it.
        assert_eq!(
            set(WRITTEN, &Change::Keymap(KeymapPreset::Mutt)).unwrap(),
            WRITTEN.replace("keymap = \"vim\"", "keymap = \"mutt\"")
        );
        assert_eq!(
            set(WRITTEN, &Change::UndoSendSeconds(5)).unwrap(),
            WRITTEN.replace("undo_send_seconds = 30", "undo_send_seconds = 5")
        );
        // One the file has not got yet goes at the end of its section.
        assert_eq!(
            set(WRITTEN, &Change::Notifications(false)).unwrap(),
            WRITTEN.replace(
                "keymap = \"vim\"\n",
                "keymap = \"vim\"\nnotifications = false\n"
            )
        );
        assert_eq!(
            set(WRITTEN, &Change::Threads(false)).unwrap(),
            WRITTEN.replace("keymap = \"vim\"\n", "keymap = \"vim\"\nthreads = false\n")
        );
        assert_eq!(
            set(WRITTEN, &Change::WebReading(true)).unwrap(),
            WRITTEN.replace(
                "keymap = \"vim\"\n",
                "keymap = \"vim\"\nweb_reading = true\n"
            )
        );
        // The appearance, and a theme for each: keys of their own, beside
        // the `theme` the file had, which is left as it was written and
        // still stands for whichever of the two is not said.
        assert_eq!(
            set(WRITTEN, &Change::Appearance(Appearance::Dark)).unwrap(),
            WRITTEN.replace(
                "keymap = \"vim\"\n",
                "keymap = \"vim\"\nappearance = \"dark\"\n"
            )
        );
        let dark = set(WRITTEN, &Change::ThemeDark("Catppuccin Mocha".to_owned())).unwrap();
        assert_eq!(
            dark,
            WRITTEN.replace(
                "keymap = \"vim\"\n",
                "keymap = \"vim\"\ntheme_dark = \"Catppuccin Mocha\"\n"
            )
        );
        let ui = Config::parse(&dark).unwrap().ui;
        assert_eq!(
            (ui.theme.as_str(), ui.theme_for(false), ui.theme_for(true)),
            ("light", "light", "Catppuccin Mocha")
        );
        let both = set(&dark, &Change::ThemeLight("terminal".to_owned())).unwrap();
        assert_eq!(
            both,
            dark.replace(
                "theme_dark = \"Catppuccin Mocha\"\n",
                "theme_dark = \"Catppuccin Mocha\"\ntheme_light = \"terminal\"\n"
            )
        );
        // Changed again, each is changed where it stands.
        assert_eq!(
            set(&both, &Change::ThemeDark("dark".to_owned())).unwrap(),
            both.replace("theme_dark = \"Catppuccin Mocha\"", "theme_dark = \"dark\"")
        );
        assert!(both.contains("theme = \"light\"   # easier on the eyes\n"));

        // A theme to draw with from now on, as one just imported is. With
        // one theme for both appearances in the file, it is that one that
        // changes, where it stands, whichever appearance is in force.
        let imported = |dark| Change::ThemeInForce {
            name: "Dracula+".to_owned(),
            dark,
        };
        for dark in [false, true] {
            assert_eq!(
                set(WRITTEN, &imported(dark)).unwrap(),
                WRITTEN.replace("theme = \"light\"   #", "theme = \"Dracula+\"   #")
            );
            // A file that says nothing of themes is given the one.
            assert_eq!(
                set("", &imported(dark)).unwrap(),
                "[ui]\ntheme = \"Dracula+\"\n"
            );
        }
        // With a theme for either appearance in the file, it is the theme
        // of the appearance in force, and `theme` is left alone.
        assert_eq!(
            set(&dark, &imported(true)).unwrap(),
            dark.replace(
                "theme_dark = \"Catppuccin Mocha\"",
                "theme_dark = \"Dracula+\""
            )
        );
        assert_eq!(
            set(&dark, &imported(false)).unwrap(),
            dark.replace(
                "theme_dark = \"Catppuccin Mocha\"\n",
                "theme_dark = \"Catppuccin Mocha\"\ntheme_light = \"Dracula+\"\n"
            )
        );
        assert_eq!(
            set(&both, &imported(false)).unwrap(),
            both.replace("theme_light = \"terminal\"", "theme_light = \"Dracula+\"")
        );
        for (change, line) in [
            (Change::Editor(ComposeEditor::Nano), "editor = \"nano\""),
            (Change::Editor(ComposeEditor::Mg), "editor = \"mg\""),
            (
                Change::EditorPath(Some("/opt/homebrew/bin/nvim".to_owned())),
                "editor_path = \"/opt/homebrew/bin/nvim\"",
            ),
            (Change::Format(ComposeFormat::Plain), "format = \"plain\""),
            (
                Change::ReplyPosition(ReplyPosition::Below),
                "reply_position = \"below\"",
            ),
            (Change::SpellCheck(false), "spell_check = false"),
        ] {
            assert_eq!(
                set(WRITTEN, &change).unwrap(),
                WRITTEN.replace(
                    "undo_send_seconds = 30\n",
                    &format!("undo_send_seconds = 30\n{line}\n")
                ),
                "{change:?}"
            );
        }
        // And a section the file has not got is made, after what is there.
        assert_eq!(
            set(WRITTEN, &Change::RemoteImages(RemoteImages::Never)).unwrap(),
            format!("{WRITTEN}\n[privacy]\nremote_images = \"never\"\n")
        );
        assert_eq!(
            set(WRITTEN, &gigabytes(2.0)).unwrap(),
            format!("{WRITTEN}\n[storage]\ncache_gb = 2\n")
        );
        assert_eq!(
            set(WRITTEN, &gigabytes(0.5)).unwrap(),
            format!("{WRITTEN}\n[storage]\ncache_gb = 0.5\n")
        );
        // Into no file at all, it is the whole of it.
        assert_eq!(
            set("", &Change::Keymap(KeymapPreset::Mutt)).unwrap(),
            "[ui]\nkeymap = \"mutt\"\n"
        );

        // One change after another, each to what the last left: all of
        // them hold, and the comments, the account and the rule are as
        // they were written.
        let mut text = WRITTEN.to_owned();
        for change in [
            Change::ThemeDark("terminal".to_owned()),
            Change::Appearance(Appearance::Light),
            Change::Notifications(false),
            Change::Editor(ComposeEditor::Vim),
            Change::UndoSendSeconds(0),
            Change::RemoteImages(RemoteImages::Never),
            gigabytes(12.25),
            Change::ThemeDark("dark".to_owned()),
            Change::Appearance(Appearance::System),
        ] {
            text = set(&text, &change).unwrap();
        }
        let config = Config::parse(&text).unwrap();
        assert_eq!(config.ui.theme_for(true), "dark");
        assert_eq!(config.ui.theme_for(false), "light");
        assert_eq!(config.ui.appearance, Appearance::System);
        assert!(!config.ui.notifications);
        assert_eq!(config.compose.editor, ComposeEditor::Vim);
        assert_eq!(config.compose.undo_send_seconds, 0);
        assert_eq!(config.privacy.remote_images, RemoteImages::Never);
        assert_eq!(config.storage.cache_gb, 12.25);
        assert_eq!(config.accounts.len(), 1);
        assert_eq!(config.rules.len(), 1);
        for kept in [
            "# My settings\n",
            "# easier on the eyes\n",
            "# ten is too short for me\n",
            "name = \"Work\"   # the office\n",
            "match = \"from:buildbot\"   # noisy\n",
        ] {
            assert!(text.contains(kept), "{kept:?} is gone from:\n{text}");
        }
        // Setting what is already so changes nothing at all.
        assert_eq!(
            set(&text, &Change::ThemeDark("dark".to_owned())).unwrap(),
            text
        );
    }

    #[test]
    fn a_setting_is_never_written_into_a_file_with_a_mistake_in_it() {
        let change = Change::Notifications(false);
        for mistaken in [
            // Not TOML.
            "[ui\ntheme = ",
            // TOML, and not a config: a key Quill does not know.
            "[ui]\nthemee = \"dark\"\n",
            // A mistake nowhere near what is being changed.
            "[ui]\ntheme = \"dark\"\n\n[[rule]]\nmatch = \"\"\nstar = true\n",
            "[compose]\neditor = \"emacs\"\n",
            "[storage]\ncache_gb = -1\n",
            // The section is not a section.
            "ui = 3\n",
        ] {
            let refused = set(mistaken, &change).unwrap_err();
            assert!(
                refused.contains("config.toml has a mistake to fix first"),
                "{mistaken:?}: {refused}"
            );
        }
        // Even when the change would have put the mistake right: the
        // window does not edit a file it could not show.
        assert!(set("[storage]\ncache_gb = -1\n", &gigabytes(5.0)).is_err());
    }

    #[test]
    fn a_value_the_file_could_not_be_read_back_with_is_not_written() {
        // Checked before it gets here, and here again: what comes out is
        // always a config Quill can read.
        let refused = set(WRITTEN, &gigabytes(-2.0)).unwrap_err();
        assert!(
            refused.contains("the change would leave config.toml unreadable"),
            "{refused}"
        );
        // Text that would end a string in TOML is written so that it does
        // not: the file reads back as what was asked for.
        let odd = "dark\"\n[ui]\nkeymap = \"mutt";
        let text = set(WRITTEN, &Change::ThemeLight(odd.to_owned())).unwrap();
        let config = Config::parse(&text).unwrap();
        assert_eq!(config.ui.theme_for(false), odd);
        assert_eq!(config.ui.keymap, quill_core::config::KeymapPreset::Vim);
    }

    #[test]
    fn a_setting_written_on_one_line_with_its_section_is_changed_there() {
        let inline = "ui = { theme = \"light\", keymap = \"vim\" }   # all on one line\n";
        let after = set(
            inline,
            &Change::Keymap(quill_core::config::KeymapPreset::Mutt),
        )
        .unwrap();
        assert_eq!(after, inline.replace("\"vim\"", "\"mutt\""));
        let after = set(inline, &Change::ThemeDark("dark".to_owned())).unwrap();
        let config = Config::parse(&after).unwrap();
        assert_eq!(
            (config.ui.theme_for(false), config.ui.theme_for(true)),
            ("light", "dark")
        );
        let after = set(inline, &Change::Threads(false)).unwrap();
        let config = Config::parse(&after).unwrap();
        assert!(!config.ui.threads);
        assert_eq!(config.ui.theme, "light");
        assert!(after.ends_with("# all on one line\n"), "{after}");
    }

    #[test]
    fn web_reading_is_not_written_into_a_config_quill_cannot_read() {
        assert!(set_web_reading("[ui\ntheme = ", true).is_err());
        assert!(set_web_reading("ui = 3\n", true).is_err());
        // Nor is a config left unreadable that was already wrong elsewhere.
        assert!(set_web_reading("[ui]\nthemee = \"dark\"\n", true).is_err());
    }

    #[test]
    fn a_config_that_cannot_be_read_is_not_touched() {
        assert!(add_account("[ui\ntheme = ", &draft(), None).is_err());
        assert!(add_account("account = 3\n", &draft(), None).is_err());
        assert!(add_sign_in_account("[ui\ntheme = ", &microsoft(None)).is_err());
        assert!(add_sign_in_account("account = 3\n", &microsoft(None)).is_err());
    }

    fn microsoft(tenant: Option<&str>) -> SignInDraft {
        SignInDraft {
            name: "Work".to_owned(),
            address: "robin@example.com".to_owned(),
            application_id: "app-1".to_owned(),
            tenant: tenant.map(str::to_owned),
        }
    }

    #[test]
    fn an_account_that_signs_in_with_microsoft_is_a_graph_account_with_its_application() {
        // What docs/oauth.md has the user write by hand, key for key.
        let text = add_sign_in_account("", &microsoft(None)).unwrap();
        assert_eq!(
            text,
            "[[account]]\nname = \"Work\"\ntype = \"graph\"\naddress = \"robin@example.com\"\noauth_client_id = \"app-1\"\n"
        );
        let account = &Config::parse(&text).unwrap().accounts[0];
        assert_eq!(account.kind, quill_core::config::AccountType::Graph);
        assert_eq!(account.oauth_client_id.as_deref(), Some("app-1"));
        // No tenant named, none written: any organisation's accounts.
        assert_eq!(account.oauth_tenant, None);
        // It has no servers and no password to keep.
        assert!(account.imap.is_none() && account.smtp.is_none());

        // After what the user wrote, which stays as it was.
        let before = "# Mine\n[[account]]\nname = \"Home\"   # the old one\ntype = \"imap\"\n\
                      address = \"robin@example.org\"\nimap = { host = \"mail.example.org\" }\n";
        let after = add_sign_in_account(before, &microsoft(Some("example.com"))).unwrap();
        assert!(after.starts_with(before), "{after}");
        assert!(
            after.ends_with("oauth_client_id = \"app-1\"\noauth_tenant = \"example.com\"\n"),
            "{after}"
        );
        let config = Config::parse(&after).unwrap();
        assert_eq!(config.accounts.len(), 2);
        assert_eq!(
            config.accounts[1].oauth_tenant.as_deref(),
            Some("example.com")
        );

        // Text that would end a string in TOML is written so that it does
        // not: the file still reads back as what was asked for.
        let mut odd = microsoft(None);
        odd.name = "Work \"main\"\n[ui]".to_owned();
        let text = add_sign_in_account("", &odd).unwrap();
        let config = Config::parse(&text).unwrap();
        assert_eq!(config.accounts.len(), 1);
        assert_eq!(config.accounts[0].name, odd.name);
    }

    #[test]
    fn a_certificate_is_pinned_on_the_right_account() {
        let before = "[[account]]\nname = \"Work\"\ntype = \"imap\"\naddress = \"a@example.com\"\n\
                      imap = { host = \"one.example.com\" } # the office\n\n\
                      [[account]]\nname = \"Home\"\ntype = \"imap\"\naddress = \"b@example.com\"\n\n\
                      [account.imap]\nhost = \"two.example.com\"\n";
        let fingerprint = "cd".repeat(32);
        let after = pin_fingerprint(before, "Home", "imap", &fingerprint).unwrap();
        let config = Config::parse(&after).unwrap();
        assert_eq!(config.accounts[0].imap.as_ref().unwrap().fingerprint, None);
        assert_eq!(
            config.accounts[1]
                .imap
                .as_ref()
                .unwrap()
                .fingerprint
                .as_deref(),
            Some(fingerprint.as_str())
        );
        assert!(after.contains("# the office"));

        let after = pin_fingerprint(&after, "Work", "imap", &fingerprint).unwrap();
        let config = Config::parse(&after).unwrap();
        assert_eq!(
            config.accounts[0]
                .imap
                .as_ref()
                .unwrap()
                .fingerprint
                .as_deref(),
            Some(fingerprint.as_str())
        );
        // Pinning again replaces the old pin.
        let again = pin_fingerprint(&after, "Work", "imap", &"ef".repeat(32)).unwrap();
        assert_eq!(again.matches("fingerprint").count(), 2);

        assert!(pin_fingerprint(before, "Nobody", "imap", &fingerprint).is_err());
        assert!(pin_fingerprint("", "Work", "imap", &fingerprint).is_err());
    }

    #[test]
    fn the_sending_server_has_a_pin_of_its_own() {
        let before = "[[account]]\nname = \"Work\"\ntype = \"imap\"\naddress = \"a@example.com\"\n\
                      imap = { host = \"in.example.com\" }\n\
                      smtp = { host = \"out.example.com\", port = 587, tls = \"starttls\" }\n";
        let fingerprint = "ab".repeat(32);
        let after = pin_fingerprint(before, "Work", "smtp", &fingerprint).unwrap();
        let account = &Config::parse(&after).unwrap().accounts[0];
        assert_eq!(account.imap.as_ref().unwrap().fingerprint, None);
        let smtp = account.smtp.as_ref().unwrap();
        assert_eq!(smtp.fingerprint.as_deref(), Some(fingerprint.as_str()));
        // The rest of what was written about the server is kept.
        assert_eq!(
            (smtp.host.as_str(), smtp.port),
            ("out.example.com", Some(587))
        );

        // An account with nowhere to send through has nothing to pin.
        let reading_only = "[[account]]\nname = \"Work\"\ntype = \"imap\"\n\
                            address = \"a@example.com\"\nimap = { host = \"in.example.com\" }\n";
        assert!(pin_fingerprint(reading_only, "Work", "smtp", &fingerprint).is_err());
    }

    #[test]
    fn the_file_is_replaced_whole() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config").join("config.toml");
        write(&path, "first\n").unwrap();
        write(&path, "second\n").unwrap();
        assert_eq!(fs::read_to_string(&path).unwrap(), "second\n");
        let left: Vec<_> = fs::read_dir(path.parent().unwrap())
            .unwrap()
            .map(|entry| entry.unwrap().file_name())
            .collect();
        assert_eq!(left, ["config.toml"]);
    }

    fn mode(path: &Path) -> u32 {
        fs::metadata(path).unwrap().permissions().mode() & 0o7777
    }

    #[test]
    fn a_config_quill_makes_is_the_users_alone() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config").join("config.toml");
        write(&path, "first\n").unwrap();
        assert_eq!(mode(&path), 0o600);
        // And stays so when it is written again.
        write(&path, "second\n").unwrap();
        assert_eq!(mode(&path), 0o600);
        assert_eq!(fs::read_to_string(&path).unwrap(), "second\n");
    }

    #[test]
    fn a_config_that_was_there_keeps_its_permissions() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config.toml");
        for kept in [0o644, 0o640, 0o600] {
            fs::write(&path, "theirs\n").unwrap();
            fs::set_permissions(&path, fs::Permissions::from_mode(kept)).unwrap();
            write(&path, "changed\n").unwrap();
            assert_eq!(mode(&path), kept, "{kept:o}");
            assert_eq!(fs::read_to_string(&path).unwrap(), "changed\n");
        }
    }

    #[test]
    fn a_file_left_by_an_earlier_attempt_does_not_lend_its_permissions() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config.toml");
        let partial = dir
            .path()
            .join(format!(".config.toml.{}", std::process::id()));
        fs::write(&partial, "half\n").unwrap();
        fs::set_permissions(&partial, fs::Permissions::from_mode(0o666)).unwrap();
        write(&path, "whole\n").unwrap();
        assert_eq!(mode(&path), 0o600);
        assert_eq!(fs::read_to_string(&path).unwrap(), "whole\n");
    }
}
