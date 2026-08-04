use std::collections::BTreeSet;
use std::fmt;

use serde::de::DeserializeOwned;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use super::{APGAR_CONTRACT_IDENTITY, PINNED_APGAR_SOURCE_REVISION};

pub(crate) const REQUEST_SCHEMA_NAME: &str = "circuitc.apgar_route_request";
pub(crate) const RESULT_SCHEMA_NAME: &str = "circuitc.apgar_route_result";
pub(crate) const CONTRACT_SCHEMA_VERSION: u32 = 1;
pub(crate) const APGAR_DBU_PER_MILLIMETER: i64 = 2_000_000;
pub(crate) const MAX_CONTRACT_BYTES: usize = 64 * 1024 * 1024;

const MAX_CONTRACT_ENTRIES: usize = 10_000;
const MAX_ABS_DBU_COORDINATE: i64 = 1_000_000_000_000;
const MAX_EXPANDED_RESOURCE_EDGES: u64 = 1_000_000;

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct ContractDiagnostic {
    pub(crate) code: String,
    pub(crate) path: String,
    pub(crate) message: String,
}

impl fmt::Display for ContractDiagnostic {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(formatter, "{} [{}]: {}", self.code, self.path, self.message)
    }
}

impl std::error::Error for ContractDiagnostic {}

#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct EntityRef {
    pub(crate) id: u64,
    pub(crate) generation: u32,
}

#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct PointDbu {
    pub(crate) x: i64,
    pub(crate) y: i64,
}

#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct BoxDbu {
    pub(crate) min: PointDbu,
    pub(crate) max: PointDbu,
}

#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum LayerSide {
    Front,
    Back,
}

