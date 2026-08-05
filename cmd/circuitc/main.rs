use std::env;
use std::ffi::OsString;
use std::fs;
use std::io;
use std::path::{Component, Path, PathBuf};
use std::process::ExitCode;

use circuitc::CompiledArtifacts;
use circuitc::frontend::{DiagnosticFormat, compile_source_checked, render_diagnostics};

const EXIT_SOURCE: u8 = 1;
const EXIT_INVOCATION: u8 = 2;
const EXIT_IO: u8 = 3;
const USAGE: &str =
    "usage: circuitc compile INPUT --output-dir OUTPUT_DIRECTORY [--diagnostic-format=human|json]";

fn main() -> ExitCode {
    match run(env::args_os().skip(1).collect()) {
        Ok(()) => ExitCode::SUCCESS,
        Err(code) => ExitCode::from(code),
    }
}

fn run(arguments: Vec<OsString>) -> Result<(), u8> {
    let options = parse_arguments(arguments)?;
    let filename = options.input.to_string_lossy().into_owned();
    let input_path = resolve_bazel_run_input_path(
        &options.input,
        env::var_os("BUILD_WORKSPACE_DIRECTORY").as_deref(),
    );
    let output_directory = options.output_directory;
    let source = fs::read_to_string(&input_path).map_err(|error| {
        eprintln!(
            "CC-CLI-IO-001: failed to read {}: {error}",
            options.input.display()
        );
        EXIT_IO
    })?;
    let output_directory = validate_output_directory_path(&output_directory).map_err(|error| {
        eprintln!(
            "CC-CLI-IO-002: failed to write output directory {}: {error}",
            output_directory.display()
        );
        EXIT_IO
    })?;
    let work_root = CompileWorkRoot::create(&output_directory).map_err(|error| {
        if CompileWorkRoot::is_output_boundary_error(&error) {
            eprintln!(
                "CC-CLI-IO-002: failed to write output directory {}: {error}",
                output_directory.display()
            );
        } else {
            eprintln!("CC-CLI-IO-005: failed to create a private compiler work root: {error}");
        }
        EXIT_IO
    })?;
    let checked = compile_source_checked(filename, source, work_root.path());
    work_root.cleanup().map_err(|error| {
        eprintln!("CC-CLI-IO-005: failed to clean the private compiler work root: {error}");
        EXIT_IO
    })?;

    match checked {
        Ok(compiled) => {
            let mut outputs = Vec::new();
            append_static_outputs(
                &mut outputs,
                &compiled.elaborated.design.name,
                compiled.artifacts.static_artifacts(),
                &compiled.kicad_identity_map,
            );
            if let Some(routing) = compiled.artifacts.routing() {
                append_routing_output_chain(
                    &mut outputs,
                    [
                        (
                            routing.request_path.as_str(),
                            routing.request_json.as_bytes(),
                        ),
                        (routing.result_path.as_str(), routing.result_json.as_bytes()),
                        (
                            routing.projection_path.as_str(),
                            routing.projection_json.as_bytes(),
                        ),
                    ],
                );
            }
            for simulation in compiled.artifacts.simulations() {
                append_simulation_output_chain(
                    &mut outputs,
                    [
                        (
                            simulation.netlist_path.as_str(),
                            simulation.netlist.as_bytes(),
                        ),
                        (
                            simulation.request_path.as_str(),
                            simulation.request_json.as_bytes(),
                        ),
                        (
                            simulation.map_path.as_str(),
                            simulation.spice_identity_map_json.as_bytes(),
                        ),
                        (
                            simulation.result_path.as_str(),
                            simulation.result_json.as_bytes(),
                        ),
                        (
                            simulation.report_path.as_str(),
                            simulation.report_json.as_bytes(),
                        ),
                    ],
                );
            }
            publish_outputs(&output_directory, &mut outputs)
        }
        Err(error) => {
            report_source_diagnostics(&error.diagnostics, options.diagnostic_format);
            if error.simulations.is_empty() {
                return Err(EXIT_SOURCE);
            }

            let failure_directory = failure_output_directory(&output_directory).map_err(|error| {
                eprintln!(
                    "CC-CLI-IO-002: failed to derive checked-failure output directory from {}: {error}",
                    output_directory.display()
                );
                EXIT_IO
            })?;
            let mut outputs = Vec::new();
            for simulation in &error.simulations {
                append_simulation_output_chain(
                    &mut outputs,
                    [
                        (
                            simulation.netlist_path.as_str(),
                            simulation.netlist.as_bytes(),
                        ),
                        (
                            simulation.request_path.as_str(),
                            simulation.request_json.as_bytes(),
                        ),
                        (
                            simulation.map_path.as_str(),
                            simulation.spice_identity_map_json.as_bytes(),
                        ),
                        (
                            simulation.result_path.as_str(),
                            simulation.result_json.as_bytes(),
                        ),
                        (
                            simulation.report_path.as_str(),
                            simulation.report_json.as_bytes(),
                        ),
                    ],
                );
            }
            publish_outputs(&failure_directory, &mut outputs)?;
            Err(EXIT_SOURCE)
        }
    }
}

fn report_source_diagnostics(
    diagnostics: &[circuitc::frontend::SourceDiagnostic],
    format: DiagnosticFormat,
) {
    eprint!("{}", render_diagnostics(diagnostics, format));
    if format == DiagnosticFormat::Human {
        eprintln!();
    }
}

fn append_static_outputs<'a>(
    outputs: &mut Vec<(String, &'a [u8])>,
    stem: &str,
    artifacts: &'a CompiledArtifacts,
    kicad_identity_map: &'a str,
) {
    outputs.extend([
        (
            format!("{stem}.kicad_sch"),
            artifacts.kicad_schematic.as_bytes(),
        ),
        (format!("{stem}.kicad_pcb"), artifacts.kicad_pcb.as_bytes()),
        (
            format!("{stem}.kicad_pro"),
            artifacts.kicad_project.as_bytes(),
        ),
    ]);
    outputs.extend(artifacts.kicad_library_files.iter().map(|file| {
        (
            file.relative_path.as_str().to_owned(),
            file.contents.as_bytes(),
        )
    }));
    outputs.extend([
        (
            "sym-lib-table".to_owned(),
            artifacts.kicad_symbol_table.as_bytes(),
        ),
        (
            "fp-lib-table".to_owned(),
            artifacts.kicad_footprint_table.as_bytes(),
        ),
        (
            format!("{stem}.kicad-map.json"),
            kicad_identity_map.as_bytes(),
        ),
        (format!("{stem}.spice"), artifacts.spice.as_bytes()),
    ]);
}

fn append_simulation_output_chain<'a>(
    outputs: &mut Vec<(String, &'a [u8])>,
    chain: [(&str, &'a [u8]); 5],
) {
    outputs.extend(
        chain
            .into_iter()
            .map(|(path, contents)| (path.to_owned(), contents)),
    );
}

fn append_routing_output_chain<'a>(
    outputs: &mut Vec<(String, &'a [u8])>,
    chain: [(&str, &'a [u8]); 3],
) {
    outputs.extend(
        chain
            .into_iter()
            .map(|(path, contents)| (path.to_owned(), contents)),
    );
}

fn publish_outputs(output_directory: &Path, outputs: &mut Vec<(String, &[u8])>) -> Result<(), u8> {
    if let Err(error) = sort_outputs(outputs) {
        eprintln!(
            "CC-CLI-IO-002: failed to write output directory {}: {error}",
            output_directory.display(),
        );
        return Err(EXIT_IO);
    }
    let write_outcome = write_outputs(output_directory, outputs).map_err(|error| {
        eprintln!(
            "CC-CLI-IO-002: failed to write output directory {}: {error}",
            output_directory.display()
        );
        EXIT_IO
    })?;
    let mut stdout = io::stdout().lock();
    let mut stderr = io::stderr().lock();
    report_successful_publication(
        output_directory,
        outputs,
        &write_outcome,
        &mut stdout,
        &mut stderr,
    )
}

fn sort_outputs(outputs: &mut Vec<(String, &[u8])>) -> io::Result<()> {
    outputs.sort_by(|left, right| left.0.cmp(&right.0));
    if outputs.windows(2).any(|pair| pair[0].0 == pair[1].0) {
        Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "duplicate generated output path",
        ))
    } else {
        Ok(())
    }
}

fn failure_output_directory(output_directory: &Path) -> io::Result<PathBuf> {
    let normalized = absolute_lexical_path(output_directory)?;
    let file_name = normalized.file_name().ok_or_else(|| {
        io::Error::new(
            io::ErrorKind::InvalidInput,
            "checked output directory must have a terminal path component",
        )
    })?;
    let mut failed = file_name.to_os_string();
    failed.push(".failed");
    Ok(normalized.with_file_name(failed))
}

struct CompileWorkRoot {
    #[cfg(any(
        all(
            target_os = "linux",
            any(target_arch = "aarch64", target_arch = "x86_64")
        ),
        all(
            target_os = "macos",
            any(target_arch = "aarch64", target_arch = "x86_64")
        ),
    ))]
    inner: anchored_output::CompilerWorkRoot,
}

impl CompileWorkRoot {
    fn create(output_directory: &Path) -> io::Result<Self> {
        #[cfg(not(any(
            all(
                target_os = "linux",
                any(target_arch = "aarch64", target_arch = "x86_64")
            ),
            all(
                target_os = "macos",
                any(target_arch = "aarch64", target_arch = "x86_64")
            ),
        )))]
        {
            let _ = output_directory;
            return Err(io::Error::new(
                io::ErrorKind::Unsupported,
                "private compiler work roots are unavailable on this platform",
            ));
        }

        #[cfg(any(
            all(
                target_os = "linux",
                any(target_arch = "aarch64", target_arch = "x86_64")
            ),
            all(
                target_os = "macos",
                any(target_arch = "aarch64", target_arch = "x86_64")
            ),
        ))]
        {
            anchored_output::CompilerWorkRoot::create(output_directory).map(|inner| Self { inner })
        }
    }

    fn is_output_boundary_error(error: &io::Error) -> bool {
        #[cfg(any(
            all(
                target_os = "linux",
                any(target_arch = "aarch64", target_arch = "x86_64")
            ),
            all(
                target_os = "macos",
                any(target_arch = "aarch64", target_arch = "x86_64")
            ),
        ))]
        {
            anchored_output::compiler_work_root_error_is_output_boundary(error)
        }
        #[cfg(not(any(
            all(
                target_os = "linux",
                any(target_arch = "aarch64", target_arch = "x86_64")
            ),
            all(
                target_os = "macos",
                any(target_arch = "aarch64", target_arch = "x86_64")
            ),
        )))]
        {
            let _ = error;
            false
        }
    }

    #[cfg(any(
        all(
            target_os = "linux",
            any(target_arch = "aarch64", target_arch = "x86_64")
        ),
        all(
            target_os = "macos",
            any(target_arch = "aarch64", target_arch = "x86_64")
        ),
    ))]
    #[cfg(test)]
    fn create_in_parent(output_directory: &Path, work_parent: &Path) -> io::Result<Self> {
        anchored_output::CompilerWorkRoot::create_in_parent_for_test(output_directory, work_parent)
            .map(|inner| Self { inner })
    }

    fn path(&self) -> &Path {
        #[cfg(any(
            all(
                target_os = "linux",
                any(target_arch = "aarch64", target_arch = "x86_64")
            ),
            all(
                target_os = "macos",
                any(target_arch = "aarch64", target_arch = "x86_64")
            ),
        ))]
        {
            self.inner.path()
        }
        #[cfg(not(any(
            all(
                target_os = "linux",
                any(target_arch = "aarch64", target_arch = "x86_64")
            ),
            all(
                target_os = "macos",
                any(target_arch = "aarch64", target_arch = "x86_64")
            ),
        )))]
        {
            unreachable!("compiler work roots are unsupported on this platform")
        }
    }

    fn cleanup(self) -> io::Result<()> {
        #[cfg(any(
            all(
                target_os = "linux",
                any(target_arch = "aarch64", target_arch = "x86_64")
            ),
            all(
                target_os = "macos",
                any(target_arch = "aarch64", target_arch = "x86_64")
            ),
        ))]
        {
            self.inner.cleanup()
        }
        #[cfg(not(any(
            all(
                target_os = "linux",
                any(target_arch = "aarch64", target_arch = "x86_64")
            ),
            all(
                target_os = "macos",
                any(target_arch = "aarch64", target_arch = "x86_64")
            ),
        )))]
        {
            unreachable!("compiler work roots are unsupported on this platform")
        }
    }
}

fn absolute_lexical_path(path: &Path) -> io::Result<PathBuf> {
    let absolute = if path.is_absolute() {
        path.to_owned()
    } else {
        env::current_dir()?.join(path)
    };
    let mut normalized = PathBuf::new();
    for component in absolute.components() {
        match component {
            Component::Prefix(prefix) => normalized.push(prefix.as_os_str()),
            Component::RootDir => normalized.push(Path::new("/")),
            Component::CurDir => {}
            Component::ParentDir => {
                normalized.pop();
            }
            Component::Normal(segment) => normalized.push(segment),
        }
    }
    Ok(normalized)
}

fn report_successful_publication(
    output_directory: &Path,
    outputs: &[(String, &[u8])],
    write_outcome: &WriteOutcome,
    stdout: &mut impl io::Write,
    stderr: &mut impl io::Write,
) -> Result<(), u8> {
    let report_result = (|| -> io::Result<()> {
        if let Some(error) = &write_outcome.cleanup_warning {
            writeln!(
                stderr,
                "CC-CLI-IO-003: output publication to {} succeeded, but post-publication durability or cleanup was incomplete: {error}",
                output_directory.display()
            )?;
        }
        for (filename, _) in outputs {
            writeln!(
                stdout,
                "wrote {}",
                output_directory.join(filename).display()
            )?;
        }
        Ok(())
    })();
    match report_result {
        Ok(()) => Ok(()),
        Err(error) if error.kind() == io::ErrorKind::BrokenPipe => Ok(()),
        Err(error) => {
            let _ = writeln!(
                stderr,
                "CC-CLI-IO-004: outputs were published to {}, but reporting them failed: {error}",
                output_directory.display()
            );
            Err(EXIT_IO)
        }
    }
}

fn resolve_bazel_run_input_path(
    input: &Path,
    bazel_workspace: Option<&std::ffi::OsStr>,
) -> PathBuf {
    if input.is_relative()
        && let Some(workspace) = bazel_workspace
    {
        return PathBuf::from(workspace).join(input);
    }
    input.to_owned()
}

#[derive(Debug)]
struct Options {
    input: PathBuf,
    output_directory: PathBuf,
    diagnostic_format: DiagnosticFormat,
}

fn parse_arguments(arguments: Vec<OsString>) -> Result<Options, u8> {
    let mut arguments = arguments.into_iter();
    if arguments.next().as_deref() != Some(std::ffi::OsStr::new("compile")) {
        return invocation_error("expected the `compile` subcommand");
    }
    let mut input = None;
    let mut output_directory = None;
    let mut diagnostic_format = DiagnosticFormat::Human;
    let mut positional_only = false;
    while let Some(argument) = arguments.next() {
        if !positional_only && argument == "--" {
            positional_only = true;
        } else if !positional_only && argument == "--output-dir" {
            let Some(value) = arguments.next() else {
                return invocation_error("`--output-dir` requires a value");
            };
            if output_directory.replace(PathBuf::from(value)).is_some() {
                return invocation_error("`--output-dir` may be supplied only once");
            }
        } else if !positional_only && argument == "--diagnostic-format" {
            let Some(value) = arguments.next() else {
                return invocation_error("`--diagnostic-format` requires `human` or `json`");
            };
            diagnostic_format = parse_diagnostic_format(&value)?;
        } else if let Some(value) = (!positional_only)
            .then(|| argument.to_str())
            .flatten()
            .and_then(|argument| argument.strip_prefix("--diagnostic-format="))
        {
            diagnostic_format = parse_diagnostic_format(&OsString::from(value))?;
        } else if !positional_only && argument.to_string_lossy().starts_with('-') {
            return invocation_error(&format!(
                "unsupported option `{}`",
                argument.to_string_lossy()
            ));
        } else if input.replace(PathBuf::from(argument)).is_some() {
            return invocation_error("exactly one source input is supported");
        }
    }
    let Some(input) = input else {
        return invocation_error("missing source input");
    };
    let Some(output_directory) = output_directory else {
        return invocation_error("missing required `--output-dir`");
    };
    Ok(Options {
        input,
        output_directory,
        diagnostic_format,
    })
}

fn parse_diagnostic_format(value: &OsString) -> Result<DiagnosticFormat, u8> {
    match value.to_str() {
        Some("human") => Ok(DiagnosticFormat::Human),
        Some("json") => Ok(DiagnosticFormat::Json),
        _ => invocation_error("`--diagnostic-format` requires `human` or `json`"),
    }
}

fn invocation_error<T>(message: &str) -> Result<T, u8> {
    eprintln!("CC-CLI-ARGS-001: {message}\n{USAGE}");
    Err(EXIT_INVOCATION)
}

#[derive(Debug)]
struct WriteOutcome {
    cleanup_warning: Option<io::Error>,
}

fn write_outputs(
    output_directory: &Path,
    outputs: &[(String, &[u8])],
) -> std::io::Result<WriteOutcome> {
    #[cfg(any(
        all(
            target_os = "linux",
            any(target_arch = "aarch64", target_arch = "x86_64")
        ),
        all(
            target_os = "macos",
            any(target_arch = "aarch64", target_arch = "x86_64")
        ),
    ))]
    {
        anchored_output::write_outputs(output_directory, outputs)
    }
    #[cfg(not(any(
        all(
            target_os = "linux",
            any(target_arch = "aarch64", target_arch = "x86_64")
        ),
        all(
            target_os = "macos",
            any(target_arch = "aarch64", target_arch = "x86_64")
        ),
    )))]
    {
        let _ = (output_directory, outputs);
        Err(io::Error::new(
            io::ErrorKind::Unsupported,
            "secure descriptor-anchored output publication is unavailable on this platform",
        ))
    }
}

fn validate_output_directory_path(output_directory: &Path) -> io::Result<PathBuf> {
    #[cfg(any(
        all(
            target_os = "linux",
            any(target_arch = "aarch64", target_arch = "x86_64")
        ),
        all(
            target_os = "macos",
            any(target_arch = "aarch64", target_arch = "x86_64")
        ),
    ))]
    {
        anchored_output::validate_output_directory_path(output_directory)
    }
    #[cfg(not(any(
        all(
            target_os = "linux",
            any(target_arch = "aarch64", target_arch = "x86_64")
        ),
        all(
            target_os = "macos",
            any(target_arch = "aarch64", target_arch = "x86_64")
        ),
    )))]
    {
        let _ = output_directory;
        Err(io::Error::new(
            io::ErrorKind::Unsupported,
            "secure output-path validation is unavailable on this platform",
        ))
    }
}

fn validate_relative_output_path(filename: &str) -> io::Result<&Path> {
    let path = Path::new(filename);
    if path.as_os_str().is_empty()
        || path.is_absolute()
        || path
            .components()
            .any(|component| !matches!(component, Component::Normal(_)))
    {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            format!("generated output path {filename:?} is not a safe relative path"),
        ));
    }
    Ok(path)
}

#[cfg(any(
    all(
        target_os = "linux",
        any(target_arch = "aarch64", target_arch = "x86_64")
    ),
    all(
        target_os = "macos",
        any(target_arch = "aarch64", target_arch = "x86_64")
    ),
))]
mod anchored_output {
    use std::cell::RefCell;
    use std::collections::BTreeSet;
    use std::env;
    #[cfg(target_os = "macos")]
    use std::ffi::c_void;
    use std::ffi::{CStr, CString, OsStr, OsString};
    use std::fmt;
    #[cfg(test)]
    use std::fs;
    use std::fs::File;
    use std::io::{self, Read as _, Write as _};
    use std::os::fd::{AsRawFd as _, FromRawFd as _, RawFd};
    use std::os::raw::{c_char, c_int, c_uint};
    use std::os::unix::ffi::OsStrExt as _;
    use std::os::unix::fs::MetadataExt as _;
    use std::path::{Component, Path, PathBuf};
    use std::process;

    // CircuitC has no third-party Rust crate graph yet. Keep these bindings
    // limited to the Linux and Darwin ABIs exercised by CI and local release
    // gates; every other OS/architecture fails closed in `write_outputs`.
    #[cfg(target_os = "linux")]
    const O_CLOEXEC: c_int = 0x80000;
    #[cfg(target_os = "linux")]
    const O_CREAT: c_int = 0x40;
    #[cfg(all(target_os = "linux", target_arch = "aarch64"))]
    const O_DIRECTORY: c_int = 0x4000;
    #[cfg(all(target_os = "linux", target_arch = "x86_64"))]
    const O_DIRECTORY: c_int = 0x10000;
    #[cfg(target_os = "linux")]
    const O_EXCL: c_int = 0x80;
    #[cfg(all(target_os = "linux", target_arch = "aarch64"))]
    const O_NOFOLLOW: c_int = 0x8000;
    #[cfg(all(target_os = "linux", target_arch = "x86_64"))]
    const O_NOFOLLOW: c_int = 0x20000;
    #[cfg(target_os = "linux")]
    const O_NONBLOCK: c_int = 0x800;
    #[cfg(target_os = "linux")]
    const AT_REMOVEDIR: c_int = 0x200;
    #[cfg(target_os = "linux")]
    const RENAME_NOREPLACE: c_uint = 1;
    #[cfg(target_os = "linux")]
    const ENOSYS: c_int = 38;
    #[cfg(target_os = "linux")]
    const ENOTSUP_OR_EOPNOTSUPP: c_int = 95;
    #[cfg(target_os = "linux")]
    type Mode = c_uint;

    #[cfg(target_os = "macos")]
    const O_CLOEXEC: c_int = 0x01000000;
    #[cfg(target_os = "macos")]
    const O_CREAT: c_int = 0x00000200;
    #[cfg(target_os = "macos")]
    const O_DIRECTORY: c_int = 0x00100000;
    #[cfg(target_os = "macos")]
    const O_EXCL: c_int = 0x00000800;
    #[cfg(target_os = "macos")]
    const O_NOFOLLOW: c_int = 0x00000100;
    #[cfg(target_os = "macos")]
    const O_NONBLOCK: c_int = 0x00000004;
    #[cfg(target_os = "macos")]
    const AT_REMOVEDIR: c_int = 0x0080;
    #[cfg(target_os = "macos")]
    const RENAME_EXCL: c_uint = 0x00000004;
    #[cfg(target_os = "macos")]
    const ENOSYS: c_int = 78;
    #[cfg(target_os = "macos")]
    const ENOTSUP_OR_EOPNOTSUPP: c_int = 45;
    #[cfg(target_os = "macos")]
    const ACL_TYPE_EXTENDED: c_int = 0x0000_0100;
    #[cfg(target_os = "macos")]
    const ACL_FIRST_ENTRY: c_int = 0;
    #[cfg(target_os = "macos")]
    const ACL_NEXT_ENTRY: c_int = -1;
    #[cfg(target_os = "macos")]
    type Mode = u16;

