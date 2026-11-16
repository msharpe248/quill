//! The `quill` binary. `quill tui` is what the app shell runs inside its
//! terminal; run by hand in Ghostty or Kitty it works standalone.

mod accounts;
mod config_edit;
mod discover;
mod headless;
mod oauth;
mod password;
mod paths;
mod rules;
mod service;
mod setup;
mod signin;

/// The IMAP crate's own test server, for tests here that need a real one: a
/// Dovecot started per test, with TLS in front of it under a certificate no
/// system trusts.
#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
#[path = "../../quill-imap/tests/common/mod.rs"]
mod imap_test_server;

/// The POP3 crate's test server, likewise.
#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
#[path = "../../quill-pop3/tests/common/mod.rs"]
mod pop3_test_server;

#[cfg(test)]
mod pop3_tests;

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
use quill_tui::app::SyncLink;
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
    /// Manage the saved passwords and sign-ins of the accounts in config.toml
    Account {
        #[command(subcommand)]
        action: AccountAction,
        #[command(flatten)]
        dirs: Dirs,
    },
    /// List messages: the unified inbox, or one folder
    List {
        /// A folder, as after `in:` in a search: `work/sent`, `archive`
        #[arg(long = "in", value_name = "FOLDER")]
        within: Option<String>,
        /// Only unread messages
        #[arg(long)]
        unread: bool,
        /// At most this many, newest first
        #[arg(long, default_value_t = 50)]
        limit: usize,
        #[command(flatten)]
        output: Output,
    },
    /// Search mail, with the same words and filters as the search box
    Search {
        /// What to find: `from:ana is:unread "exact phrase"`
        #[arg(required = true)]
        words: Vec<String>,
        /// At most this many, newest first
        #[arg(long, default_value_t = 50)]
        limit: usize,
        #[command(flatten)]
        output: Output,
    },
    /// Print one message, by the id `list` and `search` show
    Read {
        id: i64,
        /// The message exactly as it arrived, headers and all
        #[arg(long, conflicts_with = "json")]
        raw: bool,
        #[command(flatten)]
        output: Output,
    },
    /// Put a message in the outbox. The body is read from standard input
    Send {
        /// A recipient. Repeat for more
        #[arg(long, value_name = "ADDRESS")]
        to: Vec<String>,
        #[arg(long, value_name = "ADDRESS")]
        cc: Vec<String>,
        #[arg(long, value_name = "ADDRESS")]
        bcc: Vec<String>,
        #[arg(long, default_value = "")]
        subject: String,
        /// The account to send from, by its name. The first one otherwise
        #[arg(long, value_name = "NAME")]
        account: Option<String>,
        /// A file to attach. Repeat for more
        #[arg(long, value_name = "FILE")]
        attach: Vec<String>,
        #[command(flatten)]
        output: Output,
    },
    /// Fetch new mail, carry out what is queued and send what is waiting,
    /// once, and stop
    Sync {
        #[command(flatten)]
        output: Output,
    },
}

#[derive(Args)]
struct Output {
    /// Print JSON
    #[arg(long)]
    json: bool,
    #[command(flatten)]
    dirs: Dirs,
}

