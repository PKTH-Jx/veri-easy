//! Differential Fuzzing step.

use anyhow::anyhow;
use proc_macro2::TokenStream;
use quote::{format_ident, quote};
use regex::Regex;
use std::io::{BufRead, BufReader};

use crate::{
    check::{CheckResult, Checker, Component},
    config::DiffFuzzConfig,
    defs::{CommonFunction, Path, Precondition},
    generate::{FunctionCollection, HarnessBackend, HarnessGenerator},
    utils::{create_harness_project, run_command},
};

/// Differential fuzzing harness generator backend.
struct DFHarnessBackend;

impl HarnessBackend for DFHarnessBackend {
    fn arg_struct_attrs(&self) -> TokenStream {
        quote! {
            #[derive(Debug, serde::Deserialize)]
        }
    }

    fn make_harness_for_function(
        &self,
        function: &CommonFunction,
        function_args: &[TokenStream],
        _precondition: Option<&Precondition>,
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
        &self,
        method: &CommonFunction,
        constructor: &CommonFunction,
        getter: Option<&CommonFunction>,
        method_args: &[TokenStream],
        constructor_args: &[TokenStream],
        receiver_prefix: TokenStream,
        _precondition: Option<&Precondition>,
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

    fn additional_code(&self, collection: &FunctionCollection) -> TokenStream {
        // Generate dispatch function as additional code
        let test_fns = collection
            .functions
            .iter()
            .map(|func| format!("check_{}", func.metadata.name.to_ident()))
            .chain(
                collection
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
        &self,
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
pub struct DifferentialFuzzing {
    config: DiffFuzzConfig,
}

impl DifferentialFuzzing {
    /// Create a new Differential Fuzzing component with the given configuration.
    pub fn new(config: DiffFuzzConfig) -> Self {
        Self { config }
    }

    fn generate_harness_file(&self, checker: &Checker) -> (Vec<Path>, TokenStream) {
        let generator = DFHarnessGenerator::new(checker, DFHarnessBackend);
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
    fn create_harness_project(
        &self,
        checker: &Checker,
        harness: TokenStream,
        harness_path: &str,
    ) -> anyhow::Result<()> {
        let toml = r#"
[package]
name = "harness"
version = "0.1.0"
edition = "2024"

[dependencies]
serde = "*"
postcard = "*"
"#;
        create_harness_project(
            harness_path,
            &checker.src1.content,
            &checker.src2.content,
            &harness.to_string(),
            toml,
            true,
        )
    }

    /// Run libAFL fuzzer and save the ouput in "df.tmp".
    fn run_fuzzer(&self, fuzzer_path: &str, output_path: &str) -> anyhow::Result<()> {
        let status = run_command(
            "cargo",
            &["run", "--release"],
            Some(output_path),
            Some(fuzzer_path),
        )?;

        if status.code() == Some(101) {
            return Err(anyhow!("Command failed due to compilation error"));
        }
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
                    res.fail.push(Path::from_str(&func_name));
                }
            }
        }

        res
    }

    /// Remove the harness project.
    fn remove_harness_project(&self) -> anyhow::Result<()> {
        std::fs::remove_dir_all(&self.config.harness_path)
            .map_err(|_| anyhow!("Failed to remove harness file"))
    }

    /// Remove the output file.
    fn remove_output_file(&self) -> anyhow::Result<()> {
        std::fs::remove_file(&self.config.output_path)
            .map_err(|_| anyhow!("Failed to remove output file"))
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
        let (functions, harness) = self.generate_harness_file(checker);

        let res = self.create_harness_project(checker, harness, &self.config.harness_path);
        if let Err(e) = res {
            return CheckResult::failed(e);
        }

        let res = self.run_fuzzer(&self.config.fuzzer_path, &self.config.output_path);
        if let Err(e) = res {
            return CheckResult::failed(e);
        }
        let check_res = self.analyze_fuzzer_output(&functions, &self.config.output_path);

        if !self.config.keep_harness {
            if let Err(e) = self.remove_harness_project() {
                return CheckResult::failed(e);
            }
        }
        if !self.config.keep_output {
            if let Err(e) = self.remove_output_file() {
                return CheckResult::failed(anyhow!("Failed to remove output file: {}", e));
            }
        }

        check_res
    }
}
