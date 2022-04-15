//! In memory blockchain backend

use crate::eth::{
    backend::{db::Db, executor::TransactionExecutor},
    pool::transactions::PoolTransaction,
};
use ethers::prelude::{BlockNumber, TxHash, H256, U256, U64};

use crate::{
    eth::{
        backend::{cheats::CheatsManager, time::TimeManager},
        error::{BlockchainError, InvalidTransactionError},
        fees::FeeDetails,
    },
    fork::ClientFork,
};
use anvil_core::{
    eth::{
        block::{Block, BlockInfo, Header},
        call::CallRequest,
        filter::{Filter, FilteredParams},
        receipt::{EIP658Receipt, TypedReceipt},
        transaction::{PendingTransaction, TransactionInfo, TypedTransaction},
        utils::to_access_list,
    },
    types::Index,
};
use ethers::{
    types::{
        Address, Block as EthersBlock, Bytes, Filter as EthersFilter, Log, Transaction,
        TransactionReceipt,
    },
    utils::{keccak256, rlp},
};
use foundry_evm::{
    revm,
    revm::{db::CacheDB, CreateScheme, Env, Return, TransactOut, TransactTo, TxEnv},
    utils::u256_to_h256_le,
};
use parking_lot::RwLock;
use std::sync::Arc;
use storage::Blockchain;
use tracing::trace;

pub mod storage;

#[derive(Debug, Clone)]
pub struct MinedTransaction {
    pub info: TransactionInfo,
    pub receipt: TypedReceipt,
    pub block_hash: H256,
}

/// Gives access to the [revm::Database]
#[derive(Clone)]
pub struct Backend {
    /// access to revm's database related operations
    /// This stores the actual state of the blockchain
    /// Supports concurrent reads
    db: Arc<RwLock<dyn Db>>,
    /// stores all block related data in memory
    blockchain: Blockchain,
    /// env data of the chain
    env: Arc<RwLock<Env>>,
    /// Default gas price for all transactions
    gas_price: Arc<RwLock<U256>>,
    /// this is set if this is currently forked off another client
    fork: Option<ClientFork>,
    /// provides time related info, like timestamp
    time: TimeManager,
    /// Contains state of custom overrides
    cheats: CheatsManager,
}

impl Backend {
    /// Create a new instance of in-mem backend.
    pub fn new(db: Arc<RwLock<dyn Db>>, env: Arc<RwLock<Env>>, gas_price: U256) -> Self {
        Self {
            db,
            blockchain: Blockchain::default(),
            env,
            gas_price: Arc::new(RwLock::new(gas_price)),
            fork: None,
            time: Default::default(),
            cheats: Default::default(),
        }
    }

    /// Creates a new empty blockchain backend
    pub fn empty(env: Arc<RwLock<Env>>, gas_price: U256) -> Self {
        let db = CacheDB::default();
        Self::new(Arc::new(RwLock::new(db)), env, gas_price)
    }

    /// Initialises the balance of the given accounts
    pub fn with_genesis_balance(
        db: Arc<RwLock<dyn Db>>,
        env: Arc<RwLock<Env>>,
        balance: U256,
        accounts: impl IntoIterator<Item = Address>,
        gas_price: U256,
        fork: Option<ClientFork>,
    ) -> Self {
        // insert genesis accounts
        {
            let mut db = db.write();
            for account in accounts {
                let mut info = db.basic(account);
                info.balance = balance;
                db.insert_account(account, info);
            }
        }

        // if this is a fork then adjust the blockchain storage
        let blockchain = if let Some(ref fork) = fork {
            trace!(target: "backend", "using forked blockchain at {}", fork.block_number());
            Blockchain::forked(fork.block_number(), fork.block_hash())
        } else {
            Default::default()
        };

        Self {
            db,
            blockchain,
            env,
            gas_price: Arc::new(RwLock::new(gas_price)),
            fork,
            time: Default::default(),
            cheats: Default::default(),
        }
    }

    /// Returns the configured fork, if any
    pub fn get_fork(&self) -> Option<&ClientFork> {
        self.fork.as_ref()
    }

