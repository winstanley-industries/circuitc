//! Bounded execution of the pinned APGAR CPU route adapter.

use std::env;
use std::ffi::OsString;
use std::fs;
use std::io::{self, Read, Write};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, ExitStatus, Stdio};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::mpsc::{self, Receiver, RecvTimeoutError};
use std::sync::{Arc, OnceLock};
use std::thread;
use std::time::{Duration, Instant};

use sha2::{Digest, Sha256};

#[cfg(unix)]
use std::os::unix::fs::{DirBuilderExt as _, MetadataExt as _};
#[cfg(unix)]
use std::os::unix::process::CommandExt as _;

use super::contract::{ContractDiagnostic, MAX_CONTRACT_BYTES, ToolIdentity, parse_result};
use super::import::expected_cpu_tool;
use super::lower::RouteInputBundle;
use super::{
    APGAR_CONTRACT_IDENTITY, APGAR_CPU_DEVICE_CLASS, APGAR_TOOL_NAME, APGAR_TOOL_VERSION,
    PINNED_APGAR_SOURCE_REVISION,
};

const PROCESS_IDENTITY: &str = "CC-ROUTE-PROCESS-001";
const PROCESS_IO: &str = "CC-ROUTE-PROCESS-002";
const PROCESS_RESOURCE: &str = "CC-ROUTE-PROCESS-003";
const PROCESS_EXIT: &str = "CC-ROUTE-PROCESS-004";
const PROCESS_OUTPUT: &str = "CC-ROUTE-PROCESS-005";
const PROVENANCE_HEADER: &str = "circuitc-apgar-route-provenance-v1";
const EXECUTABLE_RUNFILE: &str = "_main/apgar_route_adapter";
const PROVENANCE_RUNFILE: &str = "_main/apgar-route-provenance.txt";
const MAX_PROVENANCE_BYTES: u64 = 1_024;
const MAX_EXECUTABLE_BYTES: u64 = 2 * 1024 * 1024 * 1024;
const READ_CHUNK_BYTES: usize = 8 * 1024;
const POLL_INTERVAL: Duration = Duration::from_millis(5);
const IDENTITY_WALL: Duration = Duration::from_secs(30);
const ADDRESS_SPACE_BYTES: u64 = 8 * 1024 * 1024 * 1024;
const FILE_BYTES: u64 = 64 * 1024 * 1024;
const OPEN_FILES: u64 = 32;

static WORK_SEQUENCE: AtomicU64 = AtomicU64::new(0);

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct ExecutedRoute {
    pub(crate) result_json: String,
    pub(crate) tool: ToolIdentity,
}

#[derive(Clone, Debug)]
pub(crate) struct ApgarRunner {
    executable: PathBuf,
    provenance: PathBuf,
    work_root: PathBuf,
    verified: Arc<OnceLock<Result<VerifiedExecutable, ContractDiagnostic>>>,
}

#[derive(Clone, Debug)]
struct VerifiedExecutable {
    path: PathBuf,
    tool: ToolIdentity,
}

impl ApgarRunner {
    pub(crate) fn from_bazel_runfiles(
        work_root: impl Into<PathBuf>,
    ) -> Result<Self, ContractDiagnostic> {
        Ok(Self {
            executable: resolve_bazel_runfile(EXECUTABLE_RUNFILE)?,
            provenance: resolve_bazel_runfile(PROVENANCE_RUNFILE)?,
            work_root: work_root.into().join("circuitc-apgar-route-work"),
            verified: Arc::new(OnceLock::new()),
        })
    }

    #[cfg(test)]
    fn from_paths(
        executable: impl Into<PathBuf>,
        provenance: impl Into<PathBuf>,
        work_root: impl Into<PathBuf>,
    ) -> Self {
        Self {
            executable: executable.into(),
            provenance: provenance.into(),
            work_root: work_root.into().join("circuitc-apgar-route-work"),
            verified: Arc::new(OnceLock::new()),
        }
    }

