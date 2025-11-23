//! Veri-easy functional equivalence checker.
use anyhow::Error;

use crate::{
    collect::{FunctionCollector, PathResolver, SymbolCollector, TypeCollector},
    defs::{CommonFunction, Function, InstantiatedType, Path, PreciseType, Precondition, Type},
    log,
};

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
    /// Instantiated generic types.
    pub inst_types: Vec<InstantiatedType>,
}

impl Source {
    /// Open a source file from path and parse its content.
    pub fn open(path: &str) -> anyhow::Result<Self> {
        let content =
            std::fs::read_to_string(&path).map_err(|_| anyhow::anyhow!("Failed to read source"))?;
        let mut syntax = syn::parse_file(&content)
            .map_err(|_| anyhow::anyhow!("Failed to parse source file"))?;

        // Resolve paths
        PathResolver::new().resolve_paths(&mut syntax);
        // Collect functions
        let unique_funcs = FunctionCollector::new().collect(&syntax);
        // Collect symbols
        let symbols = SymbolCollector::new().collect(&syntax);
        // Collect instantiated generic types
        let inst_types = TypeCollector::new().collect(&syntax);

        Ok(Self {
            path: path.to_owned(),
            content,
            unique_funcs,
            symbols,
            inst_types,
        })
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
    /// Constructors (not checked directly).
    pub constructors: Vec<CommonFunction>,
    /// Getters (not checked directly).
    pub getters: Vec<CommonFunction>,
    /// Preconditions (used to filter out tests that do not satisfy preconditions).
    pub preconditions: Vec<Precondition>,
}

impl Checker {
    pub fn new(
        src1: Source,
        src2: Source,
        steps: Vec<Box<dyn Component>>,
        preconditions: Vec<Precondition>,
    ) -> Self {
        let mut checker = Self {
            src1,
            src2,
            components: steps,
            verified_funcs: Vec::new(),
            unchecked_funcs: Vec::new(),
            tested_funcs: Vec::new(),
            constructors: Vec::new(),
            getters: Vec::new(),
            preconditions,
        };
        checker.preprocess();
        checker
    }

