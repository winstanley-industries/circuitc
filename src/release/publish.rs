//! Descriptor-anchored, failure-atomic materialization of verified releases.

use std::collections::{BTreeMap, BTreeSet};
use std::ffi::{CStr, CString, OsStr};
use std::fmt;
use std::fs::{File, Permissions};
use std::io::{self, Read as _, Write as _};
use std::os::fd::{AsRawFd as _, FromRawFd as _, RawFd};
use std::os::raw::{c_char, c_int, c_uint};
use std::os::unix::ffi::OsStrExt as _;
use std::os::unix::fs::{MetadataExt as _, PermissionsExt as _};
use std::path::{Component, Path, PathBuf};

use super::{ReleaseFile, VerifiedReleaseBundle};
use crate::RelativeArtifactPath;

#[cfg(not(any(target_os = "linux", target_os = "macos")))]
compile_error!("secure release publication is supported only on Linux and macOS");

#[cfg(target_os = "linux")]
const RENAME_NOREPLACE: c_uint = 1;
#[cfg(target_os = "macos")]
const RENAME_EXCL: c_uint = 0x00000004;
#[cfg(target_os = "macos")]
const ENOENT: c_int = 2;
#[cfg(target_os = "macos")]
const ACL_TYPE_EXTENDED: c_int = 0x0000_0100;
#[cfg(target_os = "macos")]
const ACL_FIRST_ENTRY: c_int = 0;
#[cfg(target_os = "macos")]
const ACL_NEXT_ENTRY: c_int = -1;
type Mode = libc::mode_t;

const O_CLOEXEC: c_int = libc::O_CLOEXEC;
const O_CREAT: c_int = libc::O_CREAT;
const O_DIRECTORY: c_int = libc::O_DIRECTORY;
const O_EXCL: c_int = libc::O_EXCL;
const O_NOFOLLOW: c_int = libc::O_NOFOLLOW;
const O_NONBLOCK: c_int = libc::O_NONBLOCK;
const O_RDONLY: c_int = libc::O_RDONLY;
const O_WRONLY: c_int = libc::O_WRONLY;
const AT_REMOVEDIR: c_int = libc::AT_REMOVEDIR;
const EINVAL: c_int = libc::EINVAL;
const ENOSYS: c_int = libc::ENOSYS;
const ENOTSUP_OR_EOPNOTSUPP: c_int = libc::ENOTSUP;

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
    fn acl_get_fd_np(descriptor: c_int, acl_type: c_int) -> *mut libc::c_void;
    fn acl_get_entry(
        acl: *mut libc::c_void,
        entry_id: c_int,
        entry: *mut *mut libc::c_void,
    ) -> c_int;
    fn acl_get_tag_type(entry: *mut libc::c_void, tag_type: *mut c_int) -> c_int;
    fn acl_free(object: *mut libc::c_void) -> c_int;
}

/// A descriptor capability for a caller-owned, mode-0700 publication root.
///
/// All later operations are relative to this retained descriptor. The input
/// pathname is used only while acquiring and validating the capability. The
/// caller's effective UID is the publication authority: unrelated security
/// principals are excluded, while malicious code running as that same UID is
/// able to mutate caller-owned files and is outside this filesystem boundary.
pub struct ReleaseDestination {
    directory: Directory,
    display_path: PathBuf,
}

impl fmt::Debug for ReleaseDestination {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("ReleaseDestination")
            .field("display_path", &self.display_path)
            .finish_non_exhaustive()
    }
}

impl ReleaseDestination {
    pub fn open(path: impl AsRef<Path>) -> Result<Self, ReleasePublishError> {
        let path = path.as_ref();
        let directory = open_validated_destination(path).map_err(|error| {
            publish_error(
                "CC-RELEASE-PUBLISH-DESTINATION-001",
                path.display().to_string(),
                error,
            )
        })?;
        Ok(Self {
            directory,
            display_path: path.to_owned(),
        })
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ReleasePublishError {
    pub code: &'static str,
    pub path: String,
    pub message: String,
}

impl fmt::Display for ReleasePublishError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(formatter, "{} [{}]: {}", self.code, self.path, self.message)
    }
}

