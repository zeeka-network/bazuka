use std::fmt::{Debug, Formatter};
use std::str::FromStr;

use serde::de::{Error, Visitor};
use serde::{Deserializer, Serialize, Serializer};
use sha3::digest::core_api::CoreWrapper;
use sha3::{Digest, Sha3_256, Sha3_256Core};

use super::{AutoDeserialize, AutoHash, AutoSerialize, MemberBound};

pub trait Hash: Debug + Clone + Serialize + 'static {
    /// The length in bytes of the Hasher output
    const LENGTH: usize;

    type Output: MemberBound
        + AutoSerialize
        + AutoDeserialize
        + AutoHash
        + AsRef<[u8]>
        + AsMut<[u8]>
        + Default
        + Copy
        + PartialOrd;

    fn hash(s: &[u8]) -> Self::Output;

    fn update(&mut self, s: &[u8]);

    fn finalize(self) -> Self::Output;
}

#[derive(Debug, Clone, Default)]
pub struct Sha3Hasher(Option<CoreWrapper<Sha3_256Core>>);

impl serde::ser::Serialize for Sha3Hasher {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        serializer.serialize_str("sha3_256")
    }
}

impl<'de> serde::de::Deserialize<'de> for Sha3Hasher {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        struct StringVisitor<T>(std::marker::PhantomData<fn() -> T>);

        impl<'de, T> Visitor<'de> for StringVisitor<T>
        where
            T: std::str::FromStr,
        {
            type Value = T;

            fn expecting(&self, formatter: &mut Formatter) -> std::fmt::Result {
                write!(formatter, "sha3_256")
            }

            fn visit_str<E>(self, v: &str) -> Result<Self::Value, E>
            where
                E: Error,
            {
                FromStr::from_str(v).map_err(|_e| E::custom("hasher was badly mismatched"))
            }
        }

        deserializer.deserialize_str(StringVisitor::<Sha3Hasher>(std::marker::PhantomData))
    }
}

impl Sha3Hasher {
    pub fn new() -> Self {
        Self(Some(Sha3_256::new()))
    }
}

impl FromStr for Sha3Hasher {
    type Err = HasherError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        if s == "sha3_256" {
            Ok(Sha3Hasher::default())
        } else {
            Err(HasherError::Mismatch)
        }
    }
}

impl Hash for Sha3Hasher {
    const LENGTH: usize = 32;
    // U32 is copy from the macro named impl_sha3 in RustCrypto
    type Output = [u8; 32];

    fn hash(s: &[u8]) -> Self::Output {
        let mut h = Sha3_256::new();
        h.update(s);
        h.finalize().into()
    }

    fn update(&mut self, s: &[u8]) {
        if self.0.is_none() {
            self.0 = Some(Sha3_256::new())
        }
        if let Some(h) = self.0.as_mut() {
            h.update(s);
        }
    }

    fn finalize(self) -> Self::Output {
        assert!(self.0.is_some());
        // self.0.as_ref().map(|h| (*h).finalize().into()).unwrap()
        self.0.map(|h| h.finalize().into()).unwrap()
    }
}

#[derive(Debug, thiserror::Error)]
pub enum HasherError {
    #[error("hasher was badly mismatched")]
    Mismatch,
}

#[cfg(test)]
mod tests {
    use crate::core::hash::Hash;
    use crate::core::hash::Sha3Hasher;

    #[test]
    fn test_sha3_works() {
        let output1 = Sha3Hasher::hash(b"123123");

        let mut sha3 = Sha3Hasher::default();
        sha3.update(b"123");
        sha3.update(b"123");
        let output2 = sha3.finalize();
        assert_eq!(output1, output2)
    }
}
