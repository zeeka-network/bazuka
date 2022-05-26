use thiserror::Error;

use crate::config;
use crate::config::TOTAL_SUPPLY;
use crate::core::{
    hash::Hash, Account, Address, Block, ContractAccount, ContractId, Hasher, Header, Money,
    ProofOfWork, Signature, Transaction, TransactionAndDelta, TransactionData,
};
use crate::db::{KvStore, KvStoreError, RamMirrorKvStore, StringKey, WriteOp};
use crate::utils;
use crate::wallet::Wallet;
use crate::zk;

use serde::{Deserialize, Serialize};
use std::collections::HashMap;

#[derive(Error, Debug)]
pub enum BlockchainError {
    #[error("kvstore error happened: {0}")]
    KvStoreError(#[from] KvStoreError),
    #[error("transaction signature is invalid")]
    SignatureError,
    #[error("balance insufficient")]
    BalanceInsufficient,
    #[error("inconsistency error")]
    Inconsistency,
    #[error("block not found")]
    BlockNotFound,
    #[error("cannot extend from the genesis block")]
    ExtendFromGenesis,
    #[error("cannot extend from very future blocks")]
    ExtendFromFuture,
    #[error("block number invalid")]
    InvalidBlockNumber,
    #[error("parent hash invalid")]
    InvalidParentHash,
    #[error("merkle root invalid")]
    InvalidMerkleRoot,
    #[error("transaction nonce invalid")]
    InvalidTransactionNonce,
    #[error("block timestamp is in past")]
    InvalidTimestamp,
    #[error("unmet difficulty target")]
    DifficultyTargetUnmet,
    #[error("wrong difficulty target on blocks")]
    DifficultyTargetWrong,
    #[error("miner reward not present")]
    MinerRewardNotFound,
    #[error("illegal access to treasury funds")]
    IllegalTreasuryAccess,
    #[error("miner reward transaction is invalid")]
    InvalidMinerReward,
    #[error("contract not found")]
    ContractNotFound,
    #[error("update function not found in the given contract")]
    ContractFunctionNotFound,
    #[error("Incorrect zero-knowledge proof")]
    IncorrectZkProof,
    #[error("Full-state not found in the update provided")]
    FullStateNotFound,
    #[error("Invalid full-state in the update provided")]
    FullStateNotValid,
    #[error("cannot draft a new block when full-states are outdated")]
    StatesOutdated,
    #[error("contract states at requested height are unavailable")]
    StatesUnavailable,
    #[error("block too big")]
    BlockTooBig,
    #[error("compressed-state at specified height not found")]
    CompressedStateNotFound,
    #[error("full-state has invalid deltas")]
    DeltasInvalid,
    #[error("zk error happened")]
    ZkError(#[from] zk::ZkError),
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub enum ZkStatePatch {
    Full(zk::ZkState),
    Delta(zk::ZkStateDelta),
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ZkBlockchainPatch {
    pub patches: HashMap<ContractId, ZkStatePatch>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ZkCompressedStateChange {
    prev_state: zk::ZkCompressedState,
    state: zk::ZkCompressedState,
}

#[derive(Clone)]
pub struct BlockAndPatch {
    pub block: Block,
    pub patch: ZkBlockchainPatch,
}

pub enum TxSideEffect {
    StateChange {
        contract_id: ContractId,
        state_change: ZkCompressedStateChange,
    },
    Nothing,
}

pub trait Blockchain {
    fn validate_transaction(&self, tx_delta: &TransactionAndDelta)
        -> Result<bool, BlockchainError>;
    fn get_account(&self, addr: Address) -> Result<Account, BlockchainError>;
    fn get_contract_account(
        &self,
        contract_id: ContractId,
    ) -> Result<ContractAccount, BlockchainError>;
    fn next_reward(&self) -> Result<Money, BlockchainError>;
    fn will_extend(
        &self,
        from: u64,
        headers: &[Header],
        check_pow: bool,
    ) -> Result<bool, BlockchainError>;
    fn extend(&mut self, from: u64, blocks: &[Block]) -> Result<(), BlockchainError>;
    fn draft_block(
        &self,
        timestamp: u32,
        mempool: &[TransactionAndDelta],
        wallet: &Wallet,
    ) -> Result<BlockAndPatch, BlockchainError>;
    fn get_height(&self) -> Result<u64, BlockchainError>;
    fn get_tip(&self) -> Result<Header, BlockchainError>;
    fn get_headers(&self, since: u64, until: Option<u64>) -> Result<Vec<Header>, BlockchainError>;
    fn get_blocks(&self, since: u64, until: Option<u64>) -> Result<Vec<Block>, BlockchainError>;
    fn get_power(&self) -> Result<u128, BlockchainError>;
    fn pow_key(&self, index: u64) -> Result<Vec<u8>, BlockchainError>;

    fn get_contract(&self, contract_id: ContractId) -> Result<zk::ZkContract, BlockchainError>;
    fn get_state(&self, contract_id: ContractId) -> Result<zk::ZkState, BlockchainError>;

    fn get_outdated_states(
        &self,
    ) -> Result<HashMap<ContractId, zk::ZkCompressedState>, BlockchainError>;
    fn generate_state_patch(
        &self,
        aways: HashMap<ContractId, zk::ZkCompressedState>,
        to: <Hasher as Hash>::Output,
    ) -> Result<ZkBlockchainPatch, BlockchainError>;
    fn update_states(&mut self, patch: &ZkBlockchainPatch) -> Result<(), BlockchainError>;
}

pub struct KvStoreChain<K: KvStore> {
    genesis: BlockAndPatch,
    database: K,
}

impl<K: KvStore> KvStoreChain<K> {
    pub fn new(database: K, genesis: BlockAndPatch) -> Result<KvStoreChain<K>, BlockchainError> {
        let mut chain = KvStoreChain::<K> {
            database,
            genesis: genesis.clone(),
        };
        if chain.get_height()? == 0 {
            chain.apply_block(&genesis.block, true)?;
            chain.update_states(&genesis.patch)?;
        }
        Ok(chain)
    }

    fn fork_on_ram(&self) -> KvStoreChain<RamMirrorKvStore<'_, K>> {
        KvStoreChain {
            database: RamMirrorKvStore::new(&self.database),
            genesis: self.genesis.clone(),
        }
    }

    fn median_timestamp(&self, index: u64) -> Result<u32, BlockchainError> {
        Ok(utils::median(
            &(0..std::cmp::min(index + 1, config::MEDIAN_TIMESTAMP_COUNT))
                .map(|i| {
                    self.get_header(index - i)
                        .map(|b| b.proof_of_work.timestamp)
                })
                .collect::<Result<Vec<u32>, BlockchainError>>()?,
        ))
    }

    fn next_difficulty(&self) -> Result<u32, BlockchainError> {
        let height = self.get_height()?;
        let last_block = self.get_header(height - 1)?;
        if height % config::DIFFICULTY_CALC_INTERVAL == 0 {
            let prev_block = self.get_header(height - config::DIFFICULTY_CALC_INTERVAL)?;
            Ok(utils::calc_pow_difficulty(
                &last_block.proof_of_work,
                &prev_block.proof_of_work,
            ))
        } else {
            Ok(last_block.proof_of_work.target)
        }
    }

    fn get_block(&self, index: u64) -> Result<Block, BlockchainError> {
        if index >= self.get_height()? {
            return Err(BlockchainError::BlockNotFound);
        }
        let block_key: StringKey = format!("block_{:010}", index).into();
        Ok(match self.database.get(block_key)? {
            Some(b) => b.try_into()?,
            None => {
                return Err(BlockchainError::Inconsistency);
            }
        })
    }

    #[allow(dead_code)]
    fn get_compressed_state_at(
        &self,
        contract_id: ContractId,
        index: u64,
    ) -> Result<zk::ZkCompressedState, BlockchainError> {
        let state_model = self.get_contract(contract_id)?.state_model;
        if index >= self.get_contract_account(contract_id)?.height {
            return Err(BlockchainError::CompressedStateNotFound);
        }
        if index == 0 {
            return Ok(zk::ZkState::default().compress(state_model));
        }
        let header_key: StringKey =
            format!("contract_compressed_state_{}_{}", contract_id, index).into();
        Ok(match self.database.get(header_key)? {
            Some(b) => b.try_into()?,
            None => {
                return Err(BlockchainError::Inconsistency);
            }
        })
    }

    fn get_header(&self, index: u64) -> Result<Header, BlockchainError> {
        if index >= self.get_height()? {
            return Err(BlockchainError::BlockNotFound);
        }
        let header_key: StringKey = format!("header_{:010}", index).into();
        Ok(match self.database.get(header_key)? {
            Some(b) => b.try_into()?,
            None => {
                return Err(BlockchainError::Inconsistency);
            }
        })
    }

    fn apply_tx(
        &mut self,
        tx: &Transaction,
        allow_treasury: bool,
    ) -> Result<TxSideEffect, BlockchainError> {
        let mut side_effect = TxSideEffect::Nothing;

        let mut ops = Vec::new();

        let mut acc_src = self.get_account(tx.src.clone())?;

        if tx.src == Address::Treasury && !allow_treasury {
            return Err(BlockchainError::IllegalTreasuryAccess);
        }

        if !tx.verify_signature() {
            return Err(BlockchainError::SignatureError);
        }

        if tx.nonce != acc_src.nonce + 1 {
            return Err(BlockchainError::InvalidTransactionNonce);
        }

        if acc_src.balance < tx.fee {
            return Err(BlockchainError::BalanceInsufficient);
        }

        acc_src.balance -= tx.fee;
        acc_src.nonce += 1;

        match &tx.data {
            TransactionData::RegularSend { dst, amount } => {
                if acc_src.balance < *amount {
                    return Err(BlockchainError::BalanceInsufficient);
                }

                if *dst != tx.src {
                    acc_src.balance -= *amount;

                    let mut acc_dst = self.get_account(dst.clone())?;
                    acc_dst.balance += *amount;

                    ops.push(WriteOp::Put(
                        format!("account_{}", dst).into(),
                        acc_dst.into(),
                    ));
                }
            }
            TransactionData::CreateContract { contract } => {
                let contract_id = ContractId::new(tx);
                ops.push(WriteOp::Put(
                    format!("contract_{}", contract_id).into(),
                    contract.clone().into(),
                ));
                ops.push(WriteOp::Put(
                    format!("contract_account_{}", contract_id).into(),
                    ContractAccount {
                        height: 1,
                        compressed_state: contract.initial_state,
                        balance: 0,
                    }
                    .into(),
                ));
                ops.push(WriteOp::Put(
                    format!("contract_compressed_state_{}_{}", contract_id, 1).into(),
                    contract.initial_state.into(),
                ));
                side_effect = TxSideEffect::StateChange {
                    contract_id,
                    state_change: ZkCompressedStateChange {
                        prev_state: zk::ZkState::default().compress(contract.state_model),
                        state: contract.initial_state,
                    },
                };
            }
            TransactionData::DepositWithdraw {
                contract_id,
                deposit_withdraws: _,
                next_state,
                proof,
            } => {
                let contract = self.get_contract(*contract_id)?;
                let prev_account = self.get_contract_account(*contract_id)?;
                let aux_data = zk::ZkCompressedState::default();
                if !zk::check_proof(
                    &contract.deposit_withdraw,
                    &prev_account.compressed_state,
                    &aux_data,
                    next_state,
                    proof,
                ) {
                    return Err(BlockchainError::IncorrectZkProof);
                }
                ops.push(WriteOp::Put(
                    format!("contract_account_{}", contract_id).into(),
                    ContractAccount {
                        height: prev_account.height + 1,
                        compressed_state: *next_state,
                        balance: 0,
                    }
                    .into(),
                ));
                ops.push(WriteOp::Put(
                    format!(
                        "contract_compressed_state_{}_{}",
                        contract_id,
                        prev_account.height + 1
                    )
                    .into(),
                    (*next_state).into(),
                ));
                side_effect = TxSideEffect::StateChange {
                    contract_id: *contract_id,
                    state_change: ZkCompressedStateChange {
                        prev_state: prev_account.compressed_state,
                        state: *next_state,
                    },
                };
            }
            TransactionData::Update {
                contract_id,
                function_id,
                next_state,
                proof,
            } => {
                let contract = self.get_contract(*contract_id)?;
                let prev_account = self.get_contract_account(*contract_id)?;
                let aux_data = zk::ZkCompressedState::default();
                let vk = contract
                    .update
                    .get(*function_id as usize)
                    .ok_or(BlockchainError::ContractFunctionNotFound)?;
                if !zk::check_proof(
                    vk,
                    &prev_account.compressed_state,
                    &aux_data,
                    next_state,
                    proof,
                ) {
                    return Err(BlockchainError::IncorrectZkProof);
                }
                ops.push(WriteOp::Put(
                    format!("contract_account_{}", contract_id).into(),
                    ContractAccount {
                        height: prev_account.height + 1,
                        compressed_state: *next_state,
                        balance: 0,
                    }
                    .into(),
                ));
                ops.push(WriteOp::Put(
                    format!(
                        "contract_compressed_state_{}_{}",
                        contract_id,
                        prev_account.height + 1
                    )
                    .into(),
                    (*next_state).into(),
                ));
                side_effect = TxSideEffect::StateChange {
                    contract_id: *contract_id,
                    state_change: ZkCompressedStateChange {
                        prev_state: prev_account.compressed_state,
                        state: *next_state,
                    },
                };
            }
        }

        ops.push(WriteOp::Put(
            format!("account_{}", tx.src).into(),
            acc_src.into(),
        ));

        self.database.update(&ops)?;
        Ok(side_effect)
    }

    fn get_changed_states(
        &self,
        index: u64,
    ) -> Result<HashMap<ContractId, ZkCompressedStateChange>, BlockchainError> {
        let k = format!("contract_updates_{:010}", index).into();
        Ok(self
            .database
            .get(k)?
            .map(|b| b.try_into())
            .ok_or(BlockchainError::Inconsistency)??)
    }

    pub fn rollback_block(&mut self) -> Result<(), BlockchainError> {
        let height = self.get_height()?;
        let rollback_key: StringKey = format!("rollback_{:010}", height - 1).into();
        let mut rollback: Vec<WriteOp> = match self.database.get(rollback_key.clone())? {
            Some(b) => b.try_into()?,
            None => {
                return Err(BlockchainError::Inconsistency);
            }
        };

        let mut outdated = self.get_outdated_states()?;
        let changed_states = self.get_changed_states(height - 1)?;
        for (cid, comp) in changed_states {
            if !outdated.contains_key(&cid) {
                let contract = self.get_contract(cid)?;
                let mut state = self.get_state(cid)?;
                if state.rollback().is_ok() {
                    if state.compress(contract.state_model) != comp.prev_state {
                        return Err(BlockchainError::Inconsistency);
                    }
                    rollback.push(WriteOp::Put(
                        format!("contract_state_{}", cid).into(),
                        (&state).into(),
                    ));
                } else if comp.prev_state.height() > 0 {
                    outdated.insert(cid, comp.prev_state);
                }
            }
        }
        rollback.push(WriteOp::Put("outdated".into(), outdated.clone().into()));

        rollback.push(WriteOp::Remove(format!("header_{:010}", height - 1).into()));
        rollback.push(WriteOp::Remove(format!("block_{:010}", height - 1).into()));
        rollback.push(WriteOp::Remove(format!("merkle_{:010}", height - 1).into()));
        rollback.push(WriteOp::Remove(
            format!("contract_updates_{:010}", height - 1).into(),
        ));
        rollback.push(WriteOp::Remove(rollback_key));
        self.database.update(&rollback)?;
        Ok(())
    }

    fn select_transactions(
        &self,
        txs: &[TransactionAndDelta],
    ) -> Result<Vec<TransactionAndDelta>, BlockchainError> {
        let mut sorted = txs.to_vec();
        sorted.sort_by(|t1, t2| t1.tx.nonce.cmp(&t2.tx.nonce));
        let mut fork = self.fork_on_ram();
        let mut result = Vec::new();
        let mut sz = 0isize;
        for tx in sorted.into_iter() {
            let delta = tx.tx.size() as isize + tx.state_delta.clone().unwrap_or_default().size();
            if sz + delta <= config::MAX_DELTA_SIZE as isize && fork.apply_tx(&tx.tx, false).is_ok()
            {
                sz += delta;
                result.push(tx);
            }
        }
        Ok(result)
    }

    fn apply_block(&mut self, block: &Block, check_pow: bool) -> Result<(), BlockchainError> {
        let curr_height = self.get_height()?;
        let is_genesis = block.header.number == 0;
        let next_reward = self.next_reward()?;

        if curr_height > 0 {
            if block.merkle_tree().root() != block.header.block_root {
                return Err(BlockchainError::InvalidMerkleRoot);
            }

            self.will_extend(curr_height, &[block.header.clone()], check_pow)?;
        }

        let mut fork = self.fork_on_ram();

        // All blocks except genesis block should have a miner reward
        let txs = if !is_genesis {
            let reward_tx = block
                .body
                .first()
                .ok_or(BlockchainError::MinerRewardNotFound)?;

            if reward_tx.src != Address::Treasury
                || reward_tx.fee != 0
                || reward_tx.sig != Signature::Unsigned
            {
                return Err(BlockchainError::InvalidMinerReward);
            }
            match reward_tx.data {
                TransactionData::RegularSend { dst: _, amount } => {
                    if amount != next_reward {
                        return Err(BlockchainError::InvalidMinerReward);
                    }
                }
                _ => {
                    return Err(BlockchainError::InvalidMinerReward);
                }
            }

            // Reward tx allowed to get money from Treasury
            fork.apply_tx(reward_tx, true)?;
            &block.body[1..]
        } else {
            &block.body[..]
        };

        let mut body_size = 0usize;
        let mut state_size_delta = 0isize;
        let mut state_updates: HashMap<ContractId, ZkCompressedStateChange> = HashMap::new();
        let mut outdated_states = self.get_outdated_states()?;

        for tx in txs.iter() {
            body_size += tx.size();
            // All genesis block txs are allowed to get from Treasury
            if let TxSideEffect::StateChange {
                contract_id,
                state_change,
            } = fork.apply_tx(tx, is_genesis)?
            {
                state_size_delta +=
                    state_change.state.size() as isize - state_change.prev_state.size() as isize;
                state_updates.insert(contract_id, state_change.clone());
                outdated_states.insert(contract_id, state_change.state);
            }
        }

        if (body_size as isize + state_size_delta) as usize > config::MAX_DELTA_SIZE {
            return Err(BlockchainError::BlockTooBig);
        }

        let mut changes = fork.database.to_ops();

        changes.push(WriteOp::Put("height".into(), (curr_height + 1).into()));

        changes.push(WriteOp::Put(
            format!("power_{:010}", block.header.number).into(),
            (block.header.power() + self.get_power()?).into(),
        ));

        changes.push(WriteOp::Put(
            format!("rollback_{:010}", block.header.number).into(),
            self.database.rollback_of(&changes)?.into(),
        ));
        changes.push(WriteOp::Put(
            format!("header_{:010}", block.header.number).into(),
            block.header.clone().into(),
        ));
        changes.push(WriteOp::Put(
            format!("block_{:010}", block.header.number).into(),
            block.into(),
        ));
        changes.push(WriteOp::Put(
            format!("merkle_{:010}", block.header.number).into(),
            block.merkle_tree().into(),
        ));
        changes.push(WriteOp::Put(
            format!("contract_updates_{:010}", block.header.number).into(),
            state_updates.into(),
        ));
        changes.push(WriteOp::Put("outdated".into(), outdated_states.into()));

        self.database.update(&changes)?;
        Ok(())
    }
    pub fn get_outdated_states_request(
        &self,
    ) -> Result<HashMap<ContractId, zk::ZkCompressedState>, BlockchainError> {
        let outdated = self.get_outdated_states()?;
        let mut ret = HashMap::new();
        for (cid, _) in outdated {
            let contract = self.get_contract(cid)?;
            ret.insert(cid, self.get_state(cid)?.compress(contract.state_model));
        }
        Ok(ret)
    }
}

impl<K: KvStore> Blockchain for KvStoreChain<K> {
    fn get_outdated_states(
        &self,
    ) -> Result<HashMap<ContractId, zk::ZkCompressedState>, BlockchainError> {
        Ok(match self.database.get("outdated".into())? {
            Some(b) => b.try_into()?,
            None => HashMap::new(),
        })
    }
    fn get_tip(&self) -> Result<Header, BlockchainError> {
        self.get_header(self.get_height()? - 1)
    }
    fn get_contract(&self, contract_id: ContractId) -> Result<zk::ZkContract, BlockchainError> {
        let k = format!("contract_{}", contract_id).into();
        Ok(self
            .database
            .get(k)?
            .map(|b| b.try_into())
            .ok_or(BlockchainError::ContractNotFound)??)
    }
    fn get_contract_account(
        &self,
        contract_id: ContractId,
    ) -> Result<ContractAccount, BlockchainError> {
        let k = format!("contract_account_{}", contract_id).into();
        Ok(self
            .database
            .get(k)?
            .map(|b| b.try_into())
            .ok_or(BlockchainError::ContractNotFound)??)
    }
    fn get_state(&self, contract_id: ContractId) -> Result<zk::ZkState, BlockchainError> {
        let k = format!("contract_state_{}", contract_id).into();
        Ok(match self.database.get(k)? {
            Some(b) => b.try_into()?,
            None => zk::ZkState::default(),
        })
    }

    fn get_account(&self, addr: Address) -> Result<Account, BlockchainError> {
        let k = format!("account_{}", addr).into();
        Ok(match self.database.get(k)? {
            Some(b) => b.try_into()?,
            None => Account {
                balance: if addr == Address::Treasury {
                    TOTAL_SUPPLY
                } else {
                    0
                },
                nonce: 0,
            },
        })
    }

    fn will_extend(
        &self,
        from: u64,
        headers: &[Header],
        check_pow: bool,
    ) -> Result<bool, BlockchainError> {
        let current_power = self.get_power()?;

        if from == 0 {
            return Err(BlockchainError::ExtendFromGenesis);
        } else if from > self.get_height()? {
            return Err(BlockchainError::ExtendFromFuture);
        }

        let mut new_power: u128 = self
            .database
            .get(format!("power_{:010}", from - 1).into())?
            .ok_or(BlockchainError::Inconsistency)?
            .try_into()?;

        let mut last_header = self.get_header(from - 1)?;
        let mut last_pow = self
            .get_header(
                last_header.number - (last_header.number % config::DIFFICULTY_CALC_INTERVAL),
            )?
            .proof_of_work;

        for h in headers.iter() {
            if h.number % config::DIFFICULTY_CALC_INTERVAL == 0 {
                if h.proof_of_work.target
                    != utils::calc_pow_difficulty(&last_header.proof_of_work, &last_pow)
                {
                    return Err(BlockchainError::DifficultyTargetWrong);
                }
                last_pow = h.proof_of_work;
            }

            let pow_key = self.pow_key(h.number)?;

            if h.proof_of_work.timestamp < self.median_timestamp(from - 1)? {
                return Err(BlockchainError::InvalidTimestamp);
            }

            if last_pow.target != h.proof_of_work.target {
                return Err(BlockchainError::DifficultyTargetWrong);
            }

            if check_pow && !h.meets_target(&pow_key) {
                return Err(BlockchainError::DifficultyTargetUnmet);
            }

            if h.number != last_header.number + 1 {
                return Err(BlockchainError::InvalidBlockNumber);
            }

            if h.parent_hash != last_header.hash() {
                return Err(BlockchainError::InvalidParentHash);
            }

            last_header = h.clone();
            new_power += h.power();
        }

        Ok(new_power > current_power)
    }
    fn extend(&mut self, from: u64, blocks: &[Block]) -> Result<(), BlockchainError> {
        let curr_height = self.get_height()?;

        if from == 0 {
            return Err(BlockchainError::ExtendFromGenesis);
        } else if from > curr_height {
            return Err(BlockchainError::ExtendFromFuture);
        }

        let mut forked = self.fork_on_ram();

        while forked.get_height()? > from {
            forked.rollback_block()?;
        }

        for block in blocks.iter() {
            forked.apply_block(block, true)?;
        }
        let ops = forked.database.to_ops();

        self.database.update(&ops)?;
        Ok(())
    }
    fn get_height(&self) -> Result<u64, BlockchainError> {
        Ok(match self.database.get("height".into())? {
            Some(b) => b.try_into()?,
            None => 0,
        })
    }
    fn get_headers(&self, since: u64, until: Option<u64>) -> Result<Vec<Header>, BlockchainError> {
        let mut blks: Vec<Header> = Vec::new();
        let height = self.get_height()?;
        for i in since..until.unwrap_or(height) {
            if i >= height {
                break;
            }
            blks.push(self.get_header(i)?);
        }
        Ok(blks)
    }
    fn get_blocks(&self, since: u64, until: Option<u64>) -> Result<Vec<Block>, BlockchainError> {
        let mut blks: Vec<Block> = Vec::new();
        let height = self.get_height()?;
        for i in since..until.unwrap_or(height) {
            if i >= height {
                break;
            }
            blks.push(self.get_block(i)?);
        }
        Ok(blks)
    }
    fn next_reward(&self) -> Result<Money, BlockchainError> {
        let supply = self.get_account(Address::Treasury)?.balance;
        Ok(supply / config::REWARD_RATIO)
    }
    fn draft_block(
        &self,
        timestamp: u32,
        mempool: &[TransactionAndDelta],
        wallet: &Wallet,
    ) -> Result<BlockAndPatch, BlockchainError> {
        let height = self.get_height()?;
        let outdated_states = self.get_outdated_states()?;

        if outdated_states.len() > 0 {
            return Err(BlockchainError::StatesOutdated);
        }

        let last_header = self.get_header(height - 1)?;
        let treasury_nonce = self.get_account(Address::Treasury)?.nonce;

        let mut txs = vec![Transaction {
            src: Address::Treasury,
            data: TransactionData::RegularSend {
                dst: wallet.get_address(),
                amount: self.next_reward()?,
            },
            nonce: treasury_nonce + 1,
            fee: 0,
            sig: Signature::Unsigned,
        }];

        let tx_and_deltas = self.select_transactions(mempool)?;
        let mut block_delta: HashMap<ContractId, ZkStatePatch> = HashMap::new();
        for tx_delta in tx_and_deltas.iter() {
            if let Some(contract_id) = match &tx_delta.tx.data {
                TransactionData::CreateContract { .. } => Some(ContractId::new(&tx_delta.tx)),
                TransactionData::DepositWithdraw { contract_id, .. } => Some(*contract_id),
                TransactionData::Update { contract_id, .. } => Some(*contract_id),
                _ => None,
            } {
                block_delta.insert(
                    contract_id,
                    ZkStatePatch::Delta(
                        tx_delta
                            .state_delta
                            .clone()
                            .ok_or(BlockchainError::FullStateNotFound)?,
                    ),
                );
            }
        }
        let block_delta = ZkBlockchainPatch {
            patches: block_delta,
        };

        txs.extend(tx_and_deltas.iter().map(|tp| tp.tx.clone()));

        let mut blk = Block {
            header: Header {
                parent_hash: last_header.hash(),
                number: height as u64,
                block_root: Default::default(),
                proof_of_work: ProofOfWork {
                    timestamp,
                    target: self.next_difficulty()?,
                    nonce: 0,
                },
            },
            body: txs,
        };
        blk.header.block_root = blk.merkle_tree().root();

        let mut ram_fork = self.fork_on_ram();
        ram_fork.apply_block(&blk, false)?; // Check if everything is ok
        ram_fork.update_states(&block_delta)?;
        Ok(BlockAndPatch {
            block: blk,
            patch: block_delta,
        })
    }

    fn get_power(&self) -> Result<u128, BlockchainError> {
        let height = self.get_height()?;
        if height == 0 {
            Ok(0)
        } else {
            Ok(self
                .database
                .get(format!("power_{:010}", height - 1).into())?
                .ok_or(BlockchainError::Inconsistency)?
                .try_into()?)
        }
    }

    fn pow_key(&self, index: u64) -> Result<Vec<u8>, BlockchainError> {
        Ok(if index < config::POW_KEY_CHANGE_DELAY {
            config::POW_BASE_KEY.to_vec()
        } else {
            let reference = ((index - config::POW_KEY_CHANGE_DELAY)
                / config::POW_KEY_CHANGE_INTERVAL)
                * config::POW_KEY_CHANGE_INTERVAL;
            self.get_header(reference)?.hash().to_vec()
        })
    }

    fn update_states(&mut self, patch: &ZkBlockchainPatch) -> Result<(), BlockchainError> {
        let mut ops = Vec::new();
        let mut outdated_states = self.get_outdated_states()?;

        for (cid, comp_state) in outdated_states.clone() {
            let contract = self.get_contract(cid)?;
            let contract_account = self.get_contract_account(cid)?;
            let patch = patch
                .patches
                .get(&cid)
                .ok_or(BlockchainError::FullStateNotFound)?;
            let full_state = match &patch {
                ZkStatePatch::Full(full) => {
                    for (i, calc_state) in full
                        .compress_prev_states(contract.state_model)
                        .into_iter()
                        .enumerate()
                    {
                        let actual_state = self
                            .get_compressed_state_at(cid, contract_account.height - 1 - i as u64)?;
                        if calc_state != actual_state {
                            return Err(BlockchainError::DeltasInvalid);
                        }
                    }
                    full.clone()
                }
                ZkStatePatch::Delta(delta) => {
                    let mut state = self.get_state(cid)?;
                    state.push_delta(delta);
                    state
                }
            };
            if full_state.compress(contract.state_model) == comp_state {
                ops.push(WriteOp::Put(
                    format!("contract_state_{}", cid).into(),
                    (&full_state).into(),
                ));
            } else {
                return Err(BlockchainError::FullStateNotValid);
            }
            outdated_states.remove(&cid);
        }

        ops.push(WriteOp::Put("outdated".into(), outdated_states.into()));

        self.database.update(&ops)?;
        Ok(())
    }

    fn validate_transaction(
        &self,
        tx_delta: &TransactionAndDelta,
    ) -> Result<bool, BlockchainError> {
        Ok(
            self.get_account(tx_delta.tx.src.clone())?.balance > 0
                && tx_delta.tx.verify_signature(),
        )
    }
    fn generate_state_patch(
        &self,
        aways: HashMap<ContractId, zk::ZkCompressedState>,
        to: <Hasher as Hash>::Output,
    ) -> Result<ZkBlockchainPatch, BlockchainError> {
        let height = self.get_height()?;
        let last_header = self.get_header(height - 1)?;

        if last_header.hash() != to {
            return Err(BlockchainError::StatesUnavailable);
        }

        let outdated_states = self.get_outdated_states()?;

        let mut blockchain_patch = ZkBlockchainPatch {
            patches: HashMap::new(),
        };
        for (cid, away) in aways {
            if !outdated_states.contains_key(&cid) {
                let state = self.get_state(cid)?;
                let away = state.height() - away.height();
                blockchain_patch.patches.insert(
                    cid,
                    if let Ok(delta) = state.delta_of(away as usize) {
                        ZkStatePatch::Delta(delta)
                    } else {
                        ZkStatePatch::Full(state)
                    },
                );
            }
        }

        Ok(blockchain_patch)
    }
}

#[cfg(test)]
mod test;
