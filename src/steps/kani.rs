//! Use model-checker Kani

use anyhow::anyhow;
use regex::Regex;
use std::{io::Write, process::Command, str::FromStr};

use proc_macro2::TokenStream;
use quote::quote;
use syn::{
    AngleBracketedGenericArguments, FnArg, Ident, ImplItem, ImplItemFn, Item, ItemFn, ItemImpl,
    ItemStruct, Pat, Path, Receiver, ReturnType, Type, TypeArray, TypePath, TypePtr, TypeReference,
    TypeSlice, TypeTuple, token::Mut,
};

use crate::{
    checker::{CheckResult, CheckStep},
    function::Function,
    source::Source,
};

/// Kani step: use Kani model-checker to check function equivalence.
pub struct Kani;

impl Kani {
    fn generate_harness(&self, src1: &Source, src2: &Source) -> anyhow::Result<TokenStream> {
        let content1 =
            TokenStream::from_str(&src1.content).map_err(|_| anyhow!("Invalid Rust code"))?;
        let mod1 = quote! {
            mod mod1 {
                #content1
            }
        };
        let content2 =
            TokenStream::from_str(&src2.content).map_err(|_| anyhow!("Invalid Rust code"))?;
        let mod2 = quote! {
            mod mod2 {
                #content2
            }
        };
        let harness_fn = src1
            .unchecked_funcs
            .iter()
            .filter(|f| f.impl_type.is_none())
            .map(|f| generate_harness_fn(f))
            .collect::<Vec<_>>();
        let harness_method = src1
            .unchecked_funcs
            .iter()
            .filter(|f| f.impl_type.is_some())
            .map(|f| generate_harness_method(f, f.impl_type.as_ref().unwrap()))
            .collect::<Vec<_>>();

        Ok(quote! {
            #mod1

            #mod2

            #(#harness_fn)*

            #(#harness_method)*
        })
    }

    fn write_harness_file(&self, harness: TokenStream) -> anyhow::Result<()> {
        let mut file = std::fs::File::create("harness.rs")?;
        file.write(harness.to_string().as_bytes())?;
        Command::new("rustfmt").arg("harness.rs").status()?;
        Ok(())
    }

    fn run_kani(&self) -> anyhow::Result<String> {
        let tmp_path = "kani.tmp";
        let tmp_file =
            std::fs::File::create(tmp_path).map_err(|_| anyhow!("Failed to create tmp file"))?;
        Command::new("kani")
            .arg("harness.rs")
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
                res.fail.push(func_name.take().unwrap());
            }
        }

        res
    }

    fn remove_harness_file(&self) -> anyhow::Result<()> {
        std::fs::remove_file("harness.rs").map_err(|_| anyhow!("Failed to remove harness file"))?;
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

    fn run(&self, src1: &Source, src2: &Source) -> CheckResult {
        let res = self.generate_harness(src1, src2);
        if let Err(e) = res {
            return CheckResult::failed(e);
        }
        let harness = res.unwrap();

        let res = self.write_harness_file(harness);
        if let Err(e) = res {
            return CheckResult::failed(e);
        }

        let res = self.run_kani();
        if let Err(e) = res {
            return CheckResult::failed(e);
        }
        let output = res.unwrap();

        let check_res = self.analyze_kani_output(&output);

        if let Err(e) = self.remove_harness_file() {
            return CheckResult::failed(e);
        }

        check_res
    }
}

