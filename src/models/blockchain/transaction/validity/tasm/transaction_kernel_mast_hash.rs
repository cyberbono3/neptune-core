use itertools::Itertools;
use num_traits::One;
use tasm_lib::{
    hashing::hash_varlen::HashVarlen,
    list::unsafe_u32::{
        get::UnsafeGet, new::UnsafeNew, set::UnsafeSet, set_length::UnsafeSetLength,
    },
    rust_shadowing_helper_functions,
    snippet::{DataType, Snippet},
    structure::get_field_with_size::GetFieldWithSize,
    ExecutionState,
};
use triton_vm::BFieldElement;
use twenty_first::{
    shared_math::{tip5::Digest, tip5::DIGEST_LENGTH},
    util_types::{
        algebraic_hasher::AlgebraicHasher, merkle_tree::CpuParallel,
        merkle_tree_maker::MerkleTreeMaker,
    },
};

use crate::models::blockchain::shared::Hash;

/// Computes the mast hash of a transaction kernel object
#[derive(Debug, Clone)]
pub struct TransactionKernelMastHash;

impl Snippet for TransactionKernelMastHash {
    fn entrypoint(&self) -> String {
        "tasm_neptune_transaction_transaction_kernel_mast_hash".to_string()
    }
    fn function_code(&self, library: &mut tasm_lib::snippet_state::SnippetState) -> String {
        let entrypoint = self.entrypoint();
        let new_list = library.import(Box::new(UnsafeNew(DataType::Digest)));
        let get_element = library.import(Box::new(UnsafeGet(DataType::Digest)));
        let set_element = library.import(Box::new(UnsafeSet(DataType::Digest)));
        let set_length = library.import(Box::new(UnsafeSetLength(DataType::Digest)));

        let get_field_with_size = library.import(Box::new(GetFieldWithSize));

        let hash_varlen = library.import(Box::new(HashVarlen));

        format!(
            "
        // BEFORE: _ *kernel
        // AFTER: _ d4 d3 d2 d1 d0
        {entrypoint}:
            // allocate new list of 16 digests
            push 16                      // _ *kernel 16
            dup 0                        // _ *kernel 16 16
            call {new_list}              // _ *kernel 16 *list
            swap 1                       // _ *kernel *list 16
            call {set_length}            // _ *kernel *list

            // populate list[8] with inputs digest
            dup 1                       // _ *kernel *list *kernel
            push 0
            call {get_field_with_size}  // _ *kernel *list *inputs *inputs_size
            call {hash_varlen}          // _ *kernel *list d4 d3 d2 d1 d0
            dup 5 push 8                // _ *kernel *list d4 d3 d2 d1 d0 *list 8
            call {set_element}          // _ *kernel *list

            // populate list[9] with outputs digest
            dup 1                       // _ *kernel *list *kernel
            push 1
            call {get_field_with_size}  // _ *kernel *list *outputs *outputs_size
            call {hash_varlen}          // _ *kernel *list d4 d3 d2 d1 d0
            dup 5 push 9                // _ *kernel *list d4 d3 d2 d1 d0 *list 9
            call {set_element}          // _ *kernel *list

            // populate list[10] with pubscript_hashes_and_inputs digest
            dup 1                       // _ *kernel *list *kernel
            push 2
            call {get_field_with_size}  // _ *kernel *list *pubscript_hashes_and_inputs *pubscript_hashes_and_inputs_size_size
            call {hash_varlen}          // _ *kernel *list d4 d3 d2 d1 d0
            dup 5 push 10               // _ *kernel *list d4 d3 d2 d1 d0 *list 10
            call {set_element}          // _ *kernel *list

            // populate list[11] with fee digest
            dup 1                       // _ *kernel *list *kernel
            push 3
            call {get_field_with_size}  // _ *kernel *list *fee *fee_size
            call {hash_varlen}          // _ *kernel *list d4 d3 d2 d1 d0
            dup 5 push 11               // _ *kernel *list d4 d3 d2 d1 d0 *list 11
            call {set_element}          // _ *kernel *list

            // populate list[12] with coinbase digest
            dup 1                       // _ *kernel *list *kernel
            push 4
            call {get_field_with_size}  // _ *kernel *list *coinbase *coinbase_size
            call {hash_varlen}          // _ *kernel *list d4 d3 d2 d1 d0
            dup 5 push 12               // _ *kernel *list d4 d3 d2 d1 d0 *list 12
            call {set_element}          // _ *kernel *list

            // populate list[13] with timestamp digest
            dup 1                       // _ *kernel *list *kernel
            push 5
            call {get_field_with_size}  // _ *kernel *list *timestamp *timestamp_size
            call {hash_varlen}          // _ *kernel *list d4 d3 d2 d1 d0
            dup 5 push 13               // _ *kernel *list d4 d3 d2 d1 d0 *list 13
            call {set_element}          // _ *kernel *list

            // populate list[14] with mutator set hash digest
            dup 1                       // _ *kernel *list *kernel
            push 6
            call {get_field_with_size}  // _ *kernel *list *mutator_set_hash *mutator_set_hash_size
            call {hash_varlen}          // _ *kernel *list d4 d3 d2 d1 d0
            dup 5 push 14               // _ *kernel *list d4 d3 d2 d1 d0 *list 14
            call {set_element}          // _ *kernel *list

            // populate list[15] with default digest
            push 0 push 0 push 0 push 0 push 0
            dup 5 push 15               // _ *kernel *list d4 d3 d2 d1 d0 *list 15
            call {set_element}          // _ *kernel *list

            // hash 14||15 and store in 7
            dup 0 push 15               // _ *kernel *list *list 15
            call {get_element}          // _ *kernel *list d4 d3 d2 d1 d0
            dup 5 push 14               // _ *kernel *list d4 d3 d2 d1 d0 *list 14
            call {get_element}          // _ *kernel *list d4 d3 d2 d1 d0 e4 e3 e2 e1 e0
            hash                        // _ *kernel *list f4 f3 f2 f1 f0 0 0 0 0 0
            pop pop pop pop pop         // _ *kernel *list f4 f3 f2 f1 f0
            dup 5 push 7                // _ *kernel *list f4 f3 f2 f1 f0 *list 7
            call {set_element}

            // hash 12||13 and store in 6
            dup 0 push 13               // _ *kernel *list *list 13
            call {get_element}          // _ *kernel *list d4 d3 d2 d1 d0
            dup 5 push 12               // _ *kernel *list d4 d3 d2 d1 d0 *list 12
            call {get_element}          // _ *kernel *list d4 d3 d2 d1 d0 e4 e3 e2 e1 e0
            hash                        // _ *kernel *list f4 f3 f2 f1 f0 0 0 0 0 0
            pop pop pop pop pop         // _ *kernel *list f4 f3 f2 f1 f0
            dup 5 push 6                // _ *kernel *list f4 f3 f2 f1 f0 *list 6
            call {set_element}

            // hash 10||11 and store in 5
            dup 0 push 11               // _ *kernel *list *list 11
            call {get_element}          // _ *kernel *list d4 d3 d2 d1 d0
            dup 5 push 10               // _ *kernel *list d4 d3 d2 d1 d0 *list 10
            call {get_element}          // _ *kernel *list d4 d3 d2 d1 d0 e4 e3 e2 e1 e0
            hash                        // _ *kernel *list f4 f3 f2 f1 f0 0 0 0 0 0
            pop pop pop pop pop         // _ *kernel *list f4 f3 f2 f1 f0
            dup 5 push 5                // _ *kernel *list f4 f3 f2 f1 f0 *list 5
            call {set_element}

            // hash 8||9 and store in 4
            dup 0 push 9                // _ *kernel *list *list 9
            call {get_element}          // _ *kernel *list d4 d3 d2 d1 d0
            dup 5 push 8                // _ *kernel *list d4 d3 d2 d1 d0 *list 8
            call {get_element}          // _ *kernel *list d4 d3 d2 d1 d0 e4 e3 e2 e1 e0
            hash                        // _ *kernel *list f4 f3 f2 f1 f0 0 0 0 0 0
            pop pop pop pop pop         // _ *kernel *list f4 f3 f2 f1 f0
            dup 5 push 4                // _ *kernel *list f4 f3 f2 f1 f0 *list 4
            call {set_element}

            // hash 6||7 and store in 3
            dup 0 push 7                // _ *kernel *list *list 7
            call {get_element}          // _ *kernel *list d4 d3 d2 d1 d0
            dup 5 push 6                // _ *kernel *list d4 d3 d2 d1 d0 *list 6
            call {get_element}          // _ *kernel *list d4 d3 d2 d1 d0 e4 e3 e2 e1 e0
            hash                        // _ *kernel *list f4 f3 f2 f1 f0 0 0 0 0 0
            pop pop pop pop pop         // _ *kernel *list f4 f3 f2 f1 f0
            dup 5 push 3                // _ *kernel *list f4 f3 f2 f1 f0 *list 3
            call {set_element}

            // hash 4||5 and store in 2
            dup 0 push 5                // _ *kernel *list *list 5
            call {get_element}          // _ *kernel *list d4 d3 d2 d1 d0
            dup 5 push 4                // _ *kernel *list d4 d3 d2 d1 d0 *list 4
            call {get_element}          // _ *kernel *list d4 d3 d2 d1 d0 e4 e3 e2 e1 e0
            hash                        // _ *kernel *list f4 f3 f2 f1 f0 0 0 0 0 0
            pop pop pop pop pop         // _ *kernel *list f4 f3 f2 f1 f0
            dup 5 push 2                // _ *kernel *list f4 f3 f2 f1 f0 *list 2
            call {set_element}

            // hash 2||3 and store in 1
            dup 0 push 3                // _ *kernel *list *list 3
            call {get_element}          // _ *kernel *list d4 d3 d2 d1 d0
            dup 5 push 2                // _ *kernel *list d4 d3 d2 d1 d0 *list 2
            call {get_element}          // _ *kernel *list d4 d3 d2 d1 d0 e4 e3 e2 e1 e0
            hash                        // _ *kernel *list f4 f3 f2 f1 f0 0 0 0 0 0
            pop pop pop pop pop         // _ *kernel *list f4 f3 f2 f1 f0
            dup 5 push 1                // _ *kernel *list f4 f3 f2 f1 f0 *list 1

            call {set_element}

            // return list[1]
            swap 1                      // _ *list *kernel
            pop
            push 1 // _ *list 1
            call {get_element}          // _ d4 d3 d2 d1 d0

            return
            "
        )
    }

