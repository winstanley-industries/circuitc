//! Bounded Ohmnivore process execution and strict CSV normalization.

use std::collections::{BTreeMap, BTreeSet};
use std::env;
use std::ffi::OsStr;
use std::fs::{self, OpenOptions};
use std::io::{self, Read, Write};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, ExitStatus, Stdio};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::mpsc::{self, Receiver, RecvTimeoutError};
use std::sync::{Arc, OnceLock};
use std::thread;
use std::time::{Duration, Instant};

use sha2::{Digest, Sha256};

#[cfg(all(unix, test))]
use std::os::unix::fs::PermissionsExt as _;
#[cfg(unix)]
use std::os::unix::fs::{DirBuilderExt as _, MetadataExt as _, OpenOptionsExt as _};
#[cfg(unix)]
use std::os::unix::process::CommandExt as _;

use super::lower::verify_spice_identity_binding;
use super::{
    AnalysisKind, AxisKind, ContractDiagnostic, ExecutionStatus, MAX_CONTRACT_BYTES,
    MAX_CONTRACT_ENTRIES, NormalizedDiagnostic, OHMNIVORE_BACKEND_CONTRACT, OHMNIVORE_BACKEND_NAME,
    OHMNIVORE_BACKEND_VERSION, OHMNIVORE_SOURCE_REVISION, RESULT_SCHEMA_NAME, ResultAxis,
    ResultSignal, ResultUnit, SignalKind, SimulationRequest, SimulationResult, SpiceIdentityMap,
    canonical_f64, parse_request, parse_spice_identity_map, sha256_hex,
};

const PROCESS_IDENTITY: &str = "CC-SIM-PROCESS-001";
const PROCESS_IO: &str = "CC-SIM-PROCESS-002";
const PROCESS_RESOURCE: &str = "CC-SIM-PROCESS-003";
const PROCESS_EXIT: &str = "CC-SIM-PROCESS-004";
const PROCESS_STDERR: &str = "CC-SIM-PROCESS-005";
const PROCESS_OUTPUT: &str = "CC-SIM-PROCESS-006";
const PROCESS_BINDING: &str = "CC-SIM-PROCESS-007";

const PROVENANCE_HEADER: &str = "circuitc-ohmnivore-provenance-v1";
const EXPECTED_VERSION_STDOUT: &[u8] = b"ohmnivore 0.1.0\n";
const DEFAULT_STDOUT_BYTES: usize = 16 * 1024 * 1024;
const DEFAULT_STDERR_BYTES: usize = 64 * 1024;
const DEFAULT_ADDRESS_SPACE_BYTES: u64 = 8 * 1024 * 1024 * 1024;
const DEFAULT_FILE_BYTES: u64 = 64 * 1024 * 1024;
const DEFAULT_OPEN_FILES: u64 = 32;
const DEFAULT_COMPILATION_WALL: Duration = Duration::from_secs(5 * 60);
const DEFAULT_IDENTITY_WALL: Duration = Duration::from_secs(30);
const MAX_PROVENANCE_BYTES: u64 = 1_024;
const MAX_EXECUTABLE_BYTES: u64 = 2 * 1024 * 1024 * 1024;
const READ_CHUNK_BYTES: usize = 8 * 1024;
const POLL_INTERVAL: Duration = Duration::from_millis(5);
const OHMNIVORE_EXECUTABLE_RUNFILE: &str = "_main/ohmnivore-cpu";
const OHMNIVORE_PROVENANCE_RUNFILE: &str = "_main/ohmnivore-provenance.txt";

static WORK_DIRECTORY_SEQUENCE: AtomicU64 = AtomicU64::new(0);

#[derive(Clone, Debug, Eq, PartialEq)]
struct OhmnivoreLimits {
    compilation_wall: Duration,
    handshake_wall: Duration,
    analysis_wall: Duration,
    stdout_bytes: usize,
    stderr_bytes: usize,
    address_space_bytes: u64,
    file_bytes: u64,
    open_files: u64,
}

impl Default for OhmnivoreLimits {
    fn default() -> Self {
        Self {
            compilation_wall: DEFAULT_COMPILATION_WALL,
            handshake_wall: Duration::from_secs(2),
            analysis_wall: Duration::from_secs(30),
            stdout_bytes: DEFAULT_STDOUT_BYTES,
            stderr_bytes: DEFAULT_STDERR_BYTES,
            address_space_bytes: DEFAULT_ADDRESS_SPACE_BYTES,
            file_bytes: DEFAULT_FILE_BYTES,
            open_files: DEFAULT_OPEN_FILES,
        }
    }
}

#[derive(Clone, Debug)]
pub struct OhmnivoreRunner {
    executable: PathBuf,
    provenance: PathBuf,
    work_root: PathBuf,
    limits: OhmnivoreLimits,
    compilation_deadline: Instant,
    verified_executable: Arc<OnceLock<Result<PathBuf, ProcessFailure>>>,
}

impl OhmnivoreRunner {
    /// Resolves the fixed Ohmnivore executable and provenance sidecar from
    /// Bazel runfiles and creates one aggregate compilation budget.
    pub fn from_bazel_runfiles(work_root: impl Into<PathBuf>) -> Result<Self, ContractDiagnostic> {
        let executable = resolve_bazel_runfile(OHMNIVORE_EXECUTABLE_RUNFILE)?;
        let provenance = resolve_bazel_runfile(OHMNIVORE_PROVENANCE_RUNFILE)?;
        Ok(Self::from_paths(executable, provenance, work_root.into()))
    }

    fn from_paths(
        executable: impl Into<PathBuf>,
        provenance: impl Into<PathBuf>,
        work_root: impl Into<PathBuf>,
    ) -> Self {
        let limits = OhmnivoreLimits::default();
        Self {
            executable: executable.into(),
            provenance: provenance.into(),
            work_root: work_root.into().join("circuitc-ohmnivore-work"),
            compilation_deadline: Instant::now()
                .checked_add(limits.compilation_wall)
                .unwrap_or_else(Instant::now),
            verified_executable: Arc::new(OnceLock::new()),
            limits,
        }
    }

    #[cfg(test)]
    fn with_limits(mut self, limits: OhmnivoreLimits) -> Self {
        self.compilation_deadline = Instant::now()
            .checked_add(limits.compilation_wall)
            .unwrap_or_else(Instant::now);
        self.limits = limits;
        self
    }

    pub fn execute(
        &self,
        netlist: &[u8],
        request_json: &[u8],
        map_json: &[u8],
    ) -> Result<SimulationResult, ContractDiagnostic> {
        validate_netlist_size(netlist.len(), MAX_CONTRACT_BYTES)?;
        validate_limits(&self.limits)?;
        let request_text = std::str::from_utf8(request_json).map_err(|_| ContractDiagnostic {
            code: "CC-SIM-CONTRACT-001",
            path: "request".to_owned(),
            message: "simulation request is not UTF-8".to_owned(),
        })?;
        let map_text = std::str::from_utf8(map_json).map_err(|_| ContractDiagnostic {
            code: "CC-SIM-CONTRACT-001",
            path: "map".to_owned(),
            message: "SPICE identity map is not UTF-8".to_owned(),
        })?;
        let request = parse_request(request_text)?;
        request.verify_netlist_bytes(netlist)?;
        let map = parse_spice_identity_map(map_text)?;
        let bound_request = map.verify_request_bytes(request_json)?;
        if request != bound_request {
            return Err(contract_binding_error(
                "request",
                "parsed request does not match the request bound by the identity map",
            ));
        }
        let failed = |failure| failed_result(&request, request_json, map_json, failure);

        let netlist_text = match std::str::from_utf8(netlist) {
            Ok(netlist) => netlist,
            Err(_) => return Ok(failed(output_failure())),
        };
        if verify_spice_identity_binding(netlist_text, &map, request.analysis.kind).is_err() {
            return Ok(failed(ProcessFailure::failed(
                PROCESS_BINDING,
                "SPICE input does not satisfy its authenticated identity map",
            )));
        }

        let plan = match AnalysisPlan::parse(netlist, request.analysis.kind) {
            Ok(plan) => plan,
            Err(failure) => return Ok(failed(failure)),
        };
        let expected_inventory = match ExpectedInventory::parse(netlist, &map) {
            Ok(inventory) => inventory,
            Err(failure) => return Ok(failed(failure)),
        };

        let executable = match self
            .verified_executable
            .get_or_init(|| self.resolve_verified_executable())
        {
            Ok(executable) => executable.clone(),
            Err(failure) => return Ok(failed(failure.clone())),
        };

        let handshake_directory = match ScopedWorkDirectory::create(&self.work_root) {
            Ok(directory) => directory,
            Err(_) => {
                return Ok(failed(ProcessFailure::failed(
                    PROCESS_IO,
                    "could not create a private Ohmnivore working directory",
                )));
            }
        };
        let handshake_limit = match self.remaining_limit(self.limits.handshake_wall) {
            Ok(limit) => limit,
            Err(failure) => {
                if handshake_directory.cleanup().is_err() {
                    return Ok(failed(ProcessFailure::failed(
                        PROCESS_IO,
                        "could not clean the private Ohmnivore handshake directory",
                    )));
                }
                return Ok(failed(failure));
            }
        };
        let version = run_process(
            &executable,
            handshake_directory.path(),
            [OsStr::new("--version")],
            handshake_limit,
            &self.limits,
        );
        if handshake_directory.cleanup().is_err() {
            return Ok(failed(ProcessFailure::failed(
                PROCESS_IO,
                "could not clean the private Ohmnivore handshake directory",
            )));
        }
        let version = match version {
            Ok(output) => output,
            Err(failure) => return Ok(failed(failure)),
        };
        if !version.status.success()
            || version.stdout != EXPECTED_VERSION_STDOUT
            || !version.stderr.is_empty()
        {
            return Ok(failed(ProcessFailure::unsupported(
                PROCESS_IDENTITY,
                "Ohmnivore did not satisfy the exact version handshake",
            )));
        }

        let work_directory = match ScopedWorkDirectory::create(&self.work_root) {
            Ok(directory) => directory,
            Err(_) => {
                return Ok(failed(ProcessFailure::failed(
                    PROCESS_IO,
                    "could not create a private Ohmnivore analysis directory",
                )));
            }
        };
        let netlist_name = "analysis.spice";
        let netlist_path = work_directory.path().join(netlist_name);
        let mut result = (|| {
            if write_private_file(&netlist_path, netlist).is_err() {
                return failed(ProcessFailure::failed(
                    PROCESS_IO,
                    "could not materialize the Ohmnivore netlist",
                ));
            }
            let analysis_limit = match self.remaining_limit(self.limits.analysis_wall) {
                Ok(limit) => limit,
                Err(failure) => return failed(failure),
            };
            let output = match run_process(
                &executable,
                work_directory.path(),
                [OsStr::new(netlist_name), OsStr::new("--cpu")],
                analysis_limit,
                &self.limits,
            ) {
                Ok(output) => output,
                Err(failure) => return failed(failure),
            };
            if !output.status.success() {
                return failed(ProcessFailure::failed(
                    PROCESS_EXIT,
                    "Ohmnivore exited unsuccessfully",
                ));
            }
            if !output.stderr.is_empty() {
                return failed(ProcessFailure::failed(
                    PROCESS_STDERR,
                    "Ohmnivore emitted unexpected standard error",
                ));
            }

            let result = match normalize_csv(
                &output.stdout,
                NormalizationContext {
                    request: &request,
                    map: &map,
                    request_json,
                    map_json,
                    plan: &plan,
                    inventory: &expected_inventory,
                    deadline: self.compilation_deadline,
                },
            ) {
                Ok(result) => result,
                Err(failure) => return failed(failure),
            };
            if result.verify_binding_bytes(request_json, map_json).is_err() {
                return failed(ProcessFailure::failed(
                    PROCESS_BINDING,
                    "normalized output does not cover every authenticated assertion signal and sample",
                ));
            }
            if result.to_canonical_json().is_err() || Instant::now() >= self.compilation_deadline {
                return failed(resource_failure());
            }
            result
        })();
        if work_directory.cleanup().is_err() {
            result = failed(ProcessFailure::failed(
                PROCESS_IO,
                "could not clean the private Ohmnivore working directory",
            ));
        }
        Ok(result)
    }

