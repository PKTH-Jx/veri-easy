use anyhow::{Result, anyhow};
use prettyplease;
use std::{collections::BTreeMap, fmt::Debug};
use syn::{
    File, FnArg, ImplItemFn, ItemFn, ItemImpl, ReturnType, Signature, Type,
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
    pub fn _ident(&self) -> String {
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

    /// Equal by Signature
    pub fn sig_eq(&self, other: &Self) -> bool {
        let sig1 = &self.item.sig;
        let sig2 = &other.item.sig;

        sig1.ident == sig2.ident
            && sig1.inputs.len() == sig2.inputs.len()
            && sig1
                .inputs
                .iter()
                .zip(sig2.inputs.iter())
                .all(|(a, b)| match (a, b) {
                    (syn::FnArg::Receiver(_), syn::FnArg::Receiver(_)) => true,
                    (syn::FnArg::Typed(a), syn::FnArg::Typed(b)) => type_eq(&a.ty, &b.ty),
                    _ => false,
                })
            && match (&sig1.output, &sig2.output) {
                (syn::ReturnType::Default, syn::ReturnType::Default) => true,
                (syn::ReturnType::Type(_, a), syn::ReturnType::Type(_, b)) => type_eq(a, b),
                _ => false,
            }
    }
}

impl Debug for Function {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.name)
    }
}

/// Function shared by 2 source files, with same signature but different bodies.
#[derive(Clone)]
pub struct CommonFunction {
    pub f1: Function,
    pub f2: Function,
}

impl CommonFunction {
    pub fn new(f1: Function, f2: Function) -> Result<Self> {
        if !f1.sig_eq(&f2) {
            Err(anyhow!("Functions have different signatures"))
        } else {
            Ok(Self { f1, f2 })
        }
    }

    pub fn name(&self) -> &str {
        &self.f1.name
    }

    pub fn scope(&self) -> String {
        self.f1.scope()
    }

    pub fn sig(&self) -> &Signature {
        &self.f1.item.sig
    }
}

impl Debug for CommonFunction {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.name())
    }
}

/// Visitor that collects free functions and impl methods.
struct FunctionCollector {
    funcs: Vec<Function>,
    scope_stack: Vec<String>,
    impl_block: Option<ItemImpl>,
}

impl FunctionCollector {
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

impl<'ast> Visit<'ast> for FunctionCollector {
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
    let mut collector = FunctionCollector::new();
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

/// Check if two types are equal
fn type_eq(a: &Type, b: &Type) -> bool {
    type_to_string(a, "::") == type_to_string(b, "::")
}

/// Used to classify functions into free-standing functions, methods and constructors.
pub struct FunctionClassifier {
    /// Free-standing functions.
    pub functions: Vec<CommonFunction>,
    /// Methods (with `self` receiver).
    pub methods: Vec<CommonFunction>,
    /// Constructors (return `Self` type).
    pub constructors: BTreeMap<String, CommonFunction>,
}

impl FunctionClassifier {
    /// Classify functions into free-standing functions, methods and constructors.
    pub fn classify(functions: Vec<CommonFunction>) -> Self {
        let mut res = Self {
            functions: Vec::new(),
            methods: Vec::new(),
            constructors: BTreeMap::new(),
        };
        for func in functions {
            if let Some(impl_block) = &func.f1.impl_block {
                // The name of the struct
                let struct_name = match &*impl_block.self_ty {
                    Type::Path(type_path) => type_path.path.get_ident(),
                    _ => None,
                };
                if func
                    .sig()
                    .inputs
                    .iter()
                    .any(|arg| matches!(arg, FnArg::Receiver(_)))
                {
                    // Has `self` receiver, consider it as a method.
                    res.methods.push(func);
                } else {
                    if let ReturnType::Type(_, rt) = &func.sig().output {
                        if let Type::Path(type_path) = &**rt {
                            if type_path.path.is_ident("Self") {
                                // Return `Self` type, consider it as a constructor.
                                res.constructors.insert(func.scope(), func);
                                continue;
                            } else if let Some(name) = struct_name {
                                if type_path.path.is_ident(name) {
                                    // Return `struct_name` type, consider it as a constructor.
                                    res.constructors.insert(func.scope(), func);
                                    continue;
                                }
                            }
                        }
                    }
                    // No `self` receiver and not return `Self` type, consider it as a free-standing function.
                    res.functions.push(func);
                }
            } else {
                // Function outside of impl block is a free-standing function.
                res.functions.push(func);
            }
        }
        res
    }
}
