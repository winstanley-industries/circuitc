mod artifacts;
mod catalog;

pub use artifacts::{
    ProductArtifactBundle, ProductArtifactDiagnostic, compile_product_artifacts,
    verify_product_artifact_bundle,
};
pub use catalog::{
    CatalogDiagnostic, CatalogResolution, ResolvedCatalogPart, verify_product_catalog,
};
