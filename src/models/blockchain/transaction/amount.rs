use std::{
    fmt::Display,
    iter::Sum,
    ops::{Add, Neg, Sub},
    str::FromStr,
};

use anyhow::bail;
use itertools::Itertools;
use num_bigint::BigInt;
use num_traits::{CheckedSub, Signed, Zero};
use serde::{de::DeserializeOwned, Deserialize, Serialize};
use twenty_first::{
    amount::u32s::U32s,
    shared_math::{b_field_element::BFieldElement, tip5::Digest},
    util_types::algebraic_hasher::Hashable,
};

use super::native_coin::NATIVE_COIN_TYPESCRIPT_DIGEST;

pub trait AmountLike:
    Add
    + Sum
    + CheckedSub
    + Neg
    + PartialEq
    + Eq
    + PartialOrd
    + Ord
    + Zero
    + FromStr
    + Display
    + Copy
    + Serialize
    + DeserializeOwned
    + From<i32>
    + From<u32>
    + From<u64>
    + Hashable
{
    fn from_bfes(bfes: &[BFieldElement]) -> Self;
    fn scalar_mul(&self, factor: u64) -> Self;
}

#[derive(Clone, Copy, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub enum Sign {
    NonNegative,
    Negative,
}

pub const AMOUNT_SIZE_FOR_U32: usize = 4;

#[derive(Clone, Copy, Debug, Serialize, Deserialize, Eq)]
pub struct Amount(pub U32s<AMOUNT_SIZE_FOR_U32>);

impl AmountLike for Amount {
    fn from_bfes(bfes: &[BFieldElement]) -> Self {
        let limbs: [u32; AMOUNT_SIZE_FOR_U32] = bfes
            .iter()
            .map(|b| b.value() as u32)
            .collect_vec()
            .try_into()
            .unwrap();
        Amount(U32s::new(limbs))
    }

    fn scalar_mul(&self, factor: u64) -> Self {
        let factor_as_u32s: U32s<AMOUNT_SIZE_FOR_U32> = factor.try_into().unwrap();
        Amount(factor_as_u32s * self.0)
    }
}

impl Ord for Amount {
    fn cmp(&self, other: &Self) -> std::cmp::Ordering {
        self.0.cmp(&other.0)
    }
}

impl Amount {
    /// Return the element that corresponds to 1. Use in tests only.
    pub fn one() -> Amount {
        let mut values = [0u32; AMOUNT_SIZE_FOR_U32];
        values[0] = 1;
        Amount(U32s::new(values))
    }

    pub fn div_two(&mut self) {
        self.0.div_two();
    }

    pub fn to_native_coins(&self) -> Vec<(Digest, Vec<BFieldElement>)> {
        let mut dictionary: Vec<(Digest, Vec<BFieldElement>)> = vec![];
        dictionary.push((NATIVE_COIN_TYPESCRIPT_DIGEST, self.to_sequence()));
        dictionary
    }
}

impl Display for Amount {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.0)
    }
}

impl Add for Amount {
    type Output = Self;

    fn add(self, rhs: Self) -> Self::Output {
        Self(self.0 + rhs.0)
    }
}

impl Sum for Amount {
    fn sum<I: Iterator<Item = Self>>(iter: I) -> Self {
        Amount(iter.map(|a| a.0).sum())
    }
}

impl Sub for Amount {
    type Output = Amount;

    fn sub(self, _rhs: Self) -> Self::Output {
        panic!("Cannot subtract Amounts; use `checked_sub` instead.")
    }
}

impl CheckedSub for Amount {
    fn checked_sub(&self, v: &Self) -> Option<Self> {
        if self >= v {
            Some(Amount(self.0 - v.0))
        } else {
            None
        }
    }
}

impl Neg for Amount {
    type Output = Self;

    fn neg(self) -> Self::Output {
        Self::zero() - self
    }
}

impl PartialEq for Amount {
    fn eq(&self, other: &Self) -> bool {
        self.0 == other.0
    }
}

impl PartialOrd for Amount {
    fn partial_cmp(&self, other: &Self) -> Option<std::cmp::Ordering> {
        self.0.partial_cmp(&other.0)
    }
}

impl Zero for Amount {
    fn zero() -> Self {
        Amount(U32s::<AMOUNT_SIZE_FOR_U32>::zero())
    }

    fn is_zero(&self) -> bool {
        self.0 == U32s::<AMOUNT_SIZE_FOR_U32>::zero()
    }
}

