use anyhow::Error;

use crate::source::Source;

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
    fn run(&self, src1: &Source, src2: &Source) -> CheckResult;
}

/// Checker coordinating multiple steps
pub struct Checker {
    src1: Source,
    src2: Source,
    steps: Vec<Box<dyn CheckStep>>,
}

impl Checker {
    pub fn new(src1: Source, src2: Source, steps: Vec<Box<dyn CheckStep>>) -> Self {
        let mut checker = Self { src1, src2, steps };
        Source::set_ignored(&mut checker.src1, &mut checker.src2);
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

            let res = step.run(&self.src1, &self.src2);
            if let Err(e) = res.status {
                println!("Step `{}` failed to execute: {}", step.name(), e);
            }

            for func in &res.ok {
                println!("  ✅ OK: {}", func);
                self.src1.set_checked(func);
                self.src2.set_checked(func);
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

        if !self.src1.unchecked_funcs.is_empty() {
            let names: Vec<_> = self.src1.unchecked_funcs.iter().map(|f| &f.name).collect();
            Err(anyhow::anyhow!("Unchecked functions remain: {:?}", names))
        } else {
            Ok(())
        }
    }

    /// Print current state of the checker
    pub fn print_state(&self) {
        println!("Checker state:");
        println!("  Source 1: {:?}", self.src1);
        println!("  Source 2: {:?}", self.src2);
    }
}
