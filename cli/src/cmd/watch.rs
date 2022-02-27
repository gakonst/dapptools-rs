//! Watch mode support

use crate::cmd::build::BuildArgs;
use clap::Parser;
use foundry_config::Config;
use std::{convert::Infallible, path::PathBuf};
use watchexec::{
    action::{Action, Outcome, PreSpawn},
    command::Shell,
    config::{InitConfig, RuntimeConfig},
    event::{Event, ProcessEnd},
    handler::SyncFnHandler,
    paths::summarise_events_to_env,
    signal::source::MainSignal,
    Watchexec,
};

use crate::utils;

/// Executes a [`Watchexec`] that listens for changes in the project's src dir and reruns `forge
/// build`

// TODO in order to support this dynamically in forge test (change the command to run based on
// changed paths) we need to create an additional channel, observe the `Action` send a message to
// the channel rx and reconfigure the RuntimeConfig.
pub async fn watch_build(mut args: BuildArgs) -> eyre::Result<()> {
    let init = init()?;
    let mut runtime = runtime(&args.watch)?;

    // contains all the arguments `--watch p1, p2, p3`
    let paths = args.watch.watch.take().unwrap_or_default();

    if paths.is_empty() {
        // listen for changes in the project's src dir
        let config = Config::from(&args);
        runtime.pathset(Some(config.src));
    }

    // all the forge arguments including path to forge bin
    let mut cmd_args: Vec<_> = std::env::args().collect();

    // need to remove the `--watch` flag from the args for the Watchexec command
    if let Some(pos) = cmd_args.iter().position(|arg| arg == "--watch" || arg == "-w") {
        cmd_args.drain(pos..=(pos + paths.len()));
    }
    runtime.command(cmd_args);

    let wx = Watchexec::new(init, runtime)?;
    // start immediately
    wx.send_event(Event::default()).await?;
    wx.main().await??;

    Ok(())
}

#[derive(Debug, Clone, Parser, Default)]
pub struct WatchArgs {
    /// File updates debounce delay
    ///
    /// During this time, incoming change events are accumulated and
    /// only once the delay has passed, is an action taken. Note that
    /// this does not mean a command will be started: if --no-restart is
    /// given and a command is already running, the outcome of the
    /// action will be to do nothing.
    ///
    /// Defaults to 50ms. Parses as decimal seconds by default, but
    /// using an integer with the `ms` suffix may be more convenient.
    /// When using --poll mode, you'll want a larger duration, or risk
    /// overloading disk I/O.
    #[clap(short = 'd', long = "delay", forbid_empty_values = true)]
    pub delay: Option<String>,

    /// Shell to use for the command, or `none` for direct execution
    #[cfg_attr(
        windows,
        doc = "Defaults to Powershell. Examples: --use-shell=cmd, --use-shell=gitbash.exe"
    )]
    #[cfg_attr(
        not(windows),
        doc = "Defaults to $SHELL. Examples: --use-shell=sh, --use-shell=fish"
    )]
    #[clap(long = "use-shell", value_name = "shell")]
    pub shell: Option<String>,

    /// Don’t restart command while it’s still running
    #[clap(long = "no-restart")]
    pub no_restart: bool,

    /// Show paths that changed
    #[clap(long = "why")]
    pub why: bool,

    /// Watch specific file(s) or folder(s)
    ///
    /// By default, the project's source dir is watched
    #[clap(
        short = 'w',
        long = "watch",
        value_name = "path",
        multiple_values = true,
        multiple_occurrences = false
    )]
    pub watch: Option<Vec<PathBuf>>,
}

/// Returns the Initialisation configuration for [`Watchexec`].
pub fn init() -> eyre::Result<InitConfig> {
    let mut config = InitConfig::default();
    config.on_error(SyncFnHandler::from(|data| -> std::result::Result<(), Infallible> {
        eprintln!("[[{:?}]]", data);
        Ok(())
    }));

    Ok(config)
}

