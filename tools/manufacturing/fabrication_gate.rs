use std::collections::BTreeMap;
use std::env;
use std::ffi::{CStr, CString};
use std::fs::{File, Metadata, OpenOptions};
use std::io::{Read, Write};
use std::os::fd::{AsRawFd, FromRawFd};
use std::os::unix::ffi::{OsStrExt, OsStringExt};
use std::os::unix::fs::MetadataExt;
use std::os::unix::fs::OpenOptionsExt;
use std::path::{Component, Path, PathBuf};
use std::process::ExitCode;

use circuitc::frontend::{CheckedCompiledSource, compile_source_checked};
use circuitc::manufacturing::{
    FabricationCompilerArtifacts, FabricationHostFile, bind_kicad10_fabrication,
    prepare_kicad10_fabrication_request, verify_kicad10_fabrication_manifest,
};
use circuitc::product::compile_product_artifacts;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

const ANALYSIS_PATH: &str = "release.manufacturability";
const ASSERTION_PATH: &str = "release.manufacturability.fabrication";
const MAX_FILE_BYTES: u64 = 64 * 1024 * 1024;
const MAX_RAW_AGGREGATE_BYTES: u64 = 256 * 1024 * 1024;

struct CompilerWorkRootOwnership {
    parent: File,
    directory: File,
    name: CString,
    device: u64,
    inode: u64,
}

struct CompilerWorkRoot {
    path: PathBuf,
    ownership: Option<CompilerWorkRootOwnership>,
}

impl CompilerWorkRoot {
    fn create() -> Result<Self, String> {
        #[cfg(target_os = "linux")]
        let parent_path = Path::new("/tmp");
        #[cfg(target_os = "macos")]
        let parent_path = Path::new("/private/tmp");
        let parent = OpenOptions::new()
            .read(true)
            .custom_flags(directory_flags())
            .open(parent_path)
            .map_err(|error| format!("failed to open trusted temporary root: {error}"))?;
        let parent_metadata = parent
            .metadata()
            .map_err(|error| format!("failed to inspect trusted temporary root: {error}"))?;
        if !parent_metadata.is_dir()
            || parent_metadata.uid() != 0
            || parent_metadata.mode() & 0o7777 != 0o1777
        {
            return Err("trusted temporary root is not a root-owned sticky directory".to_owned());
        }
        let template_path = parent_path.join("circuitc-fabrication-XXXXXX");
        let mut template = template_path.as_os_str().as_bytes().to_vec();
        template.push(0);
        // SAFETY: `template` is writable, NUL-terminated, and ends in the six
        // `X` bytes required by `mkdtemp`; its allocation remains live here.
        let created = unsafe { libc::mkdtemp(template.as_mut_ptr().cast()) };
        if created.is_null() {
            return Err(format!(
                "failed to create private compiler work root: {}",
                std::io::Error::last_os_error()
            ));
        }
        // SAFETY: successful `mkdtemp` returns a pointer into the still-live,
        // NUL-terminated `template` allocation.
        let path = PathBuf::from(std::ffi::OsString::from_vec(
            unsafe { CStr::from_ptr(created) }.to_bytes().to_vec(),
        ));
        let basename = path
            .file_name()
            .and_then(|name| name.to_str())
            .ok_or_else(|| "compiler work-root name is not UTF-8".to_owned())?;
        let name = CString::new(basename)
            .map_err(|_| "compiler work-root name contains NUL".to_owned())?;
        let directory = match open_directory_at(&parent, basename) {
            Ok(directory) => directory,
            Err(error) => {
                let _ = remove_directory_at(&parent, &name);
                return Err(error);
            }
        };
        let metadata = match directory.metadata() {
            Ok(metadata) => metadata,
            Err(error) => {
                let _ = remove_directory_at(&parent, &name);
                return Err(format!("failed to inspect compiler work root: {error}"));
            }
        };
        // SAFETY: `geteuid` has no preconditions and reads process state.
        let effective_uid = unsafe { libc::geteuid() };
        if metadata.uid() != effective_uid || metadata.mode() & 0o777 != 0o700 {
            let _ = remove_directory_at(&parent, &name);
            return Err("compiler work root is not a caller-owned 0700 directory".to_owned());
        }
        Ok(Self {
            path,
            ownership: Some(CompilerWorkRootOwnership {
                parent,
                directory,
                name,
                device: metadata.dev(),
                inode: metadata.ino(),
            }),
        })
    }

