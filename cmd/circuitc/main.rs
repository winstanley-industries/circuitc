use std::env;
use std::ffi::OsString;
use std::fs;
use std::io;
use std::path::{Component, Path, PathBuf};
use std::process::ExitCode;

use circuitc::frontend::{DiagnosticFormat, compile_source, render_diagnostics};

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
    let input_path = resolve_input_path(
        &options.input,
        env::var_os("BUILD_WORKSPACE_DIRECTORY").as_deref(),
    );
    let output_directory = resolve_input_path(
        &options.output_directory,
        env::var_os("BUILD_WORKSPACE_DIRECTORY").as_deref(),
    );
    let source = fs::read_to_string(&input_path).map_err(|error| {
        eprintln!(
            "CC-CLI-IO-001: failed to read {}: {error}",
            options.input.display()
        );
        EXIT_IO
    })?;
    let compiled = compile_source(filename, source).map_err(|diagnostics| {
        eprint!(
            "{}",
            render_diagnostics(&diagnostics, options.diagnostic_format)
        );
        if options.diagnostic_format == DiagnosticFormat::Human {
            eprintln!();
        }
        EXIT_SOURCE
    })?;

    let stem = &compiled.elaborated.design.name;
    let mut outputs = vec![
        (
            format!("{stem}.kicad_sch"),
            compiled.artifacts.kicad_schematic.as_bytes(),
        ),
        (
            format!("{stem}.kicad_pcb"),
            compiled.artifacts.kicad_pcb.as_bytes(),
        ),
        (
            format!("{stem}.kicad_pro"),
            compiled.artifacts.kicad_project.as_bytes(),
        ),
    ];
    outputs.extend(
        compiled
            .artifacts
            .kicad_library_files
            .iter()
            .map(|file| (file.relative_path.clone(), file.contents.as_bytes())),
    );
    outputs.extend([
        (
            "sym-lib-table".to_owned(),
            compiled.artifacts.kicad_symbol_table.as_bytes(),
        ),
        (
            "fp-lib-table".to_owned(),
            compiled.artifacts.kicad_footprint_table.as_bytes(),
        ),
        (
            format!("{stem}.kicad-map.json"),
            compiled.kicad_identity_map.as_bytes(),
        ),
        (format!("{stem}.spice"), compiled.artifacts.spice.as_bytes()),
    ]);
    let write_outcome = write_outputs(&output_directory, &outputs).map_err(|error| {
        eprintln!(
            "CC-CLI-IO-002: failed to write output directory {}: {error}",
            output_directory.display()
        );
        EXIT_IO
    })?;
    let mut stdout = io::stdout().lock();
    let mut stderr = io::stderr().lock();
    report_successful_publication(
        &output_directory,
        &outputs,
        &write_outcome,
        &mut stdout,
        &mut stderr,
    )
}

fn report_successful_publication(
    output_directory: &Path,
    outputs: &[(String, &[u8])],
    write_outcome: &WriteOutcome,
    stdout: &mut impl io::Write,
    stderr: &mut impl io::Write,
) -> Result<(), u8> {
    if let Some(error) = &write_outcome.cleanup_warning {
        writeln!(
            stderr,
            "CC-CLI-IO-003: output publication to {} succeeded, but backup staging cleanup was incomplete: {error}",
            output_directory.display()
        )
        .map_err(|_| EXIT_IO)?;
    }
    for (filename, _) in outputs {
        writeln!(
            stdout,
            "wrote {}",
            output_directory.join(filename).display()
        )
        .map_err(|_| EXIT_IO)?;
    }
    Ok(())
}

