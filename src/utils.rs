//! Utility functions and helpers.

use crate::log;
use anyhow::anyhow;
use std::{
    io::{BufRead, Write},
    process::{Command, Output},
};

/// Run a subprocess command and log its stderr though global logger.
pub fn run_command_and_log_error(program: &str, args: &[&str]) -> anyhow::Result<Output> {
    log!(
        Verbose,
        Info,
        "Logging stderr of command '{} {}':",
        program,
        args.join(" ")
    );
    let output = Command::new(program)
        .args(args)
        .stderr(std::process::Stdio::piped())
        .output()
        .map_err(|e| anyhow::anyhow!("Failed to run command: {}", e))?;

    let reader = std::io::BufReader::new(output.stderr.as_slice());
    for line in reader.lines() {
        if let Ok(line) = line {
            log!(Verbose, Simple, "{}", line);
        }
    }

    if output.status.success() {
        log!(
            Verbose,
            Info,
            "Command '{}' finished successfully.",
            program
        );
    } else {
        log!(
            Brief,
            Warning,
            "Command '{}' failed with exit code: {}",
            program,
            output.status
        );
    }
    Ok(output)
}


/// Create a typical harness project directory structure. Dir structure:
///
/// harness_path
/// ├── Cargo.toml
/// └── src
///     ├── main.rs
///     ├── mod1.rs
///     └── mod2.rs
pub fn create_harness_project(
    path: &str,
    src1: &str,
    src2: &str,
    harness: &str,
    toml: &str,
    lib: bool,
) -> anyhow::Result<()> {
    // Remove existing directory if any
    if std::path::Path::new(path).exists() {
        std::fs::remove_dir_all(path)
            .map_err(|_| anyhow!("Failed to remove existing harness directory"))?;
    }
    let project_type = if lib { "--lib" } else { "--bin" };
    run_command_and_log_error("cargo", &["new", project_type, "--vcs", "none", path])?;
    let harness_file = path.to_owned() + if lib {
        "/src/lib.rs"
    } else {
        "/src/main.rs"
    };

    // Write rust files
    std::fs::File::create(path.to_owned() + "/src/mod1.rs")
        .unwrap()
        .write_all(src1.as_bytes())
        .map_err(|_| anyhow!("Failed to write mod1 file"))?;
    std::fs::File::create(path.to_owned() + "/src/mod2.rs")
        .unwrap()
        .write_all(src2.as_bytes())
        .map_err(|_| anyhow!("Failed to write mod2 file"))?;
    std::fs::File::create(harness_file)
        .unwrap()
        .write_all(harness.as_bytes())
        .map_err(|_| anyhow!("Failed to write harness file"))?;

    // Write Cargo.toml
    std::fs::File::create(path.to_owned() + "/Cargo.toml")
        .unwrap()
        .write_all(toml.as_bytes())
        .map_err(|_| anyhow!("Failed to write Cargo.toml"))?;

    // Cargo fmt
    let cur_dir = std::env::current_dir().unwrap();
    let _ = std::env::set_current_dir(path);
    run_command_and_log_error("cargo", &["fmt"])?;
    let _ = std::env::set_current_dir(cur_dir);

    Ok(())
}
