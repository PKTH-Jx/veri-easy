use anyhow::Result;
use std::fs;

use crate::function::{Function, collect_functions};

/// Represent a Rust source file
pub struct Source {
    /// Path to the file
    pub path: String,
    /// Full text content
    pub content: String,
    /// Unique functions (exist only in one file)
    pub unique_funcs: Vec<Function>,
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
            unique_funcs: funcs,
        })
    }
}