impl std::error::Error for ReleasePublishError {}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ReleasePublicationWarning {
    pub code: &'static str,
    pub path: String,
    pub message: String,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum PublicationDurability {
    Durable,
    PublishedWithWarnings,
    /// The rename and namespace probes could not establish whether visibility
    /// occurred. No possible release or transaction residue was removed.
    VisibilityIndeterminate,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ReleasePublication {
    pub root: RelativeArtifactPath,
    pub durability: PublicationDurability,
    pub warnings: Vec<ReleasePublicationWarning>,
}

/// Materialize and atomically publish one independently verified release.
///
/// Errors are returned only before visibility. Once the atomic rename has
/// succeeded, any subsequent durability or exact-tree verification failure is
/// reported in `warnings`; a visible immutable release is never rolled back.
/// An inconclusive rename result returns `VisibilityIndeterminate` rather than
/// claiming either failure or publication.
pub fn publish_release(
    destination: &ReleaseDestination,
    release: &VerifiedReleaseBundle,
) -> Result<ReleasePublication, ReleasePublishError> {
    let mut hooks = ProductionHooks;
    publish_with_hooks(destination, release, &mut hooks)
}

trait PublicationHooks {
    fn after_directory_create(&mut self, _path: &str, _directory: &Directory) -> io::Result<()> {
        Ok(())
    }

    fn after_file_create(&mut self, _index: usize, _file: &File) -> io::Result<()> {
        Ok(())
    }

    fn after_file_write(&mut self, _index: usize, _file: &File) -> io::Result<()> {
        Ok(())
    }

    fn after_file_mode(&mut self, _index: usize, _file: &File) -> io::Result<()> {
        Ok(())
    }

    fn after_file(&mut self, _index: usize, _staging: &Directory) -> io::Result<()> {
        Ok(())
    }

    fn before_rename(&mut self, _release_parent: &Directory, _final_name: &CStr) -> io::Result<()> {
        Ok(())
    }

    fn after_rename(&mut self, _final_directory: &Directory) -> io::Result<()> {
        Ok(())
    }

    fn sync_release_parent(&mut self, release_parent: &Directory) -> io::Result<()> {
        release_parent.sync_all()
    }

    fn rename_release(
        &mut self,
        old_directory: &Directory,
        old_name: &CStr,
        new_directory: &Directory,
        new_name: &CStr,
    ) -> io::Result<()> {
        rename_noreplace(old_directory, old_name, new_directory, new_name)
    }

    fn probe_rename_identity(
        &mut self,
        directory: &Directory,
        name: &CStr,
    ) -> io::Result<(u64, u64)> {
        directory.probe_entry_identity(name)
    }
}

struct ProductionHooks;
impl PublicationHooks for ProductionHooks {}

fn publish_with_hooks(
    destination: &ReleaseDestination,
    verified: &VerifiedReleaseBundle,
    hooks: &mut impl PublicationHooks,
) -> Result<ReleasePublication, ReleasePublishError> {
    let bundle = verified.bundle();
    validate_verified_shape(
        bundle.root().as_str(),
        bundle.release_identity_sha256(),
        bundle.files(),
    )
    .map_err(|error| publish_error("CC-RELEASE-PUBLISH-CONTRACT-001", "release", error))?;
    let final_name = c_name(OsStr::new(bundle.release_identity_sha256())).map_err(|error| {
        publish_error(
            "CC-RELEASE-PUBLISH-CONTRACT-001",
            bundle.root().to_string(),
            error,
        )
    })?;
    let release_name = c"release";
    let release_parent = ensure_private_child(
        &destination.directory,
        release_name,
        &format!("{}/release", destination.display_path.display()),
    )
    .map_err(|error| {
        publish_error(
            "CC-RELEASE-PUBLISH-DESTINATION-001",
            format!("{}/release", destination.display_path.display()),
            error,
        )
    })?;
    destination.directory.sync_all().map_err(|error| {
        publish_error(
            "CC-RELEASE-PUBLISH-SYNC-001",
            destination.display_path.display().to_string(),
            error,
        )
    })?;

    let mut staging = StagingTransaction::create(&release_parent).map_err(|error| {
        publish_error(
            "CC-RELEASE-PUBLISH-STAGE-001",
            format!("{}/release", destination.display_path.display()),
            error,
        )
    })?;
    let staged_result = (|| -> io::Result<()> {
        let directories = required_directories(bundle.files())?;
        for directory in &directories {
            staging.create_directory(directory, hooks)?;
        }
        for (index, file) in bundle.files().iter().enumerate() {
            staging.create_file(index, file, hooks)?;
            hooks.after_file(index, &staging.directory)?;
        }
        staging.seal(&directories)?;
        release_parent.sync_all()?;
        hooks.before_rename(&release_parent, &final_name)?;
        let named_staging = release_parent.open_child(&staging.name)?;
        if named_staging.identity()? != staging.identity {
            return Err(io::Error::other(
                "staging directory identity changed before publication",
            ));
        }
        verify_exact_tree(
            &named_staging,
            bundle.files(),
            &staging.files,
            &staging.directories,
        )?;
        Ok(())
    })();
    if let Err(error) = staged_result {
        let cleanup = staging.cleanup();
        return Err(publish_error(
            "CC-RELEASE-PUBLISH-STAGE-001",
            bundle.root().to_string(),
            with_cleanup_error(error, cleanup),
        ));
    }

    let mut warnings = Vec::new();
    let mut visibility_indeterminate = false;
    if let Err(error) =
        hooks.rename_release(&release_parent, &staging.name, &release_parent, &final_name)
    {
        let final_identity = hooks.probe_rename_identity(&release_parent, &final_name);
        let source_identity = hooks.probe_rename_identity(&release_parent, &staging.name);
        if final_identity
            .as_ref()
            .is_ok_and(|identity| *identity == staging.identity)
        {
            staging.disarm();
            warnings.push(publication_warning(
                "CC-RELEASE-PUBLISH-RENAME-AMBIGUOUS-001",
                bundle.root().to_string(),
                io::Error::new(
                    error.kind(),
                    format!(
                        "no-replace rename reported failure after the staged identity became visible: {error}"
                    ),
                ),
            ));
        } else if probe_conclusively_excludes_identity(&final_identity, staging.identity, &error)
            && source_identity
                .as_ref()
                .is_ok_and(|identity| *identity == staging.identity)
        {
            let code = if error.kind() == io::ErrorKind::AlreadyExists {
                "CC-RELEASE-PUBLISH-EXISTS-001"
            } else if matches!(
                error.raw_os_error(),
                Some(EINVAL | ENOSYS | ENOTSUP_OR_EOPNOTSUPP)
            ) {
                "CC-RELEASE-PUBLISH-NOREPLACE-001"
            } else {
                "CC-RELEASE-PUBLISH-RENAME-001"
            };
            let cleanup = staging.cleanup();
            return Err(publish_error(
                code,
                bundle.root().to_string(),
                with_cleanup_error(error, cleanup),
            ));
        } else {
            staging.disarm();
            visibility_indeterminate = true;
            warnings.push(publication_warning(
                "CC-RELEASE-PUBLISH-RENAME-INDETERMINATE-001",
                bundle.root().to_string(),
                io::Error::new(
                    error.kind(),
                    format!(
                        "no-replace rename failed and visibility could not be reconciled; final probe: {}; source probe: {}; rename: {error}",
                        probe_description(&final_identity),
                        probe_description(&source_identity),
                    ),
                ),
            ));
        }
    } else {
        staging.disarm();
    }

    if !visibility_indeterminate && let Err(error) = hooks.after_rename(&staging.directory) {
        warnings.push(publication_warning(
            "CC-RELEASE-PUBLISH-POSTRENAME-001",
            bundle.root().to_string(),
            error,
        ));
    }
    if let Err(error) = hooks.sync_release_parent(&release_parent) {
        warnings.push(publication_warning(
            "CC-RELEASE-PUBLISH-DURABILITY-001",
            format!("{}/release", destination.display_path.display()),
            error,
        ));
    }
    match release_parent.open_child(&final_name) {
        Ok(final_directory) => {
            match final_directory.identity() {
                Ok(identity) if identity == staging.identity => {}
                Ok(_) => warnings.push(ReleasePublicationWarning {
                    code: "CC-RELEASE-PUBLISH-VERIFY-001",
                    path: bundle.root().to_string(),
                    message: "visible release identity differs from the staged directory"
                        .to_owned(),
                }),
                Err(error) => warnings.push(publication_warning(
                    "CC-RELEASE-PUBLISH-VERIFY-001",
                    bundle.root().to_string(),
                    error,
                )),
            }
            if let Err(error) = verify_exact_tree(
                &final_directory,
                bundle.files(),
                &staging.files,
                &staging.directories,
            ) {
                warnings.push(publication_warning(
                    "CC-RELEASE-PUBLISH-VERIFY-001",
                    bundle.root().to_string(),
                    error,
                ));
            }
        }
        Err(error) => warnings.push(publication_warning(
            "CC-RELEASE-PUBLISH-VERIFY-001",
            bundle.root().to_string(),
            error,
        )),
    }
    Ok(publication(
        bundle.root().clone(),
        warnings,
        visibility_indeterminate,
    ))
}

fn publication(
    root: RelativeArtifactPath,
    warnings: Vec<ReleasePublicationWarning>,
    visibility_indeterminate: bool,
) -> ReleasePublication {
    ReleasePublication {
        root,
        durability: if visibility_indeterminate {
            PublicationDurability::VisibilityIndeterminate
        } else if warnings.is_empty() {
            PublicationDurability::Durable
        } else {
            PublicationDurability::PublishedWithWarnings
        },
        warnings,
    }
}

fn probe_description(probe: &io::Result<(u64, u64)>) -> String {
    match probe {
        Ok((device, inode)) => format!("identity {device}:{inode}"),
        Err(error) => error.to_string(),
    }
}

fn probe_conclusively_excludes_identity(
    probe: &io::Result<(u64, u64)>,
    expected: (u64, u64),
    rename_error: &io::Error,
) -> bool {
    probe.as_ref().is_ok_and(|identity| *identity != expected)
        || matches!(
            rename_error.raw_os_error(),
            Some(EINVAL | ENOSYS | ENOTSUP_OR_EOPNOTSUPP)
        )
}

fn publication_warning(
    code: &'static str,
    path: impl Into<String>,
    error: io::Error,
) -> ReleasePublicationWarning {
    ReleasePublicationWarning {
        code,
        path: path.into(),
        message: error.to_string(),
    }
}

fn publish_error(
    code: &'static str,
    path: impl Into<String>,
    error: io::Error,
) -> ReleasePublishError {
    ReleasePublishError {
        code,
        path: path.into(),
        message: error.to_string(),
    }
}

fn validate_verified_shape(
    root: &str,
    release_identity: &str,
    files: &[ReleaseFile],
) -> io::Result<()> {
    let identity = root.strip_prefix("release/").ok_or_else(|| {
        io::Error::new(
            io::ErrorKind::InvalidInput,
            "verified root is not release/<sha256>",
        )
    })?;
    if identity.len() != 64
        || !identity
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
    {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "verified release identity is not lowercase SHA-256",
        ));
    }
    if identity != release_identity {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "verified root and release identity disagree",
        ));
    }
    if files.first().map(|file| file.path.as_str()) != Some("request.json")
        || files.last().map(|file| file.path.as_str()) != Some("manifest.json")
    {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "verified inventory must write request.json first and manifest.json last",
        ));
    }
    let mut paths = BTreeSet::new();
    for file in files {
        RelativeArtifactPath::try_new(file.path.as_str())
            .map_err(|error| io::Error::new(io::ErrorKind::InvalidInput, error.to_string()))?;
        if !paths.insert(file.path.as_str()) {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                format!("duplicate verified release path {}", file.path),
            ));
        }
    }
    Ok(())
}

fn required_directories(files: &[ReleaseFile]) -> io::Result<Vec<String>> {
    let file_paths = files
        .iter()
        .map(|file| file.path.as_str())
        .collect::<BTreeSet<_>>();
    let mut directories = BTreeSet::new();
    for file in files {
        let segments = file.path.as_str().split('/').collect::<Vec<_>>();
        let mut path = String::new();
        for segment in &segments[..segments.len() - 1] {
            if !path.is_empty() {
                path.push('/');
            }
            path.push_str(segment);
            if file_paths.contains(path.as_str()) {
                return Err(io::Error::new(
                    io::ErrorKind::InvalidInput,
                    format!("release path {path} is both a file and directory"),
                ));
            }
            directories.insert(path.clone());
        }
    }
    let mut directories = directories.into_iter().collect::<Vec<_>>();
    directories.sort_by_key(|path| (path.matches('/').count(), path.clone()));
    Ok(directories)
}

#[derive(Clone)]
struct CreatedEntry {
    path: String,
    identity: (u64, u64),
}

struct StagingTransaction<'a> {
    parent: &'a Directory,
    name: CString,
    directory: Directory,
    identity: (u64, u64),
    files: Vec<CreatedEntry>,
    directories: Vec<CreatedEntry>,
    armed: bool,
}

impl<'a> StagingTransaction<'a> {
    fn create(parent: &'a Directory) -> io::Result<Self> {
        for _ in 0..32 {
            let name = CString::new(format!(
                ".circuitc-release-transaction-{}-{}",
                std::process::id(),
                random_nonce()?
            ))
            .expect("hex transaction name has no NUL");
            if parent.create_child(&name, 0o700)? {
                if let Err(error) = parent.set_child_mode(&name, 0o700) {
                    return Err(with_cleanup_error(error, parent.remove_directory(&name)));
                }
                let directory = match parent.open_child(&name) {
                    Ok(directory) => directory,
                    Err(error) => {
                        return Err(with_cleanup_error(
                            io::Error::new(
                                error.kind(),
                                format!(
                                    "could not open new transaction {}: {error}",
                                    name.to_string_lossy()
                                ),
                            ),
                            parent.remove_directory(&name),
                        ));
                    }
                };
                let identity = match directory.identity() {
                    Ok(identity) => identity,
                    Err(error) => {
                        return Err(with_cleanup_error(error, parent.remove_directory(&name)));
                    }
                };
                if let Err(error) = validate_private_directory(&directory, "release transaction") {
                    return Err(with_cleanup_error(
                        error,
                        remove_named_directory_if_identity(
                            parent,
                            &name,
                            identity,
                            &name.to_string_lossy(),
                        ),
                    ));
                }
                return Ok(Self {
                    parent,
                    name,
                    directory,
                    identity,
                    files: Vec::new(),
                    directories: Vec::new(),
                    armed: true,
                });
            }
        }
        Err(io::Error::new(
            io::ErrorKind::AlreadyExists,
            "could not allocate a unique release transaction directory",
        ))
    }

