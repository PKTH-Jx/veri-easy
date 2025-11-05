//! Differential Fuzzing step.

use std::{io::Write, process::Command, str::FromStr};

use crate::{
    checker::{CheckResult, CheckStep, Checker},
    function::{CommonFunction, FunctionClassifier},
};
use anyhow::anyhow;
use proc_macro2::TokenStream;
use quote::{format_ident, quote};
use syn::FnArg;

/// Collect a function's arguments into a struct.
fn generate_function_arg_struct(function: &CommonFunction) -> TokenStream {
    let struct_name = format_ident!("Args{}", function.flat_name());
    let mut fields = Vec::<TokenStream>::new();
    for arg in &function.sig().inputs {
        fields.push(quote! {
            #arg
        });
    }

    quote! {
        #[derive(serde::Deserialize)]
        pub struct #struct_name {
            #(pub #fields),*
        }
    }
}

/// Collect a method's arguments into a struct.
fn generate_method_arg_struct(method: &CommonFunction) -> TokenStream {
    let struct_name = format_ident!("Args{}", method.flat_name());
    let mut fields = Vec::<TokenStream>::new();
    for arg in &method.sig().inputs {
        if matches!(arg, FnArg::Typed(_)) {
            fields.push(quote! {
                #arg
            });
        }
    }
    quote! {
        #[derive(serde::Deserialize)]
        pub struct #struct_name {
            #(pub #fields),*
        }
    }
}

/// Differential fuzzing harness generator.
pub struct HarnessGenerator {
    /// All functions used in the fuzzing process.
    funcs: FunctionClassifier,
}

impl HarnessGenerator {
    /// Create a new DifferentialFuzzing step.
    pub fn new(functions: Vec<CommonFunction>) -> Self {
        let mut classifier = FunctionClassifier::classify(functions);
        classifier.remove_unused_constructors();
        classifier.remove_no_constructor_methods();
        Self { funcs: classifier }
    }

    /// Generate Argument structs.
    fn generate_arg_structs(&self) -> TokenStream {
        let func_structs = self
            .funcs
            .functions
            .iter()
            .map(|func| generate_function_arg_struct(func))
            .collect::<Vec<_>>();

        let mut method_structs = Vec::<TokenStream>::new();
        let mut used_constructors = Vec::<&CommonFunction>::new();
        for method in &self.funcs.methods {
            let constructor = self.funcs.constructors.get(&method.scope()).unwrap();
            method_structs.push(generate_method_arg_struct(method));
            if !used_constructors
                .iter()
                .any(|c| c.name() == constructor.name())
            {
                used_constructors.push(&constructor);
            }
        }

        let constructor_structs = used_constructors
            .iter()
            .map(|func| generate_function_arg_struct(func))
            .collect::<Vec<_>>();

        quote! {
            #(#func_structs)*
            #(#method_structs)*
            #(#constructor_structs)*
        }
    }

