//! Property-based testing step.

use anyhow::anyhow;
use proc_macro2::TokenStream;
use quote::{format_ident, quote};
use regex::Regex;
use std::io::{BufRead, BufReader, Write};

use crate::{
    check::{CheckResult, Checker, Component},
    defs::{CommonFunction, Path},
    generate::{HarnessBackend, HarnessGenerator},
    utils::run_command_and_log_error,
};

/// PBT harness generator backend.
struct PBTHarnessBackend;

impl HarnessBackend for PBTHarnessBackend {
    fn arg_struct_attrs() -> TokenStream {
        quote! {
            #[derive(Debug)]
            #[cfg_attr(test, derive(proptest_derive::Arbitrary))]
        }
    }

    fn make_harness_for_function(
        function: &CommonFunction,
        function_args: &[TokenStream],
    ) -> TokenStream {
        let fn_name = &function.metadata.name;
        let fn_name_string = fn_name.to_string();

        // Test function name
        let test_fn_name = format_ident!("check_{}", fn_name.to_ident());
        // Function argument struct name
        let function_arg_struct = format_ident!("Args{}", fn_name.to_ident());

        quote! {
            #[test]
            fn #test_fn_name(function_args in any::<#function_arg_struct>()) {
                // Function call
                let r1 = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                    mod1::#fn_name(#(function_arg_struct.#function_args),*)
                }))
                .map_err(|_| ());
                let r2 = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                    mod2::#fn_name(#(function_arg_struct.#function_args),*)
                }))
                .map_err(|_| ());

                if r1 != r2 {
                    println!("MISMATCH {}", #fn_name_string);
                    println!("function: {:?}", function_arg_struct);
                    println!("r1 = {:?}, r2 = {:?}", r1, r2);
                }
                assert(r1 == r2);
            }
        }
    }

    fn make_harness_for_method(
        method: &CommonFunction,
        constructor: &CommonFunction,
        method_args: &[TokenStream],
        constructor_args: &[TokenStream],
        receiver_prefix: TokenStream,
    ) -> TokenStream {
        let fn_name = &method.metadata.name;
        let constr_name = &constructor.metadata.name;
        let fn_name_string = fn_name.to_string();

        // Test function name
        let test_fn_name = format_ident!("check_{}", fn_name.to_ident());
        // Method argument struct name
        let method_arg_struct = format_ident!("Args{}", fn_name.to_ident());
        // Constructor argument struct name
        let constructor_arg_struct = format_ident!("Args{}", constr_name.to_ident());
        quote! {
            #[test]
            fn #test_fn_name(
                constr_arg_struct in any::<#constructor_arg_struct>(),
                method_arg_struct in any::<#method_arg_struct>(),
            ) {
                // Construct s1 and s2
                let mut s1 = match std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                    mod1::#constr_name(#(constr_arg_struct.#constructor_args),*)
                })) {
                    Ok(s) => s,
                    Err(_) => return Ok(()),
                };
                let mut s2 = match std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                    mod2::#constr_name(#(constr_arg_struct.#constructor_args),*)
                })) {
                    Ok(s) => s,
                    Err(_) => return Ok(()),
                };

                // Method call
                let r1 = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                    mod1::#fn_name(
                        #receiver_prefix s1, #(method_arg_struct.#method_args),*
                    )
                }))
                .map_err(|_| ());
                let r2 = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                    mod2::#fn_name(
                        #receiver_prefix s2, #(method_arg_struct.#method_args),*
                    )
                }))
                .map_err(|_| ());

                if r1 != r2 || s1.get_val() != s2.get_val() {
                    println!("MISMATCH: {}", #fn_name_string);
                    println!("contructor: {:?}", constr_arg_struct);
                    println!("method: {:?}", method_arg_struct);
                    println!("r1 = {:?}, r2 = {:?}", r1, r2);
                    println!("s1 = {:?}, s2 = {:?}", s1.get_val(), s2.get_val());
                }
                assert!(r1 == r2);
                assert!(s1.get_val() == s2.get_val());
            }
        }
    }

    fn finalize(
        imports: Vec<TokenStream>,
        args_structs: Vec<TokenStream>,
        functions: Vec<TokenStream>,
        methods: Vec<TokenStream>,
        _additional: TokenStream,
    ) -> TokenStream {
        quote! {
            #![allow(unused)]
            #![allow(non_snake_case)]
            #![allow(non_camel_case_types)]

            mod mod1;
            mod mod2;

            use proptest::prelude::*;
            use std::ops::Range;
            #(#imports)*

            #(#args_structs)*
            proptest! {
                #![proptest_config(ProptestConfig::with_cases(100000))]
                #(#functions)*
                #(#methods)*
            }
            fn main() {}
        }
    }
}