    fn create_directory(
        &mut self,
        path: &str,
        hooks: &mut impl PublicationHooks,
    ) -> io::Result<()> {
        let (parent_path, name) = split_parent(path);
        let parent = self.directory.open_relative(parent_path)?;
        let name = c_name(OsStr::new(name))?;
        if !parent.create_child(&name, 0o700)? {
            return Err(io::Error::new(
                io::ErrorKind::AlreadyExists,
                format!("staging directory {path} already exists"),
            ));
        }
        if let Err(error) = parent.set_child_mode(&name, 0o700) {
            return Err(with_cleanup_error(error, parent.remove_directory(&name)));
        }
        let directory = match parent.open_child(&name) {
            Ok(directory) => directory,
            Err(error) => {
                return Err(with_cleanup_error(error, parent.remove_directory(&name)));
            }
        };
        let entry = CreatedEntry {
            path: path.to_owned(),
            identity: directory.identity()?,
        };
        self.directories.push(entry);
        hooks.after_directory_create(path, &directory)?;
        validate_private_directory(&directory, path)?;
        Ok(())
    }

    fn create_file(
        &mut self,
        index: usize,
        release_file: &ReleaseFile,
        hooks: &mut impl PublicationHooks,
    ) -> io::Result<()> {
        let path = release_file.path.as_str();
        let (parent_path, name) = split_parent(path);
        let parent = self.directory.open_relative(parent_path)?;
        let name = c_name(OsStr::new(name))?;
        let mut file = parent.create_file(&name)?;
        let created_metadata = match file.metadata() {
            Ok(metadata) => metadata,
            Err(error) => {
                return Err(with_cleanup_error(error, parent.remove_file(&name)));
            }
        };
        self.files.push(CreatedEntry {
            path: path.to_owned(),
            identity: (created_metadata.dev(), created_metadata.ino()),
        });
        hooks.after_file_create(index, &file)?;
        file.write_all(&release_file.contents)?;
        hooks.after_file_write(index, &file)?;
        file.set_permissions(Permissions::from_mode(0o400))?;
        hooks.after_file_mode(index, &file)?;
        file.sync_all()?;
        let metadata = file.metadata()?;
        if !metadata.file_type().is_file()
            || metadata.nlink() != 1
            || metadata.uid() != effective_uid()
            || metadata.mode() & 0o7777 != 0o400
        {
            return Err(io::Error::new(
                io::ErrorKind::PermissionDenied,
                format!("staged file {path} does not retain private immutable identity"),
            ));
        }
        if (metadata.dev(), metadata.ino()) != self.files.last().expect("recorded file").identity {
            return Err(io::Error::other(format!(
                "staged file {path} changed identity during materialization"
            )));
        }
        Ok(())
    }

    fn seal(&self, directories: &[String]) -> io::Result<()> {
        for path in directories.iter().rev() {
            let directory = self.directory.open_relative(path)?;
            directory.set_mode(0o500)?;
            directory.sync_all()?;
        }
        self.directory.set_mode(0o500)?;
        self.directory.sync_all()
    }

    fn disarm(&mut self) {
        self.armed = false;
    }

    fn cleanup(&mut self) -> io::Result<()> {
        if !self.armed {
            return Ok(());
        }
        let mut errors = Vec::new();
        match self.parent.open_child(&self.name) {
            Ok(directory) if directory.identity()? == self.identity => {
                directory.set_mode(0o700)?;
            }
            Ok(_) => {
                self.armed = false;
                return Err(io::Error::other(format!(
                    "preserved replacement transaction {} after identity changed",
                    self.name.to_string_lossy()
                )));
            }
            Err(error) => {
                self.armed = false;
                return Err(io::Error::new(
                    error.kind(),
                    format!(
                        "could not reacquire transaction {} by held parent identity: {error}",
                        self.name.to_string_lossy()
                    ),
                ));
            }
        }
        for entry in &self.directories {
            match self.directory.open_relative(&entry.path) {
                Ok(directory) if directory.identity()? == entry.identity => {
                    if let Err(error) = directory.set_mode(0o700) {
                        errors.push(format!(
                            "could not restore staged directory {} for cleanup: {error}",
                            entry.path
                        ));
                    }
                }
                Ok(_) => errors.push(format!(
                    "preserved replacement staged directory {}",
                    entry.path
                )),
                Err(error) => errors.push(format!(
                    "could not inspect staged directory {} for cleanup: {error}",
                    entry.path
                )),
            }
        }
        for entry in self.files.iter().rev() {
            if let Err(error) = remove_file_if_identity(&self.directory, entry) {
                errors.push(error.to_string());
            }
        }
        for entry in self.directories.iter().rev() {
            if let Err(error) = remove_directory_if_identity(&self.directory, entry) {
                errors.push(error.to_string());
            }
        }
        if let Err(error) = remove_named_directory_if_identity(
            self.parent,
            &self.name,
            self.identity,
            &self.name.to_string_lossy(),
        ) {
            errors.push(error.to_string());
        }
        self.armed = false;
        if errors.is_empty() {
            Ok(())
        } else {
            Err(io::Error::other(format!(
                "transaction cleanup was incomplete: {}",
                errors.join("; ")
            )))
        }
    }
}

impl Drop for StagingTransaction<'_> {
    fn drop(&mut self) {
        let _ = self.cleanup();
    }
}

fn remove_file_if_identity(root: &Directory, entry: &CreatedEntry) -> io::Result<()> {
    let (parent_path, name) = split_parent(&entry.path);
    let parent = root.open_relative(parent_path)?;
    let source = c_name(OsStr::new(name))?;
    let claim = claim_for_cleanup(&parent, &source)?;
    let file = match parent.open_file(&claim) {
        Ok(file) => file,
        Err(error) => {
            return Err(with_cleanup_error(
                error,
                restore_cleanup_claim(&parent, &claim, &source),
            ));
        }
    };
    let metadata = match file.metadata() {
        Ok(metadata) => metadata,
        Err(error) => {
            return Err(with_cleanup_error(
                error,
                restore_cleanup_claim(&parent, &claim, &source),
            ));
        }
    };
    if (metadata.dev(), metadata.ino()) != entry.identity {
        return Err(with_cleanup_error(
            io::Error::other(format!("preserved replacement staged file {}", entry.path)),
            restore_cleanup_claim(&parent, &claim, &source),
        ));
    }
    drop(file);
    match parent.remove_file(&claim) {
        Ok(()) => Ok(()),
        Err(error) => Err(with_cleanup_error(
            error,
            restore_cleanup_claim(&parent, &claim, &source),
        )),
    }
}

fn remove_directory_if_identity(root: &Directory, entry: &CreatedEntry) -> io::Result<()> {
    let (parent_path, name) = split_parent(&entry.path);
    let parent = root.open_relative(parent_path)?;
    remove_named_directory_if_identity(
        &parent,
        &c_name(OsStr::new(name))?,
        entry.identity,
        &entry.path,
    )
}

fn remove_named_directory_if_identity(
    parent: &Directory,
    source: &CStr,
    expected_identity: (u64, u64),
    display: &str,
) -> io::Result<()> {
    let claim = claim_for_cleanup(parent, source)?;
    let directory = match parent.open_child(&claim) {
        Ok(directory) => directory,
        Err(error) => {
            return Err(with_cleanup_error(
                error,
                restore_cleanup_claim(parent, &claim, source),
            ));
        }
    };
    let actual_identity = match directory.identity() {
        Ok(identity) => identity,
        Err(error) => {
            return Err(with_cleanup_error(
                error,
                restore_cleanup_claim(parent, &claim, source),
            ));
        }
    };
    if actual_identity != expected_identity {
        return Err(with_cleanup_error(
            io::Error::other(format!("preserved replacement staged directory {display}")),
            restore_cleanup_claim(parent, &claim, source),
        ));
    }
    if let Err(error) = directory.set_mode(0o700) {
        return Err(with_cleanup_error(
            error,
            restore_cleanup_claim(parent, &claim, source),
        ));
    }
    drop(directory);
    match parent.remove_directory(&claim) {
        Ok(()) => Ok(()),
        Err(error) => Err(with_cleanup_error(
            error,
            restore_cleanup_claim(parent, &claim, source),
        )),
    }
}

fn claim_for_cleanup(parent: &Directory, source: &CStr) -> io::Result<CString> {
    for _ in 0..16 {
        let claim = CString::new(format!(".circuitc-cleanup-{}", random_nonce()?))
            .expect("hex cleanup name has no NUL");
        match rename_noreplace(parent, source, parent, &claim) {
            Ok(()) => return Ok(claim),
            Err(error) if error.kind() == io::ErrorKind::AlreadyExists => continue,
            Err(error) => return Err(error),
        }
    }
    Err(io::Error::new(
        io::ErrorKind::AlreadyExists,
        "could not allocate a private cleanup claim",
    ))
}

fn restore_cleanup_claim(parent: &Directory, claim: &CStr, source: &CStr) -> io::Result<()> {
    rename_noreplace(parent, claim, parent, source)
}

