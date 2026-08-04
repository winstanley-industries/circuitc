use std::collections::BTreeMap;

use serde::Serialize;
use sha2::{Digest, Sha256};

use crate::compile::RelativeArtifactPath;
use crate::design::{CopperLayer, Design, Diagnostic, Placement, PointNm, RoutingRequest, SizeNm};

use super::contract::{
    APGAR_DBU_PER_MILLIMETER, ActiveRegionContract, BoxDbu, CONTRACT_SCHEMA_VERSION,
    CandidateObjective, CandidatePolicyContract, CompilerProfileContract,
    DeterministicCostsContract, EntityRef, Heading, LayerContract, LayerSide, MAX_CONTRACT_BYTES,
    NetContract, ObstacleContract, PlanarRouteContract, PointDbu, REQUEST_SCHEMA_NAME,
    ResourceLimitsContract, RouteRequestContract, RoutingProfileContract,
    SchedulingIdentityContract, TerminalContract, UnsupportedHostRuleContract, render_request,
    sha256_hex,
};
use super::{APGAR_CONTRACT_IDENTITY, PINNED_APGAR_SOURCE_REVISION};

const LOWER_CONVERSION: &str = "CC-ROUTE-LOWER-001";
const LOWER_IDENTITY: &str = "CC-ROUTE-LOWER-002";
const LOWER_INTERNAL: &str = "CC-ROUTE-LOWER-003";
const MAX_ABS_DBU_COORDINATE: i64 = 1_000_000_000_000;

const FRONT_ROUTING_ID: u32 = 0;
const BACK_ROUTING_ID: u32 = 31;
const TILE_DIMENSION_NODES: u32 = 32;
const ORTHOGONAL_STEP_COST: u32 = 1_000;
const DIAGONAL_STEP_COST: u32 = 1_414;
const BEND_COST: u32 = 100;

const ROUTE_TIMEOUT_MILLISECONDS: u64 = 30_000;
const ROUTE_STDERR_BYTES: u64 = 1_048_576;
const ROUTE_DIAGNOSTIC_BYTES: u64 = 65_536;
const ROUTE_CANDIDATE_PRIMITIVES: u64 = 10_000;
const ROUTE_EXPANDED_RESOURCE_EDGES: u64 = 1_000_000;

const HEADINGS: [Heading; 3] = [Heading::Horizontal, Heading::Vertical, Heading::Diagonal45];

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct RouteInputBundle {
    pub(crate) request_path: RelativeArtifactPath,
    pub(crate) request: RouteRequestContract,
    pub(crate) request_json: String,
    pub(crate) request_sha256: String,
}

pub(crate) fn lower_request(design: &Design) -> Result<Option<RouteInputBundle>, Vec<Diagnostic>> {
    design.validate()?;
    let Some(request) = design.board.routing_requests.first() else {
        return Ok(None);
    };
    lower_one(design, request)
        .map(Some)
        .map_err(|error| vec![error])
}

