use std::collections::{BTreeMap, BTreeSet};
use std::env;
use std::ffi::OsStr;
use std::fs::{self, File, OpenOptions};
use std::io::{self, Read};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, ExitStatus, Stdio};
use std::thread;
use std::time::{Duration, Instant};

use circuitc::CompiledSimulation;
use circuitc::frontend::compile_source_checked;
use circuitc::simulation::{
    AnalysisKind, AxisKind, ExecutionStatus, ResultUnit, SignalKind, SpiceIdentityMap,
    canonical_f64, parse_report, parse_request, parse_result, parse_spice_identity_map,
};
use serde::Serialize;

const REQUIRED_NGSPICE_VERSION: &str = "45.2";
const VOLTAGE_ABSOLUTE_TOLERANCE: f64 = 1e-6;
const AXIS_RELATIVE_TOLERANCE: f64 = 1e-12;
const MAX_SOURCE_BYTES: u64 = 1 << 20;
const MAX_EXECUTABLE_BYTES: u64 = 256 << 20;
const MAX_RAW_BYTES: u64 = 16 << 20;
const MAX_LOG_BYTES: u64 = 64 << 10;
const MAX_RAW_VARIABLES: usize = 4_096;
const MAX_RAW_POINTS: usize = 100_000;
const MAX_RAW_CELLS: usize = 1_000_000;
const MAX_RAW_LINES: usize = MAX_RAW_CELLS + MAX_RAW_VARIABLES + 64;
const PROCESS_WALL_LIMIT: Duration = Duration::from_secs(5);
const PROCESS_POLL_INTERVAL: Duration = Duration::from_millis(20);

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum RawFlags {
    Real,
    Complex,
}

#[derive(Clone, Copy, Debug, PartialEq)]
enum RawValue {
    Real(f64),
    Complex(f64, f64),
}

impl RawValue {
    fn axis_value(self) -> f64 {
        match self {
            Self::Real(value) | Self::Complex(value, _) => value,
        }
    }

    fn real(self) -> Result<f64, String> {
        match self {
            Self::Real(value) => Ok(value),
            Self::Complex(_, _) => Err("expected a real ngspice value".to_owned()),
        }
    }

    fn magnitude(self) -> Result<f64, String> {
        match self {
            Self::Complex(real, imaginary) => Ok(real.hypot(imaginary)),
            Self::Real(_) => Err("expected a complex ngspice value".to_owned()),
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
struct RawVariable {
    name: String,
    kind: String,
}

#[derive(Clone, Debug, PartialEq)]
struct RawPlot {
    plot_name: String,
    flags: RawFlags,
    variables: Vec<RawVariable>,
    points: Vec<Vec<RawValue>>,
}

#[derive(Clone, Debug, Serialize)]
struct DifferentialTolerance {
    name: &'static str,
    absolute_volts: String,
    relative: String,
    axis_relative: String,
}

#[derive(Clone, Debug, Serialize)]
struct DifferentialComparison {
    analysis_path: String,
    analysis_kind: AnalysisKind,
    canonical_identity: String,
    signal_kind: SignalKind,
    sample_kind: AxisKind,
    sample: String,
    ohmnivore_value: String,
    ngspice_value: String,
    absolute_delta: String,
    allowed_delta: String,
    status: &'static str,
}

#[derive(Clone, Debug, Serialize)]
struct DifferentialSummary {
    pass: usize,
    fail: usize,
}

#[derive(Clone, Debug, Serialize)]
struct DifferentialReport {
    format: &'static str,
    ngspice_version: &'static str,
    tolerance: DifferentialTolerance,
    comparisons: Vec<DifferentialComparison>,
    summary: DifferentialSummary,
}

#[derive(Clone, Copy)]
struct ComparisonContext<'a> {
    analysis_path: &'a str,
    analysis_kind: AnalysisKind,
    canonical_identity: &'a str,
    signal_kind: SignalKind,
    sample_kind: AxisKind,
}

impl DifferentialReport {
    fn canonical_json(&self) -> Result<String, String> {
        let mut json = serde_json::to_string_pretty(self)
            .map_err(|_| "could not serialize differential evidence".to_owned())?;
        json.push('\n');
        Ok(json)
    }
}

struct ScratchDirectory(PathBuf);

impl ScratchDirectory {
    fn create(label: &str) -> Result<Self, String> {
        let base = env::var_os("TEST_TMPDIR")
            .map(PathBuf::from)
            .unwrap_or_else(env::temp_dir);
        let path = base.join(format!("circuitc-{label}-{}", std::process::id()));
        fs::create_dir(&path)
            .map_err(|_| "could not create the private differential work directory".to_owned())?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            fs::set_permissions(&path, fs::Permissions::from_mode(0o700)).map_err(|_| {
                "could not restrict the private differential work directory".to_owned()
            })?;
        }
        Ok(Self(path))
    }

    fn path(&self) -> &Path {
        &self.0
    }
}

impl Drop for ScratchDirectory {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

fn main() {
    match run(env::args_os().skip(1).collect()) {
        Ok(report) => {
            let failed = report.summary.fail != 0;
            match report.canonical_json() {
                Ok(json) => print!("{json}"),
                Err(error) => {
                    eprintln!("ngspice 45.2 differential gate failed: {error}");
                    std::process::exit(1);
                }
            }
            if failed {
                std::process::exit(1);
            }
        }
        Err(error) => {
            eprintln!("ngspice 45.2 differential gate failed: {error}");
            std::process::exit(1);
        }
    }
}

fn run(arguments: Vec<std::ffi::OsString>) -> Result<DifferentialReport, String> {
    if arguments.len() != 2 {
        return Err("usage: ngspice_differential_gate NGSPICE FIXTURE".to_owned());
    }
    let selected_executable = PathBuf::from(&arguments[0]);
    let fixture = PathBuf::from(&arguments[1]);
    let scratch = ScratchDirectory::create("ngspice-differential")?;
    let executable = pin_executable(&selected_executable, scratch.path())?;
    verify_ngspice_version(&executable, scratch.path())?;
    let simulations = compile_fixture(&fixture, scratch.path())?;
    compare_simulations(&executable, &simulations, scratch.path())
}

fn pin_executable(path: &Path, scratch: &Path) -> Result<PathBuf, String> {
    if !path.is_absolute() {
        return Err(
            "host gate unavailable: the selected ngspice executable path is not absolute"
                .to_owned(),
        );
    }
    let mut source = File::open(path).map_err(|_| {
        "host gate unavailable: the selected ngspice executable does not exist".to_owned()
    })?;
    let metadata = source.metadata().map_err(|_| {
        "host gate unavailable: the selected ngspice executable cannot be inspected".to_owned()
    })?;
    if !metadata.is_file() {
        return Err(
            "host gate unavailable: the selected ngspice executable is not a regular file"
                .to_owned(),
        );
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        if metadata.permissions().mode() & 0o111 == 0 {
            return Err(
                "host gate unavailable: the selected ngspice file is not executable".to_owned(),
            );
        }
    }
    if metadata.len() == 0 || metadata.len() > MAX_EXECUTABLE_BYTES {
        return Err(
            "host gate unavailable: the selected ngspice executable exceeds the 256 MiB bound"
                .to_owned(),
        );
    }
    let pinned_path = scratch.join("ngspice-pinned");
    let mut pinned = OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&pinned_path)
        .map_err(|_| "could not create the private pinned ngspice executable".to_owned())?;
    let copied = copy_bounded(&mut source, &mut pinned, MAX_EXECUTABLE_BYTES)
        .map_err(|_| "could not pin the selected ngspice executable".to_owned())?
        .ok_or_else(|| "selected ngspice executable grew beyond the 256 MiB bound".to_owned())?;
    if copied != metadata.len()
        || source
            .metadata()
            .map_err(|_| "could not recheck the selected ngspice executable".to_owned())?
            .len()
            != metadata.len()
    {
        return Err("selected ngspice executable changed while it was being pinned".to_owned());
    }
    pinned
        .sync_all()
        .map_err(|_| "could not synchronize the pinned ngspice executable".to_owned())?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(&pinned_path, fs::Permissions::from_mode(0o500))
            .map_err(|_| "could not make the pinned ngspice copy executable".to_owned())?;
    }
    Ok(pinned_path)
}

