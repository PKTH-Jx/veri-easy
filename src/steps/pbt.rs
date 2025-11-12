//! Property-based testing step.

use anyhow::anyhow;
use proc_macro2::TokenStream;
use quote::{format_ident, quote};
use regex::Regex;
use std::{
    io::{BufRead, BufReader, Write},
    process::Command,
    str::FromStr,
};

use crate::{
    checker::{CheckResult, CheckStep, Checker},
    function::CommonFunction,
    generator::{HarnessBackend, HarnessGenerator},
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
        // Test function name
        let fn_name = format_ident!("check_{}", function.flat_name());
        // Function name
        let function_name = function.name();
        let function_name_tk = TokenStream::from_str(function_name).unwrap();
        // Function argument struct name
        let function_arg_struct = format_ident!("Args{}", function.flat_name());

        quote! {
            #[test]
            fn #fn_name(function_args in any::<#function_arg_struct>()) {
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

        quote! {
            #[test]
            fn #fn_name(
                constr_arg_struct in any::<#constructor_arg_struct>(),
                method_arg_struct in any::<#method_arg_struct>(),
            ) {
                // Construct s1 and s2
                let mut s1 = match std::panic::catch_unwind(std::panic::AssertUnwindSafe(
                    || mod1::#constructor_name_tk(#(constr_arg_struct.#constructor_args),*))) {
                    Ok(s) => s,
                    Err(_) => return Ok(()),
                };
                let mut s2 = match std::panic::catch_unwind(std::panic::AssertUnwindSafe(
                    || mod2::#constructor_name_tk(#(constr_arg_struct.#constructor_args),*))) {
                    Ok(s) => s,
                    Err(_) => return Ok(()),
                };

                // Do method call
                let r1 = std::panic::catch_unwind(std::panic::AssertUnwindSafe(
                        || mod1::#method_name_tk(
                            #receiver_prefix s1, #(method_arg_struct.#method_args),*
                        )
                    )).map_err(|_| ());
                let r2 = std::panic::catch_unwind(std::panic::AssertUnwindSafe(
                        || mod2::#method_name_tk(
                            #receiver_prefix s2, #(method_arg_struct.#method_args),*
                        )
                    )).map_err(|_| ());

                if r1 != r2 || s1.get_val() != s2.get_val() {
                    println!("MISMATCH: {}", #method_name);
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
    fn generate_harness_file(&self, checker: &Checker) -> (Vec<String>, TokenStream) {
        let generator = PBTHarnessGenerator::new(checker.unchecked_funcs.clone());
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
        Command::new("cargo")
            .args(["fmt"])
            .status()
            .map_err(|_| anyhow!("Failed to run cargo fmt"))?;
        let _ = std::env::set_current_dir(cur_dir);

        Ok(())
    }

    /// Run libAFL fuzzer and save the ouput in "df.tmp".
    fn run_test(&self, harness_path: &str, output_path: &str) -> anyhow::Result<()> {
        let output_file =
            std::fs::File::create(output_path).map_err(|_| anyhow!("Failed to create tmp file"))?;

        let cur_dir = std::env::current_dir().unwrap();
        let _ = std::env::set_current_dir(harness_path);
        Command::new("cargo")
            .args(["test"])
            .stdout(output_file)
            .stderr(std::fs::File::open("/dev/null").unwrap())
            .status()
            .map_err(|_| anyhow!("Failed to run proptest"))?;
        let _ = std::env::set_current_dir(cur_dir);

        Ok(())
    }

    /// Analyze the fuzzer output and return the functions that are not checked.
    fn analyze_pbt_output(&self, functions: &[String], output_path: &str) -> CheckResult {
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

impl CheckStep for PropertyBasedTesting {
    fn name(&self) -> &str {
        "Property-Based Testing"
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