    pub(crate) fn execute(
        &self,
        bundle: &RouteInputBundle,
    ) -> Result<ExecutedRoute, ContractDiagnostic> {
        if bundle.request_json.len() > MAX_CONTRACT_BYTES
            || super::contract::sha256_hex(bundle.request_json.as_bytes()) != bundle.request_sha256
        {
            return Err(process_error(
                PROCESS_OUTPUT,
                "request",
                "routing request bytes do not satisfy their authenticated bound",
            ));
        }
        let verified = self
            .verified
            .get_or_init(|| self.verify_executable())
            .clone()?;
        let directory = ScopedWorkDirectory::create(&self.work_root).map_err(|_| {
            process_error(
                PROCESS_IO,
                "runner.work_directory",
                "could not create a private APGAR working directory",
            )
        })?;
        let timeout = Duration::from_millis(bundle.request.resource_limits.timeout_milliseconds);
        let stdout_limit = usize::try_from(bundle.request.resource_limits.stdout_bytes)
            .unwrap_or(usize::MAX)
            .min(MAX_CONTRACT_BYTES);
        let stderr_limit = usize::try_from(bundle.request.resource_limits.stderr_bytes)
            .unwrap_or(usize::MAX)
            .min(MAX_CONTRACT_BYTES);
        let arguments = [
            OsString::from("--request-sha256"),
            OsString::from(&bundle.request_sha256),
            OsString::from("--executable-sha256"),
            OsString::from(&verified.tool.executable_sha256),
        ];
        let process = run_process(
            &verified.path,
            directory.path(),
            &arguments,
            bundle.request_json.as_bytes(),
            timeout,
            stdout_limit,
            stderr_limit,
        );
        let cleanup = directory.cleanup();
        if cleanup.is_err() {
            return Err(process_error(
                PROCESS_IO,
                "runner.work_directory",
                "could not clean the private APGAR working directory",
            ));
        }
        let process = process?;
        if !process.status.success() {
            return Err(process_error(
                PROCESS_EXIT,
                &bundle.request.request_path,
                "APGAR route adapter exited unsuccessfully",
            ));
        }
        if !process.stderr.is_empty() {
            return Err(process_error(
                PROCESS_OUTPUT,
                &bundle.request.request_path,
                "APGAR route adapter emitted unexpected standard error",
            ));
        }
        let result_json = String::from_utf8(process.stdout).map_err(|_| {
            process_error(
                PROCESS_OUTPUT,
                &bundle.request.request_path,
                "APGAR route adapter output is not UTF-8",
            )
        })?;
        let result = parse_result(&result_json).map_err(|error| {
            process_error(
                PROCESS_OUTPUT,
                error.path,
                format!(
                    "APGAR route adapter output is not canonical: {}",
                    error.message
                ),
            )
        })?;
        if result.tool != verified.tool {
            return Err(process_error(
                PROCESS_IDENTITY,
                "result.tool",
                "APGAR route adapter output does not repeat its authenticated executable identity",
            ));
        }
        Ok(ExecutedRoute {
            result_json,
            tool: verified.tool,
        })
    }

    fn verify_executable(&self) -> Result<VerifiedExecutable, ContractDiagnostic> {
        let executable = self.executable.canonicalize().map_err(|_| {
            process_error(
                PROCESS_IDENTITY,
                "runner.executable",
                "could not resolve the Bazel-owned APGAR executable",
            )
        })?;
        let deadline = Instant::now()
            .checked_add(IDENTITY_WALL)
            .unwrap_or_else(Instant::now);
        let executable_sha256 =
            sha256_bounded_regular_file(&executable, MAX_EXECUTABLE_BYTES, deadline).map_err(
                |_| {
                    process_error(
                        PROCESS_IDENTITY,
                        "runner.executable",
                        "could not hash the bounded Bazel-owned APGAR executable",
                    )
                },
            )?;
        let provenance =
            read_bounded_regular_file(&self.provenance, MAX_PROVENANCE_BYTES, deadline).map_err(
                |_| {
                    process_error(
                        PROCESS_IDENTITY,
                        "runner.provenance",
                        "could not read the bounded Bazel-owned APGAR provenance",
                    )
                },
            )?;
        let provenance = std::str::from_utf8(&provenance).map_err(|_| {
            process_error(
                PROCESS_IDENTITY,
                "runner.provenance",
                "APGAR provenance is not UTF-8",
            )
        })?;
        let expected = format!(
            "{PROVENANCE_HEADER}\nname={APGAR_TOOL_NAME}\nversion={APGAR_TOOL_VERSION}\ncontract={APGAR_CONTRACT_IDENTITY}\nsource_revision={PINNED_APGAR_SOURCE_REVISION}\nexecutable_sha256={executable_sha256}\ndevice_class={APGAR_CPU_DEVICE_CLASS}\n"
        );
        if provenance != expected {
            return Err(process_error(
                PROCESS_IDENTITY,
                "runner.provenance",
                "APGAR executable provenance does not match the pinned route contract",
            ));
        }
        Ok(VerifiedExecutable {
            path: executable,
            tool: expected_cpu_tool(executable_sha256),
        })
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
                .map_err(|_| runfiles_error())?;
        let manifest = std::str::from_utf8(&manifest).map_err(|_| runfiles_error())?;
        for line in manifest.lines() {
            if let Some((logical, physical)) = line.split_once(' ')
                && logical == logical_path
            {
                return Ok(PathBuf::from(physical));
            }
        }
    }
    Err(runfiles_error())
}

