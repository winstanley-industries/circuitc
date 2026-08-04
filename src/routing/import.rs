//! Authentication and lossless import of an APGAR route result.

use crate::compile::RelativeArtifactPath;
use crate::design::{CopperLayer, Design, Diagnostic, PointNm, RouteSegment};

use super::contract::{
    AdmittedCandidate, CandidateAssociations, CandidateBackendKind, CandidateGeneratorKind,
    CandidateMetrics, CandidateObjective, CandidatePolicyContract, CandidateProvenance,
    ContractDiagnostic, EdgeDirection, EdgeResourceContract, EntityRef, ExactValidationStatus,
    Heading, LinePrimitive, PhysicalEdgeSpan, PointDbu, ReplayIdentity, RouteOutcome,
    RouteRequestContract, RouteResultContract, ToolIdentity, parse_request, parse_result,
    sha256_hex,
};
use super::lower::{RouteInputBundle, lower_request};
use super::{
    APGAR_CONTRACT_IDENTITY, APGAR_CPU_DEVICE_CLASS, APGAR_TOOL_NAME, APGAR_TOOL_VERSION,
    PINNED_APGAR_SOURCE_REVISION,
};

const IMPORT_CONTRACT: &str = "CC-ROUTE-IMPORT-001";
const IMPORT_REQUEST: &str = "CC-ROUTE-IMPORT-002";
const IMPORT_TOOL: &str = "CC-ROUTE-IMPORT-003";
const IMPORT_REPLAY: &str = "CC-ROUTE-IMPORT-004";
const IMPORT_ASSOCIATION: &str = "CC-ROUTE-IMPORT-005";
const IMPORT_PAYLOAD: &str = "CC-ROUTE-IMPORT-006";
const IMPORT_GEOMETRY: &str = "CC-ROUTE-IMPORT-007";
const IMPORT_CONVERSION: &str = "CC-ROUTE-IMPORT-008";
const IMPORT_DESIGN: &str = "CC-ROUTE-IMPORT-009";

const GEOMETRY_COMPILER_VERSION: u32 = 1;
const NO_INCOMING_DIRECTION: u8 = 8;

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct ImportedRoute {
    pub(crate) design: Design,
    pub(crate) request_path: RelativeArtifactPath,
    pub(crate) result_path: RelativeArtifactPath,
    pub(crate) request_json: String,
    pub(crate) request_sha256: String,
    pub(crate) result_json: String,
    pub(crate) result: RouteResultContract,
    pub(crate) selected_candidate_id: String,
}

pub(crate) fn expected_cpu_tool(executable_sha256: String) -> ToolIdentity {
    ToolIdentity {
        name: APGAR_TOOL_NAME.to_owned(),
        version: APGAR_TOOL_VERSION.to_owned(),
        contract_identity: APGAR_CONTRACT_IDENTITY.to_owned(),
        source_revision: PINNED_APGAR_SOURCE_REVISION.to_owned(),
        executable_sha256,
        device_class: APGAR_CPU_DEVICE_CLASS.to_owned(),
    }
}

pub(crate) fn import_result(
    design: &Design,
    bundle: &RouteInputBundle,
    result_json: &str,
    expected_tool: &ToolIdentity,
) -> Result<ImportedRoute, ContractDiagnostic> {
    authenticate_current_request(design, bundle)?;
    let result = parse_result(result_json).map_err(|error| {
        import_error(
            IMPORT_CONTRACT,
            error.path,
            format!(
                "APGAR result violates its canonical contract: {}",
                error.message
            ),
        )
    })?;
    authenticate_result_root(bundle, &result, expected_tool)?;
    let (selected_candidate_id, candidate) = match &result.outcome {
        RouteOutcome::Completed {
            selected_candidate_id,
            candidates,
        } => {
            let candidate = candidates
                .iter()
                .find(|candidate| candidate.id == *selected_candidate_id)
                .ok_or_else(|| {
                    import_error(
                        IMPORT_ASSOCIATION,
                        "outcome.selected_candidate_id",
                        "selected candidate is absent from the completed result",
                    )
                })?;
            (selected_candidate_id.clone(), candidate)
        }
        RouteOutcome::Failure { diagnostic, .. } => {
            return Err(import_error(
                IMPORT_PAYLOAD,
                &bundle.request.request_path,
                format!(
                    "APGAR routing did not complete: {}: {}",
                    diagnostic.code, diagnostic.message
                ),
            ));
        }
    };
    authenticate_candidate(&bundle.request, candidate)?;

    let imported_design = import_geometry(design, &bundle.request, candidate)?;
    let result_path = RelativeArtifactPath::try_new(format!(
        "routing/{}/result.json",
        bundle.request.request_identity_sha256
    ))
    .map_err(|error| {
        import_error(
            IMPORT_DESIGN,
            &bundle.request.request_path,
            format!("could not derive route-result artifact path: {error}"),
        )
    })?;
    Ok(ImportedRoute {
        design: imported_design,
        request_path: bundle.request_path.clone(),
        result_path,
        request_json: bundle.request_json.clone(),
        request_sha256: bundle.request_sha256.clone(),
        result_json: result_json.to_owned(),
        result,
        selected_candidate_id,
    })
}

fn authenticate_current_request(
    design: &Design,
    bundle: &RouteInputBundle,
) -> Result<(), ContractDiagnostic> {
    let parsed = parse_request(&bundle.request_json).map_err(|error| {
        import_error(
            IMPORT_CONTRACT,
            error.path,
            format!(
                "stored APGAR request violates its canonical contract: {}",
                error.message
            ),
        )
    })?;
    if parsed != bundle.request {
        return Err(import_error(
            IMPORT_REQUEST,
            "request",
            "stored request bytes do not equal the in-memory request contract",
        ));
    }
    if sha256_hex(bundle.request_json.as_bytes()) != bundle.request_sha256 {
        return Err(import_error(
            IMPORT_REQUEST,
            "request_sha256",
            "stored request digest does not authenticate the exact request bytes",
        ));
    }
    let current = lower_request(design)
        .map_err(first_design_diagnostic)?
        .ok_or_else(|| {
            import_error(
                IMPORT_REQUEST,
                "design.board.routing_requests",
                "current Design IR no longer contains the routed request",
            )
        })?;
    if current.request_json != bundle.request_json
        || current.request_sha256 != bundle.request_sha256
        || current.request_path != bundle.request_path
    {
        return Err(import_error(
            IMPORT_REQUEST,
            &bundle.request.request_path,
            "routing request is stale relative to the current canonical Design IR",
        ));
    }
    Ok(())
}

