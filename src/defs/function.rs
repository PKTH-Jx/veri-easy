use super::path::Path;
use super::types::Type;
use std::fmt::Debug;

/// Wrap `syn::Signature`.
#[derive(Clone)]
pub struct Signature(pub syn::Signature);

impl PartialEq for Signature {
    fn eq(&self, other: &Self) -> bool {
        self.0.ident == other.0.ident
            && self.0.inputs.len() == other.0.inputs.len()
            && self
                .0
                .inputs
                .iter()
                .zip(other.0.inputs.iter())
                .all(|(a, b)| match (a, b) {
                    (syn::FnArg::Receiver(_), syn::FnArg::Receiver(_)) => true,
                    (syn::FnArg::Typed(a), syn::FnArg::Typed(b)) => type_eq(&a.ty, &b.ty),
                    _ => false,
                })
            && match (&self.0.output, &other.0.output) {
                (syn::ReturnType::Default, syn::ReturnType::Default) => true,
                (syn::ReturnType::Type(_, a), syn::ReturnType::Type(_, b)) => type_eq(a, b),
                _ => false,
            }
    }
}

/// Function metadata, including name, signature, impl type and trait (if any).
#[derive(Clone)]
pub struct FunctionMetadata {
    /// Fully-qualified name, e.g. "foo" or "MyType::bar" or "module::MyType::bar"
    pub name: Path,
    /// Function signature.
    pub signature: Signature,
    /// If the function is an impl method, the impl type.
    pub impl_type: Option<Type>,
    /// If the function is implemented against a trait, the trait name.
    pub trait_: Option<Path>,
}

impl FunctionMetadata {
    /// Create a new FunctionMetadata.
    pub fn new(
        name: Path,
        signature: Signature,
        impl_type: Option<Type>,
        trait_: Option<Path>,
    ) -> Self {
        Self {
            name,
            signature,
            impl_type,
            trait_,
        }
    }
    /// Get the function identifier.
    pub fn ident(&self) -> String {
        self.signature.0.ident.to_string()
    }
}

impl Debug for FunctionMetadata {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{:?}", self.name)
    }
}

/// Function metadata and body.
pub struct Function {
    /// Metadata of the function.
    pub metadata: FunctionMetadata,
    /// Function body.
    pub body: String,
}

impl Function {
    /// Create a new Function.
    pub fn new(metadata: FunctionMetadata, body: String) -> Self {
        Self { metadata, body }
    }
}

impl Debug for Function {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{:?}", self.metadata.name)
    }
}

/// Function shared by 2 source files, with same metadata but different bodies.
#[derive(Clone)]
pub struct CommonFunction {
    /// Metadata of the function.
    pub metadata: FunctionMetadata,
    /// Body from first source file.
    pub body1: String,
    /// Body from second source file.
    pub body2: String,
}

impl CommonFunction {
    /// Create a new CommonFunction.
    pub fn new(metadata: FunctionMetadata, body1: String, body2: String) -> Self {
        Self {
            metadata,
            body1,
            body2,
        }
    }
    /// Get the implementation type unchecked.
    pub fn impl_type(&self) -> &Type {
        self.metadata.impl_type.as_ref().unwrap()
    }
}

impl Debug for CommonFunction {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{:?}", self.metadata.name)
    }
}

/// Convert a type to a string
fn type_to_string(ty: &syn::Type, sep: &str) -> String {
    match ty {
        syn::Type::Path(tp) => tp
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
fn type_eq(a: &syn::Type, b: &syn::Type) -> bool {
    type_to_string(a, "::") == type_to_string(b, "::")
}
