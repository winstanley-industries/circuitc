use std::fmt;

use serde::{Deserialize, Serialize};

use crate::RelativeArtifactPath;
use crate::design::Design;
use crate::manufacturing::{
    FabricationCompilerArtifacts, FabricationHostFile, FabricationManifestBundle,
};
use crate::product::ProductArtifactBundle;
use crate::product_analysis::{BoardAnalysisBundle, BoardAnalysisHostEvidence};

pub(crate) const REQUEST_SCHEMA: &str = "circuitc.release_request";
pub(crate) const MANIFEST_SCHEMA: &str = "circuitc.release_manifest";
pub(crate) const SCHEMA_VERSION: u32 = 1;
pub(crate) const RELEASE_IDENTITY_DOMAIN: &[u8] = b"CIRCUITC-RELEASE-IDENTITY-V1\0";
pub(crate) const MAX_FILE_BYTES: usize = 64 * 1024 * 1024;
pub(crate) const MAX_FILES: usize = 4_096;
pub(crate) const MAX_PATH_BYTES: usize = 1_048_576;
pub(crate) const MAX_AGGREGATE_BYTES: usize = 1024 * 1024 * 1024;

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ReleaseDiagnostic {
    pub code: &'static str,
    pub path: String,
    pub message: String,
}

impl fmt::Display for ReleaseDiagnostic {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(formatter, "{} [{}]: {}", self.code, self.path, self.message)
    }
}

impl std::error::Error for ReleaseDiagnostic {}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ReleaseFile {
    pub path: RelativeArtifactPath,
    pub contents: Vec<u8>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ReleaseBundle {
    pub(crate) release_identity_sha256: String,
    pub(crate) root: RelativeArtifactPath,
    pub(crate) request_json: String,
    pub(crate) manifest_json: String,
    pub(crate) files: Vec<ReleaseFile>,
}

impl ReleaseBundle {
    pub fn release_identity_sha256(&self) -> &str {
        &self.release_identity_sha256
    }

    pub fn root(&self) -> &RelativeArtifactPath {
        &self.root
    }

    pub fn request_json(&self) -> &str {
        &self.request_json
    }

    pub fn manifest_json(&self) -> &str {
        &self.manifest_json
    }

    /// Complete publication inventory, including `request.json` first and the
    /// `manifest.json` completion sentinel last.
    pub fn files(&self) -> &[ReleaseFile] {
        &self.files
    }
}

#[derive(Debug)]
pub struct VerifiedReleaseBundle(pub(crate) ReleaseBundle);

impl VerifiedReleaseBundle {
    pub fn bundle(&self) -> &ReleaseBundle {
        &self.0
    }
}

#[derive(Clone, Copy)]
pub struct ReleaseFabricationEvidence<'a> {
    pub analysis_path: &'a str,
    pub assertion_path: &'a str,
    pub host_version: &'a str,
    pub host_executable: &'a [u8],
    pub host_files: &'a [FabricationHostFile],
    pub bundle: &'a FabricationManifestBundle,
}

#[derive(Clone, Copy)]
pub struct ReleaseAnalysisEvidence<'a> {
    pub analysis_path: &'a str,
    pub host: &'a BoardAnalysisHostEvidence,
    pub bundle: &'a BoardAnalysisBundle,
}

#[derive(Clone, Copy)]
pub struct ReleaseRoutingEvidence<'a> {
    /// Canonical `circuitc.apgar_route_acceptance` v1 bytes.
    pub acceptance_json: &'a str,
}

#[derive(Clone, Copy)]
pub struct ReleaseToolchainEvidence<'a> {
    pub ohmnivore_executable: Option<&'a [u8]>,
    pub ohmnivore_provenance: Option<&'a [u8]>,
    pub apgar_executable: Option<&'a [u8]>,
    pub apgar_provenance: Option<&'a [u8]>,
}

#[derive(Clone, Copy)]
pub struct ReleaseInputs<'a> {
    pub source: &'a str,
    pub design: &'a Design,
    pub catalog_snapshot: &'a [u8],
    pub variant_path: &'a str,
    pub compiler: FabricationCompilerArtifacts<'a>,
    pub kicad_identity_map_json: &'a str,
    pub product: &'a ProductArtifactBundle,
    pub fabrication: ReleaseFabricationEvidence<'a>,
    pub analysis: ReleaseAnalysisEvidence<'a>,
    pub routing: Option<ReleaseRoutingEvidence<'a>>,
    pub tools: ReleaseToolchainEvidence<'a>,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct ArtifactBinding {
    pub role: String,
    pub path: String,
    pub byte_length: u64,
    pub sha256: String,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct ToolBinding {
    pub role: String,
    pub name: String,
    pub version: String,
    pub source_revision: String,
    pub byte_length: u64,
    pub sha256: String,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct Applicability {
    pub simulation: bool,
    pub routing: bool,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct ResourcePolicy {
    pub file_bytes: u64,
    pub file_count: u32,
    pub path_bytes: u64,
    pub consumed_aggregate_bytes: u64,
    pub emitted_aggregate_bytes: u64,
}

impl Default for ResourcePolicy {
    fn default() -> Self {
        Self {
            file_bytes: MAX_FILE_BYTES as u64,
            file_count: MAX_FILES as u32,
            path_bytes: MAX_PATH_BYTES as u64,
            consumed_aggregate_bytes: MAX_AGGREGATE_BYTES as u64,
            emitted_aggregate_bytes: MAX_AGGREGATE_BYTES as u64,
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub(crate) struct ReleaseIdentityPreimage {
    pub schema_name: String,
    pub schema_version: u32,
    pub design_name: String,
    pub variant_path: String,
    pub variant_identity_sha256: String,
    pub product_input_sha256: String,
    pub source: ArtifactBinding,
    pub design_identity_sha256: String,
    pub catalog: ArtifactBinding,
    pub applicability: Applicability,
    pub tools: Vec<ToolBinding>,
    pub artifacts: Vec<ArtifactBinding>,
    pub resources: ResourcePolicy,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct ReleaseRequest {
    pub schema_name: String,
    pub schema_version: u32,
    pub release_identity_sha256: String,
    pub design_name: String,
    pub variant_path: String,
    pub variant_identity_sha256: String,
    pub product_input_sha256: String,
    pub source: ArtifactBinding,
    pub design_identity_sha256: String,
    pub catalog: ArtifactBinding,
    pub applicability: Applicability,
    pub tools: Vec<ToolBinding>,
    pub artifacts: Vec<ArtifactBinding>,
    pub resources: ResourcePolicy,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct RequestBinding {
    pub path: String,
    pub byte_length: u64,
    pub sha256: String,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct ValidationOutcome {
    pub capability: String,
    pub evidence_role: String,
    pub outcome: String,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct ReleaseManifest {
    pub schema_name: String,
    pub schema_version: u32,
    pub release_identity_sha256: String,
    pub request: RequestBinding,
    pub source: ArtifactBinding,
    pub design_identity_sha256: String,
    pub applicability: Applicability,
    pub tools: Vec<ToolBinding>,
    pub validations: Vec<ValidationOutcome>,
    pub artifacts: Vec<ArtifactBinding>,
    pub all_pass: bool,
}

pub(crate) fn diagnostic(
    code: &'static str,
    path: impl Into<String>,
    message: impl Into<String>,
) -> ReleaseDiagnostic {
    ReleaseDiagnostic {
        code,
        path: path.into(),
        message: message.into(),
    }
}
