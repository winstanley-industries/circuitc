use std::env;
use std::fs;
use std::io::{self, Read};
use std::path::Path;

use sha2::{Digest, Sha256};

const SOURCE_REVISION: &str = env!("CIRCUITC_OHMNIVORE_SOURCE_REVISION");
const MAX_EXECUTABLE_BYTES: u64 = 2 * 1024 * 1024 * 1024;

fn main() {
    let arguments: Vec<_> = env::args_os().collect();
    if arguments.len() != 3 {
        eprintln!("usage: ohmnivore_provenance_writer EXECUTABLE OUTPUT");
        std::process::exit(2);
    }
    let executable = Path::new(&arguments[1]);
    let output = Path::new(&arguments[2]);
    let digest = sha256_file(executable).unwrap_or_else(|error| {
        eprintln!("could not read executable: {error}");
        std::process::exit(1);
    });
    let provenance = format!(
        "circuitc-ohmnivore-provenance-v1\nname=ohmnivore\nversion=0.1.0\ncontract=ohmnivore-cli-csv/v1\nsource_revision={SOURCE_REVISION}\nexecutable_sha256={digest}\n"
    );
    fs::write(output, provenance).unwrap_or_else(|error| {
        eprintln!("could not write provenance: {error}");
        std::process::exit(1);
    });
}

fn sha256_file(path: &Path) -> io::Result<String> {
    let mut file = fs::File::open(path)?;
    let metadata = file.metadata()?;
    if !metadata.is_file() || metadata.len() == 0 || metadata.len() > MAX_EXECUTABLE_BYTES {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "executable is not a bounded regular file",
        ));
    }
    let mut digest = Sha256::new();
    let mut chunk = [0_u8; 8192];
    let mut total = 0_u64;
    loop {
        let count = file.read(&mut chunk)?;
        if count == 0 {
            break;
        }
        total = total.checked_add(count as u64).ok_or_else(|| {
            io::Error::new(io::ErrorKind::InvalidData, "executable size overflow")
        })?;
        if total > MAX_EXECUTABLE_BYTES {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "executable exceeded its provenance bound",
            ));
        }
        digest.update(&chunk[..count]);
    }
    if total != metadata.len() {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "executable changed while hashing",
        ));
    }
    Ok(digest
        .finalize()
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect())
}
