//! Seth
//!
//! TODO
use ethers_core::{types::*, utils::{self, keccak256}};
use ethers_providers::{Middleware, PendingTransaction};
use eyre::Result;
use rustc_hex::ToHex;
use std::str::FromStr;
use chrono::NaiveDateTime;

use dapp_utils::{encode_args, get_func, to_table};

// TODO: SethContract with common contract initializers? Same for SethProviders?

pub struct Seth<M> {
    provider: M,
}

impl<M: Middleware> Seth<M>
where
    M::Error: 'static,
{
    /// Converts ASCII text input to hex
    ///
    /// ```
    /// use seth::Seth;
    /// use ethers_providers::{Provider, Http};
    /// use std::convert::TryFrom;
    ///
    /// # async fn foo() -> eyre::Result<()> {
    /// let provider = Provider::<Http>::try_from("http://localhost:8545")?;
    /// let seth = Seth::new(provider);
    /// # Ok(())
    /// # }
    /// ```
    pub fn new(provider: M) -> Self {
        Self { provider }
    }

    /// Makes a read-only call to the specified address
    ///
    /// ```no_run
    ///
    /// use seth::Seth;
    /// use ethers_core::types::Address;
    /// use ethers_providers::{Provider, Http};
    /// use std::{str::FromStr, convert::TryFrom};
    ///
    /// # async fn foo() -> eyre::Result<()> {
    /// let provider = Provider::<Http>::try_from("http://localhost:8545")?;
    /// let seth = Seth::new(provider);
    /// let to = Address::from_str("0xB3C95ff08316fb2F2e3E52Ee82F8e7b605Aa1304")?;
    /// let sig = "function greeting(uint256 i) public returns (string)";
    /// let args = vec!["5".to_owned()];
    /// let data = seth.call(to, sig, args).await?;
    /// println!("{}", data);
    /// # Ok(())
    /// # }
    /// ```
    pub async fn call<T: Into<NameOrAddress>>(
        &self,
        to: T,
        sig: &str,
        args: Vec<String>,
    ) -> Result<String> {
        let func = get_func(sig)?;
        let data = encode_args(&func, &args)?;

        // make the call
        let tx = Eip1559TransactionRequest::new().to(to).data(data).into();
        let res = self.provider.call(&tx, None).await?;

        // decode args into tokens
        let res = func.decode_output(res.as_ref())?;

        // concatenate them
        let mut s = String::new();
        for output in res {
            s.push_str(&format!("{}\n", output));
        }

        // return string
        Ok(s)
    }

    pub async fn balance<T: Into<NameOrAddress> + Send + Sync>(
        &self,
        who: T,
        block: Option<BlockId>,
    ) -> Result<U256> {
        Ok(self.provider.get_balance(who, block).await?)
    }

    /// Sends a transaction to the specified address
    ///
    /// ```no_run
    /// use seth::Seth;
    /// use ethers_core::types::Address;
    /// use ethers_providers::{Provider, Http};
    /// use std::{str::FromStr, convert::TryFrom};
    ///
    /// # async fn foo() -> eyre::Result<()> {
    /// let provider = Provider::<Http>::try_from("http://localhost:8545")?;
    /// let seth = Seth::new(provider);
    /// let to = Address::from_str("0xB3C95ff08316fb2F2e3E52Ee82F8e7b605Aa1304")?;
    /// let sig = "function greet(string memory) public returns (string)";
    /// let args = vec!["5".to_owned()];
    /// let data = seth.call(to, sig, args).await?;
    /// println!("{}", data);
    /// # Ok(())
    /// # }
    /// ```
    pub async fn send<F: Into<NameOrAddress>, T: Into<NameOrAddress>>(
        &self,
        from: F,
        to: T,
        args: Option<(&str, Vec<String>)>,
    ) -> Result<PendingTransaction<'_, M::Provider>> {
        let from = match from.into() {
            NameOrAddress::Name(ref ens_name) => self.provider.resolve_name(ens_name).await?,
            NameOrAddress::Address(addr) => addr,
        };

        // make the call
        let mut tx = Eip1559TransactionRequest::new().from(from).to(to);

        if let Some((sig, args)) = args {
            let func = get_func(sig)?;
            let data = encode_args(&func, &args)?;
            tx = tx.data(data);
        }

        let res = self.provider.send_transaction(tx, None).await?;

        Ok::<_, eyre::Error>(res)
    }

    /// ```no_run
    /// use seth::Seth;
    /// use ethers_providers::{Provider, Http};
    /// use std::convert::TryFrom;
    ///
    /// # async fn foo() -> eyre::Result<()> {
    /// let provider = Provider::<Http>::try_from("http://localhost:8545")?;
    /// let seth = Seth::new(provider);
    /// let block = seth.block(5, true, None, false).await?;
    /// println!("{}", block);
    /// # Ok(())
    /// # }
    /// ```
    pub async fn block<T: Into<BlockId>>(
        &self,
        block: T,
        full: bool,
        field: Option<String>,
        to_json: bool,
    ) -> Result<String> {
        let block = block.into();
        let block = if full {
            let block = self
                .provider
                .get_block_with_txs(block)
                .await?
                .ok_or_else(|| eyre::eyre!("block {:?} not found", block))?;
            if let Some(ref field) = field {
                // TODO: Use custom serializer to serialize
                // u256s as decimals
                serde_json::to_value(&block)?
                    .get(field)
                    .cloned()
                    .ok_or_else(|| eyre::eyre!("field {} not found", field))?
            } else {
                serde_json::to_value(&block)?
            }
        } else {
            let block = self
                .provider
                .get_block(block)
                .await?
                .ok_or_else(|| eyre::eyre!("block {:?} not found", block))?;
            if let Some(ref field) = field {
                serde_json::to_value(block)?
                    .get(field)
                    .cloned()
                    .ok_or_else(|| eyre::eyre!("field {} not found", field))?
            } else {
                serde_json::to_value(&block)?
            }
        };

        let block = if to_json {
            serde_json::to_string(&block)?
        } else {
            to_table(block)
        };

        Ok(block)
    }

    async fn block_field_as_num<T: Into<BlockId>>(
        &self, block: T, 
        field: String
    ) -> Result<U256> {
        let block = block.into();
        let base_fee_hex = Seth::block(
            &self,
            block,
            false,
            // Select only select field
            Some(field),
            false
        ).await?;
        Ok(U256::from_str_radix(
            strip_0x(&base_fee_hex),
            16
        ).expect("Unable to convert hexadecimal to U256"))
    }

    pub async fn base_fee<T: Into<BlockId>>(&self, block: T) -> Result<U256> {
        Ok(Seth::block_field_as_num(
            &self, 
            block, 
            String::from("baseFeePerGas")
        ).await?)
    }

    pub async fn age<T: Into<BlockId>>(&self, block: T) -> Result<String> {
        let timestamp_str = Seth::block_field_as_num(
            &self,
            block,
            String::from("timestamp")
        ).await?.to_string();
        let datetime = NaiveDateTime::from_timestamp(
            timestamp_str.parse::<i64>().unwrap(), 
            0
        );
        Ok(datetime.format("%a %b %e %H:%M:%S %Y").to_string())
    }

    pub async fn chain_id(&self) -> Result<U256> {
        Ok(self.provider.get_chainid().await?)
    }

    pub async fn block_number(&self) -> Result<U64> {
        Ok(self.provider.get_block_number().await?)
    }
}