impl FromStr for Amount {
    type Err = anyhow::Error;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        if let Ok(big_int) = BigInt::from_str(s) {
            if big_int.is_positive() {
                Ok(Amount(U32s::<AMOUNT_SIZE_FOR_U32>::from(
                    big_int.to_biguint().unwrap(),
                )))
            } else if big_int.is_zero() {
                Ok(Amount::zero())
            } else {
                Ok(Amount::zero()
                    - Amount(U32s::<AMOUNT_SIZE_FOR_U32>::from(
                        big_int.neg().to_biguint().unwrap(),
                    )))
            }
        } else {
            bail!("Cannot parse string as amount.");
        }
    }
}

impl Hashable for Amount {
    fn to_sequence(&self) -> Vec<BFieldElement> {
        self.0.to_sequence()
    }
}

impl From<i32> for Amount {
    fn from(value: i32) -> Self {
        let mut limbs = [0u32; AMOUNT_SIZE_FOR_U32];
        if value < 0 {
            limbs[0] = -value as u32;
            -Amount(U32s::new(limbs))
        } else {
            limbs[0] = value as u32;
            Amount(U32s::new(limbs))
        }
    }
}

impl From<u32> for Amount {
    fn from(value: u32) -> Self {
        let mut limbs = [0u32; AMOUNT_SIZE_FOR_U32];
        limbs[0] = value;
        Amount(U32s::new(limbs))
    }
}

impl From<u64> for Amount {
    fn from(value: u64) -> Self {
        let mut limbs = [0u32; AMOUNT_SIZE_FOR_U32];
        limbs[0] = (value & (u32::MAX as u64)) as u32;
        limbs[1] = (value >> 32) as u32;
        Amount(U32s::new(limbs))
    }
}

#[cfg(test)]
mod amount_tests {
    use std::str::FromStr;

    use itertools::Itertools;
    use rand::{thread_rng, Rng, RngCore};
    use twenty_first::{amount::u32s::U32s, util_types::algebraic_hasher::Hashable};

    use crate::models::blockchain::transaction::amount::{Amount, AmountLike};

    use super::AMOUNT_SIZE_FOR_U32;

    #[test]
    fn test_string_conversion() {
        let mut rng = thread_rng();

        for _ in 0..100 {
            let limbs: [u32; AMOUNT_SIZE_FOR_U32] = (0..AMOUNT_SIZE_FOR_U32)
                .map(|_| rng.next_u32())
                .collect_vec()
                .try_into()
                .unwrap();
            let amount = Amount(U32s::new(limbs));
            let string = amount.to_string();
            let reconstructed_amount = Amount::from_str(&string)
                .expect("Coult not parse as number a string generated from a number.");

            assert_eq!(amount, reconstructed_amount);
        }
    }

    #[test]
    fn test_bfe_conversion() {
        let mut rng = thread_rng();

        for _ in 0..100 {
            let limbs: [u32; AMOUNT_SIZE_FOR_U32] = (0..AMOUNT_SIZE_FOR_U32)
                .map(|_| rng.next_u32())
                .collect_vec()
                .try_into()
                .unwrap();
            let amount = Amount(U32s::new(limbs));
            let bfes = amount.to_sequence();
            let reconstructed_amount = Amount::from_bfes(&bfes);

            assert_eq!(amount, reconstructed_amount);
        }
    }

    #[test]
    fn from_u64_conversion_simple_test() {
        let a: u64 = u32::MAX as u64;
        let b: u64 = 100u64;
        let a_amount: Amount = a.into();
        let b_amount: Amount = b.into();
        assert_eq!(a_amount + b_amount, (a + b).into());
    }

    #[test]
    fn from_u64_conversion_pbt() {
        let mut rng = thread_rng();
        let a: u64 = rng.gen_range(0..(1 << 63));
        let b: u64 = rng.gen_range(0..(1 << 63));
        let a_amount: Amount = a.into();
        let b_amount: Amount = b.into();
        assert_eq!(a_amount + b_amount, (a + b).into());
    }

    #[test]
    fn amount_simple_scalar_mul_test() {
        let fourteen: Amount = 14.into();
        let fourtytwo: Amount = 42.into();
        assert_eq!(fourtytwo, fourteen.scalar_mul(3));
    }

    #[test]
    fn amount_scalar_mul_pbt() {
        let mut rng = thread_rng();
        let a: u64 = rng.gen_range(0..u32::MAX as u64);
        let b: u64 = rng.gen_range(0..u32::MAX as u64);
        let prod_checked: Amount = (a * b).into();
        let mut prod_calculated: Amount = Into::<Amount>::into(a);
        prod_calculated = prod_calculated.scalar_mul(b);
        assert_eq!(prod_checked, prod_calculated);
    }
}
