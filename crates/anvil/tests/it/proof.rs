//! tests for `eth_getProof`

use alloy_primitives::{address, fixed_bytes, Address, Bytes, B256, U256};
use anvil::{eth::EthApi, spawn, NodeConfig};
use std::{collections::BTreeMap, str::FromStr};

async fn verify_account_proof(
    api: &EthApi,
    address: Address,
    proof: impl IntoIterator<Item = &str>,
) {
    let expected_proof =
        proof.into_iter().map(Bytes::from_str).collect::<Result<Vec<_>, _>>().unwrap();
    let proof = api.get_proof(address, Vec::new(), None).await.unwrap();

    assert_eq!(proof.account_proof, expected_proof);
}

async fn verify_storage_proof(
    api: &EthApi,
    address: Address,
    slot: B256,
    proof: impl IntoIterator<Item = &str>,
) {
    let expected_proof =
        proof.into_iter().map(Bytes::from_str).collect::<Result<Vec<_>, _>>().unwrap();
    let proof = api.get_proof(address, vec![slot], None).await.unwrap();

    assert_eq!(proof.storage_proof[0].proof, expected_proof);
}

#[tokio::test(flavor = "multi_thread")]
async fn test_account_proof() {
    let (api, _handle) = spawn(NodeConfig::empty_state()).await;

    api.anvil_set_balance(
        address!("2031f89b3ea8014eb51a78c316e42af3e0d7695f"),
        U256::from(45000000000000000000_u128),
    )
    .await
    .unwrap();
    api.anvil_set_balance(address!("33f0fc440b8477fcfbe9d0bf8649e7dea9baedb2"), U256::from(1))
        .await
        .unwrap();
    api.anvil_set_balance(
        address!("62b0dd4aab2b1a0a04e279e2b828791a10755528"),
        U256::from(1100000000000000000_u128),
    )
    .await
    .unwrap();
    api.anvil_set_balance(
        address!("1ed9b1dd266b607ee278726d324b855a093394a6"),
        U256::from(120000000000000000_u128),
    )
    .await
    .unwrap();

    verify_account_proof(&api, address!("2031f89b3ea8014eb51a78c316e42af3e0d7695f"), [
        "0xe48200a7a040f916999be583c572cc4dd369ec53b0a99f7de95f13880cf203d98f935ed1b3",
        "0xf87180a04fb9bab4bb88c062f32452b7c94c8f64d07b5851d44a39f1e32ba4b1829fdbfb8080808080a0b61eeb2eb82808b73c4ad14140a2836689f4ab8445d69dd40554eaf1fce34bc080808080808080a0dea230ff2026e65de419288183a340125b04b8405cc61627b3b4137e2260a1e880",
        "0xf8719f31355ec1c8f7e26bb3ccbcb0b75d870d15846c0b98e5cc452db46c37faea40b84ff84d80890270801d946c940000a056e81f171bcc55a6ff8345e692c0f86e5b48e01b996cadc001622fb5e363b421a0c5d2460186f7233c927e7db2dcc703c0e500b653ca82273b7bfad8045d85a470"
    ]).await;

    verify_account_proof(&api, address!("33f0fc440b8477fcfbe9d0bf8649e7dea9baedb2"), [
        "0xe48200a7a040f916999be583c572cc4dd369ec53b0a99f7de95f13880cf203d98f935ed1b3",
        "0xf87180a04fb9bab4bb88c062f32452b7c94c8f64d07b5851d44a39f1e32ba4b1829fdbfb8080808080a0b61eeb2eb82808b73c4ad14140a2836689f4ab8445d69dd40554eaf1fce34bc080808080808080a0dea230ff2026e65de419288183a340125b04b8405cc61627b3b4137e2260a1e880",
        "0xe48200d3a0ef957210bca5b9b402d614eb8408c88cfbf4913eb6ab83ca233c8b8f0e626b54",
        "0xf851808080a02743a5addaf4cf9b8c0c073e1eaa555deaaf8c41cb2b41958e88624fa45c2d908080808080a0bfbf6937911dfb88113fecdaa6bde822e4e99dae62489fcf61a91cb2f36793d680808080808080",
        "0xf8679e207781e762f3577784bab7491fcc43e291ce5a356b9bc517ac52eed3a37ab846f8448001a056e81f171bcc55a6ff8345e692c0f86e5b48e01b996cadc001622fb5e363b421a0c5d2460186f7233c927e7db2dcc703c0e500b653ca82273b7bfad8045d85a470",
    ]).await;

    verify_account_proof(&api, address!("62b0dd4aab2b1a0a04e279e2b828791a10755528"), [
        "0xe48200a7a040f916999be583c572cc4dd369ec53b0a99f7de95f13880cf203d98f935ed1b3",
        "0xf87180a04fb9bab4bb88c062f32452b7c94c8f64d07b5851d44a39f1e32ba4b1829fdbfb8080808080a0b61eeb2eb82808b73c4ad14140a2836689f4ab8445d69dd40554eaf1fce34bc080808080808080a0dea230ff2026e65de419288183a340125b04b8405cc61627b3b4137e2260a1e880",
        "0xf8709f3936599f93b769acf90c7178fd2ddcac1b5b4bc9949ee5a04b7e0823c2446eb84ef84c80880f43fc2c04ee0000a056e81f171bcc55a6ff8345e692c0f86e5b48e01b996cadc001622fb5e363b421a0c5d2460186f7233c927e7db2dcc703c0e500b653ca82273b7bfad8045d85a470",
    ]).await;

    verify_account_proof(&api, address!("1ed9b1dd266b607ee278726d324b855a093394a6"), [
        "0xe48200a7a040f916999be583c572cc4dd369ec53b0a99f7de95f13880cf203d98f935ed1b3",
        "0xf87180a04fb9bab4bb88c062f32452b7c94c8f64d07b5851d44a39f1e32ba4b1829fdbfb8080808080a0b61eeb2eb82808b73c4ad14140a2836689f4ab8445d69dd40554eaf1fce34bc080808080808080a0dea230ff2026e65de419288183a340125b04b8405cc61627b3b4137e2260a1e880",
        "0xe48200d3a0ef957210bca5b9b402d614eb8408c88cfbf4913eb6ab83ca233c8b8f0e626b54",
        "0xf851808080a02743a5addaf4cf9b8c0c073e1eaa555deaaf8c41cb2b41958e88624fa45c2d908080808080a0bfbf6937911dfb88113fecdaa6bde822e4e99dae62489fcf61a91cb2f36793d680808080808080",
        "0xf86f9e207a32b8ab5eb4b043c65b1f00c93f517bc8883c5cd31baf8e8a279475e3b84ef84c808801aa535d3d0c0000a056e81f171bcc55a6ff8345e692c0f86e5b48e01b996cadc001622fb5e363b421a0c5d2460186f7233c927e7db2dcc703c0e500b653ca82273b7bfad8045d85a470"
    ]).await;
}

