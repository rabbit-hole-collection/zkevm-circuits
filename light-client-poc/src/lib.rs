#[macro_use]
extern crate lazy_static;

#[cfg(feature = "prover")]
pub mod circuits;

#[cfg(feature = "prover")]
pub mod witness;

pub mod verifier;