    /// Whether we're forked off some remote client
    pub fn is_fork(&self) -> bool {
        self.fork.is_some()
    }

    /// Returns the `TimeManager` responsible for timestamps
    pub fn time(&self) -> &TimeManager {
        &self.time
    }

    /// Returns the `CheatsManager` responsible for executing cheatcodes
    pub fn cheats(&self) -> &CheatsManager {
        &self.cheats
    }

    /// The env data of the blockchain
    pub fn env(&self) -> &Arc<RwLock<Env>> {
        &self.env
    }

    /// Returns the current best hash of the chain
    pub fn best_hash(&self) -> H256 {
        self.blockchain.storage.read().best_hash
    }

    /// Returns the current best number of the chain
    pub fn best_number(&self) -> U64 {
        let num: u64 = self.env.read().block.number.try_into().unwrap_or(u64::MAX);
        num.into()
    }

    /// Sets the block number
    pub fn set_block_number(&self, number: U256) {
        let mut env = self.env.write();
        env.block.number = number;
    }

    /// Returns the client coinbase address.
    pub fn coinbase(&self) -> Address {
        self.env.read().block.coinbase
    }

    /// Returns the client coinbase address.
    pub fn chain_id(&self) -> U256 {
        self.env.read().cfg.chain_id
    }

    /// Returns balance of the given account.
    pub fn current_balance(&self, address: Address) -> U256 {
        self.db.read().basic(address).balance
    }

    /// Returns balance of the given account.
    pub fn current_nonce(&self, address: Address) -> U256 {
        self.db.read().basic(address).nonce.into()
    }

    /// Sets the coinbase address
    pub fn set_coinbase(&self, address: Address) {
        self.env.write().block.coinbase = address;
    }

    /// Sets the nonce of the given address
    pub fn set_nonce(&self, address: Address, nonce: U256) {
        self.db.write().set_nonce(address, nonce.try_into().unwrap_or(u64::MAX));
    }

    /// Sets the balance of the given address
    pub fn set_balance(&self, address: Address, balance: U256) {
        self.db.write().set_balance(address, balance);
    }

    /// Sets the code of the given address
    pub fn set_code(&self, address: Address, code: Bytes) {
        self.db.write().set_code(address, code);
    }

    /// Sets the value for the given slot of the given address
    pub fn set_storage_at(&self, address: Address, slot: U256, val: U256) {
        self.db.write().set_storage_at(address, slot, val);
    }

    pub fn gas_limit(&self) -> U256 {
        self.env().read().block.gas_limit
    }

    /// Returns the current basefee
    pub fn base_fee(&self) -> U256 {
        self.env().read().block.basefee
    }

    /// Sets the current basefee
    pub fn set_base_fee(&self, basefee: U256) {
        // TODO this should probably managed separately
        let mut env = self.env().write();
        env.block.basefee = basefee;
    }

    /// Returns the current gas price
    pub fn gas_price(&self) -> U256 {
        *self.gas_price.read()
    }

    /// Returns the current gas price
    pub fn set_gas_price(&self, price: U256) {
        let mut gas = self.gas_price.write();
        *gas = price;
    }

    /// Return the base fee at the given height
    pub fn elasticity(&self) -> f64 {
        // default elasticity
        0.125
    }

    /// Validates the transaction's validity when it comes to nonce, payment
    ///
    /// This is intended to be checked before the transaction makes it into the pool and whether it
    /// should rather be outright rejected if the sender has insufficient funds.
    pub fn validate_transaction(
        &self,
        tx: &PendingTransaction,
    ) -> Result<(), InvalidTransactionError> {
        let sender = *tx.sender();
        let tx = &tx.transaction;
        let account = self.db.read().basic(sender);

        // check nonce
        if tx.nonce().as_u64() < account.nonce {
            return Err(InvalidTransactionError::Payment)
        }

        let max_cost = tx.max_cost();
        let value = tx.value();
        // check sufficient funds: `gas * price + value`
        let req_funds = max_cost.checked_add(value).ok_or(InvalidTransactionError::Payment)?;

        if account.balance < req_funds {
            return Err(InvalidTransactionError::Payment)
        }
        Ok(())
    }