/// PBT harness generator.
type PBTHarnessGenerator = HarnessGenerator<PBTHarnessBackend>;

/// Property-based testing step using Proptest.
pub struct PropertyBasedTesting;

impl PropertyBasedTesting {
    fn generate_harness_file(&self, checker: &Checker) -> (Vec<Path>, TokenStream) {
        let generator = PBTHarnessGenerator::new(
            checker.unchecked_funcs.clone(),
            checker.src1.symbols.clone(),
            checker.src2.symbols.clone(),
        );
        // Collect functions and methods that are checked in harness
        let functions = generator
            .classifier
            .functions
            .iter()
            .map(|f| f.metadata.name.clone())
            .chain(
                generator
                    .classifier
                    .methods
                    .iter()
                    .map(|f| f.metadata.name.clone()),
            )
            .collect::<Vec<_>>();
        let harness = generator.generate_harness();
        (functions, harness)
    }

    /// Create a cargo project for proptest harness.
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
        run_command_and_log_error("cargo", &["new", "--bin", "--vcs", "none", harness_path])?;

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

[dependencies]
proptest = "1.9"
proptest-derive = "0.2.0"
"#
                .as_bytes(),
            )
            .map_err(|_| anyhow!("Failed to write Cargo.toml"))?;

        // Cargo fmt
        let cur_dir = std::env::current_dir().unwrap();
        let _ = std::env::set_current_dir(harness_path);
        run_command_and_log_error("cargo", &["fmt"])?;
        let _ = std::env::set_current_dir(cur_dir);

        Ok(())
    }

    /// Run libAFL fuzzer and save the ouput in "df.tmp".
    fn run_test(&self, harness_path: &str, output_path: &str) -> anyhow::Result<()> {
        let output_file =
            std::fs::File::create(output_path).map_err(|_| anyhow!("Failed to create tmp file"))?;

        let cur_dir = std::env::current_dir().unwrap();
        let _ = std::env::set_current_dir(harness_path);
        let output = run_command_and_log_error("cargo", &["test"])?;
        let _ = std::env::set_current_dir(cur_dir);

        std::io::copy(&mut output.stdout.as_slice(), &mut &output_file)
            .map_err(|_| anyhow!("Failed to write fuzzer output"))?;
        Ok(())
    }

    /// Analyze the fuzzer output and return the functions that are not checked.
    fn analyze_pbt_output(&self, functions: &[Path], output_path: &str) -> CheckResult {
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
                if let Some(i) = res.ok.iter().position(|f| f.to_string() == func_name) {
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

impl Component for PropertyBasedTesting {
    fn name(&self) -> &str {
        "Property-Based Testing"
    }

    fn is_formal(&self) -> bool {
        false
    }

    fn note(&self) -> Option<&str> {
        Some("Uses Proptest to generate inputs and compare function behaviors.")
    }

    fn run(&self, checker: &Checker) -> CheckResult {
        let harness_path = "pbt_harness";
        let (functions, harness) = self.generate_harness_file(checker);

        let res = self.create_harness_project(checker, harness, harness_path);
        if let Err(e) = res {
            return CheckResult::failed(e);
        }

        let output_path = "pbt.tmp";
        let res = self.run_test(harness_path, output_path);
        if let Err(e) = res {
            return CheckResult::failed(e);
        }
        let check_res = self.analyze_pbt_output(&functions, output_path);

        if let Err(e) = self.remove_harness_project(harness_path) {
            return CheckResult::failed(e);
        }

        check_res
    }
}