fn runfiles_error() -> ContractDiagnostic {
    process_error(
        PROCESS_IDENTITY,
        "runner.runfiles",
        "could not resolve the Bazel-owned APGAR runfiles",
    )
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
            let sequence = WORK_SEQUENCE.fetch_add(1, Ordering::Relaxed);
            let path = root.join(format!("circuitc-apgar-{}-{sequence}", std::process::id()));
            match create_new_private_directory(&path) {
                Ok(()) => return Ok(Self(Some(path))),
                Err(error) if error.kind() == io::ErrorKind::AlreadyExists => continue,
                Err(error) => return Err(error),
            }
        }
        Err(io::Error::new(
            io::ErrorKind::AlreadyExists,
            "could not allocate a unique APGAR process directory",
        ))
    }

    fn path(&self) -> &Path {
        self.0.as_deref().expect("working directory is live")
    }

    fn cleanup(mut self) -> io::Result<()> {
        fs::remove_dir_all(self.0.take().expect("working directory is live"))
    }
}

impl Drop for ScopedWorkDirectory {
    fn drop(&mut self) {
        if let Some(path) = self.0.take() {
            let _ = fs::remove_dir_all(path);
        }
    }
}

#[cfg(unix)]
fn create_private_directory(path: &Path) -> io::Result<()> {
    let mut builder = fs::DirBuilder::new();
    builder.recursive(true).mode(0o700).create(path)
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
            "APGAR work root is not private and caller-owned",
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
            "APGAR work root is not a directory",
        ))
    }
}

#[derive(Debug)]
struct CapturedProcess {
    status: ExitStatus,
    stdout: Vec<u8>,
    stderr: Vec<u8>,
}

#[allow(clippy::too_many_arguments)]
fn run_process(
    executable: &Path,
    current_directory: &Path,
    arguments: &[OsString],
    input: &[u8],
    wall_limit: Duration,
    stdout_limit: usize,
    stderr_limit: usize,
) -> Result<CapturedProcess, ContractDiagnostic> {
    let mut command = Command::new(executable);
    command
        .args(arguments)
        .current_dir(current_directory)
        .env_clear()
        .env("LANG", "C")
        .env("LC_ALL", "C")
        .env("TZ", "UTC")
        .env("NO_COLOR", "1")
        .env("OMP_NUM_THREADS", "1")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    configure_unix_process(&mut command, wall_limit);
    let mut child = command.spawn().map_err(|_| {
        process_error(
            PROCESS_IO,
            "runner.process",
            "could not launch the APGAR route adapter",
        )
    })?;
    let stdin = child.stdin.take().ok_or_else(|| {
        process_error(
            PROCESS_IO,
            "runner.process.stdin",
            "could not open APGAR standard input",
        )
    })?;
    let stdout = child.stdout.take().ok_or_else(|| {
        process_error(
            PROCESS_IO,
            "runner.process.stdout",
            "could not capture APGAR standard output",
        )
    })?;
    let stderr = child.stderr.take().ok_or_else(|| {
        process_error(
            PROCESS_IO,
            "runner.process.stderr",
            "could not capture APGAR standard error",
        )
    })?;
    let overflow = Arc::new(AtomicBool::new(false));
    let stdout_reader = spawn_reader(stdout, stdout_limit, Arc::clone(&overflow));
    let stderr_reader = spawn_reader(stderr, stderr_limit, Arc::clone(&overflow));
    let input_writer = spawn_writer(stdin, input.to_vec());
    let deadline = Instant::now()
        .checked_add(wall_limit)
        .unwrap_or_else(Instant::now);
    let status = loop {
        if overflow.load(Ordering::SeqCst) {
            terminate(&mut child);
            return Err(resource_error(
                "APGAR exceeded a bounded process output limit",
            ));
        }
        match child.try_wait() {
            Ok(Some(status)) => break status,
            Ok(None) if Instant::now() < deadline => thread::sleep(POLL_INTERVAL),
            Ok(None) => {
                terminate(&mut child);
                return Err(resource_error(
                    "APGAR exceeded its wall-clock execution limit",
                ));
            }
            Err(_) => {
                terminate(&mut child);
                return Err(process_error(
                    PROCESS_IO,
                    "runner.process",
                    "could not observe the APGAR process status",
                ));
            }
        }
    };
    let write_result = receive_writer(&input_writer);
    let stdout = receive_reader(&stdout_reader, &mut child)?;
    let stderr = receive_reader(&stderr_reader, &mut child)?;
    if overflow.load(Ordering::SeqCst) {
        return Err(resource_error(
            "APGAR exceeded a bounded process output limit",
        ));
    }
    if status.success() {
        write_result?;
    }
    Ok(CapturedProcess {
        status,
        stdout,
        stderr,
    })
}