#[tokio::test(flavor = "multi_thread")]
async fn test_storage_proof() {
    let target = address!("1ed9b1dd266b607ee278726d324b855a093394a6");

    let (api, _handle) = spawn(NodeConfig::empty_state()).await;
    let storage: BTreeMap<U256, B256> =
        serde_json::from_str(include_str!("../../test-data/storage_sample.json")).unwrap();

    for (key, value) in storage {
        api.anvil_set_storage_at(target, key, value).await.unwrap();
    }

    verify_storage_proof(&api, target, fixed_bytes!("0000000000000000000000000000000000000000000000000000000000000022"), [
        "0xf9019180a0aafd5b14a6edacd149e110ba6776a654f2dbffca340902be933d011113f2750380a0a502c93b1918c4c6534d4593ae03a5a23fa10ebc30ffb7080b297bff2446e42da02eb2bf45fd443bd1df8b6f9c09726a4c6252a0f7896a131a081e39a7f644b38980a0a9cf7f673a0bce76fd40332afe8601542910b48dea44e93933a3e5e930da5d19a0ddf79db0a36d0c8134ba143bcb541cd4795a9a2bae8aca0ba24b8d8963c2a77da0b973ec0f48f710bf79f63688485755cbe87f9d4c68326bb83c26af620802a80ea0f0855349af6bf84afc8bca2eda31c8ef8c5139be1929eeb3da4ba6b68a818cb0a0c271e189aeeb1db5d59d7fe87d7d6327bbe7cfa389619016459196497de3ccdea0e7503ba5799e77aa31bbe1310c312ca17b2c5bcc8fa38f266675e8f154c2516ba09278b846696d37213ab9d20a5eb42b03db3173ce490a2ef3b2f3b3600579fc63a0e9041059114f9c910adeca12dbba1fef79b2e2c8899f2d7213cd22dfe4310561a047c59da56bb2bf348c9dd2a2e8f5538a92b904b661cfe54a4298b85868bbe4858080",
        "0xf85180a0776aa456ba9c5008e03b82b841a9cf2fc1e8578cfacd5c9015804eae315f17fb80808080808080808080808080a072e3e284d47badbb0a5ca1421e1179d3ea90cc10785b26b74fb8a81f0f9e841880",
        "0xf843a020035b26e3e9eee00e0d72fd1ee8ddca6894550dca6916ea2ac6baa90d11e510a1a0f5a5fd42d16a20302798ef6ed309979b43003d2320d9f0e8ea9831a92759fb4b"
    ]).await;

    verify_storage_proof(&api, target, fixed_bytes!("0000000000000000000000000000000000000000000000000000000000000023"), [
        "0xf9019180a0aafd5b14a6edacd149e110ba6776a654f2dbffca340902be933d011113f2750380a0a502c93b1918c4c6534d4593ae03a5a23fa10ebc30ffb7080b297bff2446e42da02eb2bf45fd443bd1df8b6f9c09726a4c6252a0f7896a131a081e39a7f644b38980a0a9cf7f673a0bce76fd40332afe8601542910b48dea44e93933a3e5e930da5d19a0ddf79db0a36d0c8134ba143bcb541cd4795a9a2bae8aca0ba24b8d8963c2a77da0b973ec0f48f710bf79f63688485755cbe87f9d4c68326bb83c26af620802a80ea0f0855349af6bf84afc8bca2eda31c8ef8c5139be1929eeb3da4ba6b68a818cb0a0c271e189aeeb1db5d59d7fe87d7d6327bbe7cfa389619016459196497de3ccdea0e7503ba5799e77aa31bbe1310c312ca17b2c5bcc8fa38f266675e8f154c2516ba09278b846696d37213ab9d20a5eb42b03db3173ce490a2ef3b2f3b3600579fc63a0e9041059114f9c910adeca12dbba1fef79b2e2c8899f2d7213cd22dfe4310561a047c59da56bb2bf348c9dd2a2e8f5538a92b904b661cfe54a4298b85868bbe4858080",
        "0xf8518080808080a0d546c4ca227a267d29796643032422374624ed109b3d94848c5dc06baceaee76808080808080a027c48e210ccc6e01686be2d4a199d35f0e1e8df624a8d3a17c163be8861acd6680808080",
        "0xf843a0207b2b5166478fd4318d2acc6cc2c704584312bdd8781b32d5d06abda57f4230a1a0db56114e00fdd4c1f85c892bf35ac9a89289aaecb1ebd0a96cde606a748b5d71"
    ]).await;

    verify_storage_proof(&api, target, fixed_bytes!("0000000000000000000000000000000000000000000000000000000000000024"), [
        "0xf9019180a0aafd5b14a6edacd149e110ba6776a654f2dbffca340902be933d011113f2750380a0a502c93b1918c4c6534d4593ae03a5a23fa10ebc30ffb7080b297bff2446e42da02eb2bf45fd443bd1df8b6f9c09726a4c6252a0f7896a131a081e39a7f644b38980a0a9cf7f673a0bce76fd40332afe8601542910b48dea44e93933a3e5e930da5d19a0ddf79db0a36d0c8134ba143bcb541cd4795a9a2bae8aca0ba24b8d8963c2a77da0b973ec0f48f710bf79f63688485755cbe87f9d4c68326bb83c26af620802a80ea0f0855349af6bf84afc8bca2eda31c8ef8c5139be1929eeb3da4ba6b68a818cb0a0c271e189aeeb1db5d59d7fe87d7d6327bbe7cfa389619016459196497de3ccdea0e7503ba5799e77aa31bbe1310c312ca17b2c5bcc8fa38f266675e8f154c2516ba09278b846696d37213ab9d20a5eb42b03db3173ce490a2ef3b2f3b3600579fc63a0e9041059114f9c910adeca12dbba1fef79b2e2c8899f2d7213cd22dfe4310561a047c59da56bb2bf348c9dd2a2e8f5538a92b904b661cfe54a4298b85868bbe4858080",
        "0xf85180808080a030263404acfee103d0b1019053ff3240fce433c69b709831673285fa5887ce4c80808080808080a0f8f1fbb1f7b482d9860480feebb83ff54a8b6ec1ead61cc7d2f25d7c01659f9c80808080",
        "0xf843a020d332d19b93bcabe3cce7ca0c18a052f57e5fd03b4758a09f30f5ddc4b22ec4a1a0c78009fdf07fc56a11f122370658a353aaa542ed63e44c4bc15ff4cd105ab33c",
    ]).await;

    verify_storage_proof(&api, target, fixed_bytes!("0000000000000000000000000000000000000000000000000000000000000100"), [
        "0xf9019180a0aafd5b14a6edacd149e110ba6776a654f2dbffca340902be933d011113f2750380a0a502c93b1918c4c6534d4593ae03a5a23fa10ebc30ffb7080b297bff2446e42da02eb2bf45fd443bd1df8b6f9c09726a4c6252a0f7896a131a081e39a7f644b38980a0a9cf7f673a0bce76fd40332afe8601542910b48dea44e93933a3e5e930da5d19a0ddf79db0a36d0c8134ba143bcb541cd4795a9a2bae8aca0ba24b8d8963c2a77da0b973ec0f48f710bf79f63688485755cbe87f9d4c68326bb83c26af620802a80ea0f0855349af6bf84afc8bca2eda31c8ef8c5139be1929eeb3da4ba6b68a818cb0a0c271e189aeeb1db5d59d7fe87d7d6327bbe7cfa389619016459196497de3ccdea0e7503ba5799e77aa31bbe1310c312ca17b2c5bcc8fa38f266675e8f154c2516ba09278b846696d37213ab9d20a5eb42b03db3173ce490a2ef3b2f3b3600579fc63a0e9041059114f9c910adeca12dbba1fef79b2e2c8899f2d7213cd22dfe4310561a047c59da56bb2bf348c9dd2a2e8f5538a92b904b661cfe54a4298b85868bbe4858080",
        "0xf891a090bacef44b189ddffdc5f22edc70fe298c58e5e523e6e1dfdf7dbc6d657f7d1b80a026eed68746028bc369eb456b7d3ee475aa16f34e5eaa0c98fdedb9c59ebc53b0808080a09ce86197173e14e0633db84ce8eea32c5454eebe954779255644b45b717e8841808080a0328c7afb2c58ef3f8c4117a8ebd336f1a61d24591067ed9c5aae94796cac987d808080808080",
    ]).await;
}

#[tokio::test(flavor = "multi_thread")]
async fn can_get_random_account_proofs() {
    let (api, mut handle) = spawn(NodeConfig::test()).await;

    for acc in std::iter::repeat_with(Address::random).take(10) {
        let _ = api
            .get_proof(acc, Vec::new(), None)
            .await
            .unwrap_or_else(|_| panic!("Failed to get proof for {acc:?}"));
    }

    if let Some(signal) = handle.shutdown_signal_mut().take() {
        signal.fire().unwrap();
    }
}
