use std::time::Instant;

use crate::verifier::FullVerifierKey;
use halo2_proofs::{
    dev::MockProver,
    halo2curves::bn256::{Bn256, Fr, G1Affine},
    plonk::{create_proof, keygen_pk, keygen_vk, VerifyingKey},
    poly::{
        commitment::ParamsProver,
        kzg::{
            commitment::{KZGCommitmentScheme, ParamsKZG, ParamsVerifierKZG},
            multiopen::ProverSHPLONK,
        },
    },
    transcript::{Blake2bWrite, Challenge255, TranscriptWriterBuffer},
};
use rand::SeedableRng;
use rand_chacha::ChaCha20Rng;

use super::{circuit::InitialCircuitHelper, InitialStateCircuit};
use eyre::{eyre, Result};

impl InitialStateCircuit<Fr> {
    pub fn assert_satisfied(&self) {
        let num_rows: usize = self
            .mpt_circuit
            .nodes
            .iter()
            .map(|node| node.values.len())
            .sum();

        let hash = self.lc_witness.initial_values_hash();
        let prover =
            MockProver::<Fr>::run(self.degree as u32, self, vec![vec![hash.lo(), hash.hi()]])
                .unwrap();
        prover.assert_satisfied_at_rows_par(0..num_rows, 0..num_rows);
    }
    pub fn gen_pk_and_prove(self) -> Result<(FullVerifierKey, Vec<u8>, Vec<Fr>)> {
        let mut rng = ChaCha20Rng::seed_from_u64(42);

        let start = Instant::now();

        let general_params = ParamsKZG::<Bn256>::setup(self.degree as u32, &mut rng);
        let verifier_params: ParamsVerifierKZG<Bn256> = general_params.verifier_params().clone();

        // Initialize the proving key
        let vk: VerifyingKey<G1Affine> =
            keygen_vk(&general_params, &self).expect("keygen_vk should not fail");
        let pk = keygen_pk(&general_params, vk.clone(), &self).expect("keygen_pk should not fail");

        println!("key generation time: {:?}", start.elapsed());

        let rng = ChaCha20Rng::seed_from_u64(42);

        // Create a proof
        let mut transcript = Blake2bWrite::<_, G1Affine, Challenge255<_>>::init(vec![]);

        let hash = self.lc_witness.initial_values_hash();
        let public_inputs = vec![hash.lo(), hash.hi()];

        // Bench proof generation time
        let start = Instant::now();
        create_proof::<
            KZGCommitmentScheme<Bn256>,
            ProverSHPLONK<'_, Bn256>,
            Challenge255<G1Affine>,
            ChaCha20Rng,
            Blake2bWrite<Vec<u8>, G1Affine, Challenge255<G1Affine>>,
            InitialStateCircuit<Fr>,
        >(
            &general_params,
            &pk,
            &[self],
            &[&[&public_inputs]],
            rng,
            &mut transcript,
        )?;

        let proof = transcript.finalize();
        println!("proof generation time: {:?}", start.elapsed());

        Ok((
            FullVerifierKey {
                verifier_params,
                vk,
            },
            proof,
            public_inputs,
        ))
    }

    // Sense keccak: Proof: 147k
    // Amb keccak: Proof: 250k

    pub fn assert_real_prover(self) -> Result<(FullVerifierKey, Vec<u8>, Vec<Fr>)> {
        let (fk, proof, pi) = self.gen_pk_and_prove()?;

        let result = crate::verifier::verify(&fk, &proof, &pi)?;
        if !result {
            return Err(eyre!("proof verification failed"));
        }
        Ok((fk, proof, pi))
    }
}