    /// Generate one test function for a function.
    fn generate_test_fn_for_function(&self, function: &CommonFunction) -> TokenStream {
        // Test function name
        let fn_name = format_ident!("test_{}", function.flat_name());
        // Function name
        let function_name = function.name();
        let function_name_tk = TokenStream::from_str(function_name).unwrap();

        // Function argument struct name
        let function_arg_struct = format_ident!("Args{}", function.flat_name());

        // Function call arguments
        let mut function_args = Vec::<TokenStream>::new();
        for arg in &function.sig().inputs {
            if let FnArg::Typed(pat_type) = arg {
                let arg_name = match &*pat_type.pat {
                    syn::Pat::Ident(pat_ident) => pat_ident.ident.to_string(),
                    _ => "arg".to_string(),
                };
                let arg_ident = quote::format_ident!("{}", arg_name);
                function_args.push(quote! { #arg_ident.clone() });
            }
        }

        quote! {
            fn #fn_name(input: &[u8]) -> bool {
                // Function arguments
                let function_arg_struct = match postcard::from_bytes::<#function_arg_struct>(&input[..]) {
                    Ok(args) => args,
                    Err(_) => return true,
                };

                // Function call
                let r1 = std::panic::catch_unwind(std::panic::AssertUnwindSafe(
                    || mod1::#function_name_tk(#(#function_args),*))).map_err(|_| ());
                let r2 = std::panic::catch_unwind(std::panic::AssertUnwindSafe(
                    || mod2::#function_name_tk(#(#function_args),*))).map_err(|_| ());

                if r1 != r2 {
                    println!("MISMATCH {}", #function_name);
                }
                r1 == r2
            }
        }
    }

    /// Generate one test function for a method.
    fn generate_test_fn_for_method(&self, method: &CommonFunction) -> TokenStream {
        let constructor = self.funcs.constructors.get(&method.scope()).unwrap();

        // Test function name
        let fn_name = format_ident!("test_{}", method.flat_name());
        // Constructor name
        let constructor_name = constructor.name();
        let constructor_name_tk = TokenStream::from_str(constructor_name).unwrap();
        // Method name
        let method_name = method.name();
        let method_name_tk = TokenStream::from_str(method_name).unwrap();

        // Method argument struct name
        let method_arg_struct = format_ident!("Args{}", method.flat_name());
        // Constructor argument struct name
        let constructor_arg_struct = format_ident!("Args{}", constructor.flat_name());

        // Constructor call arguments
        let mut constructor_args = Vec::<TokenStream>::new();
        for arg in &constructor.sig().inputs {
            if let FnArg::Typed(pat_type) = arg {
                let arg_name = match &*pat_type.pat {
                    syn::Pat::Ident(pat_ident) => pat_ident.ident.to_string(),
                    _ => "arg".to_string(),
                };
                let arg_ident = quote::format_ident!("{}", arg_name);
                constructor_args.push(quote! { #arg_ident.clone() });
            } else {
                unreachable!("Constructor should not have receiver.");
            }
        }

        // Method call arguments
        let mut reference = None;
        let mut mutability = None;
        let mut method_args = Vec::<TokenStream>::new();
        for arg in &method.sig().inputs {
            match arg {
                FnArg::Receiver(receiver) => {
                    mutability = receiver.mutability.clone();
                    reference = receiver.reference.clone();
                }
                FnArg::Typed(pat_type) => {
                    let arg_name = match &*pat_type.pat {
                        syn::Pat::Ident(pat_ident) => pat_ident.ident.to_string(),
                        _ => "arg".to_string(),
                    };
                    let arg_ident = quote::format_ident!("{}", arg_name);
                    method_args.push(quote! { #arg_ident.clone() });
                }
            }
        }
        let lifetime = match &reference {
            Some((_, lt)) => lt.clone(),
            None => None,
        };
        let reference = reference.map(|(and, _)| and);

        quote! {
            fn #fn_name(input: &[u8]) -> bool {
                // Constructor arguments
                let (constr_arg_struct, remain) = match postcard::take_from_bytes::<#constructor_arg_struct>(&input[..]) {
                    Ok((args, remain)) => (args, remain),
                    Err(_) => return true,
                };
                // Method arguments
                let method_arg_struct = match postcard::from_bytes::<#method_arg_struct>(&remain[..]) {
                    Ok(args) => args,
                    Err(_) => return true,
                };

                // Construct s1 and s2
                let mut s1 = match std::panic::catch_unwind(std::panic::AssertUnwindSafe(
                    || mod1::#constructor_name_tk(#(constr_arg_struct.#constructor_args),*))) {
                    Ok(s) => s,
                    Err(_) => return true,
                };
                let mut s2 = match std::panic::catch_unwind(std::panic::AssertUnwindSafe(
                    || mod2::#constructor_name_tk(#(constr_arg_struct.#constructor_args),*))) {
                    Ok(s) => s,
                    Err(_) => return true,
                };

                // Do method call
                let r1 = std::panic::catch_unwind(std::panic::AssertUnwindSafe(
                    || mod1::#method_name_tk(#reference #lifetime #mutability s1, #(method_arg_struct.#method_args),*) )).map_err(|_| ());
                let r2 = std::panic::catch_unwind(std::panic::AssertUnwindSafe(
                    || mod2::#method_name_tk(#reference #lifetime #mutability s2, #(method_arg_struct.#method_args),*) )).map_err(|_| ());

                if r1 != r2 {
                    println!("MISMATCH: {}", #method_name);
                }
                r1 == r2
            }
        }
    }

    // Generate test dispatch function
    fn generate_dispatch_fn(&self, test_fns: &[String]) -> TokenStream {
        let fn_count = test_fns.len();
        let match_arms = test_fns.iter().enumerate().map(|(i, name)| {
            let fn_name = format_ident!("{}", name);
            let i = i as u8;
            quote! {
                #i => #fn_name(&input[1..]),
            }
        });
        quote! {
            pub fn run_harness(input: &[u8]) -> bool {
                if input.len() == 0 {
                    return true;
                }
                let fn_id = input[0] % #fn_count as u8;
                match fn_id {
                    #(#match_arms)*
                    _ => true,
                }
            }
        }
    }

    // Generate harness main file
    fn generate_harness(&self) -> TokenStream {
        let args = self.generate_arg_structs();
        let test_functions = self
            .funcs
            .functions
            .iter()
            .map(|func| self.generate_test_fn_for_function(func))
            .collect::<Vec<_>>();
        let test_methods = self
            .funcs
            .methods
            .iter()
            .map(|method| self.generate_test_fn_for_method(method))
            .collect::<Vec<_>>();
        let test_fns = self
            .funcs
            .functions
            .iter()
            .map(|func| format!("test_{}", func.flat_name()))
            .chain(
                self.funcs
                    .methods
                    .iter()
                    .map(|method| format!("test_{}", method.flat_name())),
            )
            .collect::<Vec<_>>();
        let dispatch_fn = self.generate_dispatch_fn(&test_fns);

        quote! {
            use std::ops::Range;
            use mod1::BitAlloc as Mod1BitAlloc;
            use mod2::{BitAlloc as Mod2BitAlloc, BitAllocView as Mod2BitAllocView};

            mod mod1;
            mod mod2;

            #args
            #(#test_functions)*
            #(#test_methods)*
            #dispatch_fn
        }
    }
}

/// Differential Fuzzing step.
pub struct DifferentialFuzzing;

impl CheckStep for DifferentialFuzzing {
    fn name(&self) -> &str {
        "Differential Fuzzing"
    }

    fn note(&self) -> Option<&str> {
        Some("Using differential fuzzing to find inconsistencies.")
    }

    fn run(&self, checker: &Checker) -> CheckResult {
        let harness = HarnessGenerator::new(checker.unchecked_funcs.clone()).generate_harness();
        println!("{}", harness);
        let _ = std::fs::File::create("harness.rs")
            .unwrap()
            .write_all(harness.to_string().as_bytes());
        let _ = Command::new("rustfmt")
            .args(["harness.rs", "--unstable-features", "--skip-children"])
            .status();
        CheckResult::failed(anyhow!("unimplemented!"))
    }
}
