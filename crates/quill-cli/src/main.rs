//! The `quill` binary. `quill tui` is what the app shell runs inside its
//! terminal; run by hand in Ghostty or Kitty it works standalone.

mod accounts;
mod config_edit;
mod discover;
mod password;
mod paths;
mod service;
mod setup;

/// The IMAP crate's own test server, for tests here that need a real one: a
/// Dovecot started per test, with TLS in front of it under a certificate no
/// system trusts.
#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
#[path = "../../quill-imap/tests/common/mod.rs"]
mod imap_test_server;

use std::io::IsTerminal;
use std::path::PathBuf;
use std::process::ExitCode;
use std::sync::Arc;

use anyhow::Context;
use clap::{Args, Parser, Subcommand};
use jiff::tz::TimeZone;
use quill_auth::{Keychain, SecretStore};
use quill_core::config::{AccountConfig, Config};
use quill_store::{BlobStore, Store};
use quill_sync::SyncOptions;
use quill_tui::app::{AccountsLink, SyncLink};
use quill_tui::theme::ColorMode;
use quill_tui::{app, demo};

use crate::paths::Paths;

#[derive(Parser)]
#[command(name = "quill", version, about = "A keyboard-first mail client")]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    /// Open the mail UI in this terminal
    Tui(TuiArgs),
    /// Manage the saved passwords of the accounts in config.toml
    Account {
        #[command(subcommand)]
        action: AccountAction,
        #[command(flatten)]
        dirs: Dirs,
    },
}

#[derive(Subcommand)]
enum AccountAction {
    /// List the configured accounts and whether each has a saved password
    List,
    /// Save an account's password in the Keychain
    Password {
        /// The account's `name` in config.toml
        name: String,
    },
    /// Remove an account's password from the Keychain
    Forget {
        /// The account's `name` in config.toml
        name: String,
    },
}

#[derive(Args)]
struct Dirs {
    /// Where config.toml, keys.toml and themes/ live [env: QUILL_CONFIG_DIR]
    #[arg(long, value_name = "DIR", global = true)]
    config_dir: Option<PathBuf>,
    /// Where the mail database lives [env: QUILL_DATA_DIR]
    #[arg(long, value_name = "DIR", global = true)]
    data_dir: Option<PathBuf>,
}

#[derive(Args)]
struct TuiArgs {
    /// Browse a built-in demo mailbox. Your own mail and data are not touched
    #[arg(long)]
    demo: bool,
    #[command(flatten)]
    dirs: Dirs,
}

fn main() -> ExitCode {
    match run(Cli::parse()) {
        Ok(()) => ExitCode::SUCCESS,
        Err(err) => {
            eprintln!("quill: {err:#}");
            ExitCode::FAILURE
        }
    }
}

fn run(cli: Cli) -> anyhow::Result<()> {
    let runtime = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
        .context("cannot start the async runtime")?;
    match cli.command {
        Command::Tui(args) => runtime.block_on(tui(args)),
        Command::Account { action, dirs } => account(action, dirs, &Keychain::new()),
    }
}

async fn tui(args: TuiArgs) -> anyhow::Result<()> {
    if !std::io::stdout().is_terminal() || !std::io::stdin().is_terminal() {
        anyhow::bail!("`quill tui` needs a terminal to draw in");
    }
    let paths = Paths::resolve(args.dirs.config_dir, args.dirs.data_dir, |name| {
        std::env::var_os(name)
    })?;
    let mode = ColorMode::detect(
        std::env::var("NO_COLOR").ok().as_deref(),
        std::env::var("COLORTERM").ok().as_deref(),
    );
    let loaded = setup::load(&paths, mode, TimeZone::system());
    let mut notices = loaded.notices;

    let (store, sync, accounts) = if args.demo {
        let mut store = Store::open_in_memory().context("cannot create the demo mailbox")?;
        demo::seed(&mut store, app::now())
            .await
            .context("cannot fill the demo mailbox")?;
        (store, SyncLink::none(), AccountsLink::none())
    } else {
        create_private_dir(&paths.data_dir)
            .with_context(|| format!("cannot create {}", paths.data_dir.display()))?;
        let database = paths.database();
        let open = || {
            Store::open(&database).with_context(|| format!("cannot open {}", database.display()))
        };
        // The UI reads raw messages too: the reader draws a message's HTML
        // from the message as it arrived.
        let mut store = open()?.with_blobs(BlobStore::new(paths.data_dir.join("blobs")));
        let secrets: Arc<dyn SecretStore> = Arc::new(Keychain::new());
        let prepared = accounts::prepare(&loaded.config, &mut store, &secrets, loaded.config_ok)
            .context("cannot register the accounts")?;
        notices.extend(prepared.notices);
        let (events_tx, events) = tokio::sync::mpsc::unbounded_channel();
        // The engine runs even with no accounts, so that one added from the
        // setup flow starts syncing at once. It has a connection of its
        // own; the UI keeps this one.
        let handle = Arc::new(quill_sync::spawn(
            open()?,
            Some(paths.data_dir.join("blobs")),
            prepared.accounts,
            SyncOptions::default(),
            events_tx,
        ));
        let accounts = service::spawn(service::Service {
            config_file: paths.config_file(),
            secrets,
            store: std::sync::Mutex::new(open()?),
            sync: Arc::clone(&handle),
            dns: Box::new(discover::SystemDns::new()),
            prober: Box::new(discover::NetworkProber),
        });
        (
            store,
            SyncLink {
                handle: Some(handle),
                events,
            },
            accounts,
        )
    };

    let attached = quill_host::attach(std::env::var_os(quill_host::SOCKET_VARIABLE).as_deref());
    let mut env = loaded.env;
    // Quill's own shell always draws images. Elsewhere, go by what the
    // terminal says it is.
    // The demo mailbox has no config to add an account to.
    env.accounts = !args.demo;
    env.images = attached.host.is_shell()
        || quill_media::kitty::supported_by_environment(|name| std::env::var(name).ok());
    app::run(store, env, notices, attached, sync, accounts)
        .await
        .context("the terminal UI stopped")
}