    fn rust_shadowing(
        &self,
        stack: &mut Vec<triton_vm::BFieldElement>,
        _std_in: Vec<triton_vm::BFieldElement>,
        _secret_in: Vec<triton_vm::BFieldElement>,
        memory: &mut std::collections::HashMap<triton_vm::BFieldElement, triton_vm::BFieldElement>,
    ) {
        // read address
        let mut address = stack.pop().unwrap();

        // inputs
        let inputs_size = memory.get(&address).unwrap().value() as usize;
        let inputs_encoded = (0..inputs_size)
            .map(|i| {
                *memory
                    .get(&(address + BFieldElement::new(1 + i as u64)))
                    .unwrap()
            })
            .collect_vec();
        let inputs_hash = Hash::hash_varlen(&inputs_encoded);
        address += BFieldElement::one() + BFieldElement::new(inputs_size as u64);

        // outputs
        let outputs_size = memory.get(&address).unwrap().value() as usize;
        let outputs_encoded = (0..outputs_size)
            .map(|i| {
                *memory
                    .get(&(address + BFieldElement::new(1 + i as u64)))
                    .unwrap()
            })
            .collect_vec();
        let outputs_hash = Hash::hash_varlen(&outputs_encoded);
        address += BFieldElement::one() + BFieldElement::new(outputs_size as u64);

        // pubscript_hashes_and_inputs
        let pubscript_hashes_and_inputs_size = memory.get(&address).unwrap().value() as usize;
        let pubscript_hashes_and_inputs_encoded = (0..pubscript_hashes_and_inputs_size)
            .map(|i| {
                *memory
                    .get(&(address + BFieldElement::new(1 + i as u64)))
                    .unwrap()
            })
            .collect_vec();
        let pubscript_hashes_and_inputs_hash =
            Hash::hash_varlen(&pubscript_hashes_and_inputs_encoded);
        address +=
            BFieldElement::one() + BFieldElement::new(pubscript_hashes_and_inputs_size as u64);

        // fee
        let fee_size = memory.get(&address).unwrap().value() as usize;
        let fee_encoded = (0..fee_size)
            .map(|i| {
                *memory
                    .get(&(address + BFieldElement::new(1 + i as u64)))
                    .unwrap()
            })
            .collect_vec();
        let fee_hash = Hash::hash_varlen(&fee_encoded);
        address += BFieldElement::one() + BFieldElement::new(fee_size as u64);

        // coinbase
        let coinbase_size = memory.get(&address).unwrap().value() as usize;
        let coinbase_encoded = (0..coinbase_size)
            .map(|i| {
                *memory
                    .get(&(address + BFieldElement::new(1 + i as u64)))
                    .unwrap()
            })
            .collect_vec();
        let coinbase_hash = Hash::hash_varlen(&coinbase_encoded);
        address += BFieldElement::one() + BFieldElement::new(coinbase_size as u64);

        // timestamp
        let timestamp_size = memory.get(&address).unwrap().value() as usize;
        assert_eq!(timestamp_size, 1);
        let timestamp_encoded = (0..timestamp_size)
            .map(|i| {
                *memory
                    .get(&(address + BFieldElement::new(1 + i as u64)))
                    .unwrap()
            })
            .collect_vec();
        let timestamp_hash = Hash::hash_varlen(&timestamp_encoded);
        address += BFieldElement::one() + BFieldElement::new(timestamp_size as u64);

        // mutator_set_hash
        let mutator_set_hash_size = memory.get(&address).unwrap().value() as usize;
        let mutator_set_hash_encoded = (0..mutator_set_hash_size)
            .map(|i| {
                *memory
                    .get(&(address + BFieldElement::new(1 + i as u64)))
                    .unwrap()
            })
            .collect_vec();
        let mutator_set_hash_hash = Hash::hash_varlen(&mutator_set_hash_encoded);
        address += BFieldElement::one() + BFieldElement::new(mutator_set_hash_size as u64);

        // padding
        let zero = Digest::default();

        // Merkleize
        let leafs = [
            inputs_hash,
            outputs_hash,
            pubscript_hashes_and_inputs_hash,
            fee_hash,
            coinbase_hash,
            timestamp_hash,
            mutator_set_hash_hash,
            zero,
        ];
        let tree = <CpuParallel as MerkleTreeMaker<Hash>>::from_digests(&leafs);
        let root = tree.get_root();

        // populate memory with merkle tree
        let list_address = rust_shadowing_helper_functions::dyn_malloc::dynamic_allocator(
            16 * DIGEST_LENGTH,
            memory,
        );
        rust_shadowing_helper_functions::unsafe_list::unsafe_list_new(list_address, memory);
        rust_shadowing_helper_functions::unsafe_list::unsafe_list_set_length(
            list_address,
            16,
            memory,
        );
        for (i, node) in tree.nodes.into_iter().enumerate().skip(1) {
            for j in 0..DIGEST_LENGTH {
                memory.insert(
                    list_address
                        + BFieldElement::one()
                        + BFieldElement::new((i * DIGEST_LENGTH + j) as u64),
                    node.values()[j],
                );
            }
        }

        // write digest to stack
        stack.push(root.values()[4]);
        stack.push(root.values()[3]);
        stack.push(root.values()[2]);
        stack.push(root.values()[1]);
        stack.push(root.values()[0]);
    }

