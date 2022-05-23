pub mod ram;

use ff::Field;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use zeekit::Fr;

pub fn check_proof(
    vk: &ZkVerifierKey,
    prev_state: &ZkCompressedState,
    aux_data: &ZkCompressedState,
    next_state: &ZkCompressedState,
    proof: &ZkProof,
) -> bool {
    match vk {
        ZkVerifierKey::Groth16(vk) => {
            if let ZkProof::Groth16(proof) = proof {
                zeekit::groth16_verify(
                    vk,
                    prev_state.state_hash.0,
                    aux_data.state_hash.0,
                    next_state.state_hash.0,
                    proof,
                )
            } else {
                false
            }
        }
        #[cfg(test)]
        ZkVerifierKey::Dummy => {
            if let ZkProof::Dummy(result) = proof {
                *result
            } else {
                false
            }
        }
        _ => {
            unimplemented!()
        }
    }
}

// A single state cell
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize, Default)]
pub struct ZkScalar(Fr);

// Each leaf of the target sparse merkle tree will be the
// result of consecutive hash of `leaf_size` cells.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct ZkStateModel {
    leaf_size: u32,
    tree_depth: u8,
}

impl ZkStateModel {
    pub fn new(leaf_size: u32, tree_depth: u8) -> Self {
        Self {
            leaf_size,
            tree_depth,
        }
    }
}

// Full state of a contract
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
pub struct ZkState(HashMap<u32, ZkScalar>);

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
pub struct ZkStateDelta(HashMap<u32, ZkScalar>);

impl ZkState {
    pub fn as_delta(&self) -> ZkStateDelta {
        ZkStateDelta(self.0.clone())
    }
    pub fn apply_patch(&mut self, patch: &ZkStateDelta) {
        for (k, v) in patch.0.iter() {
            if v.0.is_zero().into() {
                self.0.insert(*k, *v);
            } else {
                self.0.remove(k);
            }
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct ZkCompressedState {
    state_hash: ZkScalar,
    state_size: u32,
}

impl ZkState {
    pub fn size(&self) -> u32 {
        self.0.len() as u32
    }
}

impl ZkCompressedState {
    pub fn size(&self) -> u32 {
        self.state_size
    }
    pub fn empty() -> Self {
        Self {
            state_hash: ZkScalar::default(),
            state_size: 0,
        }
    }
}

impl ZkStateDelta {
    pub fn size(&self) -> isize {
        let mut sz = 0isize;
        for (_, v) in self.0.iter() {
            if v.0.is_zero().into() {
                sz -= 1;
            } else {
                sz += 1;
            }
        }
        sz
    }
}

impl ZkState {
    pub fn compress(&self, _model: ZkStateModel) -> ZkCompressedState {
        let root = ZkScalar(ram::ZkRam::from_state(self).root());
        ZkCompressedState {
            state_hash: root,
            state_size: self.size(),
        }
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub enum ZkVerifierKey {
    Groth16(zeekit::Groth16VerifyingKey),
    Plonk(u8),
    #[cfg(test)]
    Dummy,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ZkContract {
    pub initial_state: ZkCompressedState, // 32byte
    pub state_model: ZkStateModel,
    pub deposit_withdraw: ZkVerifierKey, // VK f(prev_state, io_txs (L1)) -> next_state
    pub update: Vec<ZkVerifierKey>,      // Vec<VK> f(prev_state) -> next_state
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub enum ZkProof {
    Groth16(zeekit::Groth16Proof),
    Plonk(u8),
    #[cfg(test)]
    Dummy(bool),
}
