use anyhow::Result;
use prettyplease;
use std::fmt::Debug;
use syn::{
    File, ItemFn,
    visit::{self, Visit},
};

/// Function identity + AST payload. Hash/Eq by `name` only.
#[derive(Clone)]
pub struct Function {
    /// Fully-qualified name, e.g. "foo" or "MyType::bar" or "crate::module::MyType::bar"
    pub name: String,
    pub item: ItemFn,
}

impl Function {
    pub fn new_free(name: String, item: ItemFn) -> Self {
        Self { name, item }
    }

    /// Pretty-print the function body
    pub fn body(&self) -> String {
        prettyplease::unparse(&File {
            shebang: None,
            attrs: Vec::new(),
            items: vec![syn::Item::Fn(self.item.clone())],
        })
    }

    /// Eq by both `name` and `body`
    pub fn eq(&self, other: &Self) -> bool {
        self.name == other.name && self.body() == other.body()
    }
}

impl Debug for Function {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.name)
    }
}

/// Visitor that collects free functions and impl methods, tracking current impl target.
struct FnCollector {
    funcs: Vec<Function>,
}

impl FnCollector {
    fn new() -> Self {
        Self { funcs: Vec::new() }
    }
    fn into_vec(self) -> Vec<Function> {
        self.funcs
    }
}

impl<'ast> Visit<'ast> for FnCollector {
    fn visit_item_fn(&mut self, node: &'ast ItemFn) {
        let name = node.sig.ident.to_string();
        self.funcs.push(Function::new_free(name, node.clone()));
        // continue walking inside (in case nested items exist)
        visit::visit_item_fn(self, node);
    }
}

/// Parse a file and extract functions
pub fn extract_functions(src: &str) -> Result<Vec<Function>> {
    let syntax: File = syn::parse_file(src)?;
    let mut collector = FnCollector::new();
    collector.visit_file(&syntax);
    Ok(collector.into_vec())
}