fn lower_one(design: &Design, request: &RoutingRequest) -> Result<RouteInputBundle, Diagnostic> {
    let request_diagnostic_path = request_path(request);
    let mut identities = IdentityAllocator::new(stable_u64);
    let layers = lower_layers(design, &mut identities, &request_diagnostic_path)?;

    let mut net_refs = BTreeMap::new();
    for net in sorted_nets(design) {
        let reference = identities.allocate(
            &design.name,
            "net",
            &[net.name.as_str()],
            format!("design.nets.{}", net.name),
        )?;
        net_refs.insert(net.name.as_str(), reference);
    }
    let routed_net = net_refs.get(request.net.as_str()).copied().ok_or_else(|| {
        lower_diagnostic(
            LOWER_INTERNAL,
            &request_diagnostic_path,
            format!(
                "validated routing request references missing canonical net {}",
                request.net
            ),
        )
    })?;

    let mut terminals = Vec::new();
    let mut target_terminals = Vec::new();
    let mut obstacles = Vec::new();
    let mut terminals_by_net: BTreeMap<&str, Vec<EntityRef>> = BTreeMap::new();

    let mut components: Vec<_> = design
        .components
        .iter()
        .filter(|component| component.physical.is_some())
        .collect();
    components
        .sort_by(|left, right| (&left.path, &left.reference).cmp(&(&right.path, &right.reference)));
    for component in components {
        let Some(physical) = component.physical.as_ref() else {
            continue;
        };
        let mut pads: Vec<_> = physical.footprint.pads.iter().collect();
        pads.sort_by(|left, right| left.number.cmp(&right.number));
        for pad in pads {
            let pad_path = format!("{}.footprint.pad.{}", component.path, pad.number);
            let center_nm = physical.placement.transform(pad.offset).ok_or_else(|| {
                lower_diagnostic(
                    LOWER_CONVERSION,
                    &pad_path,
                    "validated pad placement could not be transformed with checked arithmetic",
                )
            })?;
            let center = point_to_dbu(center_nm, &format!("{pad_path}.center"))?;
            let bounds = pad_bounds(
                physical.placement,
                center,
                pad.size,
                &format!("{pad_path}.bounds"),
            )?;
            let layer = layer_id(physical.placement.layer);
            let owner_net = match component.net_for_pad(&pad.number) {
                Some(name) => Some(net_refs.get(name).copied().ok_or_else(|| {
                    lower_diagnostic(
                        LOWER_INTERNAL,
                        &pad_path,
                        format!("validated pad references missing canonical net {name}"),
                    )
                })?),
                None => None,
            };

            if let Some(net) = owner_net {
                let reference = identities.allocate(
                    &design.name,
                    "terminal",
                    &[component.path.as_str(), pad.number.as_str()],
                    pad_path.clone(),
                )?;
                let terminal = TerminalContract {
                    reference,
                    net,
                    component_path: component.path.clone(),
                    pad: pad.number.clone(),
                    center,
                    connection_region: bounds,
                    layers: vec![layer],
                };
                terminals_by_net
                    .entry(component.net_for_pad(&pad.number).ok_or_else(|| {
                        lower_diagnostic(
                            LOWER_INTERNAL,
                            &pad_path,
                            "connected terminal lost its canonical net association",
                        )
                    })?)
                    .or_default()
                    .push(reference);
                if net == routed_net {
                    target_terminals.push((
                        component.path.as_str(),
                        pad.number.as_str(),
                        terminal.clone(),
                    ));
                }
                terminals.push(terminal);
            }

            obstacles.push(ObstacleContract {
                reference: identities.allocate(
                    &design.name,
                    "pad-obstacle",
                    &[component.path.as_str(), pad.number.as_str()],
                    format!("{pad_path}.obstacle"),
                )?,
                layer,
                bounds,
                owner_net,
                provenance: pad_path,
            });
        }
    }

    let mut routes: Vec<_> = design.board.routes.iter().collect();
    routes.sort_by(|left, right| left.path.cmp(&right.path));
    for route in routes {
        let route_path = format!("design.board.routes.{}", route.path);
        let owner_net = net_refs.get(route.net.as_str()).copied().ok_or_else(|| {
            lower_diagnostic(
                LOWER_INTERNAL,
                &route_path,
                format!(
                    "validated route references missing canonical net {}",
                    route.net
                ),
            )
        })?;
        obstacles.push(ObstacleContract {
            reference: identities.allocate(
                &design.name,
                "route-obstacle",
                &[route.path.as_str()],
                format!("{route_path}.obstacle"),
            )?,
            layer: layer_id(route.layer),
            bounds: route_bounds(route.start, route.end, route.width_nm, &route_path)?,
            owner_net: Some(owner_net),
            provenance: route.path.clone(),
        });
    }

    terminals.sort_by_key(|terminal| terminal.reference);
    obstacles.sort_by_key(|obstacle| obstacle.reference);
    target_terminals.sort_by(|left, right| (left.0, left.1).cmp(&(right.0, right.1)));
    let [start_terminal, goal_terminal] = target_terminals.as_slice() else {
        return Err(lower_diagnostic(
            LOWER_INTERNAL,
            &request_diagnostic_path,
            format!(
                "validated two-terminal request lowered {} target terminals instead of two",
                target_terminals.len()
            ),
        ));
    };

    let mut nets = Vec::with_capacity(net_refs.len());
    for net in sorted_nets(design) {
        let reference = net_refs.get(net.name.as_str()).copied().ok_or_else(|| {
            lower_diagnostic(
                LOWER_INTERNAL,
                format!("design.nets.{}", net.name),
                "canonical net identity disappeared during lowering",
            )
        })?;
        let mut net_terminals = terminals_by_net
            .remove(net.name.as_str())
            .unwrap_or_default();
        net_terminals.sort();
        nets.push(NetContract {
            reference,
            name: net.name.clone(),
            terminals: net_terminals,
        });
    }
    nets.sort_by_key(|net| net.reference);

    let board_bounds = board_bounds(design, &request_diagnostic_path)?;
    let selected_layer = layer_id(request.layer);
    let nominal_width_dbu = size_to_dbu(
        request.width_nm,
        &format!("{request_diagnostic_path}.width_nm"),
    )?;
    let clearance_dbu = size_to_dbu(
        request.clearance_nm,
        &format!("{request_diagnostic_path}.clearance_nm"),
    )?;
    let lattice_step_dbu = size_to_dbu(
        request.grid_step_nm,
        &format!("{request_diagnostic_path}.grid_step_nm"),
    )?;
    let routing_profile = RoutingProfileContract {
        net: routed_net,
        nominal_width_dbu,
        clearance_dbu,
        allowed_layers: vec![selected_layer],
        allowed_headings: HEADINGS.to_vec(),
    };
    let compiler_profile = CompilerProfileContract {
        schema_version: 1,
        lattice_origin: board_bounds.min,
        lattice_step_dbu,
        tile_width_nodes: TILE_DIMENSION_NODES,
        tile_height_nodes: TILE_DIMENSION_NODES,
        compilation_roi: board_bounds,
        active_regions: vec![ActiveRegionContract {
            layer: selected_layer,
            bounds: board_bounds,
        }],
        allowed_headings: HEADINGS.to_vec(),
        costs: DeterministicCostsContract {
            orthogonal_step: ORTHOGONAL_STEP_COST,
            diagonal_step: DIAGONAL_STEP_COST,
            bend: BEND_COST,
        },
    };
    let unsupported_host_rules = unsupported_host_rules(request);

    let design_fingerprint_sha256 = lowered_design_fingerprint(
        design,
        request,
        &layers,
        &nets,
        &terminals,
        &obstacles,
        &routing_profile,
        &compiler_profile,
        &unsupported_host_rules,
    )?;
    let request_identity_sha256 = request_identity(design, request, &design_fingerprint_sha256);
    let artifact_path =
        RelativeArtifactPath::try_new(format!("routing/{request_identity_sha256}/request.json"))
            .map_err(|error| {
                lower_diagnostic(
                    LOWER_INTERNAL,
                    &request_diagnostic_path,
                    format!("could not derive routing request artifact path: {error}"),
                )
            })?;
    let board_revision = stable_nonzero_u64(
        "board-revision",
        &[design.name.as_str(), design_fingerprint_sha256.as_str()],
        &request_diagnostic_path,
    )?;
    let batch_identity = stable_nonzero_u64(
        "batch",
        &[request_identity_sha256.as_str()],
        &request_diagnostic_path,
    )?;
    let query_identity = stable_nonzero_u64(
        "query",
        &[request_identity_sha256.as_str(), request.net.as_str()],
        &request_diagnostic_path,
    )?;

    let lowered = RouteRequestContract {
        schema_name: REQUEST_SCHEMA_NAME.to_owned(),
        schema_version: CONTRACT_SCHEMA_VERSION,
        design_name: design.name.clone(),
        design_fingerprint_sha256,
        request_path: request.path.clone(),
        request_identity_sha256,
        expected_apgar_source_revision: PINNED_APGAR_SOURCE_REVISION.to_owned(),
        expected_apgar_contract_identity: APGAR_CONTRACT_IDENTITY.to_owned(),
        dbu_per_millimeter: APGAR_DBU_PER_MILLIMETER,
        board_revision,
        adapter_name: "circuitc".to_owned(),
        adapter_version: "apgar-route-request-v1".to_owned(),
        layers,
        nets,
        terminals,
        obstacles,
        routing_profile,
        compiler_profile,
        planar_route: PlanarRouteContract {
            net: routed_net,
            start: start_terminal.2.center,
            goal: goal_terminal.2.center,
            start_layer: selected_layer,
            goal_layer: selected_layer,
            candidate_policy: CandidatePolicyContract {
                schema_version: 1,
                objective: CandidateObjective::BaseScalarCost,
                deterministic_seed: 0,
                candidate_ordinal: 0,
                orthogonal_step_surcharge: 0,
                diagonal_step_surcharge: 0,
                bend_surcharge: 0,
                banned_resources: Vec::new(),
                resource_penalties: Vec::new(),
            },
            scheduling: SchedulingIdentityContract {
                batch_identity,
                query_identity,
            },
        },
        resource_limits: ResourceLimitsContract {
            timeout_milliseconds: ROUTE_TIMEOUT_MILLISECONDS,
            stdout_bytes: MAX_CONTRACT_BYTES as u64,
            stderr_bytes: ROUTE_STDERR_BYTES,
            diagnostic_bytes: ROUTE_DIAGNOSTIC_BYTES,
            candidate_primitives: ROUTE_CANDIDATE_PRIMITIVES,
            expanded_resource_edges: ROUTE_EXPANDED_RESOURCE_EDGES,
        },
        unsupported_host_rules,
    };
    let request_json = render_request(&lowered).map_err(|error| {
        lower_diagnostic(
            LOWER_INTERNAL,
            &request_diagnostic_path,
            format!("lowered APGAR request violates its canonical contract: {error}"),
        )
    })?;
    let request_sha256 = sha256_hex(request_json.as_bytes());
    Ok(RouteInputBundle {
        request_path: artifact_path,
        request: lowered,
        request_json,
        request_sha256,
    })
}

