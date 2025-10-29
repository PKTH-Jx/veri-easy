//! Use model-checker Kani

use anyhow::anyhow;
use regex::Regex;
use std::{collections::BTreeMap, io::Write, process::Command, str::FromStr};

use proc_macro2::TokenStream;
use quote::{format_ident, quote};
use syn::{
    FnArg, Pat, Path, Receiver, ReturnType, Type, TypeArray, TypePath, TypePtr, TypeReference,
    TypeSlice, TypeTuple, token::Mut,
};

use crate::{
    checker::{CheckResult, CheckStep, Checker},
    function::CommonFunction,
};

/// Kani step: use Kani model-checker to check function equivalence.
pub struct Kani;

impl Kani {
    fn generate_harness(&self, checker: &Checker) -> anyhow::Result<TokenStream> {
        let content1 = TokenStream::from_str(&checker.src1.content)
            .map_err(|_| anyhow!("Invalid Rust code"))?;
        let mod1 = quote! {
            mod mod1 {
                #content1
            }
        };
        let content2 = TokenStream::from_str(&checker.src2.content)
            .map_err(|_| anyhow!("Invalid Rust code"))?;
        let mod2 = quote! {
            mod mod2 {
                #content2
            }
        };

        let generator = HarnessGenerator::new(checker.unchecked_funcs.clone());
        let functions = generator.generate_functions();
        let methods = generator.generate_methods();
        let imports = generator.generate_imports();

        Ok(quote! {
            use std::ops::Range;

            #(#imports)*

            #mod1

            #mod2

            #(#functions)*

            #(#methods)*
        })
    }

    fn write_harness_file(&self, harness: TokenStream, path: &str) -> anyhow::Result<()> {
        let mut file = std::fs::File::create(path)?;
        file.write(harness.to_string().as_bytes())?;
        Command::new("rustfmt").arg(path).status()?;
        Ok(())
    }

    fn run_kani(&self, harness_path: &str) -> anyhow::Result<String> {
        let tmp_path = "kani.tmp";
        let tmp_file =
            std::fs::File::create(tmp_path).map_err(|_| anyhow!("Failed to create tmp file"))?;
        Command::new("kani")
            .args([
                harness_path,
                "-Z",
                "unstable-options",
                "--harness-timeout",
                "10s",
            ])
            // .stderr(std::fs::File::open("/dev/null").unwrap())
            .stdout(tmp_file)
            .status()
            .map_err(|_| anyhow!("Failed to run kani"))?;
        let output =
            std::fs::read_to_string(tmp_path).map_err(|_| anyhow!("Failed to read tmp file"))?;
        std::fs::remove_file(tmp_path).map_err(|_| anyhow!("Failed to remove tmp file"))?;
        Ok(output)
    }

    fn analyze_kani_output(&self, output: &str) -> CheckResult {
        let mut res = CheckResult {
            status: Ok(()),
            ok: vec![],
            fail: vec![],
        };

        let re = Regex::new(r"Checking harness check___([0-9a-zA-Z_]+)\.").unwrap();
        let mut func_name: Option<String> = None;

        for line in output.lines() {
            if let Some(caps) = re.captures(line) {
                func_name = Some(caps[1].replace("___", "::"));
            }
            if line.contains("VERIFICATION:- SUCCESSFUL") && func_name.is_some() {
                res.ok.push(func_name.take().unwrap());
            } else if line.contains("VERIFICATION:- FAILED") && func_name.is_some() {
                // res.fail.push(func_name.take().unwrap());
                func_name = None;
            }
        }

        res
    }

    fn remove_harness_file(&self, harness_path: &str) -> anyhow::Result<()> {
        // std::fs::remove_file(harness_path).map_err(|_| anyhow!("Failed to remove harness file"))?;
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
        let harness_path = "harness.rs";
        let res = self.generate_harness(checker);
        if let Err(e) = res {
            return CheckResult::failed(e);
        }
        let harness = res.unwrap();

        let res = self.write_harness_file(harness, harness_path);
        if let Err(e) = res {
            return CheckResult::failed(e);
        }
        let res = self.run_kani(harness_path);
        if let Err(e) = res {
            return CheckResult::failed(e);
        }
        let output = res.unwrap();

        let check_res = self.analyze_kani_output(&output);

        if let Err(e) = self.remove_harness_file(harness_path) {
            return CheckResult::failed(e);
        }

        check_res
    }
}