    fn path(&self) -> &Path {
        &self.path
    }

    fn cleanup(mut self) -> Result<(), String> {
        self.remove_owned_directory()?;
        self.ownership.take();
        Ok(())
    }

    fn remove_owned_directory(&self) -> Result<(), String> {
        let ownership = self
            .ownership
            .as_ref()
            .expect("compiler work-root ownership is live");
        let runner_root = CString::new("circuitc-ohmnivore-work").expect("literal has no NUL");
        match remove_directory_at(&ownership.directory, &runner_root) {
            Ok(()) => {}
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(error) => {
                return Err(format!(
                    "failed to remove compiler runner work directory: {error}"
                ));
            }
        }
        let current = open_directory_at(
            &ownership.parent,
            ownership
                .name
                .to_str()
                .map_err(|_| "compiler work-root name is not UTF-8 during cleanup".to_owned())?,
        )?;
        let metadata = current
            .metadata()
            .map_err(|error| format!("failed to inspect compiler work root at cleanup: {error}"))?;
        if metadata.dev() != ownership.device || metadata.ino() != ownership.inode {
            return Err(
                "compiler work-root name no longer identifies the owned directory".to_owned(),
            );
        }
        remove_directory_at(&ownership.parent, &ownership.name)
            .map_err(|error| format!("failed to remove compiler work root: {error}"))
    }
}

impl Drop for CompilerWorkRoot {
    fn drop(&mut self) {
        if self.ownership.is_some() && self.remove_owned_directory().is_ok() {
            self.ownership.take();
        }
    }
}

fn compile_authenticated_source(
    source_path: &str,
    source: String,
) -> Result<CheckedCompiledSource, String> {
    let work_root = CompilerWorkRoot::create()?;
    let compiled = compile_source_checked(source_path, source, work_root.path()).map_err(|error| {
        error
            .diagnostics
            .into_iter()
            .map(|diagnostic| diagnostic.to_string())
            .collect::<Vec<_>>()
            .join("\n")
    });
    work_root.cleanup()?;
    compiled
}

fn fabrication_compiler_artifacts(
    compiled: &CheckedCompiledSource,
) -> FabricationCompilerArtifacts<'_> {
    if compiled.elaborated.design.analyses.is_empty()
        && compiled.elaborated.design.board.routing_requests.is_empty()
    {
        FabricationCompilerArtifacts::Static(compiled.artifacts.static_artifacts())
    } else {
        FabricationCompilerArtifacts::Checked(&compiled.artifacts)
    }
}

#[derive(Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct ReceiptOutput {
    path: String,
    sha256: String,
}

#[derive(Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct HostReceipt {
    schema_name: String,
    schema_version: u32,
    request_sha256: String,
    board_sha256: String,
    executable_sha256: String,
    outputs: Vec<ReceiptOutput>,
}

fn sha256_hex(bytes: &[u8]) -> String {
    let mut hasher = Sha256::new();
    hasher.update(bytes);
    hasher
        .finalize()
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect()
}

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

fn file_identity(metadata: &Metadata) -> FileIdentity {
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

struct BoundedFile {
    file: File,
    identity: FileIdentity,
}

fn open_bounded(path: &str, maximum: u64) -> Result<BoundedFile, String> {
    let file = OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_NOFOLLOW | libc::O_CLOEXEC)
        .open(path)
        .map_err(|error| format!("failed to open bounded regular file {path}: {error}"))?;
    let metadata = file
        .metadata()
        .map_err(|error| format!("failed to inspect {path}: {error}"))?;
    if !metadata.is_file() || metadata.len() > maximum {
        return Err(format!("input is not a bounded regular file: {path}"));
    }
    Ok(BoundedFile {
        identity: file_identity(&metadata),
        file,
    })
}

