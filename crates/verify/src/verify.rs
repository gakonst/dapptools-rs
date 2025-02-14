//! The `forge verify-bytecode` command.

use crate::{
    etherscan::EtherscanVerificationProvider,
    install,
    provider::{VerificationProvider, VerificationProviderType},
    utils::is_host_only,
    RetryArgs,
};
use alloy_primitives::Address;
use alloy_provider::Provider;
use clap::{Parser, ValueHint};
use eyre::Result;
use foundry_cli::{
    opts::{EtherscanOpts, RpcOpts},
    utils::{self, LoadConfig},
};
use foundry_common::{compile::ProjectCompiler, ContractsByArtifact};
use foundry_compilers::{artifacts::EvmVersion, compilers::solc::Solc, info::ContractInfo};
use foundry_config::{figment, impl_figment_convert, impl_figment_convert_cast, Config, SolcReq};
use itertools::Itertools;
use reqwest::Url;
use revm_primitives::HashSet;
use semver::BuildMetadata;
use std::path::PathBuf;

use crate::provider::VerificationContext;

/// Verification provider arguments
#[derive(Clone, Debug, Parser)]
pub struct VerifierArgs {
    /// The contract verification provider to use.
    #[arg(long, help_heading = "Verifier options", default_value = "sourcify", value_enum)]
    pub verifier: VerificationProviderType,

    /// The verifier API KEY, if using a custom provider.
    #[arg(long, help_heading = "Verifier options", env = "VERIFIER_API_KEY")]
    pub verifier_api_key: Option<String>,

    /// The verifier URL, if using a custom provider.
    #[arg(long, help_heading = "Verifier options", env = "VERIFIER_URL")]
    pub verifier_url: Option<String>,
}

impl Default for VerifierArgs {
    fn default() -> Self {
        Self {
            verifier: VerificationProviderType::Sourcify,
            verifier_api_key: None,
            verifier_url: None,
        }
    }
}

/// CLI arguments for `forge verify-contract`.
#[derive(Clone, Debug, Parser)]
pub struct VerifyArgs {
    /// The address of the contract to verify.
    pub address: Address,

    /// The contract identifier in the form `<path>:<contractname>`.
    pub contract: Option<ContractInfo>,

    /// The ABI-encoded constructor arguments.
    #[arg(
        long,
        conflicts_with = "constructor_args_path",
        value_name = "ARGS",
        visible_alias = "encoded-constructor-args"
    )]
    pub constructor_args: Option<String>,

    /// The path to a file containing the constructor arguments.
    #[arg(long, value_hint = ValueHint::FilePath, value_name = "PATH")]
    pub constructor_args_path: Option<PathBuf>,

    /// Try to extract constructor arguments from on-chain creation code.
    #[arg(long)]
    pub guess_constructor_args: bool,

    /// The `solc` version to use to build the smart contract.
    #[arg(long, value_name = "VERSION")]
    pub compiler_version: Option<String>,

    /// The compilation profile to use to build the smart contract.
    #[arg(long, value_name = "PROFILE_NAME")]
    pub compilation_profile: Option<String>,

    /// The number of optimization runs used to build the smart contract.
    #[arg(long, visible_alias = "optimizer-runs", value_name = "NUM")]
    pub num_of_optimizations: Option<usize>,

    /// Flatten the source code before verifying.
    #[arg(long)]
    pub flatten: bool,

    /// Do not compile the flattened smart contract before verifying (if --flatten is passed).
    #[arg(short, long)]
    pub force: bool,

    /// Do not check if the contract is already verified before verifying.
    #[arg(long)]
    pub skip_is_verified_check: bool,

    /// Wait for verification result after submission.
    #[arg(long)]
    pub watch: bool,

    /// Set pre-linked libraries.
    #[arg(long, help_heading = "Linker options", env = "DAPP_LIBRARIES")]
    pub libraries: Vec<String>,

    /// The project's root path.
    ///
    /// By default root of the Git repository, if in one,
    /// or the current working directory.
    #[arg(long, value_hint = ValueHint::DirPath, value_name = "PATH")]
    pub root: Option<PathBuf>,

    /// Prints the standard json compiler input.
    ///
    /// The standard json compiler input can be used to manually submit contract verification in
    /// the browser.
    #[arg(long, conflicts_with = "flatten")]
    pub show_standard_json_input: bool,

    /// Use the Yul intermediate representation compilation pipeline.
    #[arg(long)]
    pub via_ir: bool,

    /// The EVM version to use.
    ///
    /// Overrides the version specified in the config.
    #[arg(long)]
    pub evm_version: Option<EvmVersion>,

    #[command(flatten)]
    pub etherscan: EtherscanOpts,

    #[command(flatten)]
    pub rpc: RpcOpts,

    #[command(flatten)]
    pub retry: RetryArgs,

    #[command(flatten)]
    pub verifier: VerifierArgs,
}