fn authenticate_result_root(
    bundle: &RouteInputBundle,
    result: &RouteResultContract,
    expected_tool: &ToolIdentity,
) -> Result<(), ContractDiagnostic> {
    if result.request_sha256 != bundle.request_sha256
        || result.request_path != bundle.request.request_path
    {
        return Err(import_error(
            IMPORT_REQUEST,
            "result.request_sha256",
            "APGAR result does not bind the exact current request bytes and semantic path",
        ));
    }
    if result.tool != *expected_tool
        || result.tool.name != APGAR_TOOL_NAME
        || result.tool.version != APGAR_TOOL_VERSION
        || result.tool.contract_identity != APGAR_CONTRACT_IDENTITY
        || result.tool.source_revision != PINNED_APGAR_SOURCE_REVISION
        || result.tool.device_class != APGAR_CPU_DEVICE_CLASS
    {
        return Err(import_error(
            IMPORT_TOOL,
            "result.tool",
            "APGAR result tool identity does not equal the authenticated pinned CPU executable",
        ));
    }
    let expected_replay = ReplayIdentity {
        design_fingerprint_sha256: bundle.request.design_fingerprint_sha256.clone(),
        request_identity_sha256: bundle.request.request_identity_sha256.clone(),
        board_revision: bundle.request.board_revision,
        deterministic_seed: bundle
            .request
            .planar_route
            .candidate_policy
            .deterministic_seed,
        batch_identity: bundle.request.planar_route.scheduling.batch_identity,
        query_identity: bundle.request.planar_route.scheduling.query_identity,
    };
    if result.replay != expected_replay {
        return Err(import_error(
            IMPORT_REPLAY,
            "result.replay",
            "APGAR result replay identity is stale or belongs to another route request",
        ));
    }
    Ok(())
}

fn authenticate_candidate(
    request: &RouteRequestContract,
    candidate: &AdmittedCandidate,
) -> Result<(), ContractDiagnostic> {
    let target_net = request
        .nets
        .iter()
        .find(|net| net.reference == request.routing_profile.net)
        .ok_or_else(|| internal_import_error(request, "routed net is absent after validation"))?;
    let expected_terminals: [EntityRef; 2] =
        target_net.terminals.clone().try_into().map_err(|_| {
            internal_import_error(request, "routed net is not exactly two-terminal")
        })?;
    if candidate.net != request.routing_profile.net
        || candidate.intended_terminals != expected_terminals
        || candidate.policy != request.planar_route.candidate_policy
    {
        return Err(import_error(
            IMPORT_ASSOCIATION,
            "outcome.candidate",
            "selected candidate net, terminals, or normalized policy do not match the request",
        ));
    }
    let expected_provenance = CandidateProvenance {
        generator: CandidateGeneratorKind::CpuAStar,
        generator_version: 1,
        backend: CandidateBackendKind::Cpu,
        supported_device_class: APGAR_CPU_DEVICE_CLASS.to_owned(),
        deterministic_seed: request.planar_route.candidate_policy.deterministic_seed,
        batch_identity: request.planar_route.scheduling.batch_identity,
        query_identity: request.planar_route.scheduling.query_identity,
        candidate_ordinal: request.planar_route.candidate_policy.candidate_ordinal,
    };
    if candidate.provenance != expected_provenance
        || !candidate.constraints.supported_hard_constraints_satisfied
        || candidate.constraints.unsupported_rules_remain
        || candidate.constraints.connected_intended_terminal_count != 2
        || candidate.constraints.exact_validation_status != ExactValidationStatus::Passed
    {
        return Err(import_error(
            IMPORT_ASSOCIATION,
            "outcome.candidate.provenance",
            "selected candidate lacks the exact pinned CPU admission provenance",
        ));
    }

    let associations = expected_associations(request);
    let policy_identity = fingerprint_policy(&candidate.policy);
    if candidate.associations != associations || candidate.policy_identity != policy_identity {
        return Err(import_error(
            IMPORT_ASSOCIATION,
            "outcome.candidate.associations",
            "selected candidate associations or policy identity do not recompute from the request",
        ));
    }
    let expected_id = candidate_id(
        candidate.net,
        &associations,
        policy_identity,
        &candidate.provenance,
    );
    if candidate.id != expected_id {
        return Err(import_error(
            IMPORT_PAYLOAD,
            "outcome.candidate.id",
            "selected candidate identity does not recompute from its authenticated fields",
        ));
    }

    let derived = derive_candidate_fields(request, candidate)?;
    if candidate.resources != derived.resources || candidate.metrics != derived.metrics {
        return Err(import_error(
            IMPORT_PAYLOAD,
            "outcome.candidate.resources",
            "selected candidate resources or metrics do not independently reconstruct from geometry",
        ));
    }
    let geometry_signature = geometry_signature(&candidate.geometry);
    let resource_signature = resource_signature(&candidate.resources);
    if candidate.geometry_signature != geometry_signature
        || candidate.resource_signature != resource_signature
    {
        return Err(import_error(
            IMPORT_PAYLOAD,
            "outcome.candidate.geometry_signature",
            "selected candidate public signatures do not match its exact payload",
        ));
    }
    let (checksum, logical_bytes) = candidate_checksum_and_bytes(candidate);
    if candidate.payload_checksum != format!("{checksum:016x}")
        || candidate.logical_bytes != logical_bytes
    {
        return Err(import_error(
            IMPORT_PAYLOAD,
            "outcome.candidate.payload_checksum",
            "selected candidate checksum or logical byte count does not match its exact public payload",
        ));
    }
    Ok(())
}

fn import_geometry(
    design: &Design,
    request: &RouteRequestContract,
    candidate: &AdmittedCandidate,
) -> Result<Design, ContractDiagnostic> {
    let mut imported = design.clone();
    let request_index = imported
        .board
        .routing_requests
        .iter()
        .position(|routing| routing.path == request.request_path)
        .ok_or_else(|| {
            import_error(
                IMPORT_REQUEST,
                &request.request_path,
                "current Design IR lost the authenticated routing request before import",
            )
        })?;
    let authored = imported.board.routing_requests.remove(request_index);
    let selected_layer = request.routing_profile.allowed_layers[0];
    let layer = copper_layer(selected_layer).ok_or_else(|| {
        import_error(
            IMPORT_GEOMETRY,
            "outcome.candidate.geometry.layer",
            "candidate selected an unsupported CircuitC copper layer",
        )
    })?;

    for (index, line) in candidate.geometry.iter().enumerate() {
        imported.board.routes.push(RouteSegment {
            path: format!("{}.segment.{index:08}", authored.path),
            net: authored.net.clone(),
            start: point_to_nm(
                line.start,
                &format!("outcome.candidate.geometry[{index}].start"),
            )?,
            end: point_to_nm(
                line.end,
                &format!("outcome.candidate.geometry[{index}].end"),
            )?,
            width_nm: dbu_to_nm(
                line.width_dbu,
                &format!("outcome.candidate.geometry[{index}].width_dbu"),
            )?,
            layer,
        });
    }
    imported.canonicalize();
    imported.validate().map_err(first_design_diagnostic)?;
    Ok(imported)
}

pub(super) struct DerivedFields {
    pub(super) resources: Vec<PhysicalEdgeSpan>,
    pub(super) metrics: CandidateMetrics,
}

