use std::fmt;

use serde::{Deserialize, Serialize};

use crate::{CheckedCompiledArtifacts, CompiledArtifacts, RelativeArtifactPath};

pub(crate) const SCHEMA_VERSION: u32 = 1;
pub(crate) const REQUEST_SCHEMA: &str = "circuitc.fabrication_request";
pub(crate) const MANIFEST_SCHEMA: &str = "circuitc.fabrication_manifest";
pub(crate) const KICAD_ADAPTER: &str = "kicad";
pub(crate) const KICAD_MAJOR: u32 = 10;
pub(crate) const KICAD_VERSION: &str = "10.0.5";
pub(crate) const MAX_FILE_BYTES: usize = 64 * 1024 * 1024;
pub(crate) const MAX_AGGREGATE_BYTES: usize = 256 * 1024 * 1024;
pub(crate) const MAX_POSITION_ROWS: usize = 10_000;
pub(crate) const FABRICATION_IDENTITY_DOMAIN: &[u8] = b"CIRCUITC-FABRICATION-IDENTITY-V1\0";

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct FabricationDiagnostic {
    pub code: &'static str,
    pub path: String,
    pub message: String,
}

impl fmt::Display for FabricationDiagnostic {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(formatter, "{} [{}]: {}", self.code, self.path, self.message)
    }
}

impl std::error::Error for FabricationDiagnostic {}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct FabricationRequestBundle {
    pub fabrication_identity_sha256: String,
    pub request_path: RelativeArtifactPath,
    pub manifest_path: RelativeArtifactPath,
    pub request_json: String,
    pub expected_host_paths: Vec<RelativeArtifactPath>,
}

