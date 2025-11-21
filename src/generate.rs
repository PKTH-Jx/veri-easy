//! Harness generator used by various steps (Kani, PBT, DFT).
use proc_macro2::TokenStream;
use quote::{format_ident, quote};
use std::collections::BTreeMap;

use crate::{
    defs::{CommonFunction, Path, Type},
    log,
};

/// Used to classify functions into
///
/// - Free-standing functions (without `self` receiver)
/// - methods (with `self` receiver)
/// - constructors (functions that has name `verieasy_new` inside an `impl` block)
/// - state getters (functions that has name `verieasy_get` inside an `impl` block)
#[derive(Debug)]
pub struct FunctionClassifier {
    /// Free-standing functions.
    pub functions: Vec<CommonFunction>,
    /// Methods.
    pub methods: Vec<CommonFunction>,
    /// Constructors mapped by their type.
    pub constructors: BTreeMap<Type, CommonFunction>,
    /// State getters mapped by their type.
    pub getters: BTreeMap<Type, CommonFunction>,
}

impl FunctionClassifier {
    /// Classify functions into free-standing functions, methods and constructors.
    pub fn classify(functions: Vec<CommonFunction>) -> Self {
        let mut res = Self {
            functions: Vec::new(),
            methods: Vec::new(),
            constructors: BTreeMap::new(),
            getters: BTreeMap::new(),
        };
        for func in functions {
            if let Some(impl_type) = &func.metadata.impl_type {
                if func.metadata.ident() == "verieasy_new" {
                    // Constructor
                    res.constructors.insert(impl_type.clone(), func);
                    continue;
                } else if func.metadata.ident() == "verieasy_get" {
                    // State getter
                    res.getters.insert(impl_type.clone(), func);
                    continue;
                }

                if func
                    .metadata
                    .signature
                    .0
                    .inputs
                    .iter()
                    .any(|arg| matches!(arg, syn::FnArg::Receiver(_)))
                {
                    // Has `self` receiver, consider it as a method.
                    res.methods.push(func);
                } else {
                    // No `self` receiver, consider it as a free-standing function.
                    res.functions.push(func);
                }
            } else {
                // Function outside of impl block is a free-standing function.
                res.functions.push(func);
            }
        }
        res
    }

    /// If `methods` doesn't have a method of type `T`, then its constructor and getter asre unused.
    ///
    /// This function removes those constructors and getters.
    pub fn remove_unused_constructors_and_getters(&mut self) {
        let mut unused_types = Vec::new();
        for (type_, _) in &self.constructors {
            if !self
                .methods
                .iter()
                .any(|method| method.metadata.impl_type.as_ref() == Some(type_))
            {
                unused_types.push(type_.clone());
            }
        }
        for type_ in &unused_types {
            log!(
                Verbose,
                Warning,
                "Type `{:?}` doesn't have any methods, remove its constructor and getter.",
                type_.as_path()
            );
            self.constructors.remove(type_);
            self.getters.remove(type_);
        }
    }

    /// If `methods` has a method of type `T`, but `constructors` doesn't have a constructor of type `T`.
    ///
    /// This function removes those methods.
    pub fn remove_methods_without_constructors(&mut self) {
        let mut no_constructor_types = Vec::new();
        for method in &self.methods {
            if !self.constructors.contains_key(method.impl_type())
                && !no_constructor_types.iter().any(|t| t == method.impl_type())
            {
                no_constructor_types.push(method.impl_type().clone());
            }
        }
        for type_ in &no_constructor_types {
            log!(
                Normal,
                Warning,
                "Type `{:?}` doesn't have a constructor, skip all its methods.",
                type_.as_path()
            );
            self.methods
                .retain(|m| m.metadata.impl_type.as_ref() != Some(type_));
        }
    }
}

/// Generic harness generator using a backend.
pub struct HarnessGenerator<B: HarnessBackend> {
    /// Functions used to generate the harness
    pub classifier: FunctionClassifier,
    /// Imports from mod1
    pub mod1_imports: Vec<Path>,
    /// Imports from mod2
    pub mod2_imports: Vec<Path>,
    /// Backend marker
    _backend: std::marker::PhantomData<B>,
}

impl<B: HarnessBackend> HarnessGenerator<B> {
    /// Create a new harness generator for the given functions and traits.
    pub fn new(
        functions: Vec<CommonFunction>,
        mod1_imports: Vec<Path>,
        mod2_imports: Vec<Path>,
    ) -> Self {
        let mut classifier = FunctionClassifier::classify(functions);
        classifier.remove_unused_constructors_and_getters();
        classifier.remove_methods_without_constructors();
        Self {
            classifier,
            mod1_imports,
            mod2_imports,
            _backend: std::marker::PhantomData,
        }
    }