fn spawn_writer(
    mut writer: impl Write + Send + 'static,
    bytes: Vec<u8>,
) -> Receiver<io::Result<()>> {
    let (sender, receiver) = mpsc::sync_channel(1);
    thread::spawn(move || {
        let result = writer.write_all(&bytes).and_then(|()| writer.flush());
        let _ = sender.send(result);
    });
    receiver
}

fn receive_writer(writer: &Receiver<io::Result<()>>) -> Result<(), ContractDiagnostic> {
    match writer.recv_timeout(Duration::from_secs(1)) {
        Ok(Ok(())) => Ok(()),
        _ => Err(process_error(
            PROCESS_IO,
            "runner.process.stdin",
            "could not write bounded APGAR process input",
        )),
    }
}

fn spawn_reader(
    mut reader: impl Read + Send + 'static,
    limit: usize,
    overflow: Arc<AtomicBool>,
) -> Receiver<io::Result<Vec<u8>>> {
    let (sender, receiver) = mpsc::sync_channel(1);
    thread::spawn(move || {
        let mut bytes = Vec::new();
        let mut chunk = [0_u8; READ_CHUNK_BYTES];
        let result = loop {
            match reader.read(&mut chunk) {
                Ok(0) => break Ok(bytes),
                Ok(count)
                    if bytes
                        .len()
                        .checked_add(count)
                        .is_some_and(|size| size <= limit) =>
                {
                    bytes.extend_from_slice(&chunk[..count]);
                }
                Ok(_) => {
                    overflow.store(true, Ordering::SeqCst);
                    break Ok(bytes);
                }
                Err(error) => break Err(error),
            }
        };
        let _ = sender.send(result);
    });
    receiver
}

fn receive_reader(
    reader: &Receiver<io::Result<Vec<u8>>>,
    child: &mut Child,
) -> Result<Vec<u8>, ContractDiagnostic> {
    match reader.recv_timeout(Duration::from_millis(250)) {
        Ok(Ok(bytes)) => Ok(bytes),
        Ok(Err(_)) | Err(RecvTimeoutError::Disconnected) => Err(process_error(
            PROCESS_IO,
            "runner.process.output",
            "could not read bounded APGAR process output",
        )),
        Err(RecvTimeoutError::Timeout) => {
            terminate(child);
            Err(resource_error(
                "APGAR output pipes did not close within the drain limit",
            ))
        }
    }
}

fn terminate(child: &mut Child) {
    #[cfg(unix)]
    // SAFETY: the child PID comes from spawn and the negative PID targets only
    // the child-owned process group configured below.
    unsafe {
        let _ = libc::kill(-(child.id() as libc::pid_t), libc::SIGKILL);
    }
    let _ = child.kill();
    let _ = child.wait();
}

#[cfg(unix)]
fn configure_unix_process(command: &mut Command, wall_limit: Duration) {
    let cpu_seconds = wall_limit.as_secs().saturating_add(1).max(1);
    command.process_group(0);
    // SAFETY: pre_exec performs only async-signal-safe setrlimit calls with
    // captured integer values.
    unsafe {
        command.pre_exec(move || {
            set_memory_limit(ADDRESS_SPACE_BYTES)?;
            set_limit(libc::RLIMIT_CPU, cpu_seconds)?;
            set_limit(libc::RLIMIT_FSIZE, FILE_BYTES)?;
            set_limit(libc::RLIMIT_NOFILE, OPEN_FILES)?;
            set_limit(libc::RLIMIT_CORE, 0)?;
            Ok(())
        });
    }
}