pub(super) fn derive_candidate_fields(
    request: &RouteRequestContract,
    candidate: &AdmittedCandidate,
) -> Result<DerivedFields, ContractDiagnostic> {
    let profile = &request.compiler_profile;
    let selected_layer = request.routing_profile.allowed_layers[0];
    let width = request.routing_profile.nominal_width_dbu;
    let endpoint = |reference: EntityRef| {
        request
            .terminals
            .iter()
            .find(|terminal| terminal.reference == reference)
            .map(|terminal| terminal.center)
            .ok_or_else(|| {
                import_error(
                    IMPORT_ASSOCIATION,
                    "outcome.candidate.intended_terminals",
                    "candidate intended-terminal reference is absent from the authenticated request",
                )
            })
    };
    let expected_start = endpoint(candidate.intended_terminals[0])?;
    let expected_goal = endpoint(candidate.intended_terminals[1])?;
    let mut atomic_resources = Vec::new();
    let mut incoming = NO_INCOMING_DIRECTION;
    let mut orthogonal_steps = 0_u64;
    let mut diagonal_steps = 0_u64;
    let mut bends = 0_u64;
    let mut expanded_steps = 0_u64;
    let mut scalar_cost = 0_u64;
    let mut intrinsic_cost = 0_u64;

    for (index, line) in candidate.geometry.iter().enumerate() {
        let path = format!("outcome.candidate.geometry[{index}]");
        if line.layer != selected_layer || line.width_dbu != width {
            return Err(import_error(
                IMPORT_GEOMETRY,
                path,
                "candidate line layer or width differs from the exact request",
            ));
        }
        if index == 0 && line.start != expected_start {
            return Err(import_error(
                IMPORT_GEOMETRY,
                path,
                "candidate chain does not start at the first authenticated terminal",
            ));
        }
        if index + 1 == candidate.geometry.len() && line.end != expected_goal {
            return Err(import_error(
                IMPORT_GEOMETRY,
                path,
                "candidate chain does not end at the second authenticated terminal",
            ));
        }
        for (point, field) in [(line.start, "start"), (line.end, "end")] {
            point_to_nm(point, &format!("{path}.{field}"))?;
            if !point_on_lattice(point, profile.lattice_origin, profile.lattice_step_dbu) {
                return Err(import_error(
                    IMPORT_GEOMETRY,
                    format!("{path}.{field}"),
                    "candidate point is not exactly aligned to the authenticated routing lattice",
                ));
            }
        }
        dbu_to_nm(line.width_dbu, &format!("{path}.width_dbu"))?;
        let start = lattice_index(line.start, profile.lattice_origin, profile.lattice_step_dbu);
        let end = lattice_index(line.end, profile.lattice_origin, profile.lattice_step_dbu);
        let dx = end.0 - start.0;
        let dy = end.1 - start.1;
        let direction = direction_for_delta(dx, dy).ok_or_else(|| {
            import_error(
                IMPORT_GEOMETRY,
                &path,
                "candidate line is not non-zero horizontal, vertical, or exact 45-degree geometry",
            )
        })?;
        if index != 0 && incoming == direction {
            return Err(import_error(
                IMPORT_GEOMETRY,
                path,
                "adjacent collinear candidate lines were not canonically coalesced",
            ));
        }
        let step_count = dx.unsigned_abs().max(dy.unsigned_abs());
        expanded_steps = expanded_steps.checked_add(step_count).ok_or_else(|| {
            import_error(
                IMPORT_GEOMETRY,
                &path,
                "candidate expanded-resource count overflows uint64",
            )
        })?;
        if expanded_steps > request.resource_limits.expanded_resource_edges {
            return Err(import_error(
                IMPORT_GEOMETRY,
                "outcome.candidate.resources",
                "candidate exceeds the authenticated expanded-resource bound",
            ));
        }
        let mut source = start;
        for _ in 0..step_count {
            let resource = canonical_resource(selected_layer, source.0, source.1, direction);
            let step_cost =
                policy_step_cost(request, &candidate.policy, direction, incoming, resource)?;
            scalar_cost = scalar_cost.checked_add(step_cost).ok_or_else(|| {
                import_error(
                    IMPORT_PAYLOAD,
                    &path,
                    "candidate scalar cost overflows uint64",
                )
            })?;
            let base = if is_diagonal(direction) {
                u64::from(profile.costs.diagonal_step)
            } else {
                u64::from(profile.costs.orthogonal_step)
            }
            .checked_add(
                if incoming != NO_INCOMING_DIRECTION && incoming != direction {
                    u64::from(profile.costs.bend)
                } else {
                    0
                },
            )
            .ok_or_else(|| import_error(IMPORT_PAYLOAD, &path, "candidate base cost overflows"))?;
            intrinsic_cost = intrinsic_cost.checked_add(base).ok_or_else(|| {
                import_error(
                    IMPORT_PAYLOAD,
                    &path,
                    "candidate intrinsic cost overflows uint64",
                )
            })?;
            if incoming != NO_INCOMING_DIRECTION && incoming != direction {
                bends = bends.checked_add(1).ok_or_else(|| {
                    import_error(
                        IMPORT_PAYLOAD,
                        &path,
                        "candidate bend count overflows uint64",
                    )
                })?;
            }
            if is_diagonal(direction) {
                diagonal_steps = diagonal_steps.checked_add(1).ok_or_else(|| {
                    import_error(
                        IMPORT_PAYLOAD,
                        &path,
                        "candidate step count overflows uint64",
                    )
                })?;
            } else {
                orthogonal_steps = orthogonal_steps.checked_add(1).ok_or_else(|| {
                    import_error(
                        IMPORT_PAYLOAD,
                        &path,
                        "candidate step count overflows uint64",
                    )
                })?;
            }
            atomic_resources.push(resource);
            let delta = direction_delta(direction);
            source.0 += delta.0;
            source.1 += delta.1;
            incoming = direction;
        }
    }
    atomic_resources.sort();
    if atomic_resources.windows(2).any(|pair| pair[0] == pair[1]) {
        return Err(import_error(
            IMPORT_GEOMETRY,
            "outcome.candidate.geometry",
            "candidate traverses the same canonical physical edge more than once",
        ));
    }
    if atomic_resources.len() as u64 > request.resource_limits.expanded_resource_edges {
        return Err(import_error(
            IMPORT_GEOMETRY,
            "outcome.candidate.resources",
            "candidate exceeds the authenticated expanded-resource bound",
        ));
    }
    let resources = compress_resources(&atomic_resources);
    let step = u64::try_from(profile.lattice_step_dbu)
        .map_err(|_| internal_import_error(request, "validated lattice step is not positive"))?;
    let axis_aligned_length_dbu = orthogonal_steps.checked_mul(step).ok_or_else(|| {
        import_error(
            IMPORT_PAYLOAD,
            "outcome.candidate.metrics",
            "axis length overflows uint64",
        )
    })?;
    let diagonal_projection_dbu = diagonal_steps.checked_mul(step).ok_or_else(|| {
        import_error(
            IMPORT_PAYLOAD,
            "outcome.candidate.metrics",
            "diagonal projection overflows uint64",
        )
    })?;
    Ok(DerivedFields {
        resources,
        metrics: CandidateMetrics {
            scalar_policy_cost: scalar_cost,
            intrinsic_base_cost: intrinsic_cost,
            orthogonal_step_count: orthogonal_steps,
            diagonal_step_count: diagonal_steps,
            bend_count: bends,
            line_primitive_count: candidate.geometry.len() as u64,
            via_count: 0,
            axis_aligned_length_dbu,
            diagonal_projection_dbu,
        },
    })
}

pub(super) fn expected_associations(request: &RouteRequestContract) -> CandidateAssociations {
    CandidateAssociations {
        board_content_hash: board_content_hash(request),
        compiler_profile_fingerprint: compiler_profile_fingerprint(request),
        geometry_compiler_version: GEOMETRY_COMPILER_VERSION,
        routing_profile_fingerprint: routing_profile_fingerprint(request),
        rule_bucket_identity: rule_bucket_identity(request),
    }
}

