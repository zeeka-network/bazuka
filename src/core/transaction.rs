use super::address::{Address, Signature};
use super::hash::Hash;
use super::Money;
use crate::crypto::SignatureScheme;
use crate::zk::{ZkProof, ZkScalar, ZkStateData, ZkStateModel, ZkVerifierKey};

#[derive(serde::Serialize, serde::Deserialize, PartialEq, Debug, Clone)]
pub struct ContractId<H: Hash> {
    hash: H::Output,
}

#[derive(serde::Serialize, serde::Deserialize, PartialEq, Debug, Clone)]
pub enum PaymentDirection {
    Deposit,
    Withdraw,
}

#[derive(serde::Serialize, serde::Deserialize, PartialEq, Debug, Clone)]
pub struct ContractPayment<H: Hash, S: SignatureScheme> {
    initiator: Address<S>,
    contract_id: ContractId<H>, // Makes sure the payment can only run on this contract.
    nonce: usize, // Makes sure a contract payment cannot be replayed on this contract.
    amount: Money,
    fee: Money,
    direction: PaymentDirection,
    sig: Signature<S>,
}

// A transaction could be as simple as sending some funds, or as complicated as
// creating a smart-contract.
#[derive(serde::Serialize, serde::Deserialize, PartialEq, Debug, Clone)]
pub enum TransactionData<H: Hash, S: SignatureScheme> {
    RegularSend {
        dst: Address<S>,
        amount: Money,
    },
    // Create a Zero-Contract. The creator can consider multiple ways (Circuits) of updating
    // the state. But there should be only one circuit for entering and exiting the contract.
    CreateContract {
        deposit_withdraw_circuit: ZkVerifierKey,
        update_circuits: Vec<ZkVerifierKey>,
        initial_state: ZkStateData,
        state_model: ZkStateModel,
    },
    // Proof for DepositWithdrawCircuit(curr_state, next_state, hash(entries))
    DepositWithdraw {
        contract_id: ContractId<H>,
        deposit_withdraws: Vec<ContractPayment<H, S>>,
        next_state: ZkScalar,
        proof: ZkProof,
    },
    // Proof for UpdateCircuit[circuit_index](curr_state, next_state)
    Update {
        contract_id: ContractId<H>,
        circuit_index: u32,
        next_state: ZkScalar,
        proof: ZkProof,
    },
}

#[derive(serde::Serialize, serde::Deserialize, Debug, Clone)]
pub struct Transaction<H: Hash, S: SignatureScheme> {
    pub src: Address<S>,
    pub nonce: u32,
    pub data: TransactionData<H, S>,
    pub fee: Money,
    pub sig: Signature<S>,
}

impl<H: Hash, S: SignatureScheme> PartialEq<Transaction<H, S>> for Transaction<H, S> {
    fn eq(&self, other: &Transaction<H, S>) -> bool {
        bincode::serialize(self).unwrap() == bincode::serialize(other).unwrap()
    }
}

impl<H: Hash, S: SignatureScheme> Transaction<H, S> {
    pub fn hash(&self) -> H::Output {
        H::hash(&bincode::serialize(self).unwrap())
    }
    pub fn verify_signature(&self) -> bool {
        match &self.src {
            Address::<S>::Treasury => true,
            Address::<S>::PublicKey(pk) => match &self.sig {
                Signature::Unsigned => false,
                Signature::Signed(sig) => {
                    let mut unsigned = self.clone();
                    unsigned.sig = Signature::Unsigned;
                    let bytes = bincode::serialize(&unsigned).unwrap();
                    S::verify(pk, &bytes, sig)
                }
            },
        }
    }
}

impl<H: Hash, S: SignatureScheme + PartialEq> Eq for Transaction<H, S> {}
impl<H: Hash, S: SignatureScheme> std::hash::Hash for Transaction<H, S> {
    fn hash<Hasher>(&self, state: &mut Hasher)
    where
        Hasher: std::hash::Hasher,
    {
        state.write(&bincode::serialize(self).unwrap());
        state.finish();
    }
}
