//! The one sentence Catalyst says about what its adapter boundary is.
//!
//! `docs/interface.md` §7 fixes the words; §6 says they appear verbatim on
//! `catalyst adapter --help`, on `catalyst tools discover` and on the
//! conformance report. Three surfaces, one string: a claim that drifts between
//! surfaces is a claim a reader cannot check, and the whole point of this one
//! is that it is honest about what has *not* been measured.

/// The label of `docs/interface.md` §7, verbatim.
pub const LABEL: &str =
    "local-adapter extension interface; live local-model compatibility untested";

#[cfg(test)]
mod tests {
    use super::*;

    /// `catalyst adapter --help` carries the label as plain text, so the usage
    /// string holds its own copy of these words. This is what keeps the two
    /// from drifting apart.
    #[test]
    fn the_usage_text_of_adapter_carries_the_label_verbatim() {
        assert!(crate::cli::usage("adapter").contains(LABEL));
    }
}
