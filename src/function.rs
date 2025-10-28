use anyhow::Result;
use prettyplease;
use std::fmt::Debug;
use syn::{
    File, ImplItemFn, ItemFn, ItemImpl, Type,
    visit::{self, Visit},
};

/// Function identity + AST payload. Hash/Eq by `name` only.
#[derive(Clone)]
pub struct Function {
    /// Fully-qualified name, e.g. "foo" or "MyType::bar" or "module::MyType::bar"
    pub name: String,
    pub item: ItemFn,
    /// If the function is an impl method, the whole impl block.
    pub impl_block: Option<ItemImpl>,
}

impl Function {
    pub fn new(name: String, item: ItemFn, impl_block: Option<ItemImpl>) -> Self {
        Self {
            name,
            item,
            impl_block,
        }
    }

    /// Get the scope of the function
    pub fn scope(&self) -> String {
        let mut scope = self.name.split("::").collect::<Vec<_>>();
        scope.pop();
        scope.join("::")
    }

    /// Get the identifier of the function
    pub fn ident(&self) -> String {
        self.name.split("::").last().unwrap_or("").to_string()
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

/// Visitor that collects free functions and impl methods.
struct FnCollector {
    funcs: Vec<Function>,
    scope_stack: Vec<String>,
    impl_block: Option<ItemImpl>,
}

impl FnCollector {
    fn new() -> Self {
        Self {
            funcs: Vec::new(),
            scope_stack: Vec::new(),
            impl_block: None,
        }
    }
    fn into_vec(self) -> Vec<Function> {
        self.funcs
    }
    fn concat_name(&self, name: &str) -> String {
        if self.scope_stack.is_empty() {
            name.to_string()
        } else {
            self.scope_stack.join("::") + "::" + name
        }
    }
}

impl<'ast> Visit<'ast> for FnCollector {
    fn visit_item_fn(&mut self, node: &'ast ItemFn) {
        if !node.sig.generics.params.is_empty() {
            return;
        } // Skip generic functions
        let name = self.concat_name(&node.sig.ident.to_string());
        self.funcs
            .push(Function::new(name, node.clone(), self.impl_block.clone()));
        visit::visit_item_fn(self, node);
    }

    fn visit_item_mod(&mut self, node: &'ast syn::ItemMod) {
        self.scope_stack.push(node.ident.to_string());
        visit::visit_item_mod(self, node);
        self.scope_stack.pop();
    }

    fn visit_item_impl(&mut self, node: &'ast ItemImpl) {
        self.scope_stack.push(type_to_string(&node.self_ty, "::"));
        self.impl_block = Some(node.clone());
        visit::visit_item_impl(self, node);
        self.scope_stack.pop();
        self.impl_block = None;
    }

    fn visit_impl_item_fn(&mut self, node: &'ast ImplItemFn) {
        if !node.sig.generics.params.is_empty() {
            return;
        } // Skip generic functions
        let name = self.concat_name(&node.sig.ident.to_string());
        self.funcs.push(Function {
            name,
            item: ItemFn {
                attrs: node.attrs.clone(),
                vis: node.vis.clone(),
                sig: node.sig.clone(),
                block: Box::new(node.block.clone()),
            },
            impl_block: self.impl_block.clone(),
        });
        visit::visit_impl_item_fn(self, node);
    }
}

/// Parse a file and collect functions
pub fn collect_functions(src: &str) -> Result<Vec<Function>> {
    let syntax: File = syn::parse_file(src)?;
    let mut collector = FnCollector::new();
    collector.visit_file(&syntax);
    Ok(collector.into_vec())
}

/// Convert a type to a string
pub fn type_to_string(ty: &Type, sep: &str) -> String {
    match ty {
        Type::Path(tp) => tp
            .path
            .segments
            .iter()
            .map(|seg| seg.ident.to_string())
            .collect::<Vec<_>>()
            .join(sep),
        _ => "unsupported".to_owned(),
    }
}