    fn remaining_limit(&self, per_process: Duration) -> Result<Duration, ProcessFailure> {
        let remaining = self
            .compilation_deadline
            .checked_duration_since(Instant::now())
            .ok_or_else(resource_failure)?;
        let limit = remaining.min(per_process);
        if limit.is_zero() {
            Err(resource_failure())
        } else {
            Ok(limit)
        }
    }

    fn resolve_verified_executable(&self) -> Result<PathBuf, ProcessFailure> {
        let executable = self.executable.canonicalize().map_err(|_| {
            ProcessFailure::failed(PROCESS_IO, "could not resolve the Ohmnivore executable")
        })?;
        let identity_limit = self.remaining_limit(DEFAULT_IDENTITY_WALL)?;
        let identity_deadline = Instant::now()
            .checked_add(identity_limit)
            .unwrap_or_else(Instant::now);
        verify_provenance(&executable, &self.provenance, identity_deadline)?;
        Ok(executable)
    }
}

fn validate_netlist_size(size: usize, limit: usize) -> Result<(), ContractDiagnostic> {
    if size > limit {
        Err(ContractDiagnostic {
            code: "CC-SIM-CONTRACT-006",
            path: "netlist".to_owned(),
            message: format!("SPICE netlist exceeds the {limit}-byte runner input limit"),
        })
    } else {
        Ok(())
    }
}

fn validate_limits(limits: &OhmnivoreLimits) -> Result<(), ContractDiagnostic> {
    if limits.compilation_wall.is_zero()
        || limits.handshake_wall.is_zero()
        || limits.analysis_wall.is_zero()
        || limits.stdout_bytes == 0
        || limits.stdout_bytes > MAX_CONTRACT_BYTES
        || limits.stderr_bytes == 0
        || limits.stderr_bytes > MAX_CONTRACT_BYTES
        || limits.address_space_bytes == 0
        || limits.file_bytes == 0
        || limits.open_files < 3
    {
        return Err(ContractDiagnostic {
            code: "CC-SIM-CONTRACT-002",
            path: "runner.limits".to_owned(),
            message: "Ohmnivore process limits must be positive and contract-bounded".to_owned(),
        });
    }
    Ok(())
}

#[derive(Clone, Debug)]
struct ProcessFailure {
    status: ExecutionStatus,
    code: &'static str,
    message: &'static str,
}

impl ProcessFailure {
    const fn failed(code: &'static str, message: &'static str) -> Self {
        Self {
            status: ExecutionStatus::Failed,
            code,
            message,
        }
    }

    const fn unsupported(code: &'static str, message: &'static str) -> Self {
        Self {
            status: ExecutionStatus::Unsupported,
            code,
            message,
        }
    }
}

fn failed_result(
    request: &SimulationRequest,
    request_json: &[u8],
    map_json: &[u8],
    failure: ProcessFailure,
) -> SimulationResult {
    SimulationResult {
        schema_name: RESULT_SCHEMA_NAME.to_owned(),
        schema_version: super::CONTRACT_SCHEMA_VERSION,
        design: request.design.clone(),
        analysis_path: request.analysis.path.clone(),
        analysis_kind: request.analysis.kind,
        status: failure.status,
        request_sha256: sha256_hex(request_json),
        map_sha256: sha256_hex(map_json),
        axis: ResultAxis {
            kind: axis_kind(request.analysis.kind),
            samples: Vec::new(),
        },
        signals: Vec::new(),
        diagnostics: vec![NormalizedDiagnostic {
            code: failure.code.to_owned(),
            message: failure.message.to_owned(),
        }],
    }
}

fn axis_kind(kind: AnalysisKind) -> AxisKind {
    match kind {
        AnalysisKind::DcOperatingPoint => AxisKind::Scalar,
        AnalysisKind::AcLinearSweep => AxisKind::FrequencyHertz,
        AnalysisKind::Transient => AxisKind::TimeSeconds,
    }
}

fn contract_binding_error(path: &str, message: &str) -> ContractDiagnostic {
    ContractDiagnostic {
        code: "CC-SIM-CONTRACT-004",
        path: path.to_owned(),
        message: message.to_owned(),
    }
}

fn resolve_bazel_runfile(logical_path: &str) -> Result<PathBuf, ContractDiagnostic> {
    let environment_directory = env::var_os("RUNFILES_DIR").or_else(|| env::var_os("TEST_SRCDIR"));
    let executable_directory = env::current_exe().ok().and_then(|executable| {
        let mut runfiles_name = executable.file_name()?.to_os_string();
        runfiles_name.push(".runfiles");
        Some(executable.parent()?.join(runfiles_name))
    });
    for directory in environment_directory
        .map(PathBuf::from)
        .into_iter()
        .chain(executable_directory)
    {
        let path = directory.join(logical_path);
        if path.exists() {
            return Ok(path);
        }
    }
    if let Some(manifest) = env::var_os("RUNFILES_MANIFEST_FILE") {
        let deadline = Instant::now()
            .checked_add(Duration::from_secs(2))
            .unwrap_or_else(Instant::now);
        let manifest =
            read_bounded_regular_file(Path::new(&manifest), MAX_CONTRACT_BYTES as u64, deadline)
                .map_err(|_| runfiles_diagnostic())?;
        let manifest = std::str::from_utf8(&manifest).map_err(|_| runfiles_diagnostic())?;
        for line in manifest.lines() {
            if let Some((logical, physical)) = line.split_once(' ')
                && logical == logical_path
            {
                return Ok(PathBuf::from(physical));
            }
        }
    }
    Err(runfiles_diagnostic())
}

fn runfiles_diagnostic() -> ContractDiagnostic {
    ContractDiagnostic {
        code: "CC-SIM-CONTRACT-004",
        path: "runner.runfiles".to_owned(),
        message: "could not resolve the Bazel-owned Ohmnivore runfiles".to_owned(),
    }
}

fn verify_provenance(
    executable: &Path,
    provenance: &Path,
    deadline: Instant,
) -> Result<(), ProcessFailure> {
    let provenance = read_bounded_regular_file(provenance, MAX_PROVENANCE_BYTES, deadline)
        .map_err(|_| {
            ProcessFailure::unsupported(
                PROCESS_IDENTITY,
                "could not read the Bazel-owned Ohmnivore provenance",
            )
        })?;
    let provenance = std::str::from_utf8(&provenance).map_err(|_| {
        ProcessFailure::unsupported(
            PROCESS_IDENTITY,
            "Bazel-owned Ohmnivore provenance is not UTF-8",
        )
    })?;
    let executable_sha256 = sha256_bounded_regular_file(executable, MAX_EXECUTABLE_BYTES, deadline)
        .map_err(|_| {
            ProcessFailure::unsupported(PROCESS_IDENTITY, "could not hash the Ohmnivore executable")
        })?;
    let expected = format!(
        "{PROVENANCE_HEADER}\nname={OHMNIVORE_BACKEND_NAME}\nversion={OHMNIVORE_BACKEND_VERSION}\ncontract={OHMNIVORE_BACKEND_CONTRACT}\nsource_revision={OHMNIVORE_SOURCE_REVISION}\nexecutable_sha256={}\n",
        executable_sha256
    );
    if provenance != expected {
        return Err(ProcessFailure::unsupported(
            PROCESS_IDENTITY,
            "Ohmnivore executable provenance does not match the pinned backend contract",
        ));
    }
    Ok(())
}

fn read_bounded_regular_file(path: &Path, limit: u64, deadline: Instant) -> io::Result<Vec<u8>> {
    let file = fs::File::open(path)?;
    let metadata = file.metadata()?;
    if !metadata.is_file() || metadata.len() > limit {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "runfile is not a bounded regular file",
        ));
    }
    let mut bytes = Vec::new();
    bytes
        .try_reserve_exact(metadata.len() as usize)
        .map_err(|_| {
            io::Error::new(
                io::ErrorKind::OutOfMemory,
                "could not reserve runfile bytes",
            )
        })?;
    file.take(limit + 1).read_to_end(&mut bytes)?;
    if bytes.len() as u64 != metadata.len() || Instant::now() >= deadline {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "runfile changed or exceeded its identity deadline",
        ));
    }
    Ok(bytes)
}