fn copy_bounded(
    source: &mut impl Read,
    destination: &mut impl io::Write,
    limit: u64,
) -> io::Result<Option<u64>> {
    let probe_limit = limit
        .checked_add(1)
        .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidInput, "copy limit overflowed"))?;
    let copied = io::copy(&mut source.take(probe_limit), destination)?;
    Ok((copied <= limit).then_some(copied))
}

fn verify_ngspice_version(executable: &Path, scratch: &Path) -> Result<(), String> {
    let version_directory = scratch.join("version");
    create_private_directory(&version_directory)?;
    let outcome = run_bounded_process(
        executable,
        [OsStr::new("-n"), OsStr::new("--version")],
        &version_directory,
        "version.stdout",
        "version.stderr",
    )?;
    if !outcome.status.success() {
        return Err("ngspice version handshake returned a nonzero status".to_owned());
    }
    if !outcome.stderr.is_empty() {
        return Err("ngspice version handshake wrote to standard error".to_owned());
    }
    let version = parse_ngspice_version(&outcome.stdout)?;
    if version != REQUIRED_NGSPICE_VERSION {
        return Err(format!(
            "host gate requires ngspice {REQUIRED_NGSPICE_VERSION}; found {version}"
        ));
    }
    Ok(())
}

fn parse_ngspice_version(output: &[u8]) -> Result<&str, String> {
    let text = std::str::from_utf8(output)
        .map_err(|_| "ngspice version output is not UTF-8".to_owned())?;
    let versions: Vec<_> = text
        .lines()
        .filter_map(|line| {
            let remainder = line.trim().strip_prefix("** ngspice-")?;
            let (version, description) = remainder.split_once(" :")?;
            (!version.is_empty() && !description.is_empty()).then_some(version)
        })
        .collect();
    if versions.len() != 1 {
        return Err("ngspice version output must contain exactly one version banner".to_owned());
    }
    Ok(versions[0])
}

fn compile_fixture(fixture: &Path, scratch: &Path) -> Result<Vec<CompiledSimulation>, String> {
    let metadata = fs::metadata(fixture)
        .map_err(|_| "could not inspect the differential source fixture".to_owned())?;
    if !metadata.is_file() || metadata.len() > MAX_SOURCE_BYTES {
        return Err("differential source fixture exceeds its bounded file contract".to_owned());
    }
    let source = fs::read_to_string(fixture)
        .map_err(|_| "could not read the differential source fixture as UTF-8".to_owned())?;
    let work_root = scratch.join("ohmnivore");
    create_private_directory(&work_root)?;
    let checked = compile_source_checked("ngspice_differential.circuitc", source, &work_root)
        .map_err(|_| "the checked CircuitC fixture did not compile successfully".to_owned())?;
    Ok(checked.artifacts.into_simulations())
}