fn read_bounded(path: &str, maximum: u64) -> Result<Vec<u8>, String> {
    let mut bounded = open_bounded(path, maximum)?;
    let mut contents = Vec::with_capacity(
        usize::try_from(bounded.identity.length)
            .map_err(|_| format!("input is too large: {path}"))?,
    );
    (&mut bounded.file)
        .take(maximum + 1)
        .read_to_end(&mut contents)
        .map_err(|error| format!("failed to read {path}: {error}"))?;
    let after = bounded
        .file
        .metadata()
        .map_err(|error| format!("failed to re-inspect {path}: {error}"))?;
    if contents.len() as u64 != bounded.identity.length || file_identity(&after) != bounded.identity
    {
        return Err(format!("input changed while it was read: {path}"));
    }
    Ok(contents)
}

fn directory_flags() -> i32 {
    libc::O_RDONLY | libc::O_DIRECTORY | libc::O_NOFOLLOW | libc::O_CLOEXEC
}

fn open_directory_at(parent: &File, name: &str) -> Result<File, String> {
    let name = CString::new(name).map_err(|_| "directory name contains NUL".to_owned())?;
    let descriptor = unsafe { libc::openat(parent.as_raw_fd(), name.as_ptr(), directory_flags()) };
    if descriptor < 0 {
        return Err(format!(
            "failed to open no-follow directory: {}",
            std::io::Error::last_os_error()
        ));
    }
    Ok(unsafe { File::from_raw_fd(descriptor) })
}

fn remove_directory_at(parent: &File, name: &CStr) -> std::io::Result<()> {
    // SAFETY: `parent` is a live directory descriptor, `name` is
    // NUL-terminated, and `AT_REMOVEDIR` restricts the operation to a directory.
    if unsafe { libc::unlinkat(parent.as_raw_fd(), name.as_ptr(), libc::AT_REMOVEDIR) } == 0 {
        Ok(())
    } else {
        Err(std::io::Error::last_os_error())
    }
}

fn open_anchored_directory(path: &Path) -> Result<File, String> {
    let start = if path.is_absolute() { "/" } else { "." };
    let mut current = OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_DIRECTORY | libc::O_NOFOLLOW | libc::O_CLOEXEC)
        .open(start)
        .map_err(|error| format!("failed to open directory anchor {start}: {error}"))?;
    for component in path.components() {
        match component {
            Component::RootDir | Component::CurDir => {}
            Component::Normal(name) => {
                let name = name
                    .to_str()
                    .ok_or_else(|| "directory path contains non-UTF-8 component".to_owned())?;
                current = open_directory_at(&current, name)?;
            }
            Component::ParentDir | Component::Prefix(_) => {
                return Err("directory path contains a non-anchored component".to_owned());
            }
        }
    }
    Ok(current)
}

struct DirectoryStream(*mut libc::DIR);

impl Drop for DirectoryStream {
    fn drop(&mut self) {
        unsafe {
            libc::closedir(self.0);
        }
    }
}

#[cfg(target_os = "macos")]
unsafe fn errno_pointer() -> *mut libc::c_int {
    unsafe { libc::__error() }
}

#[cfg(not(target_os = "macos"))]
unsafe fn errno_pointer() -> *mut libc::c_int {
    unsafe { libc::__errno_location() }
}

fn directory_names(directory: &File) -> Result<Vec<String>, String> {
    let duplicate = unsafe { libc::fcntl(directory.as_raw_fd(), libc::F_DUPFD_CLOEXEC, 0) };
    if duplicate < 0 {
        return Err(format!(
            "failed to duplicate directory descriptor: {}",
            std::io::Error::last_os_error()
        ));
    }
    let pointer = unsafe { libc::fdopendir(duplicate) };
    if pointer.is_null() {
        unsafe {
            libc::close(duplicate);
        }
        return Err(format!(
            "failed to enumerate held directory: {}",
            std::io::Error::last_os_error()
        ));
    }
    let stream = DirectoryStream(pointer);
    unsafe {
        libc::rewinddir(stream.0);
    }
    let mut names = Vec::new();
    loop {
        unsafe {
            *errno_pointer() = 0;
        }
        let entry = unsafe { libc::readdir(stream.0) };
        if entry.is_null() {
            let error_number = unsafe { *errno_pointer() };
            if error_number != 0 {
                return Err(format!(
                    "failed while enumerating held directory: {}",
                    std::io::Error::from_raw_os_error(error_number)
                ));
            }
            break;
        }
        let name = unsafe { CStr::from_ptr((*entry).d_name.as_ptr()) };
        if name.to_bytes() == b"." || name.to_bytes() == b".." {
            continue;
        }
        names.push(
            std::str::from_utf8(name.to_bytes())
                .map_err(|_| "inventory contains a non-UTF-8 name".to_owned())?
                .to_owned(),
        );
    }
    names.sort();
    Ok(names)
}

