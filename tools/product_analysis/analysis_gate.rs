use std::env;
use std::fs::{Metadata, OpenOptions};
use std::io::{Read, Write};
use std::os::unix::fs::{MetadataExt, OpenOptionsExt};
use std::path::{Path, PathBuf};
use std::process::ExitCode;

use circuitc::frontend::compile_source;
use circuitc::manufacturing::{
    FabricationCompilerArtifacts, FabricationHostFile, bind_kicad10_fabrication,
    prepare_kicad10_fabrication_request,
};
use circuitc::product::compile_product_artifacts;
use circuitc::product_analysis::{
    BoardAnalysisHostEvidence, bind_kicad10_board_analysis, prepare_kicad10_board_analysis_request,
    verify_kicad10_board_analysis,
};

const ANALYSIS_PATH: &str = "release.manufacturability";
const FABRICATION_ASSERTION: &str = "release.manufacturability.fabrication";
const KICAD_VERSION: &str = "10.0.5";
const MAX_FILE_BYTES: u64 = 64 * 1024 * 1024;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
struct FileIdentity {
    device: u64,
    inode: u64,
    length: u64,
    mode: u32,
    links: u64,
    modified_seconds: i64,
    modified_nanoseconds: i64,
    changed_seconds: i64,
    changed_nanoseconds: i64,
}

fn identity(metadata: &Metadata) -> FileIdentity {
    FileIdentity {
        device: metadata.dev(),
        inode: metadata.ino(),
        length: metadata.len(),
        mode: metadata.mode(),
        links: metadata.nlink(),
        modified_seconds: metadata.mtime(),
        modified_nanoseconds: metadata.mtime_nsec(),
        changed_seconds: metadata.ctime(),
        changed_nanoseconds: metadata.ctime_nsec(),
    }
}

fn read_bounded(path: &Path) -> Result<Vec<u8>, String> {
    let mut file = OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_NOFOLLOW | libc::O_CLOEXEC)
        .open(path)
        .map_err(|error| format!("failed to open bounded input {}: {error}", path.display()))?;
    let before = file
        .metadata()
        .map_err(|error| format!("failed to inspect {}: {error}", path.display()))?;
    if !before.is_file() || before.nlink() != 1 || before.len() > MAX_FILE_BYTES {
        return Err(format!(
            "input is not a bounded single-link regular file: {}",
            path.display()
        ));
    }
    let mut bytes = Vec::with_capacity(
        usize::try_from(before.len()).map_err(|_| "input length does not fit usize")?,
    );
    (&mut file)
        .take(MAX_FILE_BYTES + 1)
        .read_to_end(&mut bytes)
        .map_err(|error| format!("failed to read {}: {error}", path.display()))?;
    let after = file
        .metadata()
        .map_err(|error| format!("failed to re-inspect {}: {error}", path.display()))?;
    if bytes.len() as u64 != before.len() || identity(&before) != identity(&after) {
        return Err(format!("input changed while read: {}", path.display()));
    }
    Ok(bytes)
}

fn parse_utf8(path: &Path, bytes: Vec<u8>) -> Result<String, String> {
    String::from_utf8(bytes).map_err(|_| format!("input is not UTF-8: {}", path.display()))
}

struct Inputs {
    source_path: PathBuf,
    catalog_path: PathBuf,
    variant: String,
    raw_root: PathBuf,
    kicad_cli: PathBuf,
}

fn parse_common(arguments: &[String]) -> Result<Inputs, String> {
    if arguments.len() < 7 {
        return Err(
            "usage: analysis_gate <prepare|bind> <source> <catalog> <variant> <fabrication-raw-root> <kicad-cli> [normalizer host-runner analysis-output-root]"
                .to_owned(),
        );
    }
    Ok(Inputs {
        source_path: PathBuf::from(&arguments[2]),
        catalog_path: PathBuf::from(&arguments[3]),
        variant: arguments[4].clone(),
        raw_root: PathBuf::from(&arguments[5]),
        kicad_cli: PathBuf::from(&arguments[6]),
    })
}