fn compare_simulations(
    executable: &Path,
    simulations: &[CompiledSimulation],
    scratch: &Path,
) -> Result<DifferentialReport, String> {
    let mut by_kind = BTreeMap::new();
    for simulation in simulations {
        let request = parse_request(&simulation.request_json)
            .map_err(|error| format!("request authentication failed: {error}"))?;
        if by_kind.insert(request.analysis.kind, simulation).is_some() {
            return Err("differential fixture has a duplicate analysis kind".to_owned());
        }
    }
    let expected_kinds = BTreeSet::from([
        AnalysisKind::DcOperatingPoint,
        AnalysisKind::AcLinearSweep,
        AnalysisKind::Transient,
    ]);
    if by_kind.keys().copied().collect::<BTreeSet<_>>() != expected_kinds {
        return Err(
            "differential fixture must contain exactly one DC, AC, and transient analysis"
                .to_owned(),
        );
    }

    let mut comparisons = Vec::new();
    for (ordinal, kind) in expected_kinds.into_iter().enumerate() {
        let simulation = by_kind[&kind];
        let (request, map, result) = authenticate_simulation(simulation)?;
        require_supported_device_coverage(&simulation.netlist, kind)?;
        let analysis_directory = scratch.join(format!("analysis-{ordinal}"));
        let raw = execute_ngspice(executable, &simulation.netlist, kind, &analysis_directory)?;
        compare_analysis(&request, &map, &result, &raw, &mut comparisons)?;
    }
    comparisons.sort_by(|left, right| {
        left.analysis_path
            .cmp(&right.analysis_path)
            .then(left.canonical_identity.cmp(&right.canonical_identity))
            .then(left.sample.cmp(&right.sample))
    });
    let pass = comparisons
        .iter()
        .filter(|comparison| comparison.status == "pass")
        .count();
    let fail = comparisons.len() - pass;
    Ok(DifferentialReport {
        format: "circuitc-ngspice-differential/v1",
        ngspice_version: REQUIRED_NGSPICE_VERSION,
        tolerance: DifferentialTolerance {
            name: "inclusive_absolute_voltage",
            absolute_volts: canonical(VOLTAGE_ABSOLUTE_TOLERANCE)?,
            relative: canonical(0.0)?,
            axis_relative: canonical(AXIS_RELATIVE_TOLERANCE)?,
        },
        comparisons,
        summary: DifferentialSummary { pass, fail },
    })
}

fn authenticate_simulation(
    simulation: &CompiledSimulation,
) -> Result<
    (
        circuitc::simulation::SimulationRequest,
        SpiceIdentityMap,
        circuitc::simulation::SimulationResult,
    ),
    String,
> {
    let request = parse_request(&simulation.request_json)
        .map_err(|error| format!("request authentication failed: {error}"))?;
    request
        .verify_netlist_bytes(simulation.netlist.as_bytes())
        .map_err(|error| format!("netlist authentication failed: {error}"))?;
    let map = parse_spice_identity_map(&simulation.spice_identity_map_json)
        .map_err(|error| format!("map authentication failed: {error}"))?;
    let bound_request = map
        .verify_request_bytes(simulation.request_json.as_bytes())
        .map_err(|error| format!("map authentication failed: {error}"))?;
    if request != bound_request {
        return Err("map returned a different authenticated request".to_owned());
    }
    let result = parse_result(&simulation.result_json)
        .map_err(|error| format!("result authentication failed: {error}"))?;
    let (result_request, result_map) = result
        .verify_binding_bytes(
            simulation.request_json.as_bytes(),
            simulation.spice_identity_map_json.as_bytes(),
        )
        .map_err(|error| format!("result authentication failed: {error}"))?;
    if request != result_request || map != result_map || result.status != ExecutionStatus::Completed
    {
        return Err(
            "normalized result does not match its authenticated completed chain".to_owned(),
        );
    }
    let report = parse_report(&simulation.report_json)
        .map_err(|error| format!("report authentication failed: {error}"))?;
    report
        .verify_binding_bytes(
            simulation.request_json.as_bytes(),
            simulation.spice_identity_map_json.as_bytes(),
            simulation.result_json.as_bytes(),
        )
        .map_err(|error| format!("report authentication failed: {error}"))?;
    Ok((request, map, result))
}

fn require_supported_device_coverage(
    netlist: &str,
    analysis_kind: AnalysisKind,
) -> Result<(), String> {
    let mut resistor = false;
    let mut voltage_source = false;
    let mut nonzero_ac_source = false;
    for line in netlist.lines() {
        let fields: Vec<_> = line.split_ascii_whitespace().collect();
        let Some(name) = fields.first() else {
            continue;
        };
        if !name.starts_with('.') && !name.starts_with('*') {
            resistor |= name.starts_with('R') && fields.len() == 4;
            voltage_source |=
                name.starts_with('V') && fields.len() >= 5 && fields[3].eq_ignore_ascii_case("DC");
            if name.starts_with('V') {
                for ac in fields.windows(3) {
                    if ac[0].eq_ignore_ascii_case("AC") {
                        nonzero_ac_source |= parse_finite(ac[1]).is_ok_and(|value| value != 0.0);
                    }
                }
            }
        }
    }
    if resistor
        && voltage_source
        && (analysis_kind != AnalysisKind::AcLinearSweep || nonzero_ac_source)
    {
        Ok(())
    } else {
        Err("differential fixture must exercise resistor, independent voltage-source, and selected AC-excitation coverage"
            .to_owned())
    }
}

fn execute_ngspice(
    executable: &Path,
    netlist: &str,
    kind: AnalysisKind,
    directory: &Path,
) -> Result<RawPlot, String> {
    create_private_directory(directory)?;
    create_private_directory(&directory.join("home"))?;
    create_private_directory(&directory.join("tmp"))?;
    let instrumented = instrument_netlist(netlist, kind)?;
    fs::write(directory.join("input.spice"), instrumented)
        .map_err(|_| "could not materialize the private ngspice input".to_owned())?;
    let outcome = run_bounded_process(
        executable,
        [
            OsStr::new("-n"),
            OsStr::new("-b"),
            OsStr::new("input.spice"),
        ],
        directory,
        "ngspice.stdout",
        "ngspice.stderr",
    )?;
    if !outcome.status.success() {
        return Err("ngspice analysis returned a nonzero status".to_owned());
    }
    if !outcome.stderr.is_empty() {
        return Err("ngspice analysis wrote to standard error".to_owned());
    }
    let raw_path = directory.join("result.raw");
    let metadata = fs::symlink_metadata(&raw_path)
        .map_err(|_| "ngspice did not produce its expected raw result".to_owned())?;
    if !metadata.file_type().is_file() || metadata.len() > MAX_RAW_BYTES {
        return Err("ngspice raw result exceeds its bounded regular-file contract".to_owned());
    }
    let bytes =
        fs::read(raw_path).map_err(|_| "could not read the ngspice raw result".to_owned())?;
    parse_raw_plot(&bytes)
}

