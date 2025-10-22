//! Test steps

use crate::{
    checker::{CheckResult, CheckStep},
    source::Source,
};

/// Identical step: if bodies are identical -> ok; if same name but different body -> fail.
pub struct Identical;

impl CheckStep for Identical {
    fn name(&self) -> &str {
        "Identical"
    }

    fn run(&self, src1: &Source, src2: &Source) -> CheckResult {
        let mut res = CheckResult {
            note: Some("Compare function bodies for identity".into()),
            ..Default::default()
        };

        // only consider functions present in both srcs (unchecked sets already contain intersection)
        for f1 in &src1.unchecked_funcs {
            // lookup corresponding function in src2
            if let Some(f2) = src2.unchecked_func(&f1.name) {
                if f1.eq(f2) {
                    res.ok.push(f1.name.clone());
                } else {
                    res.undetermined.push(f1.name.clone()); // same name but different body -> fail
                }
            } else {
                // shouldn't happen since set_ignored keeps only commons, but safe fallback
                res.undetermined.push(f1.name.clone());
            }
        }

        res
    }
}

// ---------------- Example usage ----------------