    fn inputs(&self) -> Vec<String> {
        vec!["*transaction_kernel".to_string()]
    }

    fn input_types(&self) -> Vec<DataType> {
        vec![DataType::VoidPointer]
    }

    fn output_types(&self) -> Vec<DataType> {
        vec![DataType::Digest]
    }

    fn outputs(&self) -> Vec<String> {
        ["d4", "d3", "d2", "d1", "d0"]
            .map(|s| s.to_string())
            .to_vec()
    }

    fn stack_diff(&self) -> isize {
        4
    }

    fn crash_conditions(&self) -> Vec<String> {
        vec![]
    }

    fn gen_input_states(&self) -> Vec<ExecutionState> {
        #[cfg(test)]
        {
            vec![input_state_with_kernel_in_memory(
                BFieldElement::new(rand::Rng::gen_range(&mut rand::thread_rng(), 0..(1 << 20))),
                &twenty_first::shared_math::bfield_codec::BFieldCodec::encode(
                    &crate::tests::shared::random_transaction_kernel(),
                ),
            )]
        }
        #[cfg(not(test))]
        {
            panic!("`gen_input_states` cannot be called when not in testing environment")
        }
    }

    #[allow(unreachable_code)]
    fn common_case_input_state(&self) -> ExecutionState {
        #[cfg(test)]
        {
            let mut seed = [0u8; 32];
            seed[0] = 0xaa;
            seed[1] = 0xf1;
            seed[2] = 0xba;
            seed[3] = 0xd5;
            seed[4] = 0xee;
            seed[5] = 0xd5;
            let mut rng: rand::rngs::StdRng = rand::SeedableRng::from_seed(seed);
            return input_state_with_kernel_in_memory(
                BFieldElement::new(rand::Rng::gen_range(&mut rng, 0..(1 << 20))),
                &twenty_first::shared_math::bfield_codec::BFieldCodec::encode(
                    &crate::tests::shared::pseudorandom_transaction_kernel(
                        rand::Rng::gen::<[u8; 32]>(&mut rng),
                        2,
                        2,
                        0,
                    ),
                ),
            );
        }
        panic!("`common_case_input_state` cannot be called when not in testing environment")
    }

