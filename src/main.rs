use crate::{
    check::{Checker, Component, Source},
    components::{Alive2, DifferentialFuzzing, Identical, Kani, PropertyBasedTesting},
    defs::{Path, Precondition},
};

mod check;
mod collect;
mod components;
mod defs;
mod generate;
mod log;
mod utils;

// In real usage, create Sources from file paths and run Checker with steps.
fn main() -> anyhow::Result<()> {
    log::init_logger(log::LogLevel::Normal);

    let s1 = Source::open("v1_impl.rs")?;
    let s2 = Source::open("v2_impl.rs")?;
    let steps: Vec<Box<dyn Component>> = vec![
        Box::new(Identical),
        Box::new(Kani),
        Box::new(PropertyBasedTesting),
        Box::new(DifferentialFuzzing),
        Box::new(Alive2::new(
            "/Users/jingx/Dev/os/verif/cmpir/alive2/build/alive-tv".to_owned(),
        )),
    ];

    let precond_gen = precond_translator::parse_file_and_create_generator("v2_proof.rs")?;

    let gen_code = precond_gen.generate_all();
    let pretty_code = prettyplease::unparse(&syn::parse2(gen_code).unwrap());
    std::fs::write("pre.rs", pretty_code)?;

    let mut precondtions = Vec::new();
    for func in precond_gen.get_function_preconds() {
        precondtions.push(Precondition::new(Path::from_str(&func), false));
    }
    for method in precond_gen.get_method_preconds() {
        precondtions.push(Precondition::new(Path::from_str(&method), true));
    }

    log!(
        Brief,
        Critical,
        "Starting verification between `{}` and `{}`\n",
        s1.path,
        s2.path
    );

    let mut checker = Checker::new(s1, s2, steps, precondtions);
    log!(Normal, Info, "Logging initial state:");
    checker.print_state();
    log!(Normal, Simple, "");
    checker.run_all();

    Ok(())
}