#[cfg(not(unix))]
fn configure_unix_process(_command: &mut Command, _wall_limit: Duration) {}

#[cfg(all(unix, target_os = "linux"))]
fn set_memory_limit(value: u64) -> io::Result<()> {
    set_limit(libc::RLIMIT_AS, value)
}

#[cfg(all(unix, not(target_os = "linux")))]
fn set_memory_limit(_value: u64) -> io::Result<()> {
    Ok(())
}

#[cfg(all(unix, target_os = "linux"))]
type RlimitResource = libc::__rlimit_resource_t;

#[cfg(all(unix, not(target_os = "linux")))]
type RlimitResource = libc::c_int;

#[cfg(unix)]
fn set_limit(resource: RlimitResource, value: u64) -> io::Result<()> {
    let limit = libc::rlimit {
        rlim_cur: value,
        rlim_max: value,
    };
    // SAFETY: limit is initialized and resource is a platform RLIMIT constant.
    if unsafe { libc::setrlimit(resource, &limit) } == 0 {
        Ok(())
    } else {
        Err(io::Error::last_os_error())
    }
}

fn resource_error(message: &str) -> ContractDiagnostic {
    process_error(PROCESS_RESOURCE, "runner.process", message)
}

fn process_error(
    code: &str,
    path: impl Into<String>,
    message: impl Into<String>,
) -> ContractDiagnostic {
    ContractDiagnostic {
        code: code.to_owned(),
        path: path.into(),
        message: message.into(),
    }
}

#[cfg(test)]
mod tests {
    use std::env;
    use std::ffi::OsString;
    use std::fs;
    use std::path::{Path, PathBuf};
    use std::time::Duration;

    #[cfg(unix)]
    use std::os::unix::fs::PermissionsExt as _;

    use crate::demo;
    use crate::design::{CopperLayer, RoutingRequest};

    use super::super::contract::{
        BoxDbu, LinePrimitive, PointDbu, RouteFailureStatus, RouteOutcome, parse_result,
        render_request, sha256_hex,
    };
    use super::super::import::import_result;
    use super::super::lower::{RouteInputBundle, lower_request};
    use super::{
        APGAR_CONTRACT_IDENTITY, APGAR_CPU_DEVICE_CLASS, APGAR_TOOL_NAME, APGAR_TOOL_VERSION,
        ApgarRunner, PINNED_APGAR_SOURCE_REVISION, PROCESS_EXIT, PROCESS_IDENTITY, PROCESS_OUTPUT,
        PROCESS_RESOURCE, PROVENANCE_HEADER, ScopedWorkDirectory, WORK_SEQUENCE, run_process,
    };

    const MM_DBU: i64 = 2_000_000;

    fn routing_design() -> crate::design::Design {
        let mut design = demo::voltage_divider();
        design.board.routes.clear();
        design.board.routing_requests.push(RoutingRequest {
            path: "board.autoroute.vout".to_owned(),
            net: "VOUT".to_owned(),
            width_nm: 250_000,
            clearance_nm: 200_000,
            grid_step_nm: 1_000_000,
            layer: CopperLayer::Front,
        });
        design.canonicalize();
        design
    }

    fn test_root(label: &str) -> PathBuf {
        env::var_os("TEST_TMPDIR")
            .map(PathBuf::from)
            .unwrap_or_else(env::temp_dir)
            .join(format!(
                "circuitc-apgar-{label}-{}-{}",
                std::process::id(),
                WORK_SEQUENCE.fetch_add(1, std::sync::atomic::Ordering::Relaxed)
            ))
    }

