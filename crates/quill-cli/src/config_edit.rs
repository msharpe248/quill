//! Changing `config.toml` for the user: adding an account, pinning a
//! certificate. Everything else in the file, comments and layout included,
//! is left exactly as they wrote it.

use std::fs;
use std::io::{self, Write};
use std::path::Path;

use quill_core::config::{Config, TlsMode};
use quill_tui::setup::{AccountDraft, ServerDraft};
use toml_edit::{ArrayOfTables, DocumentMut, InlineTable, Item, Table, Value, value};

/// `text` with one more `[[account]]`. `fingerprint`, when given, pins the
/// IMAP server's certificate.
pub fn add_account(
    text: &str,
    draft: &AccountDraft,
    fingerprint: Option<&str>,
) -> Result<String, String> {
    let mut document: DocumentMut = text
        .parse()
        .map_err(|err| format!("config.toml cannot be read: {err}"))?;
    let accounts = document
        .entry("account")
        .or_insert(Item::ArrayOfTables(ArrayOfTables::new()))
        .as_array_of_tables_mut()
        .ok_or("`account` in config.toml is not a list of accounts")?;

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
    accounts.push(account);
    checked(document.to_string())
}

/// `text` with the certificate of `account`'s IMAP server pinned.
pub fn pin_fingerprint(text: &str, account: &str, fingerprint: &str) -> Result<String, String> {
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
    let imap = table
        .get_mut("imap")
        .ok_or_else(|| format!("{account} has no `imap` server in config.toml"))?;
    // Written either as `imap = { ... }` or as a `[account.imap]` table.
    if let Some(inline) = imap.as_inline_table_mut() {
        inline.insert("fingerprint", fingerprint.into());
    } else if let Some(table) = imap.as_table_mut() {
        table["fingerprint"] = value(fingerprint);
    } else {
        return Err(format!("`imap` of {account} in config.toml is not a table"));
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
    fn a_config_that_cannot_be_read_is_not_touched() {
        assert!(add_account("[ui\ntheme = ", &draft(), None).is_err());
        assert!(add_account("account = 3\n", &draft(), None).is_err());
    }

    #[test]
    fn a_certificate_is_pinned_on_the_right_account() {
        let before = "[[account]]\nname = \"Work\"\ntype = \"imap\"\naddress = \"a@example.com\"\n\
                      imap = { host = \"one.example.com\" } # the office\n\n\
                      [[account]]\nname = \"Home\"\ntype = \"imap\"\naddress = \"b@example.com\"\n\n\
                      [account.imap]\nhost = \"two.example.com\"\n";
        let fingerprint = "cd".repeat(32);
        let after = pin_fingerprint(before, "Home", &fingerprint).unwrap();
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

        let after = pin_fingerprint(&after, "Work", &fingerprint).unwrap();
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
        let again = pin_fingerprint(&after, "Work", &"ef".repeat(32)).unwrap();
        assert_eq!(again.matches("fingerprint").count(), 2);

        assert!(pin_fingerprint(before, "Nobody", &fingerprint).is_err());
        assert!(pin_fingerprint("", "Work", &fingerprint).is_err());
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