/// Kani harness generator.
#[derive(Debug)]
struct HarnessGenerator {
    /// Free-stand functions.
    functions: Vec<CommonFunction>,
    /// Methods (with `self` receiver).
    methods: Vec<CommonFunction>,
    /// Constructors (return `Self` type).
    constructors: BTreeMap<String, CommonFunction>,
}

impl HarnessGenerator {
    /// Separate free-standing functions and methods.
    fn new(functions: Vec<CommonFunction>) -> Self {
        let mut res = Self {
            functions: Vec::new(),
            methods: Vec::new(),
            constructors: BTreeMap::new(),
        };
        for func in functions {
            if let Some(impl_block) = &func.f1.impl_block {
                // The name of the struct
                let struct_name = match &*impl_block.self_ty {
                    Type::Path(type_path) => type_path.path.get_ident(),
                    _ => None,
                };
                if func
                    .sig()
                    .inputs
                    .iter()
                    .any(|arg| matches!(arg, FnArg::Receiver(_)))
                {
                    // Has `self` receiver, consider it as a method.
                    res.methods.push(func);
                } else {
                    if let ReturnType::Type(_, rt) = &func.sig().output {
                        if let Type::Path(type_path) = &**rt {
                            if type_path.path.is_ident("Self") {
                                // Return `Self` type, consider it as a constructor.
                                res.constructors.insert(func.scope(), func);
                                continue;
                            } else if let Some(name) = struct_name {
                                if type_path.path.is_ident(name) {
                                    // Return `struct_name` type, consider it as a constructor.
                                    res.constructors.insert(func.scope(), func);
                                    continue;
                                }
                            }
                        }
                    }
                    // No `self` receiver and not return `Self` type, consider it as a free-standing function.
                    res.functions.push(func);
                }
            } else {
                // Function outside of impl block is a free-standing function.
                res.functions.push(func);
            }
        }
        res
    }