    fn geometric_bundle(
        start: PointDbu,
        goal: PointDbu,
        obstacle: Option<BoxDbu>,
    ) -> RouteInputBundle {
        let mut bundle = lower_request(&routing_design()).unwrap().unwrap();
        let request = &mut bundle.request;
        let mut obstacle_template = request.obstacles[0].clone();
        request.obstacles.clear();

        let roi = BoxDbu {
            min: PointDbu { x: 0, y: 0 },
            max: PointDbu {
                x: 20 * MM_DBU,
                y: 20 * MM_DBU,
            },
        };
        request.compiler_profile.lattice_origin = roi.min;
        request.compiler_profile.lattice_step_dbu = MM_DBU;
        request.compiler_profile.compilation_roi = roi;
        request.compiler_profile.active_regions[0].bounds = roi;
        request.planar_route.start = start;
        request.planar_route.goal = goal;
        for (terminal, center) in request.terminals.iter_mut().zip([start, goal]) {
            terminal.center = center;
            terminal.connection_region = BoxDbu {
                min: PointDbu {
                    x: center.x - 500_000,
                    y: center.y - 500_000,
                },
                max: PointDbu {
                    x: center.x + 500_000,
                    y: center.y + 500_000,
                },
            };
        }
        if let Some(bounds) = obstacle {
            obstacle_template.bounds = bounds;
            obstacle_template.owner_net = None;
            obstacle_template.provenance = "runner.real-adapter.obstacle".to_owned();
            request.obstacles.push(obstacle_template);
        }
        request.validate().unwrap();
        bundle.request_json = render_request(request).unwrap();
        bundle.request_sha256 = sha256_hex(bundle.request_json.as_bytes());
        bundle
    }

    fn completed_geometry(result_json: &str) -> Vec<LinePrimitive> {
        let result = parse_result(result_json).unwrap();
        let RouteOutcome::Completed { candidates, .. } = result.outcome else {
            panic!("real APGAR CPU adapter returned a failure result")
        };
        assert_eq!(candidates.len(), 1);
        candidates[0].geometry.clone()
    }

    #[test]
    fn pinned_real_cpu_adapter_repeats_and_imports_exactly() {
        let design = routing_design();
        let bundle = lower_request(&design).unwrap().unwrap();
        let work_root = test_root("real-repeat");
        let runner = ApgarRunner::from_bazel_runfiles(work_root).unwrap();
        let first = runner.execute(&bundle).unwrap();
        let second = runner.execute(&bundle).unwrap();
        assert_eq!(first, second);
        let imported = import_result(&design, &bundle, &first.result_json, &first.tool).unwrap();
        assert!(imported.design.board.routing_requests.is_empty());
        let RouteOutcome::Completed { candidates, .. } = imported.result.outcome else {
            panic!("real APGAR CPU adapter returned a failure result")
        };
        assert_eq!(candidates.len(), 1);
    }

    #[test]
    fn pinned_real_cpu_adapter_covers_vertical_diagonal_detour_and_no_route() {
        let runner = ApgarRunner::from_bazel_runfiles(test_root("real-shapes")).unwrap();

        let vertical = geometric_bundle(
            PointDbu {
                x: 10 * MM_DBU,
                y: 4 * MM_DBU,
            },
            PointDbu {
                x: 10 * MM_DBU,
                y: 16 * MM_DBU,
            },
            None,
        );
        let vertical = completed_geometry(&runner.execute(&vertical).unwrap().result_json);
        assert_eq!(vertical.len(), 1);
        assert_eq!(vertical[0].start.x, vertical[0].end.x);

        let diagonal = geometric_bundle(
            PointDbu {
                x: 4 * MM_DBU,
                y: 4 * MM_DBU,
            },
            PointDbu {
                x: 16 * MM_DBU,
                y: 16 * MM_DBU,
            },
            None,
        );
        let diagonal = completed_geometry(&runner.execute(&diagonal).unwrap().result_json);
        assert_eq!(diagonal.len(), 1);
        assert_eq!(
            (diagonal[0].end.x - diagonal[0].start.x).unsigned_abs(),
            (diagonal[0].end.y - diagonal[0].start.y).unsigned_abs()
        );

        let detour = geometric_bundle(
            PointDbu {
                x: 4 * MM_DBU,
                y: 10 * MM_DBU,
            },
            PointDbu {
                x: 16 * MM_DBU,
                y: 10 * MM_DBU,
            },
            Some(BoxDbu {
                min: PointDbu {
                    x: 8 * MM_DBU,
                    y: 8 * MM_DBU,
                },
                max: PointDbu {
                    x: 12 * MM_DBU,
                    y: 12 * MM_DBU,
                },
            }),
        );
        let detour = completed_geometry(&runner.execute(&detour).unwrap().result_json);
        assert!(detour.len() > 1, "obstacle must force a multi-line route");

        let blocked = geometric_bundle(
            PointDbu {
                x: 4 * MM_DBU,
                y: 10 * MM_DBU,
            },
            PointDbu {
                x: 16 * MM_DBU,
                y: 10 * MM_DBU,
            },
            Some(BoxDbu {
                min: PointDbu {
                    x: 8 * MM_DBU,
                    y: 0,
                },
                max: PointDbu {
                    x: 12 * MM_DBU,
                    y: 20 * MM_DBU,
                },
            }),
        );
        let blocked = parse_result(&runner.execute(&blocked).unwrap().result_json).unwrap();
        let RouteOutcome::Failure { status, .. } = blocked.outcome else {
            panic!("full-height obstacle unexpectedly admitted a route")
        };
        assert_eq!(status, RouteFailureStatus::RouteNotFound);
    }