/// Returns the Runtime configuration for [`Watchexec`].
pub fn runtime(args: &WatchArgs) -> eyre::Result<RuntimeConfig> {
    let mut config = RuntimeConfig::default();

    // config.pathset(args.watch.clone().unwrap_or_default());
    config.pathset(vec!["/Users/Matthias/git/rust/foundry/cli"]);

    if let Some(delay) = &args.delay {
        config.action_throttle(utils::parse_delay(delay)?);
    }

    config.command_shell(if let Some(s) = &args.shell {
        if s.eq_ignore_ascii_case("powershell") {
            Shell::Powershell
        } else if s.eq_ignore_ascii_case("none") {
            Shell::None
        } else if s.eq_ignore_ascii_case("cmd") {
            cmd_shell(s.into())
        } else {
            Shell::Unix(s.into())
        }
    } else {
        default_shell()
    });

    let on_busy = if args.no_restart { "do-nothing" } else { "restart" };

    let print_events = args.why;

    config.on_action(move |action: Action| {
        let fut = async { Ok::<(), Infallible>(()) };
        if print_events {
            for (n, event) in action.events.iter().enumerate() {
                eprintln!("[EVENT {}] {}", n, event);
            }
        }

        let signals: Vec<MainSignal> = action.events.iter().flat_map(|e| e.signals()).collect();
        let has_paths = action.events.iter().flat_map(|e| e.paths()).next().is_some();

        if signals.contains(&MainSignal::Terminate) {
            action.outcome(Outcome::both(Outcome::Stop, Outcome::Exit));
            return fut
        }

        if signals.contains(&MainSignal::Interrupt) {
            action.outcome(Outcome::both(Outcome::Stop, Outcome::Exit));
            return fut
        }

        if !has_paths {
            if !signals.is_empty() {
                let mut out = Outcome::DoNothing;
                for sig in signals {
                    out = Outcome::both(out, Outcome::Signal(sig.into()));
                }

                action.outcome(out);
                return fut
            }

            let completion = action.events.iter().flat_map(|e| e.completions()).next();
            if let Some(status) = completion {
                let (msg, printit) = match status {
                    Some(ProcessEnd::ExitError(code)) => {
                        (format!("Command exited with {}", code), true)
                    }
                    Some(ProcessEnd::ExitSignal(sig)) => {
                        (format!("Command killed by {:?}", sig), true)
                    }
                    Some(ProcessEnd::ExitStop(sig)) => {
                        (format!("Command stopped by {:?}", sig), true)
                    }
                    Some(ProcessEnd::Continued) => ("Command continued".to_string(), true),
                    Some(ProcessEnd::Exception(ex)) => {
                        (format!("Command ended by exception {:#x}", ex), true)
                    }
                    Some(ProcessEnd::Success) => ("Command was successful".to_string(), false),
                    None => ("Command completed".to_string(), false),
                };

                if printit {
                    eprintln!("[[{}]]", msg);
                }

                action.outcome(Outcome::DoNothing);
                return fut
            }
        }

        // TODO make this configurable
        let clear = true;
        let when_running = match (clear, on_busy) {
            (_, "do-nothing") => Outcome::DoNothing,
            (true, "restart") => {
                Outcome::both(Outcome::Stop, Outcome::both(Outcome::Clear, Outcome::Start))
            }
            (false, "restart") => Outcome::both(Outcome::Stop, Outcome::Start),
            _ => Outcome::DoNothing,
        };

        let when_idle =
            if clear { Outcome::both(Outcome::Clear, Outcome::Start) } else { Outcome::Start };

        action.outcome(Outcome::if_running(when_running, when_idle));

        fut
    });

    config.command(["ls"]);

    config.on_pre_spawn(move |prespawn: PreSpawn| async move {
        let envs = summarise_events_to_env(prespawn.events.iter());
        if let Some(mut command) = prespawn.command().await {
            for (k, v) in envs {
                command.env(format!("CARGO_WATCH_{}_PATH", k), v);
            }
        }

        Ok::<(), Infallible>(())
    });

    Ok(config)
}

#[cfg(windows)]
fn default_shell() -> Shell {
    Shell::Powershell
}

#[cfg(not(windows))]
fn default_shell() -> Shell {
    Shell::default()
}

// because Shell::Cmd is only on windows
#[cfg(windows)]
fn cmd_shell(_: String) -> Shell {
    Shell::Cmd
}

#[cfg(not(windows))]
fn cmd_shell(s: String) -> Shell {
    Shell::Unix(s)
}