struct HeldInput {
    directory: &'static str,
    basename: String,
    file: File,
    identity: FileIdentity,
}

fn open_regular_at(
    directory: &'static str,
    parent: &File,
    basename: &str,
    maximum: u64,
) -> Result<HeldInput, String> {
    if basename.is_empty() || basename.contains('/') {
        return Err("raw basename is not canonical".to_owned());
    }
    let name = CString::new(basename).map_err(|_| "raw basename contains NUL".to_owned())?;
    let descriptor = unsafe {
        libc::openat(
            parent.as_raw_fd(),
            name.as_ptr(),
            libc::O_RDONLY | libc::O_NOFOLLOW | libc::O_CLOEXEC,
        )
    };
    if descriptor < 0 {
        return Err(format!(
            "failed to open bounded raw file {directory}/{basename}: {}",
            std::io::Error::last_os_error()
        ));
    }
    let file = unsafe { File::from_raw_fd(descriptor) };
    let metadata = file
        .metadata()
        .map_err(|error| format!("failed to inspect raw file {directory}/{basename}: {error}"))?;
    if !metadata.is_file() || metadata.nlink() != 1 || metadata.len() > maximum {
        return Err(format!(
            "raw input is not a bounded single-link regular file: {directory}/{basename}"
        ));
    }
    Ok(HeldInput {
        directory,
        basename: basename.to_owned(),
        identity: file_identity(&metadata),
        file,
    })
}

fn read_held_input(input: &mut HeldInput, maximum: u64) -> Result<Vec<u8>, String> {
    let mut contents =
        Vec::with_capacity(usize::try_from(input.identity.length).map_err(|_| {
            format!(
                "raw input is too large: {}/{}",
                input.directory, input.basename
            )
        })?);
    (&mut input.file)
        .take(maximum + 1)
        .read_to_end(&mut contents)
        .map_err(|error| {
            format!(
                "failed to read raw input {}/{}: {error}",
                input.directory, input.basename
            )
        })?;
    let after = input.file.metadata().map_err(|error| {
        format!(
            "failed to re-inspect raw input {}/{}: {error}",
            input.directory, input.basename
        )
    })?;
    if contents.len() as u64 != input.identity.length || file_identity(&after) != input.identity {
        return Err(format!(
            "raw input changed while it was read: {}/{}",
            input.directory, input.basename
        ));
    }
    Ok(contents)
}

struct RawDirectories {
    root_path: PathBuf,
    root: File,
    root_identity: FileIdentity,
    gerber: File,
    gerber_identity: FileIdentity,
    drill: File,
    drill_identity: FileIdentity,
    position: File,
    position_identity: FileIdentity,
    receipt: File,
    receipt_identity: FileIdentity,
}

