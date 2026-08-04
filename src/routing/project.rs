//! Deterministic KiCad projection and evidence binding for an imported route.

use serde::{Deserialize, Serialize};

use crate::compile::{CompiledArtifacts, KicadIdentity, RelativeArtifactPath, compile};
use crate::design::{CopperLayer, PointNm, RouteSegment};

use super::contract::{
    AdmittedCandidate, ContractDiagnostic, RouteOutcome, ToolIdentity, sha256_hex,
};
use super::import::ImportedRoute;

const PROJECTION_SCHEMA_NAME: &str = "circuitc.apgar_route_projection";
const PROJECTION_SCHEMA_VERSION: u32 = 1;
const PROJECTION_ERROR: &str = "CC-ROUTE-PROJECTION-001";

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct ProjectedRoute {
    pub(crate) static_artifacts: CompiledArtifacts,
    pub(crate) projection_path: RelativeArtifactPath,
    pub(crate) projection_json: String,
    pub(crate) projection_sha256: String,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct RouteProjectionContract {
    schema_name: String,
    schema_version: u32,
    design_name: String,
    request_path: String,
    request_identity_sha256: String,
    request_sha256: String,
    result_sha256: String,
    selected_candidate_id: String,
    candidate_geometry_signature: String,
    candidate_resource_signature: String,
    candidate_payload_checksum: String,
    tool: ToolIdentity,
    segments: Vec<ProjectedSegment>,
    kicad_pcb_sha256: String,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct ProjectedSegment {
    ordinal: u64,
    semantic_path: String,
    kicad_uuid: String,
    net: String,
    layer: ProjectionLayer,
    start_nm: ProjectionPoint,
    end_nm: ProjectionPoint,
    width_nm: i64,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
enum ProjectionLayer {
    Front,
    Back,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct ProjectionPoint {
    x: i64,
    y: i64,
}

pub(crate) fn project_imported_route(
    imported: &ImportedRoute,
) -> Result<ProjectedRoute, ContractDiagnostic> {
    imported.design.validate().map_err(|diagnostics| {
        diagnostics.into_iter().next().map_or_else(
            || {
                projection_error(
                    "design",
                    "imported Design IR validation failed without a diagnostic",
                )
            },
            |diagnostic| {
                projection_error(
                    diagnostic.path,
                    format!("{}: {}", diagnostic.code, diagnostic.message),
                )
            },
        )
    })?;
    if !imported.design.board.routing_requests.is_empty() {
        return Err(projection_error(
            "design.board.routing_requests",
            "imported route projection requires a fully resolved Design IR",
        ));
    }
    let candidate = selected_candidate(imported)?;
    let static_artifacts = compile(&imported.design).map_err(|error| {
        error.diagnostics.into_iter().next().map_or_else(
            || projection_error("design", "KiCad projection failed without a diagnostic"),
            |diagnostic| {
                projection_error(
                    diagnostic.path,
                    format!("{}: {}", diagnostic.code, diagnostic.message),
                )
            },
        )
    })?;
    let segments = bind_segments(imported, candidate, &static_artifacts)?;
    let contract = RouteProjectionContract {
        schema_name: PROJECTION_SCHEMA_NAME.to_owned(),
        schema_version: PROJECTION_SCHEMA_VERSION,
        design_name: imported.design.name.clone(),
        request_path: imported.result.request_path.clone(),
        request_identity_sha256: imported.result.replay.request_identity_sha256.clone(),
        request_sha256: imported.request_sha256.clone(),
        result_sha256: sha256_hex(imported.result_json.as_bytes()),
        selected_candidate_id: candidate.id.clone(),
        candidate_geometry_signature: candidate.geometry_signature.clone(),
        candidate_resource_signature: candidate.resource_signature.clone(),
        candidate_payload_checksum: candidate.payload_checksum.clone(),
        tool: imported.result.tool.clone(),
        segments,
        kicad_pcb_sha256: sha256_hex(static_artifacts.kicad_pcb.as_bytes()),
    };
    let mut projection_json = serde_json::to_string(&contract).map_err(|error| {
        projection_error(
            "projection",
            format!("could not encode canonical route projection: {error}"),
        )
    })?;
    projection_json.push('\n');
    let reparsed: RouteProjectionContract =
        serde_json::from_str(&projection_json).map_err(|error| {
            projection_error(
                "projection",
                format!("could not reparse canonical route projection: {error}"),
            )
        })?;
    if reparsed != contract {
        return Err(projection_error(
            "projection",
            "canonical route projection did not round-trip exactly",
        ));
    }
    let projection_path = RelativeArtifactPath::try_new(format!(
        "routing/{}/projection.json",
        imported.result.replay.request_identity_sha256
    ))
    .map_err(|error| {
        projection_error(
            "projection_path",
            format!("could not derive route-projection artifact path: {error}"),
        )
    })?;
    let projection_sha256 = sha256_hex(projection_json.as_bytes());
    Ok(ProjectedRoute {
        static_artifacts,
        projection_path,
        projection_json,
        projection_sha256,
    })
}

fn selected_candidate(imported: &ImportedRoute) -> Result<&AdmittedCandidate, ContractDiagnostic> {
    let RouteOutcome::Completed {
        selected_candidate_id,
        candidates,
    } = &imported.result.outcome
    else {
        return Err(projection_error(
            "result.outcome",
            "only an authenticated completed result can be projected",
        ));
    };
    if selected_candidate_id != &imported.selected_candidate_id {
        return Err(projection_error(
            "result.outcome.selected_candidate_id",
            "imported selected-candidate identity no longer matches the result",
        ));
    }
    candidates
        .iter()
        .find(|candidate| candidate.id == *selected_candidate_id)
        .ok_or_else(|| {
            projection_error(
                "result.outcome.selected_candidate_id",
                "selected candidate is absent from the imported result",
            )
        })
}

fn bind_segments(
    imported: &ImportedRoute,
    candidate: &AdmittedCandidate,
    artifacts: &CompiledArtifacts,
) -> Result<Vec<ProjectedSegment>, ContractDiagnostic> {
    let prefix = format!("{}.segment.", imported.result.request_path);
    let routes: Vec<_> = imported
        .design
        .board
        .routes
        .iter()
        .filter(|route| route.path.starts_with(&prefix))
        .collect();
    if routes.len() != candidate.geometry.len() {
        return Err(projection_error(
            "design.board.routes",
            "imported route segment count no longer matches selected APGAR geometry",
        ));
    }
    routes
        .iter()
        .enumerate()
        .map(|(index, route)| {
            let expected_path = format!("{prefix}{index:08}");
            if route.path != expected_path {
                return Err(projection_error(
                    &route.path,
                    "imported route segment path is not the canonical candidate ordinal mapping",
                ));
            }
            let line = candidate.geometry[index];
            let expected_start = point_nm(line.start);
            let expected_end = point_nm(line.end);
            let expected_layer = copper_layer(line.layer).ok_or_else(|| {
                projection_error(
                    &route.path,
                    "selected APGAR primitive names an unsupported copper layer",
                )
            })?;
            if route.start != expected_start
                || route.end != expected_end
                || route.width_nm != line.width_dbu / 2
                || route.layer != expected_layer
            {
                return Err(projection_error(
                    &route.path,
                    "imported route geometry no longer equals the selected APGAR primitive",
                ));
            }
            let identity = unique_identity(&artifacts.kicad_identities, &route.path)?;
            let pcb_identity = format!("(uuid \"{}\")", identity.uuid);
            if artifacts.kicad_pcb.match_indices(&pcb_identity).count() != 1 {
                return Err(projection_error(
                    &route.path,
                    "route identity is not unique in the emitted KiCad PCB",
                ));
            }
            Ok(projected_segment(index, route, identity))
        })
        .collect()
}

const fn copper_layer(routing_id: u32) -> Option<CopperLayer> {
    match routing_id {
        0 => Some(CopperLayer::Front),
        31 => Some(CopperLayer::Back),
        _ => None,
    }
}

fn unique_identity<'a>(
    identities: &'a [KicadIdentity],
    path: &str,
) -> Result<&'a KicadIdentity, ContractDiagnostic> {
    let mut matches = identities
        .iter()
        .filter(|identity| identity.semantic_path == path);
    let identity = matches
        .next()
        .ok_or_else(|| projection_error(path, "imported route has no emitted KiCad identity"))?;
    if matches.next().is_some() {
        return Err(projection_error(
            path,
            "imported route has more than one emitted KiCad identity",
        ));
    }
    Ok(identity)
}

fn projected_segment(
    index: usize,
    route: &RouteSegment,
    identity: &KicadIdentity,
) -> ProjectedSegment {
    ProjectedSegment {
        ordinal: index as u64,
        semantic_path: route.path.clone(),
        kicad_uuid: identity.uuid.clone(),
        net: route.net.clone(),
        layer: match route.layer {
            CopperLayer::Front => ProjectionLayer::Front,
            CopperLayer::Back => ProjectionLayer::Back,
        },
        start_nm: projection_point(route.start),
        end_nm: projection_point(route.end),
        width_nm: route.width_nm,
    }
}

const fn point_nm(point: super::contract::PointDbu) -> PointNm {
    PointNm::new(point.x / 2, point.y / 2)
}

const fn projection_point(point: PointNm) -> ProjectionPoint {
    ProjectionPoint {
        x: point.x,
        y: point.y,
    }
}

fn projection_error(path: impl Into<String>, message: impl Into<String>) -> ContractDiagnostic {
    ContractDiagnostic {
        code: PROJECTION_ERROR.to_owned(),
        path: path.into(),
        message: message.into(),
    }
}

#[cfg(test)]
mod tests {
    use crate::demo;
    use crate::design::{CopperLayer, RoutingRequest, SimulationAnalysis, SimulationAnalysisKind};

    use super::super::contract::{
        AdmittedCandidate, CONTRACT_SCHEMA_VERSION, CandidateBackendKind, CandidateGeneratorKind,
        CandidateMetrics, CandidateProvenance, ConstraintAssessment, ExactValidationStatus,
        LinePrimitive, RESULT_SCHEMA_NAME, ReplayIdentity, RouteOutcome, RouteResultContract,
        render_result,
    };
    use super::super::import::{
        candidate_checksum_and_bytes, candidate_id, derive_candidate_fields, expected_associations,
        expected_cpu_tool, fingerprint_policy, geometry_signature, import_result,
        resource_signature,
    };
    use super::super::lower::lower_request;
    use super::{
        RouteProjectionContract, bind_segments, project_imported_route, selected_candidate,
    };

    const MM: i64 = 1_000_000;
    const EXECUTABLE_SHA: &str = "0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef";

    fn imported() -> super::super::import::ImportedRoute {
        let mut design = demo::voltage_divider();
        design.board.routes.clear();
        design.board.routing_requests.push(RoutingRequest {
            path: "board.autoroute.vout".to_owned(),
            net: "VOUT".to_owned(),
            width_nm: 250_000,
            clearance_nm: 200_000,
            grid_step_nm: MM,
            layer: CopperLayer::Front,
        });
        design.canonicalize();
        let bundle = lower_request(&design).unwrap().unwrap();
        let request = &bundle.request;
        let target = request
            .nets
            .iter()
            .find(|net| net.reference == request.routing_profile.net)
            .unwrap();
        let mut candidate = AdmittedCandidate {
            schema_major: 1,
            schema_minor: 0,
            id: "00000000000000000000000000000001".to_owned(),
            net: request.routing_profile.net,
            intended_terminals: target.terminals.clone().try_into().unwrap(),
            associations: expected_associations(request),
            geometry_schema_version: 1,
            resource_schema_version: 1,
            policy: request.planar_route.candidate_policy.clone(),
            policy_identity: fingerprint_policy(&request.planar_route.candidate_policy),
            provenance: CandidateProvenance {
                generator: CandidateGeneratorKind::CpuAStar,
                generator_version: 1,
                backend: CandidateBackendKind::Cpu,
                supported_device_class: super::super::APGAR_CPU_DEVICE_CLASS.to_owned(),
                deterministic_seed: 0,
                batch_identity: request.planar_route.scheduling.batch_identity,
                query_identity: request.planar_route.scheduling.query_identity,
                candidate_ordinal: 0,
            },
            geometry: vec![LinePrimitive {
                layer: request.routing_profile.allowed_layers[0],
                start: request.planar_route.start,
                end: request.planar_route.goal,
                width_dbu: request.routing_profile.nominal_width_dbu,
            }],
            resources: Vec::new(),
            metrics: CandidateMetrics {
                scalar_policy_cost: 0,
                intrinsic_base_cost: 0,
                orthogonal_step_count: 0,
                diagonal_step_count: 0,
                bend_count: 0,
                line_primitive_count: 1,
                via_count: 0,
                axis_aligned_length_dbu: 0,
                diagonal_projection_dbu: 0,
            },
            constraints: ConstraintAssessment {
                supported_hard_constraints_satisfied: true,
                unsupported_rules_remain: false,
                connected_intended_terminal_count: 2,
                exact_validation_status: ExactValidationStatus::Passed,
            },
            geometry_signature: "00000000000000000000000000000001".to_owned(),
            resource_signature: "00000000000000000000000000000001".to_owned(),
            payload_checksum: "0000000000000001".to_owned(),
            logical_bytes: 1,
        };
        let derived = derive_candidate_fields(request, &candidate).unwrap();
        candidate.resources = derived.resources;
        candidate.metrics = derived.metrics;
        candidate.id = candidate_id(
            candidate.net,
            &candidate.associations,
            candidate.policy_identity,
            &candidate.provenance,
        );
        candidate.geometry_signature = geometry_signature(&candidate.geometry);
        candidate.resource_signature = resource_signature(&candidate.resources);
        let (checksum, logical_bytes) = candidate_checksum_and_bytes(&candidate);
        candidate.payload_checksum = format!("{checksum:016x}");
        candidate.logical_bytes = logical_bytes;
        let result = RouteResultContract {
            schema_name: RESULT_SCHEMA_NAME.to_owned(),
            schema_version: CONTRACT_SCHEMA_VERSION,
            request_sha256: bundle.request_sha256.clone(),
            request_path: request.request_path.clone(),
            tool: expected_cpu_tool(EXECUTABLE_SHA.to_owned()),
            replay: ReplayIdentity {
                design_fingerprint_sha256: request.design_fingerprint_sha256.clone(),
                request_identity_sha256: request.request_identity_sha256.clone(),
                board_revision: request.board_revision,
                deterministic_seed: 0,
                batch_identity: request.planar_route.scheduling.batch_identity,
                query_identity: request.planar_route.scheduling.query_identity,
            },
            outcome: RouteOutcome::Completed {
                selected_candidate_id: candidate.id.clone(),
                candidates: vec![candidate],
            },
        };
        let result_json = render_result(&result).unwrap();
        import_result(
            &design,
            &bundle,
            &result_json,
            &expected_cpu_tool(EXECUTABLE_SHA.to_owned()),
        )
        .unwrap()
    }

    #[test]
    fn imported_route_projects_to_deterministic_kicad_and_bound_manifest() {
        let imported = imported();
        let first = project_imported_route(&imported).unwrap();
        let second = project_imported_route(&imported).unwrap();
        assert_eq!(first, second);
        assert!(first.static_artifacts.kicad_pcb.contains("(segment"));
        let manifest: RouteProjectionContract =
            serde_json::from_str(&first.projection_json).unwrap();
        assert_eq!(manifest.segments.len(), 1);
        assert!(
            first
                .static_artifacts
                .kicad_pcb
                .contains(&manifest.segments[0].kicad_uuid)
        );
        assert_eq!(manifest.kicad_pcb_sha256.len(), 64);
        assert_eq!(first.projection_sha256.len(), 64);
        assert!(first.projection_json.ends_with('\n'));
    }

    #[test]
    fn route_geometry_or_selected_identity_drift_is_rejected() {
        let mut geometry_drift = imported();
        geometry_drift.design.board.routes[0].start.x += 1;
        assert!(project_imported_route(&geometry_drift).is_err());

        let mut layer_drift = imported();
        layer_drift.design.board.routes[0].layer = CopperLayer::Back;
        let error = project_imported_route(&layer_drift).unwrap_err();
        assert!(error.message.contains("selected APGAR primitive"));

        let mut identity_drift = imported();
        identity_drift.selected_candidate_id = "ffffffffffffffffffffffffffffffff".to_owned();
        assert!(project_imported_route(&identity_drift).is_err());
    }

    #[test]
    fn declared_simulation_analysis_fails_closed_before_projection() {
        let mut imported = imported();
        imported.design.analyses.push(SimulationAnalysis {
            path: "divider.simulation.op".to_owned(),
            kind: SimulationAnalysisKind::DcOperatingPoint,
        });
        imported.design.canonicalize();

        let error = project_imported_route(&imported).unwrap_err();
        assert_eq!(error.path, "design.analyses.divider.simulation.op");
        assert!(error.message.contains("CC-SIM-PHASE-001"));
    }

    #[test]
    fn segment_count_and_canonical_path_drift_are_rejected() {
        let mut missing_segment = imported();
        missing_segment.design.board.routes.clear();
        let error = project_imported_route(&missing_segment).unwrap_err();
        assert_eq!(error.path, "design.board.routes");
        assert!(error.message.contains("segment count"));

        let mut path_drift = imported();
        path_drift.design.board.routes[0].path = "board.autoroute.vout.segment.00000001".to_owned();
        let error = project_imported_route(&path_drift).unwrap_err();
        assert_eq!(error.path, "board.autoroute.vout.segment.00000001");
        assert!(error.message.contains("canonical candidate ordinal"));
    }

    #[test]
    fn duplicate_route_uuid_in_emitted_pcb_is_rejected() {
        let imported = imported();
        let projected = project_imported_route(&imported).unwrap();
        let candidate = selected_candidate(&imported).unwrap();
        let mut artifacts = projected.static_artifacts;
        let route_identity = artifacts
            .kicad_identities
            .iter()
            .find(|identity| {
                identity
                    .semantic_path
                    .starts_with("board.autoroute.vout.segment.")
            })
            .unwrap();
        artifacts
            .kicad_pcb
            .push_str(&format!("\n(uuid \"{}\")\n", route_identity.uuid));
        let error = bind_segments(&imported, candidate, &artifacts).unwrap_err();
        assert_eq!(error.path, "board.autoroute.vout.segment.00000000");
        assert!(error.message.contains("not unique"));
    }
}
