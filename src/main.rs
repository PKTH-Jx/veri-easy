use crate::{
    checker::{CheckStep, Checker},
    source::Source,
    steps::{Alive2, DifferentialFuzzing, Identical, Kani, PropertyBasedTesting},
};

mod checker;
mod function;
mod source;
mod steps;

// In real usage, create Sources from file paths and run Checker with steps.
fn main() -> anyhow::Result<()> {
    let s1 = Source::new("a.rs")?;
    let s2 = Source::new("b.rs")?;
    let steps: Vec<Box<dyn CheckStep>> = vec![
        Box::new(Identical),
        // Box::new(Kani),
        // Box::new(DifferentialFuzzing),
        Box::new(PropertyBasedTesting),
        // Box::new(Alive2::new(
        //     "/Users/jingx/Dev/os/verif/cmpir/alive2/build/alive-tv".to_owned(),
        // )),
    ];

    let mut checker = Checker::new(s1, s2, steps);
    checker.print_state();

    match checker.run_all() {
        Ok(()) => println!("✅ All functions consistent / checked."),
        Err(e) => println!("❌ Check failed or undetermined: {}", e),
    }

    Ok(())
}
