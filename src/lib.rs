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
pub mod manufacturing;
pub mod product;
pub mod product_analysis;
pub mod quantity;
pub(crate) mod routing;
pub mod simulation;
mod spice;

pub use compile::{
    CheckedCompileError, CheckedCompiledArtifacts, CompileError, CompiledArtifacts,
    CompiledRouting, CompiledSimulation, InvalidRelativeArtifactPath, KicadIdentity,
    KicadLibraryFile, KicadLibraryFileKind, RelativeArtifactPath, compile, compile_checked,
};
pub use spice::{SpiceComponentNameMapping, SpiceNameMap, SpiceNetNameMapping};

#[doc(hidden)]
pub fn verify_apgar_route_evidence(
    request_json: &str,
    result_json: &str,
    provenance: &str,
) -> Result<String, String> {
    routing::evidence::verify(request_json, result_json, provenance)
        .map_err(|error| error.to_string())
}
