//! Independently verified, immutable release closures.

mod bind;
mod contract;
mod identity;
mod publish;

pub use bind::{assemble_release, bind_release, verify_release};
pub use contract::{
    ReleaseAnalysisEvidence, ReleaseBundle, ReleaseDiagnostic, ReleaseFabricationEvidence,
    ReleaseFile, ReleaseInputs, ReleaseRoutingEvidence, ReleaseToolchainEvidence,
    VerifiedReleaseBundle,
};
pub use identity::canonical_design_identity;
pub use publish::{
    PublicationDurability, ReleaseDestination, ReleasePublication, ReleasePublicationWarning,
    ReleasePublishError, publish_release,
};