fn board_content_hash(request: &RouteRequestContract) -> u64 {
    let mut hash = StableHash::new();
    hash.string("APGAR-BOARD-IR");
    hash.u32(request.schema_version);
    hash.i64(request.dbu_per_millimeter);
    hash.u64(request.board_revision);
    hash.string(&request.adapter_name);
    hash.string(&request.adapter_version);
    let mut layers: Vec<_> = request.layers.iter().collect();
    layers.sort_by_key(|layer| layer.routing_id);
    hash.u64(layers.len() as u64);
    for layer in layers {
        hash.entity(layer.reference);
        hash.u32(layer.routing_id);
        hash.string(&layer.name);
        hash.i32(layer.physical_order);
        hash.byte(0);
        hash.boolean(layer.routable);
    }
    let mut nets: Vec<_> = request.nets.iter().collect();
    nets.sort_by_key(|net| net.reference.id);
    hash.u64(nets.len() as u64);
    for net in nets {
        hash.entity(net.reference);
        hash.string(&net.name);
        hash.u64(net.terminals.len() as u64);
        for terminal in &net.terminals {
            hash.entity(*terminal);
        }
    }
    let mut terminals: Vec<_> = request.terminals.iter().collect();
    terminals.sort_by_key(|terminal| terminal.reference.id);
    hash.u64(terminals.len() as u64);
    for terminal in terminals {
        hash.entity(terminal.reference);
        hash.entity(terminal.net);
        hash.string(&terminal.component_path);
        hash.string(&terminal.pad);
        hash.point(terminal.center);
        hash.point(terminal.connection_region.min);
        hash.point(terminal.connection_region.max);
        hash.u64(terminal.layers.len() as u64);
        for layer in &terminal.layers {
            hash.u32(*layer);
        }
    }
    let mut obstacles: Vec<_> = request.obstacles.iter().collect();
    obstacles.sort_by_key(|obstacle| (obstacle.layer, obstacle.reference.id));
    hash.u64(obstacles.len() as u64);
    for obstacle in obstacles {
        hash.entity(obstacle.reference);
        hash.u32(obstacle.layer);
        hash.point(obstacle.bounds.min);
        hash.point(obstacle.bounds.max);
        hash.boolean(obstacle.owner_net.is_some());
        if let Some(owner) = obstacle.owner_net {
            hash.entity(owner);
        }
        hash.string(&obstacle.provenance);
    }
    hash.entity(request.routing_profile.net);
    hash.i64(request.routing_profile.nominal_width_dbu);
    hash.i64(request.routing_profile.clearance_dbu);
    hash.u64(request.routing_profile.allowed_layers.len() as u64);
    for layer in &request.routing_profile.allowed_layers {
        hash.u32(*layer);
    }
    hash.byte(heading_mask(&request.routing_profile.allowed_headings));
    hash.finish()
}

fn compiler_profile_fingerprint(request: &RouteRequestContract) -> u64 {
    let profile = &request.compiler_profile;
    let mut hash = StableHash::new();
    hash.string("APGAR-COMPILER-PROFILE-V1");
    hash.u32(profile.schema_version);
    hash.point(profile.lattice_origin);
    hash.i64(profile.lattice_step_dbu);
    hash.u32(profile.tile_width_nodes);
    hash.u32(profile.tile_height_nodes);
    hash.point(profile.compilation_roi.min);
    hash.point(profile.compilation_roi.max);
    hash.u64(profile.active_regions.len() as u64);
    for region in &profile.active_regions {
        hash.u32(region.layer);
        hash.point(region.bounds.min);
        hash.point(region.bounds.max);
    }
    hash.byte(heading_mask(&profile.allowed_headings));
    hash.u32(profile.costs.orthogonal_step);
    hash.u32(profile.costs.diagonal_step);
    hash.u32(profile.costs.bend);
    hash.finish()
}

fn routing_profile_fingerprint(request: &RouteRequestContract) -> u64 {
    let profile = &request.routing_profile;
    let mut hash = StableHash::new();
    hash.string("APGAR-ROUTING-PROFILE-V1");
    hash.entity(profile.net);
    hash.i64(profile.nominal_width_dbu);
    hash.i64(profile.clearance_dbu);
    hash.u64(profile.allowed_layers.len() as u64);
    for layer in &profile.allowed_layers {
        hash.u32(*layer);
    }
    hash.byte(heading_mask(&profile.allowed_headings));
    hash.finish()
}

fn rule_bucket_identity(request: &RouteRequestContract) -> u64 {
    let profile = &request.routing_profile;
    let mut hash = StableHash::new();
    hash.string("APGAR-M1-RULE-BUCKET-V1");
    hash.i64(profile.nominal_width_dbu);
    hash.i64(profile.clearance_dbu);
    hash.u64(profile.allowed_layers.len() as u64);
    for layer in &profile.allowed_layers {
        hash.u32(*layer);
    }
    hash.byte(heading_mask(&profile.allowed_headings));
    hash.finish()
}

pub(super) fn fingerprint_policy(policy: &CandidatePolicyContract) -> u64 {
    let mut hash = StableHash::new();
    hash.string("APGAR-CANDIDATE-POLICY-V1");
    encode_policy(&mut hash, policy);
    hash.finish()
}

pub(super) fn candidate_id(
    net: EntityRef,
    associations: &CandidateAssociations,
    policy_identity: u64,
    provenance: &CandidateProvenance,
) -> String {
    let half = |domain: &str| {
        let mut hash = StableHash::new();
        hash.string(domain);
        hash.entity(net);
        encode_associations(&mut hash, associations);
        hash.u64(policy_identity);
        encode_provenance(&mut hash, provenance);
        hash.finish()
    };
    let high = half("APGAR-CANDIDATE-ID-V1-A");
    let mut low = half("APGAR-CANDIDATE-ID-V1-B");
    if high == 0 && low == 0 {
        low = 1;
    }
    format!("{high:016x}{low:016x}")
}

pub(super) fn geometry_signature(geometry: &[LinePrimitive]) -> String {
    hash128(
        "APGAR-CANDIDATE-GEOMETRY-V1-A",
        "APGAR-CANDIDATE-GEOMETRY-V1-B",
        |hash| {
            hash.u64(geometry.len() as u64);
            for line in geometry {
                encode_line(hash, line);
            }
        },
    )
}

pub(super) fn resource_signature(resources: &[PhysicalEdgeSpan]) -> String {
    hash128(
        "APGAR-CANDIDATE-RESOURCES-V1-A",
        "APGAR-CANDIDATE-RESOURCES-V1-B",
        |hash| {
            hash.u64(resources.len() as u64);
            for resource in resources {
                encode_span(hash, resource);
            }
        },
    )
}

fn hash128<F>(first_domain: &str, second_domain: &str, encode: F) -> String
where
    F: Fn(&mut StableHash),
{
    let mut first = StableHash::new();
    first.string(first_domain);
    encode(&mut first);
    let mut second = StableHash::new();
    second.string(second_domain);
    encode(&mut second);
    format!("{:016x}{:016x}", first.finish(), second.finish())
}

