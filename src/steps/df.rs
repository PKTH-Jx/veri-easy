//! Differential Fuzzing step.

use std::{
    io::{BufRead, BufReader, Write},
    process::Command,
    str::FromStr,
};

use crate::{
    checker::{CheckResult, CheckStep, Checker},
    function::{CommonFunction, FunctionClassifier},
};
use anyhow::anyhow;
use proc_macro2::TokenStream;
use quote::{format_ident, quote};
use regex::Regex;
use syn::FnArg;

/// Differential fuzzing harness generator.
struct HarnessGenerator {
    /// All functions used in the fuzzing process.
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
            #[derive(Debug, serde::Deserialize)]
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
                    || mod1::#function_name_tk(#(function_arg_struct.#function_args),*))).map_err(|_| ());
                let r2 = std::panic::catch_unwind(std::panic::AssertUnwindSafe(
                    || mod2::#function_name_tk(#(function_arg_struct.#function_args),*))).map_err(|_| ());

                if r1 != r2 {
                    println!("MISMATCH {}", #function_name);
                    println!("function: {:?}", function_arg_struct);
                    println!("r1 = {:?}, r2 = {:?}", r1, r2);
                }
                r1 == r2
            }
        }
    }

    /// Generate one test function for a method.
    fn generate_test_fn_for_method(&self, method: &CommonFunction) -> TokenStream {
        let constructor = self.classifier.constructors.get(&method.scope()).unwrap();

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
        let reference = reference.map(|(and, _)| and);

        quote! {
            fn #fn_name(input: &[u8]) -> bool {
                // Constructor arguments
                let (constr_arg_struct, remain) = match postcard::take_from_bytes::<#constructor_arg_struct>(
                    &input[..]
                ) {
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
                        || mod1::#method_name_tk(
                            #reference #mutability s1, #(method_arg_struct.#method_args),*
                        )
                    )).map_err(|_| ());
                let r2 = std::panic::catch_unwind(std::panic::AssertUnwindSafe(
                        || mod2::#method_name_tk(
                            #reference #mutability s2, #(method_arg_struct.#method_args),*
                        )
                    )).map_err(|_| ());

                if r1 != r2 || s1.get_val() != s2.get_val() {
                    println!("MISMATCH: {}", #method_name);
                    println!("contructor: {:?}", constr_arg_struct);
                    println!("method: {:?}", method_arg_struct);
                    println!("r1 = {:?}, r2 = {:?}", r1, r2);
                    println!("s1 = {:?}, s2 = {:?}", s1.get_val(), s2.get_val());
                }
                r1 == r2 && s1.get_val() == s2.get_val()
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

    // Generate harness main file
    fn generate_harness(&self) -> TokenStream {
        let args = self.generate_arg_structs();
        let test_functions = self
            .classifier
            .functions
            .iter()
            .map(|func| self.generate_test_fn_for_function(func))
            .collect::<Vec<_>>();
        let test_methods = self
            .classifier
            .methods
            .iter()
            .map(|method| self.generate_test_fn_for_method(method))
            .collect::<Vec<_>>();
        let test_fns = self
            .classifier
            .functions
            .iter()
            .map(|func| format!("test_{}", func.flat_name()))
            .chain(
                self.classifier
                    .methods
                    .iter()
                    .map(|method| format!("test_{}", method.flat_name())),
            )
            .collect::<Vec<_>>();
        let dispatch_fn = self.generate_dispatch_fn(&test_fns);
        let imports = self.generate_imports();

        quote! {
            mod mod1;
            mod mod2;

            use std::ops::Range;
            #(#imports)*

            #args
            #(#test_functions)*
            #(#test_methods)*
            #dispatch_fn
        }
    }
}

/// Differential Fuzzing step.
pub struct DifferentialFuzzing;