fn sha256_bounded_regular_file(path: &Path, limit: u64, deadline: Instant) -> io::Result<String> {
    let mut file = fs::File::open(path)?;
    let metadata = file.metadata()?;
    if !metadata.is_file() || metadata.len() == 0 || metadata.len() > limit {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "executable is not a bounded regular file",
        ));
    }
    let mut digest = Sha256::new();
    let mut chunk = [0_u8; READ_CHUNK_BYTES];
    let mut total = 0_u64;
    loop {
        if Instant::now() >= deadline {
            return Err(io::Error::new(
                io::ErrorKind::TimedOut,
                "executable identity deadline expired",
            ));
        }
        let count = file.read(&mut chunk)?;
        if count == 0 {
            break;
        }
        total = total.checked_add(count as u64).ok_or_else(|| {
            io::Error::new(io::ErrorKind::InvalidData, "executable size overflow")
        })?;
        if total > limit {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "executable exceeded identity bound",
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

struct ScopedWorkDirectory(Option<PathBuf>);

impl ScopedWorkDirectory {
    fn create(root: &Path) -> io::Result<Self> {
        create_private_directory(root)?;
        let root = root.canonicalize()?;
        verify_private_directory(&root)?;
        for _ in 0..1_024 {
            let sequence = WORK_DIRECTORY_SEQUENCE.fetch_add(1, Ordering::Relaxed);
            let path = root.join(format!(
                "circuitc-ohmnivore-{}-{sequence}",
                std::process::id()
            ));
            match create_new_private_directory(&path) {
                Ok(()) => {
                    if let Err(error) = create_new_private_directory(&path.join("home"))
                        .and_then(|()| create_new_private_directory(&path.join("tmp")))
                    {
                        let _ = fs::remove_dir_all(&path);
                        return Err(error);
                    }
                    return Ok(Self(Some(path)));
                }
                Err(error) if error.kind() == io::ErrorKind::AlreadyExists => continue,
                Err(error) => return Err(error),
            }
        }
        Err(io::Error::new(
            io::ErrorKind::AlreadyExists,
            "could not allocate a unique process directory",
        ))
    }

    fn path(&self) -> &Path {
        self.0.as_deref().expect("working directory is live")
    }

    fn cleanup(mut self) -> io::Result<()> {
        let path = self.0.take().expect("working directory is live");
        fs::remove_dir_all(path)
    }
}

#[cfg(unix)]
fn create_private_directory(path: &Path) -> io::Result<()> {
    let mut builder = fs::DirBuilder::new();
    builder.recursive(true).mode(0o700);
    builder.create(path)
}

#[cfg(not(unix))]
fn create_private_directory(path: &Path) -> io::Result<()> {
    fs::create_dir_all(path)
}

#[cfg(unix)]
fn create_new_private_directory(path: &Path) -> io::Result<()> {
    let mut builder = fs::DirBuilder::new();
    builder.mode(0o700).create(path)
}

#[cfg(not(unix))]
fn create_new_private_directory(path: &Path) -> io::Result<()> {
    fs::create_dir(path)
}

#[cfg(unix)]
fn verify_private_directory(path: &Path) -> io::Result<()> {
    let metadata = fs::metadata(path)?;
    if !metadata.is_dir()
        || metadata.uid() != unsafe { libc::geteuid() }
        || metadata.mode() & 0o077 != 0
    {
        return Err(io::Error::new(
            io::ErrorKind::PermissionDenied,
            "Ohmnivore work root is not private and caller-owned",
        ));
    }
    Ok(())
}

#[cfg(not(unix))]
fn verify_private_directory(path: &Path) -> io::Result<()> {
    if fs::metadata(path)?.is_dir() {
        Ok(())
    } else {
        Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "Ohmnivore work root is not a directory",
        ))
    }
}

fn write_private_file(path: &Path, bytes: &[u8]) -> io::Result<()> {
    let mut options = OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    options.mode(0o600).custom_flags(libc::O_NOFOLLOW);
    let mut file = options.open(path)?;
    file.write_all(bytes)?;
    file.flush()
}

impl Drop for ScopedWorkDirectory {
    fn drop(&mut self) {
        if let Some(path) = self.0.take() {
            let _ = fs::remove_dir_all(path);
        }
    }
}

#[derive(Debug)]
struct CapturedProcess {
    status: ExitStatus,
    stdout: Vec<u8>,
    stderr: Vec<u8>,
}

fn run_process<'a>(
    executable: &Path,
    current_directory: &Path,
    arguments: impl IntoIterator<Item = &'a OsStr>,
    wall_limit: Duration,
    limits: &OhmnivoreLimits,
) -> Result<CapturedProcess, ProcessFailure> {
    let mut command = Command::new(executable);
    command
        .args(arguments)
        .current_dir(current_directory)
        .env_clear()
        .env("LANG", "C")
        .env("LC_ALL", "C")
        .env("TZ", "UTC")
        .env("RUST_LOG", "off")
        .env("RUST_BACKTRACE", "0")
        .env("NO_COLOR", "1")
        .env("RAYON_NUM_THREADS", "1")
        .env("OMP_NUM_THREADS", "1")
        .env("OPENBLAS_NUM_THREADS", "1")
        .env("MKL_NUM_THREADS", "1")
        .env("VECLIB_MAXIMUM_THREADS", "1")
        .env("BLIS_NUM_THREADS", "1")
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    command.env("HOME", current_directory.join("home"));
    command.env("TMPDIR", current_directory.join("tmp"));
    configure_unix_process(&mut command, wall_limit, limits);

    let mut child = command.spawn().map_err(|_| {
        ProcessFailure::failed(PROCESS_IO, "could not launch the Ohmnivore process")
    })?;
    let stdout = child.stdout.take().ok_or_else(|| {
        ProcessFailure::failed(PROCESS_IO, "could not capture Ohmnivore standard output")
    })?;
    let stderr = child.stderr.take().ok_or_else(|| {
        ProcessFailure::failed(PROCESS_IO, "could not capture Ohmnivore standard error")
    })?;
    let overflow = Arc::new(AtomicBool::new(false));
    let stdout_reader = spawn_reader(stdout, limits.stdout_bytes, Arc::clone(&overflow));
    let stderr_reader = spawn_reader(stderr, limits.stderr_bytes, Arc::clone(&overflow));
    let deadline = Instant::now()
        .checked_add(wall_limit)
        .unwrap_or_else(Instant::now);

    let status = loop {
        if overflow.load(Ordering::SeqCst) {
            terminate(&mut child);
            let _ = receive_reader(&stdout_reader, &mut child);
            let _ = receive_reader(&stderr_reader, &mut child);
            return Err(ProcessFailure::failed(
                PROCESS_RESOURCE,
                "Ohmnivore exceeded a bounded process output limit",
            ));
        }
        match child.try_wait() {
            Ok(Some(status)) => break status,
            Ok(None) if Instant::now() < deadline => thread::sleep(POLL_INTERVAL),
            Ok(None) => {
                terminate(&mut child);
                let _ = receive_reader(&stdout_reader, &mut child);
                let _ = receive_reader(&stderr_reader, &mut child);
                return Err(ProcessFailure::failed(
                    PROCESS_RESOURCE,
                    "Ohmnivore exceeded its wall-clock execution limit",
                ));
            }
            Err(_) => {
                terminate(&mut child);
                let _ = receive_reader(&stdout_reader, &mut child);
                let _ = receive_reader(&stderr_reader, &mut child);
                return Err(ProcessFailure::failed(
                    PROCESS_IO,
                    "could not observe the Ohmnivore process status",
                ));
            }
        }
    };
    let stdout = receive_reader(&stdout_reader, &mut child)?;
    let stderr = receive_reader(&stderr_reader, &mut child)?;
    if overflow.load(Ordering::SeqCst) {
        return Err(ProcessFailure::failed(
            PROCESS_RESOURCE,
            "Ohmnivore exceeded a bounded process output limit",
        ));
    }
    Ok(CapturedProcess {
        status,
        stdout,
        stderr,
    })
}

fn spawn_reader<R: Read + Send + 'static>(
    mut reader: R,
    limit: usize,
    overflow: Arc<AtomicBool>,
) -> Receiver<io::Result<Vec<u8>>> {
    let (sender, receiver) = mpsc::sync_channel(1);
    thread::spawn(move || {
        let mut bytes = Vec::new();
        let mut chunk = [0_u8; READ_CHUNK_BYTES];
        let result = loop {
            let count = reader.read(&mut chunk)?;
            if count == 0 {
                break Ok(bytes);
            }
            if bytes
                .len()
                .checked_add(count)
                .is_none_or(|size| size > limit)
            {
                overflow.store(true, Ordering::SeqCst);
                break Ok(bytes);
            }
            bytes.extend_from_slice(&chunk[..count]);
        };
        let _ = sender.send(result);
        Ok::<(), io::Error>(())
    });
    receiver
}

fn receive_reader(
    reader: &Receiver<io::Result<Vec<u8>>>,
    child: &mut Child,
) -> Result<Vec<u8>, ProcessFailure> {
    match reader.recv_timeout(Duration::from_millis(250)) {
        Ok(Ok(bytes)) => Ok(bytes),
        Ok(Err(_)) | Err(RecvTimeoutError::Disconnected) => Err(ProcessFailure::failed(
            PROCESS_IO,
            "could not read bounded Ohmnivore process output",
        )),
        Err(RecvTimeoutError::Timeout) => {
            terminate(child);
            match reader.recv_timeout(Duration::from_secs(1)) {
                Ok(Ok(bytes)) => Ok(bytes),
                _ => Err(ProcessFailure::failed(
                    PROCESS_RESOURCE,
                    "Ohmnivore output pipes did not close within the drain limit",
                )),
            }
        }
    }
}

fn terminate(child: &mut Child) {
    #[cfg(unix)]
    // SAFETY: the child PID is returned by `spawn`; a negative PID targets only
    // the process group created for this child. Failure is followed by the
    // portable leader kill, and the leader is always reaped.
    unsafe {
        let _ = libc::kill(-(child.id() as libc::pid_t), libc::SIGKILL);
    }
    let _ = child.kill();
    let _ = child.wait();
}

#[cfg(unix)]
fn configure_unix_process(command: &mut Command, wall_limit: Duration, limits: &OhmnivoreLimits) {
    let address_space = limits.address_space_bytes;
    let file_bytes = limits.file_bytes;
    let open_files = limits.open_files;
    let cpu_seconds = wall_limit.as_secs().saturating_add(1).max(1);
    command.process_group(0);
    // SAFETY: `pre_exec` is intentionally limited to async-signal-safe libc
    // calls. All captured values are plain integers and no allocation occurs in
    // the child between fork and exec.
    unsafe {
        command.pre_exec(move || {
            set_memory_limit(address_space)?;
            set_limit(libc::RLIMIT_CPU, cpu_seconds)?;
            set_limit(libc::RLIMIT_FSIZE, file_bytes)?;
            set_limit(libc::RLIMIT_NOFILE, open_files)?;
            set_limit(libc::RLIMIT_CORE, 0)?;
            Ok(())
        });
    }
}

#[cfg(all(unix, target_os = "linux"))]
fn set_memory_limit(value: u64) -> io::Result<()> {
    set_limit(libc::RLIMIT_AS, value)
}

#[cfg(all(unix, not(target_os = "linux")))]
fn set_memory_limit(_value: u64) -> io::Result<()> {
    // Darwin accepts the RLIMIT_AS/RLIMIT_DATA constants but returns EINVAL for
    // finite limits. Wall, CPU, output, file, and descriptor bounds remain
    // enforced there; the address-space ceiling is a Linux-only hard limit.
    Ok(())
}

#[cfg(not(unix))]
fn configure_unix_process(
    _command: &mut Command,
    _wall_limit: Duration,
    _limits: &OhmnivoreLimits,
) {
}