    #[allow(unreachable_code)]
    fn worst_case_input_state(&self) -> ExecutionState {
        #[cfg(test)]
        {
            let mut seed = [0u8; 32];
            seed[0] = 0xaa;
            seed[1] = 0xf2;
            seed[2] = 0xba;
            seed[3] = 0xd5;
            seed[4] = 0xee;
            seed[5] = 0xd5;
            let mut rng: rand::rngs::StdRng = rand::SeedableRng::from_seed(seed);
            return input_state_with_kernel_in_memory(
                BFieldElement::new(rand::Rng::gen_range(&mut rng, 0..(1 << 20))),
                &twenty_first::shared_math::bfield_codec::BFieldCodec::encode(
                    &crate::tests::shared::pseudorandom_transaction_kernel(
                        rand::Rng::gen::<[u8; 32]>(&mut rng),
                        4,
                        4,
                        2,
                    ),
                ),
            );
        }
        panic!("`worst_case_input_state` cannot be called when not in testing environment")
    }
}

#[cfg(test)]
fn input_state_with_kernel_in_memory(
    address: BFieldElement,
    transaction_kernel_encoded: &[BFieldElement],
) -> ExecutionState {
    // populate memory
    let mut memory: std::collections::HashMap<BFieldElement, BFieldElement> =
        std::collections::HashMap::new();
    for (i, t) in transaction_kernel_encoded.iter().enumerate() {
        memory.insert(address + BFieldElement::new(i as u64), *t);
    }

    // set dynamic allocator
    memory.insert(
        <BFieldElement as num_traits::Zero>::zero(),
        BFieldElement::new(transaction_kernel_encoded.len() as u64) + address,
    );

    let mut stack = tasm_lib::get_init_tvm_stack();
    stack.push(address);
    ExecutionState {
        stack,
        std_in: vec![],
        secret_in: vec![],
        memory,
        words_allocated: 0,
    }
}