pub struct SimpleSeth;
impl SimpleSeth {
    /// Converts ASCII text input to hex
    ///
    /// ```
    /// use seth::SimpleSeth as Seth;
    ///
    /// let bin = Seth::from_ascii("yo");
    /// assert_eq!(bin, "0x796f")
    /// ```
    pub fn from_ascii(s: &str) -> String {
        let s: String = s.as_bytes().to_hex();
        format!("0x{}", s)
    }

    /// Converts integers with specified decimals into fixed point numbers
    ///
    /// ```
    /// use seth::SimpleSeth as Seth;
    ///
    /// assert_eq!(Seth::to_fix(0, 10), "10.");
    /// assert_eq!(Seth::to_fix(1, 10), "1.0");
    /// assert_eq!(Seth::to_fix(2, 10), "0.10");
    /// assert_eq!(Seth::to_fix(3, 10), "0.010");
    /// ```
    pub fn to_fix(decimals: u128, value: u128) -> String {
        let mut value: String = value.to_string();
        let decimals = decimals as usize;

        if decimals >= value.len() {
            // {0}.{0 * (number_of_decimals - value.len())}{value}
            format!("0.{:0>1$}", value, decimals)
        } else {
            // Insert decimal at -idx (i.e 1 => decimal idx = -1)
            value.insert(value.len() - decimals, '.');
            value
        }
    }