#[cfg(all(unix, target_os = "linux"))]
type RlimitResource = libc::__rlimit_resource_t;

#[cfg(all(unix, not(target_os = "linux")))]
type RlimitResource = libc::c_int;

#[cfg(unix)]
fn set_limit(resource: RlimitResource, value: u64) -> io::Result<()> {
    let value: libc::rlim_t = value;
    let limit = libc::rlimit {
        rlim_cur: value,
        rlim_max: value,
    };
    // SAFETY: `limit` is a valid initialized `rlimit` value and `resource` is
    // one of the platform constants supplied above.
    if unsafe { libc::setrlimit(resource, &limit) } == 0 {
        Ok(())
    } else {
        Err(io::Error::last_os_error())
    }
}

#[derive(Clone, Debug)]
enum AnalysisPlan {
    Dc,
    Ac { samples: Vec<f64> },
    Transient { start: f64, stop: f64 },
}

impl AnalysisPlan {
    fn parse(netlist: &[u8], kind: AnalysisKind) -> Result<Self, ProcessFailure> {
        let netlist = std::str::from_utf8(netlist).map_err(|_| output_failure())?;
        let directives: Vec<_> = netlist
            .lines()
            .filter(|line| line.starts_with('.') && !line.eq_ignore_ascii_case(".END"))
            .collect();
        if directives.len() != 1 {
            return Err(output_failure());
        }
        let fields: Vec<_> = directives[0].split_ascii_whitespace().collect();
        match kind {
            AnalysisKind::DcOperatingPoint
                if fields.len() == 1 && fields[0].eq_ignore_ascii_case(".OP") =>
            {
                Ok(Self::Dc)
            }
            AnalysisKind::AcLinearSweep
                if fields.len() == 5
                    && fields[0].eq_ignore_ascii_case(".AC")
                    && fields[1].eq_ignore_ascii_case("LIN") =>
            {
                let points = fields[2].parse::<u32>().map_err(|_| output_failure())?;
                let start = parse_finite(fields[3])?;
                let stop = parse_finite(fields[4])?;
                if points < 2 || start <= 0.0 || start >= stop {
                    return Err(output_failure());
                }
                let point_count = usize::try_from(points).map_err(|_| resource_failure())?;
                if point_count > MAX_CONTRACT_ENTRIES {
                    return Err(resource_failure());
                }
                let step = (stop - start) / f64::from(points - 1);
                let mut samples = Vec::new();
                samples
                    .try_reserve_exact(point_count)
                    .map_err(|_| resource_failure())?;
                for index in 0..points {
                    let sample = start + step * f64::from(index);
                    if !sample.is_finite()
                        || samples.last().is_some_and(|previous| sample <= *previous)
                    {
                        return Err(output_failure());
                    }
                    samples.push(sample);
                }
                Ok(Self::Ac { samples })
            }
            AnalysisKind::Transient
                if matches!(fields.len(), 4 | 5)
                    && fields[0].eq_ignore_ascii_case(".TRAN")
                    && (fields.len() == 4 || fields[4].eq_ignore_ascii_case("UIC")) =>
            {
                let step = parse_finite(fields[1])?;
                let stop = parse_finite(fields[2])?;
                let start = parse_finite(fields[3])?;
                if step <= 0.0 || start < 0.0 || start >= stop {
                    return Err(output_failure());
                }
                Ok(Self::Transient { start, stop })
            }
            _ => Err(output_failure()),
        }
    }
}

#[derive(Clone, Debug)]
struct ExpectedInventory {
    nets: BTreeMap<String, String>,
    branches: BTreeMap<String, String>,
}

impl ExpectedInventory {
    fn parse(netlist: &[u8], map: &SpiceIdentityMap) -> Result<Self, ProcessFailure> {
        let netlist = std::str::from_utf8(netlist).map_err(|_| output_failure())?;
        let net_map: BTreeMap<_, _> = map
            .nets
            .iter()
            .map(|net| {
                (
                    net.backend.as_str(),
                    (net.canonical.as_str(), net.is_ground),
                )
            })
            .collect();
        let device_map: BTreeMap<_, _> = map
            .devices
            .iter()
            .map(|device| (device.backend.as_str(), device.semantic_path.as_str()))
            .collect();
        let mut nets = BTreeMap::new();
        let mut branches = BTreeMap::new();
        for line in netlist.lines() {
            if line.is_empty() || line.starts_with('*') || line.starts_with('.') {
                continue;
            }
            let mut fields = line.split_ascii_whitespace();
            let (Some(backend), Some(first_node), Some(second_node)) =
                (fields.next(), fields.next(), fields.next())
            else {
                return Err(output_failure());
            };
            for node in [first_node, second_node] {
                let Some((canonical, is_ground)) = net_map.get(node) else {
                    return Err(output_failure());
                };
                if !is_ground {
                    nets.insert(node.to_owned(), (*canonical).to_owned());
                }
            }
            let Some(canonical) = device_map.get(backend) else {
                return Err(output_failure());
            };
            if backend
                .as_bytes()
                .first()
                .is_some_and(|byte| byte.eq_ignore_ascii_case(&b'V'))
            {
                branches.insert(backend.to_owned(), (*canonical).to_owned());
            }
        }
        if nets.is_empty() {
            return Err(output_failure());
        }
        Ok(Self { nets, branches })
    }
}

struct NormalizationContext<'a> {
    request: &'a SimulationRequest,
    map: &'a SpiceIdentityMap,
    request_json: &'a [u8],
    map_json: &'a [u8],
    plan: &'a AnalysisPlan,
    inventory: &'a ExpectedInventory,
    deadline: Instant,
}

fn normalize_csv(
    stdout: &[u8],
    context: NormalizationContext<'_>,
) -> Result<SimulationResult, ProcessFailure> {
    let NormalizationContext {
        request,
        map,
        request_json,
        map_json,
        plan,
        inventory,
        deadline,
    } = context;
    let csv = parse_csv(stdout, deadline)?;
    let (axis, signals) = match (request.analysis.kind, plan) {
        (AnalysisKind::DcOperatingPoint, AnalysisPlan::Dc) => {
            normalize_dc(&csv, inventory, deadline)?
        }
        (AnalysisKind::AcLinearSweep, AnalysisPlan::Ac { samples }) => {
            normalize_ac(&csv, inventory, samples, deadline)?
        }
        (AnalysisKind::Transient, AnalysisPlan::Transient { start, stop }) => {
            normalize_transient(&csv, inventory, *start, *stop, request, deadline)?
        }
        _ => return Err(output_failure()),
    };
    let result = SimulationResult {
        schema_name: RESULT_SCHEMA_NAME.to_owned(),
        schema_version: super::CONTRACT_SCHEMA_VERSION,
        design: request.design.clone(),
        analysis_path: request.analysis.path.clone(),
        analysis_kind: request.analysis.kind,
        status: ExecutionStatus::Completed,
        request_sha256: sha256_hex(request_json),
        map_sha256: sha256_hex(map_json),
        axis,
        signals,
        diagnostics: Vec::new(),
    };
    if result.validate().is_err() {
        return Err(output_failure());
    }
    for signal in &result.signals {
        let mapped = match signal.kind {
            SignalKind::NetVoltage
            | SignalKind::NetVoltageMagnitude
            | SignalKind::NetVoltagePhaseDegrees => map
                .nets
                .iter()
                .any(|identity| identity.canonical == signal.canonical_identity),
            SignalKind::BranchCurrent
            | SignalKind::BranchCurrentMagnitude
            | SignalKind::BranchCurrentPhaseDegrees => map
                .devices
                .iter()
                .any(|identity| identity.semantic_path == signal.canonical_identity),
        };
        if !mapped {
            return Err(output_failure());
        }
    }
    Ok(result)
}

fn parse_csv(stdout: &[u8], deadline: Instant) -> Result<Vec<Vec<&str>>, ProcessFailure> {
    if stdout.is_empty()
        || stdout.len() > DEFAULT_STDOUT_BYTES
        || !stdout.ends_with(b"\n")
        || stdout.contains(&b'\r')
        || stdout.contains(&0)
        || stdout.contains(&b'"')
    {
        return Err(output_failure());
    }
    let mut row_count = 0_usize;
    let mut cell_count = 0_usize;
    let mut expected_width = None;
    for line in stdout[..stdout.len() - 1].split(|byte| *byte == b'\n') {
        check_deadline(deadline)?;
        if line.is_empty() {
            return Err(output_failure());
        }
        row_count = row_count.checked_add(1).ok_or_else(resource_failure)?;
        if row_count > MAX_CONTRACT_ENTRIES + 1 {
            return Err(resource_failure());
        }
        let width = line
            .iter()
            .filter(|byte| **byte == b',')
            .count()
            .checked_add(1)
            .ok_or_else(resource_failure)?;
        if width < 2 || expected_width.is_some_and(|expected| expected != width) {
            return Err(output_failure());
        }
        if width > MAX_CONTRACT_ENTRIES.saturating_mul(2).saturating_add(1) {
            return Err(resource_failure());
        }
        expected_width = Some(width);
        cell_count = cell_count.checked_add(width).ok_or_else(resource_failure)?;
        if cell_count
            .checked_mul(32)
            .is_none_or(|estimated| estimated > MAX_CONTRACT_BYTES)
        {
            return Err(resource_failure());
        }
    }
    if row_count < 2 {
        return Err(output_failure());
    }
    let text = std::str::from_utf8(stdout).map_err(|_| output_failure())?;
    let mut rows = Vec::new();
    rows.try_reserve_exact(row_count)
        .map_err(|_| resource_failure())?;
    for line in text.split_terminator('\n') {
        check_deadline(deadline)?;
        let width = expected_width.ok_or_else(output_failure)?;
        let mut fields = Vec::new();
        fields
            .try_reserve_exact(width)
            .map_err(|_| resource_failure())?;
        fields.extend(line.split(','));
        if fields
            .iter()
            .any(|field| field.is_empty() || field.trim() != *field)
        {
            return Err(output_failure());
        }
        rows.push(fields);
    }
    Ok(rows)
}

fn normalize_dc(
    rows: &[Vec<&str>],
    inventory: &ExpectedInventory,
    deadline: Instant,
) -> Result<(ResultAxis, Vec<ResultSignal>), ProcessFailure> {
    if rows[0].as_slice() != ["Variable", "Value"] {
        return Err(output_failure());
    }
    let mut signals = BTreeMap::new();
    for row in &rows[1..] {
        check_deadline(deadline)?;
        let (kind, canonical, unit) = map_backend_signal(row[0], inventory, SignalShape::Scalar)?;
        if signals
            .insert((kind, canonical, unit), vec![canonical_number(row[1])?])
            .is_some()
        {
            return Err(output_failure());
        }
    }
    let signals = into_signals(signals);
    verify_inventory(&signals, inventory, SignalShape::Scalar)?;
    Ok((
        ResultAxis {
            kind: AxisKind::Scalar,
            samples: vec![canonical_f64(0.0).map_err(|_| output_failure())?],
        },
        signals,
    ))
}

