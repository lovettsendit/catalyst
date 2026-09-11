//! Where a derivative came from: the compiler that wrote the module, the
//! target it wrote it for, and the version of this crate that read it.
//!
//! None of it is a clock. `!llvm.ident` carries the compiler's own version
//! string, which is copied verbatim because it is provenance, not a
//! timestamp: two runs over one module carry the same string, and that is
//! the property the artifact's determinism rests on.

use super::llvm;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Provenance {
    /// The string inside `!llvm.ident`, or `"unknown"`; `"catalyst"` for the
    /// expression lane, whose compiler is this crate.
    pub compiler: String,
    /// `target triple`, or `"unknown"`; `"catalyst-ir"` for the expression
    /// lane.
    pub target_triple: String,
}

impl Provenance {
    /// What a read module says about itself.
    pub fn of_module(module: &llvm::Module) -> Provenance {
        Provenance {
            compiler: module.ident.clone().unwrap_or_else(|| "unknown".to_owned()),
            target_triple: module
                .triple
                .clone()
                .unwrap_or_else(|| "unknown".to_owned()),
        }
    }

    /// The expression lane: the compiler is Catalyst itself.
    pub fn native() -> Provenance {
        Provenance {
            compiler: "catalyst".to_owned(),
            target_triple: "catalyst-ir".to_owned(),
        }
    }
}

/// The crate version, which is the Catalyst Gradient Compiler's version: it
/// is not a separate product with a separate number.
pub fn cgc_version() -> &'static str {
    env!("CARGO_PKG_VERSION")
}
