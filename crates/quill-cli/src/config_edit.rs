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
    if let Some(google) = &draft.google {
        return add_google_account(text, draft, google);
    }
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

/// `text` with one more `[[account]]` that signs in with Google: an `imap`
/// account whose `auth` is `oauth`, with the mail servers it is read from
/// and sent through. The application it signs in as is written only when
/// it is not Quill's own, which an account that names none uses anyway.
fn add_google_account(
    text: &str,
    draft: &SignInDraft,
    google: &quill_tui::setup::GoogleDraft,
) -> Result<String, String> {
    let mut account = Table::new();
    account["name"] = value(&draft.name);
    account["type"] = value("imap");
    account["address"] = value(&draft.address);
    account["auth"] = value("oauth");
    let application = draft.application_id.trim();
    let quills = crate::oauth::GOOGLE_APPLICATION.map(|(id, _)| id);
    if !application.is_empty() && Some(application) != quills {
        account["oauth_client_id"] = value(application);
        if let Some(secret) = google
            .client_secret
            .as_deref()
            .map(str::trim)
            .filter(|secret| !secret.is_empty())
        {
            account["oauth_client_secret"] = value(secret);
        }
    }
    account["imap"] = Item::Value(Value::InlineTable(server(
        &google.imap,
        &draft.address,
        None,
    )));
    if let Some(smtp) = &google.smtp {
        account["smtp"] = Item::Value(Value::InlineTable(server(smtp, &draft.address, None)));
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

/// `text` without the account called `name`: its `[[account]]` and
/// whatever is written in it, and nothing else. The other accounts, and
/// every comment that is not the removed account's own, stay as they are.
pub fn remove_account(text: &str, name: &str) -> Result<String, String> {
    let mut document: DocumentMut = text
        .parse()
        .map_err(|err| format!("config.toml cannot be read: {err}"))?;
    let missing = || format!("there is no account called {name} in config.toml");
    let accounts = document
        .get_mut("account")
        .ok_or_else(missing)?
        .as_array_of_tables_mut()
        .ok_or(
            "the accounts in config.toml are not written as [[account]] sections, which is \
             the only way Quill edits them: take the account out by hand",
        )?;
    let index = accounts
        .iter()
        .position(|account| account.get("name").and_then(Item::as_str) == Some(name))
        .ok_or_else(missing)?;
    accounts.remove(index);
    if accounts.is_empty() {
        document.remove("account");
    }
    checked(document.to_string())
}

/// `text` with the servers of the account called `name` as `draft` has
/// them: where each is, how it is reached and the name it is signed in to
/// with. Everything else the account says, and every comment, stays.
/// With no sending server in the draft the account's `smtp` is taken out.
/// `fingerprint`, when given, pins the reading server's certificate; a
/// certificate pinned before stays pinned for as long as its server is
/// the same one.
pub fn change_account(
    text: &str,
    name: &str,
    draft: &AccountDraft,
    fingerprint: Option<&str>,
) -> Result<String, String> {
    let mut document: DocumentMut = text
        .parse()
        .map_err(|err| format!("config.toml cannot be read: {err}"))?;
    let missing = || format!("there is no account called {name} in config.toml");
    let accounts = document
        .get_mut("account")
        .ok_or_else(missing)?
        .as_array_of_tables_mut()
        .ok_or(
            "the accounts in config.toml are not written as [[account]] sections, which is \
             the only way Quill edits them: change the account by hand",
        )?;
    let table = accounts
        .iter_mut()
        .find(|table| table.get("name").and_then(Item::as_str) == Some(name))
        .ok_or_else(missing)?;
    change_server(table, "imap", &draft.imap, &draft.address, fingerprint)?;
    match &draft.smtp {
        Some(smtp) => change_server(table, "smtp", smtp, &draft.address, None)?,
        None => {
            table.remove("smtp");
        }
    }
    checked(document.to_string())
}

/// `text` with `letter` as the letter of the account called `name`: what
/// stands for it where mail of several accounts is listed together. With
/// none, its `letter` is taken out, and the first letter of its name
/// stands for it again. Everything else the account says stays.
pub fn set_account_letter(text: &str, name: &str, letter: Option<char>) -> Result<String, String> {
    let mut document: DocumentMut = text
        .parse()
        .map_err(|err| format!("config.toml cannot be read: {err}"))?;
    let missing = || format!("there is no account called {name} in config.toml");
    let accounts = document
        .get_mut("account")
        .ok_or_else(missing)?
        .as_array_of_tables_mut()
        .ok_or(
            "the accounts in config.toml are not written as [[account]] sections, which is \
             the only way Quill edits them: give the account its `letter` by hand",
        )?;
    let table = accounts
        .iter_mut()
        .find(|table| table.get("name").and_then(Item::as_str) == Some(name))
        .ok_or_else(missing)?;
    match letter {
        Some(letter) => match table.get_mut("letter") {
            // Changed where it is, so that a comment beside it stays.
            Some(Item::Value(old)) => {
                let decor = old.decor().clone();
                *old = Value::from(letter.to_string());
                *old.decor_mut() = decor;
            }
            _ => table["letter"] = value(letter.to_string()),
        },
        None => {
            table.remove("letter");
        }
    }
    checked(document.to_string())
}

/// Makes `account`'s server under `key` what `server` says, keeping
/// whatever else is written of it.
fn change_server(
    account: &mut Table,
    key: &str,
    server: &ServerDraft,
    address: &str,
    fingerprint: Option<&str>,
) -> Result<(), String> {
    let tls = match server.tls {
        TlsMode::Implicit => "implicit",
        TlsMode::Starttls => "starttls",
        TlsMode::None => "none",
    };
    // The address is the default sign-in name, so it is not written.
    let username = (server.username != address).then_some(server.username.as_str());
    let Some(entry) = account.get_mut(key) else {
        account[key] = Item::Value(Value::InlineTable(self::server(
            server,
            address,
            fingerprint,
        )));
        return Ok(());
    };
    // Written either as `imap = { ... }` or as a `[account.imap]` table.
    if let Some(inline) = entry.as_inline_table_mut() {
        // A certificate is pinned for the server it was shown by.
        let moved = inline.get("host").and_then(Value::as_str) != Some(server.host.as_str());
        inline.insert("host", server.host.as_str().into());
        inline.insert("port", i64::from(server.port).into());
        inline.insert("tls", tls.into());
        match username {
            Some(username) => {
                inline.insert("username", username.into());
            }
            None => {
                inline.remove("username");
            }
        }
        match fingerprint {
            Some(fingerprint) => {
                inline.insert("fingerprint", fingerprint.into());
            }
            None if moved => {
                inline.remove("fingerprint");
            }
            None => {}
        }
        inline.fmt();
    } else if let Some(table) = entry.as_table_mut() {
        let moved = table.get("host").and_then(Item::as_str) != Some(server.host.as_str());
        table["host"] = value(server.host.as_str());
        table["port"] = value(i64::from(server.port));
        table["tls"] = value(tls);
        match username {
            Some(username) => table["username"] = value(username),
            None => {
                table.remove("username");
            }
        }
        match fingerprint {
            Some(fingerprint) => table["fingerprint"] = value(fingerprint),
            None if moved => {
                table.remove("fingerprint");
            }
            None => {}
        }
    } else {
        return Err(format!(
            "`{key}` of the account in config.toml is not a table"
        ));
    }
    Ok(())
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

/// `text` with both ways in for agents switched off: `commands` and `mcp`
/// under `[agent]`. Written when the user stops the agents, so that
/// nothing comes in again until they say so, with Quill running or not.
pub fn stop_agents(text: &str) -> Result<String, String> {
    let text = set_value(text, "agent", "commands", Value::from(false))?;
    set_value(&text, "agent", "mcp", Value::from(false))
}

/// `text` with `sidebar` under `[ui]` set: whether the accounts pane is
/// on show. Written when the user hides it or puts it back, like
/// [`set_web_reading`].
pub fn set_sidebar(text: &str, on: bool) -> Result<String, String> {
    set_value(text, "ui", "sidebar", Value::from(on))
}

/// `text` with `dark_pages` under `[ui]` set: whether mail in the web pane
/// is shown dark while Quill is. Written when the user switches it, like
/// [`set_web_reading`].
pub fn set_dark_pages(text: &str, on: bool) -> Result<String, String> {
    set_value(text, "ui", "dark_pages", Value::from(on))
}

/// `text` with `sidebar_width` under `[ui]` set: how many columns the
/// accounts pane has. Written when the user lets go of the pane's edge.
/// `None` takes the line out, and the pane goes by the window's width
/// again.
pub fn set_sidebar_width(text: &str, columns: Option<u16>) -> Result<String, String> {
    match columns {
        Some(columns) => set_value(text, "ui", "sidebar_width", Value::from(i64::from(columns))),
        None => remove_value(text, "ui", "sidebar_width"),
    }
}

/// `text` with `reader_width` under `[ui]` set, from thousandths: the
/// message's share of the room right of the accounts. Written when the
/// user lets go of the edge between the list and the message. `None`
/// takes the line out.
pub fn set_reader_width(text: &str, thousandths: Option<u16>) -> Result<String, String> {
    match thousandths {
        Some(thousandths) => set_value(
            text,
            "ui",
            "reader_width",
            Value::from(f64::from(thousandths) / 1000.0),
        ),
        None => remove_value(text, "ui", "reader_width"),
    }
}

/// `text` with `editor_tips` under `[compose]` set: whether what an
/// editor's keys do is said above it. Written once they have been shown
/// for one message, and when the user puts them away.
pub fn set_editor_tips(text: &str, on: bool) -> Result<String, String> {
    set_value(text, "compose", "editor_tips", Value::from(on))
}

/// `text` with `enabled` under `[rules]` set: whether rules run by
/// themselves.
pub fn set_rules_enabled(text: &str, on: bool) -> Result<String, String> {
    set_value(text, "rules", "enabled", Value::from(on))
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
        Written::Words(words) => Value::Array(
            words
                .iter()
                .map(String::as_str)
                .collect::<toml_edit::Array>(),
        ),
        Written::Flag(on) => Value::from(on),
        Written::Whole(number) => Value::from(number),
        Written::Decimal(number) => Value::from(number),
    };
    set_value(text, setting.section(), key, written)
}

/// `text` with no setting naming the theme called `name`: each of `theme`,
/// `theme_light` and `theme_dark` under `[ui]` that names it is taken out,
/// and is then as it is when the file does not say. For a theme that is
/// about to be deleted. A file Quill cannot read is not changed.
pub fn forget_theme(text: &str, name: &str) -> Result<String, String> {
    Config::parse(text).map_err(|err| format!("config.toml has a mistake to fix first: {err}"))?;
    let mut document: DocumentMut = text
        .parse()
        .map_err(|err| format!("config.toml cannot be read: {err}"))?;
    let Some(ui) = document.get_mut("ui").and_then(Item::as_table_like_mut) else {
        return Ok(text.to_owned());
    };
    let mut any = false;
    for key in ["theme", "theme_light", "theme_dark"] {
        if ui.get(key).and_then(Item::as_str) == Some(name) {
            ui.remove(key);
            any = true;
        }
    }
    if !any {
        return Ok(text.to_owned());
    }
    checked(document.to_string())
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
    fn an_account_is_taken_out_and_everything_else_is_left_as_it_was() {
        let text = "# My settings\n\
            [ui]\n\
            theme = \"light\"   # easier on the eyes\n\
            \n\
            # The office.\n\
            [[account]]\n\
            name = \"Work\"\n\
            type = \"graph\"\n\
            address = \"robin@example.com\"\n\
            \n\
            # Mine.\n\
            [[account]]\n\
            name = \"Home\"   # the old one\n\
            type = \"imap\"\n\
            address = \"robin@example.org\"\n\
            imap = { host = \"mail.example.org\" }\n\
            \n\
            [compose]\n\
            editor = \"vim\"\n";
        let without_work = remove_account(text, "Work").unwrap();
        assert_eq!(
            without_work,
            text.replace(
                "# The office.\n[[account]]\nname = \"Work\"\ntype = \"graph\"\n\
                 address = \"robin@example.com\"\n\n",
                ""
            )
        );
        let config = Config::parse(&without_work).unwrap();
        assert_eq!(config.accounts.len(), 1);
        assert_eq!(config.accounts[0].name, "Home");
        // The last one too: no accounts is a config Quill reads.
        let none = remove_account(&without_work, "Home").unwrap();
        assert!(!none.contains("[[account]]") && !none.contains("example.org"));
        assert!(
            none.starts_with("# My settings\n[ui]\ntheme = \"light\"   # easier on the eyes\n")
        );
        assert!(none.ends_with("[compose]\neditor = \"vim\"\n"));
        assert!(Config::parse(&none).unwrap().accounts.is_empty());

        // An account that is not there is said, and a name is the whole
        // name, as it is written.
        for absent in ["Club", "work", "Wor", ""] {
            let refused = remove_account(text, absent).unwrap_err();
            assert!(refused.contains("there is no account called"), "{refused}");
        }
        assert!(remove_account("", "Work").is_err());
        // Accounts written another way are not edited, and a file with a
        // mistake in it is not touched.
        let inline =
            "account = [{ name = \"Work\", type = \"graph\", address = \"robin@example.com\" }]\n";
        assert!(
            remove_account(inline, "Work")
                .unwrap_err()
                .contains("by hand")
        );
        assert!(remove_account("[[account]\nname = \"Work\"\n", "Work").is_err());
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
    fn where_an_edge_between_panes_was_left_is_written_and_taken_out_again() {
        let before = "[ui]\ntheme = \"light\"\nreader_width = 0.4  # the list is the wider\n";
        let wide = set_sidebar_width(before, Some(34)).unwrap();
        assert_eq!(
            wide,
            "[ui]\ntheme = \"light\"\nreader_width = 0.4  # the list is the wider\nsidebar_width = 34\n"
        );
        // The message's share is changed where it stands, comment and all.
        let shared = set_reader_width(&wide, Some(625)).unwrap();
        assert_eq!(
            shared,
            "[ui]\ntheme = \"light\"\nreader_width = 0.625  # the list is the wider\nsidebar_width = 34\n"
        );
        let config = Config::parse(&shared).unwrap();
        assert_eq!(config.ui.sidebar_width, Some(34));
        assert_eq!(config.ui.reader_width, 0.625);
        // Taken out, each is as when the file never said.
        let back = set_sidebar_width(&shared, None).unwrap();
        let back = set_reader_width(&back, None).unwrap();
        assert_eq!(back, "[ui]\ntheme = \"light\"\n");
        assert_eq!(set_sidebar_width(&back, None).unwrap(), back);
        // A file with none of it gets a section for it.
        assert_eq!(
            set_sidebar_width("", Some(20)).unwrap(),
            "[ui]\nsidebar_width = 20\n"
        );
        // Nothing the config would refuse is written.
        assert!(set_sidebar_width(before, Some(3)).is_err());
        assert!(set_reader_width(before, Some(950)).is_err());
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
    fn the_accounts_agents_see_are_written_as_a_list_and_taken_out_for_all() {
        let named = |names: &[&str]| {
            Change::AgentAccounts(Some(names.iter().map(|name| (*name).to_owned()).collect()))
        };
        let text = "# Mine.\n[agent]\nmcp = true # for Claude\n";
        let one = set(text, &named(&["Work"])).unwrap();
        assert_eq!(
            one,
            "# Mine.\n[agent]\nmcp = true # for Claude\naccounts = [\"Work\"]\n"
        );
        assert_eq!(
            Config::parse(&one).unwrap().agent.accounts,
            Some(vec!["Work".to_owned()])
        );
        // Another list takes its place, and a name is written as it is,
        // whatever is in it.
        let two = set(&one, &named(&["Home", "Ana's \"old\" mail"])).unwrap();
        assert_eq!(
            Config::parse(&two).unwrap().agent.accounts,
            Some(vec!["Home".to_owned(), "Ana's \"old\" mail".to_owned()])
        );
        // Every account: the line goes, and the file is as it was.
        assert_eq!(set(&two, &Change::AgentAccounts(None)).unwrap(), text);
        // And the most messages at once, a number like any other.
        let most = set(text, &Change::AgentBulkLimit(200)).unwrap();
        assert_eq!(Config::parse(&most).unwrap().agent.bulk_limit, 200);
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
            google: None,
        }
    }

    fn google(secret: Option<&str>) -> SignInDraft {
        let server = |host: &str, port| ServerDraft {
            host: host.to_owned(),
            port,
            tls: TlsMode::Implicit,
            username: "robin@gmail.com".to_owned(),
        };
        SignInDraft {
            name: "Personal".to_owned(),
            address: "robin@gmail.com".to_owned(),
            application_id: "123-abc.apps.googleusercontent.com".to_owned(),
            tenant: None,
            google: Some(quill_tui::setup::GoogleDraft {
                client_secret: secret.map(str::to_owned),
                imap: server("imap.gmail.com", 993),
                smtp: Some(server("smtp.gmail.com", 465)),
            }),
        }
    }

    #[test]
    fn an_account_that_signs_in_with_google_is_an_imap_account_with_oauth_and_its_application() {
        // What docs/oauth.md has the user write by hand, key for key.
        let text = add_sign_in_account("", &google(Some("GOCSPX-not-a-real-one"))).unwrap();
        assert_eq!(
            text,
            "[[account]]\nname = \"Personal\"\ntype = \"imap\"\naddress = \"robin@gmail.com\"\n\
             auth = \"oauth\"\noauth_client_id = \"123-abc.apps.googleusercontent.com\"\n\
             oauth_client_secret = \"GOCSPX-not-a-real-one\"\n\
             imap = { host = \"imap.gmail.com\", port = 993, tls = \"implicit\" }\n\
             smtp = { host = \"smtp.gmail.com\", port = 465, tls = \"implicit\" }\n"
        );
        let account = &Config::parse(&text).unwrap().accounts[0];
        assert_eq!(account.kind, quill_core::config::AccountType::Imap);
        assert_eq!(account.auth, quill_core::config::AuthMethod::Oauth);
        assert_eq!(
            account.oauth_client_secret.as_deref(),
            Some("GOCSPX-not-a-real-one")
        );
        // It is one that signs in with Google, as far as the rest of
        // Quill goes by the config.
        let setup = crate::oauth::setup(account).unwrap().unwrap();
        assert_eq!(setup.provider.name, "google");
        // With no secret given, none is written.
        let text = add_sign_in_account("", &google(None)).unwrap();
        assert!(!text.contains("oauth_client_secret"), "{text}");
        assert!(text.contains("oauth_client_id"), "{text}");
        // After what the user wrote, which stays as it was.
        let before = "# Mine\n[[account]]\nname = \"Home\"   # the old one\ntype = \"imap\"\n\
                      address = \"robin@example.org\"\nimap = { host = \"mail.example.org\" }\n";
        let after = add_sign_in_account(before, &google(Some("s"))).unwrap();
        assert!(after.starts_with(before), "{after}");
        assert_eq!(Config::parse(&after).unwrap().accounts.len(), 2);
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
    fn an_accounts_servers_are_changed_and_the_rest_of_it_is_kept() {
        let pin = "ab".repeat(32);
        let before = format!(
            "# Mine.\n[ui]\ntheme = \"quill-dark\"\n\n\
             [[account]]\nname = \"Work\"\ntype = \"imap\"\naddress = \"robin@example.com\"\n\
             signature = \"Robin\" # as I sign\n\
             imap = {{ host = \"old.example.com\", fingerprint = \"{pin}\" }} # the office\n\n\
             [[account]]\nname = \"Home\"\ntype = \"imap\"\naddress = \"robin@example.org\"\n\n\
             [account.imap]\nhost = \"home.example.org\"\nfingerprint = \"{pin}\"\n\n\
             [account.smtp]\nhost = \"out.example.org\"\n"
        );
        let server = |host: &str, port, username: &str| ServerDraft {
            host: host.to_owned(),
            port,
            tls: TlsMode::Starttls,
            username: username.to_owned(),
        };
        // Work: another reading server, a name to sign in with, and a
        // sending server where there was none.
        let work = AccountDraft {
            name: "Work".to_owned(),
            address: "robin@example.com".to_owned(),
            imap: server("mail.example.com", 143, "robin"),
            smtp: Some(server("mail.example.com", 587, "robin")),
        };
        let after = change_account(&before, "Work", &work, None).unwrap();
        let config = Config::parse(&after).unwrap();
        let account = &config.accounts[0];
        let reading = account.imap.as_ref().unwrap();
        assert_eq!(
            (
                reading.host.as_str(),
                reading.port,
                reading.tls,
                reading.username.as_deref()
            ),
            (
                "mail.example.com",
                Some(143),
                TlsMode::Starttls,
                Some("robin")
            )
        );
        // The pin was for the server it had: it does not follow it.
        assert_eq!(reading.fingerprint, None);
        let sending = account.smtp.as_ref().unwrap();
        assert_eq!(
            (
                sending.host.as_str(),
                sending.port,
                sending.username.as_deref()
            ),
            ("mail.example.com", Some(587), Some("robin"))
        );
        // The other account, and everything else written, is as it was.
        assert_eq!(
            config.accounts[1],
            Config::parse(&before).unwrap().accounts[1]
        );
        for kept in [
            "# Mine.",
            "theme = \"quill-dark\"",
            "# as I sign",
            "# the office",
        ] {
            assert!(after.contains(kept), "{kept} in {after}");
        }

        // Home, written as tables: the same server at another port keeps
        // its pin, the name to sign in with goes back to the address, and
        // the sending server is taken away.
        let home = AccountDraft {
            name: "Home".to_owned(),
            address: "robin@example.org".to_owned(),
            imap: server("home.example.org", 143, "robin@example.org"),
            smtp: None,
        };
        let after = change_account(&after, "Home", &home, None).unwrap();
        let config = Config::parse(&after).unwrap();
        let account = &config.accounts[1];
        let reading = account.imap.as_ref().unwrap();
        assert_eq!(
            (reading.port, reading.username.as_deref()),
            (Some(143), None)
        );
        assert_eq!(reading.fingerprint.as_deref(), Some(pin.as_str()));
        assert!(account.smtp.is_none());
        assert!(!after.contains("out.example.org"), "{after}");

        // A certificate accepted while the account is changed is its new
        // pin; and an account that is not there is not made up.
        let newer = "cd".repeat(32);
        let after = change_account(&after, "Home", &home, Some(&newer)).unwrap();
        assert_eq!(
            Config::parse(&after).unwrap().accounts[1]
                .imap
                .as_ref()
                .unwrap()
                .fingerprint
                .as_deref(),
            Some(newer.as_str())
        );
        assert!(
            change_account(&after, "Nobody", &home, None)
                .unwrap_err()
                .contains("no account called Nobody")
        );
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

    #[test]
    fn an_accounts_letter_is_set_changed_and_taken_out_and_the_rest_is_left() {
        let before = "# Mine\n[ui]\ntheme = \"light\"\n\n[[account]]\nname = \"robin@example.com\"   # work\n\
                      type = \"imap\"\naddress = \"robin@example.com\"\n\
                      imap = { host = \"mail.example.com\" }\n\n[[account]]\n\
                      name = \"robin@example.org\"\ntype = \"imap\"\n\
                      address = \"robin@example.org\"\nimap = { host = \"mail.example.org\" }\n";
        let letters = |text: &str| -> Vec<Option<String>> {
            Config::parse(text)
                .unwrap()
                .accounts
                .iter()
                .map(|account| account.letter.clone())
                .collect()
        };
        // The second account's, and no other's.
        let set = set_account_letter(before, "robin@example.org", Some('o')).unwrap();
        assert_eq!(letters(&set), [None, Some("o".to_owned())]);
        assert!(set.starts_with(before.trim_end()), "{set}");
        assert!(set.ends_with("letter = \"o\"\n"), "{set}");

        // Changed where it is written, with the comment beside it kept.
        let noted = set.replace("letter = \"o\"\n", "letter = \"o\"  # after the @\n");
        let changed = set_account_letter(&noted, "robin@example.org", Some('7')).unwrap();
        assert!(
            changed.ends_with("letter = \"7\"  # after the @\n"),
            "{changed}"
        );
        // The same letter again writes the same file.
        assert_eq!(
            set_account_letter(&changed, "robin@example.org", Some('7')).unwrap(),
            changed
        );

        // Taken out, the file is as it was before the account had one.
        let gone = set_account_letter(&set, "robin@example.org", None).unwrap();
        assert_eq!(gone, before);
        // And an account that never had one is left exactly as it is.
        assert_eq!(
            set_account_letter(before, "robin@example.com", None).unwrap(),
            before
        );

        // An account there is none of is refused, and so is a letter the
        // file could not be read back with.
        let missing = set_account_letter(before, "Nobody", Some('n')).unwrap_err();
        assert!(missing.contains("no account called Nobody"), "{missing}");
        assert!(set_account_letter("", "Nobody", Some('n')).is_err());
        assert!(set_account_letter(before, "robin@example.com", Some('@')).is_err());
    }
}
