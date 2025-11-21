//! Utility functions and helpers.

use crate::log;
use std::{
    io::BufRead,
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

    Ok(output)
}
