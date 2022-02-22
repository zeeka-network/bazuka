use serde::{Deserialize, Serialize};

use crate::core::Transaction;

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Block<Header> {
    // @todo export Sha3 and U256 as generic
    pub header: Header,
    pub body: Vec<Transaction>,
}

impl<Header> Block<Header> {}