#[derive(Clone, Copy, Debug)]
pub enum FabricationCompilerArtifacts<'a> {
    Static(&'a CompiledArtifacts),
    Checked(&'a CheckedCompiledArtifacts),
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct FabricationHostFile {
    pub path: RelativeArtifactPath,
    pub contents: Vec<u8>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct FabricationFile {
    pub path: RelativeArtifactPath,
    pub contents: Vec<u8>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct FabricationManifestBundle {
    pub(crate) fabrication_identity_sha256: String,
    pub(crate) request_path: RelativeArtifactPath,
    pub(crate) manifest_path: RelativeArtifactPath,
    pub(crate) request_json: String,
    pub(crate) manifest_json: String,
    pub(crate) files: Vec<FabricationFile>,
}

impl FabricationManifestBundle {
    pub fn fabrication_identity_sha256(&self) -> &str {
        &self.fabrication_identity_sha256
    }

    pub fn request_path(&self) -> &RelativeArtifactPath {
        &self.request_path
    }

    pub fn manifest_path(&self) -> &RelativeArtifactPath {
        &self.manifest_path
    }

    pub fn request_json(&self) -> &str {
        &self.request_json
    }

    pub fn manifest_json(&self) -> &str {
        &self.manifest_json
    }

    pub fn files(&self) -> &[FabricationFile] {
        &self.files
    }
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct ArtifactBinding {
    pub path: String,
    pub sha256: String,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct ResourcePolicy {
    pub timeout_ms: u32,
    pub stdout_bytes: u64,
    pub stderr_bytes: u64,
    pub file_bytes: u64,
    pub aggregate_bytes: u64,
    pub primary_rows: u32,
    pub diagnostics: u32,
}

impl Default for ResourcePolicy {
    fn default() -> Self {
        Self {
            timeout_ms: 120_000,
            stdout_bytes: 1_048_576,
            stderr_bytes: 1_048_576,
            file_bytes: MAX_FILE_BYTES as u64,
            aggregate_bytes: MAX_AGGREGATE_BYTES as u64,
            primary_rows: MAX_POSITION_ROWS as u32,
            diagnostics: 256,
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct GerberLayerProfile {
    pub layer_id: u32,
    pub layer_name: String,
    pub file_function: String,
    pub job_file_function: String,
    pub file_polarity: String,
    pub path: String,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct GerberProfile {
    pub format: String,
    pub precision: u32,
    pub net_attributes: bool,
    pub protel_extensions: bool,
    pub origin: String,
    pub board_plot_params: bool,
    pub layers: Vec<GerberLayerProfile>,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct DrillProfile {
    pub format: String,
    pub origin: String,
    pub units: String,
    pub zero_format: String,
    pub oval_format: String,
    pub mirror_y: bool,
    pub minimal_header: bool,
    pub separate_plated: bool,
    pub generate_map: bool,
    pub generate_report: bool,
    pub generate_tenting: bool,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct PositionProfile {
    pub format: String,
    pub units: String,
    pub side: String,
    pub origin: String,
    pub bottom_negate_x: bool,
    pub smd_only: bool,
    pub exclude_through_hole: bool,
    pub exclude_dnp: bool,
    pub variant: Option<String>,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct ExportProfile {
    pub gerber: GerberProfile,
    pub drill: DrillProfile,
    pub position: PositionProfile,
    pub resources: ResourcePolicy,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct OutputDescriptor {
    pub role: String,
    pub path: String,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct FabricationIdentityPreimage {
    pub design_name: String,
    pub analysis_path: String,
    pub assertion_path: String,
    pub variant_path: String,
    pub variant_identity_sha256: String,
    pub product_input_sha256: String,
    pub product_resolution_sha256: String,
    pub placement_sha256: String,
    pub catalog_evaluated_on: String,
    pub kicad_pcb: ArtifactBinding,
    pub expected_adapter: String,
    pub expected_major: u32,
    pub expected_version: String,
    pub export_profile: ExportProfile,
    pub outputs: Vec<OutputDescriptor>,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct FabricationRequest {
    pub schema_name: String,
    pub schema_version: u32,
    pub design_name: String,
    pub analysis_path: String,
    pub assertion_path: String,
    pub variant_path: String,
    pub variant_identity_sha256: String,
    pub product_input_sha256: String,
    pub product_resolution_sha256: String,
    pub placement_sha256: String,
    pub catalog_evaluated_on: String,
    pub kicad_pcb: ArtifactBinding,
    pub expected_adapter: String,
    pub expected_major: u32,
    pub expected_version: String,
    pub fabrication_identity_sha256: String,
    pub export_profile: ExportProfile,
    pub outputs: Vec<OutputDescriptor>,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct ExporterIdentity {
    pub adapter: String,
    pub version: String,
    pub executable_sha256: String,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct FileBinding {
    pub path: String,
    pub byte_length: u64,
    pub sha256: String,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct GerberBinding {
    pub layer_id: u32,
    pub layer_name: String,
    pub file_function: String,
    pub path: String,
    pub byte_length: u64,
    pub sha256: String,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct GerberJobBinding {
    pub path: String,
    pub byte_length: u64,
    pub sha256: String,
    pub gerber_count: u32,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct DrillBinding {
    pub kind: String,
    pub path: String,
    pub byte_length: u64,
    pub sha256: String,
    pub tool_count: u32,
    pub round_hit_count: u64,
    pub slot_hit_count: u64,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct PositionBinding {
    pub path: String,
    pub byte_length: u64,
    pub sha256: String,
    pub row_count: u64,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct PositionRow {
    pub component_path: String,
    pub reference: String,
    pub host_value: String,
    pub host_package: String,
    pub x_nm: i64,
    pub y_nm: i64,
    pub rotation_degrees: i16,
    pub side: String,
    pub state: String,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct FabricationManifest {
    pub schema_name: String,
    pub schema_version: u32,
    pub design_name: String,
    pub analysis_path: String,
    pub assertion_path: String,
    pub variant_path: String,
    pub variant_identity_sha256: String,
    pub product_input_sha256: String,
    pub product_resolution_sha256: String,
    pub placement_sha256: String,
    pub catalog_evaluated_on: String,
    pub kicad_pcb: ArtifactBinding,
    pub fabrication_identity_sha256: String,
    pub request: FileBinding,
    pub exporter: ExporterIdentity,
    pub export_profile: ExportProfile,
    pub gerbers: Vec<GerberBinding>,
    pub gerber_job: GerberJobBinding,
    pub drills: Vec<DrillBinding>,
    pub position_csv: PositionBinding,
    pub position_rows: Vec<PositionRow>,
}
