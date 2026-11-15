//! The `quill` binary. `quill tui` is what the app shell runs inside its
//! terminal; run by hand in Ghostty or Kitty it works standalone.

mod paths;
mod setup;

use std::io::IsTerminal;
use std::path::PathBuf;
use std::process::ExitCode;

use anyhow::Context;
use clap::{Args, Parser, Subcommand};
use jiff::tz::TimeZone;
use quill_store::Store;
use quill_tui::state::Notice;
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
}

#[derive(Args)]
struct TuiArgs {
    /// Browse a built-in demo mailbox. Your own mail and data are not touched
    #[arg(long)]
    demo: bool,
    /// Where config.toml, keys.toml and themes/ live [env: QUILL_CONFIG_DIR]
    #[arg(long, value_name = "DIR")]
    config_dir: Option<PathBuf>,
    /// Where the mail database lives [env: QUILL_DATA_DIR]
    #[arg(long, value_name = "DIR")]
    data_dir: Option<PathBuf>,
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
    }
}

async fn tui(args: TuiArgs) -> anyhow::Result<()> {
    if !std::io::stdout().is_terminal() || !std::io::stdin().is_terminal() {
        anyhow::bail!("`quill tui` needs a terminal to draw in");
    }
    let paths = Paths::resolve(args.config_dir, args.data_dir, |name| {
        std::env::var_os(name)
    })?;
    let mode = ColorMode::detect(
        std::env::var("NO_COLOR").ok().as_deref(),
        std::env::var("COLORTERM").ok().as_deref(),
    );
    let loaded = setup::load(&paths, mode, TimeZone::system());
    let mut notices = loaded.notices;

    let store = if args.demo {
        let mut store = Store::open_in_memory().context("cannot create the demo mailbox")?;
        demo::seed(&mut store, app::now())
            .await
            .context("cannot fill the demo mailbox")?;
        store
    } else {
        create_private_dir(&paths.data_dir)
            .with_context(|| format!("cannot create {}", paths.data_dir.display()))?;
        let database = paths.database();
        let store = Store::open(&database)
            .with_context(|| format!("cannot open {}", database.display()))?;
        notices.push(Notice::info(match loaded.config.accounts.len() {
            0 => "No accounts yet: account setup arrives in a later milestone. Try `quill tui --demo`."
                .to_owned(),
            count => format!(
                "{count} configured account(s) will not sync yet: sync arrives in a later milestone. Try `quill tui --demo`."
            ),
        }));
        store
    };

    let attached = quill_host::attach(std::env::var_os(quill_host::SOCKET_VARIABLE).as_deref());
    let mut env = loaded.env;
    // Quill's own shell always draws images. Elsewhere, go by what the
    // terminal says it is.
    env.images = attached.host.is_shell()
        || quill_media::kitty::supported_by_environment(|name| std::env::var(name).ok());
    app::run(store, env, notices, attached)
        .await
        .context("the terminal UI stopped")
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
        let Command::Tui(args) = cli.command;
        assert!(args.demo);
        assert_eq!(args.config_dir, Some(PathBuf::from("/c")));
        assert_eq!(args.data_dir, None);
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