fn verify_exact_tree(
    root: &Directory,
    files: &[ReleaseFile],
    created_files: &[CreatedEntry],
    created_directories: &[CreatedEntry],
) -> io::Result<()> {
    validate_immutable_directory(root, "release")?;
    let directories = required_directories(files)?;
    let expected_file_identities = created_files
        .iter()
        .map(|entry| (entry.path.as_str(), entry.identity))
        .collect::<BTreeMap<_, _>>();
    let expected_directory_identities = created_directories
        .iter()
        .map(|entry| (entry.path.as_str(), entry.identity))
        .collect::<BTreeMap<_, _>>();
    let mut expected = BTreeMap::<String, BTreeSet<String>>::new();
    expected.entry(String::new()).or_default();
    for directory in &directories {
        let (parent, name) = split_parent(directory);
        expected
            .entry(parent.to_owned())
            .or_default()
            .insert(name.to_owned());
        expected.entry(directory.clone()).or_default();
    }
    for file in files {
        let (parent, name) = split_parent(file.path.as_str());
        expected
            .entry(parent.to_owned())
            .or_default()
            .insert(name.to_owned());
    }
    for (path, expected_entries) in &expected {
        let directory = root.open_relative(path)?;
        validate_immutable_directory(&directory, path)?;
        if !path.is_empty()
            && expected_directory_identities.get(path.as_str()).copied()
                != Some(directory.identity()?)
        {
            return Err(io::Error::other(format!(
                "published directory {path:?} differs from its staged identity"
            )));
        }
        let actual = directory.list_entries()?;
        if actual != *expected_entries {
            return Err(io::Error::other(format!(
                "release directory {path:?} has unexpected inventory: expected {expected_entries:?}, found {actual:?}"
            )));
        }
    }
    for expected_file in files {
        let (parent, name) = split_parent(expected_file.path.as_str());
        let directory = root.open_relative(parent)?;
        let mut file = directory.open_file(&c_name(OsStr::new(name))?)?;
        let metadata = file.metadata()?;
        if !metadata.file_type().is_file()
            || metadata.nlink() != 1
            || metadata.uid() != effective_uid()
            || metadata.mode() & 0o7777 != 0o400
            || metadata.len() != expected_file.contents.len() as u64
            || expected_file_identities
                .get(expected_file.path.as_str())
                .copied()
                != Some((metadata.dev(), metadata.ino()))
        {
            return Err(io::Error::other(format!(
                "published file {} has unexpected identity, mode, link count, or length",
                expected_file.path
            )));
        }
        let original_identity = (metadata.dev(), metadata.ino());
        let mut offset = 0;
        let mut buffer = [0_u8; 64 * 1024];
        while offset < expected_file.contents.len() {
            let remaining = expected_file.contents.len() - offset;
            let chunk_len = remaining.min(buffer.len());
            let count = file.read(&mut buffer[..chunk_len])?;
            if count == 0 || buffer[..count] != expected_file.contents[offset..offset + count] {
                return Err(io::Error::other(format!(
                    "published file {} differs from verified bytes",
                    expected_file.path
                )));
            }
            offset += count;
        }
        let mut trailing = [0_u8; 1];
        if file.read(&mut trailing)? != 0 {
            return Err(io::Error::other(format!(
                "published file {} grew during verification",
                expected_file.path
            )));
        }
        let final_metadata = file.metadata()?;
        if (final_metadata.dev(), final_metadata.ino()) != original_identity
            || final_metadata.len() != expected_file.contents.len() as u64
            || !final_metadata.file_type().is_file()
            || final_metadata.nlink() != 1
            || final_metadata.uid() != effective_uid()
            || final_metadata.mode() & 0o7777 != 0o400
        {
            return Err(io::Error::other(format!(
                "published file {} changed during verification",
                expected_file.path
            )));
        }
    }
    Ok(())
}

struct Directory(File);

impl Directory {
    fn open_root() -> io::Result<Self> {
        let root = c"/";
        // SAFETY: `root` is NUL-terminated and remains live for the call.
        file_from_descriptor(unsafe {
            open(
                root.as_ptr(),
                O_RDONLY | O_DIRECTORY | O_NOFOLLOW | O_CLOEXEC,
            )
        })
        .map(Self)
    }

    fn try_clone(&self) -> io::Result<Self> {
        self.0.try_clone().map(Self)
    }

    fn identity(&self) -> io::Result<(u64, u64)> {
        let metadata = self.0.metadata()?;
        Ok((metadata.dev(), metadata.ino()))
    }

    fn open_child(&self, name: &CStr) -> io::Result<Self> {
        // SAFETY: `name` is NUL-terminated and this descriptor remains live.
        file_from_descriptor(unsafe {
            openat(
                self.0.as_raw_fd(),
                name.as_ptr(),
                O_RDONLY | O_DIRECTORY | O_NOFOLLOW | O_CLOEXEC,
            )
        })
        .map(Self)
    }

    fn open_relative(&self, path: &str) -> io::Result<Self> {
        let mut current = self.try_clone()?;
        if path.is_empty() {
            return Ok(current);
        }
        for segment in path.split('/') {
            current = current.open_child(&c_name(OsStr::new(segment))?)?;
        }
        Ok(current)
    }

    fn create_child(&self, name: &CStr, mode: Mode) -> io::Result<bool> {
        // SAFETY: `name` is NUL-terminated and this descriptor remains live.
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

    fn create_file(&self, name: &CStr) -> io::Result<File> {
        // SAFETY: `name` is NUL-terminated; O_CREAT supplies the mode argument.
        #[cfg(target_os = "linux")]
        let descriptor = unsafe {
            openat(
                self.0.as_raw_fd(),
                name.as_ptr(),
                O_WRONLY | O_CREAT | O_EXCL | O_NOFOLLOW | O_CLOEXEC,
                0o600_u32,
            )
        };
        #[cfg(target_os = "macos")]
        let descriptor = unsafe {
            openat(
                self.0.as_raw_fd(),
                name.as_ptr(),
                O_WRONLY | O_CREAT | O_EXCL | O_NOFOLLOW | O_CLOEXEC,
                0o600_i32,
            )
        };
        file_from_descriptor(descriptor)
    }

    fn open_file(&self, name: &CStr) -> io::Result<File> {
        // O_NONBLOCK prevents a replaced FIFO from blocking verification.
        // SAFETY: `name` is NUL-terminated and this descriptor remains live.
        file_from_descriptor(unsafe {
            openat(
                self.0.as_raw_fd(),
                name.as_ptr(),
                O_RDONLY | O_NONBLOCK | O_NOFOLLOW | O_CLOEXEC,
            )
        })
    }

    fn probe_entry_identity(&self, name: &CStr) -> io::Result<(u64, u64)> {
        let mut metadata = std::mem::MaybeUninit::<libc::stat>::uninit();
        // SAFETY: `name` and the parent descriptor remain live, metadata points
        // to writable storage, and AT_SYMLINK_NOFOLLOW inspects any entry type
        // without following a replaced symbolic link.
        let status = unsafe {
            libc::fstatat(
                self.0.as_raw_fd(),
                name.as_ptr(),
                metadata.as_mut_ptr(),
                libc::AT_SYMLINK_NOFOLLOW,
            )
        };
        if status == 0 {
            // SAFETY: successful fstatat initialized the complete stat value.
            let metadata = unsafe { metadata.assume_init() };
            Ok((metadata.st_dev as u64, metadata.st_ino))
        } else {
            Err(io::Error::last_os_error())
        }
    }

    fn remove_file(&self, name: &CStr) -> io::Result<()> {
        self.unlink(name, 0)
    }

    fn remove_directory(&self, name: &CStr) -> io::Result<()> {
        self.unlink(name, AT_REMOVEDIR)
    }

    fn unlink(&self, name: &CStr, flags: c_int) -> io::Result<()> {
        // SAFETY: `name` is NUL-terminated and this descriptor remains live.
        status_result(unsafe { unlinkat(self.0.as_raw_fd(), name.as_ptr(), flags) })
    }

    fn set_mode(&self, mode: u32) -> io::Result<()> {
        self.0.set_permissions(Permissions::from_mode(mode))
    }

    fn set_child_mode(&self, name: &CStr, mode: Mode) -> io::Result<()> {
        // SAFETY: `name` is NUL-terminated, the parent descriptor is live, and
        // an exclusive mkdir in this private namespace established ownership.
        status_result(unsafe { libc::fchmodat(self.0.as_raw_fd(), name.as_ptr(), mode, 0) })
    }

    fn sync_all(&self) -> io::Result<()> {
        self.0.sync_all()
    }

    fn list_entries(&self) -> io::Result<BTreeSet<String>> {
        // SAFETY: dup returns a new owned descriptor on success.
        let duplicate = unsafe { libc::dup(self.0.as_raw_fd()) };
        if duplicate < 0 {
            return Err(io::Error::last_os_error());
        }
        // SAFETY: `duplicate` is owned and is transferred to fdopendir.
        let stream = unsafe { libc::fdopendir(duplicate) };
        if stream.is_null() {
            // SAFETY: fdopendir did not consume a descriptor on failure.
            unsafe { libc::close(duplicate) };
            return Err(io::Error::last_os_error());
        }
        let mut entries = BTreeSet::new();
        loop {
            clear_errno();
            // SAFETY: `stream` is live until closed below.
            let entry = unsafe { libc::readdir(stream) };
            if entry.is_null() {
                let error = io::Error::last_os_error();
                if error.raw_os_error() != Some(0) {
                    // SAFETY: `stream` is live and is closed once.
                    unsafe { libc::closedir(stream) };
                    return Err(error);
                }
                break;
            }
            // SAFETY: d_name is NUL-terminated for a successful readdir.
            let name = unsafe { CStr::from_ptr((*entry).d_name.as_ptr()) };
            if name.to_bytes() == b"." || name.to_bytes() == b".." {
                continue;
            }
            let name = std::str::from_utf8(name.to_bytes()).map_err(|_| {
                io::Error::new(
                    io::ErrorKind::InvalidData,
                    "published directory contains a non-UTF-8 entry",
                )
            })?;
            entries.insert(name.to_owned());
        }
        // SAFETY: `stream` is live and is closed once.
        if unsafe { libc::closedir(stream) } != 0 {
            return Err(io::Error::last_os_error());
        }
        Ok(entries)
    }
}

fn open_validated_destination(path: &Path) -> io::Result<Directory> {
    if !path.is_absolute() {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "release destination must be an absolute path",
        ));
    }
    let components = path
        .components()
        .filter(|component| !matches!(component, Component::RootDir | Component::CurDir))
        .collect::<Vec<_>>();
    if components.is_empty() {
        return Err(io::Error::new(
            io::ErrorKind::PermissionDenied,
            "filesystem root cannot be a private release destination",
        ));
    }
    let mut current = Directory::open_root()?;
    validate_namespace_ancestor(&current, "/")?;
    let mut walked = PathBuf::from("/");
    for (index, component) in components.iter().enumerate() {
        let Component::Normal(segment) = component else {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "release destination must be lexically normalized",
            ));
        };
        walked.push(segment);
        current = current.open_child(&c_name(segment)?)?;
        if index + 1 == components.len() {
            validate_private_directory(&current, &walked.display().to_string())?;
        } else {
            validate_namespace_ancestor(&current, &walked.display().to_string())?;
        }
    }
    Ok(current)
}