impl RawDirectories {
    fn open(
        raw_root: PathBuf,
        expected_paths: &[circuitc::RelativeArtifactPath],
    ) -> Result<Self, String> {
        let root = open_anchored_directory(&raw_root)?;
        if directory_names(&root)? != ["drill", "gerber", "position", "receipt"] {
            return Err("raw fabrication root inventory is not exact".to_owned());
        }
        let gerber = open_directory_at(&root, "gerber")?;
        let drill = open_directory_at(&root, "drill")?;
        let position = open_directory_at(&root, "position")?;
        let receipt = open_directory_at(&root, "receipt")?;
        let directories = Self {
            root_path: raw_root,
            root_identity: file_identity(&root.metadata().map_err(|error| error.to_string())?),
            gerber_identity: file_identity(&gerber.metadata().map_err(|error| error.to_string())?),
            drill_identity: file_identity(&drill.metadata().map_err(|error| error.to_string())?),
            position_identity: file_identity(
                &position.metadata().map_err(|error| error.to_string())?,
            ),
            receipt_identity: file_identity(
                &receipt.metadata().map_err(|error| error.to_string())?,
            ),
            root,
            gerber,
            drill,
            position,
            receipt,
        };
        directories.verify_inventory(expected_paths)?;
        Ok(directories)
    }

    fn directory(&self, name: &str) -> Result<&File, String> {
        match name {
            "gerber" => Ok(&self.gerber),
            "drill" => Ok(&self.drill),
            "position" => Ok(&self.position),
            "receipt" => Ok(&self.receipt),
            _ => Err(format!("unsupported raw directory: {name}")),
        }
    }

    fn expected_names(
        &self,
        expected_paths: &[circuitc::RelativeArtifactPath],
        directory: &str,
    ) -> Result<Vec<String>, String> {
        let mut expected = expected_paths
            .iter()
            .filter_map(|path| {
                path.as_str()
                    .strip_prefix(&format!("{directory}/"))
                    .map(ToOwned::to_owned)
            })
            .collect::<Vec<_>>();
        if expected
            .iter()
            .any(|name| name.is_empty() || name.contains('/'))
        {
            return Err("request contains a noncanonical raw output path".to_owned());
        }
        expected.sort();
        Ok(expected)
    }

    fn verify_inventory(
        &self,
        expected_paths: &[circuitc::RelativeArtifactPath],
    ) -> Result<(), String> {
        if directory_names(&self.root)? != ["drill", "gerber", "position", "receipt"] {
            return Err("raw fabrication root inventory is not exact".to_owned());
        }
        for directory in ["gerber", "drill", "position"] {
            if directory_names(self.directory(directory)?)?
                != self.expected_names(expected_paths, directory)?
            {
                return Err(format!("raw {directory} inventory is not exact"));
            }
        }
        if directory_names(&self.receipt)? != ["host.json"] {
            return Err("raw host receipt inventory is not exact".to_owned());
        }
        Ok(())
    }

    fn verify_identity(
        &self,
        expected_paths: &[circuitc::RelativeArtifactPath],
        inputs: &[HeldInput],
    ) -> Result<(), String> {
        self.verify_inventory(expected_paths)?;
        let reopened_root = open_anchored_directory(&self.root_path)?;
        if file_identity(
            &reopened_root
                .metadata()
                .map_err(|error| error.to_string())?,
        ) != self.root_identity
        {
            return Err("raw fabrication root was replaced during verification".to_owned());
        }
        for (name, held, expected) in [
            ("gerber", &self.gerber, self.gerber_identity),
            ("drill", &self.drill, self.drill_identity),
            ("position", &self.position, self.position_identity),
            ("receipt", &self.receipt, self.receipt_identity),
        ] {
            if file_identity(&held.metadata().map_err(|error| error.to_string())?) != expected
                || file_identity(
                    &open_directory_at(&self.root, name)?
                        .metadata()
                        .map_err(|error| error.to_string())?,
                ) != expected
            {
                return Err(format!(
                    "raw {name} directory was replaced during verification"
                ));
            }
        }
        for input in inputs {
            if file_identity(&input.file.metadata().map_err(|error| error.to_string())?)
                != input.identity
            {
                return Err(format!(
                    "raw input identity changed: {}/{}",
                    input.directory, input.basename
                ));
            }
            let reopened = open_regular_at(
                input.directory,
                self.directory(input.directory)?,
                &input.basename,
                input.identity.length,
            )?;
            if reopened.identity != input.identity {
                return Err(format!(
                    "raw input was replaced: {}/{}",
                    input.directory, input.basename
                ));
            }
        }
        Ok(())
    }
}

