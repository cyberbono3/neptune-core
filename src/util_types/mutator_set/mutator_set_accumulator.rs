use serde::{Deserialize, Serialize};
use twenty_first::util_types::{
    mmr::{mmr_accumulator::MmrAccumulator, mmr_trait::Mmr},
    simple_hasher::{Hashable, Hasher},
};

use super::{
    active_window::ActiveWindow, addition_record::AdditionRecord,
    ms_membership_proof::MsMembershipProof, mutator_set_trait::MutatorSet,
    removal_record::RemovalRecord, set_commitment::SetCommitment,
};

#[derive(Clone, Serialize, Deserialize, Debug, PartialEq, Eq)]
pub struct MutatorSetAccumulator<H: Hasher>
where
    u128: Hashable<<H as Hasher>::T>,
{
    pub set_commitment: SetCommitment<H, MmrAccumulator<H>>,
}

impl<H: Hasher> MutatorSetAccumulator<H>
where
    u128: Hashable<<H as Hasher>::T>,
{
    pub fn default() -> Self {
        let set_commitment = SetCommitment::<H, MmrAccumulator<H>> {
            aocl: MmrAccumulator::<H>::new(vec![]),
            swbf_inactive: MmrAccumulator::<H>::new(vec![]),
            swbf_active: ActiveWindow::default(),
        };

        Self { set_commitment }
    }
}

impl<H: Hasher> MutatorSet<H> for MutatorSetAccumulator<H>
where
    u128: Hashable<<H as Hasher>::T>,
{
    fn prove(
        &mut self,
        item: &H::Digest,
        randomness: &H::Digest,
        store_bits: bool,
    ) -> MsMembershipProof<H> {
        self.set_commitment.prove(item, randomness, store_bits)
    }

    fn verify(&mut self, item: &H::Digest, membership_proof: &MsMembershipProof<H>) -> bool {
        self.set_commitment.verify(item, membership_proof)
    }

    fn commit(&mut self, item: &H::Digest, randomness: &H::Digest) -> AdditionRecord<H> {
        self.set_commitment.commit(item, randomness)
    }

    fn drop(
        &mut self,
        item: &H::Digest,
        membership_proof: &MsMembershipProof<H>,
    ) -> RemovalRecord<H> {
        self.set_commitment.drop(item, membership_proof)
    }

    fn add(&mut self, addition_record: &mut AdditionRecord<H>) {
        self.set_commitment.add_helper(addition_record);
    }

    fn remove(&mut self, removal_record: &RemovalRecord<H>) -> Option<Vec<u128>> {
        self.set_commitment.remove_helper(removal_record);

        // Only an ArchivalMutatorSet can calculate the diff indices
        None
    }

    fn get_commitment(&mut self) -> <H as Hasher>::Digest {
        let aocl_mmr_bagged = self.set_commitment.aocl.bag_peaks();
        let inactive_swbf_bagged = self.set_commitment.swbf_inactive.bag_peaks();
        let active_swbf_bagged = self.set_commitment.swbf_active.hash();
        let hasher = H::new();
        hasher.hash_many(&[aocl_mmr_bagged, inactive_swbf_bagged, active_swbf_bagged])
    }

    fn batch_remove(
        &mut self,
        removal_records: Vec<RemovalRecord<H>>,
        preserved_membership_proofs: &mut Vec<&mut MsMembershipProof<H>>,
    ) -> Option<Vec<u128>> {
        todo!()
    }
}

#[cfg(test)]
mod ms_accumulator_tests {
    use crate::test_shared::mutator_set::{empty_archival_ms, make_item_and_randomness_for_blake3};
    use crate::util_types::mutator_set::archival_mutator_set::ArchivalMutatorSet;
    use proptest::prelude::Rng;
    use twenty_first::util_types::simple_hasher::Hasher;

    use super::*;

