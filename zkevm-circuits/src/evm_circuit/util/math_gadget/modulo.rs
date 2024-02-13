use crate::{
    evm_circuit::util::{
        self, constraint_builder::ConstraintBuilder, math_gadget::*, sum, CachedRegion,
    },
    util::Expr,
};
use eth_types::{Field, ToLittleEndian, Word};
use halo2_proofs::plonk::Error;

/// Constraints for the words a, n, r:
/// a mod n = r, if n!=0
/// r = 0,       if n==0
///
/// We use the auxiliary a_or_zero word, whose value is constrained to be:
/// a_or_zero = a if n!=0, 0 if n==0.  This allows to use the equation
/// k * n + r = a_or_zero to verify the modulus, which holds with r=0 in the
/// case of n=0. Unlike the usual k * n + r = a, which forces r = a when n=0,
/// this equation assures that r<n or r=n=0.
#[derive(Clone, Debug)]
pub struct ModGadget<F> {
    k: util::Word<F>,
    a_or_zero: util::Word<F>,
    mul: MulAddWordsGadget<F>,
    n_is_zero: IsZeroGadget<F>,
    a_or_is_zero: IsZeroGadget<F>,
    eq: IsEqualGadget<F>,
    lt: LtWordGadget<F>,
}
impl<F: Field> ModGadget<F> {
    pub fn construct(cb: &mut ConstraintBuilder<F>, words: [&util::Word<F>; 3]) -> Self {
        let (a, n, r) = (words[0], words[1], words[2]);
        let k = cb.query_word();
        let a_or_zero = cb.query_word();
        let n_is_zero = IsZeroGadget::construct(cb, sum::expr(&n.cells));
        let a_or_is_zero = IsZeroGadget::construct(cb, sum::expr(&a_or_zero.cells));
        let mul = MulAddWordsGadget::construct(cb, [&k, n, r, &a_or_zero]);
        let eq = IsEqualGadget::construct(cb, a.expr(), a_or_zero.expr());
        let lt = LtWordGadget::construct(cb, r, n);
        // Constraint the aux variable a_or_zero to be =a or =0 if n==0:
        // (a == a_zero) ^ (n == 0 & a_or_zero == 0)
        cb.add_constraint(
            " (1 - (a == a_or_zero)) * ( 1 - (n == 0) * (a_or_zero == 0)",
            (1.expr() - eq.expr()) * (1.expr() - n_is_zero.expr() * a_or_is_zero.expr()),
        );

        // Constrain the result r to be valid: (r<n) ^ n==0
        cb.add_constraint(
            " (1 - (r<n) - (n==0) ",
            1.expr() - lt.expr() - n_is_zero.expr(),
        );

        Self {
            k,
            a_or_zero,
            mul,
            n_is_zero,
            a_or_is_zero,
            eq,
            lt,
        }
    }