fn run_prepare(arguments: &[String]) -> Result<(), String> {
    let [source_path, catalog_path, variant_path, board_path] = arguments else {
        return Err("usage: fabrication_gate prepare SOURCE CATALOG VARIANT BOARD".to_owned());
    };
    let source = String::from_utf8(read_bounded(source_path, MAX_FILE_BYTES)?)
        .map_err(|error| format!("source is not UTF-8: {error}"))?;
    let snapshot = read_bounded(catalog_path, MAX_FILE_BYTES)?;
    let board = String::from_utf8(read_bounded(board_path, MAX_FILE_BYTES)?)
        .map_err(|error| format!("board is not UTF-8: {error}"))?;
    let compiled = compile_authenticated_source(source_path, source)?;
    if compiled.artifacts.static_artifacts().kicad_pcb != board {
        return Err("authenticated board does not match source compilation".to_owned());
    }
    let product = compile_product_artifacts(&compiled.elaborated.design, &snapshot, variant_path)
        .map_err(|diagnostics| {
        diagnostics
            .into_iter()
            .map(|diagnostic| diagnostic.to_string())
            .collect::<Vec<_>>()
            .join("\n")
    })?;
    let request = prepare_kicad10_fabrication_request(
        &compiled.elaborated.design,
        &snapshot,
        variant_path,
        fabrication_compiler_artifacts(&compiled),
        &product,
        ANALYSIS_PATH,
        ASSERTION_PATH,
    )
    .map_err(|error| error.to_string())?;
    std::io::stdout()
        .lock()
        .write_all(request.request_json.as_bytes())
        .map_err(|error| format!("failed to write verified request: {error}"))
}

