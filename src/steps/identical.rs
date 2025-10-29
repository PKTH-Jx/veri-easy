use crate::checker::{CheckResult, CheckStep, Checker};

/// Identical step: if bodies are identical -> ok; if same name but different body -> undetermined.
pub struct Identical;

impl CheckStep for Identical {
    fn name(&self) -> &str {
        "Identical"
    }

    fn note(&self) -> Option<&str> {
        Some("Compare function bodies for identity")
    }

    fn run(&self, checker: &Checker) -> CheckResult {
        let mut res = CheckResult {
            status: Ok(()),
            ok: vec![],
            fail: vec![],
        };

        // only consider functions present in both srcs (unchecked sets already contain intersection)
        for func in &checker.unchecked_funcs {
            if func.f1.body() == func.f2.body() {
                res.ok.push(func.name().to_owned());
            }
        }

        res
    }
}