fn instrument_netlist(netlist: &str, kind: AnalysisKind) -> Result<String, String> {
    let mut end_count = 0;
    let mut output = String::with_capacity(netlist.len() + 128);
    for line in netlist.lines() {
        if line.trim().eq_ignore_ascii_case(".END") {
            end_count += 1;
            output.push_str(".control\nset filetype=ascii\nrun\n");
            if kind == AnalysisKind::Transient {
                output.push_str("linearize\n");
            }
            output.push_str("write result.raw\nquit\n.endc\n.END\n");
        } else {
            output.push_str(line);
            output.push('\n');
        }
    }
    if end_count != 1 {
        return Err("authenticated netlist must contain exactly one .END directive".to_owned());
    }
    Ok(output)
}

struct ProcessOutcome {
    status: ExitStatus,
    stdout: Vec<u8>,
    stderr: Vec<u8>,
}

fn run_bounded_process<'a>(
    executable: &Path,
    arguments: impl IntoIterator<Item = &'a OsStr>,
    directory: &Path,
    stdout_name: &str,
    stderr_name: &str,
) -> Result<ProcessOutcome, String> {
    let stdout_path = directory.join(stdout_name);
    let stderr_path = directory.join(stderr_name);
    let stdout_file = File::create(&stdout_path)
        .map_err(|_| "could not create bounded ngspice standard output".to_owned())?;
    let stderr_file = File::create(&stderr_path)
        .map_err(|_| "could not create bounded ngspice standard error".to_owned())?;
    let mut command = Command::new(executable);
    command
        .args(arguments)
        .current_dir(directory)
        .env_clear()
        .env("HOME", directory.join("home"))
        .env("TMPDIR", directory.join("tmp"))
        .env("LANG", "C")
        .env("LC_ALL", "C")
        .env("TZ", "UTC")
        .env("OMP_NUM_THREADS", "1")
        .env("OPENBLAS_NUM_THREADS", "1")
        .stdin(Stdio::null())
        .stdout(Stdio::from(stdout_file))
        .stderr(Stdio::from(stderr_file));
    configure_unix_process(&mut command);
    let mut child = command
        .spawn()
        .map_err(|_| "could not launch the selected ngspice executable".to_owned())?;
    let deadline = Instant::now()
        .checked_add(PROCESS_WALL_LIMIT)
        .unwrap_or_else(Instant::now);
    let status = loop {
        match child.try_wait() {
            Ok(Some(status)) => break status,
            Ok(None) if Instant::now() < deadline => thread::sleep(PROCESS_POLL_INTERVAL),
            Ok(None) => {
                terminate(&mut child);
                return Err("ngspice exceeded the 5 second wall-clock limit".to_owned());
            }
            Err(_) => {
                terminate(&mut child);
                return Err("could not observe the ngspice process status".to_owned());
            }
        }
    };
    let stdout = read_bounded_file(&stdout_path, MAX_LOG_BYTES, "standard output")?;
    let stderr = read_bounded_file(&stderr_path, MAX_LOG_BYTES, "standard error")?;
    Ok(ProcessOutcome {
        status,
        stdout,
        stderr,
    })
}

fn read_bounded_file(path: &Path, limit: u64, label: &str) -> Result<Vec<u8>, String> {
    let metadata =
        fs::symlink_metadata(path).map_err(|_| format!("could not inspect ngspice {label}"))?;
    if !metadata.file_type().is_file() || metadata.len() > limit {
        return Err(format!(
            "ngspice {label} exceeded its bounded file contract"
        ));
    }
    fs::read(path).map_err(|_| format!("could not read ngspice {label}"))
}

fn create_private_directory(path: &Path) -> Result<(), String> {
    fs::create_dir(path)
        .map_err(|_| "could not create a private host-gate directory".to_owned())?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(path, fs::Permissions::from_mode(0o700))
            .map_err(|_| "could not restrict a private host-gate directory".to_owned())?;
    }
    Ok(())
}

fn terminate(child: &mut Child) {
    #[cfg(unix)]
    // SAFETY: the child PID came from `spawn`; the negative PID targets only
    // the process group created for this child. The leader is then reaped.
    unsafe {
        let _ = libc::kill(-(child.id() as libc::pid_t), libc::SIGKILL);
    }
    let _ = child.kill();
    let _ = child.wait();
}

#[cfg(unix)]
fn configure_unix_process(command: &mut Command) {
    use std::os::unix::process::CommandExt;

    command.process_group(0);
    // SAFETY: `pre_exec` is restricted to async-signal-safe `setrlimit` calls
    // over captured integers; it performs no allocation between fork and exec.
    unsafe {
        command.pre_exec(|| {
            set_limit(libc::RLIMIT_CPU, 6)?;
            set_limit(libc::RLIMIT_FSIZE, MAX_RAW_BYTES)?;
            set_limit(libc::RLIMIT_NOFILE, 64)?;
            set_limit(libc::RLIMIT_CORE, 0)?;
            #[cfg(target_os = "linux")]
            set_limit(libc::RLIMIT_AS, 2 << 30)?;
            Ok(())
        });
    }
}

#[cfg(not(unix))]
fn configure_unix_process(_command: &mut Command) {}

#[cfg(all(unix, target_os = "linux"))]
type RlimitResource = libc::__rlimit_resource_t;

#[cfg(all(unix, not(target_os = "linux")))]
type RlimitResource = libc::c_int;

#[cfg(unix)]
fn set_limit(resource: RlimitResource, value: u64) -> io::Result<()> {
    let limit = libc::rlimit {
        rlim_cur: value as libc::rlim_t,
        rlim_max: value as libc::rlim_t,
    };
    // SAFETY: `limit` is initialized and `resource` is a platform RLIMIT value.
    if unsafe { libc::setrlimit(resource, &limit) } == 0 {
        Ok(())
    } else {
        Err(io::Error::last_os_error())
    }
}

