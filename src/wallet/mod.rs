use crate::core::{
    Address, ContractId, ContractPayment, ContractUpdate, Money, PaymentDirection, Signature,
    Signer, Transaction, TransactionAndDelta, TransactionData, ZkSigner,
};
use crate::crypto::SignatureScheme;
use crate::crypto::ZkSignatureScheme;
use crate::zk;

#[derive(Clone)]
pub struct Wallet {
    seed: Vec<u8>,
    private_key: <Signer as SignatureScheme>::Priv,
    zk_private_key: <ZkSigner as ZkSignatureScheme>::Priv,
    address: Address,
}

impl Wallet {
    pub fn new(seed: Vec<u8>) -> Self {
        let (pk, sk) = Signer::generate_keys(&seed);
        let (_, zk_sk) = ZkSigner::generate_keys(&seed);
        Self {
            seed,
            address: Address::PublicKey(pk),
            private_key: sk,
            zk_private_key: zk_sk,
        }
    }
    pub fn get_address(&self) -> Address {
        self.address.clone()
    }
    pub fn sign(&self, tx: &mut Transaction) {
        let bytes = bincode::serialize(&tx).unwrap();
        tx.sig = Signature::Signed(Signer::sign(&self.private_key, &bytes));
    }
    pub fn create_transaction(
        &self,
        dst: Address,
        amount: Money,
        fee: Money,
        nonce: u32,
    ) -> TransactionAndDelta {
        let mut tx = Transaction {
            src: self.get_address(),
            data: TransactionData::RegularSend { dst, amount },
            nonce,
            fee,
            sig: Signature::Unsigned,
        };
        self.sign(&mut tx);
        TransactionAndDelta {
            tx,
            state_delta: None,
        }
    }
    pub fn create_contract(
        &self,
        contract: zk::ZkContract,
        initial_state: zk::ZkDataPairs,
        fee: Money,
        nonce: u32,
    ) -> TransactionAndDelta {
        let mut tx = Transaction {
            src: self.get_address(),
            data: TransactionData::CreateContract { contract },
            nonce,
            fee,
            sig: Signature::Unsigned,
        };
        self.sign(&mut tx);
        TransactionAndDelta {
            tx,
            state_delta: Some(initial_state.as_delta()),
        }
    }

    #[allow(clippy::too_many_arguments)]
    pub fn call_function(
        &self,
        contract_id: ContractId,
        function_id: u32,
        state_delta: zk::ZkDeltaPairs,
        next_state: zk::ZkCompressedState,
        proof: zk::ZkProof,
        fee: Money,
        nonce: u32,
    ) -> TransactionAndDelta {
        let (_, sk) = Signer::generate_keys(&self.seed);
        let mut tx = Transaction {
            src: self.get_address(),
            data: TransactionData::UpdateContract {
                contract_id,
                updates: vec![ContractUpdate::FunctionCall {
                    function_id,
                    next_state,
                    proof,
                }],
            },
            nonce,
            fee,
            sig: Signature::Unsigned,
        };
        let bytes = bincode::serialize(&tx).unwrap();
        tx.sig = Signature::Signed(Signer::sign(&sk, &bytes));
        TransactionAndDelta {
            tx,
            state_delta: Some(state_delta),
        }
    }

    #[allow(clippy::too_many_arguments)]
    pub fn contract_deposit_withdraw(
        &self,
        contract_id: ContractId,
        address_index: u32,
        nonce: u32,
        amount: Money,
        fee: Money,
        withdraw: bool,
    ) -> ContractPayment {
        let mut tx = ContractPayment {
            address: self.private_key.clone().into(),
            zk_address: self.zk_private_key.clone().into(),
            zk_address_index: address_index,
            contract_id,
            nonce,
            amount,
            fee,
            direction: if withdraw {
                PaymentDirection::Withdraw(None)
            } else {
                PaymentDirection::Deposit(None)
            },
        };
        let bytes = bincode::serialize(&tx).unwrap();
        match &mut tx.direction {
            PaymentDirection::Withdraw(sig) => {
                *sig = Some(ZkSigner::sign(
                    &self.zk_private_key,
                    crate::zk::hash_to_scalar(&bytes),
                ));
            }
            PaymentDirection::Deposit(sig) => {
                *sig = Some(Signer::sign(&self.private_key, &bytes));
            }
        }
        tx
    }
}
