use halo2_proofs::{
    circuit::Chip,
    plonk::{Advice, Column, ConstraintSystem, Expression, Fixed, VirtualCells},
    poly::Rotation,
};
use pairing::arithmetic::FieldExt;
use std::marker::PhantomData;

use crate::{
    helpers::{compute_rlc, get_bool_constraint, key_len_lookup, mult_diff_lookup, range_lookups},
    mpt::{FixedTableTag},
    param::{
        BRANCH_ROWS_NUM, IS_BRANCH_C16_POS, IS_BRANCH_C1_POS, RLP_NUM,
        R_TABLE_LEN, HASH_WIDTH, IS_BRANCH_S_PLACEHOLDER_POS, IS_BRANCH_C_PLACEHOLDER_POS, NIBBLES_COUNTER_POS,
    }, columns::{MainCols, AccumulatorCols, DenoteCols},
};

#[derive(Clone, Debug)]
pub(crate) struct LeafKeyConfig {}

// Verifies the RLC of leaf RLP: RLP meta data & key (value and then hash of
// the whole RLC are checked in leaf_value).
// Verifies RLC of a leaf key - used for a check from outside the circuit to
// verify that the proper key is used.
pub(crate) struct LeafKeyChip<F> {
    config: LeafKeyConfig,
    _marker: PhantomData<F>,
}