pub(super) fn candidate_checksum_and_bytes(candidate: &AdmittedCandidate) -> (u64, u64) {
    let mut encoder = CandidateEncoder::new("APGAR-ROUTE-CANDIDATE-V1");
    encoder.u16(candidate.schema_major);
    encoder.u16(candidate.schema_minor);
    encoder.hash128(&candidate.id);
    encoder.entity(candidate.net);
    for terminal in candidate.intended_terminals {
        encoder.entity(terminal);
    }
    encoder.associations(&candidate.associations);
    encoder.u32(candidate.geometry_schema_version);
    encoder.u32(candidate.resource_schema_version);
    encoder.policy(&candidate.policy);
    encoder.u64(candidate.policy_identity);
    encoder.provenance(&candidate.provenance);
    encoder.u64(candidate.geometry.len() as u64);
    for line in &candidate.geometry {
        encoder.line(line);
    }
    encoder.u64(candidate.resources.len() as u64);
    for resource in &candidate.resources {
        encoder.span(resource);
    }
    encoder.metrics(&candidate.metrics);
    encoder.constraints(candidate);
    encoder.hash128(&candidate.geometry_signature);
    encoder.hash128(&candidate.resource_signature);
    (encoder.hash.finish(), encoder.bytes + 16)
}

fn policy_step_cost(
    request: &RouteRequestContract,
    policy: &CandidatePolicyContract,
    direction: u8,
    incoming: u8,
    resource: EdgeResourceContract,
) -> Result<u64, ContractDiagnostic> {
    if policy.banned_resources.binary_search(&resource).is_ok() {
        return Err(import_error(
            IMPORT_GEOMETRY,
            "outcome.candidate.geometry",
            "candidate traverses a request-banned resource",
        ));
    }
    let profile = &request.compiler_profile;
    let mut cost = if is_diagonal(direction) {
        u64::from(profile.costs.diagonal_step).checked_add(policy.diagonal_step_surcharge)
    } else {
        u64::from(profile.costs.orthogonal_step).checked_add(policy.orthogonal_step_surcharge)
    }
    .ok_or_else(|| {
        import_error(
            IMPORT_PAYLOAD,
            "outcome.candidate.metrics",
            "step cost overflows uint64",
        )
    })?;
    if incoming != NO_INCOMING_DIRECTION && incoming != direction {
        cost = cost
            .checked_add(u64::from(profile.costs.bend))
            .and_then(|value| value.checked_add(policy.bend_surcharge))
            .ok_or_else(|| {
                import_error(
                    IMPORT_PAYLOAD,
                    "outcome.candidate.metrics",
                    "bend cost overflows uint64",
                )
            })?;
    }
    if let Ok(index) = policy
        .resource_penalties
        .binary_search_by_key(&resource, |penalty| penalty.resource)
    {
        cost = cost
            .checked_add(policy.resource_penalties[index].additional_cost)
            .ok_or_else(|| {
                import_error(
                    IMPORT_PAYLOAD,
                    "outcome.candidate.metrics",
                    "penalty cost overflows uint64",
                )
            })?;
    }
    Ok(cost)
}

fn compress_resources(resources: &[EdgeResourceContract]) -> Vec<PhysicalEdgeSpan> {
    let mut spans: Vec<PhysicalEdgeSpan> = Vec::new();
    for resource in resources {
        if let Some(last) = spans.last_mut() {
            let delta = span_storage_delta(last.direction);
            let expected_x = last.lattice_x + delta.0 * i64::from(last.edge_count);
            let expected_y = last.lattice_y + delta.1 * i64::from(last.edge_count);
            if last.layer == resource.layer
                && last.direction == resource.direction
                && last.usage_units == 1
                && expected_x == resource.lattice_x
                && expected_y == resource.lattice_y
            {
                last.edge_count += 1;
                continue;
            }
        }
        spans.push(PhysicalEdgeSpan {
            layer: resource.layer,
            lattice_x: resource.lattice_x,
            lattice_y: resource.lattice_y,
            direction: resource.direction,
            edge_count: 1,
            usage_units: 1,
        });
    }
    spans
}

fn canonical_resource(layer: u32, x: i64, y: i64, direction: u8) -> EdgeResourceContract {
    match direction {
        0..=3 => EdgeResourceContract {
            layer,
            lattice_x: x,
            lattice_y: y,
            direction: edge_direction(direction),
        },
        4 => EdgeResourceContract {
            layer,
            lattice_x: x - 1,
            lattice_y: y,
            direction: EdgeDirection::East,
        },
        5 => EdgeResourceContract {
            layer,
            lattice_x: x - 1,
            lattice_y: y - 1,
            direction: EdgeDirection::NorthEast,
        },
        6 => EdgeResourceContract {
            layer,
            lattice_x: x,
            lattice_y: y - 1,
            direction: EdgeDirection::North,
        },
        7 => EdgeResourceContract {
            layer,
            lattice_x: x + 1,
            lattice_y: y - 1,
            direction: EdgeDirection::NorthWest,
        },
        _ => unreachable!("validated direction"),
    }
}

fn direction_for_delta(dx: i64, dy: i64) -> Option<u8> {
    if dx == 0 && dy == 0 {
        return None;
    }
    let abs_x = dx.unsigned_abs();
    let abs_y = dy.unsigned_abs();
    if dx != 0 && dy != 0 && abs_x != abs_y {
        return None;
    }
    match (dx.signum(), dy.signum()) {
        (1, 0) => Some(0),
        (1, 1) => Some(1),
        (0, 1) => Some(2),
        (-1, 1) => Some(3),
        (-1, 0) => Some(4),
        (-1, -1) => Some(5),
        (0, -1) => Some(6),
        (1, -1) => Some(7),
        _ => None,
    }
}

const fn direction_delta(direction: u8) -> (i64, i64) {
    match direction {
        0 => (1, 0),
        1 => (1, 1),
        2 => (0, 1),
        3 => (-1, 1),
        4 => (-1, 0),
        5 => (-1, -1),
        6 => (0, -1),
        7 => (1, -1),
        _ => (0, 0),
    }
}

const fn is_diagonal(direction: u8) -> bool {
    matches!(direction, 1 | 3 | 5 | 7)
}

const fn edge_direction(direction: u8) -> EdgeDirection {
    match direction {
        0 => EdgeDirection::East,
        1 => EdgeDirection::NorthEast,
        2 => EdgeDirection::North,
        3 => EdgeDirection::NorthWest,
        4 => EdgeDirection::West,
        5 => EdgeDirection::SouthWest,
        6 => EdgeDirection::South,
        7 => EdgeDirection::SouthEast,
        _ => EdgeDirection::East,
    }
}

const fn edge_direction_byte(direction: EdgeDirection) -> u8 {
    match direction {
        EdgeDirection::East => 0,
        EdgeDirection::NorthEast => 1,
        EdgeDirection::North => 2,
        EdgeDirection::NorthWest => 3,
        EdgeDirection::West => 4,
        EdgeDirection::SouthWest => 5,
        EdgeDirection::South => 6,
        EdgeDirection::SouthEast => 7,
    }
}

const fn span_storage_delta(direction: EdgeDirection) -> (i64, i64) {
    match direction {
        EdgeDirection::East => (1, 0),
        EdgeDirection::NorthEast => (1, 1),
        EdgeDirection::North => (0, 1),
        EdgeDirection::NorthWest => (1, -1),
        _ => (0, 0),
    }
}

fn lattice_index(point: PointDbu, origin: PointDbu, step: i64) -> (i64, i64) {
    ((point.x - origin.x) / step, (point.y - origin.y) / step)
}

fn point_on_lattice(point: PointDbu, origin: PointDbu, step: i64) -> bool {
    (point.x - origin.x) % step == 0 && (point.y - origin.y) % step == 0
}

