use super::watch::WatchArgs;
use clap::Parser;
use eyre::Result;
use forge_verify::install_missing_dependencies;
use foundry_cli::{opts::BuildOpts, utils::LoadConfig};
use foundry_common::{compile::ProjectCompiler, shell};
use foundry_compilers::{
    compilers::{multi::MultiCompilerLanguage, Language},
    utils::source_files_iter,
    Project, ProjectCompileOutput,
};
use foundry_config::{
    figment::{
        self,
        error::Kind::InvalidType,
        value::{Dict, Map, Value},
        Metadata, Profile, Provider,
    },
    Config,
};
use serde::Serialize;
use std::path::PathBuf;

foundry_config::merge_impl_figment_convert!(BuildArgs, build);

/// CLI arguments for `forge build`.
///
/// CLI arguments take the highest precedence in the Config/Figment hierarchy.
/// In order to override them in the foundry `Config` they need to be merged into an existing
/// `figment::Provider`, like `foundry_config::Config` is.
///
/// `BuildArgs` implements `figment::Provider` in which all config related fields are serialized and
/// then merged into an existing `Config`, effectively overwriting them.
///
/// Some arguments are marked as `#[serde(skip)]` and require manual processing in
/// `figment::Provider` implementation
#[derive(Clone, Debug, Default, Serialize, Parser)]
#[command(next_help_heading = "Build options", about = None, long_about = None)] // override doc
pub struct BuildArgs {
    /// Build source files from specified paths.
    #[serde(skip)]
    pub paths: Option<Vec<PathBuf>>,

    /// Print compiled contract names.
    #[arg(long)]
    #[serde(skip)]
    pub names: bool,

    /// Print compiled contract sizes.
    /// Constructor argument length is not included in the calculation of initcode size.
    #[arg(long)]
    #[serde(skip)]
    pub sizes: bool,

    /// Ignore initcode contract bytecode size limit introduced by EIP-3860.
    #[arg(long, alias = "ignore-initcode-size")]
    #[serde(skip)]
    pub ignore_eip_3860: bool,

    #[command(flatten)]
    #[serde(flatten)]
    pub build: BuildOpts,

    #[command(flatten)]
    #[serde(skip)]
    pub watch: WatchArgs,
}

impl BuildArgs {
    pub fn run(self) -> Result<ProjectCompileOutput> {
        let mut config = self.load_config()?;

        if install_missing_dependencies(&mut config) && config.auto_detect_remappings {
            // need to re-configure here to also catch additional remappings
            config = self.load_config()?;
        }

        let project = config.project()?;

        // Collect sources to compile if build subdirectories specified.
        let mut files = vec![];
        if let Some(paths) = &self.paths {
            for path in paths {
                let joined = project.root().join(path);
                let path = if joined.exists() { &joined } else { path };
                files.extend(source_files_iter(path, MultiCompilerLanguage::FILE_EXTENSIONS));
            }
            if files.is_empty() {
                eyre::bail!("No source files found in specified build paths.")
            }
        }

        let format_json = shell::is_json();
        let compiler = ProjectCompiler::new()
            .files(files)
            .print_names(self.names)
            .print_sizes(self.sizes)
            .ignore_eip_3860(self.ignore_eip_3860)
            .bail(!format_json);

        let output = compiler.compile(&project)?;

        if format_json && !self.names && !self.sizes {
            sh_println!("{}", serde_json::to_string_pretty(&output.output())?)?;
        }

        Ok(output)
    }

    /// Returns the `Project` for the current workspace
    ///
    /// This loads the `foundry_config::Config` for the current workspace (see
    /// [`utils::find_project_root`] and merges the cli `BuildArgs` into it before returning
    /// [`foundry_config::Config::project()`]
    pub fn project(&self) -> Result<Project> {
        self.build.project()
    }

    /// Returns whether `BuildArgs` was configured with `--watch`
    pub fn is_watch(&self) -> bool {
        self.watch.watch.is_some()
    }

    /// Returns the [`watchexec::InitConfig`] and [`watchexec::RuntimeConfig`] necessary to
    /// bootstrap a new [`watchexe::Watchexec`] loop.
    pub(crate) fn watchexec_config(&self) -> Result<watchexec::Config> {
        // Use the path arguments or if none where provided the `src`, `test` and `script`
        // directories as well as the `foundry.toml` configuration file.
        self.watch.watchexec_config(|| {
            let config = self.load_config()?;
            let foundry_toml: PathBuf = config.root.join(Config::FILE_NAME);
            Ok([config.src, config.test, config.script, foundry_toml])
        })
    }
}

// Make this args a `figment::Provider` so that it can be merged into the `Config`
impl Provider for BuildArgs {
    fn metadata(&self) -> Metadata {
        Metadata::named("Build Args Provider")
    }

    fn data(&self) -> Result<Map<Profile, Dict>, figment::Error> {
        let value = Value::serialize(self)?;
        let error = InvalidType(value.to_actual(), "map".into());
        let mut dict = value.into_dict().ok_or(error)?;

        if self.names {
            dict.insert("names".to_string(), true.into());
        }

        if self.sizes {
            dict.insert("sizes".to_string(), true.into());
        }

        if self.ignore_eip_3860 {
            dict.insert("ignore_eip_3860".to_string(), true.into());
        }

        Ok(Map::from([(Config::selected_profile(), dict)]))
    }
}
