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
    let mut outputs = vec![
        (
            "voltage_divider.kicad_sch".to_owned(),
            artifacts.kicad_schematic,
        ),
        ("voltage_divider.kicad_pcb".to_owned(), artifacts.kicad_pcb),
        (
            "voltage_divider.kicad_pro".to_owned(),
            artifacts.kicad_project,
        ),
    ];
    outputs.extend(
        artifacts
            .kicad_library_files
            .into_iter()
            .map(|file| (file.relative_path.into_string(), file.contents)),
    );
    outputs.extend([
        ("sym-lib-table".to_owned(), artifacts.kicad_symbol_table),
        ("fp-lib-table".to_owned(), artifacts.kicad_footprint_table),
        ("voltage_divider.spice".to_owned(), artifacts.spice),
    ]);
    for (filename, contents) in outputs {
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