fn lower_layers(
    design: &Design,
    identities: &mut IdentityAllocator,
    path: &str,
) -> Result<Vec<LayerContract>, Diagnostic> {
    Ok(vec![
        LayerContract {
            reference: identities.allocate(
                &design.name,
                "layer",
                &["front"],
                format!("{path}.layers.front"),
            )?,
            routing_id: FRONT_ROUTING_ID,
            name: "F.Cu".to_owned(),
            physical_order: 0,
            side: LayerSide::Front,
            routable: true,
        },
        LayerContract {
            reference: identities.allocate(
                &design.name,
                "layer",
                &["back"],
                format!("{path}.layers.back"),
            )?,
            routing_id: BACK_ROUTING_ID,
            name: "B.Cu".to_owned(),
            physical_order: 1,
            side: LayerSide::Back,
            routable: true,
        },
    ])
}

fn sorted_nets(design: &Design) -> Vec<&crate::design::Net> {
    let mut nets: Vec<_> = design.nets.iter().collect();
    nets.sort_by(|left, right| {
        (left.is_ground, left.name.as_str()).cmp(&(right.is_ground, right.name.as_str()))
    });
    nets
}

fn board_bounds(design: &Design, path: &str) -> Result<BoxDbu, Diagnostic> {
    let outline = design.board.outline;
    let max = PointNm::new(
        outline
            .origin
            .x
            .checked_add(outline.size.width)
            .ok_or_else(|| {
                lower_diagnostic(
                    LOWER_CONVERSION,
                    path,
                    "board outline x extent overflows exact nanometre arithmetic",
                )
            })?,
        outline
            .origin
            .y
            .checked_add(outline.size.height)
            .ok_or_else(|| {
                lower_diagnostic(
                    LOWER_CONVERSION,
                    path,
                    "board outline y extent overflows exact nanometre arithmetic",
                )
            })?,
    );
    Ok(BoxDbu {
        min: point_to_dbu(outline.origin, &format!("{path}.board_outline.min"))?,
        max: point_to_dbu(max, &format!("{path}.board_outline.max"))?,
    })
}