    #[cfg(unix)]
    #[test]
    fn process_capture_preserves_exit_and_enforces_timeout_and_output_bound() {
        let root = test_root("process-capture");
        let directory = ScopedWorkDirectory::create(&root).unwrap();
        let large_input = vec![b'x'; 8 * 1024 * 1024];
        let exited = run_process(
            Path::new("/bin/sh"),
            directory.path(),
            &[OsString::from("-c"), OsString::from("exit 7")],
            &large_input,
            Duration::from_secs(2),
            128,
            128,
        )
        .unwrap();
        assert_eq!(exited.status.code(), Some(7));

        let timeout = run_process(
            Path::new("/bin/sh"),
            directory.path(),
            &[OsString::from("-c"), OsString::from("/bin/sleep 2")],
            b"",
            Duration::from_millis(50),
            128,
            128,
        )
        .unwrap_err();
        assert_eq!(timeout.code, PROCESS_RESOURCE);

        let overflow = run_process(
            Path::new("/bin/sh"),
            directory.path(),
            &[
                OsString::from("-c"),
                OsString::from("while :; do printf 12345678901234567890; done"),
            ],
            b"",
            Duration::from_secs(2),
            128,
            128,
        )
        .unwrap_err();
        assert_eq!(overflow.code, PROCESS_RESOURCE);
        directory.cleanup().unwrap();
        fs::remove_dir(root).unwrap();
    }

    #[cfg(unix)]
    #[test]
    fn full_runner_rejects_exit_malformed_output_and_provenance_drift() {
        let bundle = lower_request(&routing_design()).unwrap().unwrap();
        for (label, body, expected_code) in [
            ("nonzero", "exit 7", PROCESS_EXIT),
            (
                "malformed",
                "/bin/cat >/dev/null; printf not-json",
                PROCESS_OUTPUT,
            ),
        ] {
            let (runner, root) = fake_runner(label, body, false);
            let error = runner.execute(&bundle).unwrap_err();
            assert_eq!(error.code, expected_code);
            fs::remove_dir_all(root).unwrap();
        }

        let (runner, root) = fake_runner("provenance", "exit 99", true);
        let error = runner.execute(&bundle).unwrap_err();
        assert_eq!(error.code, PROCESS_IDENTITY);
        fs::remove_dir_all(root).unwrap();
    }

    #[cfg(unix)]
    fn fake_runner(label: &str, body: &str, corrupt_provenance: bool) -> (ApgarRunner, PathBuf) {
        let root = test_root(label);
        fs::create_dir_all(&root).unwrap();
        let executable = root.join("fake-apgar-route");
        let provenance = root.join("provenance.txt");
        fs::write(&executable, format!("#!/bin/sh\n{body}\n")).unwrap();
        fs::set_permissions(&executable, fs::Permissions::from_mode(0o700)).unwrap();
        let executable_sha256 = sha256_hex(&fs::read(&executable).unwrap());
        let mut provenance_text = format!(
            "{PROVENANCE_HEADER}\nname={APGAR_TOOL_NAME}\nversion={APGAR_TOOL_VERSION}\ncontract={APGAR_CONTRACT_IDENTITY}\nsource_revision={PINNED_APGAR_SOURCE_REVISION}\nexecutable_sha256={executable_sha256}\ndevice_class={APGAR_CPU_DEVICE_CLASS}\n"
        );
        if corrupt_provenance {
            provenance_text.push_str("unexpected=true\n");
        }
        fs::write(&provenance, provenance_text).unwrap();
        (
            ApgarRunner::from_paths(&executable, &provenance, &root),
            root,
        )
    }
}
