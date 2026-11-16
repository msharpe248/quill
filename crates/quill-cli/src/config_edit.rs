//! Changing `config.toml` for the user: adding an account, pinning a
//! certificate. Everything else in the file, comments and layout included,
//! is left exactly as they wrote it.

use std::fs;
use std::io::{self, Write};
use std::path::Path;

use quill_core::config::{Config, TlsMode};
use quill_tui::setup::{AccountDraft, ServerDraft, SignInDraft};
use toml_edit::{ArrayOfTables, DocumentMut, InlineTable, Item, Table, Value, value};

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
    let mut document: DocumentMut = text
        .parse()
        .map_err(|err| format!("config.toml cannot be read: {err}"))?;
    let had_ui = document.contains_key("ui");
    let ui = document
        .entry("ui")
        .or_insert(Item::Table(Table::new()))
        .as_table_like_mut()
        .ok_or("`ui` in config.toml is not a table")?;
    match ui.get_mut("web_reading").and_then(Item::as_value_mut) {
        // In place, with whatever is written around it.
        Some(existing) => {
            let decor = existing.decor().clone();
            *existing = Value::from(on);
            *existing.decor_mut() = decor;
        }
        None => {
            ui.insert("web_reading", value(on));
        }
    }
    // A section of Quill's own making is set apart from what is above it.
    if !had_ui
        && !text.trim().is_empty()
        && let Some(table) = document.get_mut("ui").and_then(Item::as_table_mut)
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
pub fn write(path: &Path, text: &str) -> io::Result<()> {
    let dir = path
        .parent()
        .ok_or_else(|| io::Error::other("the config file has no directory"))?;
    fs::create_dir_all(dir)?;
    let partial = dir.join(format!(".config.toml.{}", std::process::id()));
    let result = (|| {
        let mut file = fs::File::create(&partial)?;
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
}
