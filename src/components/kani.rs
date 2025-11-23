//! Use model-checker Kani to check function equivalence

use anyhow::anyhow;
use proc_macro2::TokenStream;
use quote::{format_ident, quote};
use regex::Regex;
use std::io::{BufRead, Write};

use crate::{
    check::{CheckResult, Checker, Component},
    defs::{CommonFunction, Path, Precondition},
    generate::{HarnessBackend, HarnessGenerator},
    utils::run_command_and_log_error,
};

/// Kani harness generator backend.
struct KaniHarnessBackend;

impl HarnessBackend for KaniHarnessBackend {
    fn arg_struct_attrs() -> TokenStream {
        quote! {
            #[derive(Debug, kani::Arbitrary)]
        }
    }

    fn make_harness_for_function(
        function: &CommonFunction,
        function_args: &[TokenStream],
        precondition: Option<&Precondition>,
    ) -> TokenStream {
        let fn_name = &function.metadata.name;

        // Test function name
        let test_fn_name = format_ident!("check_{}", fn_name.to_ident());
        // Function argument struct name
        let function_arg_struct = format_ident!("Args{}", fn_name.to_ident());

        // If precondition is present, we may need to add assume code
        let precondition = precondition.map(|pre| {
            let check_fn_name = pre.check_name();
            quote! {
                kani::assume(#check_fn_name(#(function_arg_struct.#function_args),*));
            }
        });

        quote! {
            #[cfg(kani)]
            #[kani::proof]
            #[allow(non_snake_case)]
            pub fn #test_fn_name() {
                let function_arg_struct = kani::any::<#function_arg_struct>();
                // Precondition assume
                #precondition
                // Function call
                let r1 = mod1::#fn_name(#(function_arg_struct.#function_args),*);
                let r2 = mod2::#fn_name(#(function_arg_struct.#function_args),*);
                assert!(r1 == r2);
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
        precondition: Option<&Precondition>,
    ) -> TokenStream {
        let fn_name = &method.metadata.name;
        let constr_name = &constructor.metadata.name;

        // Test function name
        let test_fn_name = format_ident!("check_{}", fn_name.to_ident());
        // Method argument struct name
        let method_arg_struct = format_ident!("Args{}", fn_name.to_ident());
        // Constructor argument struct name
        let constructor_arg_struct = format_ident!("Args{}", constr_name.to_ident());

        // If a getter is provided, generate state check code after method call
        let state_check = getter.map(|getter| {
            let getter = &getter.metadata.signature.0.ident;
            quote! {
                assert!(s1.#getter() == s2.#getter());
            }
        });

        // If precondition is present, we may need to add assume code
        let precondition = precondition.map(|pre| {
            let check_fn_name = pre.check_name();
            quote! {
                kani::assume(s2.#check_fn_name(#(method_arg_struct.#method_args),*));
            }
        });

        quote! {
            #[cfg(kani)]
            #[kani::proof]
            #[allow(non_snake_case)]
            pub fn #test_fn_name() {
                let constr_arg_struct = kani::any::<#constructor_arg_struct>();
                // Construct s1 and s2
                let mut s1 = mod1::#constr_name(#(constr_arg_struct.#constructor_args),*);
                let mut s2 = mod2::#constr_name(#(constr_arg_struct.#constructor_args),*);

                let method_arg_struct = kani::any::<#method_arg_struct>();
                // Precondition assume
                #precondition
                // Do method call
                let r1 = mod1::#fn_name(#receiver_prefix s1, #(method_arg_struct.#method_args),*);
                let r2 = mod2::#fn_name(#receiver_prefix s2, #(method_arg_struct.#method_args),*);

                assert!(r1 == r2);
                #state_check
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

            #(#imports)*
            #(#args_structs)*
            #(#functions)*
            #(#methods)*

            fn main() {}
        }
    }
}

/// Kani harness generator.
type KaniHarnessGenerator = HarnessGenerator<KaniHarnessBackend>;

/// Kani step: use Kani model-checker to check function equivalence.
pub struct Kani;

impl Kani {
    /// Generate harness code for Kani.
    fn generate_harness(&self, checker: &Checker) -> TokenStream {
        let generator = KaniHarnessGenerator::new(checker);
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

[dev-dependencies]
kani = "*"
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

    /// Run Kani and save the output.
    fn run_kani(&self, harness_path: &str, output_path: &str) -> anyhow::Result<()> {
        let output_file = std::fs::File::create(output_path)
            .map_err(|_| anyhow!("Failed to create output file"))?;

        let cur_dir = std::env::current_dir().unwrap();
        let _ = std::env::set_current_dir(harness_path);
        let output = run_command_and_log_error(
            "cargo",
            &["kani", "-Z", "unstable-options", "--harness-timeout", "10s"],
        )?;
        let _ = std::env::set_current_dir(cur_dir);

        std::io::copy(&mut output.stdout.as_slice(), &mut &output_file)
            .map_err(|_| anyhow!("Failed to write Kani output"))?;
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
                res.ok.push(Path::from_str(&func_name.take().unwrap()));
            } else if line.contains("VERIFICATION:- FAILED") && func_name.is_some() {
                func_name = None;
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

impl Component for Kani {
    fn name(&self) -> &str {
        "Kani"
    }

    fn is_formal(&self) -> bool {
        true
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