    #[derive(Debug)]
    struct OutputDirectoryComponentFailure {
        segment: OsString,
        source: io::Error,
    }

    impl fmt::Display for OutputDirectoryComponentFailure {
        fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
            write!(
                formatter,
                "output directory component {} must be a non-symlinked directory: {}",
                Path::new(&self.segment).display(),
                self.source
            )
        }
    }

    impl std::error::Error for OutputDirectoryComponentFailure {
        fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
            Some(&self.source)
        }
    }

    #[derive(Debug)]
    struct CompilerOutputBoundaryFailure(io::Error);

    impl fmt::Display for CompilerOutputBoundaryFailure {
        fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
            self.0.fmt(formatter)
        }
    }

    impl std::error::Error for CompilerOutputBoundaryFailure {
        fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
            Some(&self.0)
        }
    }

    fn compiler_output_boundary_error(error: io::Error) -> io::Error {
        io::Error::new(error.kind(), CompilerOutputBoundaryFailure(error))
    }

    pub(super) fn compiler_work_root_error_is_output_boundary(error: &io::Error) -> bool {
        error
            .get_ref()
            .is_some_and(|source| source.is::<CompilerOutputBoundaryFailure>())
    }

    const O_RDONLY: c_int = 0;
    const O_WRONLY: c_int = 1;
    #[cfg(target_os = "macos")]
    const ENOENT: c_int = 2;
    const EACCES: c_int = 13;
    const EISDIR: c_int = 21;
    const EINVAL: c_int = 22;
    const ENOTDIR: c_int = 20;
    #[cfg(target_os = "linux")]
    const ELOOP: c_int = 40;
    #[cfg(target_os = "macos")]
    const ELOOP: c_int = 62;

    unsafe extern "C" {
        fn open(path: *const c_char, flags: c_int, ...) -> c_int;
        fn openat(directory: c_int, path: *const c_char, flags: c_int, ...) -> c_int;
        fn mkdirat(directory: c_int, path: *const c_char, mode: Mode) -> c_int;
        fn unlinkat(directory: c_int, path: *const c_char, flags: c_int) -> c_int;
    }

    #[cfg(target_os = "linux")]
    unsafe extern "C" {
        fn renameat2(
            old_directory: c_int,
            old_path: *const c_char,
            new_directory: c_int,
            new_path: *const c_char,
            flags: c_uint,
        ) -> c_int;
    }

    #[cfg(target_os = "macos")]
    unsafe extern "C" {
        fn renameatx_np(
            old_directory: c_int,
            old_path: *const c_char,
            new_directory: c_int,
            new_path: *const c_char,
            flags: c_uint,
        ) -> c_int;
        fn acl_get_fd_np(descriptor: c_int, acl_type: c_int) -> *mut c_void;
        fn acl_get_entry(acl: *mut c_void, entry_id: c_int, entry: *mut *mut c_void) -> c_int;
        fn acl_get_tag_type(entry: *mut c_void, tag_type: *mut c_int) -> c_int;
        fn acl_free(object: *mut c_void) -> c_int;
    }

    struct Directory(File);

    impl Directory {
        fn open_path(path: &Path) -> io::Result<Self> {
            let path = CString::new(path.as_os_str().as_bytes()).map_err(|_| {
                io::Error::new(
                    io::ErrorKind::InvalidInput,
                    "output path contains a NUL byte",
                )
            })?;
            // SAFETY: `path` is NUL-terminated and remains live for this call.
            let descriptor = unsafe {
                open(
                    path.as_ptr(),
                    O_RDONLY | O_DIRECTORY | O_NOFOLLOW | O_CLOEXEC,
                )
            };
            file_from_descriptor(descriptor).map(Self)
        }

        fn try_clone(&self) -> io::Result<Self> {
            self.0.try_clone().map(Self)
        }

        fn identity(&self) -> io::Result<(u64, u64)> {
            let metadata = self.0.metadata()?;
            Ok((metadata.dev(), metadata.ino()))
        }

        fn open_child(&self, name: &CStr) -> io::Result<Self> {
            // SAFETY: `name` is NUL-terminated and remains live for this call.
            let descriptor = unsafe {
                openat(
                    self.0.as_raw_fd(),
                    name.as_ptr(),
                    O_RDONLY | O_DIRECTORY | O_NOFOLLOW | O_CLOEXEC,
                )
            };
            file_from_descriptor(descriptor).map(Self)
        }

        fn open_entry(&self, name: &CStr) -> io::Result<File> {
            // O_NONBLOCK prevents a malformed FIFO target from blocking preflight.
            // SAFETY: `name` is NUL-terminated and remains live for this call.
            let descriptor = unsafe {
                openat(
                    self.0.as_raw_fd(),
                    name.as_ptr(),
                    O_RDONLY | O_NONBLOCK | O_NOFOLLOW | O_CLOEXEC,
                )
            };
            file_from_descriptor(descriptor)
        }

        fn open_entry_write_only(&self, name: &CStr) -> io::Result<File> {
            // This fallback permits type inspection of write-only regular
            // artifacts without granting publication through symlinks.
            // SAFETY: `name` is NUL-terminated and remains live for this call.
            let descriptor = unsafe {
                openat(
                    self.0.as_raw_fd(),
                    name.as_ptr(),
                    O_WRONLY | O_NONBLOCK | O_NOFOLLOW | O_CLOEXEC,
                )
            };
            file_from_descriptor(descriptor)
        }

        fn create_file(&self, name: &CStr) -> io::Result<File> {
            // SAFETY: `name` is NUL-terminated and remains live for this call. The
            // variadic mode argument is supplied because O_CREAT is set.
            #[cfg(target_os = "linux")]
            let descriptor = unsafe {
                openat(
                    self.0.as_raw_fd(),
                    name.as_ptr(),
                    O_WRONLY | O_CREAT | O_EXCL | O_NOFOLLOW | O_CLOEXEC,
                    0o666_u32,
                )
            };
            #[cfg(target_os = "macos")]
            let descriptor = unsafe {
                openat(
                    self.0.as_raw_fd(),
                    name.as_ptr(),
                    O_WRONLY | O_CREAT | O_EXCL | O_NOFOLLOW | O_CLOEXEC,
                    0o666_i32,
                )
            };
            file_from_descriptor(descriptor)
        }

        fn create_child(&self, name: &CStr) -> io::Result<bool> {
            self.create_child_with_mode(name, 0o700)
        }

        fn create_child_with_mode(&self, name: &CStr, mode: Mode) -> io::Result<bool> {
            // SAFETY: `name` is NUL-terminated and remains live for this call.
            let status = unsafe { mkdirat(self.0.as_raw_fd(), name.as_ptr(), mode) };
            if status == 0 {
                Ok(true)
            } else {
                let error = io::Error::last_os_error();
                if error.kind() == io::ErrorKind::AlreadyExists {
                    Ok(false)
                } else {
                    Err(error)
                }
            }
        }

        fn remove_file(&self, name: &CStr) -> io::Result<()> {
            self.unlink(name, 0)
        }

        fn remove_directory(&self, name: &CStr) -> io::Result<()> {
            self.unlink(name, AT_REMOVEDIR)
        }

        fn unlink(&self, name: &CStr, flags: c_int) -> io::Result<()> {
            // SAFETY: `name` is NUL-terminated and remains live for this call.
            let status = unsafe { unlinkat(self.0.as_raw_fd(), name.as_ptr(), flags) };
            status_result(status)
        }
    }

    struct CreatedDirectory {
        parent: Directory,
        name: CString,
        cleanup: CString,
        identity: Option<(u64, u64)>,
    }

    struct PrivateQuarantine {
        parent: Directory,
        name: CString,
        directory: Directory,
        identity: (u64, u64),
        cleanup_descriptor_reserve: RefCell<Vec<File>>,
    }

    struct OutputEntry<'a> {
        parent: Directory,
        target: CString,
        temporary: CString,
        backup: CString,
        rename_probe: CString,
        published_claim: CString,
        temporary_claim: CString,
        backup_claim: CString,
        rename_probe_claim: CString,
        display_path: String,
        contents: &'a [u8],
        had_existing_target: bool,
    }

    struct RetainedStagedFile {
        index: usize,
        identity: Option<(u64, u64)>,
        descriptor: File,
    }

    struct RetainedBackup {
        index: usize,
        identity: (u64, u64),
        _descriptor: File,
    }

    struct WriteHooks<F, P, S, G, H, D> {
        pre_anchor: F,
        before_rename_probe: P,
        sync_staging: S,
        before_publish: G,
        after_publication: H,
        sync_directory: D,
    }

    fn validate_directory_owner_and_mode(
        directory: &Directory,
        display_path: &str,
    ) -> io::Result<()> {
        let metadata = directory.0.metadata().map_err(|error| {
            operation_error("inspect mutable output directory", display_path, error)
        })?;
        // SAFETY: `geteuid` has no preconditions and does not dereference pointers.
        let effective_uid = unsafe { libc::geteuid() };
        if metadata.uid() != effective_uid && metadata.uid() != 0 {
            return Err(io::Error::new(
                io::ErrorKind::PermissionDenied,
                format!("mutable output directory {display_path} is not caller- or root-owned"),
            ));
        }
        let mode = metadata.mode();
        if mode & 0o022 != 0 && mode & 0o1000 == 0 {
            return Err(io::Error::new(
                io::ErrorKind::PermissionDenied,
                format!(
                    "mutable output directory {display_path} is writable by other users without the sticky bit"
                ),
            ));
        }
        Ok(())
    }

    fn validate_transaction_parent(directory: &Directory, display_path: &str) -> io::Result<()> {
        validate_directory_owner_and_mode(directory, display_path)?;
        reject_extended_acl(directory, display_path)?;
        Ok(())
    }

    fn validate_namespace_ancestor(directory: &Directory, display_path: &str) -> io::Result<()> {
        validate_directory_owner_and_mode(directory, display_path)?;
        reject_permissive_extended_acl(directory, display_path)?;
        Ok(())
    }

    #[cfg(target_os = "linux")]
    fn reject_extended_acl(_directory: &Directory, _display_path: &str) -> io::Result<()> {
        // Linux POSIX ACL grants are bounded by the group class mode bits,
        // which `validate_transaction_parent` checks above. Darwin NFSv4 ACLs
        // are independent of those bits and require the explicit check below.
        Ok(())
    }

    #[cfg(target_os = "linux")]
    fn reject_permissive_extended_acl(
        _directory: &Directory,
        _display_path: &str,
    ) -> io::Result<()> {
        Ok(())
    }

    #[cfg(target_os = "macos")]
    fn reject_extended_acl(directory: &Directory, display_path: &str) -> io::Result<()> {
        validate_darwin_extended_acl(directory, display_path, false)
    }

    #[cfg(target_os = "macos")]
    fn reject_permissive_extended_acl(directory: &Directory, display_path: &str) -> io::Result<()> {
        validate_darwin_extended_acl(directory, display_path, true)
    }

    #[cfg(target_os = "macos")]
    fn validate_darwin_extended_acl(
        directory: &Directory,
        display_path: &str,
        allow_deny_only: bool,
    ) -> io::Result<()> {
        // SAFETY: the descriptor is live and ACL_TYPE_EXTENDED is the Darwin
        // ACL type for the object referenced by that descriptor.
        let acl = unsafe { acl_get_fd_np(directory.0.as_raw_fd(), ACL_TYPE_EXTENDED) };
        if acl.is_null() {
            let error = io::Error::last_os_error();
            if error.raw_os_error() == Some(ENOENT) {
                return Ok(());
            }
            return Err(operation_error(
                "inspect extended ACL on mutable output directory",
                display_path,
                error,
            ));
        }
        let mut entry = std::ptr::null_mut();
        let mut entry_id = ACL_FIRST_ENTRY;
        let mut rejected_entry = false;
        let mut entry_error = None;
        loop {
            // SAFETY: `acl` is live and `entry` points to writable storage for
            // the borrowed entry pointer.
            let entry_status = unsafe { acl_get_entry(acl, entry_id, &mut entry) };
            if entry_status < 0 {
                let error = io::Error::last_os_error();
                if entry_id == ACL_NEXT_ENTRY && error.raw_os_error() == Some(EINVAL) {
                    break;
                }
                entry_error = Some(error);
                break;
            }
            if !allow_deny_only {
                rejected_entry = true;
                break;
            }
            let mut tag_type = 0;
            // SAFETY: `entry` is the live borrowed entry returned above and
            // `tag_type` points to writable storage.
            if unsafe { acl_get_tag_type(entry, &mut tag_type) } != 0 {
                entry_error = Some(io::Error::last_os_error());
                break;
            }
            const ACL_EXTENDED_DENY: c_int = 2;
            if tag_type != ACL_EXTENDED_DENY {
                rejected_entry = true;
                break;
            }
            entry_id = ACL_NEXT_ENTRY;
        }
        // SAFETY: `acl` was returned by `acl_get_fd_np` and is released once.
        let free_status = unsafe { acl_free(acl) };
        if let Some(error) = entry_error {
            return Err(operation_error(
                "inspect extended ACL on mutable output directory",
                display_path,
                error,
            ));
        }
        if free_status != 0 {
            return Err(operation_error(
                "release extended ACL for mutable output directory",
                display_path,
                io::Error::last_os_error(),
            ));
        }
        if rejected_entry {
            let message = if allow_deny_only {
                format!(
                    "output namespace ancestor {display_path} has a permissive or unrecognized extended ACL"
                )
            } else {
                format!(
                    "mutable output directory {display_path} has an extended ACL and cannot provide isolated transaction cleanup"
                )
            };
            return Err(io::Error::new(io::ErrorKind::PermissionDenied, message));
        }
        Ok(())
    }

    fn random_nonce() -> io::Result<String> {
        let mut nonce = [0_u8; 16];
        File::open("/dev/urandom")?.read_exact(&mut nonce)?;
        Ok(nonce.iter().map(|byte| format!("{byte:02x}")).collect())
    }

    #[derive(Clone, Copy)]
    enum FinalDirectoryPolicy {
        NamespaceAncestor,
        #[cfg(test)]
        TransactionParent,
    }

    fn open_validated_absolute_directory(
        path: &Path,
        final_policy: FinalDirectoryPolicy,
    ) -> io::Result<Directory> {
        open_validated_absolute_directory_with_ancestry(path, final_policy)
            .map(|(directory, _)| directory)
    }

    fn open_validated_absolute_directory_with_ancestry(
        path: &Path,
        final_policy: FinalDirectoryPolicy,
    ) -> io::Result<(Directory, Vec<(u64, u64)>)> {
        if !path.is_absolute() {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                format!(
                    "validated directory path {} is not absolute",
                    path.display()
                ),
            ));
        }
        let mut current = Directory::open_path(Path::new("/"))?;
        let components = path
            .components()
            .filter(|component| !matches!(component, Component::RootDir | Component::CurDir))
            .collect::<Vec<_>>();
        if components.is_empty() {
            match final_policy {
                FinalDirectoryPolicy::NamespaceAncestor => {
                    validate_namespace_ancestor(&current, "/")?
                }
                #[cfg(test)]
                FinalDirectoryPolicy::TransactionParent => {
                    validate_transaction_parent(&current, "/")?
                }
            }
            let identity = current.identity()?;
            return Ok((current, vec![identity]));
        }
        validate_namespace_ancestor(&current, "/")?;
        let mut identities = vec![current.identity()?];
        let mut walked = PathBuf::from("/");
        for (index, component) in components.iter().enumerate() {
            let Component::Normal(segment) = component else {
                return Err(io::Error::new(
                    io::ErrorKind::InvalidInput,
                    format!(
                        "validated directory path {} is not lexically normalized",
                        path.display()
                    ),
                ));
            };
            walked.push(segment);
            let child = current.open_child(&c_name(segment)?)?;
            let is_final = index + 1 == components.len();
            if is_final {
                match final_policy {
                    FinalDirectoryPolicy::NamespaceAncestor => {
                        validate_namespace_ancestor(&child, &walked.display().to_string())?
                    }
                    #[cfg(test)]
                    FinalDirectoryPolicy::TransactionParent => {
                        validate_transaction_parent(&child, &walked.display().to_string())?
                    }
                }
            } else {
                validate_namespace_ancestor(&child, &walked.display().to_string())?;
            }
            identities.push(child.identity()?);
            current = child;
        }
        Ok((current, identities))
    }

    fn resolve_relative_output_directory(
        output_directory: &Path,
        pinned_current_directory: &Directory,
        current_directory_path: &Path,
    ) -> io::Result<PathBuf> {
        let current_directory_path = super::absolute_lexical_path(current_directory_path)?;
        let walked_current_directory = open_validated_absolute_directory(
            &current_directory_path,
            FinalDirectoryPolicy::NamespaceAncestor,
        )?;
        if walked_current_directory.identity()? != pinned_current_directory.identity()? {
            return Err(io::Error::new(
                io::ErrorKind::PermissionDenied,
                "the current directory changed while resolving the relative output path",
            ));
        }
        super::absolute_lexical_path(&current_directory_path.join(output_directory))
    }

    fn resolve_output_directory(output_directory: &Path) -> io::Result<PathBuf> {
        if output_directory.as_os_str().is_empty() {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "output directory must not be empty",
            ));
        }
        if output_directory.is_absolute() {
            return super::absolute_lexical_path(output_directory);
        }
        let pinned_current_directory = Directory::open_path(Path::new("."))?;
        resolve_relative_output_directory(
            output_directory,
            &pinned_current_directory,
            &env::current_dir()?,
        )
    }

    struct ExistingPathWalk {
        identities: Vec<(u64, u64)>,
        complete: bool,
        sibling_parent: Option<ValidatedSiblingParent>,
    }

    struct ValidatedSiblingParent {
        directory: Directory,
        path: PathBuf,
        ancestry: Vec<(u64, u64)>,
    }

    fn validate_existing_output_directory(path: &Path) -> io::Result<ExistingPathWalk> {
        let mut current = Directory::open_path(Path::new("/"))?;
        validate_namespace_ancestor(&current, "/")?;
        let mut identities = vec![current.identity()?];
        let mut walked = PathBuf::from("/");
        let components = path
            .components()
            .filter(|component| !matches!(component, Component::RootDir | Component::CurDir))
            .collect::<Vec<_>>();
        if components.is_empty() {
            validate_transaction_parent(&current, "/")?;
            return Ok(ExistingPathWalk {
                identities,
                complete: true,
                sibling_parent: None,
            });
        }
        for (index, component) in components.iter().enumerate() {
            let Component::Normal(segment) = component else {
                return Err(io::Error::new(
                    io::ErrorKind::InvalidInput,
                    "output directory is not a normalized absolute path",
                ));
            };
            let is_final = index + 1 == components.len();
            let child = match current.open_child(&c_name(segment)?) {
                Ok(child) => child,
                Err(error) if error.kind() == io::ErrorKind::NotFound => {
                    validate_transaction_parent(&current, &walked.display().to_string())?;
                    return Ok(ExistingPathWalk {
                        identities: identities.clone(),
                        complete: false,
                        sibling_parent: Some(ValidatedSiblingParent {
                            directory: current,
                            path: walked,
                            ancestry: identities,
                        }),
                    });
                }
                Err(error) => return Err(output_directory_component_error(segment, error)),
            };
            walked.push(segment);
            if is_final {
                validate_transaction_parent(&child, &path.display().to_string())?;
                validate_transaction_parent(
                    &current,
                    &format!("compiler work parent of {}", path.display()),
                )?;
                identities.push(child.identity()?);
                let sibling_ancestry = identities[..identities.len() - 1].to_vec();
                return Ok(ExistingPathWalk {
                    identities,
                    complete: true,
                    sibling_parent: Some(ValidatedSiblingParent {
                        directory: current,
                        path: walked
                            .parent()
                            .expect("a non-root absolute path has a parent")
                            .to_owned(),
                        ancestry: sibling_ancestry,
                    }),
                });
            } else {
                validate_namespace_ancestor(&child, &segment.to_string_lossy())?;
            }
            identities.push(child.identity()?);
            current = child;
        }
        unreachable!("non-root output paths return from the component walk")
    }

    pub(super) fn validate_output_directory_path(output_directory: &Path) -> io::Result<PathBuf> {
        let output_directory = resolve_output_directory(output_directory)?;
        validate_existing_output_directory(&output_directory)?;
        Ok(output_directory)
    }

    fn work_root_overlaps_output(
        temporary_ancestry: &[(u64, u64)],
        candidate_identity: (u64, u64),
        output_walk: &ExistingPathWalk,
    ) -> bool {
        let output_contains_candidate = output_walk.identities.contains(&candidate_identity);
        let candidate_is_inside_output = output_walk.complete
            && output_walk.identities.last().is_some_and(|identity| {
                *identity == candidate_identity || temporary_ancestry.contains(identity)
            });
        output_contains_candidate || candidate_is_inside_output
    }

    #[cfg(test)]
    pub(super) fn work_root_overlaps_output_for_test(
        temporary_ancestry: &[(u64, u64)],
        candidate_identity: (u64, u64),
        output_identities: Vec<(u64, u64)>,
        output_complete: bool,
    ) -> bool {
        work_root_overlaps_output(
            temporary_ancestry,
            candidate_identity,
            &ExistingPathWalk {
                identities: output_identities,
                complete: output_complete,
                sibling_parent: None,
            },
        )
    }

    #[cfg(test)]
    pub(super) fn resolve_relative_output_directory_for_test(
        output_directory: &Path,
        pinned_current_directory: &Path,
        current_directory_path: &Path,
    ) -> io::Result<PathBuf> {
        resolve_relative_output_directory(
            output_directory,
            &Directory::open_path(pinned_current_directory)?,
            current_directory_path,
        )
    }

    struct CompilerWorkRootOwnership {
        parent: Directory,
        directory: Directory,
        name: CString,
        identity: (u64, u64),
    }

    pub(super) struct CompilerWorkRoot {
        path: PathBuf,
        ownership: Option<CompilerWorkRootOwnership>,
    }

    impl CompilerWorkRoot {
        pub(super) fn create(output_directory: &Path) -> io::Result<Self> {
            let output_boundary = resolve_output_directory(output_directory)
                .map_err(compiler_output_boundary_error)?;
            let output_walk = validate_existing_output_directory(&output_boundary)
                .map_err(compiler_output_boundary_error)?;
            let sibling_parent = output_walk.sibling_parent.ok_or_else(|| {
                compiler_output_boundary_error(io::Error::new(
                    io::ErrorKind::InvalidInput,
                    "compiler output directory must have an outside sibling",
                ))
            })?;
            Self::create_with_parent(
                output_boundary,
                sibling_parent.path,
                sibling_parent.directory,
                sibling_parent.ancestry,
                |output_boundary, _| validate_existing_output_directory(output_boundary),
            )
        }

        #[cfg(test)]
        pub(super) fn create_in_parent_for_test(
            output_directory: &Path,
            work_parent: &Path,
        ) -> io::Result<Self> {
            Self::create_in_parent_with_walk_for_test(
                output_directory,
                work_parent,
                |boundary, _| validate_existing_output_directory(boundary),
            )
        }

        #[cfg(test)]
        pub(super) fn create_with_candidate_alias_for_test(
            output_directory: &Path,
            work_parent: &Path,
        ) -> io::Result<Self> {
            Self::create_in_parent_with_walk_for_test(
                output_directory,
                work_parent,
                |_, candidate_identity| {
                    Ok(ExistingPathWalk {
                        identities: vec![candidate_identity],
                        complete: false,
                        sibling_parent: None,
                    })
                },
            )
        }

        #[cfg(test)]
        fn create_in_parent_with_walk_for_test<F>(
            output_directory: &Path,
            work_parent: &Path,
            output_walk_after_create: F,
        ) -> io::Result<Self>
        where
            F: FnMut(&Path, (u64, u64)) -> io::Result<ExistingPathWalk>,
        {
            let work_parent_path = fs::canonicalize(work_parent)?;
            let (parent, work_parent_ancestry) = open_validated_absolute_directory_with_ancestry(
                &work_parent_path,
                FinalDirectoryPolicy::TransactionParent,
            )?;
            let output_boundary = resolve_output_directory(output_directory)
                .map_err(compiler_output_boundary_error)?;
            validate_existing_output_directory(&output_boundary)
                .map_err(compiler_output_boundary_error)?;
            Self::create_with_parent(
                output_boundary,
                work_parent_path,
                parent,
                work_parent_ancestry,
                output_walk_after_create,
            )
        }

        fn create_with_parent<F>(
            output_boundary: PathBuf,
            work_parent_path: PathBuf,
            parent: Directory,
            work_parent_ancestry: Vec<(u64, u64)>,
            mut output_walk_after_create: F,
        ) -> io::Result<Self>
        where
            F: FnMut(&Path, (u64, u64)) -> io::Result<ExistingPathWalk>,
        {
            validate_transaction_parent(
                &parent,
                &format!("compiler work parent {}", work_parent_path.display()),
            )?;
            for _ in 0..16 {
                let basename = format!("circuitc-compile-{}-{}", process::id(), random_nonce()?);
                let candidate = work_parent_path.join(&basename);
                if candidate.starts_with(&output_boundary)
                    || output_boundary.starts_with(&candidate)
                {
                    return Err(compiler_output_boundary_error(io::Error::new(
                        io::ErrorKind::InvalidInput,
                        "compiler work root must be outside the output directory",
                    )));
                }
                let name = c_name(OsStr::new(&basename))?;
                let cleanup_parent = parent.try_clone()?;
                match parent.create_child_with_mode(&name, 0o700 as Mode) {
                    Ok(false) => continue,
                    Err(error) => return Err(error),
                    Ok(true) => {}
                }
                let directory = match parent.open_child(&name) {
                    Ok(directory) => directory,
                    Err(error) => {
                        return Err(with_cleanup_error(error, parent.remove_directory(&name)));
                    }
                };
                let identity = match directory.identity() {
                    Ok(identity) => identity,
                    Err(error) => {
                        return Err(with_cleanup_error(error, parent.remove_directory(&name)));
                    }
                };
                let work_root = Self {
                    path: candidate,
                    ownership: Some(CompilerWorkRootOwnership {
                        parent: cleanup_parent,
                        directory,
                        name,
                        identity,
                    }),
                };
                work_root.validate_created_directory()?;
                let output_walk = output_walk_after_create(&output_boundary, identity)
                    .map_err(compiler_output_boundary_error)?;
                if work_root_overlaps_output(&work_parent_ancestry, identity, &output_walk) {
                    return Err(compiler_output_boundary_error(io::Error::new(
                        io::ErrorKind::InvalidInput,
                        "compiler work root must be outside the output directory",
                    )));
                }
                return Ok(work_root);
            }
            Err(io::Error::new(
                io::ErrorKind::AlreadyExists,
                "could not allocate a unique compiler work root",
            ))
        }

        fn validate_created_directory(&self) -> io::Result<()> {
            let ownership = self
                .ownership
                .as_ref()
                .expect("compiler work-root ownership is live");
            let metadata = ownership.directory.0.metadata()?;
            // SAFETY: `geteuid` has no preconditions and does not dereference pointers.
            let effective_uid = unsafe { libc::geteuid() };
            if metadata.uid() != effective_uid || metadata.mode() & 0o777 != 0o700 {
                return Err(io::Error::new(
                    io::ErrorKind::PermissionDenied,
                    "compiler work root is not a caller-owned 0700 directory",
                ));
            }
            reject_extended_acl(&ownership.directory, &self.path.display().to_string())?;
            Ok(())
        }

        pub(super) fn path(&self) -> &Path {
            &self.path
        }

        pub(super) fn cleanup(mut self) -> io::Result<()> {
            let result = self.remove_owned_directory();
            if result.is_ok() {
                self.ownership.take();
            }
            result
        }

        fn remove_owned_directory(&self) -> io::Result<()> {
            let ownership = self
                .ownership
                .as_ref()
                .expect("compiler work-root ownership is live");
            let runner_root_name = c_name(OsStr::new("circuitc-ohmnivore-work"))?;
            match ownership.directory.open_child(&runner_root_name) {
                Ok(runner_root) => {
                    let metadata = runner_root.0.metadata()?;
                    // SAFETY: `geteuid` has no preconditions and does not dereference pointers.
                    let effective_uid = unsafe { libc::geteuid() };
                    if metadata.uid() != effective_uid || metadata.mode() & 0o077 != 0 {
                        return Err(io::Error::new(
                            io::ErrorKind::PermissionDenied,
                            "compiler runner work directory is not private and caller-owned",
                        ));
                    }
                    ownership.directory.remove_directory(&runner_root_name)?;
                }
                Err(error) if error.kind() == io::ErrorKind::NotFound => {}
                Err(error) => return Err(error),
            }
            let current = ownership.parent.open_child(&ownership.name)?;
            if current.identity()? != ownership.identity {
                return Err(io::Error::new(
                    io::ErrorKind::PermissionDenied,
                    "compiler work-root name no longer identifies the created directory",
                ));
            }
            ownership.parent.remove_directory(&ownership.name)
        }
    }

    impl Drop for CompilerWorkRoot {
        fn drop(&mut self) {
            if self.ownership.is_some() && self.remove_owned_directory().is_ok() {
                self.ownership.take();
            }
        }
    }

    fn create_private_quarantine(root: &Directory) -> io::Result<PrivateQuarantine> {
        validate_transaction_parent(root, "transaction output root")?;
        for _ in 0..16 {
            let name = c_name(OsStr::new(&format!(
                ".circuitc-transaction-{}-{}",
                process::id(),
                random_nonce()?
            )))?;
            let cleanup = created_directory_cleanup_name(OsStr::from_bytes(name.to_bytes()))?;
            let cleanup_parent = root.try_clone()?;
            match root.create_child_with_mode(&name, 0o700 as Mode) {
                Ok(true) => {
                    // Acquire cleanup ownership immediately after mkdir. The
                    // validated parent prevents a different security principal
                    // from replacing this caller-owned name during inspection.
                    let provisional = CreatedDirectory {
                        parent: cleanup_parent,
                        name: name.clone(),
                        cleanup,
                        identity: None,
                    };
                    let directory = match root.open_child(&name) {
                        Ok(directory) => directory,
                        Err(error) => {
                            return Err(with_cleanup_error(
                                operation_error(
                                    "open private transaction quarantine",
                                    &name.to_string_lossy(),
                                    error,
                                ),
                                remove_owned_directory(&provisional),
                            ));
                        }
                    };
                    let metadata = match directory.0.metadata() {
                        Ok(metadata) => metadata,
                        Err(error) => {
                            return Err(with_cleanup_error(
                                operation_error(
                                    "inspect private transaction quarantine",
                                    &name.to_string_lossy(),
                                    error,
                                ),
                                remove_owned_directory(&provisional),
                            ));
                        }
                    };
                    // SAFETY: `geteuid` has no preconditions and does not dereference pointers.
                    let effective_uid = unsafe { libc::geteuid() };
                    let identity = (metadata.dev(), metadata.ino());
                    if metadata.uid() != effective_uid || metadata.mode() & 0o777 != 0o700 {
                        let error = io::Error::new(
                            io::ErrorKind::PermissionDenied,
                            "private transaction quarantine is not a caller-owned 0700 directory",
                        );
                        let owned = CreatedDirectory {
                            identity: Some(identity),
                            ..provisional
                        };
                        return Err(with_cleanup_error(error, remove_owned_directory(&owned)));
                    }
                    if let Err(error) = reject_extended_acl(&directory, &name.to_string_lossy()) {
                        let owned = CreatedDirectory {
                            identity: Some(identity),
                            ..provisional
                        };
                        return Err(with_cleanup_error(error, remove_owned_directory(&owned)));
                    }
                    return Ok(PrivateQuarantine {
                        parent: provisional.parent,
                        name,
                        directory,
                        identity,
                        cleanup_descriptor_reserve: RefCell::new(Vec::new()),
                    });
                }
                Ok(false) => continue,
                Err(error) => return Err(error),
            }
        }
        Err(io::Error::new(
            io::ErrorKind::AlreadyExists,
            "could not allocate a unique private transaction quarantine",
        ))
    }

    fn acquire_cleanup_descriptor_reserve() -> io::Result<Vec<File>> {
        const RESERVED_DESCRIPTORS: usize = 4;

        (0..RESERVED_DESCRIPTORS)
            .map(|_| File::open("/dev/null"))
            .collect()
    }

    fn release_cleanup_descriptor_reserve(quarantine: &PrivateQuarantine) {
        quarantine.cleanup_descriptor_reserve.borrow_mut().clear();
    }

    pub(super) fn write_outputs(
        output_directory: &Path,
        outputs: &[(String, &[u8])],
    ) -> io::Result<super::WriteOutcome> {
        write_outputs_with_hooks(
            output_directory,
            outputs,
            WriteHooks {
                pre_anchor: || Ok(()),
                before_rename_probe: || Ok(()),
                sync_staging: |_, file: &File| file.sync_all(),
                before_publish: |_| Ok(()),
                after_publication: || Ok(()),
                sync_directory: |directory: &Directory| directory.0.sync_all(),
            },
        )
    }

    fn write_outputs_with_hooks<F, P, S, G, H, D>(
        output_directory: &Path,
        outputs: &[(String, &[u8])],
        hooks: WriteHooks<F, P, S, G, H, D>,
    ) -> io::Result<super::WriteOutcome>
    where
        F: FnOnce() -> io::Result<()>,
        P: FnOnce() -> io::Result<()>,
        S: FnMut(usize, &File) -> io::Result<()>,
        G: FnMut(usize) -> io::Result<()>,
        H: FnOnce() -> io::Result<()>,
        D: FnMut(&Directory) -> io::Result<()>,
    {
        let WriteHooks {
            pre_anchor: pre_anchor_hook,
            before_rename_probe: before_rename_probe_hook,
            mut sync_staging,
            before_publish: mut before_publish_hook,
            after_publication: after_publication_hook,
            mut sync_directory,
        } = hooks;
        let relative_paths: Vec<_> = outputs
            .iter()
            .map(|(filename, _)| super::validate_relative_output_path(filename))
            .collect::<io::Result<_>>()?;
        let output_directory = resolve_output_directory(output_directory)?;
        let (root, mut created_directories) = prepare_output_directory(&output_directory)?;

        // This first pass is deliberately non-mutating. A later invalid target
        // cannot leave newly-created generated directories behind.
        let initial_preflight = relative_paths.iter().try_for_each(|relative| {
            if let Some(parent) =
                open_existing_parent(&root, relative.parent().unwrap_or_else(|| Path::new("")))
                    .map_err(|error| {
                        operation_error(
                            "inspect output parent for",
                            &relative.display().to_string(),
                            error,
                        )
                    })?
            {
                let target = c_name(
                    relative
                        .file_name()
                        .expect("validated generated paths always have a basename"),
                )?;
                inspect_target(&parent, &target, relative)?;
            }
            Ok(())
        });
        if let Err(error) = initial_preflight {
            return Err(with_cleanup_error(
                error,
                remove_created_directories(&created_directories),
            ));
        }

        if let Err(error) = pre_anchor_hook() {
            return Err(with_cleanup_error(
                error,
                remove_created_directories(&created_directories),
            ));
        }

        let quarantine = match create_private_quarantine(&root) {
            Ok(quarantine) => quarantine,
            Err(error) => {
                return Err(with_cleanup_error(
                    error,
                    remove_created_directories(&created_directories),
                ));
            }
        };
        let cleanup_descriptor_reserve = match acquire_cleanup_descriptor_reserve() {
            Ok(reserve) => reserve,
            Err(error) => {
                // The quarantine retains its own parent and directory
                // descriptors. Release the redundant root descriptor so its
                // cleanup can still authenticate the renamed directory when
                // reserve acquisition itself reaches the descriptor limit.
                drop(root);
                return Err(with_cleanup_error(
                    operation_error(
                        "reserve descriptor capacity for transaction cleanup in",
                        &output_directory.display().to_string(),
                        error,
                    ),
                    rollback(&[], &[], &[], &[], &quarantine, &created_directories),
                ));
            }
        };
        *quarantine.cleanup_descriptor_reserve.borrow_mut() = cleanup_descriptor_reserve;

        let entries_result = relative_paths
            .iter()
            .zip(outputs)
            .enumerate()
            .map(|(index, (relative, (_, contents)))| {
                let parent = create_relative_parent(
                    &root,
                    relative.parent().unwrap_or_else(|| Path::new("")),
                    &mut created_directories,
                )
                .map_err(|error| {
                    operation_error(
                        "create output parent for",
                        &relative.display().to_string(),
                        error,
                    )
                })?;
                let basename = relative
                    .file_name()
                    .expect("validated generated paths always have a basename");
                let target = c_name(basename)?;
                let basename = basename.to_string_lossy();
                Ok(OutputEntry {
                    parent,
                    target,
                    temporary: c_name(OsStr::new(&format!(".{basename}.tmp-{}", process::id())))?,
                    backup: c_name(OsStr::new(&format!(".{basename}.backup-{}", process::id())))?,
                    rename_probe: c_name(OsStr::new(&format!(
                        ".{basename}.rename-probe-{}",
                        process::id()
                    )))?,
                    published_claim: c_name(OsStr::new(&format!("{index}-published")))?,
                    temporary_claim: c_name(OsStr::new(&format!("{index}-temporary")))?,
                    backup_claim: c_name(OsStr::new(&format!("{index}-backup")))?,
                    rename_probe_claim: c_name(OsStr::new(&format!("{index}-rename-probe")))?,
                    display_path: relative.display().to_string(),
                    contents,
                    had_existing_target: false,
                })
            })
            .collect::<io::Result<Vec<_>>>();
        let mut entries = match entries_result {
            Ok(entries) => entries,
            Err(error) => {
                return Err(with_cleanup_error(
                    error,
                    rollback(&[], &[], &[], &[], &quarantine, &created_directories),
                ));
            }
        };

        let pinned_preflight = entries.iter_mut().try_for_each(|entry| {
            let parent_device = entry.parent.identity()?.0;
            if parent_device != quarantine.identity.0 {
                return Err(io::Error::new(
                    io::ErrorKind::CrossesDevices,
                    format!(
                        "output parent for {} is on a different filesystem than the transaction quarantine",
                        entry.display_path
                    ),
                ));
            }
            entry.had_existing_target =
                inspect_target(&entry.parent, &entry.target, Path::new(&entry.display_path))?;
            ensure_absent(
                &entry.parent,
                &entry.temporary,
                "temporary staging",
                &entry.display_path,
            )?;
            ensure_absent(
                &entry.parent,
                &entry.backup,
                "backup staging",
                &entry.display_path,
            )?;
            ensure_absent(
                &entry.parent,
                &entry.rename_probe,
                "transaction rename probe",
                &entry.display_path,
            )?;
            for (name, label) in [
                (&entry.published_claim, "published-output quarantine"),
                (&entry.temporary_claim, "temporary-output quarantine"),
                (&entry.backup_claim, "backup quarantine"),
                (&entry.rename_probe_claim, "rename-probe quarantine"),
            ] {
                ensure_absent(&quarantine.directory, name, label, &entry.display_path)?;
            }
            Ok(())
        });
        if let Err(error) = pinned_preflight {
            return Err(with_cleanup_error(
                error,
                rollback(&entries, &[], &[], &[], &quarantine, &created_directories),
            ));
        }

        if let Err(error) = before_rename_probe_hook() {
            return Err(with_cleanup_error(
                error,
                rollback(&entries, &[], &[], &[], &quarantine, &created_directories),
            ));
        }

        for entry in &entries {
            // Probe every pinned descriptor. Two bind-mount instances of the
            // same underlying directory can share device and inode identities
            // while still rejecting a rename across their mount boundary.
            if let Err(error) = verify_quarantine_rename_pair(entry, &quarantine) {
                return Err(with_cleanup_error(
                    error,
                    rollback(&entries, &[], &[], &[], &quarantine, &created_directories),
                ));
            }
        }

        let mut temporary_files = Vec::new();
        for (index, entry) in entries.iter().enumerate() {
            match entry.parent.create_file(&entry.temporary) {
                Ok(file) => {
                    // Register cleanup ownership and retain the descriptor
                    // before any later fallible inspection. Keeping the
                    // descriptor live also prevents a removed inode from
                    // being recycled for a racing pathname replacement.
                    temporary_files.push(RetainedStagedFile {
                        index,
                        identity: None,
                        descriptor: file,
                    });
                }
                Err(error) => {
                    return Err(with_cleanup_error(
                        operation_error("create temporary output for", &entry.display_path, error),
                        rollback(
                            &entries,
                            &temporary_files,
                            &[],
                            &[],
                            &quarantine,
                            &created_directories,
                        ),
                    ));
                }
            }
            let metadata = match temporary_files[index].descriptor.metadata() {
                Ok(metadata) => metadata,
                Err(error) => {
                    return Err(with_cleanup_error(
                        operation_error("inspect temporary output for", &entry.display_path, error),
                        rollback(
                            &entries,
                            &temporary_files,
                            &[],
                            &[],
                            &quarantine,
                            &created_directories,
                        ),
                    ));
                }
            };
            temporary_files[index].identity = Some((metadata.dev(), metadata.ino()));
            if let Err(error) = temporary_files[index].descriptor.write_all(entry.contents) {
                return Err(with_cleanup_error(
                    operation_error("write temporary output for", &entry.display_path, error),
                    rollback(
                        &entries,
                        &temporary_files,
                        &[],
                        &[],
                        &quarantine,
                        &created_directories,
                    ),
                ));
            }
            if let Err(error) = sync_staging(index, &temporary_files[index].descriptor) {
                return Err(with_cleanup_error(
                    operation_error("sync temporary output for", &entry.display_path, error),
                    rollback(
                        &entries,
                        &temporary_files,
                        &[],
                        &[],
                        &quarantine,
                        &created_directories,
                    ),
                ));
            }
        }

        let mut backed_up = Vec::new();
        for (index, entry) in entries.iter().enumerate() {
            if entry.had_existing_target {
                let (retained_descriptor, expected_identity) = match required_regular_file(
                    &entry.parent,
                    &entry.target,
                    &entry.display_path,
                ) {
                    Ok(identity) => identity,
                    Err(error) => {
                        return Err(with_cleanup_error(
                            error,
                            rollback(
                                &entries,
                                &temporary_files,
                                &backed_up,
                                &[],
                                &quarantine,
                                &created_directories,
                            ),
                        ));
                    }
                };
                if let Err(error) = rename_noreplace_with_context(
                    &entry.parent,
                    &entry.target,
                    &entry.parent,
                    &entry.backup,
                    "stage backup for",
                    &entry.display_path,
                ) {
                    return Err(with_cleanup_error(
                        error,
                        rollback(
                            &entries,
                            &temporary_files,
                            &backed_up,
                            &[],
                            &quarantine,
                            &created_directories,
                        ),
                    ));
                }
                // Retain the descriptor that authenticated the expected
                // identity. Besides registering the moved backup before any
                // fallible pathname inspection, the live descriptor prevents
                // its inode from being recycled for a racing replacement.
                backed_up.push(RetainedBackup {
                    index,
                    identity: expected_identity,
                    _descriptor: retained_descriptor,
                });
                let actual_identity = match required_regular_file_identity(
                    &entry.parent,
                    &entry.backup,
                    &entry.display_path,
                ) {
                    Ok(identity) => identity,
                    Err(error) => {
                        return Err(with_cleanup_error(
                            error,
                            rollback(
                                &entries,
                                &temporary_files,
                                &backed_up,
                                &[],
                                &quarantine,
                                &created_directories,
                            ),
                        ));
                    }
                };
                if actual_identity != expected_identity {
                    let error = io::Error::other(format!(
                        "output target {} changed while staging its backup",
                        entry.display_path
                    ));
                    return Err(with_cleanup_error(
                        error,
                        rollback(
                            &entries,
                            &temporary_files,
                            &backed_up,
                            &[],
                            &quarantine,
                            &created_directories,
                        ),
                    ));
                }
            }
        }

        let mut published = Vec::new();
        for (index, entry) in entries.iter().enumerate() {
            if let Err(error) = before_publish_hook(index) {
                return Err(with_cleanup_error(
                    error,
                    rollback(
                        &entries,
                        &temporary_files,
                        &backed_up,
                        &published,
                        &quarantine,
                        &created_directories,
                    ),
                ));
            }
            if let Err(error) = rename_noreplace_with_context(
                &entry.parent,
                &entry.temporary,
                &entry.parent,
                &entry.target,
                "publish",
                &entry.display_path,
            ) {
                return Err(with_cleanup_error(
                    error,
                    rollback(
                        &entries,
                        &temporary_files,
                        &backed_up,
                        &published,
                        &quarantine,
                        &created_directories,
                    ),
                ));
            }
            published.push((
                index,
                temporary_files[index]
                    .identity
                    .expect("published temporary output has a recorded identity"),
            ));
        }
        release_cleanup_descriptor_reserve(&quarantine);
        let hook_result = after_publication_hook();
        let cleanup_result = cleanup_backups(&entries, &backed_up, &quarantine);
        let quarantine_cleanup_result = remove_private_quarantine(&quarantine);
        let directory_sync_result = sync_published_directories(
            &entries,
            &created_directories,
            &quarantine.parent,
            &mut sync_directory,
        );
        let mut cleanup_warning = None;
        merge_post_publication_result(&mut cleanup_warning, hook_result);
        merge_post_publication_result(&mut cleanup_warning, cleanup_result);
        merge_post_publication_result(&mut cleanup_warning, quarantine_cleanup_result);
        merge_post_publication_result(&mut cleanup_warning, directory_sync_result);
        Ok(super::WriteOutcome { cleanup_warning })
    }

    fn prepare_output_directory(
        output_directory: &Path,
    ) -> io::Result<(Directory, Vec<CreatedDirectory>)> {
        if output_directory.as_os_str().is_empty() {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "output directory must not be empty",
            ));
        }
        let anchor = if output_directory.is_absolute() {
            Path::new("/")
        } else {
            Path::new(".")
        };
        let mut current = Directory::open_path(anchor)?;
        let mut created = Vec::new();
        validate_namespace_ancestor(&current, &anchor.display().to_string())?;
        let mut components = output_directory
            .components()
            .filter(|component| !matches!(component, Component::RootDir | Component::CurDir))
            .peekable();
        while let Some(component) = components.next() {
            let is_final = components.peek().is_none();
            let segment = match component {
                Component::ParentDir => OsStr::new(".."),
                Component::Normal(segment) => segment,
                Component::Prefix(_) => {
                    return Err(io::Error::new(
                        io::ErrorKind::InvalidInput,
                        "output directory contains an unsupported path prefix",
                    ));
                }
                Component::RootDir | Component::CurDir => unreachable!("components were filtered"),
            };
            let name = c_name(segment)?;
            match current.open_child(&name) {
                Ok(child) => {
                    if !is_final
                        && let Err(error) =
                            validate_namespace_ancestor(&child, &segment.to_string_lossy())
                    {
                        return Err(with_cleanup_error(
                            error,
                            remove_created_directories(&created),
                        ));
                    }
                    current = child;
                }
                Err(error)
                    if error.kind() == io::ErrorKind::NotFound
                        && !matches!(component, Component::ParentDir) =>
                {
                    if let Err(error) = validate_transaction_parent(
                        &current,
                        &format!("parent of {}", segment.to_string_lossy()),
                    ) {
                        return Err(with_cleanup_error(
                            error,
                            remove_created_directories(&created),
                        ));
                    }
                    let cleanup = match created_directory_cleanup_name(segment) {
                        Ok(cleanup) => cleanup,
                        Err(error) => {
                            return Err(with_cleanup_error(
                                error,
                                remove_created_directories(&created),
                            ));
                        }
                    };
                    if let Err(error) = ensure_absent(
                        &current,
                        &cleanup,
                        "created-directory cleanup",
                        &segment.to_string_lossy(),
                    ) {
                        return Err(with_cleanup_error(
                            error,
                            remove_created_directories(&created),
                        ));
                    }
                    let cleanup_parent = match current.try_clone() {
                        Ok(parent) => parent,
                        Err(error) => {
                            return Err(with_cleanup_error(
                                error,
                                remove_created_directories(&created),
                            ));
                        }
                    };
                    let child_created = match current.create_child(&name) {
                        Ok(created) => created,
                        Err(error) => {
                            return Err(with_cleanup_error(
                                output_directory_component_error(segment, error),
                                remove_created_directories(&created),
                            ));
                        }
                    };
                    let owned_index = if child_created {
                        // Register ownership before opening or inspecting the
                        // new directory so every subsequent failure preserves it.
                        created.push(CreatedDirectory {
                            parent: cleanup_parent,
                            name: name.clone(),
                            cleanup,
                            identity: None,
                        });
                        Some(created.len() - 1)
                    } else {
                        None
                    };
                    match current.open_child(&name) {
                        Ok(child) => {
                            if let Some(index) = owned_index {
                                let identity = match child.identity() {
                                    Ok(identity) => identity,
                                    Err(error) => {
                                        return Err(with_cleanup_error(
                                            error,
                                            remove_created_directories(&created),
                                        ));
                                    }
                                };
                                created[index].identity = Some(identity);
                            }
                            if let Err(error) =
                                validate_transaction_parent(&child, &segment.to_string_lossy())
                            {
                                return Err(with_cleanup_error(
                                    error,
                                    remove_created_directories(&created),
                                ));
                            }
                            current = child;
                        }
                        Err(error) => {
                            return Err(with_cleanup_error(
                                output_directory_component_error(segment, error),
                                remove_created_directories(&created),
                            ));
                        }
                    }
                }
                Err(error) => {
                    return Err(with_cleanup_error(
                        output_directory_component_error(segment, error),
                        remove_created_directories(&created),
                    ));
                }
            }
        }
        if let Err(error) =
            validate_transaction_parent(&current, &output_directory.display().to_string())
        {
            return Err(with_cleanup_error(
                error,
                remove_created_directories(&created),
            ));
        }
        Ok((current, created))
    }

    fn output_directory_component_error(segment: &OsStr, error: io::Error) -> io::Error {
        io::Error::new(
            error.kind(),
            OutputDirectoryComponentFailure {
                segment: segment.to_owned(),
                source: error,
            },
        )
    }

    #[cfg(test)]
    pub(super) fn output_directory_component_raw_os_error(error: &io::Error) -> Option<i32> {
        error
            .get_ref()
            .and_then(|source| source.downcast_ref::<OutputDirectoryComponentFailure>())
            .and_then(|failure| failure.source.raw_os_error())
    }

    fn open_existing_parent(root: &Directory, relative: &Path) -> io::Result<Option<Directory>> {
        let mut current = root.try_clone()?;
        for component in relative.components() {
            let Component::Normal(segment) = component else {
                return Err(io::Error::new(
                    io::ErrorKind::InvalidInput,
                    "generated output parent is not a safe relative path",
                ));
            };
            match current.open_child(&c_name(segment)?) {
                Ok(child) => current = child,
                Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(None),
                Err(error) => return Err(error),
            }
        }
        Ok(Some(current))
    }

    fn create_relative_parent(
        root: &Directory,
        relative: &Path,
        created: &mut Vec<CreatedDirectory>,
    ) -> io::Result<Directory> {
        let mut current = root.try_clone()?;
        let mut components = relative.components().peekable();
        while let Some(component) = components.next() {
            let is_final = components.peek().is_none();
            let Component::Normal(segment) = component else {
                return Err(io::Error::new(
                    io::ErrorKind::InvalidInput,
                    "generated output parent is not a safe relative path",
                ));
            };
            let name = c_name(segment)?;
            match current.open_child(&name) {
                Ok(child) => {
                    if !is_final {
                        validate_namespace_ancestor(&child, &segment.to_string_lossy())?;
                    }
                    current = child;
                }
                Err(error) if error.kind() == io::ErrorKind::NotFound => {
                    validate_transaction_parent(
                        &current,
                        &format!("parent of {}", segment.to_string_lossy()),
                    )?;
                    let cleanup = created_directory_cleanup_name(segment)?;
                    ensure_absent(
                        &current,
                        &cleanup,
                        "created-directory cleanup",
                        &segment.to_string_lossy(),
                    )?;
                    let cleanup_parent = current.try_clone()?;
                    let child_created = current.create_child(&name)?;
                    let owned_index = if child_created {
                        created.push(CreatedDirectory {
                            parent: cleanup_parent,
                            name: name.clone(),
                            cleanup,
                            identity: None,
                        });
                        Some(created.len() - 1)
                    } else {
                        None
                    };
                    let child = current.open_child(&name)?;
                    if let Some(index) = owned_index {
                        created[index].identity = Some(child.identity()?);
                    }
                    validate_transaction_parent(&child, &segment.to_string_lossy())?;
                    current = child;
                }
                Err(error) => return Err(error),
            }
        }
        validate_transaction_parent(&current, &relative.display().to_string())?;
        Ok(current)
    }

    fn inspect_target(parent: &Directory, name: &CStr, path: &Path) -> io::Result<bool> {
        let opened = match parent.open_entry(name) {
            Err(error) if error.raw_os_error() == Some(EACCES) => {
                parent.open_entry_write_only(name)
            }
            result => result,
        };
        match opened {
            Ok(file) => match file.metadata() {
                Ok(metadata) if metadata.file_type().is_file() => Ok(true),
                Ok(_) => Err(non_regular_target_error(path)),
                Err(error) => Err(io::Error::new(
                    error.kind(),
                    format!(
                        "failed to inspect output target {} metadata: {error}",
                        path.display()
                    ),
                )),
            },
            Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(false),
            Err(error)
                if error
                    .raw_os_error()
                    .is_some_and(|code| code == ELOOP || code == ENOTDIR || code == EISDIR) =>
            {
                Err(non_regular_target_error(path))
            }
            Err(error) if error.raw_os_error() == Some(EACCES) => Err(io::Error::new(
                io::ErrorKind::PermissionDenied,
                format!(
                    "failed to inspect output target {}; existing targets must be readable or writable regular files: {error}",
                    path.display()
                ),
            )),
            Err(error) => Err(io::Error::new(
                error.kind(),
                format!(
                    "failed to inspect output target {}: {error}",
                    path.display()
                ),
            )),
        }
    }

    fn non_regular_target_error(path: &Path) -> io::Error {
        io::Error::other(format!(
            "output target {} exists and is not a regular file",
            path.display()
        ))
    }

    fn ensure_absent(
        parent: &Directory,
        name: &CStr,
        label: &str,
        display_path: &str,
    ) -> io::Result<()> {
        match parent.open_entry(name) {
            Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(()),
            Ok(_) => Err(io::Error::new(
                io::ErrorKind::AlreadyExists,
                format!("{label} path for {display_path} already exists"),
            )),
            Err(error) => Err(io::Error::new(
                error.kind(),
                format!("failed to inspect {label} path for {display_path}: {error}"),
            )),
        }
    }

    fn required_regular_file(
        parent: &Directory,
        name: &CStr,
        display_path: &str,
    ) -> io::Result<(File, (u64, u64))> {
        let opened = match parent.open_entry(name) {
            Err(error) if error.raw_os_error() == Some(EACCES) => {
                parent.open_entry_write_only(name)
            }
            result => result,
        };
        let file = opened.map_err(|error| {
            io::Error::new(
                error.kind(),
                format!("failed to identify regular file for {display_path}: {error}"),
            )
        })?;
        let metadata = file.metadata().map_err(|error| {
            io::Error::new(
                error.kind(),
                format!("failed to identify regular file for {display_path}: {error}"),
            )
        })?;
        if !metadata.is_file() {
            return Err(io::Error::other(format!(
                "transaction-owned path for {display_path} is not a regular file"
            )));
        }
        let identity = (metadata.dev(), metadata.ino());
        Ok((file, identity))
    }

    fn required_regular_file_identity(
        parent: &Directory,
        name: &CStr,
        display_path: &str,
    ) -> io::Result<(u64, u64)> {
        required_regular_file(parent, name, display_path).map(|(_, identity)| identity)
    }

    fn remove_verified_probe(
        parent: &Directory,
        name: &CStr,
        expected_identity: (u64, u64),
        display_path: &str,
    ) -> io::Result<()> {
        let actual_identity = required_regular_file_identity(parent, name, display_path)?;
        if actual_identity != expected_identity {
            return Err(io::Error::other(format!(
                "transaction rename probe for {display_path} changed; preserving it"
            )));
        }
        // The parent has already passed the ownership, mode, sticky-bit, and
        // Darwin ACL checks. A different effective UID cannot replace this
        // caller-owned probe between identity validation and removal.
        parent.remove_file(name)
    }

    fn verify_quarantine_rename_pair(
        entry: &OutputEntry<'_>,
        quarantine: &PrivateQuarantine,
    ) -> io::Result<()> {
        let file = quarantine
            .directory
            .create_file(&entry.rename_probe_claim)
            .map_err(|error| {
                operation_error(
                    "create transaction rename probe for",
                    &entry.display_path,
                    error,
                )
            })?;
        // Cleanup ownership is established by the exclusive create inside the
        // private quarantine before this fallible descriptor inspection.
        let metadata = match file.metadata() {
            Ok(metadata) => metadata,
            Err(error) => {
                let cleanup = quarantine.directory.remove_file(&entry.rename_probe_claim);
                return Err(with_cleanup_error(
                    operation_error(
                        "inspect transaction rename probe for",
                        &entry.display_path,
                        error,
                    ),
                    cleanup,
                ));
            }
        };
        let expected_identity = (metadata.dev(), metadata.ino());
        drop(file);

        if let Err(error) = rename_noreplace_with_context(
            &quarantine.directory,
            &entry.rename_probe_claim,
            &entry.parent,
            &entry.rename_probe,
            "probe transaction restore path for",
            &entry.display_path,
        ) {
            let cleanup = remove_verified_probe(
                &quarantine.directory,
                &entry.rename_probe_claim,
                expected_identity,
                &entry.display_path,
            );
            return Err(with_cleanup_error(error, cleanup));
        }

        if let Err(error) = rename_noreplace_with_context(
            &entry.parent,
            &entry.rename_probe,
            &quarantine.directory,
            &entry.rename_probe_claim,
            "probe transaction cleanup path for",
            &entry.display_path,
        ) {
            let cleanup = remove_verified_probe(
                &entry.parent,
                &entry.rename_probe,
                expected_identity,
                &entry.display_path,
            );
            return Err(with_cleanup_error(error, cleanup));
        }

        remove_verified_probe(
            &quarantine.directory,
            &entry.rename_probe_claim,
            expected_identity,
            &entry.display_path,
        )
        .map_err(|error| {
            operation_error(
                "finish transaction rename probe for",
                &entry.display_path,
                error,
            )
        })
    }

    fn claim_owned_file(
        source_parent: &Directory,
        source: &CStr,
        quarantine: &PrivateQuarantine,
        claim: &CStr,
        expected_identity: (u64, u64),
        display_path: &str,
        missing_is_clean: bool,
    ) -> io::Result<bool> {
        match rename_noreplace_with_context(
            source_parent,
            source,
            &quarantine.directory,
            claim,
            "claim transaction-owned path for cleanup of",
            display_path,
        ) {
            Ok(()) => {}
            Err(error) if missing_is_clean && error.kind() == io::ErrorKind::NotFound => {
                return Ok(false);
            }
            Err(error) => return Err(error),
        }

        // The claim now lives below a random caller-owned 0700 directory held
        // open by descriptor. A different security principal cannot replace it
        // between this identity check and the final disposition.
        let actual_identity =
            match required_regular_file_identity(&quarantine.directory, claim, display_path) {
                Ok(identity) => identity,
                Err(error) => {
                    let restore = rename_noreplace_with_context(
                        &quarantine.directory,
                        claim,
                        source_parent,
                        source,
                        "restore unverified cleanup claim for",
                        display_path,
                    );
                    return Err(with_cleanup_error(error, restore));
                }
            };
        if actual_identity != expected_identity {
            let error = io::Error::other(format!(
                "transaction-owned path for {display_path} changed before cleanup; refusing to remove it"
            ));
            let restore = rename_noreplace_with_context(
                &quarantine.directory,
                claim,
                source_parent,
                source,
                "restore changed cleanup claim for",
                display_path,
            );
            return Err(with_cleanup_error(error, restore));
        }
        Ok(true)
    }

    fn remove_owned_file(
        source_parent: &Directory,
        source: &CStr,
        quarantine: &PrivateQuarantine,
        claim: &CStr,
        expected_identity: (u64, u64),
        display_path: &str,
        missing_is_clean: bool,
    ) -> io::Result<()> {
        if claim_owned_file(
            source_parent,
            source,
            quarantine,
            claim,
            expected_identity,
            display_path,
            missing_is_clean,
        )? {
            quarantine.directory.remove_file(claim)
        } else {
            Ok(())
        }
    }

    fn restore_owned_file(
        source_parent: &Directory,
        source: &CStr,
        quarantine: &PrivateQuarantine,
        claim: &CStr,
        target: &CStr,
        expected_identity: (u64, u64),
        display_path: &str,
    ) -> io::Result<()> {
        claim_owned_file(
            source_parent,
            source,
            quarantine,
            claim,
            expected_identity,
            display_path,
            false,
        )?;
        match rename_noreplace_with_context(
            &quarantine.directory,
            claim,
            source_parent,
            target,
            "restore backup for",
            display_path,
        ) {
            Ok(()) => Ok(()),
            Err(error) => {
                let preserve = rename_noreplace_with_context(
                    &quarantine.directory,
                    claim,
                    source_parent,
                    source,
                    "preserve unrestored backup for",
                    display_path,
                );
                Err(with_cleanup_error(error, preserve))
            }
        }
    }

    fn remove_owned_directory(directory: &CreatedDirectory) -> io::Result<()> {
        match rename_noreplace_with_context(
            &directory.parent,
            &directory.name,
            &directory.parent,
            &directory.cleanup,
            "claim transaction-created directory for cleanup of",
            &directory.name.to_string_lossy(),
        ) {
            Ok(()) => {}
            Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(()),
            Err(error) => return Err(error),
        }
        let Some(expected_identity) = directory.identity else {
            let error = io::Error::other(format!(
                "transaction-created directory {} could not be identified; preserving it",
                directory.name.to_string_lossy()
            ));
            let restore = rename_noreplace_with_context(
                &directory.parent,
                &directory.cleanup,
                &directory.parent,
                &directory.name,
                "restore unidentified created-directory cleanup claim for",
                &directory.name.to_string_lossy(),
            );
            return Err(with_cleanup_error(error, restore));
        };
        let actual_identity = match directory.parent.open_child(&directory.cleanup) {
            Ok(child) => child.identity(),
            Err(error) => Err(error),
        };
        match actual_identity {
            Ok(identity) if identity == expected_identity => {
                match directory.parent.remove_directory(&directory.cleanup) {
                    Ok(()) => Ok(()),
                    Err(error) => {
                        match rename_noreplace_with_context(
                            &directory.parent,
                            &directory.cleanup,
                            &directory.parent,
                            &directory.name,
                            "restore nonempty transaction-created directory after failed cleanup of",
                            &directory.name.to_string_lossy(),
                        ) {
                            Ok(()) => Err(io::Error::new(
                                error.kind(),
                                format!(
                                    "failed to remove transaction-created directory {}: {error}; restored it at its original name",
                                    directory.name.to_string_lossy()
                                ),
                            )),
                            Err(restore_error) => Err(io::Error::new(
                                error.kind(),
                                format!(
                                    "failed to remove transaction-created directory {}: {error}; recovery directory remains at {} because its original name could not be restored: {restore_error}",
                                    directory.name.to_string_lossy(),
                                    directory.cleanup.to_string_lossy()
                                ),
                            )),
                        }
                    }
                }
            }
            Ok(_) => {
                let error = io::Error::other(format!(
                    "transaction-created directory {} changed before cleanup; refusing to remove it",
                    directory.name.to_string_lossy()
                ));
                let restore = rename_noreplace_with_context(
                    &directory.parent,
                    &directory.cleanup,
                    &directory.parent,
                    &directory.name,
                    "restore changed created-directory cleanup claim for",
                    &directory.name.to_string_lossy(),
                );
                Err(with_cleanup_error(error, restore))
            }
            Err(error) => {
                let restore = rename_noreplace_with_context(
                    &directory.parent,
                    &directory.cleanup,
                    &directory.parent,
                    &directory.name,
                    "restore unverified created-directory cleanup claim for",
                    &directory.name.to_string_lossy(),
                );
                Err(with_cleanup_error(error, restore))
            }
        }
    }

    fn remove_private_quarantine(quarantine: &PrivateQuarantine) -> io::Result<()> {
        let cleanup =
            created_directory_cleanup_name(OsStr::from_bytes(quarantine.name.to_bytes()))?;
        match rename_noreplace_with_context(
            &quarantine.parent,
            &quarantine.name,
            &quarantine.parent,
            &cleanup,
            "claim private transaction quarantine for cleanup of",
            &quarantine.name.to_string_lossy(),
        ) {
            Ok(()) => {}
            Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(()),
            Err(error) => return Err(error),
        }

        let actual_identity = quarantine
            .parent
            .open_child(&cleanup)
            .and_then(|directory| directory.identity());
        match actual_identity {
            Ok(identity) if identity == quarantine.identity => {
                match quarantine.parent.remove_directory(&cleanup) {
                    Ok(()) => Ok(()),
                    Err(error) => {
                        let restore = rename_noreplace_with_context(
                            &quarantine.parent,
                            &cleanup,
                            &quarantine.parent,
                            &quarantine.name,
                            "restore nonempty private transaction quarantine after failed cleanup of",
                            &quarantine.name.to_string_lossy(),
                        );
                        match restore {
                            Ok(()) => Err(io::Error::new(
                                error.kind(),
                                format!(
                                    "failed to remove private transaction quarantine {}: {error}; restored it at its original name",
                                    quarantine.name.to_string_lossy()
                                ),
                            )),
                            Err(restore_error) => Err(io::Error::new(
                                error.kind(),
                                format!(
                                    "failed to remove private transaction quarantine {}: {error}; recovery directory remains at {} because its original name could not be restored: {restore_error}",
                                    quarantine.name.to_string_lossy(),
                                    cleanup.to_string_lossy()
                                ),
                            )),
                        }
                    }
                }
            }
            Ok(_) => {
                let error = io::Error::other(
                    "private transaction quarantine changed before cleanup; refusing to remove it",
                );
                let restore = rename_noreplace_with_context(
                    &quarantine.parent,
                    &cleanup,
                    &quarantine.parent,
                    &quarantine.name,
                    "restore changed private transaction quarantine cleanup claim for",
                    &quarantine.name.to_string_lossy(),
                );
                Err(with_cleanup_error(error, restore))
            }
            Err(error) => {
                let restore = rename_noreplace_with_context(
                    &quarantine.parent,
                    &cleanup,
                    &quarantine.parent,
                    &quarantine.name,
                    "restore unverified private transaction quarantine cleanup claim for",
                    &quarantine.name.to_string_lossy(),
                );
                Err(with_cleanup_error(error, restore))
            }
        }
    }

    fn preserve_unidentified_file(
        source_parent: &Directory,
        source: &CStr,
        quarantine: &PrivateQuarantine,
        claim: &CStr,
        display_path: &str,
    ) -> io::Result<()> {
        match rename_noreplace_with_context(
            source_parent,
            source,
            &quarantine.directory,
            claim,
            "preserve unidentified transaction-owned path for",
            display_path,
        ) {
            Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(()),
            Err(error) => Err(error),
            Ok(()) => Err(io::Error::other(format!(
                "transaction-owned path for {display_path} could not be identified and was preserved in the private transaction quarantine as {}",
                claim.to_string_lossy()
            ))),
        }
    }

    fn rollback(
        entries: &[OutputEntry<'_>],
        temporary_files: &[RetainedStagedFile],
        backed_up: &[RetainedBackup],
        published: &[(usize, (u64, u64))],
        quarantine: &PrivateQuarantine,
        created_directories: &[CreatedDirectory],
    ) -> io::Result<()> {
        release_cleanup_descriptor_reserve(quarantine);
        let mut errors = Vec::new();
        for (index, expected_identity) in published.iter().rev() {
            let entry = &entries[*index];
            record_cleanup_error(
                &mut errors,
                &format!("remove published output {}", entry.display_path),
                remove_owned_file(
                    &entry.parent,
                    &entry.target,
                    quarantine,
                    &entry.published_claim,
                    *expected_identity,
                    &entry.display_path,
                    true,
                ),
            );
        }
        for backup in backed_up.iter().rev() {
            let entry = &entries[backup.index];
            record_cleanup_error(
                &mut errors,
                &format!("restore original output {}", entry.display_path),
                restore_owned_file(
                    &entry.parent,
                    &entry.backup,
                    quarantine,
                    &entry.backup_claim,
                    &entry.target,
                    backup.identity,
                    &entry.display_path,
                ),
            );
        }
        for temporary in temporary_files {
            if published
                .iter()
                .any(|(published_index, _)| *published_index == temporary.index)
            {
                continue;
            }
            let entry = &entries[temporary.index];
            let cleanup = match temporary.identity {
                Some(expected_identity) => remove_owned_file(
                    &entry.parent,
                    &entry.temporary,
                    quarantine,
                    &entry.temporary_claim,
                    expected_identity,
                    &entry.display_path,
                    true,
                ),
                None => preserve_unidentified_file(
                    &entry.parent,
                    &entry.temporary,
                    quarantine,
                    &entry.temporary_claim,
                    &entry.display_path,
                ),
            };
            record_cleanup_error(
                &mut errors,
                &format!("remove temporary output {}", entry.display_path),
                cleanup,
            );
        }
        record_cleanup_error(
            &mut errors,
            "remove private transaction quarantine",
            remove_private_quarantine(quarantine),
        );
        record_cleanup_error(
            &mut errors,
            "remove transaction-created directories",
            remove_created_directories(created_directories),
        );
        cleanup_result("output rollback", errors)
    }

    fn cleanup_backups(
        entries: &[OutputEntry<'_>],
        backed_up: &[RetainedBackup],
        quarantine: &PrivateQuarantine,
    ) -> io::Result<()> {
        let mut errors = Vec::new();
        for backup in backed_up {
            let entry = &entries[backup.index];
            record_cleanup_error(
                &mut errors,
                &format!("remove backup for {}", entry.display_path),
                remove_owned_file(
                    &entry.parent,
                    &entry.backup,
                    quarantine,
                    &entry.backup_claim,
                    backup.identity,
                    &entry.display_path,
                    false,
                ),
            );
        }
        cleanup_result("published-output backup cleanup", errors)
    }

    fn sync_published_directories<D>(
        entries: &[OutputEntry<'_>],
        created_directories: &[CreatedDirectory],
        quarantine_parent: &Directory,
        sync_directory: &mut D,
    ) -> io::Result<()>
    where
        D: FnMut(&Directory) -> io::Result<()>,
    {
        let mut synced = BTreeSet::new();
        let mut errors = Vec::new();
        sync_directory_once(
            quarantine_parent,
            "sync private transaction quarantine parent",
            &mut synced,
            &mut errors,
            sync_directory,
        );
        for entry in entries {
            sync_directory_once(
                &entry.parent,
                &format!("sync published parent for {}", entry.display_path),
                &mut synced,
                &mut errors,
                sync_directory,
            );
        }
        for created in created_directories {
            sync_directory_once(
                &created.parent,
                &format!(
                    "sync parent of created directory {}",
                    created.name.to_string_lossy()
                ),
                &mut synced,
                &mut errors,
                sync_directory,
            );
        }
        cleanup_result("published-output directory synchronization", errors)
    }

    fn sync_directory_once<D>(
        directory: &Directory,
        action: &str,
        synced: &mut BTreeSet<(u64, u64)>,
        errors: &mut Vec<String>,
        sync_directory: &mut D,
    ) where
        D: FnMut(&Directory) -> io::Result<()>,
    {
        match directory.identity() {
            Ok(identity) if synced.insert(identity) => {
                record_cleanup_error(errors, action, sync_directory(directory));
            }
            Ok(_) => {}
            Err(error) => errors.push(format!("identify directory before {action}: {error}")),
        }
    }

    fn merge_post_publication_result(warning: &mut Option<io::Error>, result: io::Result<()>) {
        let Err(error) = result else {
            return;
        };
        *warning = Some(match warning.take() {
            Some(first) => with_cleanup_error(first, Err(error)),
            None => error,
        });
    }

    fn remove_created_directories(created: &[CreatedDirectory]) -> io::Result<()> {
        let mut errors = Vec::new();
        for directory in created.iter().rev() {
            record_cleanup_error(
                &mut errors,
                &format!(
                    "remove created directory {}",
                    directory.name.to_string_lossy()
                ),
                remove_owned_directory(directory),
            );
        }
        cleanup_result("created-directory cleanup", errors)
    }

    fn record_cleanup_error(errors: &mut Vec<String>, action: &str, result: io::Result<()>) {
        if let Err(error) = result {
            errors.push(format!("{action}: {error}"));
        }
    }

    fn cleanup_result(context: &str, errors: Vec<String>) -> io::Result<()> {
        if errors.is_empty() {
            Ok(())
        } else {
            Err(io::Error::other(format!(
                "{context} was incomplete: {}",
                errors.join("; ")
            )))
        }
    }

    fn with_cleanup_error(error: io::Error, cleanup: io::Result<()>) -> io::Error {
        match cleanup {
            Ok(()) => error,
            Err(cleanup_error) => io::Error::new(
                error.kind(),
                format!("{error}; additionally, {cleanup_error}"),
            ),
        }
    }

    fn rename_noreplace(
        old_directory: &Directory,
        old_name: &CStr,
        new_directory: &Directory,
        new_name: &CStr,
    ) -> io::Result<()> {
        #[cfg(target_os = "linux")]
        // SAFETY: both names are NUL-terminated and both descriptors remain open.
        let status = unsafe {
            renameat2(
                old_directory.0.as_raw_fd(),
                old_name.as_ptr(),
                new_directory.0.as_raw_fd(),
                new_name.as_ptr(),
                RENAME_NOREPLACE,
            )
        };
        #[cfg(target_os = "macos")]
        // SAFETY: both names are NUL-terminated and both descriptors remain open.
        let status = unsafe {
            renameatx_np(
                old_directory.0.as_raw_fd(),
                old_name.as_ptr(),
                new_directory.0.as_raw_fd(),
                new_name.as_ptr(),
                RENAME_EXCL,
            )
        };
        status_result(status)
    }

    fn rename_noreplace_with_context(
        old_directory: &Directory,
        old_name: &CStr,
        new_directory: &Directory,
        new_name: &CStr,
        operation: &str,
        display_path: &str,
    ) -> io::Result<()> {
        rename_noreplace(old_directory, old_name, new_directory, new_name)
            .map_err(|error| no_replace_error(operation, display_path, error))
    }

    fn no_replace_error(operation: &str, display_path: &str, error: io::Error) -> io::Error {
        if matches!(
            error.raw_os_error(),
            Some(EINVAL | ENOSYS | ENOTSUP_OR_EOPNOTSUPP)
        ) {
            io::Error::new(
                io::ErrorKind::Unsupported,
                format!(
                    "failed to {operation} {display_path}: output filesystem does not support no-replace rename: {error}"
                ),
            )
        } else {
            io::Error::new(
                error.kind(),
                format!("failed to {operation} {display_path} with no-replace rename: {error}"),
            )
        }
    }

    fn operation_error(operation: &str, display_path: &str, error: io::Error) -> io::Error {
        io::Error::new(
            error.kind(),
            format!("failed to {operation} {display_path}: {error}"),
        )
    }

    fn c_name(name: &OsStr) -> io::Result<CString> {
        CString::new(name.as_bytes()).map_err(|_| {
            io::Error::new(
                io::ErrorKind::InvalidInput,
                "generated output path contains a NUL byte",
            )
        })
    }

    fn created_directory_cleanup_name(name: &OsStr) -> io::Result<CString> {
        let mut cleanup = OsString::from(".");
        cleanup.push(name);
        cleanup.push(format!(".directory-cleanup-{}", process::id()));
        c_name(&cleanup)
    }

    fn file_from_descriptor(descriptor: RawFd) -> io::Result<File> {
        if descriptor < 0 {
            Err(io::Error::last_os_error())
        } else {
            // SAFETY: a successful open/openat call returns a new owned descriptor.
            Ok(unsafe { File::from_raw_fd(descriptor) })
        }
    }

    fn status_result(status: c_int) -> io::Result<()> {
        if status == 0 {
            Ok(())
        } else {
            Err(io::Error::last_os_error())
        }
    }

    #[cfg(test)]
    pub(super) fn write_outputs_after_pre_anchor_hook<F>(
        output_directory: &Path,
        outputs: &[(String, &[u8])],
        hook: F,
    ) -> io::Result<super::WriteOutcome>
    where
        F: FnOnce() -> io::Result<()>,
    {
        write_outputs_with_hooks(
            output_directory,
            outputs,
            WriteHooks {
                pre_anchor: hook,
                before_rename_probe: || Ok(()),
                sync_staging: |_, file: &File| file.sync_all(),
                before_publish: |_| Ok(()),
                after_publication: || Ok(()),
                sync_directory: |directory: &Directory| directory.0.sync_all(),
            },
        )
    }

    #[cfg(test)]
    pub(super) fn write_outputs_with_staging_sync_hook<F>(
        output_directory: &Path,
        outputs: &[(String, &[u8])],
        mut hook: F,
    ) -> io::Result<super::WriteOutcome>
    where
        F: FnMut(usize) -> io::Result<()>,
    {
        write_outputs_with_hooks(
            output_directory,
            outputs,
            WriteHooks {
                pre_anchor: || Ok(()),
                before_rename_probe: || Ok(()),
                sync_staging: |index, _: &File| hook(index),
                before_publish: |_| Ok(()),
                after_publication: || Ok(()),
                sync_directory: |directory: &Directory| directory.0.sync_all(),
            },
        )
    }

    #[cfg(test)]
    pub(super) fn write_outputs_before_publish_hook<F>(
        output_directory: &Path,
        outputs: &[(String, &[u8])],
        hook: F,
    ) -> io::Result<super::WriteOutcome>
    where
        F: FnMut(usize) -> io::Result<()>,
    {
        write_outputs_with_hooks(
            output_directory,
            outputs,
            WriteHooks {
                pre_anchor: || Ok(()),
                before_rename_probe: || Ok(()),
                sync_staging: |_, file: &File| file.sync_all(),
                before_publish: hook,
                after_publication: || Ok(()),
                sync_directory: |directory: &Directory| directory.0.sync_all(),
            },
        )
    }

    #[cfg(test)]
    pub(super) fn write_outputs_after_publication_hook<F>(
        output_directory: &Path,
        outputs: &[(String, &[u8])],
        hook: F,
    ) -> io::Result<super::WriteOutcome>
    where
        F: FnOnce() -> io::Result<()>,
    {
        write_outputs_with_hooks(
            output_directory,
            outputs,
            WriteHooks {
                pre_anchor: || Ok(()),
                before_rename_probe: || Ok(()),
                sync_staging: |_, file: &File| file.sync_all(),
                before_publish: |_| Ok(()),
                after_publication: hook,
                sync_directory: |directory: &Directory| directory.0.sync_all(),
            },
        )
    }

    #[cfg(test)]
    pub(super) fn write_outputs_with_directory_sync_hook<F>(
        output_directory: &Path,
        outputs: &[(String, &[u8])],
        mut hook: F,
    ) -> io::Result<super::WriteOutcome>
    where
        F: FnMut(usize) -> io::Result<()>,
    {
        let mut index = 0;
        write_outputs_with_hooks(
            output_directory,
            outputs,
            WriteHooks {
                pre_anchor: || Ok(()),
                before_rename_probe: || Ok(()),
                sync_staging: |_, file: &File| file.sync_all(),
                before_publish: |_| Ok(()),
                after_publication: || Ok(()),
                sync_directory: |_: &Directory| {
                    let result = hook(index);
                    index += 1;
                    result
                },
            },
        )
    }

    #[cfg(test)]
    pub(super) fn write_outputs_before_rename_probe_hook<F>(
        output_directory: &Path,
        outputs: &[(String, &[u8])],
        hook: F,
    ) -> io::Result<super::WriteOutcome>
    where
        F: FnOnce() -> io::Result<()>,
    {
        write_outputs_with_hooks(
            output_directory,
            outputs,
            WriteHooks {
                pre_anchor: || Ok(()),
                before_rename_probe: hook,
                sync_staging: |_, file: &File| file.sync_all(),
                before_publish: |_| Ok(()),
                after_publication: || Ok(()),
                sync_directory: |directory: &Directory| directory.0.sync_all(),
            },
        )
    }

    #[cfg(test)]
    pub(super) fn unsupported_no_replace_error_for_test() -> io::Error {
        no_replace_error(
            "publish",
            "nested/result.txt",
            io::Error::from_raw_os_error(EINVAL),
        )
    }

    #[cfg(test)]
    pub(super) fn unsupported_no_replace_filesystem_error_for_test() -> io::Error {
        no_replace_error(
            "publish",
            "nested/result.txt",
            io::Error::from_raw_os_error(ENOTSUP_OR_EOPNOTSUPP),
        )
    }
}

#[cfg(test)]
mod tests {
    use super::{DiagnosticFormat, parse_arguments, resolve_bazel_run_input_path};
    use std::ffi::OsString;

    struct FailingWriter(std::io::ErrorKind);

    impl std::io::Write for FailingWriter {
        fn write(&mut self, _buffer: &[u8]) -> std::io::Result<usize> {
            Err(std::io::Error::from(self.0))
        }

        fn flush(&mut self) -> std::io::Result<()> {
            Ok(())
        }
    }

    fn args(values: &[&str]) -> Vec<OsString> {
        values.iter().map(OsString::from).collect()
    }

    #[test]
    fn parses_supported_argument_orders() {
        let options = parse_arguments(args(&[
            "compile",
            "input.circuitc",
            "--diagnostic-format=json",
            "--output-dir",
            "out",
        ]))
        .expect("valid arguments must parse");
        assert_eq!(options.input.to_string_lossy(), "input.circuitc");
        assert_eq!(options.output_directory.to_string_lossy(), "out");
        assert_eq!(options.diagnostic_format, DiagnosticFormat::Json);
    }

    #[test]
    fn rejects_extra_inputs_and_unsupported_options() {
        assert!(parse_arguments(args(&["compile", "a", "b", "--output-dir", "out"])).is_err());
        assert!(
            parse_arguments(args(&["compile", "a", "--output-dir", "out", "--watch"])).is_err()
        );
    }

    #[test]
    fn double_dash_allows_a_leading_dash_input() {
        let options = parse_arguments(args(&[
            "compile",
            "--output-dir",
            "out",
            "--",
            "-divider.circuitc",
        ]))
        .expect("double dash must end option parsing");
        assert_eq!(options.input.to_string_lossy(), "-divider.circuitc");
    }

    #[test]
    fn resolves_bazel_run_inputs_against_the_invoking_workspace() {
        assert_eq!(
            resolve_bazel_run_input_path(
                std::path::Path::new("examples/design.circuitc"),
                Some(std::ffi::OsStr::new("/workspace")),
            ),
            std::path::Path::new("/workspace/examples/design.circuitc")
        );
        assert_eq!(
            resolve_bazel_run_input_path(
                std::path::Path::new("/absolute/design.circuitc"),
                Some(std::ffi::OsStr::new("/workspace")),
            ),
            std::path::Path::new("/absolute/design.circuitc")
        );
    }

    #[test]
    fn checked_failure_directory_appends_suffix_to_the_complete_output_path() {
        assert_eq!(
            super::failure_output_directory(std::path::Path::new("/workspace/build/out")).unwrap(),
            std::path::Path::new("/workspace/build/out.failed")
        );
        let relative = std::env::current_dir().unwrap().join("relative/out.failed");
        assert_eq!(
            super::failure_output_directory(std::path::Path::new("relative/out")).unwrap(),
            relative
        );
        assert_eq!(
            super::failure_output_directory(std::path::Path::new("relative/out/")).unwrap(),
            relative
        );
        assert_eq!(
            super::failure_output_directory(std::path::Path::new("relative/out/.")).unwrap(),
            relative
        );
        assert_eq!(
            super::failure_output_directory(std::path::Path::new("relative/out/../final/."))
                .unwrap(),
            std::env::current_dir()
                .unwrap()
                .join("relative/final.failed")
        );
        assert!(super::failure_output_directory(std::path::Path::new("/")).is_err());
    }

    #[cfg(unix)]
    #[test]
    fn checked_failure_directory_preserves_non_utf8_basename_bytes() {
        use std::os::unix::ffi::{OsStrExt as _, OsStringExt as _};

        let output =
            std::path::PathBuf::from(std::ffi::OsString::from_vec(vec![b'o', b'u', b't', 0xff]));
        let failure = super::failure_output_directory(&output).unwrap();
        assert_eq!(
            failure.file_name().unwrap().as_bytes(),
            &[
                b'o', b'u', b't', 0xff, b'.', b'f', b'a', b'i', b'l', b'e', b'd'
            ]
        );
    }

    #[test]
    fn simulation_chain_is_sorted_and_duplicate_paths_fail_before_publication() {
        let mut outputs = Vec::new();
        super::append_simulation_output_chain(
            &mut outputs,
            [
                ("simulation/id/result.json", b"result"),
                ("simulation/id/analysis.spice", b"netlist"),
                ("simulation/id/report.json", b"report"),
                ("simulation/id/request.json", b"request"),
                ("simulation/id/spice-map.json", b"map"),
            ],
        );
        super::sort_outputs(&mut outputs).expect("distinct output paths must sort");
        assert_eq!(
            outputs
                .iter()
                .map(|(path, _)| path.as_str())
                .collect::<Vec<_>>(),
            vec![
                "simulation/id/analysis.spice",
                "simulation/id/report.json",
                "simulation/id/request.json",
                "simulation/id/result.json",
                "simulation/id/spice-map.json",
            ]
        );

        outputs.push(("simulation/id/result.json".to_owned(), b"duplicate"));
        assert!(super::sort_outputs(&mut outputs).is_err());
    }

    #[cfg(unix)]
    #[test]
    fn compiler_work_root_is_private_external_and_explicitly_cleaned() {
        use std::os::unix::fs::MetadataExt as _;

        let scratch = scratch_directory("compiler-work-root");
        let output = scratch.join("output");
        let work_root =
            super::CompileWorkRoot::create(&output).expect("allocate private compiler work root");
        let path = work_root.path().to_owned();
        assert_eq!(path.parent(), Some(scratch.as_path()));
        assert!(!path.starts_with(&output));
        assert_eq!(
            std::fs::metadata(&path)
                .expect("inspect compiler work root")
                .mode()
                & 0o777,
            0o700
        );
        // SAFETY: `geteuid` has no preconditions and does not dereference pointers.
        assert_eq!(std::fs::metadata(&path).unwrap().uid(), unsafe {
            libc::geteuid()
        });
        let nonce = path
            .file_name()
            .unwrap()
            .to_string_lossy()
            .rsplit('-')
            .next()
            .unwrap()
            .to_owned();
        assert_eq!(nonce.len(), 32);
        assert!(nonce.bytes().all(|byte| byte.is_ascii_hexdigit()));

        work_root
            .cleanup()
            .expect("explicit compiler work-root cleanup must succeed");
        assert!(!path.exists());

        std::fs::create_dir(&output).expect("create existing output root");
        let existing_output_root = super::CompileWorkRoot::create(&output)
            .expect("allocate a sibling beside an existing output root");
        assert_eq!(
            existing_output_root.path().parent(),
            Some(scratch.as_path())
        );
        existing_output_root.cleanup().unwrap();

        let nested_missing_output = scratch.join("missing/nested-output");
        let missing_output_root = super::CompileWorkRoot::create(&nested_missing_output)
            .expect("allocate a sibling of the first missing output component");
        assert_eq!(missing_output_root.path().parent(), Some(scratch.as_path()));
        missing_output_root.cleanup().unwrap();

        let dropped_path = {
            let work_root = super::CompileWorkRoot::create(&output)
                .expect("allocate compiler work root for drop cleanup");
            work_root.path().to_owned()
        };
        assert!(
            !dropped_path.exists(),
            "RAII drop must remove an unconsumed compiler work root"
        );
        std::fs::remove_dir_all(scratch).expect("remove test scratch directory");
    }

    #[cfg(unix)]
    #[test]
    fn compiler_work_root_validates_injected_work_parent_and_uses_random_names() {
        use std::os::unix::fs::PermissionsExt as _;

        let scratch = scratch_directory("compiler-work-parent");
        let private_parent = scratch.join("private");
        std::fs::create_dir(&private_parent).unwrap();
        std::fs::set_permissions(&private_parent, std::fs::Permissions::from_mode(0o700)).unwrap();
        let output = scratch.join("output");

        let first = super::CompileWorkRoot::create_in_parent(&output, &private_parent).unwrap();
        let second = super::CompileWorkRoot::create_in_parent(&output, &private_parent).unwrap();
        let first_path = first.path().to_owned();
        let second_path = second.path().to_owned();
        assert_ne!(first_path, second_path);
        first.cleanup().unwrap();
        second.cleanup().unwrap();

        let sticky_parent = scratch.join("sticky");
        std::fs::create_dir(&sticky_parent).unwrap();
        std::fs::set_permissions(&sticky_parent, std::fs::Permissions::from_mode(0o1777)).unwrap();
        super::CompileWorkRoot::create_in_parent(&output, &sticky_parent)
            .unwrap()
            .cleanup()
            .unwrap();

        let insecure_parent = scratch.join("insecure");
        std::fs::create_dir(&insecure_parent).unwrap();
        std::fs::set_permissions(&insecure_parent, std::fs::Permissions::from_mode(0o777)).unwrap();
        let error = match super::CompileWorkRoot::create_in_parent(&output, &insecure_parent) {
            Ok(root) => {
                root.cleanup().unwrap();
                panic!("non-sticky shared work parent must be rejected")
            }
            Err(error) => error,
        };
        assert_eq!(error.kind(), std::io::ErrorKind::PermissionDenied);
        assert!(!super::CompileWorkRoot::is_output_boundary_error(&error));

        let unsafe_ancestor = scratch.join("unsafe-ancestor");
        let nested_parent = unsafe_ancestor.join("nested-private");
        std::fs::create_dir(&unsafe_ancestor).unwrap();
        std::fs::create_dir(&nested_parent).unwrap();
        std::fs::set_permissions(&unsafe_ancestor, std::fs::Permissions::from_mode(0o777)).unwrap();
        let error = match super::CompileWorkRoot::create_in_parent(&output, &nested_parent) {
            Ok(root) => {
                root.cleanup().unwrap();
                panic!("an unsafe work-parent ancestor must be rejected")
            }
            Err(error) => error,
        };
        assert!(
            error
                .to_string()
                .contains("writable by other users without the sticky bit"),
            "unexpected work-parent ancestor error: {error}"
        );
        std::fs::set_permissions(&unsafe_ancestor, std::fs::Permissions::from_mode(0o700)).unwrap();

        std::fs::remove_dir_all(scratch).unwrap();
    }

    #[cfg(unix)]
    #[test]
    fn compiler_work_root_cleanup_preserves_a_replacement_name() {
        let scratch = scratch_directory("compiler-work-cleanup-replacement");
        let temporary = scratch.join("temporary");
        std::fs::create_dir(&temporary).unwrap();
        let output = scratch.join("output");
        let work_root = super::CompileWorkRoot::create_in_parent(&output, &temporary).unwrap();
        let path = work_root.path().to_owned();
        let moved = scratch.join("moved-original-work-root");
        std::fs::rename(&path, &moved).unwrap();
        std::fs::create_dir(&path).unwrap();
        std::fs::write(path.join("replacement.txt"), b"preserve").unwrap();

        let error = work_root
            .cleanup()
            .expect_err("cleanup must reject a replacement work-root name");
        assert!(
            error
                .to_string()
                .contains("no longer identifies the created directory"),
            "unexpected replacement error: {error}"
        );
        assert_eq!(
            std::fs::read(path.join("replacement.txt")).unwrap(),
            b"preserve"
        );
        assert!(moved.is_dir());

        std::fs::remove_dir_all(&path).unwrap();
        std::fs::remove_dir_all(&moved).unwrap();
        std::fs::remove_dir_all(&scratch).unwrap();
    }

    #[cfg(unix)]
    #[test]
    fn compiler_work_root_cleanup_refuses_unexpected_contents() {
        let scratch = scratch_directory("compiler-work-cleanup-contents");
        let temporary = scratch.join("temporary");
        std::fs::create_dir(&temporary).unwrap();
        let output = scratch.join("output");
        let work_root = super::CompileWorkRoot::create_in_parent(&output, &temporary).unwrap();
        let path = work_root.path().to_owned();
        std::fs::write(path.join("unexpected.txt"), b"preserve").unwrap();

        work_root
            .cleanup()
            .expect_err("unexpected compiler work-root contents must prevent cleanup");
        assert_eq!(
            std::fs::read(path.join("unexpected.txt")).unwrap(),
            b"preserve"
        );

        std::fs::remove_dir_all(&path).unwrap();
        std::fs::remove_dir_all(&scratch).unwrap();
    }

    #[cfg(any(
        all(
            target_os = "linux",
            any(target_arch = "aarch64", target_arch = "x86_64")
        ),
        all(
            target_os = "macos",
            any(target_arch = "aarch64", target_arch = "x86_64")
        ),
    ))]
    #[test]
    fn relative_output_resolution_validates_and_binds_the_complete_cwd_chain() {
        use std::os::unix::fs::PermissionsExt as _;

        let scratch = scratch_directory("relative-output-cwd-chain");
        let unsafe_ancestor = scratch.join("unsafe");
        let pinned_cwd = unsafe_ancestor.join("cwd");
        std::fs::create_dir(&unsafe_ancestor).unwrap();
        std::fs::create_dir(&pinned_cwd).unwrap();
        std::fs::set_permissions(&unsafe_ancestor, std::fs::Permissions::from_mode(0o777)).unwrap();

        let error = super::anchored_output::resolve_relative_output_directory_for_test(
            std::path::Path::new("output"),
            &pinned_cwd,
            &pinned_cwd,
        )
        .expect_err("an unsafe cwd ancestor must fail relative-output resolution");
        assert!(
            error
                .to_string()
                .contains("writable by other users without the sticky bit"),
            "unexpected cwd-ancestor error: {error}"
        );

        std::fs::set_permissions(&unsafe_ancestor, std::fs::Permissions::from_mode(0o700)).unwrap();
        let replacement_cwd = scratch.join("replacement-cwd");
        std::fs::create_dir(&replacement_cwd).unwrap();
        let error = super::anchored_output::resolve_relative_output_directory_for_test(
            std::path::Path::new("output"),
            &pinned_cwd,
            &replacement_cwd,
        )
        .expect_err("the walked cwd must match the descriptor pinned before resolution");
        assert!(
            error
                .to_string()
                .contains("current directory changed while resolving"),
            "unexpected cwd-binding error: {error}"
        );
        assert_eq!(
            super::anchored_output::resolve_relative_output_directory_for_test(
                std::path::Path::new("output"),
                &pinned_cwd,
                &pinned_cwd,
            )
            .unwrap(),
            pinned_cwd.join("output")
        );
        std::fs::remove_dir_all(&scratch).unwrap();
    }

    #[cfg(any(
        all(
            target_os = "linux",
            any(target_arch = "aarch64", target_arch = "x86_64")
        ),
        all(
            target_os = "macos",
            any(target_arch = "aarch64", target_arch = "x86_64")
        ),
    ))]
    #[test]
    fn empty_output_directory_is_rejected_before_relative_resolution() {
        let error =
            super::anchored_output::validate_output_directory_path(std::path::Path::new(""))
                .expect_err("an empty output argument must not resolve to the current directory");
        assert_eq!(error.kind(), std::io::ErrorKind::InvalidInput);
        assert!(error.to_string().contains("must not be empty"));
    }

    #[cfg(any(
        all(
            target_os = "linux",
            any(target_arch = "aarch64", target_arch = "x86_64")
        ),
        all(
            target_os = "macos",
            any(target_arch = "aarch64", target_arch = "x86_64")
        ),
    ))]
    #[test]
    fn compiler_work_root_identity_overlap_rejects_aliases_but_allows_siblings() {
        let root = (1, 1);
        let temporary = (1, 2);
        let candidate = (1, 3);
        let safe = (1, 4);
        let output = (1, 5);
        let temporary_ancestry = [root, temporary];

        assert!(super::anchored_output::work_root_overlaps_output_for_test(
            &temporary_ancestry,
            candidate,
            vec![root, safe, temporary],
            true,
        ));
        assert!(super::anchored_output::work_root_overlaps_output_for_test(
            &temporary_ancestry,
            candidate,
            vec![root, temporary, candidate],
            false,
        ));
        assert!(!super::anchored_output::work_root_overlaps_output_for_test(
            &temporary_ancestry,
            candidate,
            vec![root, temporary, output],
            true,
        ));
        assert!(!super::anchored_output::work_root_overlaps_output_for_test(
            &temporary_ancestry,
            candidate,
            vec![root, temporary],
            false,
        ));
    }

    #[cfg(any(
        all(
            target_os = "linux",
            any(target_arch = "aarch64", target_arch = "x86_64")
        ),
        all(
            target_os = "macos",
            any(target_arch = "aarch64", target_arch = "x86_64")
        ),
    ))]
    #[test]
    fn compiler_work_root_post_create_overlap_is_classified_and_cleaned() {
        let scratch = scratch_directory("compiler-work-post-create-overlap");
        let work_parent = scratch.join("work-parent");
        std::fs::create_dir(&work_parent).unwrap();
        let output = scratch.join("output");

        let error =
            match super::anchored_output::CompilerWorkRoot::create_with_candidate_alias_for_test(
                &output,
                &work_parent,
            ) {
                Ok(root) => {
                    root.cleanup().unwrap();
                    panic!("an injected post-create alias must be rejected")
                }
                Err(error) => error,
            };
        assert!(super::CompileWorkRoot::is_output_boundary_error(&error));
        assert!(
            error
                .to_string()
                .contains("must be outside the output directory")
        );
        assert_eq!(
            std::fs::read_dir(&work_parent).unwrap().count(),
            0,
            "RAII cleanup must remove the rejected owned candidate"
        );
        std::fs::remove_dir_all(scratch).unwrap();
    }

    #[cfg(any(
        all(
            target_os = "linux",
            any(target_arch = "aarch64", target_arch = "x86_64")
        ),
        all(
            target_os = "macos",
            any(target_arch = "aarch64", target_arch = "x86_64")
        ),
    ))]
    #[test]
    fn compiler_work_root_rejects_the_filesystem_root_output() {
        let error = match super::CompileWorkRoot::create(std::path::Path::new("/")) {
            Ok(root) => {
                root.cleanup().unwrap();
                panic!("the filesystem root has no outside sibling")
            }
            Err(error) => error,
        };
        assert!(super::CompileWorkRoot::is_output_boundary_error(&error));
        assert!(error.to_string().contains("must have an outside sibling"));
    }

    #[cfg(all(
        target_os = "macos",
        any(target_arch = "aarch64", target_arch = "x86_64")
    ))]
    #[test]
    fn missing_output_requires_a_mutable_deepest_existing_parent() {
        use std::process::Command;

        let scratch = scratch_directory("missing-output-deny-only-parent");
        let deepest_existing = scratch.join("deepest-existing");
        std::fs::create_dir(&deepest_existing).unwrap();
        let status = Command::new("/bin/chmod")
            .arg("+a")
            .arg("everyone deny delete")
            .arg(&deepest_existing)
            .status()
            .unwrap();
        assert!(status.success());

        let output = deepest_existing.join("missing/nested-output");
        let error = super::anchored_output::validate_output_directory_path(&output)
            .expect_err("the future sibling-creation parent must reject every extended ACL");
        assert!(
            error
                .to_string()
                .contains("has an extended ACL and cannot provide isolated transaction cleanup"),
            "unexpected deepest-existing-parent ACL error: {error}"
        );

        let status = Command::new("/bin/chmod")
            .arg("-N")
            .arg(&deepest_existing)
            .status()
            .unwrap();
        assert!(status.success());
        std::fs::remove_dir_all(scratch).unwrap();
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn compiler_work_root_rejects_an_extended_acl_on_an_injected_work_parent() {
        use std::os::unix::fs::PermissionsExt as _;
        use std::process::Command;

        let scratch = scratch_directory("compiler-work-acl-parent");
        let work_parent = scratch.join("work-parent");
        std::fs::create_dir(&work_parent).unwrap();
        std::fs::set_permissions(&work_parent, std::fs::Permissions::from_mode(0o700)).unwrap();
        let status = Command::new("/bin/chmod")
            .arg("+a")
            .arg("everyone allow add_file,add_subdirectory,delete_child")
            .arg(&work_parent)
            .status()
            .unwrap();
        assert!(status.success());

        let error =
            match super::CompileWorkRoot::create_in_parent(&scratch.join("output"), &work_parent) {
                Ok(root) => {
                    root.cleanup().unwrap();
                    panic!("an extended ACL work parent must be rejected")
                }
                Err(error) => error,
            };
        assert!(
            error
                .to_string()
                .contains("has an extended ACL and cannot provide isolated transaction cleanup"),
            "unexpected compiler ACL error: {error}"
        );
        let status = Command::new("/bin/chmod")
            .arg("-N")
            .arg(&work_parent)
            .status()
            .unwrap();
        assert!(status.success());
        std::fs::remove_dir_all(scratch).unwrap();
    }

    #[cfg(unix)]
    #[test]
    fn injected_compiler_work_parent_refuses_to_equal_the_output_directory() {
        use std::os::unix::fs::PermissionsExt as _;

        let scratch = scratch_directory("compiler-work-boundary");
        let temporary = scratch.join("temporary");
        std::fs::create_dir(&temporary).unwrap();
        std::fs::set_permissions(&temporary, std::fs::Permissions::from_mode(0o700)).unwrap();
        let before = std::fs::read_dir(&temporary)
            .unwrap()
            .filter_map(Result::ok)
            .filter(|entry| {
                entry
                    .file_name()
                    .to_string_lossy()
                    .starts_with("circuitc-compile-")
            })
            .count();
        let error = match super::CompileWorkRoot::create_in_parent(&temporary, &temporary) {
            Ok(root) => {
                root.cleanup().expect("clean unexpected work root");
                panic!("a work parent equal to the output root must be rejected")
            }
            Err(error) => error,
        };
        assert!(
            error
                .to_string()
                .contains("must be outside the output directory")
        );
        assert!(super::CompileWorkRoot::is_output_boundary_error(&error));
        let after = std::fs::read_dir(&temporary)
            .unwrap()
            .filter_map(Result::ok)
            .filter(|entry| {
                entry
                    .file_name()
                    .to_string_lossy()
                    .starts_with("circuitc-compile-")
            })
            .count();
        assert_eq!(
            before, after,
            "fallible validation must not leak work roots"
        );
        std::fs::remove_dir_all(scratch).unwrap();
    }

    #[test]
    fn cleanup_warning_is_reported_without_failing_publication() {
        let output_directory = std::path::Path::new("/output");
        let outputs = [("result.txt".to_owned(), b"generated".as_slice())];
        let write_outcome = super::WriteOutcome {
            cleanup_warning: Some(std::io::Error::other(
                "remove backup for result.txt: residue .result.txt.backup-test",
            )),
        };
        let mut stdout = Vec::new();
        let mut stderr = Vec::new();

        let status = super::report_successful_publication(
            output_directory,
            &outputs,
            &write_outcome,
            &mut stdout,
            &mut stderr,
        );

        assert_eq!(status, Ok(()));
        let stderr = String::from_utf8(stderr).expect("cleanup warning must be UTF-8");
        assert!(stderr.contains("CC-CLI-IO-003"));
        assert!(stderr.contains(".result.txt.backup-test"));
        let stdout = String::from_utf8(stdout).expect("publication output must be UTF-8");
        assert!(stdout.contains("wrote /output/result.txt"));
    }

    #[test]
    fn broken_pipe_while_reporting_does_not_misreport_publication_as_failed() {
        let outputs = [("result.txt".to_owned(), b"generated".as_slice())];
        let write_outcome = super::WriteOutcome {
            cleanup_warning: None,
        };
        let mut stdout = FailingWriter(std::io::ErrorKind::BrokenPipe);
        let mut stderr = Vec::new();

        let status = super::report_successful_publication(
            std::path::Path::new("/output"),
            &outputs,
            &write_outcome,
            &mut stdout,
            &mut stderr,
        );

        assert_eq!(status, Ok(()));
        assert!(stderr.is_empty());
    }

    #[test]
    fn non_pipe_reporting_failure_identifies_already_published_outputs() {
        let outputs = [("result.txt".to_owned(), b"generated".as_slice())];
        let write_outcome = super::WriteOutcome {
            cleanup_warning: None,
        };
        let mut stdout = FailingWriter(std::io::ErrorKind::Other);
        let mut stderr = Vec::new();

        let status = super::report_successful_publication(
            std::path::Path::new("/output"),
            &outputs,
            &write_outcome,
            &mut stdout,
            &mut stderr,
        );

        assert_eq!(status, Err(super::EXIT_IO));
        let stderr = String::from_utf8(stderr).expect("reporting diagnostic must be UTF-8");
        assert!(stderr.contains("CC-CLI-IO-004"));
        assert!(stderr.contains("outputs were published to /output"));
        assert!(stderr.contains("reporting them failed"));
    }

    #[cfg(any(
        all(
            target_os = "linux",
            any(target_arch = "aarch64", target_arch = "x86_64")
        ),
        all(
            target_os = "macos",
            any(target_arch = "aarch64", target_arch = "x86_64")
        ),
    ))]
    #[test]
    fn unsupported_no_replace_rename_reports_operation_and_filename() {
        for error in [
            super::anchored_output::unsupported_no_replace_error_for_test(),
            super::anchored_output::unsupported_no_replace_filesystem_error_for_test(),
        ] {
            assert_eq!(error.kind(), std::io::ErrorKind::Unsupported);
            let message = error.to_string();
            assert!(message.contains("publish nested/result.txt"));
            assert!(message.contains("output filesystem does not support no-replace rename"));
        }
    }

    #[cfg(any(
        all(
            target_os = "linux",
            any(target_arch = "aarch64", target_arch = "x86_64")
        ),
        all(
            target_os = "macos",
            any(target_arch = "aarch64", target_arch = "x86_64")
        ),
    ))]
    #[test]
    fn directory_swap_between_preflight_and_anchoring_fails_closed() {
        use std::fs;
        use std::os::unix::fs::symlink;

        let scratch = scratch_directory("directory-swap");
        let output = scratch.join("output");
        let original_parent = output.join("nested");
        let moved_parent = scratch.join("moved-parent");
        let external = scratch.join("external");
        fs::create_dir_all(&original_parent).expect("create original output parent");
        fs::create_dir_all(&external).expect("create external directory");

        let outputs = [("nested/result.txt".to_owned(), b"generated".as_slice())];
        let result =
            super::anchored_output::write_outputs_after_pre_anchor_hook(&output, &outputs, || {
                fs::rename(&original_parent, &moved_parent)?;
                symlink(&external, &original_parent)
            });

        assert!(
            result.is_err(),
            "a swapped generated parent must fail closed"
        );
        assert!(!external.join("result.txt").exists());
        assert!(!moved_parent.join("result.txt").exists());
        fs::remove_dir_all(&scratch).expect("remove test scratch directory");
    }

    #[cfg(any(
        all(
            target_os = "linux",
            any(target_arch = "aarch64", target_arch = "x86_64")
        ),
        all(
            target_os = "macos",
            any(target_arch = "aarch64", target_arch = "x86_64")
        ),
    ))]
    #[test]
    fn output_directory_symlink_ancestor_fails_closed() {
        use std::fs;
        use std::os::unix::fs::symlink;

        let scratch = scratch_directory("symlink-ancestor");
        let real = scratch.join("real");
        let link = scratch.join("link");
        fs::create_dir_all(&real).expect("create real output ancestor");
        symlink("real", &link).expect("create output ancestor symlink");
        let outputs = [("result.txt".to_owned(), b"generated".as_slice())];

        let error = super::write_outputs(&link.join("out"), &outputs)
            .expect_err("symlinked output ancestor must fail closed");
        let raw_os_error = super::anchored_output::output_directory_component_raw_os_error(&error);
        #[cfg(target_os = "linux")]
        assert!(
            matches!(raw_os_error, Some(20 | 40)),
            "Linux symlinked ancestor must fail with ENOTDIR or ELOOP, got: {error:?}"
        );
        #[cfg(target_os = "macos")]
        assert!(
            matches!(raw_os_error, Some(20 | 62)),
            "macOS symlinked ancestor must fail with ENOTDIR or ELOOP, got: {error:?}"
        );
        assert!(
            error
                .to_string()
                .contains("must be a non-symlinked directory"),
            "unexpected ancestor error: {error}"
        );
        assert!(!real.join("out").exists());
        fs::remove_dir_all(&scratch).expect("remove test scratch directory");
    }

    #[cfg(any(
        all(
            target_os = "linux",
            any(target_arch = "aarch64", target_arch = "x86_64")
        ),
        all(
            target_os = "macos",
            any(target_arch = "aarch64", target_arch = "x86_64")
        ),
    ))]
    #[test]
    fn failed_preflight_does_not_create_generated_directories() {
        use std::fs;

        let scratch = scratch_directory("failed-preflight");
        let output = scratch.join("output");
        fs::create_dir_all(output.join("invalid-target"))
            .expect("create invalid output target directory");
        let outputs = [
            ("nested/result.txt".to_owned(), b"generated".as_slice()),
            ("invalid-target".to_owned(), b"invalid".as_slice()),
        ];

        assert!(super::write_outputs(&output, &outputs).is_err());
        assert!(
            !output.join("nested").exists(),
            "non-mutating preflight must reject later invalid targets before creating parents"
        );
        fs::remove_dir_all(&scratch).expect("remove test scratch directory");
    }

    #[cfg(any(
        all(
            target_os = "linux",
            any(target_arch = "aarch64", target_arch = "x86_64")
        ),
        all(
            target_os = "macos",
            any(target_arch = "aarch64", target_arch = "x86_64")
        ),
    ))]
    #[test]
    fn unsafe_relative_output_paths_fail_before_creating_the_output_root() {
        use std::fs;

        let scratch = scratch_directory("unsafe-relative-path");
        let output = scratch.join("output");
        for name in ["../escape.txt", "/absolute.txt", "", "a/../../escape.txt"] {
            let outputs = [(name.to_owned(), b"generated".as_slice())];
            let error = super::write_outputs(&output, &outputs)
                .expect_err("unsafe generated output path must fail closed");
            assert!(
                error.to_string().contains("is not a safe relative path"),
                "unexpected unsafe-path error for {name:?}: {error}"
            );
        }
        assert!(!scratch.join("escape.txt").exists());
        assert!(
            !output.exists(),
            "rejected publication must not create its output root"
        );
        fs::remove_dir_all(&scratch).expect("remove test scratch directory");
    }

    #[cfg(any(
        all(
            target_os = "linux",
            any(target_arch = "aarch64", target_arch = "x86_64")
        ),
        all(
            target_os = "macos",
            any(target_arch = "aarch64", target_arch = "x86_64")
        ),
    ))]
    #[test]
    fn failed_transaction_removes_descriptor_created_output_root() {
        use std::fs;
        use std::io;

        let scratch = scratch_directory("root-cleanup");
        let output = scratch.join("created/child");
        let outputs = [("result.txt".to_owned(), b"generated".as_slice())];

        let error =
            super::anchored_output::write_outputs_after_pre_anchor_hook(&output, &outputs, || {
                Err(io::Error::other("injected failure after root creation"))
            })
            .expect_err("injected transaction failure must be reported");
        assert!(
            error
                .to_string()
                .contains("injected failure after root creation")
        );
        assert!(
            !scratch.join("created").exists(),
            "failed transaction must remove every output-root directory it created"
        );
        fs::remove_dir_all(&scratch).expect("remove test scratch directory");
    }

    #[cfg(any(
        all(
            target_os = "linux",
            any(target_arch = "aarch64", target_arch = "x86_64")
        ),
        all(
            target_os = "macos",
            any(target_arch = "aarch64", target_arch = "x86_64")
        ),
    ))]
    #[test]
    fn descriptor_anchored_publication_replaces_regular_files() {
        use std::fs;

        let scratch = scratch_directory("replace-regular-file");
        let output = scratch.join("output");
        fs::create_dir(&output).expect("create output directory");
        fs::write(output.join("result.txt"), b"old").expect("write existing output");
        let outputs = [("result.txt".to_owned(), b"new".as_slice())];

        super::write_outputs(&output, &outputs).expect("replace existing regular output");
        assert_eq!(
            fs::read(output.join("result.txt")).expect("read replaced output"),
            b"new"
        );
        assert_eq!(
            fs::read_dir(&output)
                .expect("list output directory")
                .count(),
            1,
            "successful publication must remove temporary and backup entries"
        );
        fs::remove_dir_all(&scratch).expect("remove test scratch directory");
    }

    #[cfg(any(
        all(
            target_os = "linux",
            any(target_arch = "aarch64", target_arch = "x86_64")
        ),
        all(
            target_os = "macos",
            any(target_arch = "aarch64", target_arch = "x86_64")
        ),
    ))]
    #[test]
    fn publication_rejects_a_shared_writable_parent_without_the_sticky_bit() {
        use std::fs;
        use std::os::unix::fs::PermissionsExt;

        let scratch = scratch_directory("unsafe-shared-parent");
        let output = scratch.join("output");
        fs::create_dir(&output).expect("create output directory");
        fs::set_permissions(&output, fs::Permissions::from_mode(0o777))
            .expect("make output directory shared writable");
        let outputs = [("result.txt".to_owned(), b"generated".as_slice())];

        let error = super::write_outputs(&output, &outputs)
            .expect_err("a non-sticky shared-writable output parent must fail closed");
        assert!(
            error
                .to_string()
                .contains("writable by other users without the sticky bit"),
            "unexpected unsafe-parent error: {error}"
        );
        assert!(!output.join("result.txt").exists());
        fs::set_permissions(&output, fs::Permissions::from_mode(0o700))
            .expect("restore output permissions for cleanup");
        fs::remove_dir_all(&scratch).expect("remove test scratch directory");
    }

    #[cfg(any(
        all(
            target_os = "linux",
            any(target_arch = "aarch64", target_arch = "x86_64")
        ),
        all(
            target_os = "macos",
            any(target_arch = "aarch64", target_arch = "x86_64")
        ),
    ))]
    #[test]
    fn publication_rejects_an_unsafe_existing_output_ancestor() {
        use std::fs;
        use std::os::unix::fs::PermissionsExt;

        let scratch = scratch_directory("unsafe-output-ancestor");
        let unsafe_ancestor = scratch.join("unsafe");
        let output = unsafe_ancestor.join("output");
        fs::create_dir(&unsafe_ancestor).expect("create output ancestor");
        fs::create_dir(&output).expect("create output directory");
        fs::set_permissions(&unsafe_ancestor, fs::Permissions::from_mode(0o777))
            .expect("make output ancestor shared writable");
        let outputs = [("result.txt".to_owned(), b"generated".as_slice())];

        let error = super::write_outputs(&output, &outputs)
            .expect_err("an unsafe existing output ancestor must fail closed");
        assert!(
            error
                .to_string()
                .contains("writable by other users without the sticky bit"),
            "unexpected unsafe-ancestor error: {error}"
        );
        assert!(!output.join("result.txt").exists());
        fs::set_permissions(&unsafe_ancestor, fs::Permissions::from_mode(0o700))
            .expect("restore ancestor permissions for cleanup");
        fs::remove_dir_all(&scratch).expect("remove test scratch directory");
    }

    #[cfg(any(
        all(
            target_os = "linux",
            any(target_arch = "aarch64", target_arch = "x86_64")
        ),
        all(
            target_os = "macos",
            any(target_arch = "aarch64", target_arch = "x86_64")
        ),
    ))]
    #[test]
    fn publication_rejects_an_unsafe_existing_generated_parent_ancestor() {
        use std::fs;
        use std::os::unix::fs::PermissionsExt;

        let scratch = scratch_directory("unsafe-generated-parent-ancestor");
        let output = scratch.join("output");
        let unsafe_ancestor = output.join("unsafe");
        fs::create_dir(&output).expect("create output directory");
        fs::create_dir(&unsafe_ancestor).expect("create generated-parent ancestor");
        fs::create_dir(unsafe_ancestor.join("nested")).expect("create generated parent");
        fs::set_permissions(&unsafe_ancestor, fs::Permissions::from_mode(0o777))
            .expect("make generated-parent ancestor shared writable");
        let outputs = [(
            "unsafe/nested/result.txt".to_owned(),
            b"generated".as_slice(),
        )];

        let error = super::write_outputs(&output, &outputs)
            .expect_err("an unsafe generated-parent ancestor must fail closed");
        assert!(
            error
                .to_string()
                .contains("writable by other users without the sticky bit"),
            "unexpected unsafe-ancestor error: {error}"
        );
        assert!(!unsafe_ancestor.join("nested/result.txt").exists());
        fs::set_permissions(&unsafe_ancestor, fs::Permissions::from_mode(0o700))
            .expect("restore ancestor permissions for cleanup");
        fs::remove_dir_all(&scratch).expect("remove test scratch directory");
    }

    #[cfg(any(
        all(
            target_os = "linux",
            any(target_arch = "aarch64", target_arch = "x86_64")
        ),
        all(
            target_os = "macos",
            any(target_arch = "aarch64", target_arch = "x86_64")
        ),
    ))]
    #[test]
    fn publication_allows_a_sticky_shared_writable_parent() {
        use std::fs;
        use std::os::unix::fs::PermissionsExt;

        let scratch = scratch_directory("sticky-shared-parent");
        let output = scratch.join("output");
        fs::create_dir(&output).expect("create output directory");
        fs::set_permissions(&output, fs::Permissions::from_mode(0o1777))
            .expect("make output directory sticky and shared writable");
        let outputs = [("result.txt".to_owned(), b"generated".as_slice())];

        super::write_outputs(&output, &outputs)
            .expect("sticky namespace protection must permit publication");
        assert_eq!(fs::read(output.join("result.txt")).unwrap(), b"generated");
        fs::set_permissions(&output, fs::Permissions::from_mode(0o700))
            .expect("restore output permissions for cleanup");
        fs::remove_dir_all(&scratch).expect("remove test scratch directory");
    }

    #[cfg(all(
        target_os = "macos",
        any(target_arch = "aarch64", target_arch = "x86_64")
    ))]
    #[test]
    fn publication_rejects_a_mode_private_parent_with_an_extended_acl() {
        use std::fs;
        use std::os::unix::fs::PermissionsExt;
        use std::process::Command;

        let scratch = scratch_directory("extended-acl-parent");
        let output = scratch.join("output");
        fs::create_dir(&output).expect("create output directory");
        fs::set_permissions(&output, fs::Permissions::from_mode(0o700))
            .expect("make output mode-private");
        let status = Command::new("/bin/chmod")
            .arg("+a")
            .arg("everyone allow add_file,add_subdirectory,delete_child")
            .arg(&output)
            .status()
            .expect("invoke Darwin ACL editor");
        assert!(status.success(), "install inherited everyone ACL");
        let outputs = [("result.txt".to_owned(), b"generated".as_slice())];

        let error = super::write_outputs(&output, &outputs)
            .expect_err("an extended ACL must fail the private namespace check");
        assert!(
            error
                .to_string()
                .contains("has an extended ACL and cannot provide isolated transaction cleanup"),
            "unexpected ACL error: {error}"
        );
        assert!(!output.join("result.txt").exists());
        let status = Command::new("/bin/chmod")
            .arg("-N")
            .arg(&output)
            .status()
            .expect("remove Darwin ACL");
        assert!(status.success(), "remove inherited everyone ACL");
        fs::remove_dir_all(&scratch).expect("remove test scratch directory");
    }

    #[cfg(all(
        target_os = "macos",
        any(target_arch = "aarch64", target_arch = "x86_64")
    ))]
    #[test]
    fn publication_rejects_a_permissive_acl_on_an_existing_output_ancestor() {
        use std::fs;
        use std::os::unix::fs::PermissionsExt;
        use std::process::Command;

        let scratch = scratch_directory("extended-acl-output-ancestor");
        let ancestor = scratch.join("ancestor");
        let output = ancestor.join("output");
        fs::create_dir(&ancestor).expect("create output ancestor");
        fs::create_dir(&output).expect("create output directory");
        fs::set_permissions(&ancestor, fs::Permissions::from_mode(0o700))
            .expect("make output ancestor mode-private");
        let status = Command::new("/bin/chmod")
            .arg("+a")
            .arg("everyone allow add_file,add_subdirectory,delete_child")
            .arg(&ancestor)
            .status()
            .expect("invoke Darwin ACL editor");
        assert!(status.success(), "install permissive ancestor ACL");
        let outputs = [("result.txt".to_owned(), b"generated".as_slice())];

        let error = super::write_outputs(&output, &outputs)
            .expect_err("a permissive ancestor ACL must fail the namespace check");
        assert!(
            error
                .to_string()
                .contains("has a permissive or unrecognized extended ACL"),
            "unexpected ancestor ACL error: {error}"
        );
        assert!(!output.join("result.txt").exists());
        let status = Command::new("/bin/chmod")
            .arg("-N")
            .arg(&ancestor)
            .status()
            .expect("remove Darwin ACL");
        assert!(status.success(), "remove permissive ancestor ACL");
        fs::remove_dir_all(&scratch).expect("remove test scratch directory");
    }

    #[cfg(all(
        target_os = "macos",
        any(target_arch = "aarch64", target_arch = "x86_64")
    ))]
    #[test]
    fn publication_allows_a_deny_only_acl_on_an_existing_output_ancestor() {
        use std::fs;
        use std::process::Command;

        let scratch = scratch_directory("deny-only-acl-output-ancestor");
        let ancestor = scratch.join("ancestor");
        let output = ancestor.join("output");
        fs::create_dir(&ancestor).expect("create output ancestor");
        fs::create_dir(&output).expect("create output directory");
        let status = Command::new("/bin/chmod")
            .arg("+a")
            .arg("everyone deny delete")
            .arg(&ancestor)
            .status()
            .expect("invoke Darwin ACL editor");
        assert!(status.success(), "install deny-only ancestor ACL");
        let outputs = [("result.txt".to_owned(), b"generated".as_slice())];

        super::write_outputs(&output, &outputs)
            .expect("a deny-only ancestor ACL must preserve namespace isolation");
        assert_eq!(fs::read(output.join("result.txt")).unwrap(), b"generated");
        let status = Command::new("/bin/chmod")
            .arg("-N")
            .arg(&ancestor)
            .status()
            .expect("remove Darwin ACL");
        assert!(status.success(), "remove deny-only ancestor ACL");
        fs::remove_dir_all(&scratch).expect("remove test scratch directory");
    }

    #[cfg(any(
        all(
            target_os = "linux",
            any(target_arch = "aarch64", target_arch = "x86_64")
        ),
        all(
            target_os = "macos",
            any(target_arch = "aarch64", target_arch = "x86_64")
        ),
    ))]
    #[test]
    fn output_target_symlink_has_an_explicit_non_regular_diagnostic() {
        use std::fs;
        use std::os::unix::fs::symlink;

        let scratch = scratch_directory("target-symlink");
        let output = scratch.join("output");
        let external = scratch.join("external.txt");
        fs::create_dir(&output).expect("create output directory");
        fs::write(&external, b"external sentinel").expect("write external sentinel");
        symlink(&external, output.join("result.txt")).expect("create output target symlink");
        let outputs = [("result.txt".to_owned(), b"generated".as_slice())];

        let error = super::write_outputs(&output, &outputs)
            .expect_err("publication must reject an output-target symlink");
        assert!(
            error
                .to_string()
                .contains("output target result.txt exists and is not a regular file"),
            "unexpected target-symlink error: {error}"
        );
        assert_eq!(
            fs::read(&external).expect("read external sentinel"),
            b"external sentinel"
        );
        assert!(
            fs::symlink_metadata(output.join("result.txt"))
                .expect("output target symlink remains")
                .file_type()
                .is_symlink()
        );
        fs::remove_dir_all(&scratch).expect("remove test scratch directory");
    }

    #[cfg(any(
        all(
            target_os = "linux",
            any(target_arch = "aarch64", target_arch = "x86_64")
        ),
        all(
            target_os = "macos",
            any(target_arch = "aarch64", target_arch = "x86_64")
        ),
    ))]
    #[test]
    fn descriptor_anchored_publication_replaces_an_unreadable_regular_file() {
        use std::fs;
        use std::os::unix::fs::PermissionsExt;

        let scratch = scratch_directory("replace-unreadable-file");
        let output = scratch.join("output");
        let target = output.join("result.txt");
        fs::create_dir(&output).expect("create output directory");
        fs::write(&target, b"old").expect("write existing output");
        fs::set_permissions(&target, fs::Permissions::from_mode(0o200))
            .expect("make existing output unreadable");
        let outputs = [("result.txt".to_owned(), b"new".as_slice())];

        super::write_outputs(&output, &outputs)
            .expect("directory authority must permit replacing an unreadable regular file");
        assert_eq!(fs::read(&target).expect("read replaced output"), b"new");
        assert_eq!(
            fs::read_dir(&output)
                .expect("list output directory")
                .count(),
            1,
            "successful publication must remove temporary and backup entries"
        );
        fs::remove_dir_all(&scratch).expect("remove test scratch directory");
    }

    #[cfg(any(
        all(
            target_os = "linux",
            any(target_arch = "aarch64", target_arch = "x86_64")
        ),
        all(
            target_os = "macos",
            any(target_arch = "aarch64", target_arch = "x86_64")
        ),
    ))]
    #[test]
    fn inaccessible_existing_target_fails_with_the_documented_permission_requirement() {
        use std::fs;
        use std::os::unix::fs::{MetadataExt, PermissionsExt};

        let scratch = scratch_directory("inaccessible-existing-file");
        let output = scratch.join("output");
        let target = output.join("result.txt");
        fs::create_dir(&output).expect("create output directory");
        fs::write(&target, b"old").expect("write existing output");
        fs::set_permissions(&target, fs::Permissions::from_mode(0o000))
            .expect("make existing output inaccessible");
        let outputs = [("result.txt".to_owned(), b"new".as_slice())];

        let error = super::write_outputs(&output, &outputs)
            .expect_err("a fully inaccessible target must fail with an explicit requirement");
        assert!(
            error
                .to_string()
                .contains("existing targets must be readable or writable regular files"),
            "unexpected inaccessible-target error: {error}"
        );
        let metadata = fs::metadata(&target).expect("inaccessible target remains");
        assert_eq!(metadata.mode() & 0o777, 0);
        assert_eq!(metadata.len(), 3);
        fs::set_permissions(&target, fs::Permissions::from_mode(0o600))
            .expect("restore target permissions for cleanup");
        assert_eq!(fs::read(&target).expect("read preserved target"), b"old");
        fs::remove_dir_all(&scratch).expect("remove test scratch directory");
    }

    #[cfg(any(
        all(
            target_os = "linux",
            any(target_arch = "aarch64", target_arch = "x86_64")
        ),
        all(
            target_os = "macos",
            any(target_arch = "aarch64", target_arch = "x86_64")
        ),
    ))]
    #[test]
    fn staging_sync_failure_preserves_originals_without_residue() {
        use std::fs;
        use std::io;

        let scratch = scratch_directory("staging-sync-failure");
        let output = scratch.join("output");
        fs::create_dir(&output).expect("create output directory");
        fs::write(output.join("first.txt"), b"old first").expect("write first original");
        fs::write(output.join("second.txt"), b"old second").expect("write second original");
        let outputs = [
            ("first.txt".to_owned(), b"new first".as_slice()),
            ("second.txt".to_owned(), b"new second".as_slice()),
        ];

        let error = super::anchored_output::write_outputs_with_staging_sync_hook(
            &output,
            &outputs,
            |index| {
                if index == 1 {
                    Err(io::Error::other("injected staging sync failure"))
                } else {
                    Ok(())
                }
            },
        )
        .expect_err("staging sync failure must abort publication");

        assert!(
            error
                .to_string()
                .contains("sync temporary output for second.txt"),
            "unexpected staging sync error: {error}"
        );
        assert_eq!(
            fs::read(output.join("first.txt")).expect("read first original"),
            b"old first"
        );
        assert_eq!(
            fs::read(output.join("second.txt")).expect("read second original"),
            b"old second"
        );
        assert_eq!(
            fs::read_dir(&output)
                .expect("list output directory")
                .count(),
            2,
            "failed staging sync must remove every temporary file"
        );
        fs::remove_dir_all(&scratch).expect("remove test scratch directory");
    }

    #[cfg(any(
        all(
            target_os = "linux",
            any(target_arch = "aarch64", target_arch = "x86_64")
        ),
        all(
            target_os = "macos",
            any(target_arch = "aarch64", target_arch = "x86_64")
        ),
    ))]
    #[test]
    fn no_replace_publication_preserves_a_racing_destination() {
        use std::fs;

        let scratch = scratch_directory("no-replace-race");
        let output = scratch.join("output");
        fs::create_dir(&output).expect("create output directory");
        let outputs = [("result.txt".to_owned(), b"generated".as_slice())];
        let sentinel = b"racing writer";

        let error =
            super::anchored_output::write_outputs_before_publish_hook(&output, &outputs, |index| {
                assert_eq!(index, 0);
                fs::write(output.join("result.txt"), sentinel)
            })
            .expect_err("no-replace publication must reject a racing destination");

        assert!(
            error.to_string().contains("publish result.txt"),
            "unexpected no-replace error: {error}"
        );
        assert_eq!(
            fs::read(output.join("result.txt")).expect("read racing destination"),
            sentinel,
            "publication must not clobber the racing writer"
        );
        assert_eq!(
            fs::read_dir(&output)
                .expect("list output directory")
                .count(),
            1,
            "failed no-replace publication must remove its staging entry"
        );
        fs::remove_dir_all(&scratch).expect("remove test scratch directory");
    }

    #[cfg(any(
        all(
            target_os = "linux",
            any(target_arch = "aarch64", target_arch = "x86_64")
        ),
        all(
            target_os = "macos",
            any(target_arch = "aarch64", target_arch = "x86_64")
        ),
    ))]
    #[test]
    fn transaction_rename_probe_fails_before_staging_and_preserves_a_racing_name() {
        use std::fs;

        let scratch = scratch_directory("transaction-rename-probe-race");
        let output = scratch.join("output");
        fs::create_dir(&output).expect("create output directory");
        let outputs = [("result.txt".to_owned(), b"generated".as_slice())];
        let racing_probe = output.join(format!(".result.txt.rename-probe-{}", std::process::id()));

        let error = super::anchored_output::write_outputs_before_rename_probe_hook(
            &output,
            &outputs,
            || fs::write(&racing_probe, b"racing probe writer"),
        )
        .expect_err("the reversible rename probe must reject a racing destination");

        assert!(
            error
                .to_string()
                .contains("probe transaction restore path for result.txt"),
            "unexpected transaction-probe error: {error}"
        );
        assert_eq!(fs::read(&racing_probe).unwrap(), b"racing probe writer");
        assert!(!output.join("result.txt").exists());
        assert_eq!(
            fs::read_dir(&output).unwrap().count(),
            1,
            "probe failure must occur before temporary output staging"
        );
        fs::remove_dir_all(&scratch).expect("remove test scratch directory");
    }

    #[cfg(any(
        all(
            target_os = "linux",
            any(target_arch = "aarch64", target_arch = "x86_64")
        ),
        all(
            target_os = "macos",
            any(target_arch = "aarch64", target_arch = "x86_64")
        ),
    ))]
    #[test]
    fn successful_publication_reports_backup_cleanup_failure_as_a_warning() {
        use std::fs;

        let scratch = scratch_directory("cleanup-warning");
        let output = scratch.join("output");
        fs::create_dir(&output).expect("create output directory");
        fs::write(output.join("result.txt"), b"old").expect("write existing output");
        let outputs = [("result.txt".to_owned(), b"new".as_slice())];
        let backup = output.join(format!(".result.txt.backup-{}", std::process::id()));

        let outcome =
            super::anchored_output::write_outputs_after_publication_hook(&output, &outputs, || {
                fs::remove_file(&backup)?;
                fs::create_dir(&backup)
            })
            .expect("publication must succeed even when backup cleanup later fails");

        let warning = outcome
            .cleanup_warning
            .expect("cleanup failure must remain visible to the caller");
        assert!(
            warning.to_string().contains("remove backup for result.txt"),
            "unexpected cleanup warning: {warning}"
        );
        assert_eq!(
            fs::read(output.join("result.txt")).expect("read published output"),
            b"new"
        );
        assert!(
            backup.is_dir(),
            "injected staging residue must be observable"
        );
        fs::remove_dir(&backup).expect("remove injected backup directory");
        fs::remove_dir_all(&scratch).expect("remove test scratch directory");
    }

    #[cfg(any(
        all(
            target_os = "linux",
            any(target_arch = "aarch64", target_arch = "x86_64")
        ),
        all(
            target_os = "macos",
            any(target_arch = "aarch64", target_arch = "x86_64")
        ),
    ))]
    #[test]
    fn cleanup_quarantine_is_random_caller_owned_and_private() {
        use std::fs;
        use std::os::unix::fs::{MetadataExt, PermissionsExt};

        let scratch = scratch_directory("private-cleanup-quarantine");
        let output = scratch.join("output");
        fs::create_dir(&output).expect("create output directory");
        let outputs = [("result.txt".to_owned(), b"generated".as_slice())];

        super::anchored_output::write_outputs_after_publication_hook(&output, &outputs, || {
            let prefix = format!(".circuitc-transaction-{}-", std::process::id());
            let quarantines: Vec<_> = fs::read_dir(&output)?
                .filter_map(|entry| {
                    let entry = entry.ok()?;
                    entry
                        .file_name()
                        .to_string_lossy()
                        .starts_with(&prefix)
                        .then_some(entry.path())
                })
                .collect();
            assert_eq!(quarantines.len(), 1);
            let name = quarantines[0].file_name().unwrap().to_string_lossy();
            assert_eq!(name.len(), prefix.len() + 32, "nonce must be 128 bits");
            let metadata = fs::metadata(&quarantines[0])?;
            // SAFETY: `geteuid` has no preconditions and does not dereference pointers.
            let effective_uid = unsafe { libc::geteuid() };
            assert_eq!(metadata.uid(), effective_uid);
            assert_eq!(metadata.permissions().mode() & 0o777, 0o700);
            Ok(())
        })
        .expect("publication with a private quarantine must succeed");

        assert_eq!(
            fs::read_dir(&output).unwrap().count(),
            1,
            "successful cleanup must remove the private quarantine"
        );
        fs::remove_dir_all(&scratch).expect("remove test scratch directory");
    }

    #[cfg(any(
        all(
            target_os = "linux",
            any(target_arch = "aarch64", target_arch = "x86_64")
        ),
        all(
            target_os = "macos",
            any(target_arch = "aarch64", target_arch = "x86_64")
        ),
    ))]
    #[test]
    fn nonempty_cleanup_quarantine_is_restored_at_its_original_name() {
        use std::cell::RefCell;
        use std::fs;

        let scratch = scratch_directory("nonempty-cleanup-quarantine");
        let output = scratch.join("output");
        fs::create_dir(&output).expect("create output directory");
        let outputs = [("result.txt".to_owned(), b"generated".as_slice())];
        let quarantine_path = RefCell::new(None);

        let outcome =
            super::anchored_output::write_outputs_after_publication_hook(&output, &outputs, || {
                let prefix = format!(".circuitc-transaction-{}-", std::process::id());
                let path = fs::read_dir(&output)?
                    .filter_map(Result::ok)
                    .find(|entry| entry.file_name().to_string_lossy().starts_with(&prefix))
                    .expect("private quarantine must exist during publication")
                    .path();
                fs::write(path.join("recovery-sentinel"), b"preserve me")?;
                quarantine_path.replace(Some(path));
                Ok(())
            })
            .expect("publication remains successful when quarantine cleanup warns");

        let warning = outcome
            .cleanup_warning
            .expect("nonempty quarantine must warn");
        assert!(
            warning
                .to_string()
                .contains("restored it at its original name"),
            "unexpected quarantine recovery warning: {warning}"
        );
        let quarantine = quarantine_path.into_inner().unwrap();
        assert_eq!(
            fs::read(quarantine.join("recovery-sentinel")).unwrap(),
            b"preserve me"
        );
        assert!(
            fs::read_dir(&output).unwrap().all(|entry| !entry
                .unwrap()
                .file_name()
                .to_string_lossy()
                .contains("directory-cleanup")),
            "failed cleanup must not strand the quarantine under an unnamed alias"
        );
        fs::remove_dir_all(&scratch).expect("remove test scratch directory");
    }

    #[cfg(any(
        all(
            target_os = "linux",
            any(target_arch = "aarch64", target_arch = "x86_64")
        ),
        all(
            target_os = "macos",
            any(target_arch = "aarch64", target_arch = "x86_64")
        ),
    ))]
    #[test]
    fn successful_publication_reports_directory_sync_failure_as_a_warning() {
        use std::cell::Cell;
        use std::fs;
        use std::io;

        let scratch = scratch_directory("directory-sync-warning");
        let output = scratch.join("output");
        fs::create_dir(&output).expect("create output directory");
        let outputs = [("nested/result.txt".to_owned(), b"generated".as_slice())];
        let sync_calls = Cell::new(0);

        let outcome = super::anchored_output::write_outputs_with_directory_sync_hook(
            &output,
            &outputs,
            |index| {
                sync_calls.set(sync_calls.get() + 1);
                if index == 0 {
                    Err(io::Error::other("injected directory sync failure"))
                } else {
                    Ok(())
                }
            },
        )
        .expect("publication must remain successful after a directory sync failure");

        let warning = outcome
            .cleanup_warning
            .expect("directory sync failure must remain visible to the caller");
        assert!(
            warning
                .to_string()
                .contains("published-output directory synchronization was incomplete"),
            "unexpected directory sync warning: {warning}"
        );
        assert!(
            warning
                .to_string()
                .contains("injected directory sync failure")
        );
        assert_eq!(
            fs::read(output.join("nested/result.txt")).expect("read published output"),
            b"generated"
        );
        assert_eq!(
            sync_calls.get(),
            2,
            "publication must sync the artifact parent and its created-directory parent exactly once each"
        );
        fs::remove_dir_all(&scratch).expect("remove test scratch directory");
    }

    #[cfg(any(
        all(
            target_os = "linux",
            any(target_arch = "aarch64", target_arch = "x86_64")
        ),
        all(
            target_os = "macos",
            any(target_arch = "aarch64", target_arch = "x86_64")
        ),
    ))]
    #[test]
    fn mid_publication_failure_restores_originals_without_residue() {
        use std::fs;
        use std::io;

        let scratch = scratch_directory("publication-rollback");
        let output = scratch.join("output");
        fs::create_dir(&output).expect("create output directory");
        fs::write(output.join("first.txt"), b"old first").expect("write first original");
        fs::write(output.join("second.txt"), b"old second").expect("write second original");
        let outputs = [
            ("first.txt".to_owned(), b"new first".as_slice()),
            ("second.txt".to_owned(), b"new second".as_slice()),
        ];

        let error =
            super::anchored_output::write_outputs_before_publish_hook(&output, &outputs, |index| {
                if index == 1 {
                    Err(io::Error::other("injected mid-publication failure"))
                } else {
                    Ok(())
                }
            })
            .expect_err("injected publication failure must be reported");
        assert!(
            error
                .to_string()
                .contains("injected mid-publication failure")
        );
        assert_eq!(
            fs::read(output.join("first.txt")).expect("read restored first output"),
            b"old first"
        );
        assert_eq!(
            fs::read(output.join("second.txt")).expect("read restored second output"),
            b"old second"
        );
        let mut names: Vec<_> = fs::read_dir(&output)
            .expect("list rolled-back output directory")
            .map(|entry| entry.expect("read output entry").file_name())
            .collect();
        names.sort();
        assert_eq!(
            names,
            [
                std::ffi::OsString::from("first.txt"),
                std::ffi::OsString::from("second.txt"),
            ]
        );
        fs::remove_dir_all(&scratch).expect("remove test scratch directory");
    }

    #[cfg(all(
        target_os = "linux",
        any(target_arch = "aarch64", target_arch = "x86_64")
    ))]
    #[test]
    fn descriptor_exhaustion_restores_every_original_without_residue() {
        const CHILD: &str = "CIRCUITC_DESCRIPTOR_EXHAUSTION_CHILD";
        const TEST_NAME: &str =
            "tests::descriptor_exhaustion_restores_every_original_without_residue";

        if std::env::var_os(CHILD).is_none() {
            let output = std::process::Command::new(std::env::current_exe().unwrap())
                .args(["--exact", TEST_NAME, "--nocapture"])
                .env(CHILD, "1")
                .output()
                .expect("run descriptor-exhaustion child test");
            assert!(
                output.status.success(),
                "descriptor-exhaustion child failed:\nstdout:\n{}\nstderr:\n{}",
                String::from_utf8_lossy(&output.stdout),
                String::from_utf8_lossy(&output.stderr)
            );
            return;
        }

        use std::fs;

        const OUTPUT_COUNT: usize = 8;
        let scratch = scratch_directory("publication-fd-exhaustion");
        let output = scratch.join("output");
        fs::create_dir(&output).unwrap();
        let originals: Vec<_> = (0..OUTPUT_COUNT)
            .map(|index| {
                (
                    format!("result-{index:02}.txt"),
                    format!("original-{index:02}").into_bytes(),
                )
            })
            .collect();
        let original_outputs: Vec<_> = originals
            .iter()
            .map(|(name, contents)| (name.clone(), contents.as_slice()))
            .collect();
        super::anchored_output::write_outputs(&output, &original_outputs)
            .expect("publish original outputs before constraining descriptors");

        let replacements: Vec<_> = (0..OUTPUT_COUNT)
            .map(|index| {
                (
                    format!("result-{index:02}.txt"),
                    format!("replacement-{index:02}").into_bytes(),
                )
            })
            .collect();
        let replacement_outputs: Vec<_> = replacements
            .iter()
            .map(|(name, contents)| (name.clone(), contents.as_slice()))
            .collect();

        let mut original_limit = std::mem::MaybeUninit::<libc::rlimit>::uninit();
        // SAFETY: the pointer is valid for one `rlimit` result.
        assert_eq!(
            unsafe { libc::getrlimit(libc::RLIMIT_NOFILE, original_limit.as_mut_ptr()) },
            0
        );
        // SAFETY: `getrlimit` succeeded and initialized the value.
        let original_limit = unsafe { original_limit.assume_init() };
        let baseline = fs::read_dir("/proc/self/fd")
            .unwrap()
            .count()
            .saturating_sub(1);
        let constrained_soft = (baseline + 7 + 2 * OUTPUT_COUNT + 2) as libc::rlim_t;
        assert!(original_limit.rlim_max >= constrained_soft);
        let constrained_limit = libc::rlimit {
            rlim_cur: constrained_soft,
            rlim_max: original_limit.rlim_max,
        };
        // SAFETY: the child process deliberately lowers only its own soft limit.
        assert_eq!(
            unsafe { libc::setrlimit(libc::RLIMIT_NOFILE, &constrained_limit) },
            0
        );
        let publication = super::anchored_output::write_outputs(&output, &replacement_outputs);
        // SAFETY: restore the child process limit before inspecting evidence.
        assert_eq!(
            unsafe { libc::setrlimit(libc::RLIMIT_NOFILE, &original_limit) },
            0
        );

        let error = publication.expect_err("descriptor exhaustion must fail publication");
        assert!(
            error.to_string().contains("Too many open files"),
            "unexpected descriptor-exhaustion diagnostic: {error}"
        );
        for (name, contents) in &originals {
            assert_eq!(
                fs::read(output.join(name)).unwrap(),
                *contents,
                "rollback must restore {name}"
            );
        }
        let mut names: Vec<_> = fs::read_dir(&output)
            .unwrap()
            .map(|entry| entry.unwrap().file_name())
            .collect();
        names.sort();
        let mut expected: Vec<_> = originals
            .iter()
            .map(|(name, _)| std::ffi::OsString::from(name))
            .collect();
        expected.sort();
        assert_eq!(
            names, expected,
            "rollback must leave no transaction residue"
        );

        fs::remove_dir_all(&scratch).unwrap();
    }

    #[cfg(any(
        all(
            target_os = "linux",
            any(target_arch = "aarch64", target_arch = "x86_64")
        ),
        all(
            target_os = "macos",
            any(target_arch = "aarch64", target_arch = "x86_64")
        ),
    ))]
    #[test]
    fn rollback_preserves_racing_replacement_and_original_backup() {
        use std::fs;
        use std::io;

        let scratch = scratch_directory("publication-rollback-race-existing");
        let output = scratch.join("output");
        fs::create_dir(&output).unwrap();
        fs::write(output.join("first.txt"), b"old first").unwrap();
        fs::write(output.join("second.txt"), b"old second").unwrap();
        let outputs = [
            ("first.txt".to_owned(), b"new first".as_slice()),
            ("second.txt".to_owned(), b"new second".as_slice()),
        ];

        let error =
            super::anchored_output::write_outputs_before_publish_hook(&output, &outputs, |index| {
                if index == 1 {
                    require_matching_open_descriptor(&output.join("first.txt"))?;
                    replace_file_with_inode_reuse_pressure(
                        &output.join("first.txt"),
                        b"racing writer",
                    )?;
                    Err(io::Error::other(
                        "injected failure after racing replacement",
                    ))
                } else {
                    Ok(())
                }
            })
            .expect_err("rollback must report a changed published target");

        assert!(
            error
                .to_string()
                .contains("changed before cleanup; refusing to remove it")
        );
        assert_eq!(
            fs::read(output.join("first.txt")).unwrap(),
            b"racing writer"
        );
        assert_eq!(fs::read(output.join("second.txt")).unwrap(), b"old second");
        let backup = output.join(format!(".first.txt.backup-{}", std::process::id()));
        assert_eq!(
            fs::read(&backup).expect("the displaced original remains recoverable"),
            b"old first"
        );

        fs::remove_dir_all(&scratch).unwrap();
    }

    #[cfg(any(
        all(
            target_os = "linux",
            any(target_arch = "aarch64", target_arch = "x86_64")
        ),
        all(
            target_os = "macos",
            any(target_arch = "aarch64", target_arch = "x86_64")
        ),
    ))]
    #[test]
    fn rollback_preserves_racing_replacement_without_an_original_target() {
        use std::fs;
        use std::io;

        let scratch = scratch_directory("publication-rollback-race-new");
        let output = scratch.join("output");
        fs::create_dir(&output).unwrap();
        let outputs = [
            ("first.txt".to_owned(), b"new first".as_slice()),
            ("second.txt".to_owned(), b"new second".as_slice()),
        ];

        let error =
            super::anchored_output::write_outputs_before_publish_hook(&output, &outputs, |index| {
                if index == 1 {
                    require_matching_open_descriptor(&output.join("first.txt"))?;
                    replace_file_with_inode_reuse_pressure(
                        &output.join("first.txt"),
                        b"racing writer",
                    )?;
                    Err(io::Error::other(
                        "injected failure after racing replacement",
                    ))
                } else {
                    Ok(())
                }
            })
            .expect_err("rollback must report a changed published target");

        assert!(
            error
                .to_string()
                .contains("changed before cleanup; refusing to remove it")
        );
        assert_eq!(
            fs::read(output.join("first.txt")).unwrap(),
            b"racing writer"
        );
        assert!(!output.join("second.txt").exists());

        fs::remove_dir_all(&scratch).unwrap();
    }

    #[cfg(any(
        all(
            target_os = "linux",
            any(target_arch = "aarch64", target_arch = "x86_64")
        ),
        all(
            target_os = "macos",
            any(target_arch = "aarch64", target_arch = "x86_64")
        ),
    ))]
    #[test]
    fn rollback_preserves_racing_temporary_replacement() {
        use std::fs;
        use std::io;

        let scratch = scratch_directory("publication-rollback-race-temporary");
        let output = scratch.join("output");
        fs::create_dir(&output).unwrap();
        let outputs = [
            ("first.txt".to_owned(), b"new first".as_slice()),
            ("second.txt".to_owned(), b"new second".as_slice()),
        ];
        let temporary = output.join(format!(".first.txt.tmp-{}", std::process::id()));

        let error =
            super::anchored_output::write_outputs_before_publish_hook(&output, &outputs, |index| {
                if index == 0 {
                    require_matching_open_descriptor(&temporary)?;
                    replace_file_with_inode_reuse_pressure(&temporary, b"racing temporary writer")?;
                    Err(io::Error::other(
                        "injected failure after temporary replacement",
                    ))
                } else {
                    Ok(())
                }
            })
            .expect_err("rollback must report a changed temporary path");

        assert!(
            error
                .to_string()
                .contains("changed before cleanup; refusing to remove it")
        );
        assert_eq!(fs::read(&temporary).unwrap(), b"racing temporary writer");
        assert!(!output.join("first.txt").exists());
        assert!(!output.join("second.txt").exists());

        fs::remove_dir_all(&scratch).unwrap();
    }

    #[cfg(any(
        all(
            target_os = "linux",
            any(target_arch = "aarch64", target_arch = "x86_64")
        ),
        all(
            target_os = "macos",
            any(target_arch = "aarch64", target_arch = "x86_64")
        ),
    ))]
    #[test]
    fn successful_cleanup_preserves_racing_backup_replacement() {
        use std::fs;

        let scratch = scratch_directory("publication-cleanup-race-backup");
        let output = scratch.join("output");
        fs::create_dir(&output).unwrap();
        fs::write(output.join("result.txt"), b"old result").unwrap();
        let outputs = [("result.txt".to_owned(), b"new result".as_slice())];
        let backup = output.join(format!(".result.txt.backup-{}", std::process::id()));

        let outcome =
            super::anchored_output::write_outputs_after_publication_hook(&output, &outputs, || {
                require_matching_open_descriptor(&backup)?;
                replace_file_with_inode_reuse_pressure(&backup, b"racing backup writer")
            })
            .expect("published outputs remain successful when cleanup preserves a changed backup");

        let warning = outcome.cleanup_warning.expect("changed backup must warn");
        assert!(
            warning
                .to_string()
                .contains("changed before cleanup; refusing to remove it")
        );
        assert_eq!(fs::read(output.join("result.txt")).unwrap(), b"new result");
        assert_eq!(fs::read(&backup).unwrap(), b"racing backup writer");

        fs::remove_dir_all(&scratch).unwrap();
    }

    #[cfg(any(
        all(
            target_os = "linux",
            any(target_arch = "aarch64", target_arch = "x86_64")
        ),
        all(
            target_os = "macos",
            any(target_arch = "aarch64", target_arch = "x86_64")
        ),
    ))]
    #[test]
    fn rollback_preserves_racing_created_directory_replacement() {
        use std::fs;
        use std::io;

        let scratch = scratch_directory("publication-rollback-race-directory");
        let output = scratch.join("created/child");
        let displaced = scratch.join("displaced-created");
        let outputs = [("result.txt".to_owned(), b"generated".as_slice())];

        let error =
            super::anchored_output::write_outputs_after_pre_anchor_hook(&output, &outputs, || {
                fs::rename(scratch.join("created"), &displaced)?;
                fs::create_dir(scratch.join("created"))?;
                fs::write(
                    scratch.join("created/sentinel.txt"),
                    b"racing directory writer",
                )?;
                Err(io::Error::other(
                    "injected failure after directory replacement",
                ))
            })
            .expect_err("rollback must report a changed created directory");

        assert!(
            error
                .to_string()
                .contains("changed before cleanup; refusing to remove it")
        );
        assert_eq!(
            fs::read(scratch.join("created/sentinel.txt")).unwrap(),
            b"racing directory writer"
        );
        assert!(
            displaced.exists(),
            "the displaced created inode remains recoverable"
        );

        fs::remove_dir_all(&scratch).unwrap();
    }

    #[cfg(any(
        all(
            target_os = "linux",
            any(target_arch = "aarch64", target_arch = "x86_64")
        ),
        all(
            target_os = "macos",
            any(target_arch = "aarch64", target_arch = "x86_64")
        ),
    ))]
    fn scratch_directory(label: &str) -> std::path::PathBuf {
        use std::sync::atomic::{AtomicU64, Ordering};

        static NEXT_ID: AtomicU64 = AtomicU64::new(0);
        let scratch_base = std::fs::canonicalize(std::env::temp_dir())
            .expect("resolve the test scratch root without symbolic links");
        let path = scratch_base.join(format!(
            "circuitc-cli-{label}-{}-{}",
            std::process::id(),
            NEXT_ID.fetch_add(1, Ordering::Relaxed)
        ));
        let _ = std::fs::remove_dir_all(&path);
        std::fs::create_dir(&path).expect("create unique test scratch directory");
        path
    }

    #[cfg(any(
        all(
            target_os = "linux",
            any(target_arch = "aarch64", target_arch = "x86_64")
        ),
        all(
            target_os = "macos",
            any(target_arch = "aarch64", target_arch = "x86_64")
        ),
    ))]
    fn replace_file_with_inode_reuse_pressure(
        path: &std::path::Path,
        contents: &[u8],
    ) -> std::io::Result<()> {
        #[cfg(target_os = "linux")]
        use std::os::unix::fs::MetadataExt as _;

        #[cfg(target_os = "linux")]
        let original_identity = {
            let original = std::fs::metadata(path)?;
            (original.dev(), original.ino())
        };
        std::fs::remove_file(path)?;

        #[cfg(target_os = "linux")]
        for attempt in 0..256 {
            std::fs::write(path, contents)?;
            let replacement = std::fs::metadata(path)?;
            if (replacement.dev(), replacement.ino()) == original_identity || attempt == 255 {
                return Ok(());
            }
            std::fs::remove_file(path)?;
        }

        #[cfg(target_os = "macos")]
        return std::fs::write(path, contents);

        #[cfg(target_os = "linux")]
        unreachable!("bounded inode-reuse pressure loop always leaves a replacement")
    }

    #[cfg(any(
        all(
            target_os = "linux",
            any(target_arch = "aarch64", target_arch = "x86_64")
        ),
        all(
            target_os = "macos",
            any(target_arch = "aarch64", target_arch = "x86_64")
        ),
    ))]
    fn require_matching_open_descriptor(path: &std::path::Path) -> std::io::Result<()> {
        #[cfg(target_os = "linux")]
        {
            use std::os::unix::fs::MetadataExt as _;

            let expected = std::fs::metadata(path)?;
            let expected_identity = (expected.dev(), expected.ino());
            let retained = std::fs::read_dir("/proc/self/fd")?.any(|entry| {
                entry
                    .ok()
                    .and_then(|entry| std::fs::metadata(entry.path()).ok())
                    .is_some_and(|metadata| (metadata.dev(), metadata.ino()) == expected_identity)
            });
            if !retained {
                return Err(std::io::Error::other(format!(
                    "transaction identity for {} has no retained descriptor",
                    path.display()
                )));
            }
        }

        #[cfg(target_os = "macos")]
        let _ = path;

        Ok(())
    }
}