#[cfg(test)]
mod tests {
    use rand::{rngs::StdRng, Rng, SeedableRng};
    use tasm_lib::test_helpers::{
        test_rust_equivalence_given_execution_state, test_rust_equivalence_multiple,
    };
    use twenty_first::shared_math::bfield_codec::BFieldCodec;

    use crate::tests::shared::pseudorandom_transaction_kernel;

    use super::*;

    #[test]
    fn verify_agreement_with_tx_kernel_mast_hash() {
        let mut seed = [99u8; 32];
        seed[17] = 0x17;
        let mut rng: StdRng = SeedableRng::from_seed(seed);
        let tx_kernel = pseudorandom_transaction_kernel(rng.gen(), 2, 2, 1);
        let mut output_with_known_digest = test_rust_equivalence_given_execution_state(
            &TransactionKernelMastHash,
            input_state_with_kernel_in_memory(BFieldElement::one(), &tx_kernel.encode()),
        );

        // read the digest from the very short TX kernel
        let d0 = output_with_known_digest.final_stack.pop().unwrap();
        let d1 = output_with_known_digest.final_stack.pop().unwrap();
        let d2 = output_with_known_digest.final_stack.pop().unwrap();
        let d3 = output_with_known_digest.final_stack.pop().unwrap();
        let d4 = output_with_known_digest.final_stack.pop().unwrap();
        let mast_hash_from_vm = Digest::new([d0, d1, d2, d3, d4]);

        // Verify agreement with mast_hash method on tx kernel
        assert_eq!(tx_kernel.mast_hash(), mast_hash_from_vm);
    }

    #[test]
    fn new_prop_test() {
        test_rust_equivalence_multiple(&TransactionKernelMastHash, true);
    }
}

#[cfg(test)]
mod benches {
    use tasm_lib::snippet_bencher::bench_and_write;

    use super::*;

    #[test]
    fn get_transaction_kernel_field_benchmark() {
        bench_and_write(TransactionKernelMastHash)
    }
}