fn pad_bounds(
    placement: Placement,
    center: PointDbu,
    size: SizeNm,
    path: &str,
) -> Result<BoxDbu, Diagnostic> {
    let normalized = placement.rotation_degrees.rem_euclid(360);
    let (width_nm, height_nm) = match normalized {
        0 | 180 => (size.width, size.height),
        90 | 270 => (size.height, size.width),
        _ => {
            return Err(lower_diagnostic(
                LOWER_INTERNAL,
                path,
                "validated pad placement has a non-orthogonal rotation",
            ));
        }
    };
    // One nanometre is two APGAR DBU, so half a pad dimension in DBU is
    // exactly the original full dimension in nanometres.
    Ok(BoxDbu {
        min: PointDbu {
            x: checked_dbu_coordinate(center.x.checked_sub(width_nm), &format!("{path}.min.x"))?,
            y: checked_dbu_coordinate(center.y.checked_sub(height_nm), &format!("{path}.min.y"))?,
        },
        max: PointDbu {
            x: checked_dbu_coordinate(center.x.checked_add(width_nm), &format!("{path}.max.x"))?,
            y: checked_dbu_coordinate(center.y.checked_add(height_nm), &format!("{path}.max.y"))?,
        },
    })
}

fn route_bounds(
    start_nm: PointNm,
    end_nm: PointNm,
    width_nm: i64,
    path: &str,
) -> Result<BoxDbu, Diagnostic> {
    let start = point_to_dbu(start_nm, &format!("{path}.start"))?;
    let end = point_to_dbu(end_nm, &format!("{path}.end"))?;
    Ok(BoxDbu {
        min: PointDbu {
            x: checked_dbu_coordinate(
                start.x.min(end.x).checked_sub(width_nm),
                &format!("{path}.bounds.min.x"),
            )?,
            y: checked_dbu_coordinate(
                start.y.min(end.y).checked_sub(width_nm),
                &format!("{path}.bounds.min.y"),
            )?,
        },
        max: PointDbu {
            x: checked_dbu_coordinate(
                start.x.max(end.x).checked_add(width_nm),
                &format!("{path}.bounds.max.x"),
            )?,
            y: checked_dbu_coordinate(
                start.y.max(end.y).checked_add(width_nm),
                &format!("{path}.bounds.max.y"),
            )?,
        },
    })
}

