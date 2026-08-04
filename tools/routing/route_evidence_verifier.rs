use std::env;
use std::fs;
use std::io::{self, Write};
use std::path::Path;

const MAX_CONTRACT_BYTES: u64 = 64 * 1024 * 1024;
const MAX_PROVENANCE_BYTES: u64 = 1_024;

fn main() {
    let arguments: Vec<_> = env::args_os().collect();
    if arguments.len() != 4 {
        eprintln!("usage: route_evidence_verifier REQUEST RESULT PROVENANCE");
        std::process::exit(2);
    }
    let request = read_bounded(Path::new(&arguments[1]), MAX_CONTRACT_BYTES);
    let result = read_bounded(Path::new(&arguments[2]), MAX_CONTRACT_BYTES);
    let provenance = read_bounded(Path::new(&arguments[3]), MAX_PROVENANCE_BYTES);
    let verified = circuitc::verify_apgar_route_evidence(&request, &result, &provenance)
        .unwrap_or_else(|error| {
            eprintln!("CircuitC APGAR evidence verification failed: {error}");
            std::process::exit(1);
        });
    io::stdout()
        .write_all(verified.as_bytes())
        .unwrap_or_else(|error| {
            eprintln!("could not write verified APGAR evidence: {error}");
            std::process::exit(1);
        });
}

fn read_bounded(path: &Path, limit: u64) -> String {
    let metadata = fs::metadata(path).unwrap_or_else(|error| {
        eprintln!("could not inspect {}: {error}", path.display());
        std::process::exit(1);
    });
    if !metadata.is_file() || metadata.len() > limit {
        eprintln!("{} is not a bounded regular file", path.display());
        std::process::exit(1);
    }
    let value = fs::read_to_string(path).unwrap_or_else(|error| {
        eprintln!("could not read {}: {error}", path.display());
        std::process::exit(1);
    });
    if value.len() as u64 != metadata.len() {
        eprintln!("{} changed while it was read", path.display());
        std::process::exit(1);
    }
    value
}