fn run(arguments: &[String]) -> Result<(), String> {
    let mode = arguments
        .get(1)
        .ok_or_else(|| "analysis gate mode is missing".to_owned())?;
    if !matches!(mode.as_str(), "prepare" | "bind") {
        return Err("analysis gate mode must be prepare or bind".to_owned());
    }
    let inputs = parse_common(arguments)?;
    if mode == "prepare" && arguments.len() != 7 {
        return Err("prepare mode received unexpected arguments".to_owned());
    }
    if mode == "bind" && arguments.len() != 10 {
        return Err(
            "bind mode requires normalizer, host-runner, and analysis-output-root".to_owned(),
        );
    }
    let source = parse_utf8(&inputs.source_path, read_bounded(&inputs.source_path)?)?;
    let catalog = read_bounded(&inputs.catalog_path)?;
    let compiled_source = compile_source("input.circuitc", source)
        .map_err(|diagnostics| format!("source compilation failed: {diagnostics:?}"))?;
    let design = &compiled_source.elaborated.design;
    let compiled = &compiled_source.artifacts;
    let product = compile_product_artifacts(design, &catalog, &inputs.variant)
        .map_err(|diagnostics| format!("product compilation failed: {diagnostics:?}"))?;
    let compiler = FabricationCompilerArtifacts::Static(compiled);
    let fabrication_request = prepare_kicad10_fabrication_request(
        design,
        &catalog,
        &inputs.variant,
        compiler,
        &product,
        ANALYSIS_PATH,
        FABRICATION_ASSERTION,
    )
    .map_err(|error| error.to_string())?;
    let mut host_files = Vec::new();
    for relative in fabrication_request.expected_host_paths {
        host_files.push(FabricationHostFile {
            contents: read_bounded(&inputs.raw_root.join(relative.as_path()))?,
            path: relative,
        });
    }
    let executable = read_bounded(&inputs.kicad_cli)?;
    let fabrication = bind_kicad10_fabrication(
        design,
        &catalog,
        &inputs.variant,
        compiler,
        &product,
        ANALYSIS_PATH,
        FABRICATION_ASSERTION,
        KICAD_VERSION,
        &executable,
        &host_files,
    )
    .map_err(|error| error.to_string())?;
    if mode == "prepare" {
        let request = prepare_kicad10_board_analysis_request(
            design,
            &catalog,
            &inputs.variant,
            compiler,
            &product,
            ANALYSIS_PATH,
            &compiled_source.kicad_identity_map,
            &fabrication,
        )
        .map_err(|error| error.to_string())?;
        std::io::stdout()
            .lock()
            .write_all(request.request_json().as_bytes())
            .map_err(|error| format!("failed to emit analysis request: {error}"))?;
        return Ok(());
    }

    let normalizer_path = Path::new(&arguments[7]);
    let host_runner_path = Path::new(&arguments[8]);
    let output_root = Path::new(&arguments[9]);
    let normalizer = read_bounded(normalizer_path)?;
    let evidence = BoardAnalysisHostEvidence {
        host_version: KICAD_VERSION.to_owned(),
        host_executable: executable,
        normalizer,
        host_runner: read_bounded(host_runner_path)?,
        erc_report_json: read_bounded(&output_root.join("erc.normalized.json"))?,
        drc_report_json: read_bounded(&output_root.join("drc.normalized.json"))?,
        receipt_json: read_bounded(&output_root.join("receipt.json"))?,
    };
    let bundle = bind_kicad10_board_analysis(
        design,
        &catalog,
        &inputs.variant,
        compiler,
        &product,
        ANALYSIS_PATH,
        &compiled_source.kicad_identity_map,
        &fabrication,
        &evidence,
    )
    .map_err(|error| error.to_string())?;
    verify_kicad10_board_analysis(
        design,
        &catalog,
        &inputs.variant,
        compiler,
        &product,
        ANALYSIS_PATH,
        &compiled_source.kicad_identity_map,
        &fabrication,
        &evidence,
        &bundle,
    )
    .map_err(|error| error.to_string())?;
    std::io::stdout()
        .lock()
        .write_all(bundle.report_json().as_bytes())
        .map_err(|error| format!("failed to emit analysis report: {error}"))?;
    Ok(())
}

fn main() -> ExitCode {
    let arguments: Vec<String> = env::args().collect();
    match run(&arguments) {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            let _ = writeln!(
                std::io::stderr().lock(),
                "board analysis gate failed: {error}"
            );
            ExitCode::FAILURE
        }
    }
}
