//! Deterministic KiCad fabrication export contracts.
//!
//! CircuitC owns the request, normalization, product joins, and manifest.
//! KiCad 10.0.5 remains the authority that parses the exact generated board
//! and produces the transient native export bytes.

mod bind;
mod contract;
mod normalize;

pub use bind::{
    bind_kicad10_fabrication, prepare_kicad10_fabrication_request,
    verify_kicad10_fabrication_manifest,
};
pub use contract::{
    FabricationCompilerArtifacts, FabricationDiagnostic, FabricationFile, FabricationHostFile,
    FabricationManifestBundle, FabricationRequestBundle,
};