    /// Mines a new block and stores it.
    ///
    /// this will execute all transaction in the order they come in and return all the markers they
    /// provide.
    ///
    /// TODO(mattsse): currently we're assuming all transactions are valid:
    ///  needs an additional validation step: gas limit, fee
    pub fn mine_block(&self, pool_transactions: Vec<Arc<PoolTransaction>>) -> U64 {
        // acquire all locks
        let mut env = self.env.write();
        let mut db = self.db.write();
        let mut storage = self.blockchain.storage.write();

        // increase block number for this block
        env.block.number = env.block.number.saturating_add(U256::one());

        let executor = TransactionExecutor {
            db: &mut *db,
            pending: pool_transactions.into_iter(),
            block_env: env.block.clone(),
            cfg_env: env.cfg.clone(),
            parent_hash: storage.best_hash,
        };

        // create the new block with the current timestamp
        let BlockInfo { block, transactions, receipts } =
            executor.create_block(self.time.current_timestamp());

        let block_hash = block.header.hash();
        let block_number: U64 = env.block.number.as_u64().into();

        trace!(target: "backend", "Created block {} with {} tx: [{:?}]", block_number, transactions.len(), block_hash);

        // update block metadata
        storage.best_number = block_number;
        storage.best_hash = block_hash;

        storage.blocks.insert(block_hash, block);
        storage.hashes.insert(block_number, block_hash);

        // insert all transactions
        for (info, receipt) in transactions.into_iter().zip(receipts) {
            let mined_tx = MinedTransaction { info, receipt, block_hash };
            storage.transactions.insert(mined_tx.info.transaction_hash, mined_tx);
        }

        block_number
    }

    /// Executes the `CallRequest` without writing to the DB
    pub fn call(
        &self,
        request: CallRequest,
        fee_details: FeeDetails,
    ) -> (Return, TransactOut, u64) {
        let CallRequest { from, to, gas, value, data, nonce, access_list, .. } = request;

        let FeeDetails { gas_price, max_fee_per_gas, max_priority_fee_per_gas } = fee_details;

        let gas_limit = gas.unwrap_or_else(|| self.gas_limit());
        let mut env = self.env.read().clone();

        env.tx = TxEnv {
            caller: from.unwrap_or_default(),
            gas_limit: gas_limit.as_u64(),
            gas_price: gas_price.or(max_fee_per_gas).unwrap_or_else(|| self.gas_price()),
            gas_priority_fee: max_priority_fee_per_gas,
            transact_to: match to {
                Some(addr) => TransactTo::Call(addr),
                None => TransactTo::Create(CreateScheme::Create),
            },
            value: value.unwrap_or_default(),
            data: data.unwrap_or_else(|| vec![].into()).to_vec().into(),
            chain_id: None,
            nonce: nonce.map(|n| n.as_u64()),
            access_list: to_access_list(access_list.unwrap_or_default().0),
        };

        trace!(target: "backend", "calling with {:?}", env.tx);

        let db = self.db.read();
        let mut evm = revm::EVM::new();
        evm.env = env;
        evm.database(&*db);

        let (exit, out, gas, _, _) = evm.transact_ref();
        trace!(target: "backend", "call return {:?} out: {:?} gas {}", exit, out, gas);

        (exit, out, gas)
    }

    /// returns all receipts for the given transactions
    fn get_receipts(&self, tx_hashes: impl IntoIterator<Item = TxHash>) -> Vec<TypedReceipt> {
        let storage = self.blockchain.storage.read();
        let mut receipts = vec![];

        for hash in tx_hashes {
            if let Some(tx) = storage.transactions.get(&hash) {
                receipts.push(tx.receipt.clone());
            }
        }

        receipts
    }

    /// Returns the logs of the block that match the filter
    async fn logs_for_block(
        &self,
        filter: Filter,
        hash: H256,
    ) -> Result<Vec<Log>, BlockchainError> {
        if let Some(block) = self.blockchain.storage.read().blocks.get(&hash).cloned() {
            return Ok(self.mined_logs_for_block(filter, block))
        }

        if let Some(ref fork) = self.fork {
            let filter = filter.into();
            return Ok(fork.logs(&filter).await?)
        }

        Ok(Vec::new())
    }