impl DifferentialFuzzing {
    fn generate_harness_file(&self, checker: &Checker) -> (Vec<String>, TokenStream) {
        let generator = HarnessGenerator::new(checker.unchecked_funcs.clone());
        // Collect functions and methods that are checked in harness
        let functions = generator
            .classifier
            .functions
            .iter()
            .map(|f| f.name().to_owned())
            .chain(
                generator
                    .classifier
                    .methods
                    .iter()
                    .map(|f| f.name().to_owned()),
            )
            .collect::<Vec<_>>();
        let harness = generator.generate_harness();
        (functions, harness)
    }

    /// Create a cargo project for LibAFL harness.
    ///
    /// Dir structure:
    ///
    /// harness_path
    /// ├── Cargo.toml
    /// └── src
    ///     ├── lib.rs
    ///     ├── mod1.rs
    ///     └── mod2.rs
    fn create_harness_project(
        &self,
        checker: &Checker,
        harness: TokenStream,
        harness_path: &str,
    ) -> anyhow::Result<()> {
        Command::new("cargo")
            .args(["new", "--lib", "--vcs", "none", harness_path])
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
        std::fs::File::create(harness_path.to_owned() + "/src/lib.rs")
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

[dependencies]
serde = "*"
postcard = "*"
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

    /// Run libAFL fuzzer and save the ouput in "df.tmp".
    fn run_fuzzer(&self, fuzzer_path: &str, output_path: &str) -> anyhow::Result<()> {
        let output_file =
            std::fs::File::create(output_path).map_err(|_| anyhow!("Failed to create tmp file"))?;

        let cur_dir = std::env::current_dir().unwrap();
        let _ = std::env::set_current_dir(fuzzer_path);
        Command::new("cargo")
            .args(["run", "--release"])
            .stdout(output_file)
            .stderr(std::fs::File::open("/dev/null").unwrap())
            .status()
            .map_err(|_| anyhow!("Failed to run kani"))?;
        let _ = std::env::set_current_dir(cur_dir);

        Ok(())
    }

    /// Analyze the fuzzer output and return the functions that are not checked.
    fn analyze_fuzzer_output(&self, functions: &[String], output_path: &str) -> CheckResult {
        let mut res = CheckResult {
            status: Ok(()),
            ok: functions.to_vec(),
            fail: vec![],
        };

        let re = Regex::new(r"MISMATCH:\s*(\S+)").unwrap();
        let file = std::fs::File::open(output_path).unwrap();
        let reader = BufReader::new(file);

        for line in reader.lines() {
            if let Some(caps) = re.captures(&line.unwrap()) {
                let func_name = caps[1].to_string();
                if let Some(i) = res.ok.iter().position(|f| *f == func_name) {
                    res.ok.swap_remove(i);
                }
            }
        }

        res
    }

    /// Remove the harness project.
    fn remove_harness_project(&self, harness_path: &str) -> anyhow::Result<()> {
        std::fs::remove_dir_all(harness_path)
            .map_err(|_| anyhow!("Failed to remove harness file"))?;
        Ok(())
    }
}

impl CheckStep for DifferentialFuzzing {
    fn name(&self) -> &str {
        "Differential Fuzzing"
    }

    fn note(&self) -> Option<&str> {
        Some("Using differential fuzzing to find inconsistencies.")
    }

    fn run(&self, checker: &Checker) -> CheckResult {
        let harness_path = "/Users/jingx/Dev/playground/fuzz/harness";
        let fuzzer_path = "/Users/jingx/Dev/playground/fuzz";

        let (functions, harness) = self.generate_harness_file(checker);

        let res = self.create_harness_project(checker, harness, harness_path);
        if let Err(e) = res {
            return CheckResult::failed(e);
        }

        let output_path = "df.tmp";
        let res = self.run_fuzzer(fuzzer_path, output_path);
        if let Err(e) = res {
            return CheckResult::failed(e);
        }
        let check_res = self.analyze_fuzzer_output(&functions, output_path);

        if let Err(e) = self.remove_harness_project(harness_path) {
            return CheckResult::failed(e);
        }

        check_res
    }
}
