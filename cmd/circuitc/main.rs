use std::env;
use std::ffi::OsString;
use std::fs::{self, OpenOptions};
use std::io::{self, Write as _};
use std::path::{Path, PathBuf};
use std::process::{self, ExitCode};

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
    let outputs = [
        (
            format!("{stem}.kicad_pcb"),
            compiled.artifacts.kicad_pcb.as_bytes(),
        ),
        (format!("{stem}.spice"), compiled.artifacts.spice.as_bytes()),
    ];
    write_outputs(&output_directory, &outputs).map_err(|error| {
        eprintln!(
            "CC-CLI-IO-002: failed to write output directory {}: {error}",
            output_directory.display()
        );
        EXIT_IO
    })?;
    for (filename, _) in outputs {
        println!("wrote {}", output_directory.join(filename).display());
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

fn write_outputs(output_directory: &Path, outputs: &[(String, &[u8])]) -> std::io::Result<()> {
    fs::create_dir_all(output_directory)?;
    let mut entries: Vec<_> = outputs
        .iter()
        .map(|(filename, contents)| OutputEntry {
            target: output_directory.join(filename),
            temporary: output_directory.join(format!(".{filename}.tmp-{}", process::id())),
            backup: output_directory.join(format!(".{filename}.backup-{}", process::id())),
            contents,
            had_existing_target: false,
        })
        .collect();

    for entry in &mut entries {
        match fs::symlink_metadata(&entry.target) {
            Ok(metadata) if metadata.file_type().is_file() => entry.had_existing_target = true,
            Ok(_) => {
                return Err(io::Error::other(format!(
                    "output target {} exists and is not a regular file",
                    entry.target.display()
                )));
            }
            Err(error) if error.kind() == io::ErrorKind::NotFound => {}
            Err(error) => return Err(error),
        }
        for staging_path in [&entry.temporary, &entry.backup] {
            if fs::symlink_metadata(staging_path).is_ok() {
                return Err(io::Error::new(
                    io::ErrorKind::AlreadyExists,
                    format!("staging path {} already exists", staging_path.display()),
                ));
            }
        }
    }

    for entry in &entries {
        let result = OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&entry.temporary)
            .and_then(|mut file| file.write_all(entry.contents));
        if let Err(error) = result {
            cleanup_files(entries.iter().map(|entry| entry.temporary.as_path()));
            return Err(error);
        }
    }

    let mut backed_up: Vec<usize> = Vec::new();
    for (index, entry) in entries.iter().enumerate() {
        if entry.had_existing_target {
            if let Err(error) = fs::rename(&entry.target, &entry.backup) {
                restore_backups(&entries, &backed_up);
                cleanup_files(entries.iter().map(|entry| entry.temporary.as_path()));
                return Err(error);
            }
            backed_up.push(index);
        }
    }

    let mut published: Vec<usize> = Vec::new();
    for (index, entry) in entries.iter().enumerate() {
        if let Err(error) = fs::rename(&entry.temporary, &entry.target) {
            cleanup_files(
                published
                    .iter()
                    .map(|published_index| entries[*published_index].target.as_path()),
            );
            restore_backups(&entries, &backed_up);
            cleanup_files(
                entries[index..]
                    .iter()
                    .map(|entry| entry.temporary.as_path()),
            );
            return Err(error);
        }
        published.push(index);
    }
    cleanup_files(
        backed_up
            .iter()
            .map(|index| entries[*index].backup.as_path()),
    );
    Ok(())
}

struct OutputEntry<'a> {
    target: PathBuf,
    temporary: PathBuf,
    backup: PathBuf,
    contents: &'a [u8],
    had_existing_target: bool,
}

fn restore_backups(entries: &[OutputEntry<'_>], backed_up: &[usize]) {
    for index in backed_up.iter().rev() {
        let entry = &entries[*index];
        let _ = fs::rename(&entry.backup, &entry.target);
    }
}

fn cleanup_files<'a>(paths: impl Iterator<Item = &'a Path>) {
    for path in paths {
        let _ = fs::remove_file(path);
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
}
