use anyhow::Error;

use crate::{function::CommonFunction, source::Source};

/// Typed check result
#[derive(Debug)]
pub struct CheckResult {
    /// Overall status (e.g., any fatal error that prevented full checking)
    pub status: anyhow::Result<()>,
    /// Functions that passed the consistency check
    pub ok: Vec<String>,
    /// Functions that failed the consistency check
    pub fail: Vec<String>,
}

impl CheckResult {
    pub fn failed(e: Error) -> Self {
        Self {
            status: Err(e),
            ok: Vec::new(),
            fail: Vec::new(),
        }
    }
}

/// A single check step
pub trait CheckStep {
    /// Name of the step
    fn name(&self) -> &str;

    /// Additional note to print
    fn note(&self) -> Option<&str> {
        None
    }

    /// Run the check step
    fn run(&self, checker: &Checker) -> CheckResult;
}

/// Checker coordinating multiple steps
pub struct Checker {
    /// Check steps to run
    steps: Vec<Box<dyn CheckStep>>,
    /// Source 1
    pub src1: Source,
    /// Source 2
    pub src2: Source,
    /// Functions that has been verified
    pub checked_funcs: Vec<CommonFunction>,
    /// Functions that has not been verification
    pub unchecked_funcs: Vec<CommonFunction>,
}

impl Checker {
    pub fn new(src1: Source, src2: Source, steps: Vec<Box<dyn CheckStep>>) -> Self {
        let mut checker = Self {
            src1,
            src2,
            steps,
            checked_funcs: Vec::new(),
            unchecked_funcs: Vec::new(),
        };
        checker.src1.unique_funcs.iter().for_each(|func| {
            if let Some(func2) = checker
                .src2
                .unique_funcs
                .iter()
                .find(|func2| func.sig_eq(&func2))
            {
                checker
                    .unchecked_funcs
                    .push(CommonFunction::new(func.clone(), func2.clone()).unwrap());
            }
        });
        checker.src1.unique_funcs.retain(|func| {
            !checker
                .unchecked_funcs
                .iter()
                .any(|func2| func.name == func2.name())
        });
        checker.src2.unique_funcs.retain(|func| {
            !checker
                .unchecked_funcs
                .iter()
                .any(|func2| func.name == func2.name())
        });
        checker
    }

    /// Run all steps in order
    pub fn run_all(&mut self) -> anyhow::Result<()> {
        for step in &self.steps {
            println!(""); // empty line to separate steps
            match step.note() {
                Some(note) => println!("Step `{}` => {:?}", step.name(), note),
                None => println!("Step `{}`", step.name()),
            }

            let res = step.run(&self);
            if let Err(e) = res.status {
                println!("Step `{}` failed to execute: {}", step.name(), e);
            }

            for name in &res.ok {
                println!("  ✅ OK: {}", name);
                if let Some(func) = self
                    .unchecked_funcs
                    .iter()
                    .find(|func2| func2.name() == name)
                {
                    self.checked_funcs.push(func.clone());
                    self.unchecked_funcs.retain(|func2| func2.name() != name);
                }
            }
            self.print_state();

            if !res.fail.is_empty() {
                return Err(anyhow::anyhow!(
                    "Step `{}` failed: inconsistent functions {:?}",
                    step.name(),
                    res.fail
                ));
            }
        }

        if !self.unchecked_funcs.is_empty() {
            let names: Vec<_> = self.unchecked_funcs.iter().map(|f| f.name()).collect();
            Err(anyhow::anyhow!("Unchecked functions remain: {:?}", names))
        } else {
            Ok(())
        }
    }

    /// Print current state of the checker
    pub fn print_state(&self) {
        println!("Checker state:");
        println!("  Checked: {:?}", self.checked_funcs);
        println!("  Unchecked: {:?}", self.unchecked_funcs);
        println!("  Source1 Unique: {:?}", self.src1.unique_funcs);
        println!("  Source2 Unique: {:?}", self.src2.unique_funcs);
    }
}
