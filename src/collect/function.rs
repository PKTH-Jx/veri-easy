//! Collect functions from a Rust program.

use super::path::PathResolver;
use crate::defs::{Path, Type};
use syn::{
    Block, File, ImplItemFn, ItemFn, ItemImpl, ItemMod, ItemUse, Signature,
    visit::{self, Visit},
};

/// Represent a function parsed from source code.
struct Function {
    /// Fully qualified name of the function.
    name: Path,
    /// Function signature.
    signature: Signature,
    /// The impl type if it's an impl method.
    impl_type: Option<Type>,
    /// The trait if it's an impl method for a trait.
    trait_: Option<Path>,
    /// Function body.
    body: Block,
}

/// Visitor that collects free functions and impl methods.
pub struct FunctionCollector<'ast> {
    /// Collected functions.
    functions: Vec<Function>,
    /// Currently visited impl block.
    impl_block: Option<&'ast ItemImpl>,
    /// Path resolver
    resolver: PathResolver,
}

impl<'ast> FunctionCollector<'ast> {
    pub fn new() -> Self {
        Self {
            functions: Vec::new(),
            impl_block: None,
            resolver: PathResolver::new(),
        }
    }
    pub fn collect(mut self, syntax: &'ast File) -> Vec<crate::defs::Function> {
        self.visit_file(syntax);

        let mut functions = Vec::new();
        for func in self.functions {
            let body = func.body;
            functions.push(crate::defs::Function::new(
                crate::defs::FunctionMetadata::new(
                    func.name,
                    crate::defs::Signature(func.signature),
                    func.impl_type,
                    func.trait_,
                ),
                quote::quote! { #body }.to_string(),
            ));
        }
        functions
    }
}

impl<'ast> Visit<'ast> for FunctionCollector<'ast> {
    fn visit_item_mod(&mut self, i: &'ast ItemMod) {
        self.resolver.enter_module(i);
        visit::visit_item_mod(self, i);
        self.resolver.exit_module();
    }

    fn visit_item_use(&mut self, i: &'ast ItemUse) {
        self.resolver.parse_use_tree(&i.tree, Path::empty());
    }

    fn visit_item_fn(&mut self, i: &'ast ItemFn) {
        if !i.sig.generics.params.is_empty() {
            return;
        } // Skip generic functions
        let name = self.resolver.concat_module(&i.sig.ident.to_string());
        self.functions.push(Function {
            name,
            signature: i.sig.clone(),
            impl_type: None,
            trait_: None,
            body: (*i.block).clone(),
        });
    }

    fn visit_item_impl(&mut self, i: &'ast ItemImpl) {
        self.impl_block = Some(i);
        visit::visit_item_impl(self, i);
        self.impl_block = None;
    }

    fn visit_impl_item_fn(&mut self, i: &'ast ImplItemFn) {
        if !i.sig.generics.params.is_empty() {
            return;
        } // Skip generic functions
        let impl_block = self.impl_block.cloned().unwrap();
        if let Ok(mut self_ty) = Type::try_from(*impl_block.self_ty) {
            match &mut self_ty {
                Type::Generic(g) => g.path = self.resolver.resolve_path(&g.path),
                Type::Precise(p) => p.0 = self.resolver.resolve_path(&p.0),
            }
            let name = self_ty.as_path().join(i.sig.ident.to_string());
            self.functions.push(Function {
                name,
                impl_type: Some(self_ty),
                trait_: impl_block.trait_.map(|(_, path, _)| path.into()),
                signature: i.sig.clone(),
                body: i.block.clone(),
            });
        }
    }
}