    /// Generate argument struct `ArgsFoo` for function `foo`; backend supplies the derive/attrs.
    fn generate_arg_struct(&self, func: &CommonFunction) -> TokenStream {
        let struct_name = format_ident!("Args{}", func.metadata.name.to_ident());
        let mut fields = Vec::<TokenStream>::new();
        for arg in &func.metadata.signature.0.inputs {
            if matches!(arg, syn::FnArg::Typed(_)) {
                fields.push(quote! { #arg });
            }
        }
        let attrs = B::arg_struct_attrs();
        quote! {
            #attrs
            pub struct #struct_name {
                #(pub #fields),*
            }
        }
    }

    /// Generate all argument structs for functions, methods, and constructors.
    fn generate_all_arg_structs(&self) -> Vec<TokenStream> {
        let mut func_structs = self
            .classifier
            .functions
            .iter()
            .map(|f| self.generate_arg_struct(f))
            .collect::<Vec<_>>();

        let mut method_structs = Vec::<TokenStream>::new();
        let mut used_constructors = Vec::<&CommonFunction>::new();
        for method in &self.classifier.methods {
            let constructor = self
                .classifier
                .constructors
                .get(method.impl_type())
                .unwrap();
            method_structs.push(self.generate_arg_struct(method));
            if !used_constructors
                .iter()
                .any(|c| c.metadata.name == constructor.metadata.name)
            {
                used_constructors.push(&constructor);
            }
        }

        let constructor_structs = used_constructors
            .iter()
            .map(|c| self.generate_arg_struct(c))
            .collect::<Vec<_>>();

        func_structs.extend(constructor_structs);
        func_structs.extend(method_structs);
        func_structs
    }

    /// Generate a harness function for comparing two free-standing functions.
    fn generate_harness_for_function(&self, func: &CommonFunction) -> TokenStream {
        let mut function_args = Vec::<TokenStream>::new();
        for arg in &func.metadata.signature.0.inputs {
            if let syn::FnArg::Typed(pat_type) = arg {
                let arg_name = match &*pat_type.pat {
                    syn::Pat::Ident(pat_ident) => pat_ident.ident.to_string(),
                    _ => "arg".to_string(),
                };
                let ident = format_ident!("{}", arg_name);
                function_args.push(quote! { #ident.clone() });
            }
        }
        B::make_harness_for_function(func, &function_args)
    }

    /// Generate a harness function for comparing two methods.
    fn generate_harness_for_method(&self, method: &CommonFunction) -> TokenStream {
        let constructor = self
            .classifier
            .constructors
            .get(method.impl_type())
            .unwrap();
        // getter may be absent
        let getter = self.classifier.getters.get(method.impl_type());

        // collect constructor args
        let mut constructor_args = Vec::new();
        for arg in &constructor.metadata.signature.0.inputs {
            if let syn::FnArg::Typed(pat_type) = arg {
                let name = match &*pat_type.pat {
                    syn::Pat::Ident(pi) => pi.ident.to_string(),
                    _ => "arg".into(),
                };
                let ident = format_ident!("{}", name);
                constructor_args.push(quote! { #ident.clone() });
            }
        }

        // method args and receiver info
        let mut method_args = Vec::new();
        let mut receiver_mut = None;
        let mut receiver_ref = None;
        for arg in &method.metadata.signature.0.inputs {
            match arg {
                syn::FnArg::Receiver(rec) => {
                    receiver_mut = rec.mutability.clone();
                    receiver_ref = rec.reference.clone();
                }
                syn::FnArg::Typed(pat) => {
                    let name = match &*pat.pat {
                        syn::Pat::Ident(pi) => pi.ident.to_string(),
                        _ => "arg".into(),
                    };
                    let ident = format_ident!("{}", name);
                    method_args.push(quote! { #ident.clone() });
                }
            }
        }
        let receiver_prefix = {
            let reference = receiver_ref.map(|(amp, _)| amp);
            let mut_tok = receiver_mut;
            // We will call backend with something like `#reference #mut` as the receiver prefix.
            quote! { #reference #mut_tok }
        };

        B::make_harness_for_method(
            method,
            constructor,
            getter,
            &method_args,
            &constructor_args,
            receiver_prefix,
        )
    }

    /// Generate trait imports (`use` statements) for the harness file.
    fn generate_imports(&self) -> Vec<TokenStream> {
        let mod1_import_stmts = self.mod1_imports.iter().map(|path| {
            let ident = format_ident!("Mod1{}", path.0.last().unwrap());
            quote! {
                use mod1::#path as #ident;
            }
        });
        let mod2_import_stmts = self.mod2_imports.iter().map(|path| {
            let ident = format_ident!("Mod2{}", path.0.last().unwrap());
            quote! {
                use mod2::#path as #ident;
            }
        });
        mod1_import_stmts.chain(mod2_import_stmts).collect()
    }

    /// Generate the complete harness file as a TokenStream.
    pub fn generate_harness(&self) -> TokenStream {
        let imports = self.generate_imports();
        let arg_structs = self.generate_all_arg_structs();
        let functions = self
            .classifier
            .functions
            .iter()
            .map(|func| self.generate_harness_for_function(func))
            .collect::<Vec<_>>();
        let methods = self
            .classifier
            .methods
            .iter()
            .map(|method| self.generate_harness_for_method(method))
            .collect::<Vec<_>>();
        let additional = B::additional_code(&self.classifier);

        B::finalize(imports, arg_structs, functions, methods, additional)
    }
}

/// The trait capturing differences between different check/test harness backends.
pub trait HarnessBackend {
    /// Attributes / derives to put on generated `Args*` structs.
    fn arg_struct_attrs() -> TokenStream;

    /// Build the test function TokenStream for a free-standing function.
    fn make_harness_for_function(
        function: &CommonFunction,
        function_args: &[TokenStream],
    ) -> TokenStream;

    /// Build the test function TokenStream for a method.
    fn make_harness_for_method(
        method: &CommonFunction,
        constructor: &CommonFunction,
        getter: Option<&CommonFunction>,
        method_args: &[TokenStream],
        constructor_args: &[TokenStream],
        receiver_prefix: TokenStream,
    ) -> TokenStream;

    /// Other additional code pieces needed can be added as associated functions here.
    fn additional_code(_classifier: &FunctionClassifier) -> TokenStream {
        quote! {}
    }

    /// Final wrapper given all pieces: used to assemble final file.
    fn finalize(
        imports: Vec<TokenStream>,
        args_structs: Vec<TokenStream>,
        functions: Vec<TokenStream>,
        methods: Vec<TokenStream>,
        additional: TokenStream,
    ) -> TokenStream;
}