    /// Generate one Kani harness for comparing two free-standing functions.
    fn generate_function(&self, func: &CommonFunction) -> TokenStream {
        let harness_name = quote::format_ident!("check___{}", func.name().replace("::", "___"));
        let func_name = TokenStream::from_str(&func.name()).unwrap();
        let inputs = &func.sig().inputs;

        let mut harness_body = Vec::new();
        let mut call_args: Vec<proc_macro2::TokenStream> = Vec::new();

        for arg in inputs {
            if let FnArg::Typed(pat_type) = arg {
                let arg_name = match &*pat_type.pat {
                    syn::Pat::Ident(pat_ident) => pat_ident.ident.to_string(),
                    _ => "arg".to_string(),
                };
                let arg_type = &pat_type.ty;
                let mutability = match &*pat_type.pat {
                    Pat::Ident(pat_ident) => pat_ident.mutability,
                    _ => None,
                };
                let init_stmt = arg_type.init_for_type(&arg_name, &mutability);
                harness_body.push(init_stmt);

                let arg_ident = quote::format_ident!("{}", arg_name);
                call_args.push(quote! { #arg_ident.clone() });
            } else {
                unreachable!("Free-standing function should not have receiver.");
            }
        }

        quote! {
            #[cfg(kani)]
            #[kani::proof]
            #[allow(non_snake_case)]
            pub fn #harness_name() {
                #(#harness_body)*

                let r1 = mod1::#func_name(#(#call_args),*);
                let r2 = mod2::#func_name(#(#call_args),*);
                assert_eq!(r1, r2);
            }
        }
    }

    /// Generate one Kani harness for comparing two methods.
    fn generate_method(&self, method: &CommonFunction) -> TokenStream {
        let harness_name = quote::format_ident!("check___{}", method.name().replace("::", "___"));
        let func_name = TokenStream::from_str(&method.name()).unwrap();
        let inputs = &method.sig().inputs;

        let mut harness_body = Vec::new();
        let mut call_args: Vec<TokenStream> = Vec::new();

        let ident1 = quote::format_ident!("s1");
        let ident2 = quote::format_ident!("s2");
        let mut reference = None;
        let mut mutability = None;

        let (init, constructor, args) = match self.construct(&method.scope()) {
            Some((init, constructor, args)) => (init, constructor, args),
            None => {
                println!("No constructor found for struct: {}", method.scope());
                return quote! {};
            }
        };

        for arg in inputs {
            match arg {
                FnArg::Receiver(receiver) => {
                    mutability = receiver.mutability.clone();
                    reference = receiver.reference.clone();
                    let construct_stmt = quote! {
                        #init
                        let #mutability #ident1 = mod1::#constructor(#args);
                        let #mutability #ident2 = mod2::#constructor(#args);
                    };
                    harness_body.push(construct_stmt);
                }
                FnArg::Typed(pat_type) => {
                    let arg_name = match &*pat_type.pat {
                        syn::Pat::Ident(pat_ident) => pat_ident.ident.to_string(),
                        _ => "arg".to_string(),
                    };
                    let arg_type = &pat_type.ty;
                    let mutability = match &*pat_type.pat {
                        Pat::Ident(pat_ident) => pat_ident.mutability,
                        _ => None,
                    };
                    let init_stmt = arg_type.init_for_type(&arg_name, &mutability);
                    harness_body.push(init_stmt);
                    let arg_ident = quote::format_ident!("{}", arg_name);
                    call_args.push(quote! { #arg_ident.clone() });
                }
            }
        }

        let lifetime = match &reference {
            Some((_, lt)) => lt.clone(),
            None => None,
        };
        let reference = reference.map(|(and, _)| and);

        quote! {
            #[cfg(kani)]
            #[kani::proof]
            #[allow(non_snake_case)]
            pub fn #harness_name() {
                #(#harness_body)*

                let r1 = mod1::#func_name(#reference #lifetime #mutability #ident1, #(#call_args),*);
                let r2 = mod2::#func_name(#reference #lifetime #mutability #ident2, #(#call_args),*);
                assert_eq!(r1, r2);
            }
        }
    }

    /// Find the constructor of a struct, and use "kani::any()" as arguments to construct the struct.
    ///
    /// Returns (init_code, constructor_name, call_args)
    fn construct(&self, struct_name: &str) -> Option<(TokenStream, TokenStream, TokenStream)> {
        let constructor = self.constructors.get(struct_name)?;
        let func_name = TokenStream::from_str(&constructor.name()).unwrap();
        let inputs = &constructor.sig().inputs;

        let mut init_code = Vec::new();
        let mut call_args: Vec<TokenStream> = Vec::new();

        for arg in inputs {
            match arg {
                FnArg::Receiver(_) => unreachable!("Constructor should not have receiver"),
                FnArg::Typed(pat_type) => {
                    let arg_name = match &*pat_type.pat {
                        syn::Pat::Ident(pat_ident) => pat_ident.ident.to_string(),
                        _ => "arg".to_string(),
                    };
                    let arg_type = &pat_type.ty;
                    let mutability = match &*pat_type.pat {
                        Pat::Ident(pat_ident) => pat_ident.mutability,
                        _ => None,
                    };
                    let init_stmt = arg_type.init_for_type(&arg_name, &mutability);
                    init_code.push(init_stmt);

                    let arg_ident = quote::format_ident!("{}", arg_name);
                    call_args.push(quote! { #arg_ident });
                }
            }
        }

        Some((
            quote! {
                #(#init_code)*
            },
            func_name,
            quote! {
                #(#call_args),*
            },
        ))
    }

    /// Generate all free-standing functions
    fn generate_functions(&self) -> Vec<TokenStream> {
        self.functions
            .iter()
            .map(|func| self.generate_function(func))
            .collect()
    }

    /// Generate all methods
    fn generate_methods(&self) -> Vec<TokenStream> {
        self.methods
            .iter()
            .map(|method| self.generate_method(method))
            .collect()
    }

    /// Generate all import (`use`) statements
    fn generate_imports(&self) -> Vec<TokenStream> {
        let mut mod1_imports = Vec::new();
        let mut mod2_imports = Vec::new();

        for fun in self.functions.iter().chain(self.methods.iter()) {
            // To use a function in a trait, we need to import the trait
            if let Some(impl_block) = &fun.f1.impl_block {
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
            if let Some(impl_block) = &fun.f2.impl_block {
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
}

const ARR_LIMIT: usize = 16;

/// Any type implement ArbitraryInit trait can generate an init statement for itself
trait ArbitraryInit {
    /// Generate an init statement for the given type
    fn init_for_type(&self, arg_name: &str, mutability: &Option<Mut>) -> TokenStream;
}

fn error_msg(msg: &str) -> proc_macro2::TokenStream {
    quote! {
        compile_error!(#msg);
    }
}

impl ArbitraryInit for Receiver {
    fn init_for_type(&self, arg_name: &str, mutability: &Option<Mut>) -> proc_macro2::TokenStream {
        let arg_ident = quote::format_ident!("{}", arg_name);
        let to_ref = match self.reference {
            Some(_) => quote! { let #arg_ident = &#mutability #arg_ident; },
            None => quote!(),
        };
        let init_stmt = quote! {
            let #mutability #arg_ident: Self = kani::any();
            #to_ref
        };
        init_stmt
    }
}

impl ArbitraryInit for TypePath {
    fn init_for_type(&self, arg_name: &str, mutability: &Option<Mut>) -> proc_macro2::TokenStream {
        // TODO: support Enum types
        let arg_ident = quote::format_ident!("{}", arg_name);
        if self.path.is_ident("u32") || self.path.is_ident("u64") || self.path.is_ident("usize") {
            quote! {
                let #mutability #arg_ident: #self = kani::any();
                kani::assume(#arg_ident < 10000);
            }
        } else if self.path.is_ident("i32") || self.path.is_ident("i64") {
            quote! {
                let #mutability #arg_ident: #self = kani::any();
                kani::assume(#arg_ident < 10000 && #arg_ident > -10000);
            }
        } else if self.path.is_ident("String") || self.path.is_ident("str") {
            init_for_string(arg_name, mutability)
        } else if self.path.segments.last().is_some() {
            let final_seg = self.path.segments.last().unwrap();
            let inner_type = &final_seg.arguments;
            // TODO: support more types, e.g., HashMap, HashSet, etc.
            if final_seg.ident == "Vec" {
                let vec_type = inner_type.clone();
                match vec_type {
                    syn::PathArguments::AngleBracketed(args) => {
                        if let Some(syn::GenericArgument::Type(ty)) = args.args.first() {
                            quote! {
                                let #mutability #arg_ident: #self = kani::vec::any_vec::<#ty, #ARR_LIMIT>();
                            }
                        } else {
                            error_msg("Unsupported Vec Type")
                        }
                    }
                    _ => error_msg("Unsupported Vec Pattern"),
                }
            } else if final_seg.ident == "Option" {
                let option_type = inner_type.clone();
                match option_type {
                    syn::PathArguments::AngleBracketed(args) => {
                        if let Some(syn::GenericArgument::Type(ty)) = args.args.first() {
                            let init_stmt = ty.init_for_type(arg_name, mutability);
                            quote! {
                                let #mutability #arg_ident: #self = if kani::any::<bool>() {
                                    #init_stmt
                                    Some(#arg_ident)
                                } else {
                                    None
                                };
                            }
                        } else {
                            error_msg("Unsupported Option Type")
                        }
                    }
                    _ => error_msg("Unsupported Option Pattern"),
                }
            } else if final_seg.ident == "Result" {
                let result_type = inner_type.clone();
                match result_type {
                    syn::PathArguments::AngleBracketed(syn::AngleBracketedGenericArguments {
                        args,
                        ..
                    }) => {
                        let ok_init = match args.first() {
                            Some(syn::GenericArgument::Type(ty)) => {
                                ty.init_for_type(arg_name, mutability)
                            }
                            _ => error_msg("Unsupported Result Type"),
                        };
                        let err_init = match args.last() {
                            Some(syn::GenericArgument::Type(ty)) => {
                                ty.init_for_type(arg_name, mutability)
                            }
                            _ => error_msg("Unsupported Result Type"),
                        };
                        quote! {
                            let #mutability #arg_ident: #self = if kani::any::<bool>() {
                                #ok_init
                                Ok(#arg_ident)
                            } else {
                                #err_init
                                Err(#arg_ident)
                            };
                        }
                    }
                    _ => error_msg("Unsupported Result Pattern"),
                }
            } else if final_seg.ident == "Range" {
                let range_type = inner_type.clone();
                match range_type {
                    syn::PathArguments::AngleBracketed(args) => {
                        if let Some(syn::GenericArgument::Type(ty)) = args.args.first() {
                            quote! {
                                let #mutability #arg_ident: #self = kani::any::<Range<#ty>>();
                            }
                        } else {
                            error_msg("Unsupported Range Type")
                        }
                    }
                    _ => error_msg("Unsupported Range Pattern"),
                }
            } else {
                // both typical types and user-defined structs are handled here
                let final_ident = &final_seg.ident;
                quote! {
                    let #mutability #arg_ident: #final_ident = kani::any();
                }
            }
        } else {
            error_msg("Failed to get the final segment of the path.")
        }
    }
}

fn init_for_string(arg_name: &str, mutability: &Option<Mut>) -> proc_macro2::TokenStream {
    const STRING_LIMIT: usize = 8;
    let arg_ident = quote::format_ident!("{}", arg_name);
    let arr_name = quote::format_ident!("{}_arr", arg_ident);
    quote! {
        let #arr_name = kani::any::<[char; #STRING_LIMIT]>();
        let #mutability #arg_ident = String::from_iter(#arr_name);
    }
}

impl ArbitraryInit for TypeArray {
    fn init_for_type(&self, arg_name: &str, mutability: &Option<Mut>) -> proc_macro2::TokenStream {
        let arr_type = &self.elem;
        let arr_len = &self.len;
        let arg_ident = quote::format_ident!("{}", arg_name);
        quote! {
            let #mutability #arg_ident = kani::any::<[#arr_type; #arr_len]>();
        }
    }
}

impl ArbitraryInit for TypeSlice {
    fn init_for_type(&self, arg_name: &str, mutability: &Option<Mut>) -> proc_macro2::TokenStream {
        let slice_type = &self.elem;
        let arg_ident = quote::format_ident!("{}", arg_name);
        quote! {
            let #mutability #arg_ident = kani::any::<[#slice_type; #ARR_LIMIT]>();
        }
    }
}

impl ArbitraryInit for TypeReference {
    fn init_for_type(&self, arg_name: &str, _mutability: &Option<Mut>) -> proc_macro2::TokenStream {
        let obj_name = quote::format_ident!("{}_obj", arg_name);
        let arg_ident = quote::format_ident!("{}", arg_name);
        let mutability = self.mutability;
        let obj_init = self.elem.init_for_type(&obj_name.to_string(), &mutability);
        match self.elem.as_ref() {
            Type::Slice(_) => {
                let slice_method = match mutability {
                    Some(_) => "kani::slice::any_slice_of_array_mut",
                    None => "kani::slice::any_slice_of_array",
                };
                let slice_method = syn::parse_str::<Path>(slice_method).unwrap();
                quote! {
                    #obj_init
                    let #arg_ident = #slice_method(&#mutability #obj_name);
                }
            }
            _ => quote! {
                #obj_init
                let #arg_ident = &#mutability #obj_name;
            },
        }
    }
}

impl ArbitraryInit for TypePtr {
    fn init_for_type(&self, arg_name: &str, _mutability: &Option<Mut>) -> proc_macro2::TokenStream {
        let arg_ident = quote::format_ident!("{}", arg_name);
        let mutability = self.mutability;
        let const_token = self.const_token;
        let elem = &self.elem;
        quote! {
            let mut generator = kani::PointerGenerator::<{if std::mem::size_of::<#elem>() > 0 {std::mem::size_of::<#elem>()} else {1}}>::new();
            let #arg_ident: *#const_token #mutability #elem = generator.any_alloc_status().ptr;
        }
    }
}

impl ArbitraryInit for Type {
    fn init_for_type(&self, arg_name: &str, mutability: &Option<Mut>) -> proc_macro2::TokenStream {
        match self {
            Type::Path(type_path) => type_path.init_for_type(arg_name, mutability),
            Type::Array(type_arr) => type_arr.init_for_type(arg_name, mutability),
            Type::Slice(type_slice) => type_slice.init_for_type(arg_name, mutability),
            Type::Tuple(type_tuple) => type_tuple.init_for_type(arg_name, mutability),
            Type::Reference(type_ref) => type_ref.init_for_type(arg_name, mutability),
            Type::Ptr(type_ptr) => type_ptr.init_for_type(arg_name, mutability),
            _ => error_msg("Unsupported argument type"),
        }
    }
}

impl ArbitraryInit for TypeTuple {
    fn init_for_type(&self, arg_name: &str, mutability: &Option<Mut>) -> proc_macro2::TokenStream {
        let tuple_elems = self.elems.iter().map(|elem| {
            let elem_name = quote::format_ident!("{}_elem", arg_name);
            let elem_init = elem.init_for_type(&elem_name.to_string(), mutability);
            quote! {
                {
                    #elem_init
                    #elem_name
                }
            }
        });
        let arg_ident = quote::format_ident!("{}", arg_name);
        quote! {
            let #mutability #arg_ident = (#(#tuple_elems),*);
        }
    }
}