#[derive(Subcommand)]
enum AccountAction {
    /// List the configured accounts and whether each has a saved password
    /// or sign-in
    List,
    /// Sign an account in to Microsoft or Google, in the browser
    SignIn {
        /// The account's `name` in config.toml
        name: String,
        /// Sign in with a code typed on another device, not a browser here
        #[arg(long)]
        code: bool,
    },
    /// Save an account's password in the Keychain
    Password {
        /// The account's `name` in config.toml
        name: String,
    },
    /// Remove an account's password or sign-in from the Keychain
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
        Command::Account { action, dirs } => {
            runtime.block_on(account(action, dirs, Arc::new(Keychain::new())))
        }
        Command::List {
            within,
            unread,
            limit,
            output,
        } => {
            let (store, _) = mail(output.dirs)?;
            headless::list(
                &mut std::io::stdout(),
                &store,
                within.as_deref(),
                unread,
                limit,
                output.json,
            )
        }
        Command::Search {
            words,
            limit,
            output,
        } => {
            let (store, _) = mail(output.dirs)?;
            headless::search(
                &mut std::io::stdout(),
                &store,
                &words.join(" "),
                limit,
                output.json,
            )
        }
        Command::Read { id, raw, output } => {
            let (store, _) = mail(output.dirs)?;
            headless::read(&mut std::io::stdout(), &store, id, raw, output.json)
        }
        Command::Send {
            to,
            cc,
            bcc,
            subject,
            account,
            attach,
            output,
        } => {
            let (mut store, paths) = mail(output.dirs)?;
            let config = Config::load(&paths.config_file())?;
            let mut body = String::new();
            if !std::io::stdin().is_terminal() {
                std::io::Read::read_to_string(&mut std::io::stdin(), &mut body)
                    .context("cannot read the message from standard input")?;
            }
            headless::send(
                &mut std::io::stdout(),
                &mut store,
                &config,
                headless::Message {
                    account,
                    to,
                    cc,
                    bcc,
                    subject,
                    body,
                    attach,
                },
                output.json,
            )
        }
        Command::Sync { output } => {
            let paths = Paths::resolve(output.dirs.config_dir, output.dirs.data_dir, |name| {
                std::env::var_os(name)
            })?;
            let (config, config_ok) = match Config::load(&paths.config_file()) {
                Ok(config) => (config, true),
                Err(err) => anyhow::bail!("{err}. Nothing was synced"),
            };
            create_private_dir(&paths.data_dir)
                .with_context(|| format!("cannot create {}", paths.data_dir.display()))?;
            let fine = runtime.block_on(headless::sync(
                &mut std::io::stdout(),
                &config,
                config_ok,
                &paths.database(),
                paths.data_dir.join("blobs"),
                Arc::new(Keychain::new()),
                output.json,
            ))?;
            anyhow::ensure!(fine, "not everything synced");
            Ok(())
        }
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
        // No accounts to add or sign in to. What the user chooses in the UI
        // is still remembered, in the config this run was pointed at.
        (
            store,
            SyncLink::none(),
            service::spawn_remembering(paths.config_file()),
        )
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
        let handle = Arc::new(quill_sync::spawn_with_rules(
            open()?,
            Some(paths.data_dir.join("blobs")),
            prepared.accounts,
            SyncOptions::default(),
            rules::from_config(&loaded.config),
            events_tx,
        ));
        // Downloaded mail is kept within the room the config gives it.
        if let Some(bytes) = loaded.config.storage.cache_bytes() {
            handle.keep_cache_within(bytes);
        }
        let accounts = service::spawn(service::Service {
            config_file: paths.config_file(),
            secrets,
            store: std::sync::Mutex::new(open()?),
            sync: Arc::clone(&handle),
            dns: Box::new(discover::SystemDns::new()),
            prober: Box::new(discover::NetworkProber),
            trusted_once: std::sync::Mutex::default(),
            remote: over_ssh(),
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
    suit_host(
        &mut env,
        attached.host.is_shell(),
        std::env::var_os("QUILL_VIM"),
    );
    env.images = attached.host.is_shell()
        || quill_media::kitty::supported_by_environment(|name| std::env::var(name).ok());
    app::run(store, env, notices, attached, sync, accounts)
        .await
        .context("the terminal UI stopped")
}

/// Sets what depends on where Quill is running: inside its own app
/// (`shell`), or in a terminal. `vim` is what `QUILL_VIM` says.
fn suit_host(env: &mut quill_tui::state::Env, shell: bool, vim: Option<std::ffi::OsString>) {
    // A terminal program keeps cmd for itself; the shell passes it on.
    env.cmd_keys = shell;
    // And it has a panel to choose files in.
    env.file_panel = shell;
    // And the system's own readers for pictures Quill cannot decode.
    env.thumbnails = shell;
    // And the system's spelling checker.
    env.spell_check = shell;
    // And a sheet on its window for the form that adds an account, which
    // a terminal has drawn over its panes.
    env.account_sheet = shell;
    // And a Vim of its own, in a pane. In a terminal the user's own Vim
    // is run there: `vim`, or the program QUILL_VIM names. The app never
    // runs that one, whatever its environment says.
    env.editor_pane = shell;
    env.vim_program = vim.filter(|name| !shell && !name.is_empty());
    // The app is sandboxed and starts no program outside its bundle, so
    // hooks run only in a terminal.
    env.hooks_enabled = !shell;
    // And a web view pane that can stand in the reader's place. A
    // terminal has none, whatever the config says about reading in one.
    env.web_pane = shell;
}

/// `quill account ...`: the saved passwords and sign-ins of the configured
/// accounts.
async fn account(
    action: AccountAction,
    dirs: Dirs,
    secrets: Arc<dyn SecretStore>,
) -> anyhow::Result<()> {
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
    let reading = |account: &AccountConfig| {
        accounts::reading(account)
            .map_err(|reason| anyhow::anyhow!("{} cannot sync: {reason}", account.name))
    };
    let oauth = |account: &AccountConfig| {
        oauth::setup(account)
            .map_err(|reason| anyhow::anyhow!("{} cannot sign in: {reason}", account.name))
    };
    match action {
        AccountAction::List => {
            if config.accounts.is_empty() {
                println!("No accounts in {}.", paths.config_file().display());
            }
            for account in &config.accounts {
                let status = if oauth::wanted(account) {
                    match oauth::setup(account) {
                        Ok(Some(setup)) => match secrets.get(&setup.secret_name(account))? {
                            Some(_) => "signed in".to_owned(),
                            None => "not signed in".to_owned(),
                        },
                        Ok(None) => "not signed in".to_owned(),
                        Err(reason) => format!("cannot sign in: {reason}"),
                    }
                } else {
                    match accounts::reading(account) {
                        Ok(reading) => match secrets.get(&reading.secret)? {
                            Some(_) => "password saved".to_owned(),
                            None => "no password saved".to_owned(),
                        },
                        Err(reason) => format!("cannot sync: {reason}"),
                    }
                };
                println!(
                    "{}  {}  {}  {status}",
                    account.name,
                    account.kind.as_str(),
                    account.address
                );
            }
        }
        AccountAction::SignIn { name, code } => {
            let terminal = signin::Terminal {
                say: Arc::new(|line| println!("{line}")),
                open: Box::new(|url| {
                    // Not sandboxed here: the system's own opener.
                    let _ = std::process::Command::new("/usr/bin/open")
                        .arg(url)
                        .stdin(std::process::Stdio::null())
                        .stdout(std::process::Stdio::null())
                        .stderr(std::process::Stdio::null())
                        .status();
                }),
                remote: over_ssh(),
            };
            signin::sign_in(find(&name)?, secrets, code, terminal).await?;
        }
        AccountAction::Password { name } => {
            let account = find(&name)?;
            anyhow::ensure!(
                !oauth::wanted(account),
                "{name} signs in with its provider, not a password: quill account sign-in \"{name}\""
            );
            let reading = reading(account)?;
            let prompt = format!("Password for {} on {}: ", reading.username, reading.host);
            let secret = password::read(&prompt)?;
            anyhow::ensure!(!secret.is_empty(), "no password given; nothing was saved");
            secrets.set(&reading.secret, &secret)?;
            println!("Saved in the Keychain.");
        }
        AccountAction::Forget { name } => {
            let account = find(&name)?;
            if let Some(setup) = oauth(account)? {
                secrets.delete(&setup.secret_name(account))?;
            } else {
                secrets.delete(&reading(account)?.secret)?;
            }
            println!("Removed from the Keychain.");
        }
    }
    Ok(())
}

/// Opens the mail there is, for the commands that only look at it or add
/// to the outbox. They make nothing: with no mail yet, they say so.
fn mail(dirs: Dirs) -> anyhow::Result<(Store, Paths)> {
    let paths = Paths::resolve(dirs.config_dir, dirs.data_dir, |name| {
        std::env::var_os(name)
    })?;
    let database = paths.database();
    anyhow::ensure!(
        database.is_file(),
        "there is no mail in {} yet. Run `quill tui` or `quill sync` first",
        paths.data_dir.display()
    );
    let store = Store::open(&database)
        .with_context(|| format!("cannot open {}", database.display()))?
        .with_blobs(BlobStore::new(paths.data_dir.join("blobs")));
    Ok((store, paths))
}

/// Whether whoever is typing is at another computer.
fn over_ssh() -> bool {
    std::env::var_os("SSH_CONNECTION").is_some() || std::env::var_os("SSH_TTY").is_some()
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

    #[test]
    fn the_app_runs_no_programs_and_a_terminal_has_no_panels() {
        let dir = tempfile::tempdir().unwrap();
        let paths = Paths {
            config_dir: dir.path().join("config"),
            data_dir: dir.path().join("data"),
        };
        let load = || setup::load(&paths, ColorMode::None, TimeZone::UTC).env;

        // In Quill's own app: its panels and its Vim, and no hooks.
        let mut env = load();
        suit_host(&mut env, true, Some("/opt/homebrew/bin/nvim".into()));
        assert!(env.cmd_keys && env.file_panel && env.thumbnails && env.spell_check);
        assert!(env.editor_pane && env.account_sheet);
        assert!(!env.hooks_enabled);
        assert!(env.web_pane);
        assert_eq!(env.vim_program, None);

        // In a terminal: the user's own Vim, and their hooks.
        let mut env = load();
        suit_host(&mut env, false, Some("/opt/homebrew/bin/nvim".into()));
        assert!(!env.cmd_keys && !env.file_panel && !env.thumbnails && !env.spell_check);
        assert!(!env.editor_pane && !env.account_sheet);
        assert!(env.hooks_enabled);
        assert!(!env.web_pane);
        assert_eq!(env.vim_program, Some("/opt/homebrew/bin/nvim".into()));
        suit_host(&mut env, false, Some("".into()));
        assert_eq!(env.vim_program, None);
        suit_host(&mut env, false, None);
        assert_eq!(env.vim_program, None);
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