fn resolve_input_path(input: &Path, bazel_workspace: Option<&std::ffi::OsStr>) -> PathBuf {
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
    use std::ffi::{CStr, CString, OsStr, OsString};
    use std::fmt;
    use std::fs::File;
    use std::io::{self, Write as _};
    use std::os::fd::{AsRawFd as _, FromRawFd as _, RawFd};
    use std::os::raw::{c_char, c_int, c_uint};
    use std::os::unix::ffi::OsStrExt as _;
    use std::path::{Component, Path};
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

    const O_RDONLY: c_int = 0;
    const O_WRONLY: c_int = 1;
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
            // SAFETY: `name` is NUL-terminated and remains live for this call.
            let status = unsafe { mkdirat(self.0.as_raw_fd(), name.as_ptr(), 0o777 as Mode) };
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
    }

    struct OutputEntry<'a> {
        parent: Directory,
        target: CString,
        temporary: CString,
        backup: CString,
        display_path: String,
        contents: &'a [u8],
        had_existing_target: bool,
    }

    pub(super) fn write_outputs(
        output_directory: &Path,
        outputs: &[(String, &[u8])],
    ) -> io::Result<super::WriteOutcome> {
        write_outputs_with_hooks(
            output_directory,
            outputs,
            || Ok(()),
            |_, file| file.sync_all(),
            |_| Ok(()),
            || Ok(()),
        )
    }

    fn write_outputs_with_hooks<F, S, G, H>(
        output_directory: &Path,
        outputs: &[(String, &[u8])],
        pre_anchor_hook: F,
        mut sync_staging: S,
        mut before_publish_hook: G,
        after_publication_hook: H,
    ) -> io::Result<super::WriteOutcome>
    where
        F: FnOnce() -> io::Result<()>,
        S: FnMut(usize, &File) -> io::Result<()>,
        G: FnMut(usize) -> io::Result<()>,
        H: FnOnce() -> io::Result<()>,
    {
        let relative_paths: Vec<_> = outputs
            .iter()
            .map(|(filename, _)| super::validate_relative_output_path(filename))
            .collect::<io::Result<_>>()?;
        let (root, mut created_directories) = prepare_output_directory(output_directory)?;

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

        let entries_result = relative_paths
            .iter()
            .zip(outputs)
            .map(|(relative, (_, contents))| {
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
                    remove_created_directories(&created_directories),
                ));
            }
        };

        let pinned_preflight = entries.iter_mut().try_for_each(|entry| {
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
            )
        });
        if let Err(error) = pinned_preflight {
            return Err(with_cleanup_error(
                error,
                remove_created_directories(&created_directories),
            ));
        }

        let mut temporary_files = Vec::new();
        for (index, entry) in entries.iter().enumerate() {
            let mut file = match entry.parent.create_file(&entry.temporary) {
                Ok(file) => file,
                Err(error) => {
                    return Err(with_cleanup_error(
                        operation_error("create temporary output for", &entry.display_path, error),
                        rollback(&entries, &temporary_files, &[], &[], &created_directories),
                    ));
                }
            };
            temporary_files.push(index);
            if let Err(error) = file.write_all(entry.contents) {
                return Err(with_cleanup_error(
                    operation_error("write temporary output for", &entry.display_path, error),
                    rollback(&entries, &temporary_files, &[], &[], &created_directories),
                ));
            }
            if let Err(error) = sync_staging(index, &file) {
                return Err(with_cleanup_error(
                    operation_error("sync temporary output for", &entry.display_path, error),
                    rollback(&entries, &temporary_files, &[], &[], &created_directories),
                ));
            }
        }

        let mut backed_up = Vec::new();
        for (index, entry) in entries.iter().enumerate() {
            if entry.had_existing_target {
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
                            &created_directories,
                        ),
                    ));
                }
                backed_up.push(index);
                match inspect_target(&entry.parent, &entry.backup, Path::new(&entry.display_path)) {
                    Ok(true) => {}
                    Ok(false) => {
                        return Err(with_cleanup_error(
                            io::Error::other(format!(
                                "output target {} disappeared during publication",
                                entry.display_path
                            )),
                            rollback(
                                &entries,
                                &temporary_files,
                                &backed_up,
                                &[],
                                &created_directories,
                            ),
                        ));
                    }
                    Err(error) => {
                        return Err(with_cleanup_error(
                            error,
                            rollback(
                                &entries,
                                &temporary_files,
                                &backed_up,
                                &[],
                                &created_directories,
                            ),
                        ));
                    }
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
                        &created_directories,
                    ),
                ));
            }
            published.push(index);
        }
        let hook_result = after_publication_hook();
        let cleanup_result = cleanup_backups(&entries, &backed_up);
        let cleanup_warning = match hook_result {
            Ok(()) => cleanup_result.err(),
            Err(error) => Some(with_cleanup_error(error, cleanup_result)),
        };
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
        for component in output_directory.components() {
            let segment = match component {
                Component::RootDir | Component::CurDir => continue,
                Component::ParentDir => OsStr::new(".."),
                Component::Normal(segment) => segment,
                Component::Prefix(_) => {
                    return Err(io::Error::new(
                        io::ErrorKind::InvalidInput,
                        "output directory contains an unsupported path prefix",
                    ));
                }
            };
            let name = c_name(segment)?;
            match current.open_child(&name) {
                Ok(child) => current = child,
                Err(error)
                    if error.kind() == io::ErrorKind::NotFound
                        && !matches!(component, Component::ParentDir) =>
                {
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
                    if child_created {
                        created.push(CreatedDirectory {
                            parent: cleanup_parent,
                            name: name.clone(),
                        });
                    }
                    match current.open_child(&name) {
                        Ok(child) => current = child,
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
        for component in relative.components() {
            let Component::Normal(segment) = component else {
                return Err(io::Error::new(
                    io::ErrorKind::InvalidInput,
                    "generated output parent is not a safe relative path",
                ));
            };
            let name = c_name(segment)?;
            match current.open_child(&name) {
                Ok(child) => current = child,
                Err(error) if error.kind() == io::ErrorKind::NotFound => {
                    let cleanup_parent = current.try_clone()?;
                    if current.create_child(&name)? {
                        created.push(CreatedDirectory {
                            parent: cleanup_parent,
                            name: name.clone(),
                        });
                    }
                    current = current.open_child(&name)?;
                }
                Err(error) => return Err(error),
            }
        }
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

    fn rollback(
        entries: &[OutputEntry<'_>],
        temporary_files: &[usize],
        backed_up: &[usize],
        published: &[usize],
        created_directories: &[CreatedDirectory],
    ) -> io::Result<()> {
        let mut errors = Vec::new();
        for index in published.iter().rev() {
            let entry = &entries[*index];
            record_cleanup_error(
                &mut errors,
                &format!("remove published output {}", entry.display_path),
                entry.parent.remove_file(&entry.target),
            );
        }
        for index in backed_up.iter().rev() {
            let entry = &entries[*index];
            record_cleanup_error(
                &mut errors,
                &format!("restore original output {}", entry.display_path),
                rename_noreplace_with_context(
                    &entry.parent,
                    &entry.backup,
                    &entry.parent,
                    &entry.target,
                    "restore backup for",
                    &entry.display_path,
                ),
            );
        }
        for index in temporary_files {
            if published.contains(index) {
                continue;
            }
            let entry = &entries[*index];
            record_cleanup_error(
                &mut errors,
                &format!("remove temporary output {}", entry.display_path),
                entry.parent.remove_file(&entry.temporary),
            );
        }
        record_cleanup_error(
            &mut errors,
            "remove transaction-created directories",
            remove_created_directories(created_directories),
        );
        cleanup_result("output rollback", errors)
    }

    fn cleanup_backups(entries: &[OutputEntry<'_>], backed_up: &[usize]) -> io::Result<()> {
        let mut errors = Vec::new();
        for index in backed_up {
            let entry = &entries[*index];
            record_cleanup_error(
                &mut errors,
                &format!("remove backup for {}", entry.display_path),
                entry.parent.remove_file(&entry.backup),
            );
        }
        cleanup_result("published-output backup cleanup", errors)
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
                directory.parent.remove_directory(&directory.name),
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
        if matches!(error.raw_os_error(), Some(EINVAL | ENOSYS)) {
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
            hook,
            |_, file| file.sync_all(),
            |_| Ok(()),
            || Ok(()),
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
            || Ok(()),
            |index, _| hook(index),
            |_| Ok(()),
            || Ok(()),
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
            || Ok(()),
            |_, file| file.sync_all(),
            hook,
            || Ok(()),
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
            || Ok(()),
            |_, file| file.sync_all(),
            |_| Ok(()),
            hook,
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
}

#[cfg(test)]
mod tests {
    use super::{DiagnosticFormat, parse_arguments, resolve_input_path};
    use std::ffi::OsString;

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
            resolve_input_path(
                std::path::Path::new("examples/design.circuitc"),
                Some(std::ffi::OsStr::new("/workspace")),
            ),
            std::path::Path::new("/workspace/examples/design.circuitc")
        );
        assert_eq!(
            resolve_input_path(
                std::path::Path::new("/absolute/design.circuitc"),
                Some(std::ffi::OsStr::new("/workspace")),
            ),
            std::path::Path::new("/absolute/design.circuitc")
        );
        assert_eq!(
            resolve_input_path(
                std::path::Path::new("out"),
                Some(std::ffi::OsStr::new("/workspace")),
            ),
            std::path::Path::new("/workspace/out")
        );
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
        let error = super::anchored_output::unsupported_no_replace_error_for_test();
        assert_eq!(error.kind(), std::io::ErrorKind::Unsupported);
        let message = error.to_string();
        assert!(message.contains("publish nested/result.txt"));
        assert!(message.contains("output filesystem does not support no-replace rename"));
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
}
