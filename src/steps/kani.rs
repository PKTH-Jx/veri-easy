//! Use model-checker Kani

use anyhow::anyhow;
use regex::Regex;
use std::{
    io::{BufRead, Write},
    process::Command,
    str::FromStr,
};

use proc_macro2::TokenStream;
use quote::{format_ident, quote};
use syn::FnArg;

use crate::{
    checker::{CheckResult, CheckStep, Checker},
    function::{CommonFunction, FunctionClassifier},
};

/// Kani harness generator.
struct HarnessGenerator {
    /// All functions used in the kani step.
    classifier: FunctionClassifier,
}

impl HarnessGenerator {
    /// Create a new DifferentialFuzzing step.
    pub fn new(functions: Vec<CommonFunction>) -> Self {
        let mut classifier = FunctionClassifier::classify(functions);
        classifier.remove_unused_constructors();
        classifier.remove_no_constructor_methods();
        Self { classifier }
    }

    /// Collect a function's arguments into a struct.
    fn generate_arg_struct(&self, func: &CommonFunction) -> TokenStream {
        let struct_name = format_ident!("Args{}", func.flat_name());
        let mut fields = Vec::<TokenStream>::new();
        for arg in &func.sig().inputs {
            if matches!(arg, FnArg::Typed(_)) {
                fields.push(quote! {
                    #arg
                });
            }
        }
        quote! {
            #[derive(Debug, kani::Arbitrary)]
            pub struct #struct_name {
                #(pub #fields),*
            }
        }
    }

    /// Generate argument structs.
    fn generate_arg_structs(&self) -> TokenStream {
        let func_structs = self
            .classifier
            .functions
            .iter()
            .map(|func| self.generate_arg_struct(func))
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
            .map(|func| self.generate_arg_struct(func))
            .collect::<Vec<_>>();

        quote! {
            #(#func_structs)*
            #(#method_structs)*
            #(#constructor_structs)*
        }
    }

    /// Generate one Kani harness for comparing two free-standing functions.
    fn generate_function(&self, func: &CommonFunction) -> TokenStream {
        // Test function name
        let fn_name = format_ident!("check_{}", func.flat_name());
        // Function name
        let function_name = func.name();
        let function_name_tk = TokenStream::from_str(function_name).unwrap();

        // Function argument struct name
        let function_arg_struct = format_ident!("Args{}", func.flat_name());

        // Function call arguments
        let mut function_args = Vec::<TokenStream>::new();
        for arg in &func.sig().inputs {
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
            #[cfg(kani)]
            #[kani::proof]
            #[allow(non_snake_case)]
            pub fn #fn_name() {
                let function_arg_struct = kani::any::<#function_arg_struct>();
                // Function call
                let r1 = mod1::#function_name_tk(#(function_arg_struct.#function_args),*);
                let r2 = mod2::#function_name_tk(#(function_arg_struct.#function_args),*);
                assert!(r1 == r2);
            }
        }
    }

    /// Generate one Kani harness for comparing two methods.
    fn generate_method(&self, method: &CommonFunction) -> TokenStream {
        let constructor = self.classifier.constructors.get(&method.scope()).unwrap();

        // Test function name
        let fn_name = format_ident!("check_{}", method.flat_name());
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
        let reference = reference.map(|(and, _)| and);

        quote! {
            #[cfg(kani)]
            #[kani::proof]
            #[allow(non_snake_case)]
            pub fn #fn_name() {
                let constr_arg_struct = kani::any::<#constructor_arg_struct>();
                // Construct s1 and s2
                let mut s1 = mod1::#constructor_name_tk(#(constr_arg_struct.#constructor_args),*);
                let mut s2 = mod2::#constructor_name_tk(#(constr_arg_struct.#constructor_args),*);

                let method_arg_struct = kani::any::<#method_arg_struct>();
                // Do method call
                let r1 = mod1::#method_name_tk(#reference #mutability s1, #(method_arg_struct.#method_args),*);
                let r2 = mod2::#method_name_tk(#reference #mutability s2, #(method_arg_struct.#method_args),*);

                assert!(r1 == r2);
                assert!(s1.get_val() == s2.get_val());
            }
        }
    }

    /// Generate all free-standing functions
    fn generate_functions(&self) -> Vec<TokenStream> {
        self.classifier
            .functions
            .iter()
            .map(|func| self.generate_function(func))
            .collect()
    }

    /// Generate all methods
    fn generate_methods(&self) -> Vec<TokenStream> {
        self.classifier
            .methods
            .iter()
            .map(|method| self.generate_method(method))
            .collect()
    }

    /// Generate all import (`use`) statements
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

    /// Generate harness file
    fn generate_harness(&self) -> TokenStream {
        let args = self.generate_arg_structs();
        let functions = self.generate_functions();
        let methods = self.generate_methods();
        let imports = self.generate_imports();

        quote! {
            use std::ops::Range;
            mod mod1;
            mod mod2;

            #args
            #(#imports)*
            #(#functions)*
            #(#methods)*

            fn main() {}
        }
    }
}