    /// Returns all `Log`s mined by the node that were emitted in the `block` and match the `Filter`
    fn mined_logs_for_block(&self, filter: Filter, block: Block) -> Vec<Log> {
        let params = FilteredParams::new(Some(filter.clone()));
        let mut all_logs = Vec::new();
        let block_hash = block.header.hash();
        let mut block_log_index = 0u32;

        let transactions: Vec<_> = {
            let storage = self.blockchain.storage.read();
            block
                .transactions
                .iter()
                .filter_map(|tx| storage.transactions.get(&tx.hash()).map(|tx| tx.info.clone()))
                .collect()
        };

        for transaction in transactions {
            let logs = transaction.logs.clone();
            let transaction_hash = transaction.transaction_hash;

            for (log_idx, log) in logs.into_iter().enumerate() {
                let mut log = Log {
                    address: log.address,
                    topics: log.topics,
                    data: log.data,
                    block_hash: None,
                    block_number: None,
                    transaction_hash: None,
                    transaction_index: None,
                    log_index: None,
                    transaction_log_index: None,
                    log_type: None,
                    removed: Some(false),
                };
                let mut is_match: bool = true;
                if filter.address.is_some() && filter.topics.is_some() {
                    if !params.filter_address(&log) || !params.filter_topics(&log) {
                        is_match = false;
                    }
                } else if filter.address.is_some() {
                    if !params.filter_address(&log) {
                        is_match = false;
                    }
                } else if filter.topics.is_some() && !params.filter_topics(&log) {
                    is_match = false;
                }

                if is_match {
                    log.block_hash = Some(block_hash);
                    log.block_number = Some(block.header.number.as_u64().into());
                    log.transaction_hash = Some(transaction_hash);
                    log.transaction_index = Some(transaction.transaction_index.into());
                    log.log_index = Some(U256::from(block_log_index));
                    log.transaction_log_index = Some(U256::from(log_idx));
                    all_logs.push(log);
                }
                block_log_index += 1;
            }
        }

        all_logs
    }

    /// Returns the logs that match the filter in the given range of blocks
    async fn logs_for_range(
        &self,
        filter: &Filter,
        mut from: u64,
        to: u64,
    ) -> Result<Vec<Log>, BlockchainError> {
        let mut all_logs = Vec::new();

        // get the range that predates the fork if any
        if let Some(ref fork) = self.fork {
            let mut to_on_fork = to;

            if !fork.predates_fork(to) {
                // adjust the ranges
                to_on_fork = fork.block_number();
            }

            if fork.predates_fork(from) {
                // this data is only available on the forked client
                let mut filter: EthersFilter = filter.clone().into();
                filter = filter.from_block(from).to_block(to_on_fork);
                all_logs = fork.logs(&filter).await?;

                // update the range
                from = fork.block_number() + 1;
            }
        }

        for number in from..=to {
            if let Some(block) = self.get_block(number) {
                all_logs.extend(self.mined_logs_for_block(filter.clone(), block));
            }
        }

        Ok(all_logs)
    }

    /// Returns the logs according to the filter
    pub async fn logs(&self, filter: Filter) -> Result<Vec<Log>, BlockchainError> {
        if let Some(hash) = filter.block_hash {
            self.logs_for_block(filter, hash).await
        } else {
            let best = self.best_number().as_u64();
            let to_block = filter.get_to_block_number().unwrap_or(best).min(best);

            let from_block = filter.get_from_block_number().unwrap_or(best).min(best);
            self.logs_for_range(&filter, from_block, to_block).await
        }
    }

    pub async fn block_by_hash(
        &self,
        hash: H256,
    ) -> Result<Option<EthersBlock<TxHash>>, BlockchainError> {
        if let tx @ Some(_) = self.mined_block_by_hash(hash) {
            return Ok(tx)
        }

        if let Some(ref fork) = self.fork {
            return Ok(fork.block_by_hash(hash).await?)
        }

        Ok(None)
    }

