use std::fmt;

use serde::{Deserialize, Serialize};

use crate::RelativeArtifactPath;

pub(crate) const SCHEMA_VERSION: u32 = 1;
pub(crate) const REQUEST_SCHEMA: &str = "circuitc.board_analysis_request";
pub(crate) const RESULT_SCHEMA: &str = "circuitc.board_analysis_result";
pub(crate) const REPORT_SCHEMA: &str = "circuitc.board_analysis_report";
pub(crate) const ADAPTER: &str = "kicad";
pub(crate) const ADAPTER_MAJOR: u32 = 10;
pub(crate) const ADAPTER_VERSION: &str = "10.0.5";
pub(crate) const IDENTITY_DOMAIN: &[u8] = b"CIRCUITC-BOARD-ANALYSIS-IDENTITY-V1\0";
pub(crate) const MAX_FILE_BYTES: usize = 64 * 1024 * 1024;
pub(crate) const MAX_AGGREGATE_BYTES: usize = 256 * 1024 * 1024;

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct BoardAnalysisDiagnostic {
    pub code: &'static str,
    pub path: String,
    pub message: String,
}

impl fmt::Display for BoardAnalysisDiagnostic {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(formatter, "{} [{}]: {}", self.code, self.path, self.message)
    }
}

impl std::error::Error for BoardAnalysisDiagnostic {}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct BoardAnalysisRequestBundle {
    pub(crate) analysis_identity_sha256: String,
    pub(crate) request_path: RelativeArtifactPath,
    pub(crate) result_path: RelativeArtifactPath,
    pub(crate) report_path: RelativeArtifactPath,
    pub(crate) expected_host_paths: Vec<RelativeArtifactPath>,
    pub(crate) request_json: String,
}

impl BoardAnalysisRequestBundle {
    pub fn analysis_identity_sha256(&self) -> &str {
        &self.analysis_identity_sha256
    }

    pub fn request_path(&self) -> &RelativeArtifactPath {
        &self.request_path
    }

    pub fn result_path(&self) -> &RelativeArtifactPath {
        &self.result_path
    }

    pub fn report_path(&self) -> &RelativeArtifactPath {
        &self.report_path
    }

    pub fn expected_host_paths(&self) -> &[RelativeArtifactPath] {
        &self.expected_host_paths
    }