fn point_to_dbu(point: PointNm, path: &str) -> Result<PointDbu, Diagnostic> {
    Ok(PointDbu {
        x: nm_to_dbu(point.x, &format!("{path}.x"))?,
        y: nm_to_dbu(point.y, &format!("{path}.y"))?,
    })
}

fn size_to_dbu(value: i64, path: &str) -> Result<i64, Diagnostic> {
    let lowered = nm_to_dbu(value, path)?;
    if lowered <= 0 {
        return Err(lower_diagnostic(
            LOWER_CONVERSION,
            path,
            "APGAR exact size conversion requires a positive value",
        ));
    }
    Ok(lowered)
}

fn nm_to_dbu(value: i64, path: &str) -> Result<i64, Diagnostic> {
    checked_dbu_coordinate(value.checked_mul(2), path)
}

fn checked_dbu_coordinate(value: Option<i64>, path: &str) -> Result<i64, Diagnostic> {
    let value = value.ok_or_else(|| {
        lower_diagnostic(
            LOWER_CONVERSION,
            path,
            "nanometre to APGAR DBU conversion overflowed checked i64 arithmetic",
        )
    })?;
    if value.unsigned_abs() > MAX_ABS_DBU_COORDINATE as u64 {
        return Err(lower_diagnostic(
            LOWER_CONVERSION,
            path,
            format!(
                "lowered coordinate {value} exceeds APGAR's +/-{MAX_ABS_DBU_COORDINATE} DBU envelope"
            ),
        ));
    }
    Ok(value)
}

fn layer_id(layer: CopperLayer) -> u32 {
    match layer {
        CopperLayer::Front => FRONT_ROUTING_ID,
        CopperLayer::Back => BACK_ROUTING_ID,
    }
}

fn unsupported_host_rules(request: &RoutingRequest) -> Vec<UnsupportedHostRuleContract> {
    let mut rules = vec![
        UnsupportedHostRuleContract {
            code: "CC-ROUTE-HOST-001".to_owned(),
            path: format!("{}.host.board_edge_clearance", request.path),
        },
        UnsupportedHostRuleContract {
            code: "CC-ROUTE-HOST-002".to_owned(),
            path: format!("{}.host.courtyard_clearance", request.path),
        },
        UnsupportedHostRuleContract {
            code: "CC-ROUTE-HOST-003".to_owned(),
            path: format!("{}.host.kicad_custom_rules", request.path),
        },
        UnsupportedHostRuleContract {
            code: "CC-ROUTE-HOST-004".to_owned(),
            path: format!("{}.host.schematic_board_parity", request.path),
        },
    ];
    rules.sort();
    rules
}

#[derive(Serialize)]
struct LoweredDesignFingerprint<'a> {
    domain: &'static str,
    design_name: &'a str,
    request_path: &'a str,
    layers: &'a [LayerContract],
    nets: &'a [NetContract],
    terminals: &'a [TerminalContract],
    obstacles: &'a [ObstacleContract],
    routing_profile: &'a RoutingProfileContract,
    compiler_profile: &'a CompilerProfileContract,
    unsupported_host_rules: &'a [UnsupportedHostRuleContract],
}

