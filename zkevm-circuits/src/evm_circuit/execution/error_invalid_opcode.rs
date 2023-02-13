use crate::evm_circuit::execution::ExecutionGadget;
use crate::evm_circuit::step::ExecutionState;
use crate::evm_circuit::table::{FixedTableTag, Lookup};
use crate::evm_circuit::util::common_gadget::CommonErrorGadget;
use crate::evm_circuit::util::constraint_builder::ConstraintBuilder;
use crate::evm_circuit::util::{CachedRegion, Cell};
use crate::evm_circuit::witness::{Block, Call, ExecStep, Transaction};

use eth_types::Field;
use gadgets::util::Expr;
use halo2_proofs::circuit::Value;
use halo2_proofs::plonk::Error;

/// Gadget for invalid opcodes. It verifies by a fixed lookup for
/// ResponsibleOpcode.
#[derive(Clone, Debug)]
pub(crate) struct ErrorInvalidOpcodeGadget<F> {
    opcode: Cell<F>,
    common_error: CommonErrorGadget<F>,
}

impl<F: Field> ExecutionGadget<F> for ErrorInvalidOpcodeGadget<F> {
    const NAME: &'static str = "ErrorInvalidOpcode";

    const EXECUTION_STATE: ExecutionState = ExecutionState::ErrorInvalidOpcode;

    fn configure(cb: &mut ConstraintBuilder<F>) -> Self {
        let opcode = cb.query_cell();
        cb.opcode_lookup(opcode.expr(), 1.expr());
        cb.add_lookup(
            "Responsible opcode lookup",
            Lookup::Fixed {
                tag: FixedTableTag::ResponsibleOpcode.expr(),
                values: [
                    Self::EXECUTION_STATE.as_u64().expr(),
                    opcode.expr(),
                    0.expr(),
                ],
            },
        );

        let common_error = CommonErrorGadget::construct(cb, 2.expr());

        Self {
            opcode,
            common_error,
        }
    }

    fn assign_exec_step(
        &self,
        region: &mut CachedRegion<'_, '_, F>,
        offset: usize,
        block: &Block<F>,
        _: &Transaction,
        call: &Call,
        step: &ExecStep,
    ) -> Result<(), Error> {
        let opcode = F::from(step.opcode.unwrap().as_u64());
        self.opcode.assign(region, offset, Value::known(opcode))?;
        self.common_error
            .assign(region, offset, block, call, step, 2)
    }
}

#[cfg(test)]
mod test {
    use crate::evm_circuit::test::rand_bytes;
    use crate::test_util::CircuitTestBuilder;
    use eth_types::bytecode::Bytecode;
    use eth_types::{bytecode, ToWord, Word};
    use lazy_static::lazy_static;
    use mock::TestContext;

    lazy_static! {
        static ref TESTING_INVALID_CODES: [Vec<u8>; 6] = [
            // Single invalid opcode
            vec![0x0e],
            vec![0x4f],
            vec![0xa5],
            vec![0xf6],
            vec![0xfe],
            // Multiple invalid opcodes
            vec![0x5c, 0x5e, 0x5f],
        ];
    }

    #[test]
    fn invalid_opcode_root() {
        for invalid_code in TESTING_INVALID_CODES.iter() {
            test_root_ok(invalid_code);
        }
    }

    #[test]
    fn invalid_opcode_internal() {
        for invalid_code in TESTING_INVALID_CODES.iter() {
            test_internal_ok(0x20, 0x00, invalid_code);
        }
    }

    fn test_root_ok(invalid_code: &[u8]) {
        let mut code = Bytecode::default();
        invalid_code.iter().for_each(|b| {
            code.write(*b, true);
        });

        CircuitTestBuilder::new_from_test_ctx(
            TestContext::<2, 1>::simple_ctx_with_bytecode(code).unwrap(),
        )
        .run();
    }

    fn test_internal_ok(call_data_offset: usize, call_data_length: usize, invalid_code: &[u8]) {
        let (addr_a, addr_b) = (mock::MOCK_ACCOUNTS[0], mock::MOCK_ACCOUNTS[1]);

        // Code B gets called by code A, so the call is an internal call.
        let mut code_b = Bytecode::default();
        invalid_code.iter().for_each(|b| {
            code_b.write(*b, true);
        });

        // code A calls code B.
        let pushdata = rand_bytes(8);
        let code_a = bytecode! {
            // populate memory in A's context.
            PUSH8(Word::from_big_endian(&pushdata))
            PUSH1(0x00) // offset
            MSTORE
            // call ADDR_B.
            PUSH1(0x00) // retLength
            PUSH1(0x00) // retOffset
            PUSH32(call_data_length) // argsLength
            PUSH32(call_data_offset) // argsOffset
            PUSH1(0x00) // value
            PUSH32(addr_b.to_word()) // addr
            PUSH32(0x1_0000) // gas
            CALL
            STOP
        };

        let ctx = TestContext::<3, 1>::new(
            None,
            |accs| {
                accs[0].address(addr_b).code(code_b);
                accs[1].address(addr_a).code(code_a);
                accs[2]
                    .address(mock::MOCK_ACCOUNTS[3])
                    .balance(Word::from(1_u64 << 20));
            },
            |mut txs, accs| {
                txs[0].to(accs[1].address).from(accs[2].address);
            },
            |block, _tx| block,
        )
        .unwrap();

        CircuitTestBuilder::new_from_test_ctx(ctx).run();
    }
}
