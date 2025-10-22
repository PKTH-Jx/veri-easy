use anyhow::Result;
use std::fmt::Debug;
use std::fs;

use crate::function::{Function, extract_functions};

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
        let funcs = extract_functions(&content)?;
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
        if let Some(func) = self.unchecked_func(name) {
            self.checked_funcs.push(func.clone());
            self.unchecked_funcs.retain(|f| f.name != name);
        }
    }

    /// Ignore functions that only appear in one of the sources
    pub fn set_ignored(src1: &mut Self, src2: &mut Self) {
        let ignored1: Vec<_> = src1
            .unchecked_funcs
            .iter()
            .filter(|f| src2.unchecked_func(&f.name).is_none())
            .cloned()
            .collect();
        let ignored2: Vec<_> = src2
            .unchecked_funcs
            .iter()
            .filter(|f| src1.unchecked_func(&f.name).is_none())
            .cloned()
            .collect();

        src1.ignored_funcs = ignored1;
        src2.ignored_funcs = ignored2;
        src1.unchecked_funcs
            .retain(|f| src2.unchecked_func(&f.name).is_some());
        src2.unchecked_funcs
            .retain(|f| src1.unchecked_func(&f.name).is_some());
    }

    /// Find a function by name
    pub fn unchecked_func(&self, name: &str) -> Option<&Function> {
        self.unchecked_funcs.iter().find(|f| f.name == name)
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