fn ensure_private_child(parent: &Directory, name: &CStr, display: &str) -> io::Result<Directory> {
    let created = parent.create_child(name, 0o700)?;
    if created && let Err(error) = parent.set_child_mode(name, 0o700) {
        return Err(with_cleanup_error(error, parent.remove_directory(name)));
    }
    let child = match parent.open_child(name) {
        Ok(child) => child,
        Err(error) if created => {
            return Err(with_cleanup_error(error, parent.remove_directory(name)));
        }
        Err(error) => return Err(error),
    };
    if let Err(error) = validate_private_directory(&child, display) {
        if created {
            let cleanup = child.identity().and_then(|identity| {
                remove_named_directory_if_identity(parent, name, identity, display)
            });
            return Err(with_cleanup_error(error, cleanup));
        }
        return Err(error);
    }
    Ok(child)
}

fn validate_namespace_ancestor(directory: &Directory, display: &str) -> io::Result<()> {
    let metadata = directory.0.metadata()?;
    let owner = metadata.uid();
    if owner != 0 && owner != effective_uid() {
        return Err(io::Error::new(
            io::ErrorKind::PermissionDenied,
            format!("namespace ancestor {display} is not root- or caller-owned"),
        ));
    }
    let mode = metadata.mode();
    if mode & 0o022 != 0 && mode & 0o1000 == 0 {
        return Err(io::Error::new(
            io::ErrorKind::PermissionDenied,
            format!("namespace ancestor {display} is writable by other users without sticky bit"),
        ));
    }
    reject_extended_acl(directory, display, true)
}

fn validate_private_directory(directory: &Directory, display: &str) -> io::Result<()> {
    let metadata = directory.0.metadata()?;
    if metadata.uid() != effective_uid() || metadata.mode() & 0o7777 != 0o700 {
        return Err(io::Error::new(
            io::ErrorKind::PermissionDenied,
            format!("mutable release directory {display} must be caller-owned mode 0700"),
        ));
    }
    reject_extended_acl(directory, display, false)
}

fn validate_immutable_directory(directory: &Directory, display: &str) -> io::Result<()> {
    let metadata = directory.0.metadata()?;
    if metadata.uid() != effective_uid() || metadata.mode() & 0o7777 != 0o500 {
        return Err(io::Error::new(
            io::ErrorKind::PermissionDenied,
            format!("immutable release directory {display:?} must be caller-owned mode 0500"),
        ));
    }
    reject_extended_acl(directory, display, false)
}

#[cfg(target_os = "linux")]
fn reject_extended_acl(
    _directory: &Directory,
    _display: &str,
    _allow_deny_only: bool,
) -> io::Result<()> {
    // Linux POSIX ACL grants are bounded by the mode group class checked above.
    Ok(())
}

#[cfg(target_os = "macos")]
fn reject_extended_acl(
    directory: &Directory,
    display: &str,
    allow_deny_only: bool,
) -> io::Result<()> {
    // SAFETY: the descriptor is live and ACL_TYPE_EXTENDED is the Darwin ACL type.
    let acl = unsafe { acl_get_fd_np(directory.0.as_raw_fd(), ACL_TYPE_EXTENDED) };
    if acl.is_null() {
        let error = io::Error::last_os_error();
        if error.raw_os_error() == Some(ENOENT) {
            return Ok(());
        }
        return Err(error);
    }
    let mut entry = std::ptr::null_mut();
    let mut entry_id = ACL_FIRST_ENTRY;
    let mut rejected = false;
    let mut entry_error = None;
    loop {
        // SAFETY: `acl` is live and `entry` points to writable storage.
        let status = unsafe { acl_get_entry(acl, entry_id, &mut entry) };
        if status < 0 {
            let error = io::Error::last_os_error();
            if entry_id == ACL_NEXT_ENTRY && error.raw_os_error() == Some(EINVAL) {
                break;
            }
            entry_error = Some(error);
            break;
        }
        if !allow_deny_only {
            rejected = true;
            break;
        }
        let mut tag_type = 0;
        // SAFETY: `entry` is a live borrowed ACL entry.
        if unsafe { acl_get_tag_type(entry, &mut tag_type) } != 0 {
            entry_error = Some(io::Error::last_os_error());
            break;
        }
        const ACL_EXTENDED_DENY: c_int = 2;
        if tag_type != ACL_EXTENDED_DENY {
            rejected = true;
            break;
        }
        entry_id = ACL_NEXT_ENTRY;
    }
    // SAFETY: `acl` was returned by acl_get_fd_np and is released once.
    let free_status = unsafe { acl_free(acl) };
    if let Some(error) = entry_error {
        return Err(error);
    }
    if free_status != 0 {
        return Err(io::Error::last_os_error());
    }
    if rejected {
        return Err(io::Error::new(
            io::ErrorKind::PermissionDenied,
            format!("release namespace {display} has a permissive or unexpected extended ACL"),
        ));
    }
    Ok(())
}

fn random_nonce() -> io::Result<String> {
    let mut bytes = [0_u8; 16];
    File::open("/dev/urandom")?.read_exact(&mut bytes)?;
    Ok(bytes.iter().map(|byte| format!("{byte:02x}")).collect())
}

