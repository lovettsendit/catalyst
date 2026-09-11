//! # catalyst
//!
//! Catalyst turns a numerical computation into something that can be
//! measured, differentiated, checked and handed on. At its centre is an
//! **engine** that differentiates *optimised* SSA programs -- blocks,
//! branches, phi nodes, integer counters, selects, memory -- in forward and
//! reverse mode, written entirely in Rust with no dependencies and no
//! `unsafe`. Compiler-level automatic differentiation is a general technique
//! with published prior art; this crate is its own implementation of it,
//! built around one thesis: the passes run in this order, and the order is
//! what makes the derivative cheap.
//!
//! ```text
//!   program  ->  IR  ->  optimise  ->  type analysis  ->  activity analysis
//!                                                            |
//!                              forward mode  <---------------+---------> reverse mode
//!                                     |                                       |
//!                                     +---------> optimise again -------------+
//!                                                       |
//!                                              interpret, or emit
//!                                              Go or R and run it there
//! ```
//!
//! # The engine, module by module
//!
//! * [`ir`] is the intermediate representation every pass operates on:
//!   typed SSA, blocks, phis, memory.
//! * [`build`] is a builder for writing IR by hand, used by the tests and the
//!   front ends.
//! * [`interp`] executes the IR under an instruction budget; it is the
//!   reference semantics everything else is checked against.
//! * [`opt`] is the optimiser that runs before and after differentiation.
//! * [`domtree`] computes dominators, dominance frontiers and natural loops
//!   for the optimiser and the reverse transform.
//! * [`typeanalysis`] decides what the bytes behind a load or store are.
//! * [`activity`] decides which values need a derivative at all.
//! * [`forward`] is forward mode, tangents carried beside the primal.
//! * [`reverse`] is reverse mode, adjoints accumulated backwards; in this
//!   build it replays straight-line code only.
//! * [`refuse`] is the structured refusal a transform returns, with a stable
//!   code and a remedy, rather than a formatted sentence.
//! * [`api`] is the one-call entry point, [`api::solve`]: source in, value
//!   and gradient out.
//! * [`programs`] holds the programs the tests, benchmarks and documentation
//!   are all measured on, one definition each.
//!
//! # Two front ends, one engine, one verifier
//!
//! * The **expression language** ([`text`]): `func f(x, y) = x * y + sin(x)`,
//!   parsed straight into the engine's IR. This is the lane the problem
//!   documents of `docs/interface.md` §1 use, and the lane that has portable
//!   Go and R exports ([`export`], [`gocode`], [`rcode`]).
//! * The **Catalyst Gradient Compiler** ([`gradient_compiler`]): the textual
//!   LLVM IR a user's own compiler wrote, read by [`gradient_compiler::llvm`],
//!   lowered onto the engine's IR by [`gradient_compiler::lower`], and
//!   differentiated by the same transforms. Its output is a derivative
//!   artifact, `catalyst.derivative.v1`, with a chain of SHA-256 digests from
//!   the module to the validation. The engine handles memory; the LLVM subset
//!   the CGC lowers in this phase does not yet -- scalar arithmetic,
//!   comparisons, select, branches, phis, loops and the math intrinsics are
//!   read, and a load, store or aggregate is refused as
//!   `catalyst.llvm_unsupported` with its line (`docs/interface.md` §12.3).
//!
//! Every derivative from either lane goes through **Catalyst's verifier**
//! ([`gradient_compiler::verify`]): central finite differences over the
//! primal, at the declared point and around it. The front ends propose; the
//! verifier decides; a derivative it does not confirm is refused and nothing
//! is written.
//!
//! # The crate and the command
//!
//! Everything above is the engine and its front ends. The modules below them
//! -- [`cli`], [`json`], [`paths`], [`problem`], [`lower`], [`export`],
//! [`differentiate`], [`tui`], [`tools`], [`ai`], [`adapter`], [`rehearse`]
//! and [`label`] -- are the `catalyst` binary's half: reading a problem
//! document, applying one rule to every path a user names, lowering a
//! problem to the form the Go and R exports are generated from, guiding
//! somebody through the flow, answering a machine caller, asking a configured
//! command for a proposal, and writing an export or an artifact somebody else
//! can check without this crate. They live in the library rather than in
//! `src/main.rs` so that the acceptance oracles can call them directly.
//!
//! # No unsafe, and the compiler holds the line
//!
//! `#![forbid(unsafe_code)]` is not a comment about the current state of the
//! source: it makes an `unsafe` block anywhere in the crate a compile error,
//! including in code added later by somebody who did not read this paragraph.

#![forbid(unsafe_code)]

pub mod activity;
pub mod adapter;
pub mod ai;
pub mod api;
pub mod build;
pub mod cli;
pub mod differentiate;
pub mod domtree;
pub mod export;
pub mod forward;
pub mod gocode;
pub mod gradient_compiler;
pub mod interp;
pub mod ir;
pub mod json;
pub mod label;
pub mod lower;
pub mod opt;
pub mod paths;
pub mod problem;
pub mod programs;
pub mod rcode;
pub mod refuse;
pub mod rehearse;
mod request_json;
pub mod reverse;
pub mod text;
pub mod tools;
pub mod tui;
pub mod typeanalysis;

pub use build::Builder;
pub use interp::{Interp, Memory};
pub use ir::{BinOp, BlockId, Func, FuncId, Module, Op, Pred, Term, Ty, UnOp, Value};
