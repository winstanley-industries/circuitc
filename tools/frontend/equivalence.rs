use std::env;
use std::fs;
use std::path::PathBuf;
use std::process::ExitCode;

use circuitc::compile;
use circuitc::demo::voltage_divider;
use circuitc::frontend::{DiagnosticFormat, compile_source, render_diagnostics};

fn main() -> ExitCode {
    let Some(input) = env::args_os().nth(1).map(PathBuf::from) else {
        eprintln!("usage: frontend_equivalence SOURCE");
        return ExitCode::from(2);
    };
    if env::args_os().nth(2).is_some() {
        eprintln!("usage: frontend_equivalence SOURCE");
        return ExitCode::from(2);
    }
    let source = match fs::read_to_string(&input) {
        Ok(source) => source,
        Err(error) => {
            eprintln!("failed to read {}: {error}", input.display());
            return ExitCode::FAILURE;
        }
    };
    let source_compiled = match compile_source(input.to_string_lossy(), source) {
        Ok(compiled) => compiled,
        Err(diagnostics) => {
            eprint!(
                "{}",
                render_diagnostics(&diagnostics, DiagnosticFormat::Human)
            );
            return ExitCode::FAILURE;
        }
    };
    let rust_design = voltage_divider();
    if source_compiled.elaborated.design != rust_design {
        eprintln!("source-authored and Rust-authored Design IR values differ");
        return ExitCode::FAILURE;
    }
    let rust_compiled = match compile(&rust_design) {
        Ok(compiled) => compiled,
        Err(error) => {
            eprintln!("Rust-authored reference failed to compile: {error}");
            return ExitCode::FAILURE;
        }
    };
    if source_compiled.artifacts != rust_compiled {
        eprintln!("source-authored and Rust-authored compiled artifacts or name maps differ");
        return ExitCode::FAILURE;
    }
    println!(
        "source and Rust fixtures are equal at Design IR, artifact, and SPICE name-map boundaries"
    );
    ExitCode::SUCCESS
}
