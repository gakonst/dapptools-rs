use crate::cmd::utils::Cmd;

use clap::{Parser, ValueHint};
use ethers::contract::MultiAbigen;
use foundry_config::{
    figment::{
        self,
        error::Kind::InvalidType,
        value::{Dict, Map, Value},
        Metadata, Profile, Provider,
    },
    impl_figment_convert, Config,
};
use serde::Serialize;
use std::{fs, path::PathBuf};

impl_figment_convert!(BindArgs);

static DEFAULT_CRATE_NAME: &str = "foundry-contracts";
static DEFAULT_CRATE_VERSION: &str = "0.0.1";

#[derive(Debug, Clone, Parser, Serialize)]
pub struct BindArgs {
    #[clap(
        help = "The project's root path. By default, this is the root directory of the current Git repository or the current working directory if it is not part of a Git repository",
        long,
        value_hint = ValueHint::DirPath,
        value_name = "PATH"
    )]
    #[serde(skip)]
    pub root: Option<PathBuf>,

    #[clap(
        help = "Path to where the contract artifacts are stored",
        long = "bindings-path",
        short,
        value_hint = ValueHint::DirPath,
        value_name = "PATH"
    )]
    #[serde(skip)]
    pub bindings: Option<PathBuf>,

    #[clap(
        long = "crate-name",
        help = "The name of the Rust crate to generate. This should be a valid crates.io crate name. However, it is not currently validated by this command.",
        default_value = DEFAULT_CRATE_NAME,
    )]
    #[serde(skip)]
    crate_name: String,

    #[clap(
        long = "crate-version",
        help = "The version of the Rust crate to generate. This should be a standard semver version string. However, it is not currently validated by this command.",
        default_value = DEFAULT_CRATE_VERSION,
        value_name = "NAME"
    )]
    #[serde(skip)]
    crate_version: String,

    #[clap(long, help = "Generate the bindings as a module instead of a crate")]
    module: bool,

    #[clap(
        long = "overwrite",
        help = "Overwrite existing generated bindings. By default, the command will check that the bindings are correct, and then exit. If --overwrite is passed, it will instead delete and overwrite the bindings."
    )]
    #[serde(skip)]
    overwrite: bool,

    #[clap(long = "single-file", help = "Generate bindings as a single file.")]
    #[serde(skip)]
    single_file: bool,
}

impl BindArgs {
    /// Get the path to the foundry artifacts directory
    fn artifacts(&self) -> PathBuf {
        let c: Config = self.into();
        c.out
    }

    /// Get the path to the root of the autogenerated crate
    fn bindings_root(&self) -> PathBuf {
        self.bindings.clone().unwrap_or_else(|| self.artifacts().join("bindings"))
    }

    /// `true` if the bindings root already exists
    fn bindings_exist(&self) -> bool {
        self.bindings_root().is_dir()
    }

    /// Instantiate the multi-abigen
    fn get_multi(&self) -> eyre::Result<MultiAbigen> {
        let multi = MultiAbigen::from_json_files(self.artifacts())?;

        eyre::ensure!(
            !multi.is_empty(),
            r#"
No contract artifacts found. Hint: Have you built your contracts yet? `forge bind` does not currently invoke `forge build`, although this is planned for future versions.
            "#
        );
        Ok(multi)
    }

    /// Check that the existing bindings match the expected abigen output
    fn check_existing_bindings(&self) -> eyre::Result<()> {
        let bindings = self.get_multi()?.build()?;
        println!("Checking bindings for {} contracts.", bindings.len());
        if !self.module {
            bindings.ensure_consistent_crate(
                &self.crate_name,
                &self.crate_version,
                self.bindings_root(),
                self.single_file,
            )?;
        } else {
            bindings.ensure_consistent_module(self.bindings_root(), self.single_file)?;
        }
        println!("OK.");
        Ok(())
    }

    /// Generate the bindings
    fn generate_bindings(&self) -> eyre::Result<()> {
        let bindings = self.get_multi()?.build()?;
        println!("Generating bindings for {} contracts", bindings.len());
        if !self.module {
            bindings.write_to_crate(
                &self.crate_name,
                &self.crate_version,
                self.bindings_root(),
                self.single_file,
            )?;
        } else {
            bindings.write_to_module(self.bindings_root(), self.single_file)?;
        }
        Ok(())
    }
}

impl Cmd for BindArgs {
    type Output = ();

    fn run(self) -> eyre::Result<Self::Output> {
        if !self.overwrite && self.bindings_exist() {
            println!("Bindings found. Checking for consistency.");
            return self.check_existing_bindings()
        }

        if self.overwrite {
            fs::remove_dir_all(self.bindings_root())?;
        }

        self.generate_bindings()?;

        println!("Bindings have been output to {}", self.bindings_root().to_str().unwrap());
        Ok(())
    }
}

// Make this args a `figment::Provider` so that it can be merged into the `Config`
impl Provider for BindArgs {
    fn metadata(&self) -> Metadata {
        Metadata::named("Bind Args Provider")
    }

    fn data(&self) -> Result<Map<Profile, Dict>, figment::Error> {
        let value = Value::serialize(self)?;
        let error = InvalidType(value.to_actual(), "map".into());
        let dict = value.into_dict().ok_or(error)?;
        Ok(Map::from([(Config::selected_profile(), dict)]))
    }
}
