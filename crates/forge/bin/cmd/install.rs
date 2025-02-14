use clap::{Parser, ValueHint};
use eyre::Result;
use forge_verify::DependencyInstallOpts;
use foundry_cli::{opts::Dependency, utils::LoadConfig};
use foundry_config::impl_figment_convert_basic;
use std::path::PathBuf;

/// CLI arguments for `forge install`.
#[derive(Clone, Debug, Parser)]
#[command(override_usage = "forge install [OPTIONS] [DEPENDENCIES]...
    forge install [OPTIONS] <github username>/<github project>@<tag>...
    forge install [OPTIONS] <alias>=<github username>/<github project>@<tag>...
    forge install [OPTIONS] <https://<github token>@git url>...)]
    forge install [OPTIONS] <https:// git url>...")]
pub struct InstallArgs {
    /// The dependencies to install.
    ///
    /// A dependency can be a raw URL, or the path to a GitHub repository.
    ///
    /// Additionally, a ref can be provided by adding @ to the dependency path.
    ///
    /// A ref can be:
    /// - A branch: master
    /// - A tag: v1.2.3
    /// - A commit: 8e8128
    ///
    /// For exact match, a ref can be provided with `@tag=`, `@branch=` or `@rev=` prefix.
    ///
    /// Target installation directory can be added via `<alias>=` suffix.
    /// The dependency will installed to `lib/<alias>`.
    dependencies: Vec<Dependency>,

    /// The project's root path.
    ///
    /// By default root of the Git repository, if in one,
    /// or the current working directory.
    #[arg(long, value_hint = ValueHint::DirPath, value_name = "PATH")]
    pub root: Option<PathBuf>,

    #[command(flatten)]
    opts: DependencyInstallOpts,
}

impl_figment_convert_basic!(InstallArgs);

impl InstallArgs {
    pub fn run(self) -> Result<()> {
        let mut config = self.load_config()?;
        self.opts.install(&mut config, self.dependencies)
    }
}