    pub fn mined_block_by_hash(&self, hash: H256) -> Option<EthersBlock<TxHash>> {
        let block = self.blockchain.storage.read().blocks.get(&hash)?.clone();
        self.convert_block(block)
    }

    pub async fn block_by_number(
        &self,
        number: BlockNumber,
    ) -> Result<Option<EthersBlock<TxHash>>, BlockchainError> {
        if let tx @ Some(_) = self.mined_block_by_number(number) {
            return Ok(tx)
        }

        if let Some(ref fork) = self.fork {
            return Ok(fork.block_by_number(self.convert_block_number(Some(number))).await?)
        }

        Ok(None)
    }

    fn get_block(&self, number: impl Into<BlockNumber>) -> Option<Block> {
        let storage = self.blockchain.storage.read();
        let hash = match number.into() {
            BlockNumber::Latest => storage.best_hash,
            BlockNumber::Earliest => storage.genesis_hash,
            BlockNumber::Pending => return None,
            BlockNumber::Number(num) => *storage.hashes.get(&num)?,
        };
        Some(storage.blocks.get(&hash)?.clone())
    }

    pub fn mined_block_by_number(&self, number: BlockNumber) -> Option<EthersBlock<TxHash>> {
        self.convert_block(self.get_block(number)?)
    }

    /// Takes a block as it's stored internally and returns the eth api conform block format
    fn convert_block(&self, block: Block) -> Option<EthersBlock<TxHash>> {
        let size = U256::from(rlp::encode(&block).len() as u32);

        let Block { header, transactions, .. } = block;

        let hash = header.hash();
        let Header {
            parent_hash,
            ommers_hash,
            beneficiary,
            state_root,
            transactions_root,
            receipts_root,
            logs_bloom,
            difficulty,
            number,
            gas_limit,
            gas_used,
            timestamp,
            extra_data,
            mix_hash,
            nonce,
        } = header;

        let block = EthersBlock {
            hash: Some(hash),
            parent_hash,
            uncles_hash: ommers_hash,
            author: beneficiary,
            state_root,
            transactions_root,
            receipts_root,
            number: Some(number.as_u64().into()),
            gas_used,
            gas_limit,
            extra_data,
            logs_bloom: Some(logs_bloom),
            timestamp: timestamp.into(),
            difficulty,
            total_difficulty: None,
            seal_fields: {
                let mut arr = [0u8; 8];
                nonce.to_big_endian(&mut arr);
                vec![mix_hash.as_bytes().to_vec().into(), arr.to_vec().into()]
            },
            uncles: vec![],
            transactions: transactions.into_iter().map(|tx| tx.hash()).collect(),
            size: Some(size),
            mix_hash: Some(mix_hash),
            nonce: Some(nonce),
            // TODO check
            base_fee_per_gas: Some(self.base_fee()),
        };

        Some(block)
    }

    pub fn convert_block_number(&self, block: Option<BlockNumber>) -> u64 {
        match block.unwrap_or(BlockNumber::Latest) {
            BlockNumber::Latest | BlockNumber::Pending => self.best_number().as_u64(),
            BlockNumber::Earliest => 0,
            BlockNumber::Number(num) => num.as_u64(),
        }
    }

    pub async fn storage_at(
        &self,
        address: Address,
        index: U256,
        number: Option<BlockNumber>,
    ) -> Result<H256, BlockchainError> {
        let number = self.convert_block_number(number);

        if let Some(ref fork) = self.fork {
            if fork.predates_fork(number) {
                return Ok(fork.storage_at(address, index, Some(number.into())).await?)
            }
        }

        let val = self.db.read().storage(address, index);
        Ok(u256_to_h256_le(val))
    }

