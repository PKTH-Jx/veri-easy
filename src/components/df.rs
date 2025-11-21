//! Differential Fuzzing step.

use anyhow::anyhow;
use proc_macro2::TokenStream;
use quote::{format_ident, quote};
use regex::Regex;
use std::io::{BufRead, BufReader, Write};

use crate::{
    check::{CheckResult, Checker, Component},
    defs::{CommonFunction, Path},
    generate::{FunctionCollection, HarnessBackend, HarnessGenerator},
    utils::run_command_and_log_error,
};

/// Differential fuzzing harness generator backend.
struct DFHarnessBackend;

impl HarnessBackend for DFHarnessBackend {
    fn arg_struct_attrs() -> TokenStream {
        quote! {
            #[derive(Debug, serde::Deserialize)]
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
            fn #test_fn_name(input: &[u8]) -> bool {
                // Function arguments
                let function_arg_struct = match postcard::from_bytes::<#function_arg_struct>(&input[..]) {
                    Ok(args) => args,
                    Err(_) => return true,
                };

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
                r1 == r2
            }
        }
    }

    fn make_harness_for_method(
        method: &CommonFunction,
        constructor: &CommonFunction,
        getter: Option<&CommonFunction>,
        method_args: &[TokenStream],
        constructor_args: &[TokenStream],
        receiver_prefix: TokenStream,
    ) -> TokenStream {
        let fn_name = &method.metadata.name;
        let fn_name_string = fn_name.to_string();
        let constr_name = &constructor.metadata.name;

        // Test function name
        let test_fn_name = format_ident!("check_{}", fn_name.to_ident());
        // Method argument struct name
        let method_arg_struct = format_ident!("Args{}", fn_name.to_ident());
        // Constructor argument struct name
        let constructor_arg_struct = format_ident!("Args{}", constr_name.to_ident());

        // Error report message
        let err_report = quote! {
            println!("MISMATCH: {}", #fn_name_string);
            println!("contructor: {:?}", constr_arg_struct);
            println!("method: {:?}", method_arg_struct);
        };

        // If a getter is provided, generate state check code after method call
        let state_check = getter.map(|getter| {
            let getter = &getter.metadata.signature.0.ident;
            quote! {
                if s1.#getter() != s2.#getter() {
                    #err_report
                    return false;
                }
            }
        });

        quote! {
            fn #test_fn_name(input: &[u8]) -> bool {
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
                let mut s1 = match std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                    mod1::#constr_name(#(constr_arg_struct.#constructor_args),*)
                })) {
                    Ok(s) => s,
                    Err(_) => return true,
                };
                let mut s2 = match std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                    mod2::#constr_name(#(constr_arg_struct.#constructor_args),*)
                })) {
                    Ok(s) => s,
                    Err(_) => return true,
                };

                // Do method call
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

                if r1 != r2 {
                    #err_report
                    return false;
                }
                #state_check

                true
            }
        }
    }

    fn additional_code(classifier: &FunctionCollection) -> TokenStream {
        // Generate dispatch function as additional code
        let test_fns = classifier
            .functions
            .iter()
            .map(|func| format!("check_{}", func.metadata.name.to_ident()))
            .chain(
                classifier
                    .methods
                    .iter()
                    .map(|method| format!("check_{}", method.metadata.name.to_ident())),
            )
            .collect::<Vec<_>>();

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

    fn finalize(
        imports: Vec<TokenStream>,
        args_structs: Vec<TokenStream>,
        functions: Vec<TokenStream>,
        methods: Vec<TokenStream>,
        additional: TokenStream,
    ) -> TokenStream {
        quote! {
            #![allow(unused)]
            #![allow(non_snake_case)]
            #![allow(non_camel_case_types)]

            mod mod1;
            mod mod2;

            use std::ops::Range;
            #(#imports)*

            #(#args_structs)*
            #(#functions)*
            #(#methods)*
            #additional
        }
    }
}

/// Differential fuzzing harness generator.
type DFHarnessGenerator = HarnessGenerator<DFHarnessBackend>;

/// Differential Fuzzing step.
pub struct DifferentialFuzzing;

impl DifferentialFuzzing {
    fn generate_harness_file(&self, checker: &Checker) -> (Vec<Path>, TokenStream) {
        let generator = DFHarnessGenerator::new(checker);
        // Collect functions and methods that are checked in harness
        let functions = generator
            .collection
            .functions
            .iter()
            .map(|f| f.metadata.name.clone())
            .chain(
                generator
                    .collection
                    .methods
                    .iter()
                    .map(|f| f.metadata.name.clone()),
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
        run_command_and_log_error("cargo", &["new", "--lib", "--vcs", "none", harness_path])?;

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
        run_command_and_log_error("cargo", &["fmt"])?;
        let _ = std::env::set_current_dir(cur_dir);

        Ok(())
    }

    /// Run libAFL fuzzer and save the ouput in "df.tmp".
    fn run_fuzzer(&self, fuzzer_path: &str, output_path: &str) -> anyhow::Result<()> {
        let output_file =
            std::fs::File::create(output_path).map_err(|_| anyhow!("Failed to create tmp file"))?;

        let cur_dir = std::env::current_dir().unwrap();
        let _ = std::env::set_current_dir(fuzzer_path);
        let output = run_command_and_log_error("cargo", &["run", "--release"])?;
        let _ = std::env::set_current_dir(cur_dir);

        std::io::copy(&mut output.stdout.as_slice(), &mut &output_file)
            .map_err(|_| anyhow!("Failed to write fuzzer output"))?;
        Ok(())
    }

    /// Analyze the fuzzer output and return the functions that are not checked.
    fn analyze_fuzzer_output(&self, functions: &[Path], output_path: &str) -> CheckResult {
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

impl Component for DifferentialFuzzing {
    fn name(&self) -> &str {
        "Differential Fuzzing"
    }

    fn is_formal(&self) -> bool {
        false
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