#[allow(clippy::too_many_arguments)]
fn lowered_design_fingerprint(
    design: &Design,
    request: &RoutingRequest,
    layers: &[LayerContract],
    nets: &[NetContract],
    terminals: &[TerminalContract],
    obstacles: &[ObstacleContract],
    routing_profile: &RoutingProfileContract,
    compiler_profile: &CompilerProfileContract,
    unsupported_host_rules: &[UnsupportedHostRuleContract],
) -> Result<String, Diagnostic> {
    let value = LoweredDesignFingerprint {
        domain: "circuitc-apgar-design-fingerprint-v1",
        design_name: &design.name,
        request_path: &request.path,
        layers,
        nets,
        terminals,
        obstacles,
        routing_profile,
        compiler_profile,
        unsupported_host_rules,
    };
    let bytes = serde_json::to_vec(&value).map_err(|error| {
        lower_diagnostic(
            LOWER_INTERNAL,
            request_path(request),
            format!("could not serialize deterministic design fingerprint input: {error}"),
        )
    })?;
    Ok(sha256_hex(&bytes))
}

fn request_identity(design: &Design, request: &RoutingRequest, fingerprint: &str) -> String {
    let mut hash = Sha256::new();
    append_identity_field(&mut hash, b"circuitc-apgar-request-identity-v1");
    append_identity_field(&mut hash, design.name.as_bytes());
    append_identity_field(&mut hash, fingerprint.as_bytes());
    append_identity_field(&mut hash, request.path.as_bytes());
    append_identity_field(&mut hash, request.net.as_bytes());
    append_identity_field(&mut hash, &request.width_nm.to_be_bytes());
    append_identity_field(&mut hash, &request.clearance_nm.to_be_bytes());
    append_identity_field(&mut hash, &request.grid_step_nm.to_be_bytes());
    append_identity_field(
        &mut hash,
        match request.layer {
            CopperLayer::Front => b"front",
            CopperLayer::Back => b"back",
        },
    );
    let digest = hash.finalize();
    digest.iter().map(|byte| format!("{byte:02x}")).collect()
}

fn stable_nonzero_u64(domain: &str, fields: &[&str], path: &str) -> Result<u64, Diagnostic> {
    let preimage = identity_preimage("circuitc-apgar-stable-u64-v1", domain, fields);
    let value = stable_u64(&preimage);
    if value == 0 {
        return Err(lower_diagnostic(
            LOWER_IDENTITY,
            path,
            format!("stable {domain} identity derived reserved value zero"),
        ));
    }
    Ok(value)
}

struct IdentityAllocator {
    assigned: BTreeMap<u64, (Vec<u8>, String)>,
    derive: fn(&[u8]) -> u64,
}

impl IdentityAllocator {
    fn new(derive: fn(&[u8]) -> u64) -> Self {
        Self {
            assigned: BTreeMap::new(),
            derive,
        }
    }

    fn allocate(
        &mut self,
        namespace: &str,
        domain: &str,
        fields: &[&str],
        path: impl Into<String>,
    ) -> Result<EntityRef, Diagnostic> {
        let path = path.into();
        let mut all_fields = Vec::with_capacity(fields.len() + 1);
        all_fields.push(namespace);
        all_fields.extend_from_slice(fields);
        let preimage = identity_preimage("circuitc-apgar-entity-v1", domain, &all_fields);
        let id = (self.derive)(&preimage);
        if id == 0 {
            return Err(lower_diagnostic(
                LOWER_IDENTITY,
                &path,
                "stable APGAR entity identity derived reserved value zero",
            ));
        }
        if let Some((existing_preimage, existing_path)) = self.assigned.get(&id) {
            if existing_preimage != &preimage {
                return Err(Diagnostic {
                    code: LOWER_IDENTITY,
                    path,
                    related_path: Some(existing_path.clone()),
                    message: format!(
                        "stable APGAR entity-ID collision at 0x{id:016x} between distinct semantic identities"
                    ),
                });
            }
        } else {
            self.assigned.insert(id, (preimage, path));
        }
        Ok(EntityRef { id, generation: 0 })
    }
}

fn identity_preimage(prefix: &str, domain: &str, fields: &[&str]) -> Vec<u8> {
    let mut bytes = Vec::new();
    append_vec_identity_field(&mut bytes, prefix.as_bytes());
    append_vec_identity_field(&mut bytes, domain.as_bytes());
    for field in fields {
        append_vec_identity_field(&mut bytes, field.as_bytes());
    }
    bytes
}

