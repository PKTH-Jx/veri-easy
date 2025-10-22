use std::process::Command;

use crate::{
    checker::{CheckResult, CheckStep},
    function::export_functions,
    source::Source,
};
use anyhow::anyhow;

/// Alive2 step: use alive-tv to check function equivalence.
pub struct Alive2 {
    /// Alive-tv path
    pub path: String,
}

impl Alive2 {
    pub fn new(path: String) -> Self {
        Self { path }
    }

    fn compile_to_llvm_ir(&self, src: &str, out: &str) -> anyhow::Result<()> {
        let original =
            std::fs::read_to_string(src).map_err(|_| anyhow!("Failed to read source"))?;
        let exported = export_functions(&original)?;
        let tmp_path = "tmp.rs";
        std::fs::write(&tmp_path, exported).map_err(|_| anyhow!("Failed to write tmp file"))?;

        Command::new("rustc")
            .args(["--emit=llvm-ir", "--crate-type=lib", tmp_path, "-o", out])
            .stderr(std::fs::File::open("/dev/null").unwrap())
            .status()
            .map(|_| ())
            .map_err(|_| anyhow!("Failed to compile to llvm-ir"))?;

        std::fs::remove_file(tmp_path).map_err(|_| anyhow!("Failed to remove tmp file"))
    }

    fn remove_llvm_ir(&self, path: &str) -> anyhow::Result<()> {
        std::fs::remove_file(path).map_err(|_| anyhow!("Failed to remove llvm-ir"))
    }

    fn run_alive2(&self, ir1: &str, ir2: &str) -> anyhow::Result<String> {
        let tmp_path = "alive2.tmp";
        let tmp_file =
            std::fs::File::create(tmp_path).map_err(|_| anyhow!("Failed to create tmp file"))?;
        Command::new(self.path.clone())
            .args([ir1, ir2])
            .stdout(tmp_file)
            .status()
            .map_err(|_| anyhow!("Failed to run alive2"))?;
        let output =
            std::fs::read_to_string(tmp_path).map_err(|_| anyhow!("Failed to read tmp file"))?;
        std::fs::remove_file(tmp_path).map_err(|_| anyhow!("Failed to remove tmp file"))?;
        Ok(output)
    }

    fn analyze_alive2_output(&self, output: &str) -> CheckResult {
        let mut res = CheckResult {
            status: Ok(()),
            ok: vec![],
            fail: vec![],
        };

        let mut func_name: Option<String> = None;

        for line in output.lines() {
            if line.starts_with("define") {
                if func_name.is_none() {
                    let at = line.find("@").unwrap();
                    let parenthese = line.find('(').unwrap();
                    func_name = Some(line[at + 1..parenthese].to_string().replace("___", "::"));
                }
            } else if line.starts_with("Transformation seems to be correct!") {
                res.ok.push(func_name.take().unwrap());
            } else if line.starts_with("ERROR") {
                func_name = None;
            }
        }

        res
    }
}

impl CheckStep for Alive2 {
    fn name(&self) -> &str {
        "Alive2"
    }

    fn note(&self) -> Option<&str> {
        Some("Use alive-tv to check function equivalence")
    }

    fn run(&self, src1: &Source, src2: &Source) -> CheckResult {
        let out1 = "alive2_1.ll";
        let out2 = "alive2_2.ll";

        let res = self.compile_to_llvm_ir(&src1.path, out1);
        if let Err(e) = res {
            return CheckResult::failed(e);
        }
        let res = self.compile_to_llvm_ir(&src2.path, out2);
        if let Err(e) = res {
            return CheckResult::failed(e);
        }

        let res = self.run_alive2(out1, out2);
        if let Err(e) = res {
            return CheckResult::failed(e);
        }

        let res = self.analyze_alive2_output(&res.unwrap());
        self.remove_llvm_ir(out1).unwrap();
        self.remove_llvm_ir(out2).unwrap();

        res
    }
}
