//! CircuitC compiler foundation.
//!
//! The M2 frontend parses the deliberately small, unreleased CircuitC
//! language and elaborates it into the same canonical Design IR used by the
//! programmatic regression fixtures. Until the first release, the language,
//! schema, and Rust construction API evolve in place without compatibility
//! guarantees or migrations.

mod compile;
pub mod demo;
pub mod design;
pub mod frontend;
mod kicad;
mod library;
pub mod quantity;
// The routing contract is integrated with checked compilation in a later
// stacked layer. Keep this independently tested contract slice lint-clean
// while its production process boundary is intentionally not reachable yet.
#[allow(dead_code)]
pub(crate) mod routing;
pub mod simulation;
mod spice;

pub use compile::{
    CheckedCompileError, CheckedCompiledArtifacts, CompileError, CompiledArtifacts,
    CompiledSimulation, InvalidRelativeArtifactPath, KicadIdentity, KicadLibraryFile,
    KicadLibraryFileKind, RelativeArtifactPath, compile, compile_checked,
};
pub use spice::{SpiceComponentNameMapping, SpiceNameMap, SpiceNetNameMapping};