fn normalize_ac(
    rows: &[Vec<&str>],
    inventory: &ExpectedInventory,
    expected_samples: &[f64],
    deadline: Instant,
) -> Result<(ResultAxis, Vec<ResultSignal>), ProcessFailure> {
    if rows[0][0] != "Frequency" || !(rows[0].len() - 1).is_multiple_of(2) {
        return Err(output_failure());
    }
    if rows.len() - 1 != expected_samples.len() {
        return Err(output_failure());
    }
    let mut columns = Vec::new();
    let mut signal_keys = BTreeSet::new();
    let mut signals = Vec::new();
    for pair in rows[0][1..].chunks_exact(2) {
        check_deadline(deadline)?;
        let Some(base) = pair[0].strip_suffix("_mag") else {
            return Err(output_failure());
        };
        if pair[1] != format!("{base}_phase_deg") {
            return Err(output_failure());
        }
        let (magnitude_kind, canonical, magnitude_unit) =
            map_backend_signal(base, inventory, SignalShape::Magnitude)?;
        let phase_kind = match magnitude_kind {
            SignalKind::NetVoltageMagnitude => SignalKind::NetVoltagePhaseDegrees,
            SignalKind::BranchCurrentMagnitude => SignalKind::BranchCurrentPhaseDegrees,
            _ => return Err(output_failure()),
        };
        if !signal_keys.insert((magnitude_kind, canonical.clone(), magnitude_unit))
            || !signal_keys.insert((phase_kind, canonical.clone(), ResultUnit::Degree))
        {
            return Err(output_failure());
        }
        let magnitude_index = signals.len();
        signals.push(ResultSignal {
            kind: magnitude_kind,
            canonical_identity: canonical.clone(),
            unit: magnitude_unit,
            values: Vec::new(),
        });
        let phase_index = signals.len();
        signals.push(ResultSignal {
            kind: phase_kind,
            canonical_identity: canonical,
            unit: ResultUnit::Degree,
            values: Vec::new(),
        });
        columns.push((magnitude_index, phase_index));
    }
    let mut axis = Vec::with_capacity(expected_samples.len());
    for (row_index, row) in rows[1..].iter().enumerate() {
        check_deadline(deadline)?;
        let sample = parse_display_finite(row[0])?;
        if sample.to_bits() != expected_samples[row_index].to_bits() {
            return Err(output_failure());
        }
        axis.push(canonical_f64(sample).map_err(|_| output_failure())?);
        for (column_index, (magnitude_index, phase_index)) in columns.iter().enumerate() {
            let offset = 1 + column_index * 2;
            signals[*magnitude_index]
                .values
                .push(canonical_number(row[offset])?);
            signals[*phase_index]
                .values
                .push(canonical_number(row[offset + 1])?);
        }
    }
    signals.sort_by(|left, right| {
        (left.kind, left.canonical_identity.as_str(), left.unit).cmp(&(
            right.kind,
            right.canonical_identity.as_str(),
            right.unit,
        ))
    });
    verify_inventory(&signals, inventory, SignalShape::Magnitude)?;
    Ok((
        ResultAxis {
            kind: AxisKind::FrequencyHertz,
            samples: axis,
        },
        signals,
    ))
}

fn normalize_transient(
    rows: &[Vec<&str>],
    inventory: &ExpectedInventory,
    start: f64,
    stop: f64,
    request: &SimulationRequest,
    deadline: Instant,
) -> Result<(ResultAxis, Vec<ResultSignal>), ProcessFailure> {
    if rows[0][0] != "time" {
        return Err(output_failure());
    }
    let mut columns = Vec::new();
    let mut signal_keys = BTreeSet::new();
    let mut signals = Vec::new();
    for header in &rows[0][1..] {
        check_deadline(deadline)?;
        let (kind, canonical, unit) = map_backend_signal(header, inventory, SignalShape::Scalar)?;
        if !signal_keys.insert((kind, canonical.clone(), unit)) {
            return Err(output_failure());
        }
        let signal_index = signals.len();
        signals.push(ResultSignal {
            kind,
            canonical_identity: canonical,
            unit,
            values: Vec::new(),
        });
        columns.push(signal_index);
    }
    let mut axis = Vec::with_capacity(rows.len() - 1);
    let mut previous = None;
    for row in &rows[1..] {
        check_deadline(deadline)?;
        let sample = parse_display_finite(row[0])?;
        if sample < start || sample > stop || previous.is_some_and(|previous| sample <= previous) {
            return Err(output_failure());
        }
        previous = Some(sample);
        axis.push(canonical_f64(sample).map_err(|_| output_failure())?);
        for (column_index, signal_index) in columns.iter().enumerate() {
            signals[*signal_index]
                .values
                .push(canonical_number(row[1 + column_index])?);
        }
    }
    if previous.is_none_or(|last| last.to_bits() != stop.to_bits()) {
        return Err(ProcessFailure::failed(
            PROCESS_BINDING,
            "Ohmnivore transient output does not contain the exact declared stop",
        ));
    }
    let samples: BTreeSet<_> = axis.iter().map(String::as_str).collect();
    if request
        .assertions
        .iter()
        .any(|assertion| !samples.contains(assertion.sample.value.as_str()))
    {
        return Err(ProcessFailure::failed(
            PROCESS_BINDING,
            "Ohmnivore transient output does not contain every authenticated sample",
        ));
    }
    signals.sort_by(|left, right| {
        (left.kind, left.canonical_identity.as_str(), left.unit).cmp(&(
            right.kind,
            right.canonical_identity.as_str(),
            right.unit,
        ))
    });
    verify_inventory(&signals, inventory, SignalShape::Scalar)?;
    Ok((
        ResultAxis {
            kind: AxisKind::TimeSeconds,
            samples: axis,
        },
        signals,
    ))
}

#[derive(Clone, Copy)]
enum SignalShape {
    Scalar,
    Magnitude,
}

type SignalValues = BTreeMap<(SignalKind, String, ResultUnit), Vec<String>>;

fn map_backend_signal(
    token: &str,
    inventory: &ExpectedInventory,
    shape: SignalShape,
) -> Result<(SignalKind, String, ResultUnit), ProcessFailure> {
    if let Some(backend) = token
        .strip_prefix("V(")
        .and_then(|value| value.strip_suffix(')'))
    {
        let canonical = inventory.nets.get(backend).ok_or_else(output_failure)?;
        let kind = match shape {
            SignalShape::Scalar => SignalKind::NetVoltage,
            SignalShape::Magnitude => SignalKind::NetVoltageMagnitude,
        };
        return Ok((kind, canonical.clone(), ResultUnit::Volt));
    }
    if let Some(backend) = token
        .strip_prefix("I(")
        .and_then(|value| value.strip_suffix(')'))
    {
        let canonical = inventory.branches.get(backend).ok_or_else(output_failure)?;
        let kind = match shape {
            SignalShape::Scalar => SignalKind::BranchCurrent,
            SignalShape::Magnitude => SignalKind::BranchCurrentMagnitude,
        };
        return Ok((kind, canonical.clone(), ResultUnit::Ampere));
    }
    Err(output_failure())
}

fn verify_inventory(
    signals: &[ResultSignal],
    inventory: &ExpectedInventory,
    shape: SignalShape,
) -> Result<(), ProcessFailure> {
    let expected_nets: BTreeSet<_> = inventory.nets.values().cloned().collect();
    let expected_branches: BTreeSet<_> = inventory.branches.values().cloned().collect();
    let actual_nets: BTreeSet<_> = signals
        .iter()
        .filter_map(|signal| match (shape, signal.kind) {
            (SignalShape::Scalar, SignalKind::NetVoltage)
            | (SignalShape::Magnitude, SignalKind::NetVoltageMagnitude) => {
                Some(signal.canonical_identity.clone())
            }
            _ => None,
        })
        .collect();
    let actual_branches: BTreeSet<_> = signals
        .iter()
        .filter_map(|signal| match (shape, signal.kind) {
            (SignalShape::Scalar, SignalKind::BranchCurrent)
            | (SignalShape::Magnitude, SignalKind::BranchCurrentMagnitude) => {
                Some(signal.canonical_identity.clone())
            }
            _ => None,
        })
        .collect();
    if actual_nets != expected_nets || actual_branches != expected_branches {
        return Err(output_failure());
    }
    Ok(())
}

fn into_signals(values: SignalValues) -> Vec<ResultSignal> {
    values
        .into_iter()
        .map(|((kind, canonical_identity, unit), values)| ResultSignal {
            kind,
            canonical_identity,
            unit,
            values,
        })
        .collect()
}

fn parse_finite(value: &str) -> Result<f64, ProcessFailure> {
    let value = value.parse::<f64>().map_err(|_| output_failure())?;
    if value.is_finite() {
        Ok(value)
    } else {
        Err(output_failure())
    }
}

fn parse_display_finite(value: &str) -> Result<f64, ProcessFailure> {
    let parsed = parse_finite(value)?;
    if parsed.to_string() == value {
        Ok(parsed)
    } else {
        Err(output_failure())
    }
}

fn canonical_number(value: &str) -> Result<String, ProcessFailure> {
    canonical_f64(parse_display_finite(value)?).map_err(|_| output_failure())
}

fn check_deadline(deadline: Instant) -> Result<(), ProcessFailure> {
    if Instant::now() >= deadline {
        Err(resource_failure())
    } else {
        Ok(())
    }
}

const fn output_failure() -> ProcessFailure {
    ProcessFailure::failed(
        PROCESS_OUTPUT,
        "Ohmnivore output is malformed, incomplete, or outside the supported CSV contract",
    )
}