    #[test]
    fn mutator_set_accumulator_pbt() {
        // This tests verifies that items can be added and removed from the mutator set
        // without assuming anything about the order of the adding and removal. It also
        // verifies that the membership proofs handled through an mutator set accumulator
        // are the same as those that are produced from an archival mutator set.

        // This function mixes both archival and accumulator testing.
        // It *may* be considered bad style to do it this way, but there is a
        // lot of code duplication that is avoided by doing that.
        type H = blake3::Hasher;
        let mut accumulator: MutatorSetAccumulator<H> = MutatorSetAccumulator::default();
        let mut archival_after_remove: ArchivalMutatorSet<H> = empty_archival_ms();
        let mut archival_before_remove: ArchivalMutatorSet<H> = empty_archival_ms();
        let number_of_interactions = 100;
        let mut prng = rand::thread_rng();

        // The outer loop runs two times:
        // 1. insert `number_of_interactions / 2` items, then randomly insert and remove `number_of_interactions / 2` times
        // 2. Randomly insert and remove `number_of_interactions` times
        // This should test both inserting/removing in an empty MS and in a non-empty MS
        for start_fill in [false, true] {
            let mut membership_proofs_batch: Vec<MsMembershipProof<H>> = vec![];
            let mut membership_proofs_sequential: Vec<MsMembershipProof<H>> = vec![];
            let mut items: Vec<<H as Hasher>::Digest> = vec![];
            let mut rands: Vec<<H as Hasher>::Digest> = vec![];
            let mut last_ms_commitment: Option<<H as Hasher>::Digest> = None;
            for i in 0..number_of_interactions {
                // Verify that commitment to both the accumulator and archival data structure agree
                let new_commitment = accumulator.get_commitment();
                assert_eq!(
                    new_commitment,
                    archival_after_remove.get_commitment(),
                    "Commitment to archival/accumulator MS must agree"
                );
                match last_ms_commitment {
                    None => (),
                    Some(commitment) => assert_ne!(
                        commitment, new_commitment,
                        "MS commitment must change upon insertion/deletion"
                    ),
                };
                last_ms_commitment = Some(new_commitment);

                if prng.gen_range(0u8..2) == 0 || start_fill && i < number_of_interactions / 2 {
                    // Add a new item to the mutator set and update all membership proofs
                    let (item, randomness) = make_item_and_randomness_for_blake3();

                    let mut addition_record: AdditionRecord<H> =
                        accumulator.commit(&item, &randomness);
                    let membership_proof_acc = accumulator.prove(&item, &randomness, true);

                    // Update all membership proofs
                    // Uppdate membership proofs in batch
                    let previous_mps = membership_proofs_batch.clone();
                    let update_result = MsMembershipProof::batch_update_from_addition(
                        &mut membership_proofs_batch.iter_mut().collect::<Vec<_>>(),
                        &items,
                        &mut accumulator.set_commitment,
                        &addition_record,
                    );
                    assert!(update_result.is_ok(), "Batch mutation must return OK");

                    // Update membership proofs sequentially
                    for (mp, own_item) in membership_proofs_sequential.iter_mut().zip(items.iter())
                    {
                        let update_res_seq = mp.update_from_addition(
                            own_item,
                            &mut accumulator.set_commitment,
                            &addition_record,
                        );
                        assert!(update_res_seq.is_ok());
                    }

                    accumulator.add(&mut addition_record);
                    archival_after_remove.add(&mut addition_record);
                    archival_before_remove.add(&mut addition_record);

                    let updated_mp_indices = update_result.unwrap();
                    println!("{}: Inserted", i);
                    for j in 0..items.len() {
                        if updated_mp_indices.contains(&j) {
                            assert!(
                                !accumulator.verify(&items[j], &previous_mps[j]),
                                "Verify must fail for old proof, j = {}. AOCL data index was: {}.\n\nOld mp:\n {:?}.\n\nNew mp is\n {:?}",
                                j,
                                previous_mps[j].auth_path_aocl.data_index,
                                previous_mps[j],
                                membership_proofs_batch[j]
                            );
                        } else {
                            assert!(
                                accumulator.verify(&items[j], &previous_mps[j]),
                                "Verify must succeed for old proof, j = {}. AOCL data index was: {}.\n\nOld mp:\n {:?}.\n\nNew mp is\n {:?}",
                                j,
                                previous_mps[j].auth_path_aocl.data_index,
                                previous_mps[j],
                                membership_proofs_batch[j]
                            );
                        }
                    }

                    membership_proofs_batch.push(membership_proof_acc.clone());
                    membership_proofs_sequential.push(membership_proof_acc);
                    items.push(item);
                    rands.push(randomness);
                } else {
                    // Remove an item from the mutator set and update all membership proofs
                    if membership_proofs_batch.is_empty() {
                        // Set `last_ms_commitment` to None since it will otherwise be the
                        // same as in last iteration of this inner loop, and that will fail
                        // a test condition.
                        last_ms_commitment = None;
                        continue;
                    }

                    let item_index = prng.gen_range(0..membership_proofs_batch.len());
                    let removal_item = items.remove(item_index);
                    let removal_mp = membership_proofs_batch.remove(item_index);
                    let _removal_mp_seq = membership_proofs_sequential.remove(item_index);
                    let _removal_rand = rands.remove(item_index);

                    // generate removal record
                    let removal_record: RemovalRecord<H> =
                        accumulator.drop(&removal_item, &removal_mp);
                    assert!(removal_record.validate(&mut accumulator.set_commitment));

                    // update membership proofs
                    // Uppdate membership proofs in batch
                    let original_membership_proofs_batch = membership_proofs_batch.clone();
                    let batch_update_ret = MsMembershipProof::batch_update_from_remove(
                        &mut membership_proofs_batch.iter_mut().collect::<Vec<_>>(),
                        &removal_record,
                    );
                    assert!(batch_update_ret.is_ok());

                    // Update membership proofs sequentially
                    let original_membership_proofs_sequential =
                        membership_proofs_sequential.clone();
                    let mut update_by_remove_return_values: Vec<bool> = vec![];
                    for (_i, mp) in membership_proofs_sequential.iter_mut().enumerate() {
                        let update_res_seq = mp.update_from_remove(&removal_record);
                        assert!(update_res_seq.is_ok());
                        update_by_remove_return_values.push(update_res_seq.unwrap());
                    }

                    // remove item from set
                    assert!(accumulator.verify(&removal_item, &removal_mp));
                    let removal_record_copy = removal_record.clone();
                    accumulator.remove(&removal_record);
                    let diff_indices: Vec<u128> =
                        archival_after_remove.remove(&removal_record).unwrap();
                    for diff_index in diff_indices {
                        println!("diff_index = {}", diff_index);
                        assert!(archival_after_remove.get_bloom_filter_bit(diff_index));
                        assert!(!archival_before_remove.get_bloom_filter_bit(diff_index));
                    }
                    archival_before_remove.remove(&removal_record_copy);
                    assert!(!accumulator.verify(&removal_item, &removal_mp));

                    // Verify that the sequential `update_from_remove` return value is correct
                    // The return value from `update_from_remove` shows if the membership proof
                    // was updated or not.
                    for (i, ((updated, original_mp), item)) in update_by_remove_return_values
                        .into_iter()
                        .zip(original_membership_proofs_sequential.iter())
                        .zip(items.iter())
                        .enumerate()
                    {
                        if updated {
                            assert!(
                                !accumulator.verify(item, original_mp),
                                "i = {}, \n\nOriginal mp:\n{:?}\n\nNew mp:\n{:?}",
                                i,
                                original_mp,
                                membership_proofs_sequential[i]
                            );
                        } else {
                            assert!(
                                accumulator.verify(item, original_mp),
                                "i = {}, \n\nOriginal mp:\n{:?}\n\nNew mp:\n{:?}",
                                i,
                                original_mp,
                                membership_proofs_sequential[i]
                            );
                        }
                    }

                    // Verify that `batch_update_from_remove` return value is correct
                    // The return value indicates which membership proofs
                    let updated_indices: Vec<usize> = batch_update_ret.unwrap();
                    for (i, (original_mp, item)) in original_membership_proofs_batch
                        .iter()
                        .zip(items.iter())
                        .enumerate()
                    {
                        if updated_indices.contains(&i) {
                            assert!(!accumulator.verify(item, original_mp));
                        } else {
                            assert!(accumulator.verify(item, original_mp));
                        }
                    }

                    println!("{}: Removed", i);
                }

                // Verify that all membership proofs are valid after these additions and removals
                // Also verify that batch-update and sequential update of membership proofs agree.
                for (((mp_batch, mp_seq), item), rand) in membership_proofs_batch
                    .iter()
                    .zip(membership_proofs_sequential.iter())
                    .zip(items.iter())
                    .zip(rands.iter())
                {
                    assert!(accumulator.verify(item, mp_batch));

                    // Verify that the membership proof can be restored from an archival instance
                    let arch_mp = archival_after_remove
                        .restore_membership_proof(item, rand, mp_batch.auth_path_aocl.data_index)
                        .unwrap();
                    assert_eq!(arch_mp, *mp_batch);

                    // Also verify that cached bits are set for both proofs and that they agree
                    assert!(arch_mp.cached_bits.is_some());
                    assert_eq!(arch_mp.cached_bits, mp_batch.cached_bits);

                    // Verify that sequential and batch update produces the same membership proofs
                    assert_eq!(mp_batch, mp_seq);
                }
            }
        }
    }
}