    pub fn request_json(&self) -> &str {
        &self.request_json
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct BoardAnalysisFile {
    pub path: RelativeArtifactPath,
    pub contents: Vec<u8>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct BoardAnalysisBundle {
    pub(crate) analysis_identity_sha256: String,
    pub(crate) request_path: RelativeArtifactPath,
    pub(crate) result_path: RelativeArtifactPath,
    pub(crate) report_path: RelativeArtifactPath,
    pub(crate) request_json: String,
    pub(crate) result_json: String,
    pub(crate) report_json: String,
    pub(crate) files: Vec<BoardAnalysisFile>,
}

impl BoardAnalysisBundle {
    pub fn analysis_identity_sha256(&self) -> &str {
        &self.analysis_identity_sha256
    }

    pub fn request_path(&self) -> &RelativeArtifactPath {
        &self.request_path
    }

    pub fn result_path(&self) -> &RelativeArtifactPath {
        &self.result_path
    }

    pub fn report_path(&self) -> &RelativeArtifactPath {
        &self.report_path
    }

    pub fn request_json(&self) -> &str {
        &self.request_json
    }

    pub fn result_json(&self) -> &str {
        &self.result_json
    }

    pub fn report_json(&self) -> &str {
        &self.report_json
    }

    pub fn files(&self) -> &[BoardAnalysisFile] {
        &self.files
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct BoardAnalysisHostEvidence {
    pub host_version: String,
    pub host_executable: Vec<u8>,
    pub normalizer: Vec<u8>,
    pub host_runner: Vec<u8>,
    pub erc_report_json: Vec<u8>,
    pub drc_report_json: Vec<u8>,
    pub receipt_json: Vec<u8>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum BoardAnalysisNoncompletionKind {
    Failed,
    Unsupported,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct BoardAnalysisNoncompletion {
    pub kind: BoardAnalysisNoncompletionKind,
    pub code: String,
    pub message: String,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct ArtifactBinding {
    pub path: String,
    pub byte_length: u64,
    pub sha256: String,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct AssertionDescriptor {
    pub assertion_path: String,
    pub capability: String,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct ExpectedSheet {
    pub path: String,
    pub uuid_path: String,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct AnalysisPolicy {
    pub included_severities: Vec<String>,
    pub erc_ignored_checks: Vec<String>,
    pub drc_ignored_checks: Vec<String>,
    pub drc_library_warning: String,
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
            primary_rows: 10_000,
            diagnostics: 256,
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct OutputDescriptor {
    pub role: String,
    pub path: String,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct RequestPreimage {
    pub design_name: String,
    pub analysis_path: String,
    pub adapter: String,
    pub expected_major: u32,
    pub expected_version: String,
    pub assertions: Vec<AssertionDescriptor>,
    pub kicad_schematic: ArtifactBinding,
    pub kicad_pcb: ArtifactBinding,
    pub kicad_identity_map: ArtifactBinding,
    pub expected_sheets: Vec<ExpectedSheet>,
    pub project_support: Vec<ArtifactBinding>,
    pub fabrication_request: ArtifactBinding,
    pub fabrication_manifest: ArtifactBinding,
    pub policy: AnalysisPolicy,
    pub resources: ResourcePolicy,
    pub outputs: Vec<OutputDescriptor>,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct BoardAnalysisRequest {
    pub schema_name: String,
    pub schema_version: u32,
    pub design_name: String,
    pub analysis_path: String,
    pub adapter: String,
    pub expected_major: u32,
    pub expected_version: String,
    pub analysis_identity_sha256: String,
    pub assertions: Vec<AssertionDescriptor>,
    pub kicad_schematic: ArtifactBinding,
    pub kicad_pcb: ArtifactBinding,
    pub kicad_identity_map: ArtifactBinding,
    pub expected_sheets: Vec<ExpectedSheet>,
    pub project_support: Vec<ArtifactBinding>,
    pub fabrication_request: ArtifactBinding,
    pub fabrication_manifest: ArtifactBinding,
    pub policy: AnalysisPolicy,
    pub resources: ResourcePolicy,
    pub outputs: Vec<OutputDescriptor>,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct ToolIdentity {
    pub adapter: String,
    pub version: String,
    pub executable_sha256: String,
    pub normalizer_sha256: String,
    pub host_runner_sha256: String,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct CompletedEvidence {
    pub erc: ArtifactBinding,
    pub drc: ArtifactBinding,
    pub fabrication_manifest: ArtifactBinding,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct ExecutionDiagnostic {
    pub code: String,
    pub message: String,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct BoardAnalysisResult {
    pub schema_name: String,
    pub schema_version: u32,
    pub analysis_identity_sha256: String,
    pub request: ArtifactBinding,
    pub status: String,
    pub tool: Option<ToolIdentity>,
    pub evidence: Option<CompletedEvidence>,
    pub diagnostic: Option<ExecutionDiagnostic>,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct AssertionOutcome {
    pub assertion_path: String,
    pub capability: String,
    pub outcome: String,
    pub evidence_role: Option<String>,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct BoardAnalysisReport {
    pub schema_name: String,
    pub schema_version: u32,
    pub analysis_identity_sha256: String,
    pub request: ArtifactBinding,
    pub result: ArtifactBinding,
    pub execution_status: String,
    pub all_pass: bool,
    pub outcomes: Vec<AssertionOutcome>,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct Receipt {
    pub schema_name: String,
    pub schema_version: u32,
    pub request_sha256: String,
    pub schematic_sha256: String,
    pub pcb_sha256: String,
    pub identity_map_sha256: String,
    pub executable_sha256: String,
    pub normalizer_sha256: String,
    pub host_runner_sha256: String,
    pub erc_sha256: String,
    pub drc_sha256: String,
}
