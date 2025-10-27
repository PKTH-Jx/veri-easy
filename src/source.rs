use anyhow::Result;
use std::fmt::Debug;
use std::fs;

use crate::function::{Function, collect_functions, type_to_string};

/// Represent a Rust source file
pub struct Source {
    /// Path to the file
    pub path: String,
    /// Full text content
    pub content: String,
    /// Functions already verified
    pub checked_funcs: Vec<Function>,
    /// Functions pending verification
    pub unchecked_funcs: Vec<Function>,
    /// Functions ignored (exist only in one file)
    pub ignored_funcs: Vec<Function>,
}

impl Source {
    /// Load a Rust file and extract its functions using `syn`
    pub fn new(path: impl Into<String>) -> Result<Self> {
        let path = path.into();
        let content = fs::read_to_string(&path)?;
        let funcs = collect_functions(&content)?;
        Ok(Self {
            path,
            content,
            checked_funcs: Vec::new(),
            unchecked_funcs: funcs,
            ignored_funcs: Vec::new(),
        })
    }

    /// Mark a function as checked
    pub fn set_checked(&mut self, name: &str) {
        if let Some(func) = self.unchecked_func_by_name(name) {
            self.checked_funcs.push(func.clone());
            self.unchecked_funcs.retain(|f| f.name != name);
        }
    }

    /// Ignore functions that only appear in one of the sources
    pub fn set_ignored(src1: &mut Self, src2: &mut Self) {
        let ignored1: Vec<_> = src1
            .unchecked_funcs
            .iter()
            .filter(|f| src2.unchecked_func_by_signature(&f).is_none())
            .cloned()
            .collect();
        let ignored2: Vec<_> = src2
            .unchecked_funcs
            .iter()
            .filter(|f| src1.unchecked_func_by_signature(&f).is_none())
            .cloned()
            .collect();

        src1.ignored_funcs = ignored1;
        src2.ignored_funcs = ignored2;
        src1.unchecked_funcs
            .retain(|f| src2.unchecked_func_by_signature(&f).is_some());
        src2.unchecked_funcs
            .retain(|f| src1.unchecked_func_by_signature(&f).is_some());
    }

    /// Find an unchecked function by name
    pub fn unchecked_func_by_name(&self, name: &str) -> Option<&Function> {
        self.unchecked_funcs.iter().find(|f| f.name == name)
    }

    /// Find an unchecked function that has the same signature as `func`, i.e.
    /// same name, same parameters and same return type
    pub fn unchecked_func_by_signature(&self, func: &Function) -> Option<&Function> {
        self.unchecked_funcs.iter().find(|f| {
            f.name == func.name
                && f.item.sig.inputs.len() == func.item.sig.inputs.len()
                && f.item
                    .sig
                    .inputs
                    .iter()
                    .zip(func.item.sig.inputs.iter())
                    .all(|(a, b)| match (a, b) {
                        (syn::FnArg::Receiver(_), syn::FnArg::Receiver(_)) => true,
                        (syn::FnArg::Typed(a), syn::FnArg::Typed(b)) => type_eq(&a.ty, &b.ty),
                        _ => false,
                    })
                && match (&f.item.sig.output, &func.item.sig.output) {
                    (syn::ReturnType::Default, syn::ReturnType::Default) => true,
                    (syn::ReturnType::Type(_, a), syn::ReturnType::Type(_, b)) => type_eq(a, b),
                    _ => false,
                }
        })
    }
}

impl Debug for Source {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Source")
            .field("path", &self.path)
            .field("checked_funcs", &self.checked_funcs)
            .field("unchecked_funcs", &self.unchecked_funcs)
            .field("ignored_funcs", &self.ignored_funcs)
            .finish()
    }
}

fn type_eq(a: &syn::Type, b: &syn::Type) -> bool {
    type_to_string(a, "::") == type_to_string(b, "::")
}