/// Kani step: use Kani model-checker to check function equivalence.
pub struct Kani;

impl Kani {
    /// Generate harness code for Kani.
    fn generate_harness(&self, checker: &Checker) -> TokenStream {
        let generator = HarnessGenerator::new(checker.unchecked_funcs.clone());
        generator.generate_harness()
    }

    /// Create a cargo project for Kani harness.
    ///
    /// Dir structure:
    ///
    /// harness_path
    /// ├── Cargo.toml
    /// └── src
    ///     ├── main.rs
    ///     ├── mod1.rs
    ///     └── mod2.rs
    fn create_harness_project(
        &self,
        checker: &Checker,
        harness: TokenStream,
        harness_path: &str,
    ) -> anyhow::Result<()> {
        Command::new("cargo")
            .args(["new", "--bin", "--vcs", "none", harness_path])
            .status()?;

        // Write rust files
        std::fs::File::create(harness_path.to_owned() + "/src/mod1.rs")
            .unwrap()
            .write_all(checker.src1.content.as_bytes())
            .map_err(|_| anyhow!("Failed to write mod1 file"))?;
        std::fs::File::create(harness_path.to_owned() + "/src/mod2.rs")
            .unwrap()
            .write_all(checker.src2.content.as_bytes())
            .map_err(|_| anyhow!("Failed to write mod2 file"))?;
        std::fs::File::create(harness_path.to_owned() + "/src/main.rs")
            .unwrap()
            .write_all(harness.to_string().as_bytes())
            .map_err(|_| anyhow!("Failed to write harness file"))?;

        // Write Cargo.toml
        std::fs::File::create(harness_path.to_owned() + "/Cargo.toml")
            .unwrap()
            .write_all(
                r#"
[package]
name = "harness"
version = "0.1.0"
edition = "2024"

[dev-dependencies]
kani = "*"
"#
                .as_bytes(),
            )
            .map_err(|_| anyhow!("Failed to write Cargo.toml"))?;

        // Cargo fmt
        let cur_dir = std::env::current_dir().unwrap();
        let _ = std::env::set_current_dir(harness_path);
        Command::new("cargo")
            .args(["fmt"])
            .status()
            .map_err(|_| anyhow!("Failed to run cargo fmt"))?;
        let _ = std::env::set_current_dir(cur_dir);

        Ok(())
    }

    /// Run Kani and save the output.
    fn run_kani(&self, harness_path: &str, output_path: &str) -> anyhow::Result<()> {
        let output_file = std::fs::File::create(output_path)
            .map_err(|_| anyhow!("Failed to create output file"))?;

        let cur_dir = std::env::current_dir().unwrap();
        let _ = std::env::set_current_dir(harness_path);
        Command::new("cargo")
            .args(["kani", "-Z", "unstable-options", "--harness-timeout", "10s"])
            .stdout(output_file)
            .status()
            .map_err(|_| anyhow!("Failed to run kani"))?;
        let _ = std::env::set_current_dir(cur_dir);

        Ok(())
    }

    /// Analyze Kani output from "kani.tmp".
    fn analyze_kani_output(&self, output_path: &str) -> CheckResult {
        let mut res = CheckResult {
            status: Ok(()),
            ok: vec![],
            fail: vec![],
        };

        let re = Regex::new(r"Checking harness check_([0-9a-zA-Z_]+)\.").unwrap();
        let file = std::fs::File::open(output_path).unwrap();
        let reader = std::io::BufReader::new(file);
        let mut func_name: Option<String> = None;

        for line in reader.lines() {
            let line = line.unwrap();
            if let Some(caps) = re.captures(&line) {
                func_name = Some(caps[1].replace("___", "::"));
            }
            if line.contains("VERIFICATION:- SUCCESSFUL") && func_name.is_some() {
                res.ok.push(func_name.take().unwrap());
            } else if line.contains("VERIFICATION:- FAILED") && func_name.is_some() {
                func_name = None;
            }
        }

        res
    }

    /// Remove the harness project.
    fn remove_harness_project(&self, harness_path: &str) -> anyhow::Result<()> {
        // std::fs::remove_dir_all(harness_path)
        //     .map_err(|_| anyhow!("Failed to remove harness file"))?;
        Ok(())
    }
}

impl CheckStep for Kani {
    fn name(&self) -> &str {
        "Kani"
    }

    fn note(&self) -> Option<&str> {
        Some("Use Kani model-checker to check function consistency")
    }

    fn run(&self, checker: &Checker) -> CheckResult {
        let harness_path = "kani_harness";
        let harness = self.generate_harness(checker);

        let res = self.create_harness_project(checker, harness, harness_path);
        if let Err(e) = res {
            return CheckResult::failed(e);
        }

        let output_path = "kani.tmp";
        let res = self.run_kani(harness_path, output_path);
        if let Err(e) = res {
            return CheckResult::failed(e);
        }
        let check_res = self.analyze_kani_output(output_path);

        if let Err(e) = self.remove_harness_project(harness_path) {
            return CheckResult::failed(e);
        }

        check_res
    }
}