impl_figment_convert!(VerifyArgs);

impl figment::Provider for VerifyArgs {
    fn metadata(&self) -> figment::Metadata {
        figment::Metadata::named("Verify Provider")
    }

    fn data(
        &self,
    ) -> Result<figment::value::Map<figment::Profile, figment::value::Dict>, figment::Error> {
        let mut dict = self.etherscan.dict();
        dict.extend(self.rpc.dict());

        if let Some(root) = self.root.as_ref() {
            dict.insert("root".to_string(), figment::value::Value::serialize(root)?);
        }
        if let Some(optimizer_runs) = self.num_of_optimizations {
            dict.insert("optimizer".to_string(), figment::value::Value::serialize(true)?);
            dict.insert(
                "optimizer_runs".to_string(),
                figment::value::Value::serialize(optimizer_runs)?,
            );
        }
        if let Some(evm_version) = self.evm_version {
            dict.insert("evm_version".to_string(), figment::value::Value::serialize(evm_version)?);
        }
        if self.via_ir {
            dict.insert("via_ir".to_string(), figment::value::Value::serialize(self.via_ir)?);
        }

        if let Some(api_key) = &self.verifier.verifier_api_key {
            dict.insert("etherscan_api_key".into(), api_key.as_str().into());
        }

        Ok(figment::value::Map::from([(Config::selected_profile(), dict)]))
    }
}

impl VerifyArgs {
    /// Run the verify command to submit the contract's source code for verification on etherscan
    pub async fn run(mut self) -> Result<()> {
        let mut config = self.load_config()?;

        if install::install_missing_dependencies(&mut config) && config.auto_detect_remappings {
            // need to re-configure here to also catch additional remappings
            config = self.load_config()?;
        }

        if self.guess_constructor_args && config.get_rpc_url().is_none() {
            eyre::bail!(
                "You have to provide a valid RPC URL to use --guess-constructor-args feature"
            )
        }

        // If chain is not set, we try to get it from the RPC.
        // If RPC is not set, the default chain is used.
        let chain = match config.get_rpc_url() {
            Some(_) => {
                let provider = utils::get_provider(&config)?;
                utils::get_chain(config.chain, provider).await?
            }
            None => config.chain.unwrap_or_default(),
        };

        let context = self.resolve_context().await?;

        // Set Etherscan options.
        self.etherscan.chain = Some(chain);
        self.etherscan.key = config.get_etherscan_config_with_chain(Some(chain))?.map(|c| c.key);

        if self.show_standard_json_input {
            let args = EtherscanVerificationProvider::default()
                .create_verify_request(&self, &context)
                .await?;
            sh_println!("{}", args.source)?;
            return Ok(())
        }

        let verifier_url = self.verifier.verifier_url.clone();
        sh_println!("Start verifying contract `{}` deployed on {chain}", self.address)?;
        if let Some(version) = &self.compiler_version {
            sh_println!("Compiler version: {version}")?;
        }
        if let Some(optimizations) = &self.num_of_optimizations {
            sh_println!("Optimizations:    {optimizations}")?
        }
        if let Some(args) = &self.constructor_args {
            if !args.is_empty() {
                sh_println!("Constructor args: {args}")?
            }
        }
        self.verifier.verifier.client(&self.etherscan.key())?.verify(self, context).await.map_err(|err| {
            if let Some(verifier_url) = verifier_url {
                 match Url::parse(&verifier_url) {
                    Ok(url) => {
                        if is_host_only(&url) {
                            return err.wrap_err(format!(
                                "Provided URL `{verifier_url}` is host only.\n Did you mean to use the API endpoint`{verifier_url}/api` ?"
                            ))
                        }
                    }
                    Err(url_err) => {
                        return err.wrap_err(format!(
                            "Invalid URL {verifier_url} provided: {url_err}"
                        ))
                    }
                }
            }

            err
        })
    }