fn point_to_nm(point: PointDbu, path: &str) -> Result<PointNm, ContractDiagnostic> {
    Ok(PointNm::new(
        dbu_to_nm(point.x, &format!("{path}.x"))?,
        dbu_to_nm(point.y, &format!("{path}.y"))?,
    ))
}

fn dbu_to_nm(value: i64, path: &str) -> Result<i64, ContractDiagnostic> {
    if value % 2 != 0 {
        return Err(import_error(
            IMPORT_CONVERSION,
            path,
            "APGAR DBU value is not exactly divisible by two nanometres",
        ));
    }
    Ok(value / 2)
}

const fn copper_layer(routing_id: u32) -> Option<CopperLayer> {
    match routing_id {
        0 => Some(CopperLayer::Front),
        31 => Some(CopperLayer::Back),
        _ => None,
    }
}

fn heading_mask(headings: &[Heading]) -> u8 {
    headings.iter().fold(0, |mask, heading| {
        mask | match heading {
            Heading::Horizontal => 1,
            Heading::Vertical => 2,
            Heading::Diagonal45 => 4,
        }
    })
}

fn encode_policy(hash: &mut StableHash, policy: &CandidatePolicyContract) {
    hash.u32(policy.schema_version);
    hash.byte(match policy.objective {
        CandidateObjective::BaseScalarCost => 0,
        CandidateObjective::LengthBiased => 1,
        CandidateObjective::BendBiased => 2,
        CandidateObjective::ResourceDiverse => 3,
    });
    hash.u64(policy.deterministic_seed);
    hash.u32(policy.candidate_ordinal);
    hash.u64(policy.orthogonal_step_surcharge);
    hash.u64(policy.diagonal_step_surcharge);
    hash.u64(policy.bend_surcharge);
    hash.u64(policy.banned_resources.len() as u64);
    for resource in &policy.banned_resources {
        encode_resource(hash, *resource);
    }
    hash.u64(policy.resource_penalties.len() as u64);
    for penalty in &policy.resource_penalties {
        encode_resource(hash, penalty.resource);
        hash.u64(penalty.additional_cost);
    }
}

fn encode_resource(hash: &mut StableHash, resource: EdgeResourceContract) {
    hash.u32(resource.layer);
    hash.i64(resource.lattice_x);
    hash.i64(resource.lattice_y);
    hash.byte(edge_direction_byte(resource.direction));
}

fn encode_associations(hash: &mut StableHash, value: &CandidateAssociations) {
    hash.u64(value.board_content_hash);
    hash.u64(value.compiler_profile_fingerprint);
    hash.u32(value.geometry_compiler_version);
    hash.u64(value.routing_profile_fingerprint);
    hash.u64(value.rule_bucket_identity);
}

fn encode_provenance(hash: &mut StableHash, value: &CandidateProvenance) {
    hash.byte(match value.generator {
        CandidateGeneratorKind::CpuAStar => 0,
        CandidateGeneratorKind::CudaFrontier => 1,
        CandidateGeneratorKind::CudaSweep => 2,
    });
    hash.u32(value.generator_version);
    hash.byte(match value.backend {
        CandidateBackendKind::Cpu => 0,
        CandidateBackendKind::Cuda => 1,
    });
    hash.string(&value.supported_device_class);
    hash.u64(value.deterministic_seed);
    hash.u64(value.batch_identity);
    hash.u64(value.query_identity);
    hash.u32(value.candidate_ordinal);
}

fn encode_line(hash: &mut StableHash, line: &LinePrimitive) {
    hash.byte(1);
    hash.u32(line.layer);
    hash.point(line.start);
    hash.point(line.end);
}

fn encode_span(hash: &mut StableHash, span: &PhysicalEdgeSpan) {
    hash.u32(span.layer);
    hash.i64(span.lattice_x);
    hash.i64(span.lattice_y);
    hash.byte(edge_direction_byte(span.direction));
    hash.u32(span.edge_count);
    hash.u32(span.usage_units);
}

struct StableHash {
    value: u64,
}

impl StableHash {
    const fn new() -> Self {
        Self {
            value: 14_695_981_039_346_656_037,
        }
    }

    fn byte(&mut self, value: u8) {
        self.value ^= u64::from(value);
        self.value = self.value.wrapping_mul(1_099_511_628_211);
    }

    fn boolean(&mut self, value: bool) {
        self.byte(u8::from(value));
    }

    fn u16(&mut self, value: u16) {
        for byte in value.to_le_bytes() {
            self.byte(byte);
        }
    }

    fn u32(&mut self, value: u32) {
        for byte in value.to_le_bytes() {
            self.byte(byte);
        }
    }

    fn i32(&mut self, value: i32) {
        self.u32(value as u32);
    }

    fn u64(&mut self, value: u64) {
        for byte in value.to_le_bytes() {
            self.byte(byte);
        }
    }

    fn i64(&mut self, value: i64) {
        self.u64(value as u64);
    }

    fn string(&mut self, value: &str) {
        self.u64(value.len() as u64);
        for byte in value.bytes() {
            self.byte(byte);
        }
    }

    fn entity(&mut self, value: EntityRef) {
        self.u64(value.id);
        self.u32(value.generation);
    }

    fn point(&mut self, value: PointDbu) {
        self.i64(value.x);
        self.i64(value.y);
    }

    const fn finish(&self) -> u64 {
        self.value
    }
}

struct CandidateEncoder {
    hash: StableHash,
    bytes: u64,
}

impl CandidateEncoder {
    fn new(domain: &str) -> Self {
        let mut hash = StableHash::new();
        hash.string(domain);
        Self { hash, bytes: 0 }
    }

    fn byte(&mut self, value: u8) {
        self.hash.byte(value);
        self.bytes += 1;
    }

    fn boolean(&mut self, value: bool) {
        self.byte(u8::from(value));
    }

    fn u16(&mut self, value: u16) {
        self.hash.u16(value);
        self.bytes += 2;
    }

    fn u32(&mut self, value: u32) {
        self.hash.u32(value);
        self.bytes += 4;
    }

    fn u64(&mut self, value: u64) {
        self.hash.u64(value);
        self.bytes += 8;
    }

    fn i64(&mut self, value: i64) {
        self.hash.i64(value);
        self.bytes += 8;
    }

    fn string(&mut self, value: &str) {
        self.hash.string(value);
        self.bytes += 8 + value.len() as u64;
    }

    fn entity(&mut self, value: EntityRef) {
        self.u64(value.id);
        self.u32(value.generation);
    }

    fn hash128(&mut self, value: &str) {
        let high = u64::from_str_radix(&value[..16], 16).expect("contract-validated hash128");
        let low = u64::from_str_radix(&value[16..], 16).expect("contract-validated hash128");
        self.u64(high);
        self.u64(low);
    }

    fn associations(&mut self, value: &CandidateAssociations) {
        self.u64(value.board_content_hash);
        self.u64(value.compiler_profile_fingerprint);
        self.u32(value.geometry_compiler_version);
        self.u64(value.routing_profile_fingerprint);
        self.u64(value.rule_bucket_identity);
    }