/// `quill account ...`: the saved passwords of the configured accounts.
fn account(action: AccountAction, dirs: Dirs, secrets: &dyn SecretStore) -> anyhow::Result<()> {
    let paths = Paths::resolve(dirs.config_dir, dirs.data_dir, |name| {
        std::env::var_os(name)
    })?;
    let config = Config::load(&paths.config_file())?;
    let find = |name: &str| -> anyhow::Result<&AccountConfig> {
        config
            .accounts
            .iter()
            .find(|account| account.name == name)
            .with_context(|| {
                format!(
                    "there is no account named \"{name}\" in {}",
                    paths.config_file().display()
                )
            })
    };
    let imap = |account: &AccountConfig| {
        accounts::imap_config(account)
            .map_err(|reason| anyhow::anyhow!("{} cannot sync: {reason}", account.name))
    };
    match action {
        AccountAction::List => {
            if config.accounts.is_empty() {
                println!("No accounts in {}.", paths.config_file().display());
            }
            for account in &config.accounts {
                let status = match accounts::imap_config(account) {
                    Ok(imap) => match secrets.get(&accounts::imap_password_name(&imap))? {
                        Some(_) => "password saved".to_owned(),
                        None => "no password saved".to_owned(),
                    },
                    Err(reason) => format!("cannot sync: {reason}"),
                };
                println!(
                    "{}  {}  {}  {status}",
                    account.name,
                    account.kind.as_str(),
                    account.address
                );
            }
        }
        AccountAction::Password { name } => {
            let imap = imap(find(&name)?)?;
            let prompt = format!("Password for {} on {}: ", imap.username, imap.host);
            let secret = password::read(&prompt)?;
            anyhow::ensure!(!secret.is_empty(), "no password given; nothing was saved");
            secrets.set(&accounts::imap_password_name(&imap), &secret)?;
            println!("Saved in the Keychain.");
        }
        AccountAction::Forget { name } => {
            let imap = imap(find(&name)?)?;
            secrets.delete(&accounts::imap_password_name(&imap))?;
            println!("Removed from the Keychain.");
        }
    }
    Ok(())
}

/// Mail is private: the data directory is readable by its owner only.
fn create_private_dir(dir: &std::path::Path) -> std::io::Result<()> {
    let mut builder = std::fs::DirBuilder::new();
    builder.recursive(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::DirBuilderExt;
        builder.mode(0o700);
    }
    builder.create(dir)
}

#[cfg(test)]
mod tests {
    use clap::CommandFactory;

    use super::*;

    #[test]
    fn the_command_line_is_well_formed() {
        Cli::command().debug_assert();
    }

    #[test]
    fn tui_flags_parse() {
        let cli = Cli::try_parse_from(["quill", "tui", "--demo", "--config-dir", "/c"]).unwrap();
        let Command::Tui(args) = cli.command else {
            panic!("not the tui subcommand");
        };
        assert!(args.demo);
        assert_eq!(args.dirs.config_dir, Some(PathBuf::from("/c")));
        assert_eq!(args.dirs.data_dir, None);
        assert!(Cli::try_parse_from(["quill"]).is_err());
        assert!(Cli::try_parse_from(["quill", "tui", "--bogus"]).is_err());
    }

    #[cfg(unix)]
    #[test]
    fn the_data_directory_is_created_owner_only() {
        use std::os::unix::fs::PermissionsExt;
        let dir = tempfile::tempdir().unwrap();
        let data = dir.path().join("Quill").join("data");
        create_private_dir(&data).unwrap();
        create_private_dir(&data).unwrap();
        let mode = std::fs::metadata(&data).unwrap().permissions().mode();
        assert_eq!(mode & 0o777, 0o700);
    }
}