/// Automatedly generate one Kani harness for comparing two free-standing functions.
fn generate_harness_fn(func: &Function) -> TokenStream {
    let harness_name = quote::format_ident!("check___{}", func.name.replace("::", "___"));
    let func_name = TokenStream::from_str(&func.name).unwrap();
    let inputs = &func.item.sig.inputs;

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
            call_args.push(quote! { #arg_ident });
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

/// Automatedly generate one Kani harness for comparing two methods.
fn generate_harness_method(func: &Function, state_type: &Type) -> TokenStream {
    let harness_name = quote::format_ident!("check___{}", func.name.replace("::", "___"));
    let func_name = TokenStream::from_str(&func.name).unwrap();
    let inputs = &func.item.sig.inputs;

    let mut harness_body = Vec::new();
    let mut call_args: Vec<TokenStream> = Vec::new();

    let ident1 = quote::format_ident!("s1");
    let ident2 = quote::format_ident!("s2");
    let mut reference = None;
    let mut mutability = None;

    for arg in inputs {
        match arg {
            FnArg::Receiver(receiver) => {
                mutability = receiver.mutability.clone();
                reference = receiver.reference.clone();
                // Init abstract state
                let init_state_stmt = quote! {
                    // TODO
                    let state: u64  = kani::any();
                };
                // state_type.init_for_type("state", mutability);
                let into_stmt = quote! {
                    let #mutability #ident1 = state.into();
                    let #mutability #ident2 = state.into();
                };
                harness_body.push(init_state_stmt);
                harness_body.push(into_stmt);
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
                call_args.push(quote! { #arg_ident });
            }
        }
    }

    let lifetime = match &reference {
        Some((_, lt)) => lt.clone(),
        None => None,
    };
    let reference = reference.map(|(and, lt)| and);

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
                let #mutability #arg_ident = kani::any();
                kani::assume(#arg_ident < 100000000);
            }
        } else if self.path.is_ident("i32") || self.path.is_ident("i64") {
            quote! {
                let #mutability #arg_ident = kani::any();
                kani::assume(#arg_ident < 100000000 && #arg_ident > -100000000);
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
                                let #mutability #arg_ident = kani::vec::any_vec::<#ty, #ARR_LIMIT>();
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
                                let #mutability #arg_ident = if kani::any::<bool>() {
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
                    syn::PathArguments::AngleBracketed(AngleBracketedGenericArguments {
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
                            let #mutability #arg_ident = if kani::any::<bool>() {
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
            _ => error_msg("Unsupported argument type for `kani_test` macro."),
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

// /// Impl `Arbitrary` for a struct by generating the `any` method based on the fields of the struct.
// ///
// /// Since some common types (e.g., Vec) do not impl `Arbitrary`, it's inpractical to derive `Arbitrary`.
// /// Instead, this macro generates the impl block for `Arbitrary`.
// pub fn kani_arbitrary(_attr: TokenStream, item: TokenStream) -> TokenStream {
//     let input = parse_macro_input!(item as Item);
//     let struct_def = match input {
//         Item::Struct(ref struct_def) => struct_def,
//         _ => {
//             return error_msg("`kani_arbitrary` can only be used on structs.").into();
//         }
//     };
//     let impl_stmt = impl_arbitrary_via_fields(&struct_def);
//     let output = quote! {
//         #struct_def

//         #impl_stmt
//     };
//     output.into()
// }

// fn impl_arbitrary_via_fields(struct_def: &ItemStruct) -> proc_macro2::TokenStream {
//     let mutability: Option<Mut> = None;
//     let struct_name = &struct_def.ident;
//     let init_stmt = match &struct_def.fields {
//         syn::Fields::Named(fields) => {
//             let mut fields_init: Vec<proc_macro2::TokenStream> = Vec::new();
//             for field in &fields.named {
//                 let field_name = field.ident.as_ref().unwrap();
//                 let field_type = &field.ty;
//                 let obj = field_type.init_for_type(&field_name.to_string(), &mutability);
//                 let field_init = quote! {
//                     #field_name: {
//                         #obj
//                         #field_name
//                     }
//                 };
//                 fields_init.push(field_init);
//             }

//             quote! {
//                 Self {
//                     #(#fields_init),*
//                 }
//             }
//         }
//         syn::Fields::Unnamed(_fields) => {
//             todo!("Fields::Unnamed");
//         }
//         syn::Fields::Unit => {
//             todo!("Fields::Unit");
//         }
//     };
//     quote! {
//         /// Arbitrary impl Generated by autokani
//         #[cfg(any(kani, feature = "debug_log"))]
//         impl kani::Arbitrary for #struct_name {
//             /// Automatically generate the `any` method based on fields
//             fn any() -> Self {
//                 #init_stmt
//             }
//         }
//     }
// }

// /// Extend the `Arbitrary` trait for target struct based on its constructor(e.g., `new` method).
// /// Add this attribute to the impl block of the struct.
// pub fn extend_arbitrary(_attr: TokenStream, item: TokenStream) -> TokenStream {
//     let input = parse_macro_input!(item as Item);
//     let impl_block = match input {
//         Item::Impl(impl_block) => impl_block,
//         _ => {
//             return error_msg("`extend_arbitrary` can only be used on impl blocks.").into();
//         }
//     };
//     let impl_arbitrary = impl_arbitrary_via_constructor(&impl_block);
//     let output = quote! {
//         #impl_block
//         #impl_arbitrary
//     };
//     output.into()
// }

// fn find_constructor(impl_block: &ItemImpl, struct_name: Option<&Ident>) -> Option<ImplItemFn> {
//     for item in &impl_block.items {
//         if let ImplItem::Method(method) = item {
//             if let ReturnType::Type(_, return_type) = &method.sig.output {
//                 if let Type::Path(type_path) = &**return_type {
//                     if type_path.path.is_ident("Self") {
//                         return Some(method.clone());
//                     }
//                     if let Some(name) = struct_name {
//                         if type_path.path.is_ident(name) {
//                             return Some(method.clone());
//                         }
//                     }
//                 }
//             }
//         }
//     }
//     None
// }

// fn impl_arbitrary_via_constructor(impl_block: &ItemImpl) -> Option<proc_macro2::TokenStream> {
//     // This function should generate the `Arbitrary` impl block based on the `new` method.
//     // You need to parse the `new` method and generate the corresponding `Arbitrary` impl block.
//     let struct_name = match &*impl_block.self_ty {
//         Type::Path(type_path) => type_path.path.get_ident(),
//         _ => {
//             return error_msg("`extend_arbitrary` can only be used on impl blocks of structs.")
//                 .into();
//         }
//     };
//     // let constructor = find_constructor(impl_block, struct_name)?;
//     let constructor = match find_constructor(impl_block, struct_name) {
//         Some(constructor) => constructor,
//         None => {
//             return Some(quote! {
//                     impl kani::Arbitrary for Placeholder222 {
//                     }
//             });
//         }
//     };
//     let inputs = &constructor.sig.inputs;
//     let func_name = &constructor.sig.ident;
//     let mut init_code = Vec::new();
//     let mut call_args: Vec<proc_macro2::TokenStream> = Vec::new();

//     for arg in inputs {
//         match arg {
//             FnArg::Receiver(_) => unreachable!(),
//             FnArg::Typed(pat_type) => {
//                 let arg_name = match &*pat_type.pat {
//                     syn::Pat::Ident(pat_ident) => pat_ident.ident.to_string(),
//                     _ => "arg".to_string(),
//                 };
//                 let arg_type = &pat_type.ty;
//                 let mutability = match &*pat_type.pat {
//                     Pat::Ident(pat_ident) => pat_ident.mutability,
//                     _ => None,
//                 };
//                 let init_stmt = arg_type.init_for_type(&arg_name, &mutability);
//                 init_code.push(init_stmt);

//                 let arg_ident = quote::format_ident!("{}", arg_name);
//                 call_args.push(quote! { #arg_ident });
//             }
//         }
//     }
//     let impl_generics = &impl_block.generics;
//     let self_ty = &impl_block.self_ty;
//     let where_clause = &impl_block.generics.where_clause;
//     Some(quote! {
//         /// Arbitrary impl Generated by autokani
//         #[cfg(any(kani, feature = "debug_log"))]
//         impl #impl_generics kani::Arbitrary for #self_ty #where_clause {
//             /// Automatically generate the `any` method based on constructor
//             fn any() -> Self {
//                 #(#init_code)*
//                 Self::#func_name(#(#call_args),*)
//             }
//         }
//     })
// }
