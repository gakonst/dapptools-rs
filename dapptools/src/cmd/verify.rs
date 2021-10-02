//! Verify contract source on etherscan

use crate::{etherscan, utils};

use ethers::{
    abi::{Address, Function, FunctionExt},
    prelude::Provider,
};
use eyre::ContextCompat;
use seth::{Seth, SimpleSeth};
use std::convert::TryFrom;

/// Run the verify command to verify the contract on etherscan
pub async fn run(
    path: String,
    name: String,
    address: Address,
    args: Vec<String>,
) -> eyre::Result<()> {
    let etherscan_api_key = utils::etherscan_api_key()?;
    let rpc_url = utils::rpc_url();
    let provider = Seth::new(Provider::try_from(rpc_url)?);

    let chain = provider.chain().await.map_err(|err| {
        err.wrap_err(
            r#"Please make sure that you are running a local Ethereum node:
        For example, try running either `parity' or `geth --rpc'.
        You could also try connecting to an external Ethereum node:
        For example, try `export ETH_RPC_URL=https://mainnet.infura.io'.
        If you have an Infura API key, add it to the end of the URL."#,
        )
    })?;

    let contract = utils::find_dapp_json_contract(&path, &name)?;
    let metadata = contract.metadata.wrap_err("No compiler version found")?;
    let compiler_version = format!("v{}", metadata.compiler.version);
    let mut constructor_args = None;
    if let Some(constructor) = contract.abi.constructor {
        // convert constructor into function
        #[allow(deprecated)]
        let fun = Function {
            name: "constructor".to_string(),
            inputs: constructor.inputs,
            outputs: vec![],
            constant: false,
            state_mutability: Default::default(),
        };

        constructor_args = Some(SimpleSeth::calldata(fun.abi_signature(), &args)?);
    } else if !args.is_empty() {
        eyre::bail!("No constructor found but contract arguments provided")
    }

    let etherscan = etherscan::Client::new(chain, etherscan_api_key)?;

    let source =
        format!("// Verified using https://dapptools.rs\n\n{}", std::fs::read_to_string(&path)?);

    let contract = etherscan::VerifyContract::new(address, source, compiler_version)
        .constructor_arguments(constructor_args)
        .optimization(metadata.settings.optimizer.enabled)
        .runs(metadata.settings.optimizer.runs);

    let resp = etherscan.submit_contract_verification(contract).await?;

    if resp.status == "0" {
        if resp.message == "Contract source code already verified" {
            println!("Contract source code already verified.");
            return Ok(())
        } else {
            eyre::bail!(
                "Encountered an error verifying this contract:\nResponse: `{}`\nDetails: `{}`",
                resp.message,
                resp.result
            );
        }
    }

    // wait some time until contract is verified
    tokio::time::sleep(std::time::Duration::from_secs(20)).await;

    let resp = etherscan.check_verify_status(resp.result).await?;

    if resp.status == "1" {
        println!("{}", resp.result);
        Ok(())
    } else {
        eyre::bail!(
            "Encountered an checking this contract's status:\nResponse: `{}`\nDetails: `{}`",
            resp.message,
            resp.result
        );
    }
}
