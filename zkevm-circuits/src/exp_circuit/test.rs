use crate::{
    evm_circuit::witness::{block_convert, Block},
    exp_circuit::ExpCircuit,
    util::{unusable_rows, SubCircuit},
    witness::{chunk_convert, Chunk},
};
use bus_mapping::{
    circuit_input_builder::{CircuitInputBuilder, FixedCParams},
    mock::BlockData,
};
use eth_types::{bytecode, geth_types::GethData, Bytecode, Field, Word};
use halo2_proofs::{dev::MockProver, halo2curves::bn256::Fr};
use mock::TestContext;

#[test]
fn exp_circuit_unusable_rows() {
    assert_eq!(
        ExpCircuit::<Fr>::unusable_rows(),
        unusable_rows::<Fr, ExpCircuit::<Fr>>(()),
    )
}

/// Test exponentiation circuit with the provided block witness
pub fn test_exp_circuit<F: Field>(k: u32, block: Block<F>, chunk: Chunk<F>) {
    let circuit = ExpCircuit::<F>::new(block.exp_events, chunk.fixed_param.max_exp_steps);
    let prover = MockProver::<F>::run(k, &circuit, vec![]).unwrap();
    prover.assert_satisfied_par()
}

fn gen_code_single(base: Word, exponent: Word) -> Bytecode {
    bytecode! {
        PUSH32(exponent)
        PUSH32(base)
        EXP
        STOP
    }
}

fn gen_code_multiple(args: Vec<(Word, Word)>) -> Bytecode {
    let mut code = Bytecode::default();
    for (base, exponent) in args.into_iter() {
        code.op_exp(base, exponent);
    }
    code.op_stop();
    code
}

fn gen_data(code: Bytecode, default_params: bool) -> CircuitInputBuilder<FixedCParams> {
    let test_ctx = TestContext::<2, 1>::simple_ctx_with_bytecode(code).unwrap();
    let block: GethData = test_ctx.into();
    // Needs default parameters for variadic size test

    if default_params {
        BlockData::new_from_geth_data_with_params(block.clone(), FixedCParams::default())
            .new_circuit_input_builder()
            .handle_block(&block.eth_block, &block.geth_traces)
            .unwrap()
    } else {
        BlockData::new_from_geth_data(block.clone())
            .new_circuit_input_builder()
            .handle_block(&block.eth_block, &block.geth_traces)
            .unwrap()
    }
}

fn test_ok(base: Word, exponent: Word, k: Option<u32>) {
    let code = gen_code_single(base, exponent);
    let builder = gen_data(code, false);
    let block = block_convert::<Fr>(&builder).unwrap();
    let chunk = chunk_convert::<Fr>(&builder, 0).unwrap();
    test_exp_circuit(k.unwrap_or(18), block, chunk);
}

fn test_ok_multiple(args: Vec<(Word, Word)>) {
    let code = gen_code_multiple(args);
    let builder = gen_data(code, false);
    let block = block_convert::<Fr>(&builder).unwrap();
    let chunk = chunk_convert::<Fr>(&builder, 0).unwrap();
    test_exp_circuit(20, block, chunk);
}

#[test]
fn exp_circuit_single() {
    test_ok(2.into(), 2.into(), None);
    test_ok(3.into(), 7.into(), None);
    test_ok(5.into(), 11.into(), None);
    test_ok(7.into(), 13.into(), None);
    test_ok(11.into(), 17.into(), None);
    test_ok(13.into(), 23.into(), None);
    test_ok(29.into(), 43.into(), None);
    test_ok(41.into(), 259.into(), None);
}

#[test]
fn exp_circuit_big() {
    test_ok(
        2.into(),
        Word::from_str_radix("0x1FFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFE", 16).unwrap(),
        Some(20),
    );
}

#[test]
fn exp_circuit_multiple() {
    test_ok_multiple(vec![
        (3.into(), 7.into()),
        (5.into(), 11.into()),
        (7.into(), 13.into()),
        (11.into(), 17.into()),
        (13.into(), 23.into()),
        (29.into(), 43.into()),
        (41.into(), 259.into()),
    ]);
}

#[test]
fn variadic_size_check() {
    let k = 13;
    // Empty
    let block: GethData = TestContext::<0, 0>::new(None, |_| {}, |_, _| {}, |b, _| b)
        .unwrap()
        .into();
    let builder = BlockData::new_from_geth_data_with_params(block.clone(), FixedCParams::default())
        .new_circuit_input_builder()
        .handle_block(&block.eth_block, &block.geth_traces)
        .unwrap();
    let block = block_convert::<Fr>(&builder).unwrap();
    let chunk = chunk_convert::<Fr>(&builder, 0).unwrap();
    let circuit = ExpCircuit::<Fr>::new(block.exp_events, chunk.fixed_param.max_exp_steps);
    let prover1 = MockProver::<Fr>::run(k, &circuit, vec![]).unwrap();

    // Non-empty
    let code = bytecode! {
        PUSH32(8)
        PUSH32(10)
        EXP
        PUSH32(3)
        PUSH32(5)
        EXP
        STOP
    };
    let builder = gen_data(code, true);
    let block = block_convert::<Fr>(&builder).unwrap();
    let chunk = chunk_convert::<Fr>(&builder, 0).unwrap();
    let circuit = ExpCircuit::<Fr>::new(block.exp_events, chunk.fixed_param.max_exp_steps);
    let prover2 = MockProver::<Fr>::run(k, &circuit, vec![]).unwrap();

    assert_eq!(prover1.fixed(), prover2.fixed());
    assert_eq!(prover1.permutation(), prover2.permutation());
}