impl<F: FieldExt> LeafKeyChip<F> {
    pub fn configure(
        meta: &mut ConstraintSystem<F>,
        q_enable: impl Fn(&mut VirtualCells<'_, F>) -> Expression<F> + Copy,
        s_main: MainCols<F>,
        c_main: MainCols<F>,
        accs: AccumulatorCols<F>,
        denoter: DenoteCols<F>, // sel1 stores key_rlc_prev, sel2 stores key_rlc_mult_prev
        is_account_leaf_in_added_branch: Column<Advice>,
        r_table: Vec<Expression<F>>,
        fixed_table: [Column<Fixed>; 3],
        is_s: bool,
    ) -> LeafKeyConfig {
        let config = LeafKeyConfig {};
        let one = Expression::Constant(F::one());
        let c32 = Expression::Constant(F::from(32));
        let c48 = Expression::Constant(F::from(48));
        let c64 = Expression::Constant(F::from(64));
        let c128 = Expression::Constant(F::from(128));

        let mut rot_into_init = -19;
        let mut rot_into_account = -1;
        if !is_s {
            rot_into_init = -21;
            rot_into_account = -3;
        }

        // TODO: if key is of length 1, then there is one less byte in RLP meta data
        // (this is easier seen in extension nodes, it will probably be difficult
        // to generate such test for normal ShortNode)

        // Checking leaf RLC is ok - this value is then taken in the next row, where
        // leaf value is added to RLC, finally lookup is used to check the hash that
        // corresponds to this RLC is in the parent branch.
        meta.create_gate("Storage leaf RLC", |meta| {
            let q_enable = q_enable(meta);
            let mut constraints = vec![];

            let c248 = Expression::Constant(F::from(248));
            let s_rlp1 = meta.query_advice(s_main.rlp1, Rotation::cur());
            let s_rlp2 = meta.query_advice(s_main.rlp2, Rotation::cur());
            let flag1 = meta.query_advice(accs.s_mod_node_rlc, Rotation::cur());
            let flag2 = meta.query_advice(accs.c_mod_node_rlc, Rotation::cur());

            let last_level = flag1.clone() * flag2.clone();
            let is_long = flag1.clone() * (one.clone() - flag2.clone());
            let is_short = (one.clone() - flag1.clone()) * flag2.clone();
            let one_nibble = (one.clone() - flag1.clone()) * (one.clone() - flag2.clone());

            constraints.push((
                "is_long: s_rlp1 = 248",
                q_enable.clone() * is_long.clone() * (s_rlp1.clone() - c248),
            )); 
            constraints.push((
                "last_level: s_rlp2 = 32",
                q_enable.clone() * last_level.clone() * (s_rlp2.clone() - c32.clone()),
            ));
            constraints.push((
                "flag1 is boolean",
                get_bool_constraint(q_enable.clone(), flag1.clone()),
            ));
            constraints.push((
                "flag2 is boolean",
                get_bool_constraint(q_enable.clone(), flag2.clone()),
            ));

            // If leaf in last level, it contains only s_rlp1 and s_rlp2, while s_main.bytes are 0.
            let rlc_last_level_or_one_nibble = s_rlp1 + s_rlp2 * r_table[0].clone();

            let mut rlc = rlc_last_level_or_one_nibble.clone()
                + compute_rlc(meta, s_main.bytes.to_vec(), 1, one.clone(), 0, r_table.clone());

            let c_rlp1 = meta.query_advice(c_main.rlp1, Rotation::cur());
            // c_rlp2 can appear if long and if no branch above leaf
            let c_rlp2 = meta.query_advice(c_main.rlp2, Rotation::cur());
            rlc = rlc + c_rlp1 * r_table[R_TABLE_LEN - 1].clone() * r_table[1].clone();
            rlc = rlc + c_rlp2 * r_table[R_TABLE_LEN - 1].clone() * r_table[2].clone();

            let acc = meta.query_advice(accs.acc_s.rlc, Rotation::cur());
            constraints.push(("Leaf key acc",
                q_enable.clone()
                * (is_short + is_long) // activate if is_short or is_long
                * (rlc - acc.clone())));
            
            /*
            `last_level` and `one_nibble` cases have one RLP byte (`s_rlp1`) and one byte (`s_rlp2`)
            where it is 32 (for `last_level`) or `48 + last_nibble` (for `one_nibble`).
            */
            constraints.push(("Leaf key acc last level or one nibble",
                q_enable
                * (last_level + one_nibble)
                * (rlc_last_level_or_one_nibble - acc)));

            constraints
        });

        let sel_short = |meta: &mut VirtualCells<F>| {
            let q_enable = q_enable(meta);
            let flag1 = meta.query_advice(accs.s_mod_node_rlc, Rotation::cur());
            let flag2 = meta.query_advice(accs.c_mod_node_rlc, Rotation::cur());
            let is_short = (one.clone() - flag1.clone()) * flag2.clone();

            q_enable * is_short
        };
        let sel_long = |meta: &mut VirtualCells<F>| {
            let q_enable = q_enable(meta);
            let flag1 = meta.query_advice(accs.s_mod_node_rlc, Rotation::cur());
            let flag2 = meta.query_advice(accs.c_mod_node_rlc, Rotation::cur());
            let is_long = flag1.clone() * (one.clone() - flag2.clone());

            q_enable * is_long
        };

        /*
        There are 0s after key length (this doesn't need to be checked for last_level as
        in this case s_main.bytes are not used).
        for ind in 0..HASH_WIDTH {
            key_len_lookup(
                meta,
                sel_short,
                ind + 1,
                s_main.rlp2,
                s_main.bytes[ind],
                128,
                fixed_table,
            )
        }
        key_len_lookup(meta, sel_short, 32, s_main.rlp2, c_main.rlp1, 128, fixed_table);

        for ind in 1..HASH_WIDTH {
            key_len_lookup(
                meta,
                sel_long,
                ind,
                s_main.bytes[0],
                s_main.bytes[ind],
                128,
                fixed_table,
            )
        }
        key_len_lookup(meta, sel_long, 32, s_main.bytes[0], c_main.rlp1, 128, fixed_table);
        key_len_lookup(meta, sel_long, 33, s_main.bytes[0], c_main.rlp2, 128, fixed_table);
        */

        // acc_mult corresponds to key length (short):
        mult_diff_lookup(meta, sel_short, 2, s_main.rlp2, accs.acc_s.mult, 128, fixed_table);
        // acc_mult corresponds to key length (long):
        mult_diff_lookup(meta, sel_long, 3, s_main.bytes[0], accs.acc_s.mult, 128, fixed_table);

        // Checking the key - accumulated RLC is taken (computed using the path through
        // branches) and key bytes are added to the RLC. The external circuit
        // can check the key (where value in trie is being set at key) RLC is
        // the same as in key_rlc column.
        meta.create_gate(
            "Storage leaf key RLC & nibbles count (leaf not in first level, branch not placeholder) ",
            |meta| {
                let q_enable = q_enable(meta);
                let mut constraints = vec![];

                let flag1 = meta.query_advice(accs.s_mod_node_rlc, Rotation::cur());
                let flag2 = meta.query_advice(accs.c_mod_node_rlc, Rotation::cur());

                let last_level = flag1.clone() * flag2.clone();
                let is_long = flag1.clone() * (one.clone() - flag2.clone());
                let is_short = (one.clone() - flag1.clone()) * flag2.clone();
                let one_nibble = (one.clone() - flag1.clone()) * (one.clone() - flag2.clone());

                let is_leaf_in_first_level =
                    meta.query_advice(is_account_leaf_in_added_branch, Rotation(rot_into_account));

                // key rlc is in the first branch node (not branch init)
                let mut rot = -18;
                if !is_s {
                    rot = -20;
                }

                let key_rlc_acc_start = meta.query_advice(accs.key.rlc, Rotation(rot));
                let key_mult_start = meta.query_advice(accs.key.mult, Rotation(rot));

                // sel1 and sel2 are in init branch
                let sel1 = meta.query_advice(
                    s_main.bytes[IS_BRANCH_C16_POS - RLP_NUM],
                    Rotation(rot_into_init),
                );
                let sel2 = meta.query_advice(
                    s_main.bytes[IS_BRANCH_C1_POS - RLP_NUM],
                    Rotation(rot_into_init),
                );

                let mut is_branch_placeholder =
                    meta.query_advice(s_main.bytes[IS_BRANCH_S_PLACEHOLDER_POS - RLP_NUM], Rotation(rot_into_init));
                if !is_s {
                    is_branch_placeholder =
                        meta.query_advice(s_main.bytes[IS_BRANCH_C_PLACEHOLDER_POS - RLP_NUM], Rotation(rot_into_init));

                }

                // If the last branch is placeholder (the placeholder branch is the same as its
                // parallel counterpart), there is a branch modified_index nibble already
                // incorporated in key_rlc. That means we need to ignore the first nibble here
                // (in leaf key).

                // For short RLP (key starts at s_main.bytes[0]):

                // If sel1 = 1, we have one nibble+48 in s_main.bytes[0].
                let s_advice0 = meta.query_advice(s_main.bytes[0], Rotation::cur());
                let mut key_rlc_acc_short = key_rlc_acc_start.clone()
                    + (s_advice0.clone() - c48.clone()) * key_mult_start.clone() * sel1.clone();
                let mut key_mult = key_mult_start.clone() * r_table[0].clone() * sel1.clone();
                key_mult = key_mult + key_mult_start.clone() * sel2.clone(); // set to key_mult_start if sel2, stays key_mult if sel1

                // If sel2 = 1 and !is_branch_placeholder, we have 32 in s_main.bytes[0].
                constraints.push((
                    "Leaf key acc s_advice0",
                    q_enable.clone()
                        * (s_advice0.clone() - c32.clone())
                        * sel2.clone()
                        * (one.clone() - is_branch_placeholder.clone())
                        * (one.clone() - is_leaf_in_first_level.clone())
                        * is_short.clone(),
                ));

                let s_advices1 = meta.query_advice(s_main.bytes[1], Rotation::cur());
                key_rlc_acc_short = key_rlc_acc_short + s_advices1.clone() * key_mult.clone();

                for ind in 2..HASH_WIDTH {
                    let s = meta.query_advice(s_main.bytes[ind], Rotation::cur());
                    key_rlc_acc_short =
                        key_rlc_acc_short + s * key_mult.clone() * r_table[ind - 2].clone();
                }

                // c_rlp1 can appear if no branch above the leaf
                let c_rlp1 = meta.query_advice(c_main.rlp1, Rotation::cur());
                key_rlc_acc_short =
                    key_rlc_acc_short + c_rlp1.clone() * key_mult.clone() * r_table[30].clone();

                let key_rlc = meta.query_advice(accs.key.rlc, Rotation::cur());

                // No need to distinguish between sel1 and sel2 here as it was already
                // when computing key_rlc_acc_short.

                // Short example:
                // [226,160,59,138,106,70,105,186,37,13,38[227,32,161,160,187,239,170,18,88,1,56,188,38,60,149,117,120,38,223,78,36,235,129,201,170,170,170,170,170,170,170,170,170,170,170,170]
                constraints.push((
                    "Key RLC short",
                    q_enable.clone()
                        * (key_rlc_acc_short - key_rlc.clone())
                        * (one.clone() - is_branch_placeholder.clone())
                        * (one.clone() - is_leaf_in_first_level.clone())
                        * is_short.clone(),
                ));

                // For long RLP (key starts at s_main.bytes[1]):

                // If sel1 = 1, we have nibble+48 in s_main.bytes[1].
                let s_advice1 = meta.query_advice(s_main.bytes[1], Rotation::cur());
                let mut key_rlc_acc_long = key_rlc_acc_start.clone()
                    + (s_advice1.clone() - c48.clone()) * key_mult_start.clone() * sel1.clone();
                let mut key_mult = key_mult_start.clone() * r_table[0].clone() * sel1.clone();
                key_mult = key_mult + key_mult_start.clone() * sel2.clone(); // set to key_mult_start if sel2, stays key_mult if sel1

                // If sel2 = 1 and !is_branch_placeholder, we have 32 in s_main.bytes[1].
                constraints.push((
                    "Leaf key acc s_advice1",
                    q_enable.clone()
                        * (s_advice1.clone() - c32.clone())
                        * sel2.clone()
                        * (one.clone() - is_branch_placeholder.clone())
                        * (one.clone() - is_leaf_in_first_level.clone())
                        * is_long.clone(),
                ));

                let s_advices2 = meta.query_advice(s_main.bytes[2], Rotation::cur());
                key_rlc_acc_long = key_rlc_acc_long + s_advices2 * key_mult.clone();

                for ind in 3..HASH_WIDTH {
                    let s = meta.query_advice(s_main.bytes[ind], Rotation::cur());
                    key_rlc_acc_long =
                        key_rlc_acc_long + s * key_mult.clone() * r_table[ind - 3].clone();
                }

                key_rlc_acc_long =
                    key_rlc_acc_long + c_rlp1.clone() * key_mult.clone() * r_table[29].clone();
                // c_rlp2 can appear if no branch above the leaf
                let c_rlp2 = meta.query_advice(c_main.rlp2, Rotation::cur());
                key_rlc_acc_long =
                    key_rlc_acc_long + c_rlp2 * key_mult.clone() * r_table[30].clone();

                // No need to distinguish between sel1 and sel2 here as it was already
                // when computing key_rlc_acc_long.

         		// Long example:
                // [248,67,160,59,138,106,70,105,186,37,13,38,205,122,69,158,202,157,33,95,131,7,227,58,235,229,3,121,188,90,54,23,236,52,68,161,160,...
                constraints.push((
                    "Key RLC long",
                    q_enable.clone()
                        * (key_rlc_acc_long - key_rlc.clone())
                        * (one.clone() - is_branch_placeholder.clone())
                        * (one.clone() - is_leaf_in_first_level.clone())
                        * is_long.clone(),
                ));

                // Last level example:
		        // [227,32,161,160,187,239,170,18,88,1,56,188,38,60,149,117,120,38,223,78,36,235,129,201,170,170,170,170,170,170,170,170,170,170,170,170]
                constraints.push((
                    "Key RLC last level or one nibble",
                    q_enable.clone()
                        * (key_rlc_acc_start - key_rlc.clone()) // key_rlc has already been computed
                        * (one.clone() - is_branch_placeholder.clone())
                        * (one.clone() - is_leaf_in_first_level.clone())
                        * (last_level.clone() + one_nibble.clone()),
                ));

                // One nibble example short:
                // [194,48,1]

                // One nibble example long:
                // [227,48,161,160,187,239,170,18,88,1,56,188,38,60,149,117,120,38,223,78,36,235,129,201,170,170,170,170,170,170,170,170,170,170,170,170]

                // Nibbles count:
                let nibbles_count = meta.query_advice(
                    s_main.bytes[NIBBLES_COUNTER_POS - RLP_NUM],
                    Rotation(rot_into_init),
                );

                let s_rlp2 = meta.query_advice(s_main.rlp2, Rotation::cur());
                let leaf_nibbles_long = ((s_advice0.clone() - c128.clone() - one.clone()) * (one.clone() + one.clone())) * sel2.clone() +
                    ((s_advice0.clone() - c128.clone()) * (one.clone() + one.clone()) - one.clone()) * sel1.clone();
                let leaf_nibbles_short = ((s_rlp2.clone() - c128.clone() - one.clone()) * (one.clone() + one.clone())) * sel2.clone() +
                    ((s_rlp2.clone() - c128.clone()) * (one.clone() + one.clone()) - one.clone()) * sel1.clone();
                let leaf_nibbles_last_level = Expression::Constant(F::zero()); 
                let leaf_nibbles_one_nibble = one.clone(); 

                let leaf_nibbles = leaf_nibbles_long * is_long + leaf_nibbles_short * is_short
                    + leaf_nibbles_last_level * last_level + leaf_nibbles_one_nibble * one_nibble;

                 /* 
                Checking the total number of nibbles is to prevent having short addresses
                which could lead to a root node which would be shorter than 32 bytes and thus not hashed. That
                means the trie could be manipulated to reach a desired root.
                */
                constraints.push((
                    "Total number of storage address nibbles is 64 (not first level, not branch placeholder)",
                    q_enable
                        * (one.clone() - is_branch_placeholder.clone())
                        * (one.clone() - is_leaf_in_first_level.clone())
                        // Note: we need to check the number of nibbles being 64 for non_existing_account_proof too
                        // (even if the address being checked here might is the address of the wrong leaf)
                        * (nibbles_count.clone() + leaf_nibbles.clone() - c64.clone()),
                ));

                constraints
            },
        );

        meta.create_gate("Storage leaf key RLC (leaf in first level)", |meta| {
            // Note: last_level (leaf being in the last level) cannot occur here because we are
            // in the first level. If both flags would be 1, is_long and is_short would
            // both be true which would lead into failed constraints.

            let q_enable = q_enable(meta);
            let mut constraints = vec![];

            let is_long = meta.query_advice(accs.s_mod_node_rlc, Rotation::cur());
            let is_short = meta.query_advice(accs.c_mod_node_rlc, Rotation::cur());
            let is_leaf_in_first_level =
                meta.query_advice(is_account_leaf_in_added_branch, Rotation(rot_into_account));

            // Note: when leaf is in the first level, the key stored in the leaf is always of length 33 -
            // the first byte being 32 (when after branch, the information whether there the key is odd or even
            // is in s_main.bytes[IS_BRANCH_C16_POS - LAYOUT_OFFSET] (see sel1/sel2).

            // For short RLP (key starts at s_main.bytes[0]):
            let s_advice0 = meta.query_advice(s_main.bytes[0], Rotation::cur());
            let mut key_rlc_acc_short = Expression::Constant(F::zero());
            let key_mult = one.clone();

            constraints.push((
                "Leaf key acc s_advice0",
                q_enable.clone()
                    * (s_advice0.clone() - c32.clone())
                    * is_leaf_in_first_level.clone()
                    * is_short.clone(),
            ));

            let s_advices1 = meta.query_advice(s_main.bytes[1], Rotation::cur());
            key_rlc_acc_short = key_rlc_acc_short + s_advices1.clone() * key_mult.clone();

            for ind in 2..HASH_WIDTH {
                let s = meta.query_advice(s_main.bytes[ind], Rotation::cur());
                key_rlc_acc_short =
                    key_rlc_acc_short + s * key_mult.clone() * r_table[ind - 2].clone();
            }

            // c_rlp1 can appear if no branch above the leaf
            let c_rlp1 = meta.query_advice(c_main.rlp1, Rotation::cur());
            key_rlc_acc_short =
                key_rlc_acc_short + c_rlp1.clone() * key_mult.clone() * r_table[30].clone();

            let key_rlc = meta.query_advice(accs.key.rlc, Rotation::cur());

            // No need to distinguish between sel1 and sel2 here as it was already
            // when computing key_rlc_acc_short.
            constraints.push((
                "Key RLC short",
                q_enable.clone()
                    * (key_rlc_acc_short - key_rlc.clone())
                    * is_leaf_in_first_level.clone()
                    * is_short.clone(),
            ));

            // For long RLP (key starts at s_main.bytes[1]):
            let s_advice1 = meta.query_advice(s_main.bytes[1], Rotation::cur());
            let mut key_rlc_acc_long = Expression::Constant(F::zero());

            constraints.push((
                "Leaf key acc s_advice1",
                q_enable.clone()
                    * (s_advice1.clone() - c32.clone())
                    * is_leaf_in_first_level.clone()
                    * is_long.clone(),
            ));

            let s_advices2 = meta.query_advice(s_main.bytes[2], Rotation::cur());
            key_rlc_acc_long = key_rlc_acc_long + s_advices2 * key_mult.clone();

            for ind in 3..HASH_WIDTH {
                let s = meta.query_advice(s_main.bytes[ind], Rotation::cur());
                key_rlc_acc_long =
                    key_rlc_acc_long + s * key_mult.clone() * r_table[ind - 3].clone();
            }

            key_rlc_acc_long =
                key_rlc_acc_long + c_rlp1.clone() * key_mult.clone() * r_table[29].clone();
            // c_rlp2 can appear if no branch above the leaf
            let c_rlp2 = meta.query_advice(c_main.rlp2, Rotation::cur());
            key_rlc_acc_long = key_rlc_acc_long + c_rlp2 * key_mult.clone() * r_table[30].clone();

            constraints.push((
                "Key RLC long",
                q_enable.clone()
                    * (key_rlc_acc_long - key_rlc.clone())
                    * is_leaf_in_first_level.clone()
                    * is_long.clone(),
            ));

            // When in first level, it is always even number of nibbles (`sel2 = 1`).
            let s_rlp2 = meta.query_advice(s_main.rlp2, Rotation::cur());
            let leaf_nibbles_long = (s_advice0.clone() - c128.clone() - one.clone()) * (one.clone() + one.clone());
            let leaf_nibbles_short = (s_rlp2.clone() - c128.clone() - one.clone()) * (one.clone() + one.clone());
            let leaf_nibbles = leaf_nibbles_long * is_long + leaf_nibbles_short * is_short;

            /* 
            Checking the total number of nibbles is to prevent having short addresses
            which could lead to a root node which would be shorter than 32 bytes and thus not hashed. That
            means the trie could be manipulated to reach a desired root.
            */
            constraints.push((
                "Total number of account address nibbles is 64 (first level)",
                q_enable
                    // Note: we need to check the number of nibbles being 64 for non_existing_account_proof too
                    // (even if the address being checked here might is the address of the wrong leaf)
                    * is_leaf_in_first_level.clone()
                    * (leaf_nibbles.clone() - c64.clone()),
            ));

            constraints
        });

        // For leaf under placeholder branch we wouldn't need to check key RLC -
        // this leaf is something we didn't ask for. For example, when setting a leaf L
        // causes that leaf L1 (this is the leaf under branch placeholder)
        // is replaced by branch, then we get placeholder branch at S positions
        // and leaf L1 under it. However, key RLC needs to be compared for leaf L,
        // because this is where the key was changed (but it causes to change also L1).
        // In delete, the situation is just turned around.
        // However, we check key RLC for this leaf too because this simplifies
        // the constraints for checking that leaf L1 is the same as the leaf that
        // is in the branch parallel to the placeholder branch -
        // same with the exception of extension node key. This can be checked by
        // comparing key RLC of the leaf before being replaced by branch and key RLC
        // of this same leaf after it drifted into a branch.
        // Constraints for this are in leaf_key_in_added_branch.

        // Note that hash of leaf L1 needs to be checked to be in the branch
        // above the placeholder branch - this is checked in leaf_value (where RLC
        // from the first gate above is used).

        // Check that key_rlc_prev stores key_rlc from the previous branch (needed when
        // after the placeholder).
        meta.create_gate("Previous level RLC", |meta| {
            let q_enable = q_enable(meta);
            let mut constraints = vec![];

            let is_first_storage_level =
                meta.query_advice(is_account_leaf_in_added_branch, Rotation(rot_into_init - 1));

            let is_leaf_without_branch =
                meta.query_advice(is_account_leaf_in_added_branch, Rotation(rot_into_account));

            // Could be used any rotation into previous branch, because key RLC is the same
            // in all branch children:
            let rot_into_prev_branch = rot_into_init - 5;
            // TODO: check why a different rotation causes (for example rot_into_init - 3)
            // causes ConstraintPoisened

            // key_rlc_mult_prev_level = 1 if is_first_storage_level
            let key_rlc_mult_prev_level = (one.clone() - is_first_storage_level.clone())
                * meta.query_advice(accs.key.mult, Rotation(rot_into_prev_branch))
                + is_first_storage_level.clone();
            // key_rlc_prev_level = 0 if is_first_storage_level
            let key_rlc_prev_level = (one.clone() - is_first_storage_level)
                * meta.query_advice(accs.key.rlc, Rotation(rot_into_prev_branch));

            let rlc_prev = meta.query_advice(denoter.sel1, Rotation::cur());
            let mult_prev = meta.query_advice(denoter.sel2, Rotation::cur());

            constraints.push((
                "Previous key RLC",
                q_enable.clone()
                    * (rlc_prev - key_rlc_prev_level)
                    * (one.clone() - is_leaf_without_branch.clone()),
            ));
            constraints.push((
                "Previous key RLC mult",
                q_enable
                    * (mult_prev - key_rlc_mult_prev_level)
                    * (one.clone() - is_leaf_without_branch.clone()),
            ));

            constraints
        });

        // For a leaf after placeholder, we need to use key_rlc from previous level
        // (the branch above placeholder).
        meta.create_gate("Storage leaf key RLC (after placeholder)", |meta| {
            // Note: last_level cannot occur in a leaf after placeholder branch, because being
            // after placeholder branch means this leaf drifted down into a new branch (in a parallel
            // proof) and thus cannot be in the last level.

            let q_enable = q_enable(meta);
            let mut constraints = vec![];

            let flag1 = meta.query_advice(accs.s_mod_node_rlc, Rotation::cur());
            let flag2 = meta.query_advice(accs.c_mod_node_rlc, Rotation::cur());
            let is_long = flag1.clone() * (one.clone() - flag2.clone());
            let is_short = (one.clone() - flag1.clone()) * flag2.clone();
            let one_nibble = (one.clone() - flag1.clone()) * (one.clone() - flag2.clone());

            // Note: key rlc is in the first branch node (not branch init).
            let rot_level_above = rot_into_init + 1 - BRANCH_ROWS_NUM;

            let is_first_storage_level =
                meta.query_advice(is_account_leaf_in_added_branch, Rotation(rot_into_init - 1));

            let is_leaf_in_first_level =
                meta.query_advice(is_account_leaf_in_added_branch, Rotation(rot_into_account));

            let mut is_branch_placeholder =
                meta.query_advice(s_main.bytes[IS_BRANCH_S_PLACEHOLDER_POS - RLP_NUM], Rotation(rot_into_init));
            if !is_s {
                is_branch_placeholder =
                    meta.query_advice(s_main.bytes[IS_BRANCH_C_PLACEHOLDER_POS - RLP_NUM], Rotation(rot_into_init));
            }

            // Previous key RLC:
            /*
            Note: if using directly:
            let key_rlc_prev = meta.query_advice(key_rlc, Rotation(rot_level_above));
            The ConstraintPoisoned error is thrown in extension_node_key.
            */
            let key_rlc_acc_start = meta.query_advice(denoter.sel1, Rotation::cur())
                * (one.clone() - is_first_storage_level.clone());
            let key_mult_start = meta.query_advice(denoter.sel2, Rotation::cur())
                * (one.clone() - is_first_storage_level.clone())
                + is_first_storage_level.clone();

            // Note: the approach (like for sel1 and sel2) with retrieving
            // key RLC and key RLC mult from the level above placeholder fails
            // due to ConstraintPoisened error.
            // sel1 and sel2 are in init branch
            // Note that when is_first_storage_level, it is always sel2 = 1 because
            // there are all 32 bytes in a key.
            let sel1 = (one.clone() - is_first_storage_level.clone())
                * meta.query_advice(
                    s_main.bytes[IS_BRANCH_C16_POS - RLP_NUM],
                    Rotation(rot_level_above - 1),
                );
            let sel2 = (one.clone() - is_first_storage_level.clone())
                * meta.query_advice(
                    s_main.bytes[IS_BRANCH_C1_POS - RLP_NUM],
                    Rotation(rot_level_above - 1),
                )
                + is_first_storage_level.clone();

            // For short RLP (key starts at s_main.bytes[0]):

            // If sel1 = 1, we have one nibble+48 in s_main.bytes[0].
            let s_advice0 = meta.query_advice(s_main.bytes[0], Rotation::cur());
            let mut key_rlc_acc_short = key_rlc_acc_start.clone()
                + (s_advice0.clone() - c48.clone()) * key_mult_start.clone() * sel1.clone();
            let key_mult = key_mult_start.clone() * r_table[0].clone() * sel1.clone()
                + key_mult_start.clone() * sel2.clone(); // set to key_mult_start if sel2, stays key_mult if sel1

            // If sel2 = 1, we have 32 in s_main.bytes[0].
            constraints.push((
                "Leaf key acc s_advice0",
                q_enable.clone()
                    * (s_advice0.clone() - c32.clone())
                    * sel2.clone()
                    * is_branch_placeholder.clone()
                    * (one.clone() - is_leaf_in_first_level.clone())
                    * is_short.clone(),
            ));

            let s_advices1 = meta.query_advice(s_main.bytes[1], Rotation::cur());
            key_rlc_acc_short = key_rlc_acc_short + s_advices1.clone() * key_mult.clone();

            for ind in 2..HASH_WIDTH {
                let s = meta.query_advice(s_main.bytes[ind], Rotation::cur());
                key_rlc_acc_short =
                    key_rlc_acc_short + s * key_mult.clone() * r_table[ind - 2].clone();
            }

            let c_rlp1 = meta.query_advice(c_main.rlp1, Rotation::cur());
            key_rlc_acc_short =
                key_rlc_acc_short + c_rlp1.clone() * key_mult.clone() * r_table[30].clone();

            let key_rlc = meta.query_advice(accs.key.rlc, Rotation::cur());

            // No need to distinguish between sel1 and sel2 here as it was already
            // when computing key_rlc_acc_short.
            constraints.push((
                "Key RLC short",
                q_enable.clone()
                    * (key_rlc_acc_short - key_rlc.clone())
                    * is_branch_placeholder.clone()
                    * (one.clone() - is_leaf_in_first_level.clone())
                    * is_short.clone(),
            ));

            // For long RLP (key starts at s_main.bytes[1]):

            // If sel1 = 1, we have nibble+48 in s_main.bytes[1].
            let s_advice1 = meta.query_advice(s_main.bytes[1], Rotation::cur());
            let mut key_rlc_acc_long = key_rlc_acc_start.clone()
                + (s_advice1.clone() - c48.clone()) * key_mult_start.clone() * sel1.clone();

            // If sel2 = 1, we have 32 in s_main.bytes[1].
            constraints.push((
                "Leaf key acc s_advice1",
                q_enable.clone()
                    * (s_advice1.clone() - c32.clone())
                    * sel2.clone()
                    * is_branch_placeholder.clone()
                    * (one.clone() - is_leaf_in_first_level.clone())
                    * is_long.clone(),
            ));

            let s_advices2 = meta.query_advice(s_main.bytes[2], Rotation::cur());
            key_rlc_acc_long = key_rlc_acc_long + s_advices2 * key_mult.clone();

            for ind in 3..HASH_WIDTH {
                let s = meta.query_advice(s_main.bytes[ind], Rotation::cur());
                key_rlc_acc_long =
                    key_rlc_acc_long + s * key_mult.clone() * r_table[ind - 3].clone();
            }

            key_rlc_acc_long =
                key_rlc_acc_long + c_rlp1.clone() * key_mult.clone() * r_table[29].clone();

            let c_rlp2 = meta.query_advice(c_main.rlp2, Rotation::cur());
            key_rlc_acc_long = key_rlc_acc_long + c_rlp2.clone() * key_mult * r_table[30].clone();

            // No need to distinguish between sel1 and sel2 here as it was already
            // when computing key_rlc_acc_long.
            constraints.push((
                "Key RLC long",
                q_enable.clone()
                    * (key_rlc_acc_long - key_rlc.clone())
                    * is_branch_placeholder.clone()
                    * (one.clone() - is_leaf_in_first_level.clone())
                    * is_long.clone(),
            ));

            /*
            Note: When the leaf is after the placeholder branch, it cannot be in the last level
            otherwise it would not be possible to add a branch placeholder.
            */

            let s_rlp2 = meta.query_advice(s_main.rlp2, Rotation::cur());
            let leaf_nibbles_long = ((s_advice0.clone() - c128.clone() - one.clone()) * (one.clone() + one.clone())) * sel2.clone() +
                ((s_advice0.clone() - c128.clone()) * (one.clone() + one.clone()) - one.clone()) * sel1.clone();
            let leaf_nibbles_short = ((s_rlp2.clone() - c128.clone() - one.clone()) * (one.clone() + one.clone())) * sel2.clone() +
                ((s_rlp2.clone() - c128.clone()) * (one.clone() + one.clone()) - one.clone()) * sel1.clone();
            let leaf_nibbles_one_nibble = one.clone(); 

            let leaf_nibbles = leaf_nibbles_long * is_long + leaf_nibbles_short * is_short
                + leaf_nibbles_one_nibble * one_nibble;

            /*
            Note that when the leaf is in the first storage level (but positioned after the placeholder
            in the circuit), there is no branch above the placeholder branch from where
            `nibbles_count` is to be retrieved. In that case `nibbles_count = 0`.
            */
            let nibbles_count = meta.query_advice(
                s_main.bytes[NIBBLES_COUNTER_POS - RLP_NUM],
                Rotation(rot_into_init - BRANCH_ROWS_NUM),
            ) * (one.clone() - is_first_storage_level.clone());

            constraints.push((
                "Total number of account address nibbles is 64 (after placeholder)",
                q_enable
                    * is_branch_placeholder.clone()
                    * (one.clone() - is_leaf_in_first_level.clone())
                    * (nibbles_count.clone() + leaf_nibbles.clone() - c64.clone()),
            ));

            constraints
        });

        range_lookups(
            meta,
            q_enable,
            s_main.bytes.to_vec(),
            FixedTableTag::Range256,
            fixed_table,
        );
        range_lookups(
            meta,
            q_enable,
            [s_main.rlp1, s_main.rlp2, c_main.rlp1, c_main.rlp2].to_vec(),
            FixedTableTag::Range256,
            fixed_table,
        );

        config
    }

    pub fn construct(config: LeafKeyConfig) -> Self {
        Self {
            config,
            _marker: PhantomData,
        }
    }
}

impl<F: FieldExt> Chip<F> for LeafKeyChip<F> {
    type Config = LeafKeyConfig;
    type Loaded = ();

    fn config(&self) -> &Self::Config {
        &self.config
    }

    fn loaded(&self) -> &Self::Loaded {
        &()
    }
}