fn append_vec_identity_field(output: &mut Vec<u8>, value: &[u8]) {
    let length = u64::try_from(value.len()).unwrap_or(u64::MAX);
    output.extend_from_slice(&length.to_be_bytes());
    output.extend_from_slice(value);
}

fn append_identity_field(hash: &mut Sha256, value: &[u8]) {
    let length = u64::try_from(value.len()).unwrap_or(u64::MAX);
    hash.update(length.to_be_bytes());
    hash.update(value);
}

fn stable_u64(preimage: &[u8]) -> u64 {
    let digest = Sha256::digest(preimage);
    u64::from_be_bytes([
        digest[0], digest[1], digest[2], digest[3], digest[4], digest[5], digest[6], digest[7],
    ])
}

fn request_path(request: &RoutingRequest) -> String {
    format!("design.board.routing_requests.{}", request.path)
}

fn lower_diagnostic(
    code: &'static str,
    path: impl Into<String>,
    message: impl Into<String>,
) -> Diagnostic {
    Diagnostic {
        code,
        path: path.into(),
        related_path: None,
        message: message.into(),
    }
}

#[cfg(test)]
mod tests {
    use crate::demo;
    use crate::design::{CopperLayer, PointNm, RoutingRequest};

    use super::{
        BACK_ROUTING_ID, FRONT_ROUTING_ID, IdentityAllocator, LOWER_CONVERSION, LOWER_IDENTITY,
        ORTHOGONAL_STEP_COST, TILE_DIMENSION_NODES, lower_request, sha256_hex,
    };

    const MM: i64 = 1_000_000;