    /// Returns the code of the address
    ///
    /// If the code is not present and fork mode is enabled then this will try to fetch it from the
    /// forked client
    pub async fn get_code(
        &self,
        address: Address,
        block: Option<BlockNumber>,
    ) -> Result<Bytes, BlockchainError> {
        let number = self.convert_block_number(block);

        let code = self.db.read().basic(address).code.clone();

        if let Some(ref fork) = self.fork {
            if fork.predates_fork(number) || code.is_none() {
                return Ok(fork.get_code(address, number).await?)
            }
        }

        Ok(code.unwrap_or_default().into())
    }

    /// Returns the balance of the address
    ///
    /// If the requested number predates the fork then this will fetch it from the endpoint
    pub async fn get_balance(
        &self,
        address: Address,
        block: Option<BlockNumber>,
    ) -> Result<U256, BlockchainError> {
        let number = self.convert_block_number(block);

        if let Some(ref fork) = self.fork {
            if fork.predates_fork(number) {
                return Ok(fork.get_balance(address, number).await?)
            }
        }

        Ok(self.current_balance(address))
    }

    /// Returns the nonce of the address
    ///
    /// If the requested number predates the fork then this will fetch it from the endpoint
    pub async fn get_nonce(
        &self,
        address: Address,
        block: Option<BlockNumber>,
    ) -> Result<U256, BlockchainError> {
        let number = self.convert_block_number(block);

        if let Some(ref fork) = self.fork {
            if fork.predates_fork(number) {
                return Ok(fork.get_nonce(address, number).await?)
            }
        }

        Ok(self.current_nonce(address))
    }

    pub async fn transaction_receipt(
        &self,
        hash: H256,
    ) -> Result<Option<TransactionReceipt>, BlockchainError> {
        if let tx @ Some(_) = self.mined_transaction_receipt(hash) {
            return Ok(tx)
        }

        if let Some(ref fork) = self.fork {
            return Ok(fork.transaction_receipt(hash).await?)
        }

        Ok(None)
    }

    /// Returns the transaction receipt for the given hash
    pub fn mined_transaction_receipt(&self, hash: H256) -> Option<TransactionReceipt> {
        let MinedTransaction { info, receipt, block_hash, .. } =
            self.blockchain.storage.read().transactions.get(&hash)?.clone();

        let EIP658Receipt { status_code, gas_used, logs_bloom, logs } = receipt.into();

        let index = info.transaction_index as usize;

        let block = self.blockchain.storage.read().blocks.get(&block_hash).cloned()?;

        // TODO store cumulative gas used in receipt instead
        let receipts = self.get_receipts(block.transactions.iter().map(|tx| tx.hash()));

        let mut cumulative_gas_used = U256::zero();
        for receipt in receipts.iter().take(index) {
            cumulative_gas_used = cumulative_gas_used.saturating_add(receipt.gas_used());
        }

        // cumulative_gas_used = cumulative_gas_used.saturating_sub(gas_used);

        let mut cumulative_receipts = receipts;
        cumulative_receipts.truncate(index + 1);

        let transaction = block.transactions[index].clone();

        let effective_gas_price = match transaction {
            TypedTransaction::Legacy(t) => t.gas_price,
            TypedTransaction::EIP2930(t) => t.gas_price,
            TypedTransaction::EIP1559(t) => self
                .base_fee()
                .checked_add(t.max_priority_fee_per_gas)
                .unwrap_or_else(U256::max_value),
        };

        Some(TransactionReceipt {
            transaction_hash: info.transaction_hash,
            transaction_index: info.transaction_index.into(),
            block_hash: Some(block_hash),
            block_number: Some(block.header.number.as_u64().into()),
            cumulative_gas_used,
            gas_used: Some(gas_used),
            contract_address: info.contract_address,
            logs: {
                let mut pre_receipts_log_index = None;
                if !cumulative_receipts.is_empty() {
                    cumulative_receipts.truncate(cumulative_receipts.len() - 1);
                    pre_receipts_log_index =
                        Some(cumulative_receipts.iter().map(|_r| logs.len() as u32).sum::<u32>());
                }
                logs.iter()
                    .enumerate()
                    .map(|(i, log)| Log {
                        address: log.address,
                        topics: log.topics.clone(),
                        data: log.data.clone(),
                        block_hash: Some(block_hash),
                        block_number: Some(block.header.number.as_u64().into()),
                        transaction_hash: Some(info.transaction_hash),
                        transaction_index: Some(info.transaction_index.into()),
                        log_index: Some(U256::from(
                            (pre_receipts_log_index.unwrap_or(0)) + i as u32,
                        )),
                        transaction_log_index: Some(U256::from(i)),
                        log_type: None,
                        removed: None,
                    })
                    .collect()
            },
            status: Some(status_code.into()),
            root: None,
            logs_bloom,
            transaction_type: None,
            effective_gas_price: Some(effective_gas_price),
        })
    }