fn rename_noreplace(
    old_directory: &Directory,
    old_name: &CStr,
    new_directory: &Directory,
    new_name: &CStr,
) -> io::Result<()> {
    #[cfg(target_os = "linux")]
    // SAFETY: names and descriptors remain live for this call.
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
    // SAFETY: names and descriptors remain live for this call.
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

fn with_cleanup_error(error: io::Error, cleanup: io::Result<()>) -> io::Error {
    match cleanup {
        Ok(()) => error,
        Err(cleanup_error) => io::Error::new(
            error.kind(),
            format!("{error}; additionally, {cleanup_error}"),
        ),
    }
}

fn split_parent(path: &str) -> (&str, &str) {
    path.rsplit_once('/').unwrap_or(("", path))
}

fn c_name(name: &OsStr) -> io::Result<CString> {
    CString::new(name.as_bytes()).map_err(|_| {
        io::Error::new(
            io::ErrorKind::InvalidInput,
            "filesystem name contains a NUL byte",
        )
    })
}

fn effective_uid() -> u32 {
    // SAFETY: geteuid has no preconditions.
    unsafe { libc::geteuid() }
}

fn file_from_descriptor(descriptor: RawFd) -> io::Result<File> {
    if descriptor < 0 {
        Err(io::Error::last_os_error())
    } else {
        // SAFETY: a successful open/openat returns a new owned descriptor.
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

#[cfg(target_os = "linux")]
fn clear_errno() {
    // SAFETY: errno storage is writable thread-local state.
    unsafe { *libc::__errno_location() = 0 };
}

#[cfg(target_os = "macos")]
fn clear_errno() {
    // SAFETY: errno storage is writable thread-local state.
    unsafe { *libc::__error() = 0 };
}

#[cfg(test)]
mod tests {
    use std::fs;
    use std::os::unix::fs::{PermissionsExt as _, symlink};
    use std::sync::Arc;

    use super::*;
    use crate::release::ReleaseBundle;

    const IDENTITY: &str = "0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef";

    fn verified_release() -> VerifiedReleaseBundle {
        let request = b"{\"request\":true}\n".to_vec();
        let manifest = b"{\"all_pass\":true}\n".to_vec();
        VerifiedReleaseBundle(ReleaseBundle {
            release_identity_sha256: IDENTITY.to_owned(),
            root: RelativeArtifactPath::try_new(format!("release/{IDENTITY}")).unwrap(),
            request_json: String::from_utf8(request.clone()).unwrap(),
            manifest_json: String::from_utf8(manifest.clone()).unwrap(),
            files: vec![
                ReleaseFile {
                    path: RelativeArtifactPath::try_new("request.json").unwrap(),
                    contents: request,
                },
                ReleaseFile {
                    path: RelativeArtifactPath::try_new("artifacts/board.bin").unwrap(),
                    contents: b"exact-board-bytes".to_vec(),
                },
                ReleaseFile {
                    path: RelativeArtifactPath::try_new("manifest.json").unwrap(),
                    contents: manifest,
                },
            ],
        })
    }

    struct TestDirectory(PathBuf);

    impl TestDirectory {
        fn new(label: &str) -> Self {
            let base = fs::canonicalize(std::env::temp_dir()).unwrap();
            let path = base.join(format!(
                "circuitc-release-publish-{label}-{}-{}",
                std::process::id(),
                random_nonce().unwrap()
            ));
            fs::create_dir(&path).unwrap();
            fs::set_permissions(&path, fs::Permissions::from_mode(0o700)).unwrap();
            Self(path)
        }

        fn path(&self) -> &Path {
            &self.0
        }
    }

    impl Drop for TestDirectory {
        fn drop(&mut self) {
            make_tree_writable(&self.0);
            let _ = fs::remove_dir_all(&self.0);
        }
    }

    fn make_tree_writable(path: &Path) {
        let Ok(metadata) = fs::symlink_metadata(path) else {
            return;
        };
        if metadata.file_type().is_symlink() {
            return;
        }
        if metadata.is_dir() {
            let _ = fs::set_permissions(path, fs::Permissions::from_mode(0o700));
            if let Ok(entries) = fs::read_dir(path) {
                for entry in entries.flatten() {
                    make_tree_writable(&entry.path());
                }
            }
        } else {
            let _ = fs::set_permissions(path, fs::Permissions::from_mode(0o600));
        }
    }

    fn visible_root(test: &TestDirectory) -> PathBuf {
        test.path().join("release").join(IDENTITY)
    }

    fn release_entries(test: &TestDirectory) -> Vec<String> {
        let release = test.path().join("release");
        if !release.exists() {
            return Vec::new();
        }
        let mut entries = fs::read_dir(release)
            .unwrap()
            .map(|entry| entry.unwrap().file_name().into_string().unwrap())
            .collect::<Vec<_>>();
        entries.sort();
        entries
    }

    #[test]
    fn publishes_exact_private_tree_and_manifest_is_last() {
        struct ObserveOrder(bool);
        impl PublicationHooks for ObserveOrder {
            fn after_file(&mut self, index: usize, staging: &Directory) -> io::Result<()> {
                if index == 1 {
                    assert!(!staging.list_entries()?.contains("manifest.json"));
                    self.0 = true;
                }
                Ok(())
            }
        }

        let test = TestDirectory::new("success");
        let destination = ReleaseDestination::open(test.path()).unwrap();
        let release = verified_release();
        let mut hook = ObserveOrder(false);
        let outcome = publish_with_hooks(&destination, &release, &mut hook).unwrap();
        assert!(hook.0);
        assert_eq!(outcome.durability, PublicationDurability::Durable);
        assert!(outcome.warnings.is_empty());
        assert_eq!(
            fs::read(visible_root(&test).join("artifacts/board.bin")).unwrap(),
            b"exact-board-bytes"
        );
        assert_eq!(
            fs::metadata(visible_root(&test))
                .unwrap()
                .permissions()
                .mode()
                & 0o7777,
            0o500
        );
        assert_eq!(
            fs::metadata(visible_root(&test).join("manifest.json"))
                .unwrap()
                .permissions()
                .mode()
                & 0o7777,
            0o400
        );
    }

    #[test]
    fn identical_existing_release_is_immutable() {
        let test = TestDirectory::new("existing");
        let destination = ReleaseDestination::open(test.path()).unwrap();
        publish_release(&destination, &verified_release()).unwrap();
        let manifest = fs::read(visible_root(&test).join("manifest.json")).unwrap();
        let error = publish_release(&destination, &verified_release()).unwrap_err();
        assert_eq!(error.code, "CC-RELEASE-PUBLISH-EXISTS-001");
        assert_eq!(
            fs::read(visible_root(&test).join("manifest.json")).unwrap(),
            manifest
        );
        assert_eq!(release_entries(&test), vec![IDENTITY]);
    }

    #[test]
    fn previsibility_failure_cleans_the_complete_staging_tree() {
        struct FailAfterPayload;
        impl PublicationHooks for FailAfterPayload {
            fn after_file(&mut self, index: usize, _staging: &Directory) -> io::Result<()> {
                if index == 1 {
                    Err(io::Error::other("injected write interruption"))
                } else {
                    Ok(())
                }
            }
        }

        let test = TestDirectory::new("rollback");
        let destination = ReleaseDestination::open(test.path()).unwrap();
        let error = publish_with_hooks(&destination, &verified_release(), &mut FailAfterPayload)
            .unwrap_err();
        assert_eq!(error.code, "CC-RELEASE-PUBLISH-STAGE-001");
        assert!(release_entries(&test).is_empty());
        assert!(!visible_root(&test).exists());
    }

    #[derive(Clone, Copy, Debug)]
    enum FileFailurePoint {
        Write,
        Mode,
        Sync,
    }

    struct FailFileOperation(FileFailurePoint);

    impl PublicationHooks for FailFileOperation {
        fn after_file_create(&mut self, index: usize, _file: &File) -> io::Result<()> {
            if index == 1 && matches!(self.0, FileFailurePoint::Write) {
                Err(io::Error::other("injected file write failure"))
            } else {
                Ok(())
            }
        }

        fn after_file_write(&mut self, index: usize, _file: &File) -> io::Result<()> {
            if index == 1 && matches!(self.0, FileFailurePoint::Mode) {
                Err(io::Error::other("injected file mode failure"))
            } else {
                Ok(())
            }
        }

        fn after_file_mode(&mut self, index: usize, _file: &File) -> io::Result<()> {
            if index == 1 && matches!(self.0, FileFailurePoint::Sync) {
                Err(io::Error::other("injected file sync failure"))
            } else {
                Ok(())
            }
        }
    }

    #[test]
    fn every_postcreate_file_failure_rolls_back_recorded_entries() {
        for point in [
            FileFailurePoint::Write,
            FileFailurePoint::Mode,
            FileFailurePoint::Sync,
        ] {
            let test = TestDirectory::new(&format!("file-failure-{point:?}"));
            let destination = ReleaseDestination::open(test.path()).unwrap();
            let error = publish_with_hooks(
                &destination,
                &verified_release(),
                &mut FailFileOperation(point),
            )
            .unwrap_err();
            assert_eq!(error.code, "CC-RELEASE-PUBLISH-STAGE-001");
            assert!(
                release_entries(&test).is_empty(),
                "failure at {point:?} left transaction residue"
            );
            assert!(!visible_root(&test).exists());
        }
    }

    #[test]
    fn postcreate_directory_failure_rolls_back_recorded_entry() {
        struct FailDirectoryCreate;
        impl PublicationHooks for FailDirectoryCreate {
            fn after_directory_create(
                &mut self,
                path: &str,
                _directory: &Directory,
            ) -> io::Result<()> {
                assert_eq!(path, "artifacts");
                Err(io::Error::other("injected directory validation failure"))
            }
        }

        let test = TestDirectory::new("directory-failure");
        let destination = ReleaseDestination::open(test.path()).unwrap();
        let error = publish_with_hooks(&destination, &verified_release(), &mut FailDirectoryCreate)
            .unwrap_err();
        assert_eq!(error.code, "CC-RELEASE-PUBLISH-STAGE-001");
        assert!(release_entries(&test).is_empty());
        assert!(!visible_root(&test).exists());
    }

    #[test]
    fn rename_race_preserves_existing_non_directory_sentinel() {
        struct InstallSentinel;
        impl PublicationHooks for InstallSentinel {
            fn before_rename(
                &mut self,
                release_parent: &Directory,
                final_name: &CStr,
            ) -> io::Result<()> {
                let mut file = release_parent.create_file(final_name)?;
                file.write_all(b"existing-sentinel")?;
                file.sync_all()
            }
        }

        let test = TestDirectory::new("rename-race");
        let destination = ReleaseDestination::open(test.path()).unwrap();
        let error = publish_with_hooks(&destination, &verified_release(), &mut InstallSentinel)
            .unwrap_err();
        assert_eq!(error.code, "CC-RELEASE-PUBLISH-EXISTS-001");
        assert_eq!(fs::read(visible_root(&test)).unwrap(), b"existing-sentinel");
        assert_eq!(release_entries(&test), vec![IDENTITY]);
    }

    #[test]
    fn committed_rename_with_error_is_reconciled_as_visible_with_warning() {
        struct AmbiguousRename;
        impl PublicationHooks for AmbiguousRename {
            fn rename_release(
                &mut self,
                old_directory: &Directory,
                old_name: &CStr,
                new_directory: &Directory,
                new_name: &CStr,
            ) -> io::Result<()> {
                rename_noreplace(old_directory, old_name, new_directory, new_name)?;
                Err(io::Error::from_raw_os_error(libc::EIO))
            }
        }

        let test = TestDirectory::new("ambiguous-rename");
        let destination = ReleaseDestination::open(test.path()).unwrap();
        let outcome =
            publish_with_hooks(&destination, &verified_release(), &mut AmbiguousRename).unwrap();
        assert_eq!(
            outcome.durability,
            PublicationDurability::PublishedWithWarnings
        );
        assert!(
            outcome
                .warnings
                .iter()
                .any(|warning| { warning.code == "CC-RELEASE-PUBLISH-RENAME-AMBIGUOUS-001" })
        );
        assert_eq!(
            fs::read(visible_root(&test).join("manifest.json")).unwrap(),
            b"{\"all_pass\":true}\n"
        );
        assert_eq!(release_entries(&test), vec![IDENTITY]);
    }

    #[test]
    fn inconclusive_rename_preserves_residue_and_returns_indeterminate_state() {
        struct IndeterminateRename;
        impl PublicationHooks for IndeterminateRename {
            fn rename_release(
                &mut self,
                old_directory: &Directory,
                old_name: &CStr,
                _new_directory: &Directory,
                _new_name: &CStr,
            ) -> io::Result<()> {
                rename_noreplace(
                    old_directory,
                    old_name,
                    old_directory,
                    c".indeterminate-residue",
                )?;
                Err(io::Error::from_raw_os_error(libc::EIO))
            }
        }

        let test = TestDirectory::new("indeterminate-rename");
        let destination = ReleaseDestination::open(test.path()).unwrap();
        let outcome =
            publish_with_hooks(&destination, &verified_release(), &mut IndeterminateRename)
                .unwrap();
        assert_eq!(
            outcome.durability,
            PublicationDurability::VisibilityIndeterminate
        );
        assert!(
            outcome
                .warnings
                .iter()
                .any(|warning| { warning.code == "CC-RELEASE-PUBLISH-RENAME-INDETERMINATE-001" })
        );
        assert!(!visible_root(&test).exists());
        assert_eq!(release_entries(&test), vec![".indeterminate-residue"]);
        assert_eq!(
            fs::read(
                test.path()
                    .join("release/.indeterminate-residue/manifest.json")
            )
            .unwrap(),
            b"{\"all_pass\":true}\n"
        );
    }

    #[test]
    fn transient_final_probe_overrides_a_cached_source_identity() {
        #[derive(Clone, Copy, Debug)]
        enum FinalProbeFailure {
            Io,
            NotFound,
        }

        struct InconclusiveFinalProbe(FinalProbeFailure);
        impl PublicationHooks for InconclusiveFinalProbe {
            fn rename_release(
                &mut self,
                _old_directory: &Directory,
                _old_name: &CStr,
                _new_directory: &Directory,
                _new_name: &CStr,
            ) -> io::Result<()> {
                Err(io::Error::from_raw_os_error(libc::EIO))
            }

            fn probe_rename_identity(
                &mut self,
                directory: &Directory,
                name: &CStr,
            ) -> io::Result<(u64, u64)> {
                if name.to_bytes() == IDENTITY.as_bytes() {
                    match self.0 {
                        FinalProbeFailure::Io => Err(io::Error::from_raw_os_error(libc::EIO)),
                        FinalProbeFailure::NotFound => {
                            Err(io::Error::from(io::ErrorKind::NotFound))
                        }
                    }
                } else {
                    directory.open_child(name)?.identity()
                }
            }
        }

        for failure in [FinalProbeFailure::Io, FinalProbeFailure::NotFound] {
            let test = TestDirectory::new(&format!("inconclusive-final-probe-{failure:?}"));
            let destination = ReleaseDestination::open(test.path()).unwrap();
            let outcome = publish_with_hooks(
                &destination,
                &verified_release(),
                &mut InconclusiveFinalProbe(failure),
            )
            .unwrap();
            assert_eq!(
                outcome.durability,
                PublicationDurability::VisibilityIndeterminate
            );
            assert!(!visible_root(&test).exists());
            let entries = release_entries(&test);
            assert_eq!(entries.len(), 1);
            assert!(entries[0].starts_with(".circuitc-release-transaction-"));
            assert_eq!(
                fs::read(
                    test.path()
                        .join("release")
                        .join(&entries[0])
                        .join("manifest.json")
                )
                .unwrap(),
                b"{\"all_pass\":true}\n"
            );
        }
    }

    #[test]
    fn destination_and_release_namespace_must_be_private_and_nofollow() {
        let insecure = TestDirectory::new("insecure");
        fs::set_permissions(insecure.path(), fs::Permissions::from_mode(0o770)).unwrap();
        assert_eq!(
            ReleaseDestination::open(insecure.path()).unwrap_err().code,
            "CC-RELEASE-PUBLISH-DESTINATION-001"
        );

        let linked = TestDirectory::new("linked");
        symlink("outside", linked.path().join("release")).unwrap();
        let destination = ReleaseDestination::open(linked.path()).unwrap();
        assert_eq!(
            publish_release(&destination, &verified_release())
                .unwrap_err()
                .code,
            "CC-RELEASE-PUBLISH-DESTINATION-001"
        );

        let unsafe_ancestor = TestDirectory::new("unsafe-ancestor");
        let writable = unsafe_ancestor.path().join("writable");
        let nested_destination = writable.join("destination");
        fs::create_dir(&writable).unwrap();
        fs::create_dir(&nested_destination).unwrap();
        fs::set_permissions(&writable, fs::Permissions::from_mode(0o777)).unwrap();
        fs::set_permissions(&nested_destination, fs::Permissions::from_mode(0o700)).unwrap();
        assert_eq!(
            ReleaseDestination::open(&nested_destination)
                .unwrap_err()
                .code,
            "CC-RELEASE-PUBLISH-DESTINATION-001"
        );
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn destination_rejects_a_mode_private_darwin_extended_acl() {
        use std::process::Command;

        let test = TestDirectory::new("extended-acl");
        let status = Command::new("/bin/chmod")
            .arg("+a")
            .arg("everyone allow add_file,add_subdirectory,delete_child")
            .arg(test.path())
            .status()
            .unwrap();
        assert!(status.success());
        assert_eq!(
            ReleaseDestination::open(test.path()).unwrap_err().code,
            "CC-RELEASE-PUBLISH-DESTINATION-001"
        );
        let status = Command::new("/bin/chmod")
            .arg("-N")
            .arg(test.path())
            .status()
            .unwrap();
        assert!(status.success());
    }

    #[test]
    fn private_directory_creation_is_independent_of_process_umask() {
        use std::process::Command;

        const CHILD: &str = "CIRCUITC_RELEASE_UMASK_CHILD";
        if std::env::var_os(CHILD).is_some() {
            let test = TestDirectory::new("umask-child");
            let destination = ReleaseDestination::open(test.path()).unwrap();
            // SAFETY: this exact test runs alone in a subprocess, so changing
            // the process-global umask cannot race another test thread.
            let previous = unsafe { libc::umask(0o777) };
            let outcome = publish_release(&destination, &verified_release()).unwrap();
            // SAFETY: restore the child process umask before normal teardown.
            unsafe { libc::umask(previous) };
            assert_eq!(outcome.durability, PublicationDurability::Durable);
            assert!(visible_root(&test).is_dir());
            return;
        }

        let status = Command::new(std::env::current_exe().unwrap())
            .arg("--exact")
            .arg("release::publish::tests::private_directory_creation_is_independent_of_process_umask")
            .arg("--nocapture")
            .env(CHILD, "1")
            .status()
            .unwrap();
        assert!(status.success());
    }

    #[test]
    fn retained_destination_descriptor_cannot_be_redirected_by_path_replacement() {
        let test = TestDirectory::new("retained-destination");
        let anchor = test.path().join("anchor");
        let moved = test.path().join("moved-anchor");
        fs::create_dir(&anchor).unwrap();
        fs::set_permissions(&anchor, fs::Permissions::from_mode(0o700)).unwrap();
        let destination = ReleaseDestination::open(&anchor).unwrap();
        fs::rename(&anchor, &moved).unwrap();
        fs::create_dir(&anchor).unwrap();
        fs::set_permissions(&anchor, fs::Permissions::from_mode(0o700)).unwrap();

        let outcome = publish_release(&destination, &verified_release()).unwrap();
        assert_eq!(outcome.durability, PublicationDurability::Durable);
        assert!(moved.join("release").join(IDENTITY).is_dir());
        assert!(!anchor.join("release").exists());
    }

    #[test]
    fn prepublish_exact_tree_check_rejects_unknown_staging_entries() {
        struct AddUnknownEntry;
        impl PublicationHooks for AddUnknownEntry {
            fn before_rename(
                &mut self,
                release_parent: &Directory,
                _final_name: &CStr,
            ) -> io::Result<()> {
                let transaction = release_parent
                    .list_entries()?
                    .into_iter()
                    .find(|name| name.starts_with(".circuitc-release-transaction-"))
                    .expect("one live transaction");
                let staging = release_parent.open_child(&c_name(OsStr::new(&transaction))?)?;
                staging.set_mode(0o700)?;
                let mut unknown = staging.create_file(c"unknown")?;
                unknown.write_all(b"unverified")?;
                unknown.set_permissions(Permissions::from_mode(0o400))?;
                unknown.sync_all()?;
                staging.set_mode(0o500)
            }
        }

        let test = TestDirectory::new("prepublish-unknown");
        let destination = ReleaseDestination::open(test.path()).unwrap();
        let error = publish_with_hooks(&destination, &verified_release(), &mut AddUnknownEntry)
            .unwrap_err();
        assert_eq!(error.code, "CC-RELEASE-PUBLISH-STAGE-001");
        assert!(error.message.contains("unexpected inventory"));
        assert!(error.message.contains("transaction cleanup was incomplete"));
        assert!(!visible_root(&test).exists());
        let entries = release_entries(&test);
        assert_eq!(entries.len(), 1);
        assert!(entries[0].starts_with(".circuitc-release-transaction-"));
        assert_eq!(
            fs::read(
                test.path()
                    .join("release")
                    .join(&entries[0])
                    .join("unknown")
            )
            .unwrap(),
            b"unverified"
        );
    }

    #[test]
    fn postrename_sync_failure_is_a_warning_and_never_rolls_back() {
        struct FailSync;
        impl PublicationHooks for FailSync {
            fn sync_release_parent(&mut self, _release_parent: &Directory) -> io::Result<()> {
                Err(io::Error::other("injected directory fsync failure"))
            }
        }

        let test = TestDirectory::new("sync-warning");
        let destination = ReleaseDestination::open(test.path()).unwrap();
        let outcome = publish_with_hooks(&destination, &verified_release(), &mut FailSync).unwrap();
        assert_eq!(
            outcome.durability,
            PublicationDurability::PublishedWithWarnings
        );
        assert_eq!(
            outcome.warnings[0].code,
            "CC-RELEASE-PUBLISH-DURABILITY-001"
        );
        assert!(visible_root(&test).is_dir());
    }

    #[test]
    fn postrename_mutation_is_reported_without_deleting_visible_release() {
        struct MutateVisible;
        impl PublicationHooks for MutateVisible {
            fn after_rename(&mut self, final_directory: &Directory) -> io::Result<()> {
                final_directory.set_mode(0o700)?;
                let request = c"request.json";
                final_directory.remove_file(request)?;
                let mut replacement = final_directory.create_file(request)?;
                replacement.write_all(b"tampered")?;
                replacement.set_permissions(Permissions::from_mode(0o400))?;
                replacement.sync_all()?;
                final_directory.set_mode(0o500)
            }
        }

        let test = TestDirectory::new("post-mutation");
        let destination = ReleaseDestination::open(test.path()).unwrap();
        let outcome =
            publish_with_hooks(&destination, &verified_release(), &mut MutateVisible).unwrap();
        assert_eq!(
            outcome.durability,
            PublicationDurability::PublishedWithWarnings
        );
        assert!(
            outcome
                .warnings
                .iter()
                .any(|warning| warning.code == "CC-RELEASE-PUBLISH-VERIFY-001")
        );
        assert_eq!(
            fs::read(visible_root(&test).join("request.json")).unwrap(),
            b"tampered"
        );
    }

    #[derive(Clone, Copy, Debug)]
    enum PublishedTreeMutation {
        WritableFile,
        HardLink,
        SymlinkReplacement,
        EqualFileReplacement,
        EqualDirectoryReplacement,
        EqualRootReplacement,
        ExtraEntry,
    }

    struct MutatePublishedTree {
        kind: PublishedTreeMutation,
        visible: PathBuf,
        destination: PathBuf,
    }

    impl PublicationHooks for MutatePublishedTree {
        fn after_rename(&mut self, _final_directory: &Directory) -> io::Result<()> {
            let request = self.visible.join("request.json");
            match self.kind {
                PublishedTreeMutation::WritableFile => {
                    fs::set_permissions(request, fs::Permissions::from_mode(0o600))?;
                }
                PublishedTreeMutation::HardLink => {
                    fs::hard_link(request, self.destination.join("outside-hardlink"))?;
                }
                PublishedTreeMutation::SymlinkReplacement => {
                    fs::write(self.destination.join("outside"), b"outside")?;
                    fs::set_permissions(&self.visible, fs::Permissions::from_mode(0o700))?;
                    fs::remove_file(&request)?;
                    symlink("../../outside", &request)?;
                    fs::set_permissions(&self.visible, fs::Permissions::from_mode(0o500))?;
                }
                PublishedTreeMutation::EqualFileReplacement => {
                    fs::set_permissions(&self.visible, fs::Permissions::from_mode(0o700))?;
                    fs::remove_file(&request)?;
                    fs::write(&request, b"{\"request\":true}\n")?;
                    fs::set_permissions(&request, fs::Permissions::from_mode(0o400))?;
                    fs::set_permissions(&self.visible, fs::Permissions::from_mode(0o500))?;
                }
                PublishedTreeMutation::EqualDirectoryReplacement => {
                    let artifacts = self.visible.join("artifacts");
                    fs::set_permissions(&self.visible, fs::Permissions::from_mode(0o700))?;
                    fs::set_permissions(&artifacts, fs::Permissions::from_mode(0o700))?;
                    fs::remove_file(artifacts.join("board.bin"))?;
                    fs::remove_dir(&artifacts)?;
                    fs::create_dir(&artifacts)?;
                    fs::write(artifacts.join("board.bin"), b"exact-board-bytes")?;
                    fs::set_permissions(
                        artifacts.join("board.bin"),
                        fs::Permissions::from_mode(0o400),
                    )?;
                    fs::set_permissions(&artifacts, fs::Permissions::from_mode(0o500))?;
                    fs::set_permissions(&self.visible, fs::Permissions::from_mode(0o500))?;
                }
                PublishedTreeMutation::EqualRootReplacement => {
                    let release_parent = self.visible.parent().expect("visible release parent");
                    let old_root = release_parent.join(".replaced-root");
                    fs::rename(&self.visible, &old_root)?;
                    fs::create_dir(&self.visible)?;
                    fs::set_permissions(&self.visible, fs::Permissions::from_mode(0o700))?;
                    for name in ["request.json", "artifacts", "manifest.json"] {
                        fs::rename(old_root.join(name), self.visible.join(name))?;
                    }
                    fs::set_permissions(&self.visible, fs::Permissions::from_mode(0o500))?;
                    fs::remove_dir(old_root)?;
                }
                PublishedTreeMutation::ExtraEntry => {
                    fs::set_permissions(&self.visible, fs::Permissions::from_mode(0o700))?;
                    let extra = self.visible.join("extra");
                    fs::write(&extra, b"unverified")?;
                    fs::set_permissions(extra, fs::Permissions::from_mode(0o400))?;
                    fs::set_permissions(&self.visible, fs::Permissions::from_mode(0o500))?;
                }
            }
            Ok(())
        }
    }

    #[test]
    fn postpublication_inventory_metadata_and_identity_mutants_warn_without_rollback() {
        for kind in [
            PublishedTreeMutation::WritableFile,
            PublishedTreeMutation::HardLink,
            PublishedTreeMutation::SymlinkReplacement,
            PublishedTreeMutation::EqualFileReplacement,
            PublishedTreeMutation::EqualDirectoryReplacement,
            PublishedTreeMutation::EqualRootReplacement,
            PublishedTreeMutation::ExtraEntry,
        ] {
            let test = TestDirectory::new(&format!("post-tree-{kind:?}"));
            let destination = ReleaseDestination::open(test.path()).unwrap();
            let visible = visible_root(&test);
            let mut hook = MutatePublishedTree {
                kind,
                visible: visible.clone(),
                destination: test.path().to_owned(),
            };
            let outcome = publish_with_hooks(&destination, &verified_release(), &mut hook).unwrap();
            assert_eq!(
                outcome.durability,
                PublicationDurability::PublishedWithWarnings,
                "mutation {kind:?} must not be accepted as durable"
            );
            assert!(
                outcome
                    .warnings
                    .iter()
                    .any(|warning| warning.code == "CC-RELEASE-PUBLISH-VERIFY-001"),
                "mutation {kind:?} must produce an exact-tree warning"
            );
            assert!(visible.exists(), "mutation {kind:?} must never roll back");
        }
    }

    #[test]
    fn concurrent_publishers_expose_exactly_one_release() {
        let test = TestDirectory::new("concurrent");
        let destination = Arc::new(ReleaseDestination::open(test.path()).unwrap());
        let first_destination = Arc::clone(&destination);
        let second_destination = Arc::clone(&destination);
        let first =
            std::thread::spawn(move || publish_release(&first_destination, &verified_release()));
        let second =
            std::thread::spawn(move || publish_release(&second_destination, &verified_release()));
        let outcomes = [first.join().unwrap(), second.join().unwrap()];
        assert_eq!(outcomes.iter().filter(|result| result.is_ok()).count(), 1);
        assert_eq!(
            outcomes
                .iter()
                .filter_map(|result| result.as_ref().err())
                .map(|error| error.code)
                .collect::<Vec<_>>(),
            vec!["CC-RELEASE-PUBLISH-EXISTS-001"]
        );
        assert_eq!(release_entries(&test), vec![IDENTITY]);
    }

    #[test]
    fn cleanup_preserves_identity_changed_residue_for_recovery() {
        struct ReplaceThenFail;
        impl PublicationHooks for ReplaceThenFail {
            fn after_file(&mut self, index: usize, staging: &Directory) -> io::Result<()> {
                if index != 0 {
                    return Ok(());
                }
                let request = c"request.json";
                staging.remove_file(request)?;
                let mut replacement = staging.create_file(request)?;
                replacement.write_all(b"replacement-residue")?;
                Err(io::Error::other("injected interruption after replacement"))
            }
        }

        let test = TestDirectory::new("preserve-residue");
        let destination = ReleaseDestination::open(test.path()).unwrap();
        let error = publish_with_hooks(&destination, &verified_release(), &mut ReplaceThenFail)
            .unwrap_err();
        assert!(error.message.contains("preserved replacement staged file"));
        let entries = release_entries(&test);
        assert_eq!(entries.len(), 1);
        assert!(entries[0].starts_with(".circuitc-release-transaction-"));
        assert_eq!(
            fs::read(
                test.path()
                    .join("release")
                    .join(&entries[0])
                    .join("request.json")
            )
            .unwrap(),
            b"replacement-residue"
        );
        assert!(!visible_root(&test).exists());
    }
}