    /// Returns the configured verification provider
    pub fn verification_provider(&self) -> Result<Box<dyn VerificationProvider>> {
        self.verifier.verifier.client(&self.etherscan.key())
    }

    /// Resolves [VerificationContext] object either from entered contract name or by trying to
    /// match bytecode located at given address.
    pub async fn resolve_context(&self) -> Result<VerificationContext> {
        let mut config = self.load_config()?;
        config.libraries.extend(self.libraries.clone());

        let project = config.project()?;

        if let Some(ref contract) = self.contract {
            let contract_path = if let Some(ref path) = contract.path {
                project.root().join(PathBuf::from(path))
            } else {
                project.find_contract_path(&contract.name)?
            };

            let cache = project.read_cache_file().ok();

            let mut version = if let Some(ref version) = self.compiler_version {
                version.trim_start_matches('v').parse()?
            } else if let Some(ref solc) = config.solc {
                match solc {
                    SolcReq::Version(version) => version.to_owned(),
                    SolcReq::Local(solc) => Solc::new(solc)?.version,
                }
            } else if let Some(entry) =
                cache.as_ref().and_then(|cache| cache.files.get(&contract_path).cloned())
            {
                let unique_versions = entry
                    .artifacts
                    .get(&contract.name)
                    .map(|artifacts| artifacts.keys().collect::<HashSet<_>>())
                    .unwrap_or_default();

                if unique_versions.is_empty() {
                    eyre::bail!("No matching artifact found for {}", contract.name);
                } else if unique_versions.len() > 1 {
                    warn!(
                        "Ambiguous compiler versions found in cache: {}",
                        unique_versions.iter().join(", ")
                    );
                    eyre::bail!("Compiler version has to be set in `foundry.toml`. If the project was not deployed with foundry, specify the version through `--compiler-version` flag.")
                }

                unique_versions.into_iter().next().unwrap().to_owned()
            } else {
                eyre::bail!("If cache is disabled, compiler version must be either provided with `--compiler-version` option or set in foundry.toml")
            };

            let settings = if let Some(profile) = &self.compilation_profile {
                if profile == "default" {
                    &project.settings
                } else if let Some(settings) = project.additional_settings.get(profile.as_str()) {
                    settings
                } else {
                    eyre::bail!("Unknown compilation profile: {}", profile)
                }
            } else if let Some((cache, entry)) = cache
                .as_ref()
                .and_then(|cache| Some((cache, cache.files.get(&contract_path)?.clone())))
            {
                let profiles = entry
                    .artifacts
                    .get(&contract.name)
                    .and_then(|artifacts| {
                        let mut cached_artifacts = artifacts.get(&version);
                        // If we try to verify with specific build version and no cached artifacts
                        // found, then check if we have artifacts cached for same version but
                        // without any build metadata.
                        // This could happen when artifacts are built / cached
                        // with a version like `0.8.20` but verify is using a compiler-version arg
                        // as `0.8.20+commit.a1b79de6`.
                        // See <https://github.com/foundry-rs/foundry/issues/9510>.
                        if cached_artifacts.is_none() && version.build != BuildMetadata::EMPTY {
                            version.build = BuildMetadata::EMPTY;
                            cached_artifacts = artifacts.get(&version);
                        }
                        cached_artifacts
                    })
                    .map(|artifacts| artifacts.keys().collect::<HashSet<_>>())
                    .unwrap_or_default();

                if profiles.is_empty() {
                    eyre::bail!("No matching artifact found for {}", contract.name);
                } else if profiles.len() > 1 {
                    eyre::bail!("Ambiguous compilation profiles found in cache: {}, please specify the profile through `--compilation-profile` flag", profiles.iter().join(", "))
                }

                let profile = profiles.into_iter().next().unwrap().to_owned();
                let settings = cache.profiles.get(&profile).expect("must be present");

                settings
            } else if project.additional_settings.is_empty() {
                &project.settings
            } else {
                eyre::bail!("If cache is disabled, compilation profile must be provided with `--compiler-version` option or set in foundry.toml")
            };

            VerificationContext::new(
                contract_path,
                contract.name.clone(),
                version,
                config,
                settings.clone(),
            )
        } else {
            if config.get_rpc_url().is_none() {
                eyre::bail!("You have to provide a contract name or a valid RPC URL")
            }
            let provider = utils::get_provider(&config)?;
            let code = provider.get_code_at(self.address).await?;

            let output = ProjectCompiler::new().compile(&project)?;
            let contracts = ContractsByArtifact::new(
                output.artifact_ids().map(|(id, artifact)| (id, artifact.clone().into())),
            );

            let Some((artifact_id, _)) = contracts.find_by_deployed_code_exact(&code) else {
                eyre::bail!(format!(
                    "Bytecode at {} does not match any local contracts",
                    self.address
                ))
            };

            let settings = project
                .settings_profiles()
                .find_map(|(name, settings)| {
                    (name == artifact_id.profile.as_str()).then_some(settings)
                })
                .expect("must be present");

            VerificationContext::new(
                artifact_id.source.clone(),
                artifact_id.name.split('.').next().unwrap().to_owned(),
                artifact_id.version.clone(),
                config,
                settings.clone(),
            )
        }
    }
}

