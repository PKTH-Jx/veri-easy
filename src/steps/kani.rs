//! Use model-checker Kani to check function equivalence

use anyhow::anyhow;
use proc_macro2::TokenStream;
use quote::{format_ident, quote};
use regex::Regex;
use std::{
    io::{BufRead, Write},
    process::Command,
    str::FromStr,
};

use crate::{
    checker::{CheckResult, CheckStep, Checker},
    function::CommonFunction,
    generator::{HarnessBackend, HarnessGenerator},
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
    ) -> TokenStream {
        // Test function name
        let fn_name = format_ident!("check_{}", function.flat_name());
        // Function name
        let function_name = function.name();
        let function_name_tk = TokenStream::from_str(function_name).unwrap();
        // Function argument struct name
        let function_arg_struct = format_ident!("Args{}", function.flat_name());

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
                let r1 = mod1::#method_name_tk(#receiver_prefix s1, #(method_arg_struct.#method_args),*);
                let r2 = mod2::#method_name_tk(#receiver_prefix s2, #(method_arg_struct.#method_args),*);

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
            use std::ops::Range;
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
        let generator = KaniHarnessGenerator::new(checker.unchecked_funcs.clone());
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
        std::fs::remove_dir_all(harness_path)
            .map_err(|_| anyhow!("Failed to remove harness file"))?;
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