const fn resource_failure() -> ProcessFailure {
    ProcessFailure::failed(
        PROCESS_RESOURCE,
        "Ohmnivore execution exceeds a process or normalized-result resource envelope",
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::simulation::{
        BackendIdentity, CONTRACT_SCHEMA_VERSION, REQUEST_SCHEMA_NAME, ReportSample,
        RequestAnalysis, RequestAssertion, SPICE_MAP_SCHEMA_NAME, SpiceDeviceIdentity,
        SpiceNetIdentity,
    };

    #[test]
    fn strict_dc_normalization_is_complete_order_independent_and_bound() {
        let fixture = fixture(AnalysisKind::DcOperatingPoint, ".OP", AxisKind::Scalar, 0.0);
        let first = normalized(
            &fixture,
            b"Variable,Value\nV(VIN),10\nV(VOUT),5\nI(V1),-0.0005\n",
        )
        .unwrap();
        let permuted = normalized(
            &fixture,
            b"Variable,Value\nI(V1),-0.0005\nV(VOUT),5\nV(VIN),10\n",
        )
        .unwrap();
        assert_eq!(first, permuted);
        assert_eq!(first.status, ExecutionStatus::Completed);
        assert_eq!(first.axis.samples, vec![canonical_f64(0.0).unwrap()]);
        first
            .verify_binding_bytes(fixture.request.as_bytes(), fixture.map.as_bytes())
            .unwrap();

        for mutant in [
            b"Variable,Value\nV(VIN),10\nI(V1),-0.0005\n".as_slice(),
            b"Variable,Value\nV(vin),10\nV(VOUT),5\nI(V1),-0.0005\n",
            b"Variable,Value\nV(VIN),10\nV(VOUT),5\nI(R1),-0.0005\n",
            b"Variable,Value\nV(VIN),10\nV(VOUT),NaN\nI(V1),-0.0005\n",
            b"Variable,Value\nV(VIN),10.0\nV(VOUT),5\nI(V1),-0.0005\n",
            b"Variable,Value\nV(VIN),+10\nV(VOUT),5\nI(V1),-0.0005\n",
            b"Variable,Value\r\nV(VIN),10\r\nV(VOUT),5\r\nI(V1),-0.0005\r\n",
            b"Variable,Value\nV(VIN),10\nV(VOUT),5\nI(V1),-0.0005",
        ] {
            assert!(normalized(&fixture, mutant).is_err(), "accepted {mutant:?}");
        }
    }

    #[test]
    fn ac_requires_the_complete_pinned_grid_and_exact_paired_inventory() {
        let fixture = fixture(
            AnalysisKind::AcLinearSweep,
            ".AC LIN 3 1 3",
            AxisKind::FrequencyHertz,
            2.0,
        );
        let header = "Frequency,V(VIN)_mag,V(VIN)_phase_deg,V(VOUT)_mag,V(VOUT)_phase_deg,I(V1)_mag,I(V1)_phase_deg";
        let valid = format!(
            "{header}\n1,1,0,0.5,0,0.00005,180\n2,1,0,0.5,0,0.00005,180\n3,1,0,0.5,0,0.00005,180\n"
        );
        let result = normalized(&fixture, valid.as_bytes()).unwrap();
        assert_eq!(
            result.axis.samples,
            [1.0, 2.0, 3.0].map(|value| canonical_f64(value).unwrap())
        );
        assert_eq!(result.signals.len(), 6);

        for mutant in [
            format!(
                "{header}\n1,1,0,0.5,0,0.00005,180\n2.0000000000000004,1,0,0.5,0,0.00005,180\n3,1,0,0.5,0,0.00005,180\n"
            ),
            format!(
                "{header}\n1.0,1,0,0.5,0,0.00005,180\n2,1,0,0.5,0,0.00005,180\n3,1,0,0.5,0,0.00005,180\n"
            ),
            format!("{header}\n1,1,0,0.5,0,0.00005,180\n3,1,0,0.5,0,0.00005,180\n"),
            "Frequency,V(VIN)_mag,V(VOUT)_phase_deg\n1,1,0\n2,1,0\n3,1,0\n"
                .to_owned(),
            "Frequency,V(VIN)_mag,V(VIN)_phase_deg,V(VOUT)_mag,V(VOUT)_phase_deg,I(R1)_mag,I(R1)_phase_deg\n1,1,0,0.5,0,1,0\n2,1,0,0.5,0,1,0\n3,1,0,0.5,0,1,0\n"
                .to_owned(),
        ] {
            assert!(normalized(&fixture, mutant.as_bytes()).is_err());
        }
    }

    #[test]
    fn transient_preserves_adaptive_rows_and_requires_exact_samples_and_stop() {
        let fixture = fixture(
            AnalysisKind::Transient,
            ".TRAN 125e-3 500e-3 0",
            AxisKind::TimeSeconds,
            0.25,
        );
        let header = "time,V(VIN),V(VOUT),I(V1)";
        let valid = format!(
            "{header}\n0,10,5,-0.0005\n0.125,10,5,-0.0005\n0.25,10,5,-0.0005\n0.375,10,5,-0.0005\n0.5,10,5,-0.0005\n"
        );
        let result = normalized(&fixture, valid.as_bytes()).unwrap();
        assert_eq!(
            result.axis.samples.last(),
            Some(&canonical_f64(0.5).unwrap())
        );

        for mutant in [
            format!(
                "{header}\n0,10,5,-0.0005\n0.125,10,5,-0.0005\n0.25,10,5,-0.0005\n0.49999999999999994,10,5,-0.0005\n"
            ),
            format!(
                "{header}\n0,10,5,-0.0005\n0.125,10,5,-0.0005\n0.25000000000000006,10,5,-0.0005\n0.5,10,5,-0.0005\n"
            ),
            format!("{header}\n0.0,10,5,-0.0005\n0.25,10,5,-0.0005\n0.5,10,5,-0.0005\n"),
            format!(
                "{header}\n0,10,5,-0.0005\n0.25,10,5,-0.0005\n0.125,10,5,-0.0005\n0.5,10,5,-0.0005\n"
            ),
            "time,V(VIN),I(V1)\n0,10,-0.0005\n0.25,10,-0.0005\n0.5,10,-0.0005\n".to_owned(),
        ] {
            assert!(normalized(&fixture, mutant.as_bytes()).is_err());
        }
    }

    #[test]
    fn resistor_only_dc_and_transient_results_need_no_branch_inventory() {
        let dc = resistor_fixture(AnalysisKind::DcOperatingPoint, ".OP", AxisKind::Scalar, 0.0);
        let result = normalized(&dc, b"Variable,Value\nV(N),0\n").unwrap();
        assert_eq!(result.signals.len(), 1);
        assert_eq!(result.signals[0].kind, SignalKind::NetVoltage);

        let transient = resistor_fixture(
            AnalysisKind::Transient,
            ".TRAN 0.1 0.2 0",
            AxisKind::TimeSeconds,
            0.2,
        );
        let result = normalized(&transient, b"time,V(N)\n0,0\n0.1,0\n0.2,0\n").unwrap();
        assert_eq!(
            result.axis.samples.last(),
            Some(&canonical_f64(0.2).unwrap())
        );
        assert_eq!(result.signals.len(), 1);
    }

    #[test]
    fn runner_rechecks_identity_comment_totality_minimality_and_uniqueness() {
        let fixture = fixture(AnalysisKind::DcOperatingPoint, ".OP", AxisKind::Scalar, 0.0);
        let map = parse_spice_identity_map(&fixture.map).unwrap();
        assert!(
            verify_spice_identity_binding(&fixture.netlist, &map, AnalysisKind::DcOperatingPoint)
                .is_ok()
        );

        let duplicate_device = fixture
            .netlist
            .replace("R1 VIN VOUT 10e3\n", "R1 VIN VOUT 10e3\nR1 VIN VOUT 10e3\n");
        assert!(
            verify_spice_identity_binding(&duplicate_device, &map, AnalysisKind::DcOperatingPoint,)
                .is_err()
        );

        let mut extra_device_map = map.clone();
        extra_device_map.devices.push(SpiceDeviceIdentity {
            semantic_path: "runner.zz_unused".to_owned(),
            reference: "R9".to_owned(),
            backend: "R9".to_owned(),
        });
        let extra_device_comment = fixture.netlist.replace(
            "* @circuitc-device 72756E6E65722E725F746F70 5231 R1\n",
            "* @circuitc-device 72756E6E65722E725F746F70 5231 R1\n* @circuitc-device 72756E6E65722E7A7A5F756E75736564 5239 R9\n",
        );
        assert!(
            verify_spice_identity_binding(
                &extra_device_comment,
                &extra_device_map,
                AnalysisKind::DcOperatingPoint,
            )
            .is_err()
        );

        let mut extra_net_map = map.clone();
        extra_net_map.nets.push(SpiceNetIdentity {
            canonical: "ZZ_UNUSED".to_owned(),
            backend: "ZZ_UNUSED".to_owned(),
            is_ground: false,
        });
        let extra_net_comment = fixture.netlist.replace(
            "* @circuitc-net 564F5554 VOUT\n",
            "* @circuitc-net 564F5554 VOUT\n* @circuitc-net 5A5A5F554E55534544 ZZ_UNUSED\n",
        );
        assert!(
            verify_spice_identity_binding(
                &extra_net_comment,
                &extra_net_map,
                AnalysisKind::DcOperatingPoint,
            )
            .is_err()
        );

        let comment_drift = fixture.netlist.replacen("474E44 0", "474E45 0", 1);
        assert!(
            verify_spice_identity_binding(&comment_drift, &map, AnalysisKind::DcOperatingPoint,)
                .is_err()
        );
    }

    #[cfg(unix)]
    #[test]
    fn digest_rebound_identity_mutants_fail_before_process_execution() {
        let fixture = fixture(AnalysisKind::DcOperatingPoint, ".OP", AxisKind::Scalar, 0.0);
        let map = parse_spice_identity_map(&fixture.map).unwrap();
        let duplicate = rebind_fixture(
            &fixture,
            fixture
                .netlist
                .replace("R1 VIN VOUT 10e3\n", "R1 VIN VOUT 10e3\nR1 VIN VOUT 10e3\n"),
            map.clone(),
        );

        let mut extra_map = map.clone();
        extra_map.devices.push(SpiceDeviceIdentity {
            semantic_path: "runner.zz_unused".to_owned(),
            reference: "R9".to_owned(),
            backend: "R9".to_owned(),
        });
        let extra = rebind_fixture(
            &fixture,
            fixture.netlist.replace(
                "* @circuitc-device 72756E6E65722E725F746F70 5231 R1\n",
                "* @circuitc-device 72756E6E65722E725F746F70 5231 R1\n* @circuitc-device 72756E6E65722E7A7A5F756E75736564 5239 R9\n",
            ),
            extra_map,
        );
        let drift = rebind_fixture(
            &fixture,
            fixture.netlist.replacen("474E44 0", "474E45 0", 1),
            map,
        );

        let body = "if [ \"$1\" = --version ]; then exit 99; fi; exit 99";
        let (runner, root) = fake_runner("identity-mutants", body, None);
        for mutant in [duplicate, extra, drift] {
            let result = runner
                .execute(
                    mutant.netlist.as_bytes(),
                    mutant.request.as_bytes(),
                    mutant.map.as_bytes(),
                )
                .unwrap();
            assert_eq!(result.status, ExecutionStatus::Failed);
            assert_eq!(result.diagnostics[0].code, PROCESS_BINDING);
        }
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn raw_input_caps_precede_large_ac_and_csv_allocations() {
        assert_eq!(validate_netlist_size(5, 4).unwrap_err().path, "netlist");
        for directive in [".AC LIN 10001 1 2", ".AC LIN 4294967295 1 2"] {
            let netlist = format!("{directive}\n.END\n");
            let failure =
                AnalysisPlan::parse(netlist.as_bytes(), AnalysisKind::AcLinearSweep).unwrap_err();
            assert_eq!(failure.code, PROCESS_RESOURCE);
        }

        let wide = "x,".repeat(MAX_CONTRACT_ENTRIES * 2 + 1);
        let csv = format!("{wide}x\n{wide}0\n");
        assert_eq!(
            parse_csv(csv.as_bytes(), test_deadline()).unwrap_err().code,
            PROCESS_RESOURCE
        );
        assert_eq!(
            parse_csv(b"a,b\n1,2\n", Instant::now()).unwrap_err().code,
            PROCESS_RESOURCE
        );
    }

    #[test]
    fn maximum_ac_grid_appends_by_index_without_recloning_long_identities() {
        use std::fmt::Write as _;

        let canonical = "N".repeat(1024 * 1024);
        let inventory = ExpectedInventory {
            nets: BTreeMap::from([("N".to_owned(), canonical.clone())]),
            branches: BTreeMap::new(),
        };
        let samples: Vec<_> = (1..=MAX_CONTRACT_ENTRIES)
            .map(|sample| sample as f64)
            .collect();
        let mut csv = String::from("Frequency,V(N)_mag,V(N)_phase_deg\n");
        for sample in 1..=MAX_CONTRACT_ENTRIES {
            writeln!(csv, "{sample},1,0").unwrap();
        }
        let rows = parse_csv(csv.as_bytes(), test_deadline()).unwrap();
        let (_, signals) = normalize_ac(&rows, &inventory, &samples, test_deadline()).unwrap();
        assert_eq!(signals.len(), 2);
        assert!(
            signals
                .iter()
                .all(|signal| signal.canonical_identity == canonical)
        );
        assert!(signals.iter().all(|signal| signal.values.len() == 10_000));
    }

    #[test]
    fn process_capture_enforces_environment_timeout_and_output_caps() {
        let root = test_root("process");
        let directory = ScopedWorkDirectory::create(&root).unwrap();
        let mut limits = OhmnivoreLimits::default();
        limits.analysis_wall = Duration::from_millis(100);
        limits.stdout_bytes = 128;
        limits.stderr_bytes = 128;

        let isolated = run_process(
            Path::new("/bin/sh"),
            directory.path(),
            [
                OsStr::new("-c"),
                OsStr::new(
                    "test \"$LC_ALL\" = C && test \"$RAYON_NUM_THREADS\" = 1 && test -d \"$HOME\" && test -d \"$TMPDIR\" && printf ok",
                ),
            ],
            limits.analysis_wall,
            &limits,
        )
        .unwrap();
        assert!(isolated.status.success());
        assert_eq!(isolated.stdout, b"ok");
        assert!(isolated.stderr.is_empty());

        let timeout = run_process(
            Path::new("/bin/sh"),
            directory.path(),
            [OsStr::new("-c"), OsStr::new("/bin/sleep 2")],
            limits.analysis_wall,
            &limits,
        )
        .unwrap_err();
        assert_eq!(timeout.code, PROCESS_RESOURCE);

        let overflow = run_process(
            Path::new("/bin/sh"),
            directory.path(),
            [
                OsStr::new("-c"),
                OsStr::new("while :; do printf 12345678901234567890; done"),
            ],
            Duration::from_secs(2),
            &limits,
        )
        .unwrap_err();
        assert_eq!(overflow.code, PROCESS_RESOURCE);
        directory.cleanup().unwrap();
        let _ = fs::remove_dir(root);
    }

    #[test]
    fn executable_provenance_binds_the_exact_platform_artifact() {
        let root = test_root("provenance");
        fs::create_dir_all(&root).unwrap();
        let executable = root.join("ohmnivore");
        let provenance = root.join("provenance.txt");
        fs::write(&executable, b"fake-platform-binary").unwrap();
        fs::write(
            &provenance,
            format!(
                "{PROVENANCE_HEADER}\nname={OHMNIVORE_BACKEND_NAME}\nversion={OHMNIVORE_BACKEND_VERSION}\ncontract={OHMNIVORE_BACKEND_CONTRACT}\nsource_revision={OHMNIVORE_SOURCE_REVISION}\nexecutable_sha256={}\n",
                sha256_hex(b"fake-platform-binary")
            ),
        )
        .unwrap();
        verify_provenance(
            &executable,
            &provenance,
            Instant::now() + Duration::from_secs(1),
        )
        .unwrap();
        fs::write(&executable, b"mutated-platform-binary").unwrap();
        assert_eq!(
            verify_provenance(
                &executable,
                &provenance,
                Instant::now() + Duration::from_secs(1),
            )
            .unwrap_err()
            .code,
            PROCESS_IDENTITY
        );
        fs::remove_dir_all(root).unwrap();
    }

    #[cfg(unix)]
    #[test]
    fn provenance_rejects_special_files_without_unbounded_reads() {
        let root = test_root("special-provenance");
        fs::create_dir_all(&root).unwrap();
        let executable = root.join("ohmnivore");
        let provenance = root.join("provenance.txt");
        fs::write(&executable, b"bounded").unwrap();
        fs::write(&provenance, b"bounded").unwrap();
        let deadline = Instant::now() + Duration::from_secs(1);
        assert_eq!(
            verify_provenance(&executable, Path::new("/dev/zero"), deadline)
                .unwrap_err()
                .code,
            PROCESS_IDENTITY
        );
        assert_eq!(
            verify_provenance(Path::new("/dev/zero"), &provenance, deadline)
                .unwrap_err()
                .code,
            PROCESS_IDENTITY
        );
        fs::remove_dir_all(root).unwrap();
    }

    #[cfg(unix)]
    #[test]
    fn full_runner_normalizes_process_failures_without_raw_host_data() {
        let fixture = fixture(AnalysisKind::DcOperatingPoint, ".OP", AxisKind::Scalar, 0.0);
        for (label, body, expected_status, expected_code) in [
            (
                "wrong-version",
                "if [ \"$1\" = --version ]; then printf 'ohmnivore 9.9.9\\n'; exit 0; fi",
                ExecutionStatus::Unsupported,
                PROCESS_IDENTITY,
            ),
            (
                "nonzero",
                "if [ \"$1\" = --version ]; then printf 'ohmnivore 0.1.0\\n'; exit 0; fi; printf 'Variable,Value\\nV(VIN),10\\nV(VOUT),5\\nI(V1),-0.0005\\n'; printf 'host-secret-path' >&2; exit 7",
                ExecutionStatus::Failed,
                PROCESS_EXIT,
            ),
            (
                "stderr",
                "if [ \"$1\" = --version ]; then printf 'ohmnivore 0.1.0\\n'; exit 0; fi; printf 'Variable,Value\\nV(VIN),10\\nV(VOUT),5\\nI(V1),-0.0005\\n'; printf warning >&2",
                ExecutionStatus::Failed,
                PROCESS_STDERR,
            ),
            (
                "malformed",
                "if [ \"$1\" = --version ]; then printf 'ohmnivore 0.1.0\\n'; exit 0; fi; printf 'Variable,Value\\nV(VIN),10\\n'",
                ExecutionStatus::Failed,
                PROCESS_OUTPUT,
            ),
        ] {
            let (runner, root) = fake_runner(label, body, None);
            let result = runner
                .execute(
                    fixture.netlist.as_bytes(),
                    fixture.request.as_bytes(),
                    fixture.map.as_bytes(),
                )
                .unwrap();
            assert_eq!(result.status, expected_status);
            assert!(result.axis.samples.is_empty());
            assert!(result.signals.is_empty());
            assert_eq!(result.diagnostics.len(), 1);
            assert_eq!(result.diagnostics[0].code, expected_code);
            let json = result.to_canonical_json().unwrap();
            assert!(!json.contains("host-secret-path"));
            assert!(!json.contains(root.to_string_lossy().as_ref()));
            fs::remove_dir_all(root).unwrap();
        }
    }

    #[cfg(unix)]
    #[test]
    fn full_runner_enforces_cpu_argv_environment_timeout_and_fresh_cleanup() {
        let fixture = fixture(AnalysisKind::DcOperatingPoint, ".OP", AxisKind::Scalar, 0.0);
        let valid = "if [ \"$1\" = --version ]; then ln -s /dev/null analysis.spice; printf 'ohmnivore 0.1.0\\n'; exit 0; fi; test \"$1\" = analysis.spice; test \"$2\" = --cpu; test \"$LC_ALL\" = C; test \"$RAYON_NUM_THREADS\" = 1; test -f analysis.spice; printf 'Variable,Value\\nV(VIN),10\\nV(VOUT),5\\nI(V1),-0.0005\\n'";
        let (runner, root) = fake_runner("valid", valid, None);
        for _ in 0..2 {
            let result = runner
                .execute(
                    fixture.netlist.as_bytes(),
                    fixture.request.as_bytes(),
                    fixture.map.as_bytes(),
                )
                .unwrap();
            assert_eq!(result.status, ExecutionStatus::Completed, "{result:#?}");
            let remaining: Vec<_> = fs::read_dir(root.join("circuitc-ohmnivore-work"))
                .unwrap()
                .filter_map(Result::ok)
                .map(|entry| entry.file_name())
                .collect();
            assert!(remaining.is_empty(), "work directory was not cleaned");
        }
        fs::remove_dir_all(root).unwrap();

        let hanging =
            "if [ \"$1\" = --version ]; then printf 'ohmnivore 0.1.0\\n'; exit 0; fi; /bin/sleep 5";
        let mut limits = OhmnivoreLimits::default();
        limits.analysis_wall = Duration::from_millis(50);
        let (runner, root) = fake_runner("hang", hanging, Some(limits));
        let result = runner
            .execute(
                fixture.netlist.as_bytes(),
                fixture.request.as_bytes(),
                fixture.map.as_bytes(),
            )
            .unwrap();
        assert_eq!(result.status, ExecutionStatus::Failed);
        assert_eq!(result.diagnostics[0].code, PROCESS_RESOURCE);
        fs::remove_dir_all(root).unwrap();

        let cleanup_failure = "if [ \"$1\" = --version ]; then printf 'ohmnivore 0.1.0\\n'; exit 0; fi; chmod 000 .; printf 'Variable,Value\\nV(VIN),10\\nV(VOUT),5\\nI(V1),-0.0005\\n'";
        let (runner, root) = fake_runner("cleanup", cleanup_failure, None);
        let result = runner
            .execute(
                fixture.netlist.as_bytes(),
                fixture.request.as_bytes(),
                fixture.map.as_bytes(),
            )
            .unwrap();
        assert_eq!(result.status, ExecutionStatus::Failed);
        assert_eq!(result.diagnostics[0].code, PROCESS_IO);
        let work_root = root.join("circuitc-ohmnivore-work");
        for entry in fs::read_dir(&work_root).unwrap().filter_map(Result::ok) {
            fs::set_permissions(entry.path(), fs::Permissions::from_mode(0o700)).unwrap();
        }
        fs::remove_dir_all(root).unwrap();

        let mut limits = OhmnivoreLimits::default();
        limits.compilation_wall = Duration::from_millis(1);
        let (runner, root) = fake_runner("aggregate", valid, Some(limits));
        thread::sleep(Duration::from_millis(2));
        let result = runner
            .execute(
                fixture.netlist.as_bytes(),
                fixture.request.as_bytes(),
                fixture.map.as_bytes(),
            )
            .unwrap();
        assert_eq!(result.status, ExecutionStatus::Failed);
        assert_eq!(result.diagnostics[0].code, PROCESS_RESOURCE);
        fs::remove_dir_all(root).unwrap();
    }

    struct Fixture {
        netlist: String,
        request: String,
        map: String,
    }

    fn fixture(kind: AnalysisKind, directive: &str, sample_kind: AxisKind, sample: f64) -> Fixture {
        let source = if kind == AnalysisKind::AcLinearSweep {
            "V1 VIN 0 DC 10 AC 1 0"
        } else {
            "V1 VIN 0 DC 10"
        };
        let netlist = format!(
            "* @circuitc-net 474E44 0\n\
             * @circuitc-net 56494E VIN\n\
             * @circuitc-net 564F5554 VOUT\n\
             * @circuitc-device 72756E6E65722E696E707574 5631 V1\n\
             * @circuitc-device 72756E6E65722E725F626F74746F6D 5232 R2\n\
             * @circuitc-device 72756E6E65722E725F746F70 5231 R1\n\
             {source}\nR2 VOUT 0 10e3\nR1 VIN VOUT 10e3\n{directive}\n.END\n"
        );
        let request = SimulationRequest {
            schema_name: REQUEST_SCHEMA_NAME.to_owned(),
            schema_version: CONTRACT_SCHEMA_VERSION,
            design: "runner_test".to_owned(),
            backend: BackendIdentity {
                name: OHMNIVORE_BACKEND_NAME.to_owned(),
                version: OHMNIVORE_BACKEND_VERSION.to_owned(),
                contract: OHMNIVORE_BACKEND_CONTRACT.to_owned(),
                source_revision: OHMNIVORE_SOURCE_REVISION.to_owned(),
            },
            analysis: RequestAnalysis {
                path: "runner.analysis".to_owned(),
                kind,
                netlist_path: "simulation/test/analysis.spice".to_owned(),
                netlist_sha256: sha256_hex(netlist.as_bytes()),
                map_path: "simulation/test/spice-map.json".to_owned(),
            },
            assertions: vec![RequestAssertion {
                path: "runner.assertion".to_owned(),
                signal_kind: match kind {
                    AnalysisKind::AcLinearSweep => SignalKind::NetVoltageMagnitude,
                    _ => SignalKind::NetVoltage,
                },
                canonical_identity: "VOUT".to_owned(),
                sample: ReportSample {
                    kind: sample_kind,
                    value: canonical_f64(sample).unwrap(),
                },
                unit: ResultUnit::Volt,
                expected: canonical_f64(5.0).unwrap(),
                absolute_tolerance: canonical_f64(1e-6).unwrap(),
                relative_tolerance: canonical_f64(0.0).unwrap(),
            }],
        };
        let request = request.to_canonical_json().unwrap();
        let map = SpiceIdentityMap {
            schema_name: SPICE_MAP_SCHEMA_NAME.to_owned(),
            schema_version: CONTRACT_SCHEMA_VERSION,
            design: "runner_test".to_owned(),
            analysis_path: "runner.analysis".to_owned(),
            request_sha256: sha256_hex(request.as_bytes()),
            nets: vec![
                SpiceNetIdentity {
                    canonical: "GND".to_owned(),
                    backend: "0".to_owned(),
                    is_ground: true,
                },
                SpiceNetIdentity {
                    canonical: "VIN".to_owned(),
                    backend: "VIN".to_owned(),
                    is_ground: false,
                },
                SpiceNetIdentity {
                    canonical: "VOUT".to_owned(),
                    backend: "VOUT".to_owned(),
                    is_ground: false,
                },
            ],
            devices: vec![
                SpiceDeviceIdentity {
                    semantic_path: "runner.input".to_owned(),
                    reference: "V1".to_owned(),
                    backend: "V1".to_owned(),
                },
                SpiceDeviceIdentity {
                    semantic_path: "runner.r_bottom".to_owned(),
                    reference: "R2".to_owned(),
                    backend: "R2".to_owned(),
                },
                SpiceDeviceIdentity {
                    semantic_path: "runner.r_top".to_owned(),
                    reference: "R1".to_owned(),
                    backend: "R1".to_owned(),
                },
            ],
        }
        .to_canonical_json()
        .unwrap();
        Fixture {
            netlist,
            request,
            map,
        }
    }

    fn resistor_fixture(
        kind: AnalysisKind,
        directive: &str,
        sample_kind: AxisKind,
        sample: f64,
    ) -> Fixture {
        let netlist = format!(
            "* @circuitc-net 474E44 0\n\
             * @circuitc-net 4E N\n\
             * @circuitc-device 72756E6E65722E7265736973746F72 5231 R1\n\
             R1 N 0 1e3\n{directive}\n.END\n"
        );
        let request = SimulationRequest {
            schema_name: REQUEST_SCHEMA_NAME.to_owned(),
            schema_version: CONTRACT_SCHEMA_VERSION,
            design: "runner_resistor_test".to_owned(),
            backend: BackendIdentity {
                name: OHMNIVORE_BACKEND_NAME.to_owned(),
                version: OHMNIVORE_BACKEND_VERSION.to_owned(),
                contract: OHMNIVORE_BACKEND_CONTRACT.to_owned(),
                source_revision: OHMNIVORE_SOURCE_REVISION.to_owned(),
            },
            analysis: RequestAnalysis {
                path: "runner.resistor.analysis".to_owned(),
                kind,
                netlist_path: "simulation/test/analysis.spice".to_owned(),
                netlist_sha256: sha256_hex(netlist.as_bytes()),
                map_path: "simulation/test/spice-map.json".to_owned(),
            },
            assertions: vec![RequestAssertion {
                path: "runner.resistor.assertion".to_owned(),
                signal_kind: SignalKind::NetVoltage,
                canonical_identity: "N".to_owned(),
                sample: ReportSample {
                    kind: sample_kind,
                    value: canonical_f64(sample).unwrap(),
                },
                unit: ResultUnit::Volt,
                expected: canonical_f64(0.0).unwrap(),
                absolute_tolerance: canonical_f64(1e-6).unwrap(),
                relative_tolerance: canonical_f64(0.0).unwrap(),
            }],
        }
        .to_canonical_json()
        .unwrap();
        let map = SpiceIdentityMap {
            schema_name: SPICE_MAP_SCHEMA_NAME.to_owned(),
            schema_version: CONTRACT_SCHEMA_VERSION,
            design: "runner_resistor_test".to_owned(),
            analysis_path: "runner.resistor.analysis".to_owned(),
            request_sha256: sha256_hex(request.as_bytes()),
            nets: vec![
                SpiceNetIdentity {
                    canonical: "GND".to_owned(),
                    backend: "0".to_owned(),
                    is_ground: true,
                },
                SpiceNetIdentity {
                    canonical: "N".to_owned(),
                    backend: "N".to_owned(),
                    is_ground: false,
                },
            ],
            devices: vec![SpiceDeviceIdentity {
                semantic_path: "runner.resistor".to_owned(),
                reference: "R1".to_owned(),
                backend: "R1".to_owned(),
            }],
        }
        .to_canonical_json()
        .unwrap();
        Fixture {
            netlist,
            request,
            map,
        }
    }

    fn normalized(fixture: &Fixture, csv: &[u8]) -> Result<SimulationResult, ProcessFailure> {
        let request = parse_request(&fixture.request).unwrap();
        let map = parse_spice_identity_map(&fixture.map).unwrap();
        let plan = AnalysisPlan::parse(fixture.netlist.as_bytes(), request.analysis.kind).unwrap();
        let inventory = ExpectedInventory::parse(fixture.netlist.as_bytes(), &map).unwrap();
        normalize_csv(
            csv,
            NormalizationContext {
                request: &request,
                map: &map,
                request_json: fixture.request.as_bytes(),
                map_json: fixture.map.as_bytes(),
                plan: &plan,
                inventory: &inventory,
                deadline: test_deadline(),
            },
        )
    }

    fn test_deadline() -> Instant {
        Instant::now() + Duration::from_secs(60)
    }

    fn rebind_fixture(fixture: &Fixture, netlist: String, mut map: SpiceIdentityMap) -> Fixture {
        let mut request = parse_request(&fixture.request).unwrap();
        request.analysis.netlist_sha256 = sha256_hex(netlist.as_bytes());
        let request = request.to_canonical_json().unwrap();
        map.request_sha256 = sha256_hex(request.as_bytes());
        let map = map.to_canonical_json().unwrap();
        Fixture {
            netlist,
            request,
            map,
        }
    }

    fn test_root(label: &str) -> PathBuf {
        std::env::temp_dir().join(format!(
            "circuitc-runner-{label}-{}-{}",
            std::process::id(),
            WORK_DIRECTORY_SEQUENCE.fetch_add(1, Ordering::Relaxed)
        ))
    }

    #[cfg(unix)]
    fn fake_runner(
        label: &str,
        body: &str,
        limits: Option<OhmnivoreLimits>,
    ) -> (OhmnivoreRunner, PathBuf) {
        let root = test_root(label);
        fs::create_dir_all(&root).unwrap();
        let executable = root.join("fake-ohmnivore");
        let provenance = root.join("provenance.txt");
        fs::write(&executable, format!("#!/bin/sh\n{body}\n")).unwrap();
        fs::set_permissions(&executable, fs::Permissions::from_mode(0o700)).unwrap();
        let executable_bytes = fs::read(&executable).unwrap();
        fs::write(
            &provenance,
            format!(
                "{PROVENANCE_HEADER}\nname={OHMNIVORE_BACKEND_NAME}\nversion={OHMNIVORE_BACKEND_VERSION}\ncontract={OHMNIVORE_BACKEND_CONTRACT}\nsource_revision={OHMNIVORE_SOURCE_REVISION}\nexecutable_sha256={}\n",
                sha256_hex(&executable_bytes)
            ),
        )
        .unwrap();
        let runner = OhmnivoreRunner::from_paths(&executable, &provenance, &root)
            .with_limits(limits.unwrap_or_default());
        (runner, root)
    }
}