    pub async fn transaction_by_block_hash_and_index(
        &self,
        hash: H256,
        index: Index,
    ) -> Result<Option<Transaction>, BlockchainError> {
        if let tx @ Some(_) = self.mined_transaction_by_block_hash_and_index(hash, index) {
            return Ok(tx)
        }

        if let Some(ref fork) = self.fork {
            return Ok(fork.transaction_by_block_hash_and_index(hash, index.into()).await?)
        }

        Ok(None)
    }

    pub fn mined_transaction_by_block_hash_and_index(
        &self,
        block_hash: H256,
        index: Index,
    ) -> Option<Transaction> {
        let block = self.blockchain.storage.read().blocks.get(&block_hash).cloned()?;
        let index: usize = index.into();
        let tx = block.transactions.get(index)?.clone();
        let info = self.blockchain.storage.read().transactions.get(&tx.hash())?.info.clone();
        Some(transaction_build(tx, Some(block), Some(info), true, Some(self.base_fee())))
    }

    pub async fn transaction_by_hash(
        &self,
        hash: H256,
    ) -> Result<Option<Transaction>, BlockchainError> {
        if let tx @ Some(_) = self.mined_transaction_by_hash(hash) {
            return Ok(tx)
        }

        if let Some(ref fork) = self.fork {
            return Ok(fork.transaction_by_hash(hash).await?)
        }

        Ok(None)
    }

    pub fn mined_transaction_by_hash(&self, hash: H256) -> Option<Transaction> {
        let MinedTransaction { info, block_hash, .. } =
            self.blockchain.storage.read().transactions.get(&hash)?.clone();

        let block = self.blockchain.storage.read().blocks.get(&block_hash).cloned()?;

        let tx = block.transactions.get(info.transaction_index as usize)?.clone();

        Some(transaction_build(tx, Some(block), Some(info), true, Some(self.base_fee())))
    }
}

pub fn transaction_build(
    eth_transaction: TypedTransaction,
    block: Option<Block>,
    info: Option<TransactionInfo>,
    is_eip1559: bool,
    base_fee: Option<U256>,
) -> Transaction {
    let mut transaction: Transaction = eth_transaction.clone().into();

    if let TypedTransaction::EIP1559(_) = eth_transaction {
        if block.is_none() && info.is_none() {
            // transaction is not mined yet, gas price is considered just `max_fee_per_gas`
            transaction.gas_price = transaction.max_fee_per_gas;
        } else {
            // if transaction is already mined, gas price is considered base fee + priority fee: the
            // effective gas price.
            let base_fee = base_fee.unwrap_or(U256::zero());
            let max_priority_fee_per_gas =
                transaction.max_priority_fee_per_gas.unwrap_or(U256::zero());
            transaction.gas_price = Some(
                base_fee.checked_add(max_priority_fee_per_gas).unwrap_or_else(U256::max_value),
            );
        }
    } else if !is_eip1559 {
        transaction.max_fee_per_gas = None;
        transaction.max_priority_fee_per_gas = None;
        transaction.transaction_type = None;
    }

    transaction.block_hash =
        block.as_ref().map(|block| H256::from(keccak256(&rlp::encode(&block.header))));

    transaction.block_number = block.as_ref().map(|block| block.header.number.as_u64().into());

    transaction.transaction_index = info.as_ref().map(|status| status.transaction_index.into());

    transaction.from = eth_transaction.recover().unwrap();

    transaction.to = info.as_ref().map_or(eth_transaction.to().cloned(), |status| status.to);

    transaction
}
