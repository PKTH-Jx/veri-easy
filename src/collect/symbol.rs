//! Collect imports from a Rust program.
use syn::{
    ItemMod, ItemTrait, ItemUse,
    visit::{self, Visit},
};

use super::path::PathResolver;
use crate::defs::Path;

/// Visitor that collects traits.
pub struct TraitCollector {
    /// Collected traits.
    traits: Vec<Path>,
    /// Path resolver.
    resolver: PathResolver,
}

impl TraitCollector {
    /// Create a new trait collector.
    pub fn new() -> Self {
        Self {
            traits: Vec::new(),
            resolver: PathResolver::new(),
        }
    }
    /// Collect traits from the syntax tree.
    pub fn collect(mut self, syntax: &syn::File) -> Vec<Path> {
        self.visit_file(syntax);
        self.traits
    }
}

impl<'ast> Visit<'ast> for TraitCollector {
    fn visit_item_mod(&mut self, i: &'ast ItemMod) {
        self.resolver.enter_module(i);
        visit::visit_item_mod(self, i);
        self.resolver.exit_module();
    }

    fn visit_item_use(&mut self, i: &'ast ItemUse) {
        self.resolver.parse_use_tree(&i.tree, Path::empty());
    }

    fn visit_item_trait(&mut self, i: &'ast ItemTrait) {
        let trait_path = self.resolver.concat_module(&i.ident.to_string());
        self.traits.push(trait_path);
    }
}
