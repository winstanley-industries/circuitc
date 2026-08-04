//! Shared strict verifier for externally joined APGAR acceptance evidence.

use serde::Serialize;

use crate::compile::RelativeArtifactPath;

use super::contract::{
    ContractDiagnostic, LayerSide, RouteOutcome, ToolIdentity, parse_request, parse_result,
    sha256_hex,
};
use super::import::{
    authenticate_candidate, authenticate_result_root, dbu_to_nm, expected_cpu_tool, point_to_nm,
};
use super::lower::RouteInputBundle;
use super::{
    APGAR_CONTRACT_IDENTITY, APGAR_CPU_DEVICE_CLASS, APGAR_TOOL_NAME, APGAR_TOOL_VERSION,
    PINNED_APGAR_SOURCE_REVISION,
};

const EVIDENCE_ERROR: &str = "CC-ROUTE-EVIDENCE-001";

#[derive(Serialize)]
struct VerifiedEvidence {
    schema_name: &'static str,
    schema_version: u32,
    design_name: String,
    request_path: String,
    request_identity_sha256: String,
    request_sha256: String,
    result_sha256: String,
    provenance_sha256: String,
    selected_candidate_id: String,
    candidate_geometry_signature: String,
    candidate_resource_signature: String,
    candidate_payload_checksum: String,
    tool: ToolIdentity,
    segments: Vec<VerifiedSegment>,
}

#[derive(Serialize)]
struct VerifiedSegment {
    ordinal: u64,
    semantic_path: String,
    net: String,
    layer: VerifiedLayer,
    start_nm: VerifiedPoint,
    end_nm: VerifiedPoint,
    width_nm: i64,
}

#[derive(Clone, Copy, Serialize)]
#[serde(rename_all = "snake_case")]
enum VerifiedLayer {
    Front,
    Back,
}

#[derive(Serialize)]
struct VerifiedPoint {
    x: i64,
    y: i64,
}

pub(crate) fn verify(
    request_json: &str,
    result_json: &str,
    provenance: &str,
) -> Result<String, ContractDiagnostic> {
    let request = parse_request(request_json).map_err(|error| {
        evidence_error(
            error.path,
            format!("APGAR request contract is invalid: {}", error.message),
        )
    })?;
    let request_sha256 = sha256_hex(request_json.as_bytes());
    let request_path = RelativeArtifactPath::try_new(format!(
        "routing/{}/request.json",
        request.request_identity_sha256
    ))
    .map_err(|error| evidence_error("request_path", error.to_string()))?;
    let bundle = RouteInputBundle {
        request_path,
        request: request.clone(),
        request_json: request_json.to_owned(),
        request_sha256: request_sha256.clone(),
    };
    let result = parse_result(result_json).map_err(|error| {
        evidence_error(
            error.path,
            format!("APGAR result contract is invalid: {}", error.message),
        )
    })?;
    let tool = authenticated_provenance(provenance)?;
    authenticate_result_root(&bundle, &result, &tool)?;
    let (selected_candidate_id, candidate) = match &result.outcome {
        RouteOutcome::Completed {
            selected_candidate_id,
            candidates,
        } => {
            let candidate = candidates
                .iter()
                .find(|candidate| candidate.id == *selected_candidate_id)
                .ok_or_else(|| {
                    evidence_error(
                        "result.outcome.selected_candidate_id",
                        "selected candidate is absent from completed result",
                    )
                })?;
            (selected_candidate_id.clone(), candidate)
        }
        RouteOutcome::Failure { .. } => {
            return Err(evidence_error(
                "result.outcome",
                "route acceptance requires a completed APGAR result",
            ));
        }
    };
    authenticate_candidate(&request, candidate)?;

    let routed_net = request
        .nets
        .iter()
        .find(|net| net.reference == candidate.net)
        .ok_or_else(|| evidence_error("candidate.net", "selected net is absent from request"))?;
    let selected_layer = request.routing_profile.allowed_layers[0];
    let layer = request
        .layers
        .iter()
        .find(|layer| layer.routing_id == selected_layer)
        .ok_or_else(|| {
            evidence_error(
                "candidate.geometry.layer",
                "selected layer is absent from request",
            )
        })?;
    let layer = match layer.side {
        LayerSide::Front => VerifiedLayer::Front,
        LayerSide::Back => VerifiedLayer::Back,
    };
    let segments = candidate
        .geometry
        .iter()
        .enumerate()
        .map(|(index, primitive)| {
            let start = point_to_nm(
                primitive.start,
                &format!("candidate.geometry[{index}].start"),
            )?;
            let end = point_to_nm(primitive.end, &format!("candidate.geometry[{index}].end"))?;
            Ok(VerifiedSegment {
                ordinal: index as u64,
                semantic_path: format!("{}.segment.{index:08}", request.request_path),
                net: routed_net.name.clone(),
                layer: match layer {
                    VerifiedLayer::Front => VerifiedLayer::Front,
                    VerifiedLayer::Back => VerifiedLayer::Back,
                },
                start_nm: VerifiedPoint {
                    x: start.x,
                    y: start.y,
                },
                end_nm: VerifiedPoint { x: end.x, y: end.y },
                width_nm: dbu_to_nm(
                    primitive.width_dbu,
                    &format!("candidate.geometry[{index}].width_dbu"),
                )?,
            })
        })
        .collect::<Result<Vec<_>, ContractDiagnostic>>()?;
    let evidence = VerifiedEvidence {
        schema_name: "circuitc.verified_apgar_route_evidence",
        schema_version: 1,
        design_name: request.design_name,
        request_path: request.request_path,
        request_identity_sha256: request.request_identity_sha256,
        request_sha256,
        result_sha256: sha256_hex(result_json.as_bytes()),
        provenance_sha256: sha256_hex(provenance.as_bytes()),
        selected_candidate_id,
        candidate_geometry_signature: candidate.geometry_signature.clone(),
        candidate_resource_signature: candidate.resource_signature.clone(),
        candidate_payload_checksum: candidate.payload_checksum.clone(),
        tool,
        segments,
    };
    let mut json = serde_json::to_string(&evidence)
        .map_err(|error| evidence_error("evidence", error.to_string()))?;
    json.push('\n');
    Ok(json)
}

fn authenticated_provenance(provenance: &str) -> Result<ToolIdentity, ContractDiagnostic> {
    let prefix = format!(
        "circuitc-apgar-route-provenance-v1\nname={APGAR_TOOL_NAME}\nversion={APGAR_TOOL_VERSION}\ncontract={APGAR_CONTRACT_IDENTITY}\nsource_revision={PINNED_APGAR_SOURCE_REVISION}\nexecutable_sha256="
    );
    let suffix = format!("\ndevice_class={APGAR_CPU_DEVICE_CLASS}\n");
    let Some(executable_sha256) = provenance
        .strip_prefix(&prefix)
        .and_then(|value| value.strip_suffix(&suffix))
    else {
        return Err(evidence_error(
            "provenance",
            "APGAR provenance does not match the pinned CPU tool identity",
        ));
    };
    if executable_sha256.len() != 64
        || !executable_sha256
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
    {
        return Err(evidence_error(
            "provenance.executable_sha256",
            "APGAR provenance executable digest is not canonical SHA-256",
        ));
    }
    Ok(expected_cpu_tool(executable_sha256.to_owned()))
}

fn evidence_error(path: impl Into<String>, message: impl Into<String>) -> ContractDiagnostic {
    ContractDiagnostic {
        code: EVIDENCE_ERROR.to_owned(),
        path: path.into(),
        message: message.into(),
    }
}