/// Check verification status arguments
#[derive(Clone, Debug, Parser)]
pub struct VerifyCheckArgs {
    /// The verification ID.
    ///
    /// For Etherscan - Submission GUID.
    ///
    /// For Sourcify - Contract Address.
    pub id: String,

    #[command(flatten)]
    pub retry: RetryArgs,

    #[command(flatten)]
    pub etherscan: EtherscanOpts,

    #[command(flatten)]
    pub verifier: VerifierArgs,
}

impl_figment_convert_cast!(VerifyCheckArgs);

impl VerifyCheckArgs {
    /// Run the verify command to submit the contract's source code for verification on etherscan
    pub async fn run(self) -> Result<()> {
        sh_println!(
            "Checking verification status on {}",
            self.etherscan.chain.unwrap_or_default()
        )?;
        self.verifier.verifier.client(&self.etherscan.key())?.check(self).await
    }
}

impl figment::Provider for VerifyCheckArgs {
    fn metadata(&self) -> figment::Metadata {
        figment::Metadata::named("Verify Check Provider")
    }

    fn data(
        &self,
    ) -> Result<figment::value::Map<figment::Profile, figment::value::Dict>, figment::Error> {
        self.etherscan.data()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn can_parse_verify_contract() {
        let args: VerifyArgs = VerifyArgs::parse_from([
            "foundry-cli",
            "0x0000000000000000000000000000000000000000",
            "src/Domains.sol:Domains",
            "--via-ir",
        ]);
        assert!(args.via_ir);
    }
}
