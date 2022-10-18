use chisel::{
    prelude::{ChiselDisptacher, DispatchResult},
    solidity_helper::SolidityHelper,
};
use clap::Parser;
use foundry_cli::cmd::{forge::build::BuildArgs, LoadConfig};
use foundry_common::evm::EvmArgs;
use foundry_config::{
    figment::{
        value::{Dict, Map},
        Metadata, Profile, Provider,
    },
    Config,
};
use rustyline::{error::ReadlineError, Editor};
use yansi::Paint;

// Loads project's figment and merges the build cli arguments into it
foundry_config::merge_impl_figment_convert!(ChiselParser, opts, evm_opts);

/// Chisel is a fast, utilitarian, and verbose solidity REPL.
#[derive(Debug, Parser)]
#[clap(name = "chisel", version = "v0.0.1-alpha")]
pub struct ChiselParser {
    #[clap(flatten, next_help_heading = "BUILD OPTIONS")]
    pub opts: BuildArgs,

    #[clap(flatten, next_help_heading = "EVM OPTIONS")]
    pub evm_opts: EvmArgs,
}

#[tokio::main]
async fn main() {
    // Parse command args
    let args = ChiselParser::parse();

    // Keeps track of whether or not an interrupt was the last input
    let mut interrupt = false;

    // Create a new rustyline Editor
    let mut rl = Editor::<SolidityHelper>::new().unwrap_or_else(|e| {
        tracing::error!(target: "chisel-env", "Failed to initialize rustyline Editor! {}", e);
        panic!("failed to create a rustyline Editor for the chisel environment! {e}");
    });
    rl.set_helper(Some(SolidityHelper));

    let (config, evm_opts) = args.load_config_and_evm_opts().unwrap();

    // Create a new cli dispatcher
    let mut dispatcher = ChiselDisptacher::new(&chisel::session_source::SessionSourceConfig {
        config,
        evm_opts,
        backend: None,
    });

    // Begin Rustyline loop
    loop {
        // Get the prompt from the dispatcher
        // Variable based on status of the last entry
        let prompt = dispatcher.get_prompt();

        // Read the next line
        let next_string = rl.readline(prompt.as_str());

        // Try to read the string
        match next_string {
            Ok(line) => {
                interrupt = false;
                // Dispatch and match results
                match dispatcher.dispatch(&line).await {
                    DispatchResult::Success(Some(msg))
                    | DispatchResult::CommandSuccess(Some(msg)) => println!("{}", Paint::green(msg)),
                    DispatchResult::UnrecognizedCommand(e) => eprintln!("{}", e),
                    DispatchResult::SolangParserFailed(e) => {
                        eprintln!("{}", Paint::red("Compilation error"));
                        eprintln!("{}", Paint::red(format!("{:?}", e)));
                    }
                    DispatchResult::Success(None) => { /* Do nothing */ }
                    DispatchResult::CommandSuccess(_) => { /* Don't need to do anything here */ }
                    DispatchResult::FileIoError(e) => eprintln!("{}", Paint::red(format!("⚒️ Chisel File IO Error - {}", e))),
                    DispatchResult::CommandFailed(msg) | DispatchResult::Failure(Some(msg)) => eprintln!("{}", Paint::red(msg)),
                    DispatchResult::Failure(None) => eprintln!("{}\nPlease Report this bug as a github issue if it persists: https://github.com/foundry-rs/foundry/issues/new/choose", Paint::red("⚒️ Unknown Chisel Error ⚒️")),
                }
            }
            Err(ReadlineError::Interrupted) => {
                if interrupt {
                    break
                } else {
                    println!("(To exit, press Ctrl+C again)");
                    interrupt = true;
                }
            }
            Err(ReadlineError::Eof) => break,
            Err(err) => {
                println!("Error: {:?}", err);
                break
            }
        }
    }
}

impl Provider for ChiselParser {
    fn metadata(&self) -> Metadata {
        Metadata::named("Script Args Provider")
    }

    fn data(&self) -> Result<Map<Profile, Dict>, foundry_config::figment::Error> {
        Ok(Map::from([(Config::selected_profile(), Dict::default())]))
    }
}