    fn policy(&mut self, value: &CandidatePolicyContract) {
        self.u32(value.schema_version);
        self.byte(match value.objective {
            CandidateObjective::BaseScalarCost => 0,
            CandidateObjective::LengthBiased => 1,
            CandidateObjective::BendBiased => 2,
            CandidateObjective::ResourceDiverse => 3,
        });
        self.u64(value.deterministic_seed);
        self.u32(value.candidate_ordinal);
        self.u64(value.orthogonal_step_surcharge);
        self.u64(value.diagonal_step_surcharge);
        self.u64(value.bend_surcharge);
        self.u64(value.banned_resources.len() as u64);
        for resource in &value.banned_resources {
            self.resource(*resource);
        }
        self.u64(value.resource_penalties.len() as u64);
        for penalty in &value.resource_penalties {
            self.resource(penalty.resource);
            self.u64(penalty.additional_cost);
        }
    }

    fn resource(&mut self, value: EdgeResourceContract) {
        self.u32(value.layer);
        self.i64(value.lattice_x);
        self.i64(value.lattice_y);
        self.byte(edge_direction_byte(value.direction));
    }

    fn provenance(&mut self, value: &CandidateProvenance) {
        self.byte(match value.generator {
            CandidateGeneratorKind::CpuAStar => 0,
            CandidateGeneratorKind::CudaFrontier => 1,
            CandidateGeneratorKind::CudaSweep => 2,
        });
        self.u32(value.generator_version);
        self.byte(match value.backend {
            CandidateBackendKind::Cpu => 0,
            CandidateBackendKind::Cuda => 1,
        });
        self.string(&value.supported_device_class);
        self.u64(value.deterministic_seed);
        self.u64(value.batch_identity);
        self.u64(value.query_identity);
        self.u32(value.candidate_ordinal);
    }

    fn line(&mut self, value: &LinePrimitive) {
        self.byte(1);
        self.u32(value.layer);
        self.i64(value.start.x);
        self.i64(value.start.y);
        self.i64(value.end.x);
        self.i64(value.end.y);
    }

    fn span(&mut self, value: &PhysicalEdgeSpan) {
        self.u32(value.layer);
        self.i64(value.lattice_x);
        self.i64(value.lattice_y);
        self.byte(edge_direction_byte(value.direction));
        self.u32(value.edge_count);
        self.u32(value.usage_units);
    }

    fn metrics(&mut self, value: &CandidateMetrics) {
        self.u64(value.scalar_policy_cost);
        self.u64(value.intrinsic_base_cost);
        self.u64(value.orthogonal_step_count);
        self.u64(value.diagonal_step_count);
        self.u64(value.bend_count);
        self.u64(value.line_primitive_count);
        self.u64(value.via_count);
        self.u64(value.axis_aligned_length_dbu);
        self.u64(value.diagonal_projection_dbu);
    }

    fn constraints(&mut self, candidate: &AdmittedCandidate) {
        self.boolean(candidate.constraints.supported_hard_constraints_satisfied);
        self.boolean(candidate.constraints.unsupported_rules_remain);
        self.u32(candidate.constraints.connected_intended_terminal_count);
        self.byte(match candidate.constraints.exact_validation_status {
            ExactValidationStatus::Passed => 0,
            ExactValidationStatus::UnsupportedGeometry => 1,
            ExactValidationStatus::InvalidGeometry => 2,
            ExactValidationStatus::ExactRuleViolation => 3,
        });
    }
}

fn first_design_diagnostic(diagnostics: Vec<Diagnostic>) -> ContractDiagnostic {
    diagnostics
        .into_iter()
        .next()
        .map(|diagnostic| {
            import_error(
                IMPORT_DESIGN,
                diagnostic.path,
                format!("{}: {}", diagnostic.code, diagnostic.message),
            )
        })
        .unwrap_or_else(|| {
            import_error(
                IMPORT_DESIGN,
                "design",
                "Design IR validation failed without a diagnostic",
            )
        })
}

fn internal_import_error(request: &RouteRequestContract, message: &str) -> ContractDiagnostic {
    import_error(IMPORT_DESIGN, &request.request_path, message)
}

fn import_error(
    code: &str,
    path: impl Into<String>,
    message: impl Into<String>,
) -> ContractDiagnostic {
    ContractDiagnostic {
        code: code.to_owned(),
        path: path.into(),
        message: message.into(),
    }
}

#[cfg(test)]
mod tests {
    use crate::demo;
    use crate::design::{CopperLayer, RoutingRequest};

    use super::super::contract::{
        AdmittedCandidate, CONTRACT_SCHEMA_VERSION, CandidateBackendKind, CandidateGeneratorKind,
        CandidateMetrics, CandidateProvenance, ConstraintAssessment, ExactValidationStatus,
        LinePrimitive, RESULT_SCHEMA_NAME, ReplayIdentity, RouteOutcome, RouteResultContract,
        render_result,
    };
    use super::super::lower::{RouteInputBundle, lower_request};
    use super::{
        IMPORT_ASSOCIATION, IMPORT_CONVERSION, IMPORT_PAYLOAD, IMPORT_REQUEST, IMPORT_TOOL,
        candidate_checksum_and_bytes, candidate_id, derive_candidate_fields, expected_associations,
        expected_cpu_tool, fingerprint_policy, geometry_signature, import_result,
        resource_signature,
    };

    const MM: i64 = 1_000_000;
    const EXECUTABLE_SHA: &str = "0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef";

    fn routing_design() -> crate::design::Design {
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
        design
    }

    fn bundle() -> RouteInputBundle {
        lower_request(&routing_design()).unwrap().unwrap()
    }