    /// Converts decimal input to hex
    ///
    /// ```
    /// use seth::SimpleSeth as Seth;
    ///
    /// assert_eq!(Seth::to_hex(424242), "0x67932");
    /// assert_eq!(Seth::to_hex(1234), "0x4d2");
    /// ```
    pub fn to_hex(u: u128) -> String {
        format!("{:#x}", u)
    }

    /// Converts an Ethereum address to its checksum format
    /// according to [EIP-55](https://github.com/ethereum/EIPs/blob/master/EIPS/eip-55.md)
    ///
    /// ```
    /// use seth::SimpleSeth as Seth;
    /// use ethers_core::types::Address;
    /// use std::str::FromStr;
    ///
    /// # fn main() -> eyre::Result<()> {
    /// let addr = Address::from_str("0xb7e390864a90b7b923c9f9310c6f98aafe43f707")?;
    /// let addr = Seth::to_checksum_address(&addr)?;
    /// assert_eq!(addr, "0xB7e390864a90b7b923C9f9310C6F98aafE43F707");
    ///
    /// # Ok(())
    /// # }
    /// ```
    pub fn to_checksum_address(address: &Address) -> Result<String> {
        Ok(utils::to_checksum(address, None))
    }

    /// Converts hexdata into bytes32 value
    /// ```
    /// use seth::SimpleSeth as Seth;
    ///
    /// # fn main() -> eyre::Result<()> {
    /// let bytes = Seth::to_bytes32("1234")?;
    /// assert_eq!(bytes, "0x1234000000000000000000000000000000000000000000000000000000000000");
    ///
    /// let bytes = Seth::to_bytes32("0x1234")?;
    /// assert_eq!(bytes, "0x1234000000000000000000000000000000000000000000000000000000000000");
    ///
    /// let err = Seth::to_bytes32("0x123400000000000000000000000000000000000000000000000000000000000011").unwrap_err();
    /// assert_eq!(err.to_string(), "string >32 bytes");
    ///
    /// # Ok(())
    /// # }
    pub fn to_bytes32(s: &str) -> Result<String> {
        let s = strip_0x(s);
        if s.len() > 64 {
            eyre::bail!("string >32 bytes");
        }

        let padded = format!("0x{:0<64}", s);
        // need to use the Debug implementation
        Ok(format!("{:?}", H256::from_str(&padded)?))
    }

    /// Converts ENS names to their namehash representation
    /// [Namehash reference](https://docs.ens.domains/contract-api-reference/name-processing#hashing-names)
    /// [namehash-rust reference](https://github.com/InstateDev/namehash-rust/blob/master/src/lib.rs)
    ///
    /// ```
    /// use seth::SimpleSeth as Seth;
    ///
    /// assert_eq!(Seth::namehash(""), "0x0000000000000000000000000000000000000000000000000000000000000000");
    /// assert_eq!(Seth::namehash("eth"), "0x93cdeb708b7545dc668eb9280176169d1c33cfd8ed6f04690a0bcc88a93fc4ae");
    /// assert_eq!(Seth::namehash("foo.eth"), "0xde9b09fd7c5f901e23a3f19fecc54828e9c848539801e86591bd9801b019f84f");
    /// assert_eq!(Seth::namehash("sub.foo.eth"), "0x500d86f9e663479e5aaa6e99276e55fc139c597211ee47d17e1e92da16a83402");
    /// ```
    pub fn namehash(ens: &str) -> String {
        let mut node = vec![0u8; 32];

        if !ens.is_empty() {
            let ens_lower = ens.to_lowercase();
            let mut labels: Vec<&str> = ens_lower.split(".").collect();
            labels.reverse();

            for label in labels {
                let mut label_hash = keccak256(label.as_bytes());
                node.append(&mut label_hash.to_vec());

                label_hash = keccak256(node.as_slice());
                node = label_hash.to_vec();
            }
        }

        let namehash: String = node.to_hex();
        format!("0x{}", namehash)
    }
}

fn strip_0x(s: &str) -> &str {
    s.strip_prefix("0x").unwrap_or(s)
}