fn parse_raw_plot(bytes: &[u8]) -> Result<RawPlot, String> {
    if bytes.len() as u64 > MAX_RAW_BYTES {
        return Err("ngspice raw result exceeds the parser limit".to_owned());
    }
    let text = std::str::from_utf8(bytes)
        .map_err(|_| "ngspice raw result is not ASCII/UTF-8".to_owned())?;
    let lines = bounded_raw_lines(text)?;
    let variables_marker = unique_header_index(&lines, "Variables:")?;
    let values_marker = unique_header_index(&lines, "Values:")?;
    if values_marker <= variables_marker {
        return Err("ngspice raw sections are out of order".to_owned());
    }
    let plot_name = unique_header_value(&lines[..variables_marker], "Plotname:")?;
    let flags = match unique_header_value(&lines[..variables_marker], "Flags:")? {
        "real" => RawFlags::Real,
        "complex" => RawFlags::Complex,
        _ => return Err("ngspice raw result has unsupported flags".to_owned()),
    };
    let variable_count = parse_bounded_count(
        unique_header_value(&lines[..variables_marker], "No. Variables:")?,
        MAX_RAW_VARIABLES,
        "variable",
    )?;
    let point_count = parse_bounded_count(
        unique_header_value(&lines[..variables_marker], "No. Points:")?,
        MAX_RAW_POINTS,
        "point",
    )?;
    let cell_count = point_count
        .checked_mul(variable_count)
        .ok_or_else(|| "ngspice raw cell count overflowed".to_owned())?;
    if cell_count > MAX_RAW_CELLS {
        return Err("ngspice raw result exceeds the cell allocation bound".to_owned());
    }
    let variable_lines: Vec<_> = lines[variables_marker + 1..values_marker]
        .iter()
        .filter(|line| !line.trim().is_empty())
        .collect();
    if variable_lines.len() != variable_count {
        return Err("ngspice raw variable count does not match its table".to_owned());
    }
    let mut variables = Vec::with_capacity(variable_count);
    let mut variable_names = BTreeSet::new();
    for (index, line) in variable_lines.into_iter().enumerate() {
        let fields: Vec<_> = line.split_ascii_whitespace().collect();
        if fields.len() != 3 || fields[0].parse::<usize>().ok() != Some(index) {
            return Err("ngspice raw variable table is malformed or noncontiguous".to_owned());
        }
        let name = fields[1].to_ascii_lowercase();
        if !variable_names.insert(name.clone()) {
            return Err("ngspice raw variable names must be unique".to_owned());
        }
        variables.push(RawVariable {
            name,
            kind: fields[2].to_ascii_lowercase(),
        });
    }
    let value_lines: Vec<_> = lines[values_marker + 1..]
        .iter()
        .map(|line| line.trim())
        .filter(|line| !line.is_empty())
        .collect();
    let expected_value_lines = cell_count;
    if value_lines.len() != expected_value_lines {
        return Err("ngspice raw point count does not match its value table".to_owned());
    }
    let mut points = Vec::with_capacity(point_count);
    for point_index in 0..point_count {
        let mut point = Vec::with_capacity(variable_count);
        for variable_index in 0..variable_count {
            let line = value_lines[point_index * variable_count + variable_index];
            let fields: Vec<_> = line.split_ascii_whitespace().collect();
            let value_text = if variable_index == 0 {
                if fields.len() != 2 || fields[0].parse::<usize>().ok() != Some(point_index) {
                    return Err("ngspice raw point indices must be contiguous".to_owned());
                }
                fields[1]
            } else {
                if fields.len() != 1 {
                    return Err("ngspice raw value row has the wrong width".to_owned());
                }
                fields[0]
            };
            point.push(parse_raw_value(value_text, flags)?);
        }
        points.push(point);
    }
    Ok(RawPlot {
        plot_name: plot_name.to_owned(),
        flags,
        variables,
        points,
    })
}

fn bounded_raw_lines(text: &str) -> Result<Vec<&str>, String> {
    let mut lines = Vec::new();
    for line in text.lines() {
        if lines.len() == MAX_RAW_LINES {
            return Err("ngspice raw result exceeds the line-count allocation bound".to_owned());
        }
        lines.push(line);
    }
    Ok(lines)
}

fn unique_header_index(lines: &[&str], header: &str) -> Result<usize, String> {
    let indices: Vec<_> = lines
        .iter()
        .enumerate()
        .filter_map(|(index, line)| (line.trim() == header).then_some(index))
        .collect();
    if indices.len() != 1 {
        return Err(format!(
            "ngspice raw result must contain exactly one {header}"
        ));
    }
    Ok(indices[0])
}

fn unique_header_value<'a>(lines: &'a [&str], header: &str) -> Result<&'a str, String> {
    let values: Vec<_> = lines
        .iter()
        .filter_map(|line| line.trim().strip_prefix(header).map(str::trim))
        .collect();
    if values.len() != 1 || values[0].is_empty() {
        return Err(format!(
            "ngspice raw result must contain exactly one {header}"
        ));
    }
    Ok(values[0])
}

fn parse_bounded_count(value: &str, maximum: usize, label: &str) -> Result<usize, String> {
    let count = value
        .parse::<usize>()
        .map_err(|_| format!("ngspice raw {label} count is malformed"))?;
    if count == 0 || count > maximum {
        return Err(format!("ngspice raw {label} count exceeds its bound"));
    }
    Ok(count)
}

fn parse_raw_value(value: &str, flags: RawFlags) -> Result<RawValue, String> {
    match flags {
        RawFlags::Real => Ok(RawValue::Real(parse_finite(value)?)),
        RawFlags::Complex => {
            let mut parts = value.split(',');
            let real = parts
                .next()
                .ok_or_else(|| "ngspice complex value is malformed".to_owned())?;
            let imaginary = parts
                .next()
                .ok_or_else(|| "ngspice complex value is malformed".to_owned())?;
            if parts.next().is_some() {
                return Err("ngspice complex value is malformed".to_owned());
            }
            Ok(RawValue::Complex(
                parse_finite(real)?,
                parse_finite(imaginary)?,
            ))
        }
    }
}

