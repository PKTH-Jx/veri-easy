//! Veri-easy functional equivalence checker.
use anyhow::Error;

use crate::{collect::{FunctionCollector, TraitCollector}, defs::{CommonFunction, Function, Path}};

/// A Rust source file with information about functions and symbols.
pub struct Source {
    /// File path.
    pub path: String,
    /// Full text content.
    pub content: String,
    /// Unique functions (exist only in one file).
    pub unique_funcs: Vec<Function>,
    /// Symbols need to be imported when generating harness.
    pub symbols: Vec<Path>,
}

impl Source {
    /// Open a source file from path and parse its content.
    pub fn open(path: &str) -> anyhow::Result<Self> {
        let content =
            std::fs::read_to_string(&path).map_err(|_| anyhow::anyhow!("Failed to read source"))?;
        let syntax = syn::parse_file(&content)
            .map_err(|_| anyhow::anyhow!("Failed to parse source file"))?;
        
        // Collect functions
        let unique_funcs = FunctionCollector::new().collect(&syntax);
        // Collect symbols
        let symbols = TraitCollector::new().collect(&syntax);
        
        Ok(Self {
            path: path.to_owned(),
            content,
            unique_funcs,
            symbols,
        })
    }
}

/// The main Checker structure.
///
/// Check function consistency between two sources through multiple components.
pub struct Checker {
    /// Check components to run.
    components: Vec<Box<dyn Component>>,
    /// First source file.
    pub src1: Source,
    /// Second source file.
    pub src2: Source,
    /// Functions that has not been verified yet.
    pub unchecked_funcs: Vec<CommonFunction>,
    /// Functions that has been verified by formal components.
    pub verified_funcs: Vec<CommonFunction>,
    /// Functions that has been checked by testing components.
    pub tested_funcs: Vec<CommonFunction>,
}

impl Checker {
    pub fn new(src1: Source, src2: Source, steps: Vec<Box<dyn Component>>) -> Self {
        let mut checker = Self {
            src1,
            src2,
            components: steps,
            verified_funcs: Vec::new(),
            unchecked_funcs: Vec::new(),
            tested_funcs: Vec::new(),
        };
        checker.preprocess();
        checker
    }

    /// Run all steps in order
    pub fn run_all(&mut self) -> anyhow::Result<()> {
        for component in &self.components {
            println!(""); // empty line to separate steps
            match component.note() {
                Some(note) => println!("Step `{}` => {:?}", component.name(), note),
                None => println!("Step `{}`", component.name()),
            }

            let res = component.run(&self);
            if let Err(e) = res.status {
                println!("Step `{}` failed to execute: {}", component.name(), e);
            }

            for name in &res.ok {
                println!("OK: {:?}", name);
                if let Some(func) = self
                    .unchecked_funcs
                    .iter()
                    .find(|func2| func2.metadata.name == *name)
                {
                    if component.is_formal() {
                        self.verified_funcs.push(func.clone());
                    } else {
                        self.tested_funcs.push(func.clone());
                    }
                    self.unchecked_funcs
                        .retain(|func2| func2.metadata.name != *name);
                }
            }
            self.print_state();

            if !res.fail.is_empty() {
                return Err(anyhow::anyhow!(
                    "Step `{}` failed: inconsistent functions {:?}",
                    component.name(),
                    res.fail
                ));
            }
        }

        if !self.unchecked_funcs.is_empty() {
            let names: Vec<&Path> = self
                .unchecked_funcs
                .iter()
                .map(|f| &f.metadata.name)
                .collect();
            Err(anyhow::anyhow!("Unchecked functions remain: {:?}", names))
        } else {
            Ok(())
        }
    }

    /// Print current state of the checker
    pub fn print_state(&self) {
        println!("Checker state:");
        println!("  Verified: {:?}", self.verified_funcs);
        println!("  Tested: {:?}", self.tested_funcs);
        println!("  Unchecked: {:?}", self.unchecked_funcs);
        println!("  Source1 Unique: {:?}", self.src1.unique_funcs);
        println!("  Source2 Unique: {:?}", self.src2.unique_funcs);
    }

    /// Preprocess before running checks. Match functions with the same signature in both sources.
    fn preprocess(&mut self) {
        // Find common functions by signature
        for func in &self.src1.unique_funcs {
            if let Some(func2) = self
                .src2
                .unique_funcs
                .iter()
                .find(|func2| func.metadata.signature == func2.metadata.signature)
            {
                self.unchecked_funcs.push(CommonFunction::new(
                    func.metadata.clone(),
                    func.body.clone(),
                    func2.body.clone(),
                ));
            }
        }
        // Remove common functions from unique lists
        self.src1.unique_funcs.retain(|func| {
            !self
                .unchecked_funcs
                .iter()
                .any(|func2| func.metadata.name == func2.metadata.name)
        });
        self.src2.unique_funcs.retain(|func| {
            !self
                .unchecked_funcs
                .iter()
                .any(|func2| func.metadata.name == func2.metadata.name)
        });
    }
}

/// Typed check result
#[derive(Debug)]
pub struct CheckResult {
    /// Overall status (e.g., any fatal error that prevented full checking)
    pub status: anyhow::Result<()>,
    /// Functions that passed the consistency check
    pub ok: Vec<Path>,
    /// Functions that failed the consistency check
    pub fail: Vec<Path>,
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

/// A single check component, either formal or testing-based.
pub trait Component {
    /// Name of the component.
    fn name(&self) -> &str;

    /// If this component is a formal checker.
    fn is_formal(&self) -> bool;

    /// Additional note to print.
    fn note(&self) -> Option<&str> {
        None
    }

    /// Run the check component.
    fn run(&self, checker: &Checker) -> CheckResult;
}
