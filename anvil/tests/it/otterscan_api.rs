//! tests for otterscan endpoints
use crate::abi::MulticallContract;
use anvil::{spawn, NodeConfig};
use ethers::{
    prelude::{Middleware, SignerMiddleware},
    signers::Signer,
    types::BlockNumber,
    utils::get_contract_address,
};
use std::sync::Arc;

#[tokio::test(flavor = "multi_thread")]
async fn can_call_otterscan_has_code() {
    let (api, handle) = spawn(NodeConfig::test()).await;
    let provider = handle.http_provider();

    let wallet = handle.dev_wallets().next().unwrap();
    let sender = wallet.address();
    let client = Arc::new(SignerMiddleware::new(provider, wallet));
    api.mine_one().await;

    let mut deploy_tx = MulticallContract::deploy(Arc::clone(&client), ()).unwrap().deployer.tx;
    deploy_tx.set_nonce(0);

    let pending_contract_address = get_contract_address(sender, deploy_tx.nonce().unwrap());

    // no code in the address before deploying
    assert!(!api
        .ots_has_code(pending_contract_address, BlockNumber::Number(1.into()))
        .await
        .unwrap());

    client.send_transaction(deploy_tx, None).await.unwrap();

    let num = client.get_block_number().await.unwrap();
    // code is detected after deploying
    assert!(api.ots_has_code(pending_contract_address, BlockNumber::Number(num)).await.unwrap());

    // code is not detected for the previous block
    assert!(!api
        .ots_has_code(pending_contract_address, BlockNumber::Number(num - 1))
        .await
        .unwrap());
}
