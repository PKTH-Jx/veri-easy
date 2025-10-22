use anyhow::Result;
use prettyplease;
use std::fmt::Debug;
use syn::{
    Attribute, File, ImplItemFn, ItemFn, ItemImpl, Type,
    visit::{self, Visit},
    visit_mut::{self, VisitMut},
};

/// Function identity + AST payload. Hash/Eq by `name` only.
#[derive(Clone)]
pub struct Function {
    /// Fully-qualified name, e.g. "foo" or "MyType::bar" or "crate::module::MyType::bar"
    pub name: String,
    pub item: ItemFn,
}

impl Function {
    pub fn new(name: String, item: ItemFn) -> Self {
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

/// Visitor that collects free functions and impl methods.
struct FnCollector {
    funcs: Vec<Function>,
    scope_stack: Vec<String>,
}

impl FnCollector {
    fn new() -> Self {
        Self {
            funcs: Vec::new(),
            scope_stack: Vec::new(),
        }
    }
    fn into_vec(self) -> Vec<Function> {
        self.funcs
    }
}

impl<'ast> Visit<'ast> for FnCollector {
    fn visit_item_fn(&mut self, node: &'ast ItemFn) {
        let name = node.sig.ident.to_string();
        self.funcs.push(Function::new(name, node.clone()));
        visit::visit_item_fn(self, node);
    }

    fn visit_item_impl(&mut self, node: &'ast ItemImpl) {
        self.scope_stack.push(type_to_string(&node.self_ty, "::"));
        visit::visit_item_impl(self, node);
        self.scope_stack.pop();
    }

    fn visit_impl_item_fn(&mut self, node: &'ast ImplItemFn) {
        let name = self.scope_stack.join("::") + "::" + &node.sig.ident.to_string();
        self.funcs.push(Function {
            name,
            item: ItemFn {
                attrs: node.attrs.clone(),
                vis: node.vis.clone(),
                sig: node.sig.clone(),
                block: Box::new(node.block.clone()),
            },
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

/// Visitor that sets `#[export_name = "..."]` on functions and impl methods.
struct FnExporter {
    scope_stack: Vec<String>,
}

impl FnExporter {
    fn new() -> Self {
        Self {
            scope_stack: Vec::new(),
        }
    }
}

impl VisitMut for FnExporter {
    fn visit_item_fn_mut(&mut self, node: &mut ItemFn) {
        if node.sig.generics.lt_token.is_none() {
            let name = node.sig.ident.to_string();
            let attr: Attribute = syn::parse_quote!(#[export_name = #name]);
            node.attrs.push(attr);
        }
        // skip function with generic params
        visit_mut::visit_item_fn_mut(self, node);
    }

    fn visit_item_impl_mut(&mut self, node: &mut ItemImpl) {
        if node.generics.lt_token.is_none() {
            self.scope_stack.push(type_to_string(&node.self_ty, "___"));
            visit_mut::visit_item_impl_mut(self, node);
            self.scope_stack.pop();
        }
        // skip impl block with generic params
    }

    fn visit_impl_item_fn_mut(&mut self, node: &mut ImplItemFn) {
        let name = self.scope_stack.join("___") + "___" + &node.sig.ident.to_string();
        let attr: Attribute = syn::parse_quote!(#[export_name = #name]);
        node.attrs.push(attr);
        visit_mut::visit_impl_item_fn_mut(self, node);
    }
}

/// Add `#[export_name = "..."]` to all functions and impl methods
pub fn export_functions(src: &str) -> Result<String> {
    let mut syntax: File = syn::parse_file(src)?;
    let mut exporter = FnExporter::new();
    exporter.visit_file_mut(&mut syntax);
    Ok(prettyplease::unparse(&syntax))
}

/// Convert a type to a string
fn type_to_string(ty: &Type, sep: &str) -> String {
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