fn parse_finite(value: &str) -> Result<f64, String> {
    if value.is_empty()
        || !value
            .bytes()
            .all(|byte| byte.is_ascii_digit() || matches!(byte, b'+' | b'-' | b'.' | b'e' | b'E'))
    {
        return Err("ngspice numeric value has an unsupported spelling".to_owned());
    }
    let parsed = value
        .parse::<f64>()
        .map_err(|_| "ngspice numeric value is malformed".to_owned())?;
    if !parsed.is_finite() {
        return Err("ngspice numeric value must be finite".to_owned());
    }
    Ok(parsed)
}

fn compare_analysis(
    request: &circuitc::simulation::SimulationRequest,
    map: &SpiceIdentityMap,
    result: &circuitc::simulation::SimulationResult,
    raw: &RawPlot,
    comparisons: &mut Vec<DifferentialComparison>,
) -> Result<(), String> {
    let (expected_plot, expected_flags, expected_axis, expected_signal) =
        match request.analysis.kind {
            AnalysisKind::DcOperatingPoint => (
                "Operating Point",
                RawFlags::Real,
                AxisKind::Scalar,
                SignalKind::NetVoltage,
            ),
            AnalysisKind::AcLinearSweep => (
                "AC Analysis",
                RawFlags::Complex,
                AxisKind::FrequencyHertz,
                SignalKind::NetVoltageMagnitude,
            ),
            AnalysisKind::Transient => (
                "Transient Analysis (linearized)",
                RawFlags::Real,
                AxisKind::TimeSeconds,
                SignalKind::NetVoltage,
            ),
        };
    if raw.plot_name != expected_plot || raw.flags != expected_flags {
        return Err(
            "ngspice raw plot kind or flags do not match the requested analysis".to_owned(),
        );
    }
    if result.axis.kind != expected_axis {
        return Err("Ohmnivore result axis does not match the requested analysis".to_owned());
    }

    let non_ground: BTreeMap<_, _> = map
        .nets
        .iter()
        .filter(|net| !net.is_ground)
        .map(|net| (net.canonical.as_str(), net.backend.as_str()))
        .collect();
    if non_ground.is_empty() {
        return Err("differential fixture has no non-ground voltage identity".to_owned());
    }
    let ohmnivore_signals: BTreeMap<_, _> = result
        .signals
        .iter()
        .filter(|signal| signal.kind == expected_signal)
        .map(|signal| (signal.canonical_identity.as_str(), signal))
        .collect();
    if ohmnivore_signals.len() != non_ground.len()
        || !non_ground
            .keys()
            .all(|canonical| ohmnivore_signals.contains_key(canonical))
    {
        return Err(
            "Ohmnivore voltage inventory does not cover every mapped non-ground net".to_owned(),
        );
    }

    let raw_voltage_columns = raw_voltage_columns(raw, &non_ground)?;
    let result_axis = parse_axis(&result.axis.samples)?;
    let sample_indices = match request.analysis.kind {
        AnalysisKind::DcOperatingPoint => {
            if result_axis.as_slice() != [0.0] || raw.points.len() != 1 {
                return Err("DC differential evidence must contain one scalar sample".to_owned());
            }
            vec![0]
        }
        AnalysisKind::AcLinearSweep => {
            require_raw_axis(raw, "frequency", "frequency", &result_axis)?;
            (0..result_axis.len()).collect()
        }
        AnalysisKind::Transient => {
            require_raw_axis(raw, "time", "time", &result_axis)?;
            let declared: BTreeSet<_> = request
                .assertions
                .iter()
                .filter(|assertion| assertion.sample.kind == AxisKind::TimeSeconds)
                .map(|assertion| assertion.sample.value.as_str())
                .collect();
            let result_samples: BTreeSet<_> =
                result.axis.samples.iter().map(String::as_str).collect();
            if declared != result_samples {
                return Err(
                    "transient differential fixture must authenticate every comparison sample"
                        .to_owned(),
                );
            }
            (0..result_axis.len()).collect()
        }
    };

    for (canonical_identity, backend_identity) in &non_ground {
        let signal = ohmnivore_signals[canonical_identity];
        if signal.unit != ResultUnit::Volt || signal.values.len() != result_axis.len() {
            return Err("Ohmnivore voltage signal has the wrong unit or sample count".to_owned());
        }
        let raw_column = raw_voltage_columns[canonical_identity];
        let _ = backend_identity;
        for &sample_index in &sample_indices {
            let ohmnivore_value = parse_finite(&signal.values[sample_index])?;
            let ngspice_value = match request.analysis.kind {
                AnalysisKind::DcOperatingPoint | AnalysisKind::Transient => {
                    raw.points[sample_index][raw_column].real()?
                }
                AnalysisKind::AcLinearSweep => raw.points[sample_index][raw_column].magnitude()?,
            };
            comparisons.push(comparison(
                ComparisonContext {
                    analysis_path: &request.analysis.path,
                    analysis_kind: request.analysis.kind,
                    canonical_identity,
                    signal_kind: expected_signal,
                    sample_kind: expected_axis,
                },
                result_axis[sample_index],
                ohmnivore_value,
                ngspice_value,
            )?);
        }
    }
    Ok(())
}

fn raw_voltage_columns<'a>(
    raw: &RawPlot,
    non_ground: &BTreeMap<&'a str, &'a str>,
) -> Result<BTreeMap<&'a str, usize>, String> {
    let mut columns = BTreeMap::new();
    let mut observed_voltage_columns = 0;
    for (index, variable) in raw.variables.iter().enumerate() {
        if variable.kind != "voltage" {
            continue;
        }
        observed_voltage_columns += 1;
        let Some(backend) = variable
            .name
            .strip_prefix("v(")
            .and_then(|name| name.strip_suffix(')'))
        else {
            return Err("ngspice voltage variable has an unsupported identity spelling".to_owned());
        };
        let matches: Vec<_> = non_ground
            .iter()
            .filter_map(|(canonical, mapped)| {
                mapped.eq_ignore_ascii_case(backend).then_some(*canonical)
            })
            .collect();
        if matches.len() != 1 || columns.insert(matches[0], index).is_some() {
            return Err(
                "ngspice voltage identity does not join uniquely through the authenticated map"
                    .to_owned(),
            );
        }
    }
    if columns.len() != non_ground.len() || observed_voltage_columns != non_ground.len() {
        return Err(
            "ngspice voltage inventory does not exactly cover mapped non-ground nets".to_owned(),
        );
    }
    Ok(columns)
}