    fn routing_design() -> crate::design::Design {
        let mut design = demo::voltage_divider();
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

    #[test]
    fn exact_scale_and_golden_profile_fields_are_lowered() {
        let bundle = lower_request(&routing_design()).unwrap().unwrap();
        let request = &bundle.request;
        assert_eq!(request.dbu_per_millimeter, 2_000_000);
        assert_eq!(request.request_path, "board.autoroute.vout");
        assert_eq!(request.layers.len(), 2);
        assert_eq!(request.layers[0].routing_id, FRONT_ROUTING_ID);
        assert_eq!(request.layers[1].routing_id, BACK_ROUTING_ID);
        assert_eq!(request.routing_profile.nominal_width_dbu, 500_000);
        assert_eq!(request.routing_profile.clearance_dbu, 400_000);
        assert_eq!(
            request.routing_profile.allowed_layers,
            vec![FRONT_ROUTING_ID]
        );
        assert_eq!(request.compiler_profile.lattice_step_dbu, 2_000_000);
        assert_eq!(
            request.compiler_profile.tile_width_nodes,
            TILE_DIMENSION_NODES
        );
        assert_eq!(
            request.compiler_profile.tile_height_nodes,
            TILE_DIMENSION_NODES
        );
        assert_eq!(
            request.compiler_profile.compilation_roi,
            request.compiler_profile.active_regions[0].bounds
        );
        assert_eq!(
            request.compiler_profile.costs.orthogonal_step,
            ORTHOGONAL_STEP_COST
        );
        assert_eq!(request.compiler_profile.costs.diagonal_step, 1_414);
        assert_eq!(request.compiler_profile.costs.bend, 100);
        assert_eq!(request.planar_route.candidate_policy.deterministic_seed, 0);
        assert_eq!(request.planar_route.candidate_policy.candidate_ordinal, 0);
        assert_eq!(
            request.planar_route.start,
            super::PointDbu {
                x: 48 * MM,
                y: 20 * MM
            }
        );
        assert_eq!(
            request.planar_route.goal,
            super::PointDbu {
                x: 32 * MM,
                y: 20 * MM
            }
        );
        assert_eq!(
            bundle.request_sha256,
            sha256_hex(bundle.request_json.as_bytes())
        );
        assert!(bundle.request_json.ends_with('\n'));
        assert_eq!(request.unsupported_host_rules.len(), 4);
    }

    #[test]
    fn declaration_permutations_produce_identical_request_bytes() {
        let original = routing_design();
        let mut permuted = original.clone();
        permuted.nets.reverse();
        permuted.components.reverse();
        permuted.board.routes.reverse();
        for component in &mut permuted.components {
            component.connections.reverse();
            if let Some(physical) = &mut component.physical {
                physical.footprint.pads.reverse();
                physical.pin_pad_bindings.reverse();
            }
        }
        assert_eq!(
            lower_request(&original).unwrap().unwrap().request_json,
            lower_request(&permuted).unwrap().unwrap().request_json
        );
    }

    #[test]
    fn rotated_asymmetric_pad_uses_swapped_conservative_bounds() {
        let mut design = routing_design();
        let component = design
            .components
            .iter_mut()
            .find(|component| component.path == "divider.r_top")
            .unwrap();
        component
            .physical
            .as_mut()
            .unwrap()
            .placement
            .rotation_degrees = 90;
        let request = lower_request(&design).unwrap().unwrap().request;
        let obstacle = request
            .obstacles
            .iter()
            .find(|obstacle| obstacle.provenance == "divider.r_top.footprint.pad.2")
            .unwrap();
        assert_eq!(
            obstacle.bounds.min,
            super::PointDbu::new_for_test(29_050_000, 17_100_000)
        );
        assert_eq!(
            obstacle.bounds.max,
            super::PointDbu::new_for_test(30_950_000, 18_900_000)
        );
    }

    #[test]
    fn back_layer_request_uses_routing_id_31() {
        let mut design = routing_design();
        design.board.routing_requests[0].layer = CopperLayer::Back;
        for component in &mut design.components {
            if let Some(physical) = &mut component.physical {
                physical.placement.layer = CopperLayer::Back;
            }
        }
        let request = lower_request(&design).unwrap().unwrap().request;
        assert_eq!(
            request.routing_profile.allowed_layers,
            vec![BACK_ROUTING_ID]
        );
        assert_eq!(request.planar_route.start_layer, BACK_ROUTING_ID);
        assert_eq!(request.planar_route.goal_layer, BACK_ROUTING_ID);
        assert_eq!(
            request.compiler_profile.active_regions[0].layer,
            BACK_ROUTING_ID
        );
    }

    #[test]
    fn apgar_coordinate_envelope_overflow_is_rejected() {
        let mut design = routing_design();
        let delta = 600_000_000_000;
        design.board.outline.origin.x = delta;
        for component in &mut design.components {
            if let Some(physical) = &mut component.physical {
                physical.placement.position.x += delta;
            }
        }
        for route in &mut design.board.routes {
            route.start.x += delta;
            route.end.x += delta;
        }
        let error = lower_request(&design).unwrap_err();
        assert_eq!(error[0].code, LOWER_CONVERSION);
    }

    #[test]
    fn stable_entity_collision_seam_fails_closed() {
        let mut allocator = IdentityAllocator::new(|_| 7);
        allocator
            .allocate("design", "net", &["first"], "design.nets.first")
            .unwrap();
        let error = allocator
            .allocate("design", "net", &["second"], "design.nets.second")
            .unwrap_err();
        assert_eq!(error.code, LOWER_IDENTITY);
        assert_eq!(error.related_path.as_deref(), Some("design.nets.first"));
    }

    #[test]
    fn semantic_target_order_is_independent_of_hash_order() {
        let request = lower_request(&routing_design()).unwrap().unwrap().request;
        assert_eq!(
            request.planar_route.start,
            super::PointDbu {
                x: 48 * MM,
                y: 20 * MM
            }
        );
        assert_eq!(
            request.planar_route.goal,
            super::PointDbu {
                x: 32 * MM,
                y: 20 * MM
            }
        );
    }

    trait TestPoint {
        fn new_for_test(x: i64, y: i64) -> Self;
    }

    impl TestPoint for super::PointDbu {
        fn new_for_test(x: i64, y: i64) -> Self {
            Self { x, y }
        }
    }

    #[test]
    fn zero_requests_lower_to_no_bundle() {
        let design = demo::voltage_divider();
        assert!(lower_request(&design).unwrap().is_none());
    }

    #[test]
    fn source_permutation_does_not_move_terminals() {
        let mut design = routing_design();
        design.components.reverse();
        let request = lower_request(&design).unwrap().unwrap().request;
        let centers: Vec<PointNm> = request
            .terminals
            .iter()
            .filter(|terminal| terminal.net == request.routing_profile.net)
            .map(|terminal| PointNm::new(terminal.center.x / 2, terminal.center.y / 2))
            .collect();
        assert!(centers.contains(&PointNm::new(16 * MM, 10 * MM)));
        assert!(centers.contains(&PointNm::new(24 * MM, 10 * MM)));
    }
}