    /// Run all steps in order
    pub fn run_all(&mut self) {
        for component in &self.components {
            match component.note() {
                Some(note) => log!(
                    Brief,
                    Critical,
                    "Running component `{}`: {}",
                    component.name(),
                    note
                ),
                None => log!(Brief, Critical, "Running component `{}`", component.name()),
            }

            let res = component.run(&self);
            if let Err(e) = res.status {
                log!(
                    Brief,
                    Error,
                    "Component `{}` failed to execute: {}",
                    component.name(),
                    e
                );
                continue;
            }
            log!(
                Brief,
                Critical,
                "Component `{}` completed.",
                component.name()
            );

            for name in &res.ok {
                log!(Brief, Ok, "`{:?}` passed", name);
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

            if !res.fail.is_empty() {
                for name in &res.fail {
                    log!(Brief, Error, "`{:?}` failed", name);
                }
                log!(
                    Brief,
                    Error,
                    "Step `{}` found inconsistencies.",
                    component.name()
                );
                self.print_state();
                break;
            }
            log!(
                Normal,
                Info,
                "State after component `{}`:",
                component.name()
            );
            self.print_state();
            log!(Brief, Simple, "");
        }

        if !self.unchecked_funcs.is_empty() {
            let names: Vec<&Path> = self
                .unchecked_funcs
                .iter()
                .map(|f| &f.metadata.name)
                .collect();
            log!(Brief, Error, "Unchecked functions remain: {:?}", names);
        } else {
            log!(Brief, Ok, "All functions have been checked.");
        }
    }

    /// Print current state of the checker
    pub fn print_state(&self) {
        log!(Normal, Info, "  Verified: {:?}", self.verified_funcs);
        log!(Normal, Info, "  Tested: {:?}", self.tested_funcs);
        log!(Normal, Info, "  Unchecked: {:?}", self.unchecked_funcs);
        log!(
            Verbose,
            Info,
            "  Source 1 unique funcs: {:?}",
            self.src1.unique_funcs
        );
        log!(
            Verbose,
            Info,
            "  Source 2 unique funcs: {:?}",
            self.src2.unique_funcs
        );
    }

    /// Preprocess before running checks. Match functions with the same signature in both sources.
    fn preprocess(&mut self) {
        let mut common_funcs = Vec::new();

        // Find common functions by signature
        for func in &self.src1.unique_funcs {
            if let Some(func2) = self
                .src2
                .unique_funcs
                .iter()
                .find(|func2| func.metadata.signature == func2.metadata.signature)
            {
                common_funcs.push(CommonFunction::new(
                    func.metadata.clone(),
                    func.body.clone(),
                    func2.body.clone(),
                ));
            }
        }

        // Remove common functions from unique lists
        self.src1.unique_funcs.retain(|func| {
            !common_funcs
                .iter()
                .any(|func2| func.metadata.name == func2.metadata.name)
        });
        self.src2.unique_funcs.retain(|func| {
            !common_funcs
                .iter()
                .any(|func2| func.metadata.name == func2.metadata.name)
        });

        // Get the common instantiated generic types
        let mut common_inst_types = Vec::new();
        for inst_type in &self.src1.inst_types {
            if let Some(_) = self
                .src2
                .inst_types
                .iter()
                .find(|inst_type2| inst_type == *inst_type2)
            {
                common_inst_types.push(inst_type.clone());
            }
        }

        // If a common function has name `Foo<T>::foo()`, and there is an instantiated
        // type `FB = Foo<Bar>`, We need to replace `Foo<T>::foo()` with `FB::foo()`
        // in the common functions.
        let mut updated_common_funcs = Vec::new();
        for func in common_funcs {
            let mut renamed = false;
            if let Some(impl_type) = &func.metadata.impl_type {
                // Check against instantiated types
                for inst_type in &self.src1.inst_types {
                    if inst_type.concrete.eq_ignore_generics(impl_type) {
                        let mut func = func.clone();
                        // Update the impl_type to the instantiated alias type
                        func.metadata.impl_type =
                            Some(Type::Precise(PreciseType(inst_type.alias.clone())));
                        func.metadata.name = inst_type.alias.clone().join(func.metadata.ident());
                        updated_common_funcs.push(func);
                        renamed = true;
                    }
                }
            }
            if !renamed {
                updated_common_funcs.push(func);
            }
        }

        // Update precondition check functions similarly
        let mut updated_preconditions = Vec::new();
        for func in &self.preconditions {
            let mut renamed = false;
            if let Some(impl_type) = &func.impl_type {
                // Check against instantiated types
                for inst_type in &self.src1.inst_types {
                    if inst_type.concrete.eq_ignore_generics(impl_type) {
                        let mut func = func.clone();
                        // Update the impl_type to the instantiated alias type
                        func.impl_type = Some(Type::Precise(PreciseType(inst_type.alias.clone())));
                        func.name = inst_type.alias.clone().join(func.ident());
                        updated_preconditions.push(func);
                        renamed = true;
                    }
                }
            }
            if !renamed {
                updated_preconditions.push(func.clone());
            }
        }
        self.preconditions = updated_preconditions;

        // Get constructor functions (`verieasy_new`) from common functions
        self.constructors = updated_common_funcs
            .iter()
            .filter(|f| f.metadata.is_constructor())
            .cloned()
            .collect();
        // Get getter functions (`verieasy_get`) from common functions
        self.getters = updated_common_funcs
            .iter()
            .filter(|f| f.metadata.is_getter())
            .cloned()
            .collect();

        updated_common_funcs.retain(|f| !f.metadata.is_constructor() && !f.metadata.is_getter());
        self.unchecked_funcs = updated_common_funcs;

        println!("{:?}", self.preconditions);
    }
}