fn parse_axis(samples: &[String]) -> Result<Vec<f64>, String> {
    samples.iter().map(|sample| parse_finite(sample)).collect()
}

fn require_raw_axis(
    raw: &RawPlot,
    expected_name: &str,
    expected_kind: &str,
    result_axis: &[f64],
) -> Result<(), String> {
    let Some(variable) = raw.variables.first() else {
        return Err("ngspice raw plot has no axis variable".to_owned());
    };
    if variable.name != expected_name || variable.kind != expected_kind {
        return Err("ngspice raw axis has the wrong name or kind".to_owned());
    }
    if raw.points.len() != result_axis.len() {
        return Err("ngspice and Ohmnivore axes have different lengths".to_owned());
    }
    for (point, expected) in raw.points.iter().zip(result_axis) {
        let actual = point[0].axis_value();
        let tolerance = AXIS_RELATIVE_TOLERANCE * expected.abs().max(1.0);
        if (actual - expected).abs() > tolerance {
            return Err("ngspice and Ohmnivore axes do not align by declared ordinal".to_owned());
        }
    }
    Ok(())
}

fn comparison(
    context: ComparisonContext<'_>,
    sample: f64,
    ohmnivore_value: f64,
    ngspice_value: f64,
) -> Result<DifferentialComparison, String> {
    let delta = (ohmnivore_value - ngspice_value).abs();
    let allowed = VOLTAGE_ABSOLUTE_TOLERANCE;
    if !delta.is_finite() || !allowed.is_finite() {
        return Err("differential tolerance arithmetic must remain finite".to_owned());
    }
    Ok(DifferentialComparison {
        analysis_path: context.analysis_path.to_owned(),
        analysis_kind: context.analysis_kind,
        canonical_identity: context.canonical_identity.to_owned(),
        signal_kind: context.signal_kind,
        sample_kind: context.sample_kind,
        sample: canonical(sample)?,
        ohmnivore_value: canonical(ohmnivore_value)?,
        ngspice_value: canonical(ngspice_value)?,
        absolute_delta: canonical(delta)?,
        allowed_delta: canonical(allowed)?,
        status: if delta <= allowed { "pass" } else { "fail" },
    })
}

fn canonical(value: f64) -> Result<String, String> {
    canonical_f64(value).map_err(|_| "could not normalize a finite differential value".to_owned())
}

#[cfg(test)]
mod tests {
    use super::*;

    const REAL_RAW: &str = "Title: fixture\nDate: ignored\nCommand: ignored\nPlotname: Operating Point\nFlags: real\nNo. Variables: 2\nNo. Points: 1\nVariables:\n\t0\tv(vin)\tvoltage\n\t1\tv(vout)\tvoltage\nValues:\n0\t\t1.000000000000000e+01\n\t5.000000000000000e+00\n";
    const COMPLEX_RAW: &str = "Title: fixture\nPlotname: AC Analysis\nFlags: complex\nNo. Variables: 2\nNo. Points: 2\nVariables:\n\t0\tfrequency\tfrequency\n\t1\tv(vout)\tvoltage\nValues:\n0\t\t1.000000000000000e+00,4.000000000000000e-314\n\t5.000000000000000e-01,0.000000000000000e+00\n1\t\t2.000000000000000e+00,4.000000000000000e-314\n\t3.000000000000000e-01,4.000000000000000e-01\n";

    #[test]
    fn version_requires_one_exact_banner() {
        assert_eq!(
            parse_ngspice_version(b"******\n** ngspice-45.2 : Circuit level simulation program\n")
                .unwrap(),
            "45.2"
        );
        for invalid in [
            "** ngspice-45.20 : wrong\n",
            "ngspice 45.2\n",
            "** ngspice-45.2 : one\n** ngspice-45.2 : two\n",
            "prefix ** ngspice-45.2 : embedded\n",
        ] {
            let parsed = parse_ngspice_version(invalid.as_bytes());
            if invalid.contains("45.20") {
                assert_eq!(parsed.unwrap(), "45.20");
            } else {
                assert!(parsed.is_err(), "accepted {invalid:?}");
            }
        }
    }

    #[test]
    fn raw_parser_accepts_real_and_complex_ascii_plots() {
        let real = parse_raw_plot(REAL_RAW.as_bytes()).unwrap();
        assert_eq!(real.plot_name, "Operating Point");
        assert_eq!(real.points[0][1], RawValue::Real(5.0));

        let complex = parse_raw_plot(COMPLEX_RAW.as_bytes()).unwrap();
        assert_eq!(complex.plot_name, "AC Analysis");
        assert_eq!(complex.points[0][0].axis_value(), 1.0);
        assert_eq!(complex.points[1][1].magnitude().unwrap(), 0.5);
    }

    #[test]
    fn raw_parser_rejects_malformed_or_incomplete_evidence() {
        for invalid in [
            REAL_RAW.replace("No. Points: 1", "No. Points: 2"),
            REAL_RAW.replace("\t1\tv(vout)", "\t0\tv(vout)"),
            REAL_RAW.replace("v(vout)", "v(vin)"),
            REAL_RAW.replace("5.000000000000000e+00", "NaN"),
            format!("{REAL_RAW}0 1.0\n"),
            REAL_RAW.replace("Values:", "Values:\nValues:"),
        ] {
            assert!(parse_raw_plot(invalid.as_bytes()).is_err());
        }
    }

