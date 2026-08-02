use std::env;
use std::fs;
use std::path::PathBuf;
use std::process::ExitCode;

use circuitc::compile;
use circuitc::demo::voltage_divider;

fn main() -> ExitCode {
    let Some(output_directory) = env::args_os().nth(1).map(PathBuf::from) else {
        eprintln!("usage: voltage_divider OUTPUT_DIRECTORY");
        return ExitCode::from(2);
    };
    if env::args_os().nth(2).is_some() {
        eprintln!("usage: voltage_divider OUTPUT_DIRECTORY");
        return ExitCode::from(2);
    }

    let artifacts = match compile(&voltage_divider()) {
        Ok(artifacts) => artifacts,
        Err(error) => {
            eprintln!("{error}");
            return ExitCode::FAILURE;
        }
    };

    if let Err(error) = fs::create_dir_all(&output_directory) {
        eprintln!(
            "failed to create output directory {}: {error}",
            output_directory.display()
        );
        return ExitCode::FAILURE;
    }
    for (filename, contents) in [
        ("voltage_divider.kicad_sch", artifacts.kicad_schematic),
        ("voltage_divider.kicad_pcb", artifacts.kicad_pcb),
        ("voltage_divider.kicad_pro", artifacts.kicad_project),
        ("CircuitC.kicad_sym", artifacts.kicad_symbol_library),
        (
            "CircuitC.pretty/R_0603_1608Metric.kicad_mod",
            artifacts.kicad_footprint_library,
        ),
        ("sym-lib-table", artifacts.kicad_symbol_table),
        ("fp-lib-table", artifacts.kicad_footprint_table),
        ("voltage_divider.spice", artifacts.spice),
    ] {
        let path = output_directory.join(filename);
        if let Some(parent) = path.parent()
            && let Err(error) = fs::create_dir_all(parent)
        {
            eprintln!("failed to create {}: {error}", parent.display());
            return ExitCode::FAILURE;
        }
        if let Err(error) = fs::write(&path, contents) {
            eprintln!("failed to write {}: {error}", path.display());
            return ExitCode::FAILURE;
        }
        println!("wrote {}", path.display());
    }

    ExitCode::SUCCESS
}
