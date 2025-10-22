use crate::source::Source;

/// Typed check result
#[derive(Debug, Default)]
pub struct CheckResult {
    pub ok: Vec<String>,
    pub fail: Vec<String>,
    pub undetermined: Vec<String>,
    pub note: Option<String>,
}

/// A single check step
pub trait CheckStep {
    fn name(&self) -> &str;
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
            let res = step.run(&self.src1, &self.src2);
            match res.note {
                Some(note) => println!("Step `{}` => {:?}", step.name(), note),
                None => println!("Step `{}`", step.name()),
            }

            for func in &res.ok {
                println!("  ✅ OK: {}", func);
                self.src1.set_checked(func);
                self.src2.set_checked(func);
            }

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