    #[test]
    fn ac_axis_ignores_irrelevant_imaginary_storage_but_checks_every_row() {
        let raw = parse_raw_plot(COMPLEX_RAW.as_bytes()).unwrap();
        require_raw_axis(&raw, "frequency", "frequency", &[1.0, 2.0]).unwrap();
        assert!(require_raw_axis(&raw, "frequency", "frequency", &[1.0, 3.0]).is_err());
        assert!(require_raw_axis(&raw, "frequency", "frequency", &[1.0]).is_err());
        let late_drift = COMPLEX_RAW.replace(
            "2.000000000000000e+00,4.000000000000000e-314",
            "2.000000000003000e+00,4.000000000000000e-314",
        );
        let late_drift = parse_raw_plot(late_drift.as_bytes()).unwrap();
        assert!(require_raw_axis(&late_drift, "frequency", "frequency", &[1.0, 2.0]).is_err());
    }

    #[test]
    fn tolerance_is_inclusive_and_late_sample_failures_remain_visible() {
        assert_eq!(
            canonical(VOLTAGE_ABSOLUTE_TOLERANCE).unwrap(),
            "9.99999999999999955e-7"
        );
        assert_eq!(
            canonical(AXIS_RELATIVE_TOLERANCE).unwrap(),
            "9.99999999999999980e-13"
        );
        let context = ComparisonContext {
            analysis_path: "simulation.ac",
            analysis_kind: AnalysisKind::AcLinearSweep,
            canonical_identity: "VOUT",
            signal_kind: SignalKind::NetVoltageMagnitude,
            sample_kind: AxisKind::FrequencyHertz,
        };
        let boundary = comparison(context, 1.0, VOLTAGE_ABSOLUTE_TOLERANCE, 0.0).unwrap();
        assert_eq!(boundary.status, "pass");
        let outside = comparison(
            context,
            4.0,
            f64::from_bits(VOLTAGE_ABSOLUTE_TOLERANCE.to_bits() + 1),
            0.0,
        )
        .unwrap();
        assert_eq!(outside.status, "fail");
    }

    #[test]
    fn identity_join_is_case_insensitive_only_at_the_backend_boundary() {
        let raw = parse_raw_plot(REAL_RAW.as_bytes()).unwrap();
        let mappings = BTreeMap::from([("input", "VIN"), ("output", "VOUT")]);
        let columns = raw_voltage_columns(&raw, &mappings).unwrap();
        assert_eq!(columns["input"], 0);
        assert_eq!(columns["output"], 1);

        let wrong = BTreeMap::from([("VIN", "unmapped"), ("VOUT", "VOUT")]);
        assert!(raw_voltage_columns(&raw, &wrong).is_err());
    }

    #[test]
    fn netlist_instrumentation_is_static_and_linearizes_only_transient() {
        let netlist = "V1 VIN 0 DC 10\nR1 VIN 0 1e3\n.OP\n.END\n";
        let dc = instrument_netlist(netlist, AnalysisKind::DcOperatingPoint).unwrap();
        assert_eq!(
            dc,
            "V1 VIN 0 DC 10\nR1 VIN 0 1e3\n.OP\n.control\nset filetype=ascii\nrun\nwrite result.raw\nquit\n.endc\n.END\n"
        );
        let transient = instrument_netlist(netlist, AnalysisKind::Transient).unwrap();
        assert_eq!(
            transient,
            "V1 VIN 0 DC 10\nR1 VIN 0 1e3\n.OP\n.control\nset filetype=ascii\nrun\nlinearize\nwrite result.raw\nquit\n.endc\n.END\n"
        );
        assert!(instrument_netlist(".END\n.END\n", AnalysisKind::Transient).is_err());
    }

    #[test]
    fn raw_parser_rejects_allocation_amplification_before_tables() {
        let amplified = REAL_RAW
            .replace("No. Variables: 2", "No. Variables: 4096")
            .replace("No. Points: 1", "No. Points: 100000");
        assert_eq!(
            parse_raw_plot(amplified.as_bytes()).unwrap_err(),
            "ngspice raw result exceeds the cell allocation bound"
        );
    }

    #[test]
    fn raw_line_bound_counts_an_unterminated_tail() {
        let maximum = "\n".repeat(MAX_RAW_LINES);
        assert_eq!(bounded_raw_lines(&maximum).unwrap().len(), MAX_RAW_LINES);
        let excess_with_tail = maximum + "tail";
        assert_eq!(
            bounded_raw_lines(&excess_with_tail).unwrap_err(),
            "ngspice raw result exceeds the line-count allocation bound"
        );
    }

    #[test]
    fn bounded_copy_stops_an_unbounded_reader_after_one_probe_byte() {
        let mut source = io::repeat(0xa5);
        let mut destination = Vec::new();
        assert_eq!(
            copy_bounded(&mut source, &mut destination, 8).unwrap(),
            None
        );
        assert_eq!(destination, vec![0xa5; 9]);
    }

    #[test]
    fn device_matrix_requires_both_supported_classes() {
        let complete = "V1 VIN 0 DC 10\nR1 VIN 0 1e3\n.OP\n.END\n";
        assert!(
            require_supported_device_coverage(complete, AnalysisKind::DcOperatingPoint).is_ok()
        );
        assert!(
            require_supported_device_coverage(
                "R1 VIN 0 1e3\n.OP\n.END\n",
                AnalysisKind::DcOperatingPoint
            )
            .is_err()
        );
        assert!(
            require_supported_device_coverage(
                "V1 VIN 0 DC 10\n.OP\n.END\n",
                AnalysisKind::DcOperatingPoint
            )
            .is_err()
        );
        assert!(require_supported_device_coverage(complete, AnalysisKind::AcLinearSweep).is_err());
        let ac = "V1 VIN 0 DC 10 AC 1 0\nR1 VIN 0 1e3\n.AC LIN 2 1 2\n.END\n";
        assert!(require_supported_device_coverage(ac, AnalysisKind::AcLinearSweep).is_ok());
    }
}
