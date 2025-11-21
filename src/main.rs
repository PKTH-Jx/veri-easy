use crate::{
    check::{Checker, Component, Source},
    components::{Alive2, DifferentialFuzzing, Identical, Kani, PropertyBasedTesting},
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

    log::init_logger(log::LogLevel::Normal);
    log!(
        Brief,
        Critical,
        "Starting verification between `{}` and `{}`\n",
        s1.path,
        s2.path
    );

    let mut checker = Checker::new(s1, s2, steps);
    log!(Normal, Info, "Logging initial state:");
    checker.print_state();
    log!(Normal, Simple, "");
    checker.run_all();

    Ok(())
}