    pub fn assign(
        &self,
        region: &mut CachedRegion<'_, '_, F>,
        offset: usize,
        a: Word,
        n: Word,
        r: Word,
        randomness: F,
    ) -> Result<(), Error> {
        let k = if n.is_zero() { Word::zero() } else { a / n };
        let a_or_zero = if n.is_zero() { Word::zero() } else { a };

        self.k.assign(region, offset, Some(k.to_le_bytes()))?;
        self.a_or_zero
            .assign(region, offset, Some(a_or_zero.to_le_bytes()))?;
        let n_sum = (0..32).fold(0, |acc, idx| acc + n.byte(idx) as u64);
        let a_or_zero_sum = (0..32).fold(0, |acc, idx| acc + a_or_zero.byte(idx) as u64);
        self.n_is_zero.assign(region, offset, F::from(n_sum))?;
        self.a_or_is_zero
            .assign(region, offset, F::from(a_or_zero_sum))?;
        self.mul.assign(region, offset, [k, n, r, a_or_zero])?;
        self.lt.assign(region, offset, r, n)?;
        self.eq.assign(
            region,
            offset,
            util::Word::random_linear_combine(a.to_le_bytes(), randomness),
            util::Word::random_linear_combine(a_or_zero.to_le_bytes(), randomness),
        )?;

        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::test_util::*;
    use super::*;
    use eth_types::Word;
    use halo2_proofs::halo2curves::bn256::Fr;
    use halo2_proofs::plonk::Error;

    #[derive(Clone)]
    /// ModGadgetTestContainer: require(a % n == r)
    struct ModGadgetTestContainer<F> {
        mod_gadget: ModGadget<F>,
        a: util::Word<F>,
        n: util::Word<F>,
        r: util::Word<F>,
    }

    impl<F: Field> MathGadgetContainer<F> for ModGadgetTestContainer<F> {
        fn configure_gadget_container(cb: &mut ConstraintBuilder<F>) -> Self {
            let a = cb.query_word();
            let n = cb.query_word();
            let r = cb.query_word();
            let mod_gadget = ModGadget::<F>::construct(cb, [&a, &n, &r]);
            ModGadgetTestContainer {
                mod_gadget,
                a,
                n,
                r,
            }
        }

        fn assign_gadget_container(
            &self,
            witnesses: &[Word],
            region: &mut CachedRegion<'_, '_, F>,
        ) -> Result<(), Error> {
            let a = witnesses[0];
            let n = witnesses[1];
            let a_reduced = witnesses[2];
            let offset = 0;

            self.a.assign(region, offset, Some(a.to_le_bytes()))?;
            self.n.assign(region, offset, Some(n.to_le_bytes()))?;
            self.r
                .assign(region, offset, Some(a_reduced.to_le_bytes()))?;
            self.mod_gadget.assign(region, 0, a, n, a_reduced, F::one())
        }
    }

    #[test]
    fn test_mod_n_expected_rem() {
        try_test!(
            ModGadgetTestContainer<Fr>,
            vec![Word::from(0), Word::from(0), Word::from(0)],
            true,
        );
        try_test!(
            ModGadgetTestContainer<Fr>,
            vec![Word::from(1), Word::from(0), Word::from(0)],
            true,
        );
        try_test!(
            ModGadgetTestContainer<Fr>,
            vec![Word::from(548), Word::from(50), Word::from(48)],
            true,
        );
        try_test!(
            ModGadgetTestContainer<Fr>,
            vec![Word::from(30), Word::from(50), Word::from(30)],
            true,
        );
        try_test!(
            ModGadgetTestContainer<Fr>,
            vec![WORD_LOW_MAX, Word::from(1024), Word::from(1023)],
            true,
        );
        try_test!(
            ModGadgetTestContainer<Fr>,
            vec![WORD_HIGH_MAX, Word::from(1024), Word::from(0)],
            true,
        );
        try_test!(
            ModGadgetTestContainer<Fr>,
            vec![WORD_CELL_MAX, Word::from(2), Word::from(0)],
            true,
        );
    }

    #[test]
    fn test_mod_n_unexpected_rem() {
        try_test!(
            ModGadgetTestContainer<Fr>,
            vec![Word::from(1), Word::from(1), Word::from(1)],
            false,
        );
        try_test!(
            ModGadgetTestContainer<Fr>,
            vec![Word::from(46), Word::from(50), Word::from(48)],
            false,
        );
        try_test!(
            ModGadgetTestContainer<Fr>,
            vec![WORD_LOW_MAX, Word::from(999999), Word::from(888888)],
            false,
        );
        try_test!(
            ModGadgetTestContainer<Fr>,
            vec![WORD_CELL_MAX, Word::from(999999999), Word::from(666666666)],
            false,
        );
        try_test!(
            ModGadgetTestContainer<Fr>,
            vec![WORD_HIGH_MAX, Word::from(999999), Word::from(777777)],
            false,
        );
    }
}
