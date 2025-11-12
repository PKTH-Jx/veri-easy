//! Harness generator used by various steps (Kani, PBT, DFT).
use proc_macro2::TokenStream;
use quote::{format_ident, quote};
use syn::FnArg;

use crate::function::{CommonFunction, FunctionClassifier};

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

/// Generic harness generator using a backend.
pub struct HarnessGenerator<B: HarnessBackend> {
    /// Functions used to generate the harness
    pub classifier: FunctionClassifier,
    _backend: std::marker::PhantomData<B>,
}

impl<B: HarnessBackend> HarnessGenerator<B> {
    /// Create a new harness generator for the given functions.
    pub fn new(functions: Vec<CommonFunction>) -> Self {
        let mut classifier = FunctionClassifier::classify(functions);
        classifier.remove_unused_constructors();
        classifier.remove_no_constructor_methods();
        Self {
            classifier,
            _backend: std::marker::PhantomData,
        }
    }

    /// Generate argument struct `ArgsFoo` for function `foo`; backend supplies the derive/attrs.
    fn generate_arg_struct(&self, func: &CommonFunction) -> TokenStream {
        let struct_name = format_ident!("Args{}", func.flat_name());
        let mut fields = Vec::<TokenStream>::new();
        for arg in &func.sig().inputs {
            if matches!(arg, FnArg::Typed(_)) {
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
            let constructor = self.classifier.constructors.get(&method.scope()).unwrap();
            method_structs.push(self.generate_arg_struct(method));
            if !used_constructors
                .iter()
                .any(|c| c.name() == constructor.name())
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
        for arg in &func.sig().inputs {
            if let FnArg::Typed(pat_type) = arg {
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
        let constructor = self.classifier.constructors.get(&method.scope()).unwrap();

        // collect constructor args
        let mut constructor_args = Vec::new();
        for arg in &constructor.sig().inputs {
            if let FnArg::Typed(pat_type) = arg {
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
        for arg in &method.sig().inputs {
            match arg {
                FnArg::Receiver(rec) => {
                    receiver_mut = rec.mutability.clone();
                    receiver_ref = rec.reference.clone();
                }
                FnArg::Typed(pat) => {
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
            &method_args,
            &constructor_args,
            receiver_prefix,
        )
    }

    /// Generate necessary imports (`use` statements) for the harness file.
    fn generate_imports(&self) -> Vec<TokenStream> {
        let mut mod1_imports = Vec::new();
        let mut mod2_imports = Vec::new();

        for func in self
            .classifier
            .functions
            .iter()
            .chain(self.classifier.methods.iter())
        {
            // To use a function in a trait, we need to import the trait
            if let Some(impl_block) = &func.f1.impl_block {
                if let Some((_, path, _)) = &impl_block.trait_ {
                    let path = path
                        .segments
                        .iter()
                        .map(|seg| seg.ident.clone())
                        .collect::<Vec<_>>();
                    if !mod1_imports.contains(&path) {
                        mod1_imports.push(path);
                    }
                }
            }
            if let Some(impl_block) = &func.f2.impl_block {
                if let Some((_, path, _)) = &impl_block.trait_ {
                    let path = path
                        .segments
                        .iter()
                        .map(|seg| seg.ident.clone())
                        .collect::<Vec<_>>();
                    if !mod2_imports.contains(&path) {
                        mod2_imports.push(path);
                    }
                }
            }
        }

        let mod1_import_stmts = mod1_imports.iter().map(|path| {
            let ident = format_ident!("Mod1{}", path.last().unwrap());
            quote! {
                use mod1::#(#path)::* as #ident;
            }
        });
        let mod2_import_stmts = mod2_imports.iter().map(|path| {
            let ident = format_ident!("Mod2{}", path.last().unwrap());
            quote! {
                use mod2::#(#path)::* as #ident;
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
