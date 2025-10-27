use crate::{
    checker::{CheckResult, CheckStep},
    source::Source,
};

/// Identical step: if bodies are identical -> ok; if same name but different body -> undetermined.
pub struct Identical;

impl CheckStep for Identical {
    fn name(&self) -> &str {
        "Identical"
    }

    fn note(&self) -> Option<&str> {
        Some("Compare function bodies for identity")
    }

    fn run(&self, src1: &Source, src2: &Source) -> CheckResult {
        let mut res = CheckResult {
            status: Ok(()),
            ok: vec![],
            fail: vec![],
        };

        // only consider functions present in both srcs (unchecked sets already contain intersection)
        for f1 in &src1.unchecked_funcs {
            if let Some(f2) = src2.unchecked_func_by_signature(&f1) {
                if f1.eq(f2) {
                    res.ok.push(f1.name.clone());
                }
            }
        }

        res
    }
}