fn run_bind(arguments: &[String]) -> Result<(), String> {
    let [
        source_path,
        catalog_path,
        variant_path,
        board_path,
        raw_root,
        kicad_cli,
    ] = arguments
    else {
        return Err(
            "usage: fabrication_gate bind SOURCE CATALOG VARIANT BOARD RAW_ROOT KICAD_CLI"
                .to_owned(),
        );
    };
    let source = String::from_utf8(read_bounded(source_path, MAX_FILE_BYTES)?)
        .map_err(|error| format!("source is not UTF-8: {error}"))?;
    let snapshot = read_bounded(catalog_path, MAX_FILE_BYTES)?;
    let board_bytes = read_bounded(board_path, MAX_FILE_BYTES)?;
    let board = String::from_utf8(board_bytes.clone())
        .map_err(|error| format!("board is not UTF-8: {error}"))?;
    let executable = read_bounded(kicad_cli, 512 * 1024 * 1024)?;
    let compiled = compile_authenticated_source(source_path, source)?;
    if compiled.artifacts.static_artifacts().kicad_pcb != board {
        return Err("authenticated board does not match source compilation".to_owned());
    }
    let product = compile_product_artifacts(&compiled.elaborated.design, &snapshot, variant_path)
        .map_err(|diagnostics| {
        diagnostics
            .into_iter()
            .map(|diagnostic| diagnostic.to_string())
            .collect::<Vec<_>>()
            .join("\n")
    })?;
    let request = prepare_kicad10_fabrication_request(
        &compiled.elaborated.design,
        &snapshot,
        variant_path,
        fabrication_compiler_artifacts(&compiled),
        &product,
        ANALYSIS_PATH,
        ASSERTION_PATH,
    )
    .map_err(|error| error.to_string())?;
    let raw_directories =
        RawDirectories::open(PathBuf::from(raw_root), &request.expected_host_paths)?;
    let mut raw_aggregate = 0_u64;
    let mut held_inputs = Vec::with_capacity(request.expected_host_paths.len() + 1);
    for path in &request.expected_host_paths {
        let (directory, basename) = path
            .as_str()
            .split_once('/')
            .ok_or_else(|| "raw path has no directory component".to_owned())?;
        let directory = match directory {
            "gerber" => "gerber",
            "drill" => "drill",
            "position" => "position",
            _ => return Err(format!("unsupported raw path: {}", path.as_str())),
        };
        let held = open_regular_at(
            directory,
            raw_directories.directory(directory)?,
            basename,
            MAX_FILE_BYTES,
        )?;
        let prospective = raw_aggregate
            .checked_add(held.identity.length)
            .ok_or_else(|| "raw fabrication aggregate overflowed".to_owned())?;
        if prospective > MAX_RAW_AGGREGATE_BYTES {
            return Err("raw fabrication aggregate exceeds 256 MiB".to_owned());
        }
        raw_aggregate = prospective;
        held_inputs.push(held);
    }
    let host_files = request
        .expected_host_paths
        .iter()
        .zip(held_inputs.iter_mut())
        .map(|(path, input)| {
            read_held_input(input, MAX_FILE_BYTES).map(|contents| FabricationHostFile {
                path: path.clone(),
                contents,
            })
        })
        .collect::<Result<Vec<_>, _>>()?;
    let executable_sha256 = sha256_hex(&executable);
    let mut receipt_input = open_regular_at(
        "receipt",
        &raw_directories.receipt,
        "host.json",
        1024 * 1024,
    )?;
    let receipt_bytes = read_held_input(&mut receipt_input, 1024 * 1024)?;
    held_inputs.push(receipt_input);
    let receipt: HostReceipt = serde_json::from_slice(&receipt_bytes)
        .map_err(|error| format!("failed to parse host receipt: {error}"))?;
    let mut expected_receipt = serde_json::to_vec(&receipt)
        .map_err(|error| format!("failed to canonicalize host receipt: {error}"))?;
    expected_receipt.push(b'\n');
    if expected_receipt != receipt_bytes
        || receipt.schema_name != "circuitc.kicad_fabrication_receipt"
        || receipt.schema_version != 1
        || receipt.request_sha256 != sha256_hex(request.request_json.as_bytes())
        || receipt.board_sha256 != sha256_hex(board.as_bytes())
        || receipt.executable_sha256 != executable_sha256
    {
        return Err(
            "host receipt is stale, noncanonical, or not bound to the board and executable"
                .to_owned(),
        );
    }
    let receipt_outputs: BTreeMap<_, _> = host_files
        .iter()
        .map(|file| (file.path.as_str().to_owned(), sha256_hex(&file.contents)))
        .collect();
    let supplied_receipt_outputs: BTreeMap<_, _> = receipt
        .outputs
        .iter()
        .map(|output| (output.path.clone(), output.sha256.clone()))
        .collect();
    if supplied_receipt_outputs.len() != receipt.outputs.len()
        || supplied_receipt_outputs != receipt_outputs
    {
        return Err(
            "host receipt output inventory does not match the explicit raw inputs".to_owned(),
        );
    }
    raw_directories.verify_identity(&request.expected_host_paths, &held_inputs)?;
    let bundle = bind_kicad10_fabrication(
        &compiled.elaborated.design,
        &snapshot,
        variant_path,
        fabrication_compiler_artifacts(&compiled),
        &product,
        ANALYSIS_PATH,
        ASSERTION_PATH,
        "10.0.5",
        &executable,
        &host_files,
    )
    .map_err(|error| error.to_string())?;
    verify_kicad10_fabrication_manifest(
        &compiled.elaborated.design,
        &snapshot,
        variant_path,
        fabrication_compiler_artifacts(&compiled),
        &product,
        ANALYSIS_PATH,
        ASSERTION_PATH,
        "10.0.5",
        &executable,
        &host_files,
        &bundle,
    )
    .map_err(|error| error.to_string())?;
    std::io::stdout()
        .lock()
        .write_all(bundle.manifest_json.as_bytes())
        .map_err(|error| format!("failed to write verified manifest: {error}"))
}

fn main() -> ExitCode {
    let arguments: Vec<String> = env::args().skip(1).collect();
    let result = match arguments.split_first() {
        Some((command, tail)) if command == "prepare" => run_prepare(tail),
        Some((command, tail)) if command == "bind" => run_bind(tail),
        _ => Err("usage: fabrication_gate prepare|bind ...".to_owned()),
    };
    match result {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("CircuitC fabrication gate failed: {error}");
            ExitCode::FAILURE
        }
    }
}
