use get_size::GetSize;
use itertools::Itertools;
use serde::Deserialize;
use serde::Serialize;
use tasm_lib::memory::encode_to_memory;
use tasm_lib::memory::FIRST_NON_DETERMINISTICALLY_INITIALIZED_MEMORY_ADDRESS;
use tasm_lib::prelude::TasmObject;
use tasm_lib::triton_vm;
use tasm_lib::triton_vm::prelude::BFieldCodec;
use tasm_lib::triton_vm::prelude::BFieldElement;
use tasm_lib::triton_vm::prelude::Program;
use tasm_lib::triton_vm::proof::Claim;
use tasm_lib::triton_vm::proof::Proof;
use tasm_lib::triton_vm::stark::Stark;
use tasm_lib::triton_vm::vm::NonDeterminism;
use tasm_lib::triton_vm::vm::PublicInput;
use tasm_lib::verifier::stark_verify::StarkVerify;
use tasm_lib::Digest;
use tokio::sync::TryLockError;

use super::block_primitive_witness::BlockPrimitiveWitness;
use super::block_program::BlockProgram;
use crate::models::blockchain::block::block_body::BlockBody;
use crate::models::blockchain::block::BlockAppendix;
use crate::models::blockchain::transaction::validity::single_proof::SingleProof;
use crate::models::blockchain::transaction::TransactionProof;
use crate::models::proof_abstractions::mast_hash::MastHash;
use crate::models::proof_abstractions::tasm::program::ConsensusProgram;
use crate::models::proof_abstractions::tasm::program::TritonProverSync;
use crate::models::proof_abstractions::SecretWitness;

/// All information necessary to efficiently produce a proof for a block.
///
/// This is the witness for the [`BlockProgram`].
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq, GetSize, BFieldCodec, TasmObject)]
pub(crate) struct AppendixWitness {
    block_body_hash: Digest,
    pub(crate) claims: Vec<Claim>,
    pub(crate) proofs: Vec<Proof>,
}

impl AppendixWitness {
    fn new(block_body: &BlockBody) -> Self {
        Self {
            block_body_hash: block_body.mast_hash(),
            claims: Vec::default(),
            proofs: Vec::default(),
        }
    }

    fn with_claim(mut self, claim: Claim, proof: Proof) -> Self {
        assert!(triton_vm::verify(Stark::default(), &claim, &proof));
        self.claims.push(claim);
        self.proofs.push(proof);

        self
    }

    pub(crate) fn claims(&self) -> Vec<Claim> {
        self.claims.clone()
    }

    pub(crate) fn appendix(&self) -> BlockAppendix {
        BlockAppendix::new(self.claims())
    }

    pub(crate) async fn produce(
        block_primitive_witness: BlockPrimitiveWitness,
        _sync_device: &TritonProverSync,
    ) -> Result<AppendixWitness, TryLockError> {
        let txk_mast_hash = block_primitive_witness
            .body()
            .transaction_kernel
            .mast_hash();

        let tx_is_valid_claim = SingleProof::claim(txk_mast_hash);
        let tx_is_valid_proof = match &block_primitive_witness.transaction.proof {
            TransactionProof::SingleProof(proof) => proof.clone(),
            _ => {
                panic!(
                    "can only produce appendix witness from single-proof transaction; got {:?}",
                    block_primitive_witness.transaction.proof
                );
            }
        };

        // Add more claim/proof pairs here, when softforking.
        let ret = Self::new(block_primitive_witness.body())
            .with_claim(tx_is_valid_claim, tx_is_valid_proof);

        assert_eq!(
            BlockAppendix::consensus_claims(block_primitive_witness.body()),
            ret.claims,
            "appendix witness must attest to expected claims"
        );

        Ok(ret)
    }
}

impl SecretWitness for AppendixWitness {
    fn standard_input(&self) -> PublicInput {
        self.block_body_hash.reversed().values().into()
    }

    fn output(&self) -> Vec<BFieldElement> {
        self.claims().encode()
    }

    fn program(&self) -> Program {
        Program::new(&BlockProgram.code())
    }

    fn nondeterminism(&self) -> NonDeterminism {
        let mut nondeterminism = NonDeterminism::new([]);
        encode_to_memory(
            &mut nondeterminism.ram,
            FIRST_NON_DETERMINISTICALLY_INITIALIZED_MEMORY_ADDRESS,
            self,
        );
        let stark_snippet = StarkVerify::new_with_dynamic_layout(Stark::default());
        for (claim, proof) in self.claims.iter().zip_eq(&self.proofs) {
            stark_snippet.update_nondeterminism(&mut nondeterminism, proof, claim);
        }
        nondeterminism
    }
}
