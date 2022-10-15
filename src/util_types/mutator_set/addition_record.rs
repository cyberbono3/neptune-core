use serde::{Deserialize, Serialize};

use twenty_first::shared_math::b_field_element::BFieldElement;
use twenty_first::shared_math::rescue_prime_digest::Digest;
use twenty_first::util_types::algebraic_hasher::Hashable;

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct AdditionRecord {
    pub canonical_commitment: Digest,
}

impl AdditionRecord {
    pub fn new(canonical_commitment: Digest) -> Self {
        Self {
            canonical_commitment,
        }
    }
}

impl Hashable for AdditionRecord {
    fn to_sequence(&self) -> Vec<BFieldElement> {
        self.canonical_commitment.values().to_vec()
    }
}

#[cfg(test)]
mod addition_record_tests {
    use crate::util_types::mutator_set::mutator_set_accumulator::MutatorSetAccumulator;

    use twenty_first::shared_math::rescue_prime_regular::RescuePrimeRegular;
    use twenty_first::util_types::algebraic_hasher::AlgebraicHasher;

    use super::*;

    #[test]
    fn hash_identity_test() {
        type H = RescuePrimeRegular;

        let mut msa0: MutatorSetAccumulator<H> = MutatorSetAccumulator::default();
        let addition_record_0: AdditionRecord = msa0
            .set_commitment
            .commit(&H::hash(&1492u128), &H::hash(&1522u128));

        let mut msa1: MutatorSetAccumulator<H> = MutatorSetAccumulator::default();
        let addition_record_1: AdditionRecord = msa1
            .set_commitment
            .commit(&H::hash(&1492u128), &H::hash(&1522u128));

        assert_eq!(
            H::hash(&addition_record_0),
            H::hash(&addition_record_1),
            "Two addition records with same commitments and same MMR AOCLs must agree."
        );

        let mut msa3: MutatorSetAccumulator<H> = MutatorSetAccumulator::default();
        let addition_record_1: AdditionRecord = msa3
            .set_commitment
            .commit(&H::hash(&1451u128), &H::hash(&1480u128));

        // Verify behavior with empty mutator sets. All empty MS' are the same.
        assert_ne!(
            H::hash(&addition_record_0),
            H::hash(&addition_record_1),
            "Two addition records with differing commitments but same MMR AOCLs must differ."
        );
    }

    #[test]
    fn serialization_test() {
        type H = RescuePrimeRegular;

        let mut msa: MutatorSetAccumulator<H> = MutatorSetAccumulator::default();
        let item = H::hash(&1492u128);
        let randomness = H::hash(&1522u128);
        let addition_record: AdditionRecord = msa.set_commitment.commit(&item, &randomness);
        let json = serde_json::to_string(&addition_record).unwrap();
        let s_back = serde_json::from_str::<AdditionRecord>(&json).unwrap();
        assert_eq!(
            addition_record.canonical_commitment,
            s_back.canonical_commitment
        );
    }
}