#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum Heading {
    Horizontal,
    Vertical,
    Diagonal45,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct LayerContract {
    pub(crate) reference: EntityRef,
    pub(crate) routing_id: u32,
    pub(crate) name: String,
    pub(crate) physical_order: i32,
    pub(crate) side: LayerSide,
    pub(crate) routable: bool,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct NetContract {
    pub(crate) reference: EntityRef,
    pub(crate) name: String,
    pub(crate) terminals: Vec<EntityRef>,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct TerminalContract {
    pub(crate) reference: EntityRef,
    pub(crate) net: EntityRef,
    pub(crate) component_path: String,
    pub(crate) pad: String,
    pub(crate) center: PointDbu,
    pub(crate) connection_region: BoxDbu,
    pub(crate) layers: Vec<u32>,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct ObstacleContract {
    pub(crate) reference: EntityRef,
    pub(crate) layer: u32,
    pub(crate) bounds: BoxDbu,
    pub(crate) owner_net: Option<EntityRef>,
    pub(crate) provenance: String,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct RoutingProfileContract {
    pub(crate) net: EntityRef,
    pub(crate) nominal_width_dbu: i64,
    pub(crate) clearance_dbu: i64,
    pub(crate) allowed_layers: Vec<u32>,
    pub(crate) allowed_headings: Vec<Heading>,
}

#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct ActiveRegionContract {
    pub(crate) layer: u32,
    pub(crate) bounds: BoxDbu,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct DeterministicCostsContract {
    pub(crate) orthogonal_step: u32,
    pub(crate) diagonal_step: u32,
    pub(crate) bend: u32,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct CompilerProfileContract {
    pub(crate) schema_version: u32,
    pub(crate) lattice_origin: PointDbu,
    pub(crate) lattice_step_dbu: i64,
    pub(crate) tile_width_nodes: u32,
    pub(crate) tile_height_nodes: u32,
    pub(crate) compilation_roi: BoxDbu,
    pub(crate) active_regions: Vec<ActiveRegionContract>,
    pub(crate) allowed_headings: Vec<Heading>,
    pub(crate) costs: DeterministicCostsContract,
}

#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum CandidateObjective {
    BaseScalarCost,
    LengthBiased,
    BendBiased,
    ResourceDiverse,
}

#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum EdgeDirection {
    East,
    NorthEast,
    North,
    NorthWest,
    West,
    SouthWest,
    South,
    SouthEast,
}

#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct EdgeResourceContract {
    pub(crate) layer: u32,
    pub(crate) lattice_x: i64,
    pub(crate) lattice_y: i64,
    pub(crate) direction: EdgeDirection,
}

#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct ResourcePenaltyContract {
    pub(crate) resource: EdgeResourceContract,
    pub(crate) additional_cost: u64,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct CandidatePolicyContract {
    pub(crate) schema_version: u32,
    pub(crate) objective: CandidateObjective,
    pub(crate) deterministic_seed: u64,
    pub(crate) candidate_ordinal: u32,
    pub(crate) orthogonal_step_surcharge: u64,
    pub(crate) diagonal_step_surcharge: u64,
    pub(crate) bend_surcharge: u64,
    pub(crate) banned_resources: Vec<EdgeResourceContract>,
    pub(crate) resource_penalties: Vec<ResourcePenaltyContract>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct SchedulingIdentityContract {
    pub(crate) batch_identity: u64,
    pub(crate) query_identity: u64,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct PlanarRouteContract {
    pub(crate) net: EntityRef,
    pub(crate) start: PointDbu,
    pub(crate) goal: PointDbu,
    pub(crate) start_layer: u32,
    pub(crate) goal_layer: u32,
    pub(crate) candidate_policy: CandidatePolicyContract,
    pub(crate) scheduling: SchedulingIdentityContract,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct ResourceLimitsContract {
    pub(crate) timeout_milliseconds: u64,
    pub(crate) stdout_bytes: u64,
    pub(crate) stderr_bytes: u64,
    pub(crate) diagnostic_bytes: u64,
    pub(crate) candidate_primitives: u64,
    pub(crate) expanded_resource_edges: u64,
}

#[derive(Clone, Debug, Eq, Ord, PartialEq, PartialOrd, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct UnsupportedHostRuleContract {
    pub(crate) code: String,
    pub(crate) path: String,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct RouteRequestContract {
    pub(crate) schema_name: String,
    pub(crate) schema_version: u32,
    pub(crate) design_name: String,
    pub(crate) design_fingerprint_sha256: String,
    pub(crate) request_path: String,
    pub(crate) request_identity_sha256: String,
    pub(crate) expected_apgar_source_revision: String,
    pub(crate) expected_apgar_contract_identity: String,
    pub(crate) dbu_per_millimeter: i64,
    pub(crate) board_revision: u64,
    pub(crate) adapter_name: String,
    pub(crate) adapter_version: String,
    pub(crate) layers: Vec<LayerContract>,
    pub(crate) nets: Vec<NetContract>,
    pub(crate) terminals: Vec<TerminalContract>,
    pub(crate) obstacles: Vec<ObstacleContract>,
    pub(crate) routing_profile: RoutingProfileContract,
    pub(crate) compiler_profile: CompilerProfileContract,
    pub(crate) planar_route: PlanarRouteContract,
    pub(crate) resource_limits: ResourceLimitsContract,
    pub(crate) unsupported_host_rules: Vec<UnsupportedHostRuleContract>,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct ToolIdentity {
    pub(crate) name: String,
    pub(crate) version: String,
    pub(crate) contract_identity: String,
    pub(crate) source_revision: String,
    pub(crate) executable_sha256: String,
    pub(crate) device_class: String,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct ReplayIdentity {
    pub(crate) design_fingerprint_sha256: String,
    pub(crate) request_identity_sha256: String,
    pub(crate) board_revision: u64,
    pub(crate) deterministic_seed: u64,
    pub(crate) batch_identity: u64,
    pub(crate) query_identity: u64,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum CandidateGeneratorKind {
    CpuAStar,
    CudaFrontier,
    CudaSweep,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum CandidateBackendKind {
    Cpu,
    Cuda,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct CandidateAssociations {
    pub(crate) board_content_hash: u64,
    pub(crate) compiler_profile_fingerprint: u64,
    pub(crate) geometry_compiler_version: u32,
    pub(crate) routing_profile_fingerprint: u64,
    pub(crate) rule_bucket_identity: u64,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct CandidateProvenance {
    pub(crate) generator: CandidateGeneratorKind,
    pub(crate) generator_version: u32,
    pub(crate) backend: CandidateBackendKind,
    pub(crate) supported_device_class: String,
    pub(crate) deterministic_seed: u64,
    pub(crate) batch_identity: u64,
    pub(crate) query_identity: u64,
    pub(crate) candidate_ordinal: u32,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct CandidateMetrics {
    pub(crate) scalar_policy_cost: u64,
    pub(crate) intrinsic_base_cost: u64,
    pub(crate) orthogonal_step_count: u64,
    pub(crate) diagonal_step_count: u64,
    pub(crate) bend_count: u64,
    pub(crate) line_primitive_count: u64,
    pub(crate) via_count: u64,
    pub(crate) axis_aligned_length_dbu: u64,
    pub(crate) diagonal_projection_dbu: u64,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum ExactValidationStatus {
    Passed,
    UnsupportedGeometry,
    InvalidGeometry,
    ExactRuleViolation,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct ConstraintAssessment {
    pub(crate) supported_hard_constraints_satisfied: bool,
    pub(crate) unsupported_rules_remain: bool,
    pub(crate) connected_intended_terminal_count: u32,
    pub(crate) exact_validation_status: ExactValidationStatus,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct LinePrimitive {
    pub(crate) layer: u32,
    pub(crate) start: PointDbu,
    pub(crate) end: PointDbu,
    pub(crate) width_dbu: i64,
}

#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct PhysicalEdgeSpan {
    pub(crate) layer: u32,
    pub(crate) lattice_x: i64,
    pub(crate) lattice_y: i64,
    pub(crate) direction: EdgeDirection,
    pub(crate) edge_count: u32,
    pub(crate) usage_units: u32,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct AdmittedCandidate {
    pub(crate) schema_major: u16,
    pub(crate) schema_minor: u16,
    pub(crate) id: String,
    pub(crate) net: EntityRef,
    pub(crate) intended_terminals: [EntityRef; 2],
    pub(crate) associations: CandidateAssociations,
    pub(crate) geometry_schema_version: u32,
    pub(crate) resource_schema_version: u32,
    pub(crate) policy: CandidatePolicyContract,
    pub(crate) policy_identity: u64,
    pub(crate) provenance: CandidateProvenance,
    pub(crate) geometry: Vec<LinePrimitive>,
    pub(crate) resources: Vec<PhysicalEdgeSpan>,
    pub(crate) metrics: CandidateMetrics,
    pub(crate) constraints: ConstraintAssessment,
    pub(crate) geometry_signature: String,
    pub(crate) resource_signature: String,
    pub(crate) payload_checksum: String,
    pub(crate) logical_bytes: u64,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum RouteFailureStatus {
    InvalidRequest,
    Unsupported,
    BoardValidationFailed,
    CompilationFailed,
    PolicyRejected,
    RouteNotFound,
    CandidateRejected,
    ResourceLimitExceeded,
    ToolFailed,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub(crate) enum RouteOutcome {
    Completed {
        selected_candidate_id: String,
        candidates: Vec<AdmittedCandidate>,
    },
    Failure {
        status: RouteFailureStatus,
        diagnostic: ContractDiagnostic,
    },
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct RouteResultContract {
    pub(crate) schema_name: String,
    pub(crate) schema_version: u32,
    pub(crate) request_sha256: String,
    pub(crate) request_path: String,
    pub(crate) tool: ToolIdentity,
    pub(crate) replay: ReplayIdentity,
    pub(crate) outcome: RouteOutcome,
}

impl RouteRequestContract {
    pub(crate) fn validate(&self) -> Result<(), Vec<ContractDiagnostic>> {
        let mut diagnostics = Vec::new();
        validate_header(
            &self.schema_name,
            self.schema_version,
            REQUEST_SCHEMA_NAME,
            &mut diagnostics,
        );
        validate_design_name(&self.design_name, "design_name", &mut diagnostics);
        validate_sha256(
            &self.design_fingerprint_sha256,
            "design_fingerprint_sha256",
            &mut diagnostics,
        );
        validate_semantic_path(&self.request_path, "request_path", &mut diagnostics);
        validate_sha256(
            &self.request_identity_sha256,
            "request_identity_sha256",
            &mut diagnostics,
        );
        validate_lower_hex(
            &self.expected_apgar_source_revision,
            40,
            "expected_apgar_source_revision",
            &mut diagnostics,
        );
        validate_ascii_identity(
            &self.expected_apgar_contract_identity,
            "expected_apgar_contract_identity",
            &mut diagnostics,
        );
        if self.expected_apgar_source_revision != PINNED_APGAR_SOURCE_REVISION {
            invalid(
                &mut diagnostics,
                "expected_apgar_source_revision",
                format!("request must pin APGAR source revision {PINNED_APGAR_SOURCE_REVISION}"),
            );
        }
        if self.expected_apgar_contract_identity != APGAR_CONTRACT_IDENTITY {
            invalid(
                &mut diagnostics,
                "expected_apgar_contract_identity",
                format!("request must pin APGAR contract identity {APGAR_CONTRACT_IDENTITY}"),
            );
        }
        if self.dbu_per_millimeter != APGAR_DBU_PER_MILLIMETER {
            invalid(
                &mut diagnostics,
                "dbu_per_millimeter",
                format!(
                    "APGAR contract v1 requires exactly {APGAR_DBU_PER_MILLIMETER} database units per millimetre"
                ),
            );
        }
        validate_ascii_identity(&self.adapter_name, "adapter_name", &mut diagnostics);
        validate_ascii_identity(&self.adapter_version, "adapter_version", &mut diagnostics);

        validate_count(self.layers.len(), "layers", &mut diagnostics);
        validate_count(self.nets.len(), "nets", &mut diagnostics);
        validate_count(self.terminals.len(), "terminals", &mut diagnostics);
        validate_count(self.obstacles.len(), "obstacles", &mut diagnostics);
        validate_count(
            self.unsupported_host_rules.len(),
            "unsupported_host_rules",
            &mut diagnostics,
        );
        if self.layers.is_empty() {
            invalid(
                &mut diagnostics,
                "layers",
                "request requires at least one layer",
            );
        }
        if self.nets.is_empty() {
            invalid(
                &mut diagnostics,
                "nets",
                "request requires at least one net",
            );
        }
        validate_sorted_unique(
            &self
                .layers
                .iter()
                .map(|layer| layer.routing_id)
                .collect::<Vec<_>>(),
            "layers",
            &mut diagnostics,
        );
        validate_sorted_unique(
            &self
                .nets
                .iter()
                .map(|net| net.reference)
                .collect::<Vec<_>>(),
            "nets",
            &mut diagnostics,
        );
        validate_sorted_unique(
            &self
                .terminals
                .iter()
                .map(|terminal| terminal.reference)
                .collect::<Vec<_>>(),
            "terminals",
            &mut diagnostics,
        );
        validate_sorted_unique(
            &self
                .obstacles
                .iter()
                .map(|obstacle| obstacle.reference)
                .collect::<Vec<_>>(),
            "obstacles",
            &mut diagnostics,
        );
        validate_sorted_unique(
            &self.unsupported_host_rules,
            "unsupported_host_rules",
            &mut diagnostics,
        );

        let mut definition_refs = BTreeSet::new();
        for (index, layer) in self.layers.iter().enumerate() {
            validate_definition_ref(
                layer.reference,
                &format!("layers[{index}].reference"),
                &mut definition_refs,
                &mut diagnostics,
            );
            validate_ascii_identity(
                &layer.name,
                &format!("layers[{index}].name"),
                &mut diagnostics,
            );
            if !layer.routable {
                invalid(
                    &mut diagnostics,
                    format!("layers[{index}].routable"),
                    "every layer carried by route request v1 must be routable",
                );
            }
        }
        for (index, net) in self.nets.iter().enumerate() {
            validate_definition_ref(
                net.reference,
                &format!("nets[{index}].reference"),
                &mut definition_refs,
                &mut diagnostics,
            );
            validate_ascii_identity(&net.name, &format!("nets[{index}].name"), &mut diagnostics);
            validate_count(
                net.terminals.len(),
                &format!("nets[{index}].terminals"),
                &mut diagnostics,
            );
            validate_sorted_unique(
                &net.terminals,
                &format!("nets[{index}].terminals"),
                &mut diagnostics,
            );
        }
        for (index, terminal) in self.terminals.iter().enumerate() {
            validate_definition_ref(
                terminal.reference,
                &format!("terminals[{index}].reference"),
                &mut definition_refs,
                &mut diagnostics,
            );
            validate_semantic_path(
                &terminal.component_path,
                &format!("terminals[{index}].component_path"),
                &mut diagnostics,
            );
            validate_ascii_identity(
                &terminal.pad,
                &format!("terminals[{index}].pad"),
                &mut diagnostics,
            );
            validate_point(
                terminal.center,
                &format!("terminals[{index}].center"),
                &mut diagnostics,
            );
            validate_box(
                terminal.connection_region,
                &format!("terminals[{index}].connection_region"),
                &mut diagnostics,
            );
            if !box_contains_point(terminal.connection_region, terminal.center) {
                invalid(
                    &mut diagnostics,
                    format!("terminals[{index}].center"),
                    "terminal center must lie inside its exact connection region",
                );
            }
            validate_sorted_unique(
                &terminal.layers,
                &format!("terminals[{index}].layers"),
                &mut diagnostics,
            );
            if terminal.layers.is_empty() {
                invalid(
                    &mut diagnostics,
                    format!("terminals[{index}].layers"),
                    "terminal must be present on at least one layer",
                );
            }
        }
        for (index, obstacle) in self.obstacles.iter().enumerate() {
            validate_definition_ref(
                obstacle.reference,
                &format!("obstacles[{index}].reference"),
                &mut definition_refs,
                &mut diagnostics,
            );
            validate_box(
                obstacle.bounds,
                &format!("obstacles[{index}].bounds"),
                &mut diagnostics,
            );
            validate_text(
                &obstacle.provenance,
                &format!("obstacles[{index}].provenance"),
                &mut diagnostics,
            );
        }
        for (index, rule) in self.unsupported_host_rules.iter().enumerate() {
            validate_diagnostic_code(
                &rule.code,
                &format!("unsupported_host_rules[{index}].code"),
                &mut diagnostics,
            );
            validate_semantic_path(
                &rule.path,
                &format!("unsupported_host_rules[{index}].path"),
                &mut diagnostics,
            );
        }

        validate_routing_profile(&self.routing_profile, "routing_profile", &mut diagnostics);
        validate_compiler_profile(&self.compiler_profile, "compiler_profile", &mut diagnostics);
        validate_planar_route(&self.planar_route, "planar_route", &mut diagnostics);
        validate_resource_limits(&self.resource_limits, &mut diagnostics);
        validate_request_relationships(self, &mut diagnostics);
        finish(diagnostics)
    }
}

impl RouteResultContract {
    pub(crate) fn validate(&self) -> Result<(), Vec<ContractDiagnostic>> {
        let mut diagnostics = Vec::new();
        validate_header(
            &self.schema_name,
            self.schema_version,
            RESULT_SCHEMA_NAME,
            &mut diagnostics,
        );
        validate_sha256(&self.request_sha256, "request_sha256", &mut diagnostics);
        validate_semantic_path(&self.request_path, "request_path", &mut diagnostics);
        validate_tool_identity(&self.tool, &mut diagnostics);
        validate_sha256(
            &self.replay.design_fingerprint_sha256,
            "replay.design_fingerprint_sha256",
            &mut diagnostics,
        );
        validate_sha256(
            &self.replay.request_identity_sha256,
            "replay.request_identity_sha256",
            &mut diagnostics,
        );

        match &self.outcome {
            RouteOutcome::Completed {
                selected_candidate_id,
                candidates,
            } => {
                validate_lower_hex(
                    selected_candidate_id,
                    32,
                    "outcome.selected_candidate_id",
                    &mut diagnostics,
                );
                validate_count(candidates.len(), "outcome.candidates", &mut diagnostics);
                if candidates.is_empty() {
                    invalid(
                        &mut diagnostics,
                        "outcome.candidates",
                        "completed outcome requires at least one admitted candidate",
                    );
                }
                let mut candidate_ids = BTreeSet::new();
                for (index, candidate) in candidates.iter().enumerate() {
                    validate_candidate(
                        candidate,
                        &format!("outcome.candidates[{index}]"),
                        &mut diagnostics,
                    );
                    if !candidate_ids.insert(candidate.id.as_str()) {
                        sorted(
                            &mut diagnostics,
                            format!("outcome.candidates[{index}].id"),
                            "candidate IDs must be unique",
                        );
                    }
                }
                let selected = candidates
                    .iter()
                    .find(|candidate| candidate.id == *selected_candidate_id);
                let Some(candidate) = selected else {
                    binding(
                        &mut diagnostics,
                        "outcome.selected_candidate_id",
                        "selected candidate ID must exactly match one admitted candidate ID",
                    );
                    return finish(diagnostics);
                };
                if candidate.provenance.deterministic_seed != self.replay.deterministic_seed
                    || candidate.provenance.batch_identity != self.replay.batch_identity
                    || candidate.provenance.query_identity != self.replay.query_identity
                {
                    binding(
                        &mut diagnostics,
                        "replay",
                        "replay scheduling and seed must match selected candidate provenance",
                    );
                }
            }
            RouteOutcome::Failure { diagnostic, .. } => {
                validate_contract_diagnostic(diagnostic, "outcome.diagnostic", &mut diagnostics);
            }
        }
        finish(diagnostics)
    }
}

pub(crate) fn render_request(request: &RouteRequestContract) -> Result<String, ContractDiagnostic> {
    render_validated(request)
}

pub(crate) fn parse_request(input: &str) -> Result<RouteRequestContract, ContractDiagnostic> {
    parse_validated(input)
}

pub(crate) fn render_result(result: &RouteResultContract) -> Result<String, ContractDiagnostic> {
    render_validated(result)
}

pub(crate) fn parse_result(input: &str) -> Result<RouteResultContract, ContractDiagnostic> {
    parse_validated(input)
}

pub(crate) fn sha256_hex(bytes: &[u8]) -> String {
    let digest = Sha256::digest(bytes);
    digest.iter().map(|byte| format!("{byte:02x}")).collect()
}

trait ValidatedContract: Serialize + DeserializeOwned {
    fn validate_contract(&self) -> Result<(), Vec<ContractDiagnostic>>;
}

impl ValidatedContract for RouteRequestContract {
    fn validate_contract(&self) -> Result<(), Vec<ContractDiagnostic>> {
        self.validate()
    }
}

impl ValidatedContract for RouteResultContract {
    fn validate_contract(&self) -> Result<(), Vec<ContractDiagnostic>> {
        self.validate()
    }
}

fn render_validated<T: ValidatedContract>(value: &T) -> Result<String, ContractDiagnostic> {
    ensure_valid(value.validate_contract())?;
    serialize_canonical(value)
}

fn parse_validated<T: ValidatedContract>(input: &str) -> Result<T, ContractDiagnostic> {
    if input.len() > MAX_CONTRACT_BYTES {
        return Err(diagnostic(
            "CC-ROUTE-CONTRACT-006",
            "document",
            format!("routing contract exceeds the {MAX_CONTRACT_BYTES}-byte input limit"),
        ));
    }
    let value: T = serde_json::from_str(input).map_err(|error| {
        diagnostic(
            "CC-ROUTE-CONTRACT-001",
            "document",
            format!("invalid strict JSON routing contract: {error}"),
        )
    })?;
    ensure_valid(value.validate_contract())?;
    let canonical = serialize_canonical(&value)?;
    if canonical.as_bytes() != input.as_bytes() {
        return Err(diagnostic(
            "CC-ROUTE-CONTRACT-006",
            "document",
            "routing contract bytes are not canonical compact JSON with one final LF",
        ));
    }
    Ok(value)
}

fn serialize_canonical<T: Serialize>(value: &T) -> Result<String, ContractDiagnostic> {
    let mut json = serde_json::to_string(value).map_err(|error| {
        diagnostic(
            "CC-ROUTE-CONTRACT-001",
            "document",
            format!("could not serialize routing contract: {error}"),
        )
    })?;
    json.push('\n');
    if json.len() > MAX_CONTRACT_BYTES {
        return Err(diagnostic(
            "CC-ROUTE-CONTRACT-006",
            "document",
            format!(
                "canonical routing contract exceeds the {MAX_CONTRACT_BYTES}-byte output limit"
            ),
        ));
    }
    Ok(json)
}

fn validate_header(
    actual_name: &str,
    actual_version: u32,
    expected_name: &str,
    diagnostics: &mut Vec<ContractDiagnostic>,
) {
    if actual_name != expected_name {
        push(
            diagnostics,
            "CC-ROUTE-CONTRACT-001",
            "schema_name",
            format!("expected schema name `{expected_name}`; found `{actual_name}`"),
        );
    }
    if actual_version != CONTRACT_SCHEMA_VERSION {
        push(
            diagnostics,
            "CC-ROUTE-CONTRACT-001",
            "schema_version",
            format!("expected schema version {CONTRACT_SCHEMA_VERSION}; found {actual_version}"),
        );
    }
}

fn validate_routing_profile(
    profile: &RoutingProfileContract,
    path: &str,
    diagnostics: &mut Vec<ContractDiagnostic>,
) {
    validate_positive_dbu(
        profile.nominal_width_dbu,
        &format!("{path}.nominal_width_dbu"),
        diagnostics,
    );
    validate_positive_dbu(
        profile.clearance_dbu,
        &format!("{path}.clearance_dbu"),
        diagnostics,
    );
    validate_sorted_unique(
        &profile.allowed_layers,
        &format!("{path}.allowed_layers"),
        diagnostics,
    );
    validate_headings(
        &profile.allowed_headings,
        &format!("{path}.allowed_headings"),
        diagnostics,
    );
    if profile.allowed_layers.len() != 1 {
        invalid(
            diagnostics,
            format!("{path}.allowed_layers"),
            "route contract v1 requires exactly one allowed layer",
        );
    }
}

fn validate_compiler_profile(
    profile: &CompilerProfileContract,
    path: &str,
    diagnostics: &mut Vec<ContractDiagnostic>,
) {
    if profile.schema_version != 1 {
        invalid(
            diagnostics,
            format!("{path}.schema_version"),
            "compiler profile schema version must be 1",
        );
    }
    validate_point(
        profile.lattice_origin,
        &format!("{path}.lattice_origin"),
        diagnostics,
    );
    validate_positive_dbu(
        profile.lattice_step_dbu,
        &format!("{path}.lattice_step_dbu"),
        diagnostics,
    );
    if profile.tile_width_nodes == 0 || profile.tile_height_nodes == 0 {
        invalid(
            diagnostics,
            path,
            "compiler tile dimensions must be non-zero",
        );
    }
    validate_box(
        profile.compilation_roi,
        &format!("{path}.compilation_roi"),
        diagnostics,
    );
    validate_count(
        profile.active_regions.len(),
        &format!("{path}.active_regions"),
        diagnostics,
    );
    validate_sorted_unique(
        &profile.active_regions,
        &format!("{path}.active_regions"),
        diagnostics,
    );
    for (index, region) in profile.active_regions.iter().enumerate() {
        validate_box(
            region.bounds,
            &format!("{path}.active_regions[{index}].bounds"),
            diagnostics,
        );
    }
    validate_headings(
        &profile.allowed_headings,
        &format!("{path}.allowed_headings"),
        diagnostics,
    );
    if profile.costs.orthogonal_step == 0 || profile.costs.diagonal_step == 0 {
        invalid(
            diagnostics,
            format!("{path}.costs"),
            "orthogonal and diagonal compiler costs must be non-zero",
        );
    }
}

fn validate_planar_route(
    route: &PlanarRouteContract,
    path: &str,
    diagnostics: &mut Vec<ContractDiagnostic>,
) {
    validate_point(route.start, &format!("{path}.start"), diagnostics);
    validate_point(route.goal, &format!("{path}.goal"), diagnostics);
    if route.start == route.goal {
        invalid(
            diagnostics,
            format!("{path}.goal"),
            "planar route endpoints must be distinct",
        );
    }
    if route.start_layer != route.goal_layer {
        invalid(
            diagnostics,
            path,
            "route contract v1 requires one planar layer and does not permit vias",
        );
    }
    validate_candidate_policy(
        &route.candidate_policy,
        &format!("{path}.candidate_policy"),
        diagnostics,
    );
}

fn validate_candidate_policy(
    policy: &CandidatePolicyContract,
    path: &str,
    diagnostics: &mut Vec<ContractDiagnostic>,
) {
    if policy.schema_version != 1 {
        invalid(
            diagnostics,
            format!("{path}.schema_version"),
            "candidate policy schema version must be 1",
        );
    }
    validate_count(
        policy.banned_resources.len(),
        &format!("{path}.banned_resources"),
        diagnostics,
    );
    validate_count(
        policy.resource_penalties.len(),
        &format!("{path}.resource_penalties"),
        diagnostics,
    );
    validate_sorted_unique(
        &policy.banned_resources,
        &format!("{path}.banned_resources"),
        diagnostics,
    );
    validate_sorted_unique(
        &policy.resource_penalties,
        &format!("{path}.resource_penalties"),
        diagnostics,
    );
    if policy
        .resource_penalties
        .iter()
        .any(|penalty| penalty.additional_cost == 0)
    {
        invalid(
            diagnostics,
            format!("{path}.resource_penalties"),
            "resource penalties must add a non-zero cost",
        );
    }
}

fn validate_resource_limits(
    limits: &ResourceLimitsContract,
    diagnostics: &mut Vec<ContractDiagnostic>,
) {
    let values = [
        ("timeout_milliseconds", limits.timeout_milliseconds),
        ("stdout_bytes", limits.stdout_bytes),
        ("stderr_bytes", limits.stderr_bytes),
        ("diagnostic_bytes", limits.diagnostic_bytes),
        ("candidate_primitives", limits.candidate_primitives),
        ("expanded_resource_edges", limits.expanded_resource_edges),
    ];
    for (field, value) in values {
        if value == 0 {
            invalid(
                diagnostics,
                format!("resource_limits.{field}"),
                "resource limit must be non-zero",
            );
        }
    }
    if limits.stdout_bytes > MAX_CONTRACT_BYTES as u64
        || limits.stderr_bytes > MAX_CONTRACT_BYTES as u64
    {
        invalid(
            diagnostics,
            "resource_limits",
            "stdout and stderr bounds may not exceed the routing contract byte ceiling",
        );
    }
}

fn validate_request_relationships(
    request: &RouteRequestContract,
    diagnostics: &mut Vec<ContractDiagnostic>,
) {
    let layer_ids: BTreeSet<_> = request
        .layers
        .iter()
        .map(|layer| layer.routing_id)
        .collect();
    let net_refs: BTreeSet<_> = request.nets.iter().map(|net| net.reference).collect();
    let terminal_refs: BTreeSet<_> = request
        .terminals
        .iter()
        .map(|terminal| terminal.reference)
        .collect();

    if !net_refs.contains(&request.routing_profile.net) {
        invalid(
            diagnostics,
            "routing_profile.net",
            "routing profile must reference the requested net",
        );
    }
    if !net_refs.contains(&request.planar_route.net) {
        invalid(
            diagnostics,
            "planar_route.net",
            "planar route must reference the requested net",
        );
    }
    if request.planar_route.net != request.routing_profile.net {
        invalid(
            diagnostics,
            "planar_route.net",
            "planar route and routing profile must name the same net",
        );
    }
    let routed_net = request
        .nets
        .iter()
        .find(|net| net.reference == request.routing_profile.net);
    if let Some(net) = routed_net
        && net.terminals.len() != 2
    {
        invalid(
            diagnostics,
            "routing_profile.net",
            "the selected routed net requires exactly two terminal references",
        );
    }
    for (index, net) in request.nets.iter().enumerate() {
        for (terminal_index, terminal) in net.terminals.iter().enumerate() {
            if !terminal_refs.contains(terminal) {
                invalid(
                    diagnostics,
                    format!("nets[{index}].terminals[{terminal_index}]"),
                    "net references an unknown terminal",
                );
            }
        }
        let actual_terminal_refs: Vec<_> = request
            .terminals
            .iter()
            .filter(|terminal| terminal.net == net.reference)
            .map(|terminal| terminal.reference)
            .collect();
        if net.terminals != actual_terminal_refs {
            invalid(
                diagnostics,
                format!("nets[{index}].terminals"),
                "net terminal references must exactly match its sorted terminal records",
            );
        }
    }
    for (index, terminal) in request.terminals.iter().enumerate() {
        if !net_refs.contains(&terminal.net) {
            invalid(
                diagnostics,
                format!("terminals[{index}].net"),
                "terminal references an unknown net",
            );
        }
        for (layer_index, layer) in terminal.layers.iter().enumerate() {
            if !layer_ids.contains(layer) {
                invalid(
                    diagnostics,
                    format!("terminals[{index}].layers[{layer_index}]"),
                    "terminal references an unknown routing layer",
                );
            }
        }
        if !box_contains_box(
            request.compiler_profile.compilation_roi,
            terminal.connection_region,
        ) {
            invalid(
                diagnostics,
                format!("terminals[{index}].connection_region"),
                "terminal connection region must lie inside the compiler ROI",
            );
        }
    }

    for (index, obstacle) in request.obstacles.iter().enumerate() {
        if !layer_ids.contains(&obstacle.layer) {
            invalid(
                diagnostics,
                format!("obstacles[{index}].layer"),
                "obstacle references an unknown routing layer",
            );
        }
        if obstacle
            .owner_net
            .is_some_and(|owner| !net_refs.contains(&owner))
        {
            invalid(
                diagnostics,
                format!("obstacles[{index}].owner_net"),
                "obstacle owner references an unknown net",
            );
        }
        if !box_contains_box(request.compiler_profile.compilation_roi, obstacle.bounds) {
            invalid(
                diagnostics,
                format!("obstacles[{index}].bounds"),
                "obstacle bounds must lie inside the compiler ROI",
            );
        }
    }

    for (index, region) in request.compiler_profile.active_regions.iter().enumerate() {
        if !layer_ids.contains(&region.layer) {
            invalid(
                diagnostics,
                format!("compiler_profile.active_regions[{index}].layer"),
                "active region references an unknown routing layer",
            );
        }
        if !box_contains_box(request.compiler_profile.compilation_roi, region.bounds) {
            invalid(
                diagnostics,
                format!("compiler_profile.active_regions[{index}].bounds"),
                "active region must lie inside the compiler ROI",
            );
        }
    }
    if request.compiler_profile.active_regions.len() != 1 {
        invalid(
            diagnostics,
            "compiler_profile.active_regions",
            "route contract v1 requires exactly one active region",
        );
    }

    if let [selected] = request.routing_profile.allowed_layers.as_slice() {
        if !layer_ids.contains(selected) {
            invalid(
                diagnostics,
                "routing_profile.allowed_layers[0]",
                "routing profile references an unknown requested layer",
            );
        }
        if request.planar_route.start_layer != *selected
            || request.planar_route.goal_layer != *selected
        {
            invalid(
                diagnostics,
                "planar_route",
                "planar endpoints must both use the selected requested layer",
            );
        }
        if request
            .compiler_profile
            .active_regions
            .first()
            .is_some_and(|region| region.layer != *selected)
        {
            invalid(
                diagnostics,
                "compiler_profile.active_regions[0].layer",
                "active region must bind the selected requested layer",
            );
        }
        if let Some(net) = routed_net {
            for (index, terminal) in request.terminals.iter().enumerate() {
                if net.terminals.contains(&terminal.reference)
                    && terminal.layers.as_slice() != [*selected]
                {
                    invalid(
                        diagnostics,
                        format!("terminals[{index}].layers"),
                        "each routed terminal must lie only on the selected requested layer",
                    );
                }
            }
        }
    }

    let roi = request.compiler_profile.compilation_roi;
    for (field, point) in [
        ("planar_route.start", request.planar_route.start),
        ("planar_route.goal", request.planar_route.goal),
    ] {
        if !box_contains_point(roi, point) {
            invalid(
                diagnostics,
                field,
                "planar endpoint must lie inside the compiler ROI",
            );
        }
    }

    if let Some(net) = routed_net {
        let routed_terminals: Vec<_> = request
            .terminals
            .iter()
            .filter(|terminal| net.terminals.contains(&terminal.reference))
            .collect();
        if routed_terminals.len() != 2 {
            invalid(
                diagnostics,
                "routing_profile.net",
                "routed net must resolve to exactly two terminal records",
            );
            return;
        }
        let terminal_centers = [routed_terminals[0].center, routed_terminals[1].center];
        let start_matches = terminal_centers.contains(&request.planar_route.start);
        let goal_matches = terminal_centers.contains(&request.planar_route.goal);
        if !start_matches
            || !goal_matches
            || request.planar_route.start == request.planar_route.goal
        {
            invalid(
                diagnostics,
                "planar_route",
                "planar start and goal must exactly match the two physical terminal centres",
            );
        }
    }
}

fn validate_tool_identity(tool: &ToolIdentity, diagnostics: &mut Vec<ContractDiagnostic>) {
    validate_ascii_identity(&tool.name, "tool.name", diagnostics);
    validate_ascii_identity(&tool.version, "tool.version", diagnostics);
    validate_ascii_identity(
        &tool.contract_identity,
        "tool.contract_identity",
        diagnostics,
    );
    validate_lower_hex(
        &tool.source_revision,
        40,
        "tool.source_revision",
        diagnostics,
    );
    validate_sha256(
        &tool.executable_sha256,
        "tool.executable_sha256",
        diagnostics,
    );
    validate_ascii_identity(&tool.device_class, "tool.device_class", diagnostics);
}

fn validate_candidate(
    candidate: &AdmittedCandidate,
    path: &str,
    diagnostics: &mut Vec<ContractDiagnostic>,
) {
    if candidate.schema_major != 1 || candidate.schema_minor != 0 {
        invalid(
            diagnostics,
            path,
            "admitted candidate schema must be exactly 1.0",
        );
    }
    validate_lower_hex(&candidate.id, 32, &format!("{path}.id"), diagnostics);
    if candidate.intended_terminals[0] >= candidate.intended_terminals[1] {
        sorted(
            diagnostics,
            format!("{path}.intended_terminals"),
            "intended terminals must be strictly sorted and distinct",
        );
    }
    if candidate.geometry_schema_version != 1 || candidate.resource_schema_version != 1 {
        invalid(
            diagnostics,
            path,
            "candidate geometry and resource schema versions must both be 1",
        );
    }
    validate_candidate_policy(&candidate.policy, &format!("{path}.policy"), diagnostics);
    validate_ascii_identity(
        &candidate.provenance.supported_device_class,
        &format!("{path}.provenance.supported_device_class"),
        diagnostics,
    );
    validate_count(
        candidate.geometry.len(),
        &format!("{path}.geometry"),
        diagnostics,
    );
    if candidate.geometry.is_empty() {
        invalid(
            diagnostics,
            format!("{path}.geometry"),
            "completed candidate requires at least one exact line primitive",
        );
    }
    for (index, line) in candidate.geometry.iter().enumerate() {
        let line_path = format!("{path}.geometry[{index}]");
        validate_point(line.start, &format!("{line_path}.start"), diagnostics);
        validate_point(line.end, &format!("{line_path}.end"), diagnostics);
        validate_positive_dbu(
            line.width_dbu,
            &format!("{line_path}.width_dbu"),
            diagnostics,
        );
        let dx = line.end.x.abs_diff(line.start.x);
        let dy = line.end.y.abs_diff(line.start.y);
        if line.start == line.end || !(dx == 0 || dy == 0 || dx == dy) {
            invalid(
                diagnostics,
                line_path,
                "route contract v1 line must be non-zero horizontal, vertical, or 45-degree geometry",
            );
        }
        if index > 0 {
            let previous = candidate.geometry[index - 1];
            if previous.end != line.start || previous.layer != line.layer {
                invalid(
                    diagnostics,
                    format!("{path}.geometry[{index}]"),
                    "candidate line primitives must form one continuous planar chain",
                );
            }
        }
    }
    validate_count(
        candidate.resources.len(),
        &format!("{path}.resources"),
        diagnostics,
    );
    if candidate.resources.is_empty() {
        invalid(
            diagnostics,
            format!("{path}.resources"),
            "completed candidate requires a non-empty canonical physical-edge footprint",
        );
    }
    validate_sorted_unique(
        &candidate
            .resources
            .iter()
            .map(|span| (span.layer, span.lattice_x, span.lattice_y, span.direction))
            .collect::<Vec<_>>(),
        &format!("{path}.resources"),
        diagnostics,
    );
    let mut expanded_edges = 0_u64;
    for (index, span) in candidate.resources.iter().enumerate() {
        let span_path = format!("{path}.resources[{index}]");
        if !matches!(
            span.direction,
            EdgeDirection::East
                | EdgeDirection::NorthEast
                | EdgeDirection::North
                | EdgeDirection::NorthWest
        ) {
            invalid(
                diagnostics,
                format!("{span_path}.direction"),
                "physical edge spans permit only canonical east, north-east, north, or north-west directions",
            );
        }
        if span.lattice_x.unsigned_abs() > MAX_ABS_DBU_COORDINATE as u64
            || span.lattice_y.unsigned_abs() > MAX_ABS_DBU_COORDINATE as u64
        {
            invalid(
                diagnostics,
                span_path.clone(),
                "physical edge lattice coordinates exceed the contract envelope",
            );
        }
        if span.edge_count == 0 {
            invalid(
                diagnostics,
                format!("{span_path}.edge_count"),
                "physical edge span count must be positive",
            );
        }
        if span.usage_units != 1 {
            invalid(
                diagnostics,
                format!("{span_path}.usage_units"),
                "route candidate v1 physical edge usage must be exactly one",
            );
        }
        expanded_edges = expanded_edges.saturating_add(u64::from(span.edge_count));
    }
    if expanded_edges > MAX_EXPANDED_RESOURCE_EDGES {
        invalid(
            diagnostics,
            format!("{path}.resources"),
            format!(
                "expanded physical edge count exceeds the {MAX_EXPANDED_RESOURCE_EDGES}-edge contract bound"
            ),
        );
    }
    if candidate.metrics.line_primitive_count != candidate.geometry.len() as u64 {
        invalid(
            diagnostics,
            format!("{path}.metrics.line_primitive_count"),
            "line primitive metric must equal the exact geometry length",
        );
    }
    if candidate.metrics.via_count != 0 {
        invalid(
            diagnostics,
            format!("{path}.metrics.via_count"),
            "route contract v1 does not admit vias",
        );
    }
    if !candidate.constraints.supported_hard_constraints_satisfied
        || candidate.constraints.unsupported_rules_remain
        || candidate.constraints.connected_intended_terminal_count != 2
        || candidate.constraints.exact_validation_status != ExactValidationStatus::Passed
    {
        invalid(
            diagnostics,
            format!("{path}.constraints"),
            "a completed candidate must retain a passed APGAR exact-admission assessment for exactly two terminals and no unsupported rules",
        );
    }
    validate_lower_hex(
        &candidate.geometry_signature,
        32,
        &format!("{path}.geometry_signature"),
        diagnostics,
    );
    validate_lower_hex(
        &candidate.resource_signature,
        32,
        &format!("{path}.resource_signature"),
        diagnostics,
    );
    validate_lower_hex(
        &candidate.payload_checksum,
        16,
        &format!("{path}.payload_checksum"),
        diagnostics,
    );
    if candidate.logical_bytes == 0 || candidate.logical_bytes > MAX_CONTRACT_BYTES as u64 {
        invalid(
            diagnostics,
            format!("{path}.logical_bytes"),
            "candidate logical byte count must be positive and within the contract byte ceiling",
        );
    }
}

fn validate_headings(headings: &[Heading], path: &str, diagnostics: &mut Vec<ContractDiagnostic>) {
    validate_sorted_unique(headings, path, diagnostics);
    if headings != [Heading::Horizontal, Heading::Vertical, Heading::Diagonal45] {
        invalid(
            diagnostics,
            path,
            "route contract v1 requires exactly horizontal, vertical, and diagonal-45 headings",
        );
    }
}

fn validate_contract_diagnostic(
    value: &ContractDiagnostic,
    path: &str,
    diagnostics: &mut Vec<ContractDiagnostic>,
) {
    validate_diagnostic_code(&value.code, &format!("{path}.code"), diagnostics);
    validate_semantic_path(&value.path, &format!("{path}.path"), diagnostics);
    validate_text(&value.message, &format!("{path}.message"), diagnostics);
}

fn validate_diagnostic_code(value: &str, path: &str, diagnostics: &mut Vec<ContractDiagnostic>) {
    if value.is_empty()
        || !value
            .bytes()
            .all(|byte| byte.is_ascii_uppercase() || byte.is_ascii_digit() || byte == b'-')
    {
        invalid(
            diagnostics,
            path,
            "diagnostic code must be a non-empty uppercase ASCII code",
        );
    }
}

fn validate_design_name(value: &str, path: &str, diagnostics: &mut Vec<ContractDiagnostic>) {
    let mut bytes = value.bytes();
    if !bytes
        .next()
        .is_some_and(|byte| byte.is_ascii_alphabetic() || byte == b'_')
        || !bytes.all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'_' | b'-'))
    {
        invalid(
            diagnostics,
            path,
            "design name must be a safe CircuitC artifact stem",
        );
    }
}

fn validate_semantic_path(value: &str, path: &str, diagnostics: &mut Vec<ContractDiagnostic>) {
    if value.is_empty()
        || value.starts_with('.')
        || value.ends_with('.')
        || value.split('.').any(str::is_empty)
        || value.bytes().any(|byte| {
            !(byte.is_ascii_alphanumeric() || matches!(byte, b'_' | b'+' | b'-' | b'/' | b'.'))
        })
    {
        invalid(
            diagnostics,
            path,
            "semantic path must be a non-empty canonical CircuitC path",
        );
    }
}

fn validate_ascii_identity(value: &str, path: &str, diagnostics: &mut Vec<ContractDiagnostic>) {
    if value.is_empty()
        || !value.bytes().all(|byte| {
            byte.is_ascii_alphanumeric() || matches!(byte, b'_' | b'+' | b'-' | b'.' | b'/' | b':')
        })
    {
        invalid(
            diagnostics,
            path,
            "identity must be a non-empty portable ASCII token",
        );
    }
}

fn validate_text(value: &str, path: &str, diagnostics: &mut Vec<ContractDiagnostic>) {
    if value.is_empty() || value.chars().any(char::is_control) {
        invalid(
            diagnostics,
            path,
            "text must be non-empty and contain no control characters",
        );
    }
}

fn validate_point(value: PointDbu, path: &str, diagnostics: &mut Vec<ContractDiagnostic>) {
    if value.x.unsigned_abs() > MAX_ABS_DBU_COORDINATE as u64
        || value.y.unsigned_abs() > MAX_ABS_DBU_COORDINATE as u64
    {
        invalid(
            diagnostics,
            path,
            format!("point must fit APGAR's +/-{MAX_ABS_DBU_COORDINATE} DBU envelope"),
        );
    }
}

fn validate_box(value: BoxDbu, path: &str, diagnostics: &mut Vec<ContractDiagnostic>) {
    validate_point(value.min, &format!("{path}.min"), diagnostics);
    validate_point(value.max, &format!("{path}.max"), diagnostics);
    if value.min.x > value.max.x || value.min.y > value.max.y {
        invalid(
            diagnostics,
            path,
            "box minimum coordinates must not exceed maximum coordinates",
        );
    }
}

fn box_contains_point(bounds: BoxDbu, point: PointDbu) -> bool {
    bounds.min.x <= point.x
        && point.x <= bounds.max.x
        && bounds.min.y <= point.y
        && point.y <= bounds.max.y
}

fn box_contains_box(outer: BoxDbu, inner: BoxDbu) -> bool {
    box_contains_point(outer, inner.min) && box_contains_point(outer, inner.max)
}

fn validate_positive_dbu(value: i64, path: &str, diagnostics: &mut Vec<ContractDiagnostic>) {
    if value <= 0 || value > MAX_ABS_DBU_COORDINATE {
        invalid(
            diagnostics,
            path,
            format!("exact DBU size must be in 1..={MAX_ABS_DBU_COORDINATE}"),
        );
    }
}

fn validate_sha256(value: &str, path: &str, diagnostics: &mut Vec<ContractDiagnostic>) {
    validate_lower_hex(value, 64, path, diagnostics);
}

fn validate_lower_hex(
    value: &str,
    length: usize,
    path: &str,
    diagnostics: &mut Vec<ContractDiagnostic>,
) {
    if value.len() != length
        || !value
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
    {
        push(
            diagnostics,
            "CC-ROUTE-CONTRACT-004",
            path,
            format!("value must be exactly {length} lowercase hexadecimal characters"),
        );
    }
}

fn validate_count(count: usize, path: &str, diagnostics: &mut Vec<ContractDiagnostic>) {
    if count > MAX_CONTRACT_ENTRIES {
        push(
            diagnostics,
            "CC-ROUTE-CONTRACT-006",
            path,
            format!("collection contains {count} entries; maximum is {MAX_CONTRACT_ENTRIES}"),
        );
    }
}

fn validate_sorted_unique<T: Ord>(
    values: &[T],
    path: &str,
    diagnostics: &mut Vec<ContractDiagnostic>,
) {
    if values.windows(2).any(|pair| pair[0] >= pair[1]) {
        sorted(
            diagnostics,
            path,
            "entries must be strictly sorted and unique by their canonical key",
        );
    }
}

fn validate_definition_ref(
    reference: EntityRef,
    path: &str,
    definitions: &mut BTreeSet<EntityRef>,
    diagnostics: &mut Vec<ContractDiagnostic>,
) {
    if reference.id == 0 {
        invalid(diagnostics, path, "entity ID zero is reserved");
    }
    if !definitions.insert(reference) {
        sorted(
            diagnostics,
            path,
            "defined entity references must be globally unique",
        );
    }
}

fn finish(diagnostics: Vec<ContractDiagnostic>) -> Result<(), Vec<ContractDiagnostic>> {
    if diagnostics.is_empty() {
        Ok(())
    } else {
        Err(diagnostics)
    }
}

fn ensure_valid(validation: Result<(), Vec<ContractDiagnostic>>) -> Result<(), ContractDiagnostic> {
    validation.map_err(|diagnostics| {
        diagnostics
            .into_iter()
            .next()
            .expect("validation failure must contain a diagnostic")
    })
}

fn diagnostic(
    code: &'static str,
    path: impl Into<String>,
    message: impl Into<String>,
) -> ContractDiagnostic {
    ContractDiagnostic {
        code: code.to_owned(),
        path: path.into(),
        message: message.into(),
    }
}

fn push(
    diagnostics: &mut Vec<ContractDiagnostic>,
    code: &'static str,
    path: impl Into<String>,
    message: impl Into<String>,
) {
    diagnostics.push(diagnostic(code, path, message));
}

fn invalid(
    diagnostics: &mut Vec<ContractDiagnostic>,
    path: impl Into<String>,
    message: impl Into<String>,
) {
    push(diagnostics, "CC-ROUTE-CONTRACT-002", path, message);
}

fn sorted(
    diagnostics: &mut Vec<ContractDiagnostic>,
    path: impl Into<String>,
    message: impl Into<String>,
) {
    push(diagnostics, "CC-ROUTE-CONTRACT-003", path, message);
}

fn binding(
    diagnostics: &mut Vec<ContractDiagnostic>,
    path: impl Into<String>,
    message: impl Into<String>,
) {
    push(diagnostics, "CC-ROUTE-CONTRACT-004", path, message);
}

#[cfg(test)]
mod tests {
    use super::*;

    const SHA: &str = "0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef";
    const REV: &str = PINNED_APGAR_SOURCE_REVISION;
    const HASH128: &str = "0123456789abcdef0123456789abcdef";

    fn entity(id: u64) -> EntityRef {
        EntityRef { id, generation: 1 }
    }

    fn point(x: i64, y: i64) -> PointDbu {
        PointDbu { x, y }
    }

    fn policy() -> CandidatePolicyContract {
        CandidatePolicyContract {
            schema_version: 1,
            objective: CandidateObjective::BaseScalarCost,
            deterministic_seed: 7,
            candidate_ordinal: 0,
            orthogonal_step_surcharge: 0,
            diagonal_step_surcharge: 0,
            bend_surcharge: 0,
            banned_resources: Vec::new(),
            resource_penalties: Vec::new(),
        }
    }

    fn request() -> RouteRequestContract {
        let bounds = BoxDbu {
            min: point(0, 0),
            max: point(20_000_000, 20_000_000),
        };
        RouteRequestContract {
            schema_name: REQUEST_SCHEMA_NAME.to_owned(),
            schema_version: CONTRACT_SCHEMA_VERSION,
            design_name: "divider".to_owned(),
            design_fingerprint_sha256: SHA.to_owned(),
            request_path: "divider.board.autoroute.main".to_owned(),
            request_identity_sha256: SHA.to_owned(),
            expected_apgar_source_revision: REV.to_owned(),
            expected_apgar_contract_identity: APGAR_CONTRACT_IDENTITY.to_owned(),
            dbu_per_millimeter: APGAR_DBU_PER_MILLIMETER,
            board_revision: 1,
            adapter_name: "circuitc".to_owned(),
            adapter_version: "1".to_owned(),
            layers: vec![LayerContract {
                reference: entity(1),
                routing_id: 0,
                name: "front".to_owned(),
                physical_order: 0,
                side: LayerSide::Front,
                routable: true,
            }],
            nets: vec![NetContract {
                reference: entity(2),
                name: "SIGNAL".to_owned(),
                terminals: vec![entity(3), entity(4)],
            }],
            terminals: vec![
                TerminalContract {
                    reference: entity(3),
                    net: entity(2),
                    component_path: "divider.r1".to_owned(),
                    pad: "1".to_owned(),
                    center: point(2_000_000, 2_000_000),
                    connection_region: BoxDbu {
                        min: point(1_500_000, 1_500_000),
                        max: point(2_500_000, 2_500_000),
                    },
                    layers: vec![0],
                },
                TerminalContract {
                    reference: entity(4),
                    net: entity(2),
                    component_path: "divider.r2".to_owned(),
                    pad: "1".to_owned(),
                    center: point(18_000_000, 18_000_000),
                    connection_region: BoxDbu {
                        min: point(17_500_000, 17_500_000),
                        max: point(18_500_000, 18_500_000),
                    },
                    layers: vec![0],
                },
            ],
            obstacles: vec![ObstacleContract {
                reference: entity(5),
                layer: 0,
                bounds: BoxDbu {
                    min: point(8_000_000, 8_000_000),
                    max: point(10_000_000, 10_000_000),
                },
                owner_net: None,
                provenance: "board.keepout.main".to_owned(),
            }],
            routing_profile: RoutingProfileContract {
                net: entity(2),
                nominal_width_dbu: 500_000,
                clearance_dbu: 400_000,
                allowed_layers: vec![0],
                allowed_headings: vec![Heading::Horizontal, Heading::Vertical, Heading::Diagonal45],
            },
            compiler_profile: CompilerProfileContract {
                schema_version: 1,
                lattice_origin: point(0, 0),
                lattice_step_dbu: 2_000_000,
                tile_width_nodes: 16,
                tile_height_nodes: 16,
                compilation_roi: bounds,
                active_regions: vec![ActiveRegionContract { layer: 0, bounds }],
                allowed_headings: vec![Heading::Horizontal, Heading::Vertical, Heading::Diagonal45],
                costs: DeterministicCostsContract {
                    orthogonal_step: 10,
                    diagonal_step: 14,
                    bend: 2,
                },
            },
            planar_route: PlanarRouteContract {
                net: entity(2),
                start: point(2_000_000, 2_000_000),
                goal: point(18_000_000, 18_000_000),
                start_layer: 0,
                goal_layer: 0,
                candidate_policy: policy(),
                scheduling: SchedulingIdentityContract {
                    batch_identity: 11,
                    query_identity: 13,
                },
            },
            resource_limits: ResourceLimitsContract {
                timeout_milliseconds: 30_000,
                stdout_bytes: MAX_CONTRACT_BYTES as u64,
                stderr_bytes: 1_048_576,
                diagnostic_bytes: 1_024,
                candidate_primitives: 10_000,
                expanded_resource_edges: 1_000_000,
            },
            unsupported_host_rules: vec![UnsupportedHostRuleContract {
                code: "CC-ROUTE-HOST-001".to_owned(),
                path: "divider.board.clearance".to_owned(),
            }],
        }
    }

    fn result() -> RouteResultContract {
        RouteResultContract {
            schema_name: RESULT_SCHEMA_NAME.to_owned(),
            schema_version: CONTRACT_SCHEMA_VERSION,
            request_sha256: SHA.to_owned(),
            request_path: "divider.board.autoroute.main".to_owned(),
            tool: ToolIdentity {
                name: "apgar".to_owned(),
                version: "0.1.0".to_owned(),
                contract_identity: APGAR_CONTRACT_IDENTITY.to_owned(),
                source_revision: REV.to_owned(),
                executable_sha256: SHA.to_owned(),
                device_class: "cpu-reference-v1".to_owned(),
            },
            replay: ReplayIdentity {
                design_fingerprint_sha256: SHA.to_owned(),
                request_identity_sha256: SHA.to_owned(),
                board_revision: 1,
                deterministic_seed: 7,
                batch_identity: 11,
                query_identity: 13,
            },
            outcome: RouteOutcome::Completed {
                selected_candidate_id: HASH128.to_owned(),
                candidates: vec![AdmittedCandidate {
                    schema_major: 1,
                    schema_minor: 0,
                    id: HASH128.to_owned(),
                    net: entity(2),
                    intended_terminals: [entity(3), entity(4)],
                    associations: CandidateAssociations {
                        board_content_hash: 17,
                        compiler_profile_fingerprint: 19,
                        geometry_compiler_version: 1,
                        routing_profile_fingerprint: 23,
                        rule_bucket_identity: 29,
                    },
                    geometry_schema_version: 1,
                    resource_schema_version: 1,
                    policy: policy(),
                    policy_identity: 31,
                    provenance: CandidateProvenance {
                        generator: CandidateGeneratorKind::CpuAStar,
                        generator_version: 1,
                        backend: CandidateBackendKind::Cpu,
                        supported_device_class: "cpu-reference-v1".to_owned(),
                        deterministic_seed: 7,
                        batch_identity: 11,
                        query_identity: 13,
                        candidate_ordinal: 0,
                    },
                    geometry: vec![LinePrimitive {
                        layer: 0,
                        start: point(2_000_000, 2_000_000),
                        end: point(18_000_000, 18_000_000),
                        width_dbu: 500_000,
                    }],
                    resources: vec![PhysicalEdgeSpan {
                        layer: 0,
                        lattice_x: 1,
                        lattice_y: 1,
                        direction: EdgeDirection::NorthEast,
                        edge_count: 8,
                        usage_units: 1,
                    }],
                    metrics: CandidateMetrics {
                        scalar_policy_cost: 112,
                        intrinsic_base_cost: 112,
                        orthogonal_step_count: 0,
                        diagonal_step_count: 8,
                        bend_count: 0,
                        line_primitive_count: 1,
                        via_count: 0,
                        axis_aligned_length_dbu: 0,
                        diagonal_projection_dbu: 16_000_000,
                    },
                    constraints: ConstraintAssessment {
                        supported_hard_constraints_satisfied: true,
                        unsupported_rules_remain: false,
                        connected_intended_terminal_count: 2,
                        exact_validation_status: ExactValidationStatus::Passed,
                    },
                    geometry_signature: HASH128.to_owned(),
                    resource_signature: HASH128.to_owned(),
                    payload_checksum: "0123456789abcdef".to_owned(),
                    logical_bytes: 512,
                }],
            },
        }
    }

    #[test]
    fn request_and_completed_result_have_exact_compact_round_trips() {
        let request = request();
        let request_json = render_request(&request).unwrap();
        assert!(request_json.ends_with('\n'));
        assert!(!request_json[..request_json.len() - 1].contains('\n'));
        assert_eq!(parse_request(&request_json).unwrap(), request);

        let result = result();
        let result_json = render_result(&result).unwrap();
        assert!(result_json.ends_with('\n'));
        assert!(!result_json[..result_json.len() - 1].contains('\n'));
        assert_eq!(parse_result(&result_json).unwrap(), result);
    }

    #[test]
    fn strict_parsers_reject_unknown_missing_and_noncanonical_forms() {
        let request_json = render_request(&request()).unwrap();
        let unknown =
            request_json.replacen("\"schema_name\":", "\"unknown\":true,\"schema_name\":", 1);
        assert_eq!(
            parse_request(&unknown).unwrap_err().code,
            "CC-ROUTE-CONTRACT-001"
        );

        let missing = request_json.replacen("\"schema_version\":1,", "", 1);
        assert_eq!(
            parse_request(&missing).unwrap_err().code,
            "CC-ROUTE-CONTRACT-001"
        );

        let noncanonical = format!(" {}", request_json);
        assert_eq!(
            parse_request(&noncanonical).unwrap_err().code,
            "CC-ROUTE-CONTRACT-006"
        );
        assert_eq!(
            parse_request(request_json.trim_end()).unwrap_err().code,
            "CC-ROUTE-CONTRACT-006"
        );
    }

    #[test]
    fn parsers_and_renderers_enforce_the_64_mib_ceiling() {
        let oversized = " ".repeat(MAX_CONTRACT_BYTES + 1);
        assert_eq!(
            parse_request(&oversized).unwrap_err().code,
            "CC-ROUTE-CONTRACT-006"
        );

        let mut oversized_request = request();
        oversized_request.adapter_name = "x".repeat(MAX_CONTRACT_BYTES);
        assert_eq!(
            render_request(&oversized_request).unwrap_err().code,
            "CC-ROUTE-CONTRACT-006"
        );
    }

    #[test]
    fn discriminated_outcome_rejects_status_confusion() {
        let completed = render_result(&result()).unwrap();
        let mixed = completed.replacen(
            "\"kind\":\"completed\"",
            "\"kind\":\"completed\",\"status\":\"tool_failed\",\"diagnostic\":{\"code\":\"CC-ROUTE-TOOL-001\",\"path\":\"tool\",\"message\":\"failed\"}",
            1,
        );
        assert_eq!(
            parse_result(&mixed).unwrap_err().code,
            "CC-ROUTE-CONTRACT-001"
        );

        let mut failure = result();
        failure.outcome = RouteOutcome::Failure {
            status: RouteFailureStatus::RouteNotFound,
            diagnostic: ContractDiagnostic {
                code: "CC-ROUTE-SEARCH-001".to_owned(),
                path: "planar_route".to_owned(),
                message: "no exact planar route exists".to_owned(),
            },
        };
        let failure_json = render_result(&failure).unwrap();
        assert_eq!(parse_result(&failure_json).unwrap(), failure);

        let confused = failure_json.replacen(
            "\"kind\":\"failure\"",
            &format!("\"kind\":\"failure\",\"selected_candidate_id\":\"{HASH128}\""),
            1,
        );
        assert_eq!(
            parse_result(&confused).unwrap_err().code,
            "CC-ROUTE-CONTRACT-001"
        );
    }

    #[test]
    fn lowercase_digest_and_hex_helpers_are_exact() {
        assert_eq!(
            sha256_hex(b"abc"),
            "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad"
        );
        let mut request = request();
        request.design_fingerprint_sha256 = SHA.to_ascii_uppercase();
        assert_eq!(
            render_request(&request).unwrap_err().path,
            "design_fingerprint_sha256"
        );
        let mut result = result();
        if let RouteOutcome::Completed { candidates, .. } = &mut result.outcome {
            candidates[0].geometry_signature = HASH128.to_ascii_uppercase();
        }
        assert_eq!(
            render_result(&result).unwrap_err().path,
            "outcome.candidates[0].geometry_signature"
        );
    }

    #[test]
    fn request_relational_integrity_fails_closed() {
        let assert_path = |value: RouteRequestContract, expected_path: &str| {
            assert!(
                value
                    .validate()
                    .unwrap_err()
                    .iter()
                    .any(|diagnostic| diagnostic.path == expected_path),
                "expected relational diagnostic at {expected_path}"
            );
        };

        let mut unknown_net = request();
        unknown_net.terminals[0].net = entity(99);
        assert_path(unknown_net, "terminals[0].net");

        let mut mismatched_membership = request();
        mismatched_membership.nets[0].terminals[1] = entity(99);
        assert_path(mismatched_membership, "nets[0].terminals");

        let mut wrong_endpoint = request();
        wrong_endpoint.planar_route.goal = point(16_000_000, 16_000_000);
        assert_path(wrong_endpoint, "planar_route");

        let mut unknown_obstacle_layer = request();
        unknown_obstacle_layer.obstacles[0].layer = 1;
        assert_path(unknown_obstacle_layer, "obstacles[0].layer");

        let mut outside_roi = request();
        outside_roi.compiler_profile.active_regions[0].bounds.max.x = 20_000_001;
        assert_path(outside_roi, "compiler_profile.active_regions[0].bounds");
    }

    #[test]
    fn completed_candidate_collection_has_a_structural_bound() {
        let mut result = result();
        let RouteOutcome::Completed { candidates, .. } = &mut result.outcome else {
            unreachable!();
        };
        candidates.resize(MAX_CONTRACT_ENTRIES + 1, candidates[0].clone());
        assert!(result.validate().unwrap_err().iter().any(|diagnostic| {
            diagnostic.code == "CC-ROUTE-CONTRACT-006" && diagnostic.path == "outcome.candidates"
        }));
    }

    #[test]
    fn completed_resources_require_canonical_bounded_spans() {
        let mutate_and_find = |mutate: fn(&mut PhysicalEdgeSpan), expected_path: &str| {
            let mut result = result();
            let RouteOutcome::Completed { candidates, .. } = &mut result.outcome else {
                unreachable!();
            };
            mutate(&mut candidates[0].resources[0]);
            assert!(
                result
                    .validate()
                    .unwrap_err()
                    .iter()
                    .any(|diagnostic| diagnostic.path == expected_path)
            );
        };
        mutate_and_find(
            |span| span.direction = EdgeDirection::West,
            "outcome.candidates[0].resources[0].direction",
        );
        mutate_and_find(
            |span| span.edge_count = 0,
            "outcome.candidates[0].resources[0].edge_count",
        );
        mutate_and_find(
            |span| span.usage_units = 2,
            "outcome.candidates[0].resources[0].usage_units",
        );
    }

    #[test]
    fn explicit_selection_is_independent_of_candidate_array_order() {
        let mut result = result();
        let selected = {
            let RouteOutcome::Completed {
                selected_candidate_id,
                candidates,
            } = &mut result.outcome
            else {
                unreachable!();
            };
            let mut alternate = candidates[0].clone();
            alternate.id = "1123456789abcdef0123456789abcdef".to_owned();
            alternate.provenance.candidate_ordinal = 1;
            alternate.policy.candidate_ordinal = 1;
            alternate.geometry_signature = "1123456789abcdef0123456789abcdef".to_owned();
            candidates.push(alternate);
            selected_candidate_id.clone()
        };

        let first = render_result(&result).unwrap();
        let RouteOutcome::Completed { candidates, .. } = &mut result.outcome else {
            unreachable!();
        };
        candidates.reverse();
        let second = render_result(&result).unwrap();
        assert_ne!(first, second);
        for bytes in [&first, &second] {
            let parsed = parse_result(bytes).unwrap();
            let RouteOutcome::Completed {
                selected_candidate_id,
                candidates,
            } = parsed.outcome
            else {
                unreachable!();
            };
            assert_eq!(selected_candidate_id, selected);
            assert!(
                candidates
                    .iter()
                    .any(|candidate| candidate.id == selected_candidate_id)
            );
        }
    }
}