    fn completed_candidate(bundle: &RouteInputBundle) -> AdmittedCandidate {
        let request = &bundle.request;
        let target = request
            .nets
            .iter()
            .find(|net| net.reference == request.routing_profile.net)
            .unwrap();
        let canonical_start = request
            .terminals
            .iter()
            .find(|terminal| terminal.reference == target.terminals[0])
            .unwrap()
            .center;
        let canonical_goal = request
            .terminals
            .iter()
            .find(|terminal| terminal.reference == target.terminals[1])
            .unwrap()
            .center;
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
                supported_device_class: super::APGAR_CPU_DEVICE_CLASS.to_owned(),
                deterministic_seed: request.planar_route.candidate_policy.deterministic_seed,
                batch_identity: request.planar_route.scheduling.batch_identity,
                query_identity: request.planar_route.scheduling.query_identity,
                candidate_ordinal: request.planar_route.candidate_policy.candidate_ordinal,
            },
            geometry: vec![LinePrimitive {
                layer: request.routing_profile.allowed_layers[0],
                start: canonical_start,
                end: canonical_goal,
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
        candidate
    }

    fn result(
        bundle: &RouteInputBundle,
        candidates: Vec<AdmittedCandidate>,
    ) -> RouteResultContract {
        let selected_candidate_id = candidates[0].id.clone();
        RouteResultContract {
            schema_name: RESULT_SCHEMA_NAME.to_owned(),
            schema_version: CONTRACT_SCHEMA_VERSION,
            request_sha256: bundle.request_sha256.clone(),
            request_path: bundle.request.request_path.clone(),
            tool: expected_cpu_tool(EXECUTABLE_SHA.to_owned()),
            replay: ReplayIdentity {
                design_fingerprint_sha256: bundle.request.design_fingerprint_sha256.clone(),
                request_identity_sha256: bundle.request.request_identity_sha256.clone(),
                board_revision: bundle.request.board_revision,
                deterministic_seed: bundle
                    .request
                    .planar_route
                    .candidate_policy
                    .deterministic_seed,
                batch_identity: bundle.request.planar_route.scheduling.batch_identity,
                query_identity: bundle.request.planar_route.scheduling.query_identity,
            },
            outcome: RouteOutcome::Completed {
                selected_candidate_id,
                candidates,
            },
        }
    }

    fn render(value: &RouteResultContract) -> String {
        render_result(value).unwrap()
    }

    #[test]
    fn exact_candidate_authenticates_and_imports_fresh_design() {
        let design = routing_design();
        let bundle = lower_request(&design).unwrap().unwrap();
        let candidate = completed_candidate(&bundle);
        let imported = import_result(
            &design,
            &bundle,
            &render(&result(&bundle, vec![candidate.clone()])),
            &expected_cpu_tool(EXECUTABLE_SHA.to_owned()),
        )
        .unwrap();
        assert!(imported.design.board.routing_requests.is_empty());
        assert_eq!(imported.design.board.routes.len(), 1);
        assert_eq!(
            imported.design.board.routes[0].path,
            "board.autoroute.vout.segment.00000000"
        );
        assert_eq!(imported.design.board.routes[0].start.x, 16 * MM);
        assert_eq!(imported.design.board.routes[0].end.x, 24 * MM);
        assert_eq!(imported.selected_candidate_id, candidate.id);
        assert!(imported.result_json.ends_with('\n'));
    }

    #[test]
    fn selection_is_explicit_and_independent_of_candidate_order() {
        let design = routing_design();
        let bundle = lower_request(&design).unwrap().unwrap();
        let selected = completed_candidate(&bundle);
        let mut alternate = selected.clone();
        alternate.id = "ffffffffffffffffffffffffffffffff".to_owned();
        let mut result = result(&bundle, vec![selected.clone(), alternate.clone()]);
        let RouteOutcome::Completed {
            selected_candidate_id,
            candidates,
        } = &mut result.outcome
        else {
            unreachable!()
        };
        *selected_candidate_id = selected.id.clone();
        candidates.swap(0, 1);
        let imported = import_result(
            &design,
            &bundle,
            &render(&result),
            &expected_cpu_tool(EXECUTABLE_SHA.to_owned()),
        )
        .unwrap();
        assert_eq!(imported.selected_candidate_id, selected.id);
    }

    #[test]
    fn stale_request_and_wrong_tool_fail_before_import() {
        let design = routing_design();
        let bundle = lower_request(&design).unwrap().unwrap();
        let candidate = completed_candidate(&bundle);
        let mut stale = result(&bundle, vec![candidate.clone()]);
        stale.request_sha256 =
            "ffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffff".to_owned();
        assert_eq!(
            import_result(
                &design,
                &bundle,
                &render(&stale),
                &expected_cpu_tool(EXECUTABLE_SHA.to_owned()),
            )
            .unwrap_err()
            .code,
            IMPORT_REQUEST
        );

        let valid = render(&result(&bundle, vec![candidate]));
        let wrong_tool = expected_cpu_tool(
            "ffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffff".to_owned(),
        );
        assert_eq!(
            import_result(&design, &bundle, &valid, &wrong_tool)
                .unwrap_err()
                .code,
            IMPORT_TOOL
        );
    }

    #[test]
    fn candidate_association_payload_and_odd_dbu_mutations_fail_closed() {
        let design = routing_design();
        let bundle = lower_request(&design).unwrap().unwrap();

        let mut association = completed_candidate(&bundle);
        association.associations.board_content_hash ^= 1;
        let association_result = render(&result(&bundle, vec![association]));
        assert_eq!(
            import_result(
                &design,
                &bundle,
                &association_result,
                &expected_cpu_tool(EXECUTABLE_SHA.to_owned()),
            )
            .unwrap_err()
            .code,
            IMPORT_ASSOCIATION
        );

        let mut payload = completed_candidate(&bundle);
        payload.geometry_signature = "ffffffffffffffffffffffffffffffff".to_owned();
        let payload_result = render(&result(&bundle, vec![payload]));
        assert_eq!(
            import_result(
                &design,
                &bundle,
                &payload_result,
                &expected_cpu_tool(EXECUTABLE_SHA.to_owned()),
            )
            .unwrap_err()
            .code,
            IMPORT_PAYLOAD
        );

        let mut odd = completed_candidate(&bundle);
        let midpoint = super::PointDbu {
            x: 40 * MM + 1,
            y: 20 * MM,
        };
        odd.geometry = vec![
            LinePrimitive {
                layer: odd.geometry[0].layer,
                start: odd.geometry[0].start,
                end: midpoint,
                width_dbu: odd.geometry[0].width_dbu,
            },
            LinePrimitive {
                layer: odd.geometry[0].layer,
                start: midpoint,
                end: odd.geometry[0].end,
                width_dbu: odd.geometry[0].width_dbu,
            },
        ];
        odd.metrics.line_primitive_count = 2;
        let odd_result = render(&result(&bundle, vec![odd]));
        assert_eq!(
            import_result(
                &design,
                &bundle,
                &odd_result,
                &expected_cpu_tool(EXECUTABLE_SHA.to_owned()),
            )
            .unwrap_err()
            .code,
            IMPORT_CONVERSION
        );
    }

    #[test]
    fn geometry_exceeding_authenticated_edge_bound_fails_before_expansion() {
        let mut bundle = bundle();
        let mut candidate = completed_candidate(&bundle);
        bundle.request.compiler_profile.lattice_step_dbu = 1;
        bundle.request.resource_limits.expanded_resource_edges = 1;
        bundle.request.validate().unwrap();

        let start = candidate.geometry[0].start;
        let goal = candidate.geometry[0].end;
        let detour_y = if goal.y != start.y + 2 {
            start.y + 2
        } else {
            start.y + 4
        };
        let far_start = super::PointDbu {
            x: 1_000_000_000_000,
            y: start.y,
        };
        let far_end = super::PointDbu {
            x: far_start.x,
            y: detour_y,
        };
        let layer = candidate.geometry[0].layer;
        let width_dbu = candidate.geometry[0].width_dbu;
        candidate.geometry = vec![
            LinePrimitive {
                layer,
                start,
                end: far_start,
                width_dbu,
            },
            LinePrimitive {
                layer,
                start: far_start,
                end: far_end,
                width_dbu,
            },
            LinePrimitive {
                layer,
                start: far_end,
                end: super::PointDbu {
                    x: goal.x,
                    y: detour_y,
                },
                width_dbu,
            },
            LinePrimitive {
                layer,
                start: super::PointDbu {
                    x: goal.x,
                    y: detour_y,
                },
                end: goal,
                width_dbu,
            },
        ];
        let error = match derive_candidate_fields(&bundle.request, &candidate) {
            Err(error) => error,
            Ok(_) => panic!("over-bound candidate expansion unexpectedly succeeded"),
        };
        assert_eq!(error.code, super::IMPORT_GEOMETRY);
        assert_eq!(error.path, "outcome.candidate.resources");
    }

    #[test]
    fn current_design_mutation_invalidates_stored_bundle() {
        let bundle = bundle();
        let candidate = completed_candidate(&bundle);
        let result_json = render(&result(&bundle, vec![candidate]));
        let mut changed = routing_design();
        changed.board.routing_requests[0].clearance_nm += 1;
        assert_eq!(
            import_result(
                &changed,
                &bundle,
                &result_json,
                &expected_cpu_tool(EXECUTABLE_SHA.to_owned()),
            )
            .unwrap_err()
            .code,
            IMPORT_REQUEST
        );
    }
}
