use std::collections::BTreeMap;

use serde::Serialize;
use sha2::{Digest, Sha256};

use crate::design::{Design, ManufacturabilityCapability};
use crate::manufacturing::{
    FabricationCompilerArtifacts, FabricationManifestBundle, prepare_kicad10_fabrication_request,
};
use crate::product::ProductArtifactBundle;
use crate::{CompiledArtifacts, RelativeArtifactPath};

use super::contract::{
    ADAPTER, ADAPTER_MAJOR, ADAPTER_VERSION, AnalysisPolicy, ArtifactBinding, AssertionDescriptor,
    AssertionOutcome, BoardAnalysisBundle, BoardAnalysisDiagnostic, BoardAnalysisFile,
    BoardAnalysisHostEvidence, BoardAnalysisNoncompletion, BoardAnalysisNoncompletionKind,
    BoardAnalysisReport, BoardAnalysisRequest, BoardAnalysisRequestBundle, BoardAnalysisResult,
    CompletedEvidence, ExecutionDiagnostic, ExpectedSheet, IDENTITY_DOMAIN, MAX_AGGREGATE_BYTES,
    MAX_FILE_BYTES, OutputDescriptor, REPORT_SCHEMA, REQUEST_SCHEMA, RESULT_SCHEMA, Receipt,
    RequestPreimage, ResourcePolicy, SCHEMA_VERSION, ToolIdentity,
};
use super::normalize::{analysis_policy, validate_identity_map, validate_reports};

const RECEIPT_SCHEMA: &str = "circuitc.board_analysis_receipt";

fn diagnostic(
    code: &'static str,
    path: impl Into<String>,
    message: impl Into<String>,
) -> BoardAnalysisDiagnostic {
    BoardAnalysisDiagnostic {
        code,
        path: path.into(),
        message: message.into(),
    }
}

fn sha256_hex(bytes: &[u8]) -> String {
    let mut hasher = Sha256::new();
    hasher.update(bytes);
    hasher
        .finalize()
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect()
}

fn canonical_json<T: Serialize>(value: &T, path: &str) -> Result<String, BoardAnalysisDiagnostic> {
    let mut rendered = serde_json::to_string(value).map_err(|error| {
        diagnostic(
            "CC-BOARD-ANALYSIS-CONTRACT-001",
            path,
            format!("board-analysis serialization failed: {error}"),
        )
    })?;
    rendered.push('\n');
    if rendered.len() > MAX_FILE_BYTES {
        return Err(diagnostic(
            "CC-BOARD-ANALYSIS-RESOURCE-001",
            path,
            "board-analysis contract exceeds the 64 MiB byte limit",
        ));
    }
    Ok(rendered)
}

fn analysis_identity_sha256(preimage: &RequestPreimage) -> Result<String, BoardAnalysisDiagnostic> {
    let preimage_json = serde_json::to_vec(preimage).map_err(|error| {
        diagnostic(
            "CC-BOARD-ANALYSIS-CONTRACT-001",
            "request",
            format!("analysis identity preimage cannot be serialized: {error}"),
        )
    })?;
    let mut identity_hasher = Sha256::new();
    identity_hasher.update(IDENTITY_DOMAIN);
    identity_hasher.update(preimage_json);
    Ok(identity_hasher
        .finalize()
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect())
}

fn artifact_path(path: String) -> Result<RelativeArtifactPath, BoardAnalysisDiagnostic> {
    RelativeArtifactPath::try_new(path)
        .map_err(|error| diagnostic("CC-BOARD-ANALYSIS-CONTRACT-001", "path", error.to_string()))
}

fn bind_bytes(path: String, bytes: &[u8]) -> Result<ArtifactBinding, BoardAnalysisDiagnostic> {
    artifact_path(path.clone())?;
    if bytes.len() > MAX_FILE_BYTES {
        return Err(diagnostic(
            "CC-BOARD-ANALYSIS-RESOURCE-001",
            path,
            "bound analysis artifact exceeds the 64 MiB byte limit",
        ));
    }
    Ok(ArtifactBinding {
        path,
        byte_length: u64::try_from(bytes.len()).expect("usize fits u64 on supported targets"),
        sha256: sha256_hex(bytes),
    })
}

fn bounded_aggregate<I>(
    lengths: I,
    limit: usize,
    path: &'static str,
    overflow_message: &'static str,
    limit_message: &'static str,
) -> Result<usize, BoardAnalysisDiagnostic>
where
    I: IntoIterator<Item = usize>,
{
    let aggregate = lengths
        .into_iter()
        .try_fold(0_usize, |total, length| total.checked_add(length))
        .ok_or_else(|| diagnostic("CC-BOARD-ANALYSIS-RESOURCE-001", path, overflow_message))?;
    if aggregate > limit {
        return Err(diagnostic(
            "CC-BOARD-ANALYSIS-RESOURCE-001",
            path,
            limit_message,
        ));
    }
    Ok(aggregate)
}

#[cfg(test)]
fn completed_input_aggregate<I>(lengths: I) -> Result<usize, BoardAnalysisDiagnostic>
where
    I: IntoIterator<Item = usize>,
{
    completed_input_aggregate_with_limit(lengths, MAX_AGGREGATE_BYTES)
}

fn completed_input_aggregate_with_limit<I>(
    lengths: I,
    limit: usize,
) -> Result<usize, BoardAnalysisDiagnostic>
where
    I: IntoIterator<Item = usize>,
{
    bounded_aggregate(
        lengths,
        limit,
        "inputs",
        "board-analysis input aggregate size overflowed",
        "board-analysis inputs exceed the 256 MiB aggregate limit",
    )
}

#[cfg(test)]
fn completed_bundle_aggregate<I>(lengths: I) -> Result<usize, BoardAnalysisDiagnostic>
where
    I: IntoIterator<Item = usize>,
{
    completed_bundle_aggregate_with_limit(lengths, MAX_AGGREGATE_BYTES)
}

fn completed_bundle_aggregate_with_limit<I>(
    lengths: I,
    limit: usize,
) -> Result<usize, BoardAnalysisDiagnostic>
where
    I: IntoIterator<Item = usize>,
{
    bounded_aggregate(
        lengths,
        limit,
        "bundle",
        "board-analysis aggregate size overflowed",
        "board-analysis bundle exceeds the 256 MiB aggregate limit",
    )
}

fn capability_name(capability: ManufacturabilityCapability) -> &'static str {
    match capability {
        ManufacturabilityCapability::ErcClean => "erc_clean",
        ManufacturabilityCapability::DrcClean => "drc_clean",
        ManufacturabilityCapability::UnconnectedClean => "unconnected_clean",
        ManufacturabilityCapability::SchematicParityClean => "schematic_parity_clean",
        ManufacturabilityCapability::FabricationInventoryComplete => {
            "fabrication_inventory_complete"
        }
    }
}

fn evidence_role(capability: &str) -> &'static str {
    match capability {
        "erc_clean" => "erc",
        "drc_clean" | "unconnected_clean" | "schematic_parity_clean" => "drc",
        "fabrication_inventory_complete" => "fabrication_manifest",
        _ => unreachable!("closed capability set"),
    }
}

fn exact_assertions(
    design: &Design,
    analysis_path: &str,
) -> Result<(Vec<AssertionDescriptor>, String), BoardAnalysisDiagnostic> {
    let analysis = design
        .product
        .manufacturability_analyses
        .iter()
        .find(|analysis| analysis.path == analysis_path)
        .ok_or_else(|| {
            diagnostic(
                "CC-BOARD-ANALYSIS-AUTH-001",
                "analysis_path",
                "Design does not declare the selected manufacturability analysis",
            )
        })?;
    if analysis.adapter != ADAPTER || analysis.version != ADAPTER_MAJOR.to_string() {
        return Err(diagnostic(
            "CC-BOARD-ANALYSIS-AUTH-001",
            analysis_path,
            "board analysis v1 requires the authored kicad version 10 adapter",
        ));
    }
    let by_capability: BTreeMap<_, _> = analysis
        .assertions
        .iter()
        .map(|assertion| (assertion.capability, assertion.path.as_str()))
        .collect();
    let required = [
        ManufacturabilityCapability::ErcClean,
        ManufacturabilityCapability::DrcClean,
        ManufacturabilityCapability::UnconnectedClean,
        ManufacturabilityCapability::SchematicParityClean,
        ManufacturabilityCapability::FabricationInventoryComplete,
    ];
    if analysis.assertions.len() != required.len()
        || required
            .iter()
            .any(|capability| !by_capability.contains_key(capability))
    {
        return Err(diagnostic(
            "CC-BOARD-ANALYSIS-CAPABILITY-001",
            analysis_path,
            "board analysis v1 requires exactly all five KiCad manufacturability capabilities",
        ));
    }
    let assertions = required
        .into_iter()
        .map(|capability| AssertionDescriptor {
            assertion_path: by_capability[&capability].to_owned(),
            capability: capability_name(capability).to_owned(),
        })
        .collect();
    Ok((
        assertions,
        by_capability[&ManufacturabilityCapability::FabricationInventoryComplete].to_owned(),
    ))
}

fn static_artifacts(compiled: FabricationCompilerArtifacts<'_>) -> &CompiledArtifacts {
    match compiled {
        FabricationCompilerArtifacts::Static(artifacts) => artifacts,
        FabricationCompilerArtifacts::Checked(artifacts) => artifacts.static_artifacts(),
    }
}

fn fixed_outputs() -> Vec<OutputDescriptor> {
    vec![
        OutputDescriptor {
            role: "erc".to_owned(),
            path: "erc.normalized.json".to_owned(),
        },
        OutputDescriptor {
            role: "drc".to_owned(),
            path: "drc.normalized.json".to_owned(),
        },
        OutputDescriptor {
            role: "receipt".to_owned(),
            path: "receipt.json".to_owned(),
        },
    ]
}

fn expected_sheets(
    artifacts: &CompiledArtifacts,
) -> Result<Vec<ExpectedSheet>, BoardAnalysisDiagnostic> {
    let mut schematic_roots = artifacts
        .kicad_identities
        .iter()
        .filter(|identity| identity.semantic_path == "design.schematic");
    let root = schematic_roots.next().ok_or_else(|| {
        diagnostic(
            "CC-BOARD-ANALYSIS-AUTH-001",
            "expected_sheets",
            "compiled KiCad identity inventory has no schematic root",
        )
    })?;
    if schematic_roots.next().is_some() {
        return Err(diagnostic(
            "CC-BOARD-ANALYSIS-AUTH-001",
            "expected_sheets",
            "compiled KiCad identity inventory has duplicate schematic roots",
        ));
    }
    Ok(vec![ExpectedSheet {
        path: "/".to_owned(),
        uuid_path: format!("/{}", root.uuid),
    }])
}

fn project_support(
    design_name: &str,
    artifacts: &CompiledArtifacts,
) -> Result<Vec<ArtifactBinding>, BoardAnalysisDiagnostic> {
    let mut bindings = Vec::with_capacity(artifacts.kicad_library_files.len() + 3);
    bindings.push(bind_bytes(
        format!("{design_name}.kicad_pro"),
        artifacts.kicad_project.as_bytes(),
    )?);
    for library in &artifacts.kicad_library_files {
        bindings.push(bind_bytes(
            library.relative_path.as_str().to_owned(),
            library.contents.as_bytes(),
        )?);
    }
    bindings.push(bind_bytes(
        "sym-lib-table".to_owned(),
        artifacts.kicad_symbol_table.as_bytes(),
    )?);
    bindings.push(bind_bytes(
        "fp-lib-table".to_owned(),
        artifacts.kicad_footprint_table.as_bytes(),
    )?);
    bindings.sort_by(|left, right| left.path.cmp(&right.path));
    if bindings.windows(2).any(|pair| pair[0].path == pair[1].path) {
        return Err(diagnostic(
            "CC-BOARD-ANALYSIS-CONTRACT-001",
            "project_support",
            "compiled KiCad project support paths are not unique",
        ));
    }
    let aggregate = bindings.iter().try_fold(0_u64, |total, binding| {
        total.checked_add(binding.byte_length)
    });
    if aggregate.is_none_or(|bytes| bytes > MAX_AGGREGATE_BYTES as u64) {
        return Err(diagnostic(
            "CC-BOARD-ANALYSIS-RESOURCE-001",
            "project_support",
            "compiled KiCad project support exceeds the aggregate byte limit",
        ));
    }
    Ok(bindings)
}

struct Prepared {
    request: BoardAnalysisRequest,
    request_json: String,
    request_path: RelativeArtifactPath,
    result_path: RelativeArtifactPath,
    report_path: RelativeArtifactPath,
    expected_host_paths: Vec<RelativeArtifactPath>,
}

#[allow(clippy::too_many_arguments)]
fn prepare(
    design: &Design,
    snapshot_bytes: &[u8],
    variant_path: &str,
    compiled: FabricationCompilerArtifacts<'_>,
    product: &ProductArtifactBundle,
    analysis_path: &str,
    kicad_identity_map_json: &str,
    fabrication: &FabricationManifestBundle,
) -> Result<Prepared, BoardAnalysisDiagnostic> {
    let (assertions, fabrication_assertion_path) = exact_assertions(design, analysis_path)?;
    let expected_fabrication = prepare_kicad10_fabrication_request(
        design,
        snapshot_bytes,
        variant_path,
        compiled,
        product,
        analysis_path,
        &fabrication_assertion_path,
    )
    .map_err(|error| {
        diagnostic(
            "CC-BOARD-ANALYSIS-AUTH-001",
            "fabrication",
            format!("fabrication predecessor is not authoritative: {error}"),
        )
    })?;
    if fabrication.request_json() != expected_fabrication.request_json
        || fabrication.fabrication_identity_sha256()
            != expected_fabrication.fabrication_identity_sha256
        || fabrication.request_path() != &expected_fabrication.request_path
        || fabrication.manifest_path() != &expected_fabrication.manifest_path
    {
        return Err(diagnostic(
            "CC-BOARD-ANALYSIS-AUTH-001",
            "fabrication",
            "fabrication bundle does not match the current Design, product, and board",
        ));
    }
    if kicad_identity_map_json.len() > MAX_FILE_BYTES {
        return Err(diagnostic(
            "CC-BOARD-ANALYSIS-RESOURCE-001",
            "kicad_identity_map",
            "KiCad identity map exceeds the 64 MiB byte limit",
        ));
    }
    let artifacts = static_artifacts(compiled);
    validate_identity_map(
        kicad_identity_map_json.as_bytes(),
        &design.name,
        &artifacts.kicad_identities,
    )?;
    let schematic = bind_bytes(
        format!("{}.kicad_sch", design.name),
        artifacts.kicad_schematic.as_bytes(),
    )?;
    let pcb = bind_bytes(
        format!("{}.kicad_pcb", design.name),
        artifacts.kicad_pcb.as_bytes(),
    )?;
    let identity_map = bind_bytes(
        format!("{}.kicad-map.json", design.name),
        kicad_identity_map_json.as_bytes(),
    )?;
    let fabrication_request = bind_bytes(
        fabrication.request_path().as_str().to_owned(),
        fabrication.request_json().as_bytes(),
    )?;
    let fabrication_manifest = bind_bytes(
        fabrication.manifest_path().as_str().to_owned(),
        fabrication.manifest_json().as_bytes(),
    )?;
    let policy: AnalysisPolicy = analysis_policy();
    let resources = ResourcePolicy::default();
    let outputs = fixed_outputs();
    let expected_sheets = expected_sheets(artifacts)?;
    let project_support = project_support(&design.name, artifacts)?;
    let preimage = RequestPreimage {
        design_name: design.name.clone(),
        analysis_path: analysis_path.to_owned(),
        adapter: ADAPTER.to_owned(),
        expected_major: ADAPTER_MAJOR,
        expected_version: ADAPTER_VERSION.to_owned(),
        assertions: assertions.clone(),
        kicad_schematic: schematic.clone(),
        kicad_pcb: pcb.clone(),
        kicad_identity_map: identity_map.clone(),
        expected_sheets: expected_sheets.clone(),
        project_support: project_support.clone(),
        fabrication_request: fabrication_request.clone(),
        fabrication_manifest: fabrication_manifest.clone(),
        policy: policy.clone(),
        resources: resources.clone(),
        outputs: outputs.clone(),
    };
    let analysis_identity_sha256 = analysis_identity_sha256(&preimage)?;
    let root = format!("board-analysis/{analysis_identity_sha256}");
    let request_path = artifact_path(format!("{root}/request.json"))?;
    let result_path = artifact_path(format!("{root}/result.json"))?;
    let report_path = artifact_path(format!("{root}/report.json"))?;
    let request = BoardAnalysisRequest {
        schema_name: REQUEST_SCHEMA.to_owned(),
        schema_version: SCHEMA_VERSION,
        design_name: preimage.design_name,
        analysis_path: preimage.analysis_path,
        adapter: preimage.adapter,
        expected_major: preimage.expected_major,
        expected_version: preimage.expected_version,
        analysis_identity_sha256,
        assertions: preimage.assertions,
        kicad_schematic: preimage.kicad_schematic,
        kicad_pcb: preimage.kicad_pcb,
        kicad_identity_map: preimage.kicad_identity_map,
        expected_sheets: preimage.expected_sheets,
        project_support: preimage.project_support,
        fabrication_request: preimage.fabrication_request,
        fabrication_manifest: preimage.fabrication_manifest,
        policy: preimage.policy,
        resources: preimage.resources,
        outputs: preimage.outputs,
    };
    let request_json = canonical_json(&request, request_path.as_str())?;
    let expected_host_paths = request
        .outputs
        .iter()
        .map(|output| artifact_path(output.path.clone()))
        .collect::<Result<Vec<_>, _>>()?;
    Ok(Prepared {
        request,
        request_json,
        request_path,
        result_path,
        report_path,
        expected_host_paths,
    })
}

#[allow(clippy::too_many_arguments)]
pub fn prepare_kicad10_board_analysis_request(
    design: &Design,
    snapshot_bytes: &[u8],
    variant_path: &str,
    compiled: FabricationCompilerArtifacts<'_>,
    product: &ProductArtifactBundle,
    analysis_path: &str,
    kicad_identity_map_json: &str,
    fabrication: &FabricationManifestBundle,
) -> Result<BoardAnalysisRequestBundle, BoardAnalysisDiagnostic> {
    let prepared = prepare(
        design,
        snapshot_bytes,
        variant_path,
        compiled,
        product,
        analysis_path,
        kicad_identity_map_json,
        fabrication,
    )?;
    Ok(BoardAnalysisRequestBundle {
        analysis_identity_sha256: prepared.request.analysis_identity_sha256,
        request_path: prepared.request_path,
        result_path: prepared.result_path,
        report_path: prepared.report_path,
        expected_host_paths: prepared.expected_host_paths,
        request_json: prepared.request_json,
    })
}

fn report_outcomes(
    assertions: &[AssertionDescriptor],
    outcome: &str,
    include_evidence: bool,
) -> Vec<AssertionOutcome> {
    assertions
        .iter()
        .map(|assertion| AssertionOutcome {
            assertion_path: assertion.assertion_path.clone(),
            capability: assertion.capability.clone(),
            outcome: outcome.to_owned(),
            evidence_role: include_evidence
                .then(|| evidence_role(&assertion.capability).to_owned()),
        })
        .collect()
}

fn completed_report_outcomes(
    assertions: &[AssertionDescriptor],
    reports: &super::normalize::ValidatedReports,
) -> Vec<AssertionOutcome> {
    assertions
        .iter()
        .map(|assertion| {
            let passes = match assertion.capability.as_str() {
                "erc_clean" => reports.erc_clean,
                "drc_clean" => reports.drc_clean,
                "unconnected_clean" => reports.unconnected_clean,
                "schematic_parity_clean" => reports.schematic_parity_clean,
                "fabrication_inventory_complete" => true,
                _ => unreachable!("closed capability set"),
            };
            AssertionOutcome {
                assertion_path: assertion.assertion_path.clone(),
                capability: assertion.capability.clone(),
                outcome: if passes { "pass" } else { "fail" }.to_owned(),
                evidence_role: Some(evidence_role(&assertion.capability).to_owned()),
            }
        })
        .collect()
}

fn finish_bundle(
    prepared: Prepared,
    result: BoardAnalysisResult,
    outcomes: Vec<AssertionOutcome>,
    files: Vec<BoardAnalysisFile>,
    aggregate_limit: usize,
) -> Result<BoardAnalysisBundle, BoardAnalysisDiagnostic> {
    let result_json = canonical_json(&result, prepared.result_path.as_str())?;
    let report = BoardAnalysisReport {
        schema_name: REPORT_SCHEMA.to_owned(),
        schema_version: SCHEMA_VERSION,
        analysis_identity_sha256: prepared.request.analysis_identity_sha256.clone(),
        request: bind_bytes(
            prepared.request_path.as_str().to_owned(),
            prepared.request_json.as_bytes(),
        )?,
        result: bind_bytes(
            prepared.result_path.as_str().to_owned(),
            result_json.as_bytes(),
        )?,
        execution_status: result.status.clone(),
        all_pass: result.status == "completed"
            && outcomes.iter().all(|outcome| outcome.outcome == "pass"),
        outcomes,
    };
    let report_json = canonical_json(&report, prepared.report_path.as_str())?;
    completed_bundle_aggregate_with_limit(
        [
            prepared.request_json.len(),
            result_json.len(),
            report_json.len(),
        ]
        .into_iter()
        .chain(files.iter().map(|file| file.contents.len())),
        aggregate_limit,
    )?;
    Ok(BoardAnalysisBundle {
        analysis_identity_sha256: prepared.request.analysis_identity_sha256,
        request_path: prepared.request_path,
        result_path: prepared.result_path,
        report_path: prepared.report_path,
        request_json: prepared.request_json,
        result_json,
        report_json,
        files,
    })
}

fn validate_receipt(
    bytes: &[u8],
    prepared: &Prepared,
    evidence: &BoardAnalysisHostEvidence,
    erc_sha256: &str,
    drc_sha256: &str,
) -> Result<(), BoardAnalysisDiagnostic> {
    if bytes.len() > MAX_FILE_BYTES {
        return Err(diagnostic(
            "CC-BOARD-ANALYSIS-RESOURCE-001",
            "receipt",
            "analysis receipt exceeds the 64 MiB byte limit",
        ));
    }
    let receipt: Receipt = serde_json::from_slice(bytes).map_err(|error| {
        diagnostic(
            "CC-BOARD-ANALYSIS-RECEIPT-001",
            "receipt",
            format!("analysis receipt is invalid: {error}"),
        )
    })?;
    let canonical = canonical_json(&receipt, "receipt")?;
    if canonical.as_bytes() != bytes
        || receipt.schema_name != RECEIPT_SCHEMA
        || receipt.schema_version != SCHEMA_VERSION
        || receipt.request_sha256 != sha256_hex(prepared.request_json.as_bytes())
        || receipt.schematic_sha256 != prepared.request.kicad_schematic.sha256
        || receipt.pcb_sha256 != prepared.request.kicad_pcb.sha256
        || receipt.identity_map_sha256 != prepared.request.kicad_identity_map.sha256
        || receipt.executable_sha256 != sha256_hex(&evidence.host_executable)
        || receipt.normalizer_sha256 != sha256_hex(&evidence.normalizer)
        || receipt.host_runner_sha256 != sha256_hex(&evidence.host_runner)
        || receipt.erc_sha256 != erc_sha256
        || receipt.drc_sha256 != drc_sha256
    {
        return Err(diagnostic(
            "CC-BOARD-ANALYSIS-RECEIPT-001",
            "receipt",
            "analysis receipt does not bind the exact request, inputs, tools, and reports",
        ));
    }
    Ok(())
}

#[allow(clippy::too_many_arguments)]
pub fn bind_kicad10_board_analysis(
    design: &Design,
    snapshot_bytes: &[u8],
    variant_path: &str,
    compiled: FabricationCompilerArtifacts<'_>,
    product: &ProductArtifactBundle,
    analysis_path: &str,
    kicad_identity_map_json: &str,
    fabrication: &FabricationManifestBundle,
    evidence: &BoardAnalysisHostEvidence,
) -> Result<BoardAnalysisBundle, BoardAnalysisDiagnostic> {
    bind_kicad10_board_analysis_with_limits(
        design,
        snapshot_bytes,
        variant_path,
        compiled,
        product,
        analysis_path,
        kicad_identity_map_json,
        fabrication,
        evidence,
        MAX_AGGREGATE_BYTES,
        MAX_AGGREGATE_BYTES,
    )
}

#[allow(clippy::too_many_arguments)]
fn bind_kicad10_board_analysis_with_limits(
    design: &Design,
    snapshot_bytes: &[u8],
    variant_path: &str,
    compiled: FabricationCompilerArtifacts<'_>,
    product: &ProductArtifactBundle,
    analysis_path: &str,
    kicad_identity_map_json: &str,
    fabrication: &FabricationManifestBundle,
    evidence: &BoardAnalysisHostEvidence,
    input_aggregate_limit: usize,
    bundle_aggregate_limit: usize,
) -> Result<BoardAnalysisBundle, BoardAnalysisDiagnostic> {
    let prepared = prepare(
        design,
        snapshot_bytes,
        variant_path,
        compiled,
        product,
        analysis_path,
        kicad_identity_map_json,
        fabrication,
    )?;
    if evidence.host_version != ADAPTER_VERSION {
        return Err(diagnostic(
            "CC-BOARD-ANALYSIS-HOST-001",
            "host_version",
            "board analysis v1 requires exact KiCad 10.0.5",
        ));
    }
    for (path, bytes) in [
        ("host_executable", evidence.host_executable.as_slice()),
        ("normalizer", evidence.normalizer.as_slice()),
        ("host_runner", evidence.host_runner.as_slice()),
    ] {
        if bytes.is_empty() || bytes.len() > MAX_FILE_BYTES {
            return Err(diagnostic(
                "CC-BOARD-ANALYSIS-RESOURCE-001",
                path,
                "analysis tool image must be non-empty and at most 64 MiB",
            ));
        }
    }
    let project_support_bytes = prepared
        .request
        .project_support
        .iter()
        .try_fold(0_usize, |total, binding| {
            total.checked_add(
                usize::try_from(binding.byte_length).expect("bounded support length fits usize"),
            )
        })
        .ok_or_else(|| {
            diagnostic(
                "CC-BOARD-ANALYSIS-RESOURCE-001",
                "inputs",
                "board-analysis project-support size overflowed",
            )
        })?;
    completed_input_aggregate_with_limit(
        [
            prepared.request_json.len(),
            usize::try_from(prepared.request.kicad_schematic.byte_length)
                .expect("bounded length fits usize"),
            usize::try_from(prepared.request.kicad_pcb.byte_length)
                .expect("bounded length fits usize"),
            usize::try_from(prepared.request.kicad_identity_map.byte_length)
                .expect("bounded length fits usize"),
            project_support_bytes,
            usize::try_from(prepared.request.fabrication_request.byte_length)
                .expect("bounded length fits usize"),
            usize::try_from(prepared.request.fabrication_manifest.byte_length)
                .expect("bounded length fits usize"),
            evidence.host_executable.len(),
            evidence.normalizer.len(),
            evidence.host_runner.len(),
            evidence.erc_report_json.len(),
            evidence.drc_report_json.len(),
            evidence.receipt_json.len(),
        ],
        input_aggregate_limit,
    )?;
    let artifacts = static_artifacts(compiled);
    let reports = validate_reports(
        &evidence.erc_report_json,
        &evidence.drc_report_json,
        &design.name,
        &prepared.request.kicad_schematic.sha256,
        &prepared.request.kicad_pcb.sha256,
        &artifacts.kicad_identities,
        &prepared.request.expected_sheets,
    )?;
    let root = format!(
        "board-analysis/{}",
        prepared.request.analysis_identity_sha256
    );
    let erc_path = format!("{root}/evidence/erc.normalized.json");
    let drc_path = format!("{root}/evidence/drc.normalized.json");
    let erc_binding = bind_bytes(erc_path.clone(), &reports.erc)?;
    let drc_binding = bind_bytes(drc_path.clone(), &reports.drc)?;
    validate_receipt(
        &evidence.receipt_json,
        &prepared,
        evidence,
        &erc_binding.sha256,
        &drc_binding.sha256,
    )?;
    let request_binding = bind_bytes(
        prepared.request_path.as_str().to_owned(),
        prepared.request_json.as_bytes(),
    )?;
    let result = BoardAnalysisResult {
        schema_name: RESULT_SCHEMA.to_owned(),
        schema_version: SCHEMA_VERSION,
        analysis_identity_sha256: prepared.request.analysis_identity_sha256.clone(),
        request: request_binding,
        status: "completed".to_owned(),
        tool: Some(ToolIdentity {
            adapter: ADAPTER.to_owned(),
            version: evidence.host_version.clone(),
            executable_sha256: sha256_hex(&evidence.host_executable),
            normalizer_sha256: sha256_hex(&evidence.normalizer),
            host_runner_sha256: sha256_hex(&evidence.host_runner),
        }),
        evidence: Some(CompletedEvidence {
            erc: erc_binding,
            drc: drc_binding,
            fabrication_manifest: prepared.request.fabrication_manifest.clone(),
        }),
        diagnostic: None,
    };
    let outcomes = completed_report_outcomes(&prepared.request.assertions, &reports);
    finish_bundle(
        prepared,
        result,
        outcomes,
        vec![
            BoardAnalysisFile {
                path: artifact_path(erc_path)?,
                contents: reports.erc,
            },
            BoardAnalysisFile {
                path: artifact_path(drc_path)?,
                contents: reports.drc,
            },
        ],
        bundle_aggregate_limit,
    )
}

#[allow(clippy::too_many_arguments)]
pub fn record_kicad10_board_analysis_noncompletion(
    design: &Design,
    snapshot_bytes: &[u8],
    variant_path: &str,
    compiled: FabricationCompilerArtifacts<'_>,
    product: &ProductArtifactBundle,
    analysis_path: &str,
    kicad_identity_map_json: &str,
    fabrication: &FabricationManifestBundle,
    noncompletion: &BoardAnalysisNoncompletion,
) -> Result<BoardAnalysisBundle, BoardAnalysisDiagnostic> {
    let prepared = prepare(
        design,
        snapshot_bytes,
        variant_path,
        compiled,
        product,
        analysis_path,
        kicad_identity_map_json,
        fabrication,
    )?;
    if noncompletion.code.is_empty()
        || noncompletion.code.len() > 128
        || !noncompletion
            .code
            .bytes()
            .all(|byte| byte.is_ascii_uppercase() || byte.is_ascii_digit() || byte == b'-')
        || noncompletion.message.is_empty()
        || noncompletion.message.len() > 4096
        || noncompletion.message.contains(['\r', '\n', '\0'])
    {
        return Err(diagnostic(
            "CC-BOARD-ANALYSIS-CONTRACT-001",
            "diagnostic",
            "noncompletion diagnostic is not a bounded canonical one-line value",
        ));
    }
    let status = match noncompletion.kind {
        BoardAnalysisNoncompletionKind::Failed => "failed",
        BoardAnalysisNoncompletionKind::Unsupported => "unsupported",
    };
    let outcome = match noncompletion.kind {
        BoardAnalysisNoncompletionKind::Failed => "unevaluated",
        BoardAnalysisNoncompletionKind::Unsupported => "unsupported",
    };
    let result = BoardAnalysisResult {
        schema_name: RESULT_SCHEMA.to_owned(),
        schema_version: SCHEMA_VERSION,
        analysis_identity_sha256: prepared.request.analysis_identity_sha256.clone(),
        request: bind_bytes(
            prepared.request_path.as_str().to_owned(),
            prepared.request_json.as_bytes(),
        )?,
        status: status.to_owned(),
        tool: None,
        evidence: None,
        diagnostic: Some(ExecutionDiagnostic {
            code: noncompletion.code.clone(),
            message: noncompletion.message.clone(),
        }),
    };
    let outcomes = report_outcomes(&prepared.request.assertions, outcome, false);
    finish_bundle(prepared, result, outcomes, Vec::new(), MAX_AGGREGATE_BYTES)
}

#[allow(clippy::too_many_arguments)]
pub fn verify_kicad10_board_analysis(
    design: &Design,
    snapshot_bytes: &[u8],
    variant_path: &str,
    compiled: FabricationCompilerArtifacts<'_>,
    product: &ProductArtifactBundle,
    analysis_path: &str,
    kicad_identity_map_json: &str,
    fabrication: &FabricationManifestBundle,
    evidence: &BoardAnalysisHostEvidence,
    supplied: &BoardAnalysisBundle,
) -> Result<(), BoardAnalysisDiagnostic> {
    let expected = bind_kicad10_board_analysis(
        design,
        snapshot_bytes,
        variant_path,
        compiled,
        product,
        analysis_path,
        kicad_identity_map_json,
        fabrication,
        evidence,
    )?;
    if supplied != &expected {
        return Err(diagnostic(
            "CC-BOARD-ANALYSIS-VERIFY-001",
            "bundle",
            "board-analysis bundle does not match authoritative recomputation",
        ));
    }
    Ok(())
}

#[allow(clippy::too_many_arguments)]
pub fn verify_kicad10_board_analysis_noncompletion(
    design: &Design,
    snapshot_bytes: &[u8],
    variant_path: &str,
    compiled: FabricationCompilerArtifacts<'_>,
    product: &ProductArtifactBundle,
    analysis_path: &str,
    kicad_identity_map_json: &str,
    fabrication: &FabricationManifestBundle,
    noncompletion: &BoardAnalysisNoncompletion,
    supplied: &BoardAnalysisBundle,
) -> Result<(), BoardAnalysisDiagnostic> {
    let expected = record_kicad10_board_analysis_noncompletion(
        design,
        snapshot_bytes,
        variant_path,
        compiled,
        product,
        analysis_path,
        kicad_identity_map_json,
        fabrication,
        noncompletion,
    )?;
    if supplied != &expected {
        return Err(diagnostic(
            "CC-BOARD-ANALYSIS-VERIFY-001",
            "bundle",
            "board-analysis noncompletion does not match authoritative recomputation",
        ));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use std::env;
    use std::fs;
    use std::path::PathBuf;

    use serde_json::{Value, json};

    use crate::frontend::{compile_source, compile_source_checked};
    use crate::manufacturing::{FabricationHostFile, bind_kicad10_fabrication};
    use crate::product::compile_product_artifacts;
    use crate::release::{
        ReleaseAnalysisEvidence, ReleaseFabricationEvidence, ReleaseInputs, ReleaseRoutingEvidence,
        ReleaseToolchainEvidence, assemble_release, bind_release, verify_release,
    };
    use crate::simulation::assert::evaluate_assertions;
    use crate::simulation::{canonical_f64, parse_result};
    use crate::{CompiledArtifacts, RelativeArtifactPath};

    use super::*;

    const SNAPSHOT: &[u8] = include_bytes!("../../catalogs/reference-catalog.json");
    const SOURCE: &str = include_str!("../../examples/voltage_divider.circuitc");
    const ANALYSIS: &str = "release.manufacturability";
    const FABRICATION_ASSERTION: &str = "release.manufacturability.fabrication";
    const FABRICATION_EXECUTABLE: &[u8] = b"fabrication-kicad-10.0.5";
    const ANALYSIS_EXECUTABLE: &[u8] = b"analysis-kicad-10.0.5";
    const NORMALIZER: &[u8] = b"analysis-normalizer-v1";
    const HOST_RUNNER: &[u8] = b"analysis-host-runner-v1";

    struct Fixture {
        design: Design,
        compiled: CompiledArtifacts,
        identity_map: String,
        product: ProductArtifactBundle,
        fabrication: FabricationManifestBundle,
    }

    fn path(value: String) -> RelativeArtifactPath {
        RelativeArtifactPath::try_new(value).unwrap()
    }

    fn runfile(name: &str) -> Vec<u8> {
        let root = env::var_os("RUNFILES_DIR")
            .or_else(|| env::var_os("TEST_SRCDIR"))
            .map(PathBuf::from)
            .expect("Bazel test runfiles directory is available");
        fs::read(root.join("_main").join(name)).expect("required release tool runfile is readable")
    }

    fn layer_specs() -> Vec<(&'static str, &'static str, &'static str, &'static str)> {
        vec![
            ("F_Cu", "F.Cu", "Copper,L1,Top", "Copper,L1,Top"),
            ("F_Mask", "F.Mask", "Soldermask,Top", "SolderMask,Top"),
            ("B_Cu", "B.Cu", "Copper,L2,Bot", "Copper,L2,Bot"),
            ("B_Mask", "B.Mask", "Soldermask,Bot", "SolderMask,Bot"),
            ("F_Silkscreen", "F.SilkS", "Legend,Top", "Legend,Top"),
            ("B_Silkscreen", "B.SilkS", "Legend,Bot", "Legend,Bot"),
            ("F_Paste", "F.Paste", "Paste,Top", "SolderPaste,Top"),
            ("B_Paste", "B.Paste", "Paste,Bot", "SolderPaste,Bot"),
            ("Edge_Cuts", "Edge.Cuts", "Profile,NP", "Profile"),
        ]
    }

    fn layer_polarity(layer_name: &str) -> &'static str {
        if matches!(layer_name, "F.Mask" | "B.Mask") {
            "Negative"
        } else {
            "Positive"
        }
    }

    fn raw_fabrication_files() -> Vec<FabricationHostFile> {
        let design_name = "voltage_divider";
        let mut files = Vec::new();
        for (stem, layer_name, function, _) in layer_specs() {
            let polarity = if layer_name == "Edge.Cuts" {
                String::new()
            } else {
                format!("%TF.FilePolarity,{}*%\n", layer_polarity(layer_name))
            };
            files.push(FabricationHostFile {
                path: path(format!("gerber/{design_name}-{stem}.gbr")),
                contents: format!(
                    "%TF.GenerationSoftware,KiCad,Pcbnew,10.0.5*%\n%TF.CreationDate,2026-08-04T08:00:01-07:00*%\n%TF.ProjectId,{design_name},00000000-0000-0000-0000-000000000000,rev?*%\n%TF.SameCoordinates,Original*%\n%TF.FileFunction,{function}*%\n{polarity}%FSLAX46Y46*%\nG04 Gerber Fmt 4.6, Leading zero omitted, Abs format (unit mm)*\nG04 Created by KiCad (PCBNEW 10.0.5) date 2026-08-04 08:00:01*\n%MOMM*%\n%LPD*%\nG01*\nM02*\n"
                )
                .into_bytes(),
            });
        }
        let attributes: Vec<_> = layer_specs()
            .into_iter()
            .map(|(stem, layer_name, _, job_function)| {
                json!({
                    "Path": format!("{design_name}-{stem}.gbr"),
                    "FileFunction": job_function,
                    "FilePolarity": layer_polarity(layer_name),
                    "layer_name_for_test": layer_name,
                })
            })
            .map(|mut attribute| {
                attribute
                    .as_object_mut()
                    .unwrap()
                    .remove("layer_name_for_test");
                attribute
            })
            .collect();
        let job = json!({
            "Header": {
                "GenerationSoftware": {
                    "Vendor": "KiCad",
                    "Application": "Pcbnew",
                    "Version": "10.0.5"
                },
                "CreationDate": "2026-08-04T08:00:01-07:00"
            },
            "GeneralSpecs": {"ProjectId": {"Name": design_name}},
            "FilesAttributes": attributes
        });
        let mut job_json = serde_json::to_string_pretty(&job).unwrap();
        job_json.push('\n');
        files.push(FabricationHostFile {
            path: path(format!("gerber/{design_name}-job.gbrjob")),
            contents: job_json.into_bytes(),
        });
        for (suffix, function) in [("NPTH", "NonPlated,1,2,NPTH"), ("PTH", "Plated,1,2,PTH")] {
            files.push(FabricationHostFile {
                path: path(format!("drill/{design_name}-{suffix}.drl")),
                contents: format!(
                    "M48\n; DRILL file KiCad 10.0.5 date 2026-08-04T08:00:01\n; FORMAT={{-:-/ absolute / metric / decimal}}\n; #@! TF.CreationDate,2026-08-04T08:00:01-07:00\n; #@! TF.GenerationSoftware,Kicad,Pcbnew,10.0.5\n; #@! TF.FileFunction,{function}\nFMAT,2\nMETRIC\n%\nG90\nG05\nM30\n"
                )
                .into_bytes(),
            });
        }
        files.push(FabricationHostFile {
            path: path(format!("position/{design_name}-all-pos.csv")),
            contents: b"Ref,Val,Package,PosX,PosY,Rot,Side\n\"R1\",\"10k\xce\xa9\",\"R_0603_1608Metric\",15.000000,-10.000000,0.000000,top\n\"R2\",\"10k\xce\xa9\",\"R_0603_1608Metric\",25.000000,-10.000000,0.000000,top\n".to_vec(),
        });
        files
    }

    fn fixture() -> Fixture {
        let source = compile_source("input.circuitc", SOURCE).unwrap();
        let design = source.elaborated.design;
        let compiled = source.artifacts;
        let identity_map = source.kicad_identity_map;
        let product = compile_product_artifacts(&design, SNAPSHOT, "production").unwrap();
        let fabrication = bind_kicad10_fabrication(
            &design,
            SNAPSHOT,
            "production",
            FabricationCompilerArtifacts::Static(&compiled),
            &product,
            ANALYSIS,
            FABRICATION_ASSERTION,
            ADAPTER_VERSION,
            FABRICATION_EXECUTABLE,
            &raw_fabrication_files(),
        )
        .unwrap();
        Fixture {
            design,
            compiled,
            identity_map,
            product,
            fabrication,
        }
    }

    fn pretty(value: &Value) -> Vec<u8> {
        let mut rendered = serde_json::to_string_pretty(value).unwrap();
        rendered.push('\n');
        rendered.into_bytes()
    }

    fn ignored(keys: &[&str]) -> Vec<Value> {
        keys.iter()
            .map(|key| json!({"description": format!("policy {key}"), "key": key}))
            .collect()
    }

    fn evidence_for(
        design: &Design,
        compiler: FabricationCompilerArtifacts<'_>,
        product: &ProductArtifactBundle,
        identity_map: &str,
        fabrication: &FabricationManifestBundle,
    ) -> BoardAnalysisHostEvidence {
        let prepared = prepare(
            design,
            SNAPSHOT,
            "production",
            compiler,
            product,
            ANALYSIS,
            identity_map,
            fabrication,
        )
        .unwrap();
        let schematic_sha = prepared.request.kicad_schematic.sha256.clone();
        let pcb_sha = prepared.request.kicad_pcb.sha256.clone();
        let expected_sheets = prepared
            .request
            .expected_sheets
            .iter()
            .map(
                |sheet| json!({"path": sheet.path, "uuid_path": sheet.uuid_path, "violations": []}),
            )
            .collect::<Vec<_>>();
        let erc = pretty(&json!({
            "coordinate_units": "mm",
            "host": {"major": 10, "name": "kicad", "version": "10.0.5"},
            "ignored_checks": ignored(&[
                "footprint_filter",
                "four_way_junction",
                "simulation_model_issue",
                "single_global_label"
            ]),
            "included_severities": ["error", "exclusion", "warning"],
            "report_kind": "erc",
            "schema_version": 1,
            "sheets": expected_sheets,
            "source": "voltage_divider.kicad_sch",
            "source_sha256": schematic_sha,
        }));
        let drc = pretty(&json!({
            "coordinate_units": "mm",
            "host": {"major": 10, "name": "kicad", "version": "10.0.5"},
            "ignored_checks": ignored(&[
                "footprint_filters_mismatch",
                "footprint_type_mismatch",
                "missing_courtyard",
                "track_not_centered_on_via",
                "tuning_profile_track_geometries"
            ]),
            "included_severities": ["error", "exclusion", "warning"],
            "report_kind": "drc",
            "schema_version": 1,
            "schematic_parity": [],
            "source": "voltage_divider.kicad_pcb",
            "source_sha256": pcb_sha,
            "unconnected_items": [],
            "violations": [],
        }));
        let receipt = Receipt {
            schema_name: RECEIPT_SCHEMA.to_owned(),
            schema_version: SCHEMA_VERSION,
            request_sha256: sha256_hex(prepared.request_json.as_bytes()),
            schematic_sha256: prepared.request.kicad_schematic.sha256,
            pcb_sha256: prepared.request.kicad_pcb.sha256,
            identity_map_sha256: prepared.request.kicad_identity_map.sha256,
            executable_sha256: sha256_hex(ANALYSIS_EXECUTABLE),
            normalizer_sha256: sha256_hex(NORMALIZER),
            host_runner_sha256: sha256_hex(HOST_RUNNER),
            erc_sha256: sha256_hex(&erc),
            drc_sha256: sha256_hex(&drc),
        };
        BoardAnalysisHostEvidence {
            host_version: ADAPTER_VERSION.to_owned(),
            host_executable: ANALYSIS_EXECUTABLE.to_vec(),
            normalizer: NORMALIZER.to_vec(),
            host_runner: HOST_RUNNER.to_vec(),
            erc_report_json: erc,
            drc_report_json: drc,
            receipt_json: canonical_json(&receipt, "receipt").unwrap().into_bytes(),
        }
    }

    fn evidence(fixture: &Fixture) -> BoardAnalysisHostEvidence {
        evidence_for(
            &fixture.design,
            FabricationCompilerArtifacts::Static(&fixture.compiled),
            &fixture.product,
            &fixture.identity_map,
            &fixture.fabrication,
        )
    }

    fn refresh_receipt_for(
        design: &Design,
        compiler: FabricationCompilerArtifacts<'_>,
        product: &ProductArtifactBundle,
        identity_map: &str,
        fabrication: &FabricationManifestBundle,
        evidence: &mut BoardAnalysisHostEvidence,
    ) {
        let prepared = prepare(
            design,
            SNAPSHOT,
            "production",
            compiler,
            product,
            ANALYSIS,
            identity_map,
            fabrication,
        )
        .unwrap();
        let receipt = Receipt {
            schema_name: RECEIPT_SCHEMA.to_owned(),
            schema_version: SCHEMA_VERSION,
            request_sha256: sha256_hex(prepared.request_json.as_bytes()),
            schematic_sha256: prepared.request.kicad_schematic.sha256,
            pcb_sha256: prepared.request.kicad_pcb.sha256,
            identity_map_sha256: prepared.request.kicad_identity_map.sha256,
            executable_sha256: sha256_hex(&evidence.host_executable),
            normalizer_sha256: sha256_hex(&evidence.normalizer),
            host_runner_sha256: sha256_hex(&evidence.host_runner),
            erc_sha256: sha256_hex(&evidence.erc_report_json),
            drc_sha256: sha256_hex(&evidence.drc_report_json),
        };
        evidence.receipt_json = canonical_json(&receipt, "receipt").unwrap().into_bytes();
    }

    fn refresh_receipt(fixture: &Fixture, evidence: &mut BoardAnalysisHostEvidence) {
        refresh_receipt_for(
            &fixture.design,
            FabricationCompilerArtifacts::Static(&fixture.compiled),
            &fixture.product,
            &fixture.identity_map,
            &fixture.fabrication,
            evidence,
        );
    }

    fn bind_fixture(
        fixture: &Fixture,
        evidence: &BoardAnalysisHostEvidence,
    ) -> BoardAnalysisBundle {
        bind_kicad10_board_analysis(
            &fixture.design,
            SNAPSHOT,
            "production",
            FabricationCompilerArtifacts::Static(&fixture.compiled),
            &fixture.product,
            ANALYSIS,
            &fixture.identity_map,
            &fixture.fabrication,
            evidence,
        )
        .unwrap()
    }

    fn request_preimage(request: &BoardAnalysisRequest) -> RequestPreimage {
        RequestPreimage {
            design_name: request.design_name.clone(),
            analysis_path: request.analysis_path.clone(),
            adapter: request.adapter.clone(),
            expected_major: request.expected_major,
            expected_version: request.expected_version.clone(),
            assertions: request.assertions.clone(),
            kicad_schematic: request.kicad_schematic.clone(),
            kicad_pcb: request.kicad_pcb.clone(),
            kicad_identity_map: request.kicad_identity_map.clone(),
            expected_sheets: request.expected_sheets.clone(),
            project_support: request.project_support.clone(),
            fabrication_request: request.fabrication_request.clone(),
            fabrication_manifest: request.fabrication_manifest.clone(),
            policy: request.policy.clone(),
            resources: request.resources.clone(),
            outputs: request.outputs.clone(),
        }
    }

    fn finding_item(fixture: &Fixture, index: usize, description: &str) -> Value {
        let identity = fixture
            .compiled
            .kicad_identities
            .iter()
            .filter(|identity| identity.semantic_path != "design.schematic")
            .nth(index)
            .unwrap();
        json!({
            "circuitc": {
                "semantic_path": identity.semantic_path,
                "source": "voltage_divider.circuitc"
            },
            "description": description,
            "uuid": identity.uuid
        })
    }

    fn dirty_finding(fixture: &Fixture, kind: &str) -> Value {
        json!({
            "description": format!("test {kind} finding"),
            "items": [finding_item(fixture, 0, "authenticated test item")],
            "severity": "error",
            "type": kind
        })
    }

    #[test]
    fn completed_analysis_binds_five_distinct_capabilities() {
        let fixture = fixture();
        let evidence = evidence(&fixture);
        let first = bind_fixture(&fixture, &evidence);
        let second = bind_fixture(&fixture, &evidence);
        assert_eq!(first, second);
        assert_eq!(first.files().len(), 2);
        let report: Value = serde_json::from_str(first.report_json()).unwrap();
        let outcomes = report["outcomes"].as_array().unwrap();
        assert_eq!(outcomes.len(), 5);
        assert_eq!(
            outcomes
                .iter()
                .map(|outcome| outcome["capability"].as_str().unwrap())
                .collect::<Vec<_>>(),
            vec![
                "erc_clean",
                "drc_clean",
                "unconnected_clean",
                "schematic_parity_clean",
                "fabrication_inventory_complete"
            ]
        );
        assert!(outcomes.iter().all(|outcome| outcome["outcome"] == "pass"));
        assert_eq!(outcomes[0]["evidence_role"], "erc");
        assert_eq!(outcomes[1]["evidence_role"], "drc");
        assert_eq!(outcomes[2]["evidence_role"], "drc");
        assert_eq!(outcomes[3]["evidence_role"], "drc");
        assert_eq!(outcomes[4]["evidence_role"], "fabrication_manifest");
        verify_kicad10_board_analysis(
            &fixture.design,
            SNAPSHOT,
            "production",
            FabricationCompilerArtifacts::Static(&fixture.compiled),
            &fixture.product,
            ANALYSIS,
            &fixture.identity_map,
            &fixture.fabrication,
            &evidence,
            &first,
        )
        .unwrap();
    }

    #[test]
    fn release_closure_reverifies_complete_static_predecessor_graph() {
        let fixture = fixture();
        let mut host_evidence = evidence(&fixture);
        host_evidence.host_executable = FABRICATION_EXECUTABLE.to_vec();
        refresh_receipt(&fixture, &mut host_evidence);
        let analysis = bind_fixture(&fixture, &host_evidence);
        let fabrication_host_files = raw_fabrication_files();
        let toolchain = ReleaseToolchainEvidence {
            ohmnivore_executable: None,
            ohmnivore_provenance: None,
            apgar_executable: None,
            apgar_provenance: None,
        };
        let inputs = ReleaseInputs {
            source: SOURCE,
            design: &fixture.design,
            catalog_snapshot: SNAPSHOT,
            variant_path: "production",
            compiler: FabricationCompilerArtifacts::Static(&fixture.compiled),
            kicad_identity_map_json: &fixture.identity_map,
            product: &fixture.product,
            fabrication: ReleaseFabricationEvidence {
                analysis_path: ANALYSIS,
                assertion_path: FABRICATION_ASSERTION,
                host_version: ADAPTER_VERSION,
                host_executable: FABRICATION_EXECUTABLE,
                host_files: &fabrication_host_files,
                bundle: &fixture.fabrication,
            },
            analysis: ReleaseAnalysisEvidence {
                analysis_path: ANALYSIS,
                host: &host_evidence,
                bundle: &analysis,
            },
            routing: None,
            tools: toolchain,
        };

        let first = bind_release(&inputs).unwrap();
        let second = bind_release(&inputs).unwrap();
        assert_eq!(first, second);
        assert_eq!(first.files().first().unwrap().path.as_str(), "request.json");
        assert_eq!(first.files().last().unwrap().path.as_str(), "manifest.json");
        assert!(first.root().as_str().starts_with("release/"));
        assert!(first.request_json().ends_with('\n'));
        assert!(first.manifest_json().ends_with('\n'));
        verify_release(&inputs, &first).unwrap();
        let assembled = assemble_release(first.root().clone(), first.files().to_vec()).unwrap();
        verify_release(&inputs, &assembled).unwrap();

        let mut changed = first.clone();
        changed.files[1].contents.push(b'x');
        assert_eq!(
            verify_release(&inputs, &changed).unwrap_err().code,
            "CC-RELEASE-VERIFY-001"
        );
        let mut omitted_files = first.files().to_vec();
        omitted_files.remove(1);
        let omitted = assemble_release(first.root().clone(), omitted_files).unwrap();
        assert_eq!(
            verify_release(&inputs, &omitted).unwrap_err().code,
            "CC-RELEASE-VERIFY-001"
        );

        let reject_contract_mutation = |path: &str, contents: String| {
            let mut files = first.files().to_vec();
            let file = files
                .iter_mut()
                .find(|file| file.path.as_str() == path)
                .expect("release contract file is present");
            file.contents = contents.into_bytes();
            let candidate = assemble_release(first.root().clone(), files).unwrap();
            assert_eq!(
                verify_release(&inputs, &candidate).unwrap_err().code,
                "CC-RELEASE-VERIFY-001"
            );
        };
        for contract in [first.request_json(), first.manifest_json()] {
            let path = if contract == first.request_json() {
                "request.json"
            } else {
                "manifest.json"
            };
            reject_contract_mutation(path, format!("{contract} "));
            reject_contract_mutation(path, contract.replacen('{', "{\"unknown\":0,", 1));
            reject_contract_mutation(
                path,
                contract.replacen(
                    "\"schema_name\":",
                    "\"schema_name\":\"duplicate\",\"schema_name\":",
                    1,
                ),
            );
            let value: Value = serde_json::from_str(contract).unwrap();
            reject_contract_mutation(
                path,
                format!("{}\n", serde_json::to_string_pretty(&value).unwrap()),
            );
        }

        let changed_source = format!("// release-significant comment\n{SOURCE}");
        let stale_source_inputs = ReleaseInputs {
            source: &changed_source,
            ..inputs
        };
        assert_eq!(
            bind_release(&stale_source_inputs).unwrap_err().code,
            "CC-RELEASE-SOURCE-001"
        );
        let semantically_changed_source =
            SOURCE.replacen("resistance 10 kohm", "resistance 11 kohm", 1);
        let stale_semantic_inputs = ReleaseInputs {
            source: &semantically_changed_source,
            ..inputs
        };
        assert_eq!(
            bind_release(&stale_semantic_inputs).unwrap_err().code,
            "CC-RELEASE-SOURCE-001"
        );

        let mut stale_compiled = fixture.compiled.clone();
        stale_compiled.kicad_pcb.push('\n');
        let stale_compiler_inputs = ReleaseInputs {
            compiler: FabricationCompilerArtifacts::Static(&stale_compiled),
            ..inputs
        };
        assert_eq!(
            bind_release(&stale_compiler_inputs).unwrap_err().code,
            "CC-RELEASE-COMPILER-001"
        );

        let mut failing_host = host_evidence.clone();
        let mut drc: Value = serde_json::from_slice(&failing_host.drc_report_json).unwrap();
        drc["violations"] = Value::Array(vec![dirty_finding(&fixture, "clearance")]);
        failing_host.drc_report_json = pretty(&drc);
        refresh_receipt(&fixture, &mut failing_host);
        let failing_analysis = bind_fixture(&fixture, &failing_host);
        let failing_analysis_inputs = ReleaseInputs {
            analysis: ReleaseAnalysisEvidence {
                analysis_path: ANALYSIS,
                host: &failing_host,
                bundle: &failing_analysis,
            },
            ..inputs
        };
        assert_eq!(
            bind_release(&failing_analysis_inputs).unwrap_err().code,
            "CC-RELEASE-ANALYSIS-001"
        );

        let wrong_analysis_path_inputs = ReleaseInputs {
            analysis: ReleaseAnalysisEvidence {
                analysis_path: "release.other",
                ..inputs.analysis
            },
            ..inputs
        };
        let diagnostic = bind_release(&wrong_analysis_path_inputs).unwrap_err();
        assert_eq!(diagnostic.code, "CC-RELEASE-ANALYSIS-001");
        assert_eq!(diagnostic.path, "analysis_path");
        assert_eq!(
            diagnostic.message,
            "fabrication and board-analysis evidence do not name the same analysis"
        );
        let mut mismatched_host = host_evidence.clone();
        mismatched_host.host_executable.push(b'x');
        let mismatched_tool_inputs = ReleaseInputs {
            analysis: ReleaseAnalysisEvidence {
                host: &mismatched_host,
                ..inputs.analysis
            },
            ..inputs
        };
        assert_eq!(
            bind_release(&mismatched_tool_inputs).unwrap_err().code,
            "CC-RELEASE-TOOL-001"
        );
    }

    fn exercise_checked_release_case(simulation: bool, routing_applicable: bool) {
        let mut source = SOURCE.to_owned();
        if simulation {
            source = source.replacen(
                "  manufacturability release.manufacturability",
                "  analysis dc_operating_point simulation.dc;\n  assert net_voltage checks.vout analysis simulation.dc net VOUT sample scalar expected 5 V absolute_tolerance 0.001 V relative_tolerance 0 ratio;\n\n  manufacturability release.manufacturability",
                1,
            );
        }
        if routing_applicable {
            source = source.replacen(
                "route board.routes.vout_bridge net VOUT from (16 mm, 10 mm) to (24 mm, 10 mm) width 0.25 mm layer front;",
                "autoroute board.autoroute.vout net VOUT width 0.25 mm clearance 0.2 mm grid 1 mm layer front;",
                1,
            );
        }
        let work_root = env::var_os("TEST_TMPDIR")
            .map(PathBuf::from)
            .unwrap_or_else(env::temp_dir)
            .join(format!("release-checked-{simulation}-{routing_applicable}"));
        let compiled_source =
            compile_source_checked("input.circuitc", &source, &work_root).unwrap();
        let design = &compiled_source.elaborated.design;
        let checked = &compiled_source.artifacts;
        let identity_map = &compiled_source.kicad_identity_map;
        assert_eq!(checked.simulations().len(), usize::from(simulation));
        assert_eq!(checked.routing().is_some(), routing_applicable);

        let product = compile_product_artifacts(design, SNAPSHOT, "production").unwrap();
        let fabrication_host_files = raw_fabrication_files();
        let fabrication = bind_kicad10_fabrication(
            design,
            SNAPSHOT,
            "production",
            FabricationCompilerArtifacts::Checked(checked),
            &product,
            ANALYSIS,
            FABRICATION_ASSERTION,
            ADAPTER_VERSION,
            FABRICATION_EXECUTABLE,
            &fabrication_host_files,
        )
        .unwrap();
        let mut host_evidence = evidence_for(
            design,
            FabricationCompilerArtifacts::Checked(checked),
            &product,
            identity_map,
            &fabrication,
        );
        host_evidence.host_executable = FABRICATION_EXECUTABLE.to_vec();
        refresh_receipt_for(
            design,
            FabricationCompilerArtifacts::Checked(checked),
            &product,
            identity_map,
            &fabrication,
            &mut host_evidence,
        );
        let analysis = bind_kicad10_board_analysis(
            design,
            SNAPSHOT,
            "production",
            FabricationCompilerArtifacts::Checked(checked),
            &product,
            ANALYSIS,
            identity_map,
            &fabrication,
            &host_evidence,
        )
        .unwrap();

        let ohmnivore_executable = runfile("ohmnivore-cpu");
        let ohmnivore_provenance = runfile("ohmnivore-provenance.txt");
        let apgar_executable = runfile("apgar_route_adapter");
        let apgar_provenance = runfile("apgar-route-provenance.txt");
        let mut acceptance_json = if let Some(routing) = checked.routing() {
            let verified_apgar_json = crate::routing::evidence::verify(
                &routing.request_json,
                &routing.result_json,
                std::str::from_utf8(&apgar_provenance).unwrap(),
            )
            .unwrap();
            let verified_apgar: Value = serde_json::from_str(&verified_apgar_json).unwrap();
            let erc = analysis
                .files()
                .iter()
                .find(|file| {
                    file.path
                        .as_str()
                        .ends_with("/evidence/erc.normalized.json")
                })
                .unwrap();
            let drc = analysis
                .files()
                .iter()
                .find(|file| {
                    file.path
                        .as_str()
                        .ends_with("/evidence/drc.normalized.json")
                })
                .unwrap();
            serde_json::to_string(&json!({
            "authorities": {
                "apgar_exact_admission": true,
                "kicad_drc_clean": true,
                "kicad_erc_clean": true,
                "kicad_schematic_parity_clean": true,
                "kicad_unconnected_clean": true
            },
            "candidate": {
                "geometry_signature": verified_apgar["candidate_geometry_signature"],
                "payload_checksum": verified_apgar["candidate_payload_checksum"],
                "resource_signature": verified_apgar["candidate_resource_signature"]
            },
            "design_name": design.name,
            "kicad": {
                "drc_filename": "drc.normalized.json",
                "drc_sha256": sha256_hex(&drc.contents),
                "erc_filename": "erc.normalized.json",
                "erc_sha256": sha256_hex(&erc.contents),
                "host": {"major": 10, "name": "kicad", "version": ADAPTER_VERSION},
                "pcb_filename": format!("{}.kicad_pcb", design.name),
                "pcb_sha256": sha256_hex(checked.static_artifacts().kicad_pcb.as_bytes()),
                "schematic_filename": format!("{}.kicad_sch", design.name),
                "schematic_sha256": sha256_hex(checked.static_artifacts().kicad_schematic.as_bytes())
            },
            "projection_sha256": routing.projection_sha256,
            "request_identity_sha256": routing.request_identity_sha256,
            "request_path": design.board.routing_requests[0].path,
            "request_sha256": routing.request_sha256,
            "result_sha256": routing.result_sha256,
            "schema_name": "circuitc.apgar_route_acceptance",
            "schema_version": 1,
            "selected_candidate_id": routing.selected_candidate_id,
            "tool": verified_apgar["tool"],
            "tool_provenance_sha256": sha256_hex(&apgar_provenance)
            }))
            .unwrap()
        } else {
            String::new()
        };
        if routing_applicable {
            acceptance_json.push('\n');
        }

        let inputs = ReleaseInputs {
            source: &source,
            design,
            catalog_snapshot: SNAPSHOT,
            variant_path: "production",
            compiler: FabricationCompilerArtifacts::Checked(checked),
            kicad_identity_map_json: identity_map,
            product: &product,
            fabrication: ReleaseFabricationEvidence {
                analysis_path: ANALYSIS,
                assertion_path: FABRICATION_ASSERTION,
                host_version: ADAPTER_VERSION,
                host_executable: FABRICATION_EXECUTABLE,
                host_files: &fabrication_host_files,
                bundle: &fabrication,
            },
            analysis: ReleaseAnalysisEvidence {
                analysis_path: ANALYSIS,
                host: &host_evidence,
                bundle: &analysis,
            },
            routing: routing_applicable.then_some(ReleaseRoutingEvidence {
                acceptance_json: &acceptance_json,
            }),
            tools: ReleaseToolchainEvidence {
                ohmnivore_executable: simulation.then_some(ohmnivore_executable.as_slice()),
                ohmnivore_provenance: simulation.then_some(ohmnivore_provenance.as_slice()),
                apgar_executable: routing_applicable.then_some(apgar_executable.as_slice()),
                apgar_provenance: routing_applicable.then_some(apgar_provenance.as_slice()),
            },
        };
        let release = bind_release(&inputs).unwrap();
        verify_release(&inputs, &release).unwrap();
        let request: Value = serde_json::from_str(release.request_json()).unwrap();
        let mut expected_tools = vec!["kicad", "analysis_normalizer", "analysis_host_runner"];
        if simulation {
            expected_tools.push("ohmnivore");
        }
        if routing_applicable {
            expected_tools.push("apgar");
        }
        assert_eq!(
            request["tools"]
                .as_array()
                .unwrap()
                .iter()
                .map(|tool| tool["role"].as_str().unwrap())
                .collect::<Vec<_>>(),
            expected_tools
        );
        assert_eq!(request["applicability"]["simulation"], simulation);
        assert_eq!(request["applicability"]["routing"], routing_applicable);
        let artifact_roles = request["artifacts"]
            .as_array()
            .unwrap()
            .iter()
            .map(|artifact| artifact["role"].as_str().unwrap())
            .collect::<Vec<_>>();
        for role in [
            "simulation_netlist",
            "simulation_request",
            "simulation_identity_map",
            "simulation_result",
            "simulation_report",
            "ohmnivore_provenance",
        ] {
            assert_eq!(artifact_roles.contains(&role), simulation, "{role}");
        }
        for role in [
            "routing_request",
            "routing_result",
            "routing_projection",
            "routing_acceptance",
            "apgar_provenance",
        ] {
            assert_eq!(artifact_roles.contains(&role), routing_applicable, "{role}");
        }
        let manifest: Value = serde_json::from_str(release.manifest_json()).unwrap();
        let validation_capabilities = manifest["validations"]
            .as_array()
            .unwrap()
            .iter()
            .map(|validation| validation["capability"].as_str().unwrap())
            .collect::<Vec<_>>();
        assert_eq!(validation_capabilities.contains(&"simulation"), simulation);
        assert_eq!(
            validation_capabilities.contains(&"routing"),
            routing_applicable
        );

        if simulation {
            let mut stale_checked = (*checked).clone();
            stale_checked.static_artifacts_mut().kicad_pcb.push('\n');
            let stale_checked_inputs = ReleaseInputs {
                compiler: FabricationCompilerArtifacts::Checked(&stale_checked),
                ..inputs
            };
            assert_eq!(
                bind_release(&stale_checked_inputs).unwrap_err().code,
                "CC-RELEASE-COMPILER-001"
            );

            let mut failing_checked = (*checked).clone();
            let simulation_evidence = &mut failing_checked.simulations_mut()[0];
            let mut failing_result = parse_result(&simulation_evidence.result_json).unwrap();
            for signal in &mut failing_result.signals {
                for value in &mut signal.values {
                    *value = canonical_f64(6.0).unwrap();
                }
            }
            let failing_result_json = failing_result.to_canonical_json().unwrap();
            let failing_evaluation = evaluate_assertions(
                simulation_evidence.request_json.as_bytes(),
                simulation_evidence.spice_identity_map_json.as_bytes(),
                failing_result_json.as_bytes(),
            )
            .unwrap();
            assert!(!failing_evaluation.checked_success);
            simulation_evidence.result_json = failing_result_json;
            simulation_evidence.report_json = failing_evaluation.report_json;
            let failing_simulation_inputs = ReleaseInputs {
                compiler: FabricationCompilerArtifacts::Checked(&failing_checked),
                ..inputs
            };
            assert_eq!(
                bind_release(&failing_simulation_inputs).unwrap_err().code,
                "CC-RELEASE-SIMULATION-001"
            );

            let mut wrong_ohmnivore = ohmnivore_executable.clone();
            wrong_ohmnivore.push(0);
            let stale_tool_inputs = ReleaseInputs {
                tools: ReleaseToolchainEvidence {
                    ohmnivore_executable: Some(&wrong_ohmnivore),
                    ..inputs.tools
                },
                ..inputs
            };
            assert_eq!(
                bind_release(&stale_tool_inputs).unwrap_err().code,
                "CC-RELEASE-TOOL-001"
            );
            let missing_tool_inputs = ReleaseInputs {
                tools: ReleaseToolchainEvidence {
                    ohmnivore_executable: None,
                    ohmnivore_provenance: None,
                    ..inputs.tools
                },
                ..inputs
            };
            assert_eq!(
                bind_release(&missing_tool_inputs).unwrap_err().code,
                "CC-RELEASE-APPLICABILITY-001"
            );
        }

        if routing_applicable {
            let zero_digest = "0".repeat(64);
            let apgar_digest = sha256_hex(&apgar_executable);
            let tool_identity_mutant = acceptance_json.replacen(
                &format!("\"executable_sha256\":\"{apgar_digest}\""),
                &format!("\"executable_sha256\":\"{zero_digest}\""),
                1,
            );
            let provenance_digest = sha256_hex(&apgar_provenance);
            let provenance_identity_mutant = acceptance_json.replacen(
                &format!("\"tool_provenance_sha256\":\"{provenance_digest}\""),
                &format!("\"tool_provenance_sha256\":\"{zero_digest}\""),
                1,
            );
            for rejected_acceptance in [
                acceptance_json.replacen(
                    "\"apgar_exact_admission\":true",
                    "\"apgar_exact_admission\":false",
                    1,
                ),
                acceptance_json.replacen(
                    "\"geometry_signature\":\"",
                    "\"geometry_signature\":\"0",
                    1,
                ),
                acceptance_json.replacen(
                    "\"drc_filename\":\"drc.normalized.json\"",
                    "\"drc_filename\":\"wrong.json\"",
                    1,
                ),
                tool_identity_mutant,
                provenance_identity_mutant,
            ] {
                let rejected_route_inputs = ReleaseInputs {
                    routing: Some(ReleaseRoutingEvidence {
                        acceptance_json: &rejected_acceptance,
                    }),
                    ..inputs
                };
                assert_eq!(
                    bind_release(&rejected_route_inputs).unwrap_err().code,
                    "CC-RELEASE-ROUTING-001"
                );
            }
            let mut wrong_apgar = apgar_executable.clone();
            wrong_apgar.push(0);
            let stale_apgar_inputs = ReleaseInputs {
                tools: ReleaseToolchainEvidence {
                    apgar_executable: Some(&wrong_apgar),
                    ..inputs.tools
                },
                ..inputs
            };
            assert_eq!(
                bind_release(&stale_apgar_inputs).unwrap_err().code,
                "CC-RELEASE-TOOL-001"
            );
            for incomplete_inputs in [
                ReleaseInputs {
                    routing: None,
                    ..inputs
                },
                ReleaseInputs {
                    tools: ReleaseToolchainEvidence {
                        apgar_provenance: None,
                        ..inputs.tools
                    },
                    ..inputs
                },
            ] {
                assert_eq!(
                    bind_release(&incomplete_inputs).unwrap_err().code,
                    "CC-RELEASE-APPLICABILITY-001"
                );
            }
        }
    }

    #[test]
    fn release_closure_covers_every_checked_applicability_combination() {
        exercise_checked_release_case(true, false);
        exercise_checked_release_case(false, true);
        exercise_checked_release_case(true, true);
    }

    #[test]
    fn project_support_is_exact_deterministic_and_identity_bound_per_entry() {
        let fixture = fixture();
        let first = prepare(
            &fixture.design,
            SNAPSHOT,
            "production",
            FabricationCompilerArtifacts::Static(&fixture.compiled),
            &fixture.product,
            ANALYSIS,
            &fixture.identity_map,
            &fixture.fabrication,
        )
        .unwrap();
        let second = prepare(
            &fixture.design,
            SNAPSHOT,
            "production",
            FabricationCompilerArtifacts::Static(&fixture.compiled),
            &fixture.product,
            ANALYSIS,
            &fixture.identity_map,
            &fixture.fabrication,
        )
        .unwrap();
        assert_eq!(first.request_json, second.request_json);
        let mut expected = vec![
            (
                format!("{}.kicad_pro", fixture.design.name),
                fixture.compiled.kicad_project.as_bytes().to_vec(),
            ),
            (
                "sym-lib-table".to_owned(),
                fixture.compiled.kicad_symbol_table.as_bytes().to_vec(),
            ),
            (
                "fp-lib-table".to_owned(),
                fixture.compiled.kicad_footprint_table.as_bytes().to_vec(),
            ),
        ];
        expected.extend(fixture.compiled.kicad_library_files.iter().map(|library| {
            (
                library.relative_path.as_str().to_owned(),
                library.contents.as_bytes().to_vec(),
            )
        }));
        expected.sort_by(|left, right| left.0.cmp(&right.0));
        assert_eq!(first.request.project_support.len(), expected.len());
        for (binding, (path, bytes)) in first.request.project_support.iter().zip(&expected) {
            assert_eq!(&binding.path, path);
            assert_eq!(binding.byte_length, bytes.len() as u64);
            assert_eq!(binding.sha256, sha256_hex(bytes));
        }
        let expected_sheets_index = first.request_json.find("\"expected_sheets\"").unwrap();
        let project_support_index = first.request_json.find("\"project_support\"").unwrap();
        let fabrication_request_index = first.request_json.find("\"fabrication_request\"").unwrap();
        assert!(expected_sheets_index < project_support_index);
        assert!(project_support_index < fabrication_request_index);

        let baseline = request_preimage(&first.request);
        assert_eq!(
            analysis_identity_sha256(&baseline).unwrap(),
            first.request.analysis_identity_sha256
        );
        for index in 0..baseline.project_support.len() {
            let mut mutated = baseline.clone();
            let replacement = if mutated.project_support[index].sha256.starts_with('0') {
                "1"
            } else {
                "0"
            };
            mutated.project_support[index]
                .sha256
                .replace_range(..1, replacement);
            assert_ne!(
                analysis_identity_sha256(&mutated).unwrap(),
                first.request.analysis_identity_sha256
            );

            let mut omitted = baseline.clone();
            omitted.project_support.remove(index);
            assert_ne!(
                analysis_identity_sha256(&omitted).unwrap(),
                first.request.analysis_identity_sha256
            );
        }
    }

    #[test]
    fn each_host_capability_failure_is_distinct() {
        let fixture = fixture();
        let baseline = evidence(&fixture);
        let mutations = [
            ("erc", "violations", "erc_clean"),
            ("drc", "violations", "drc_clean"),
            ("drc", "unconnected_items", "unconnected_clean"),
            ("drc", "schematic_parity", "schematic_parity_clean"),
        ];
        for (report, field, failed_capability) in mutations {
            let mut changed = baseline.clone();
            let target = if report == "erc" {
                &mut changed.erc_report_json
            } else {
                &mut changed.drc_report_json
            };
            let mut value: Value = serde_json::from_slice(target).unwrap();
            if report == "erc" {
                value["sheets"][0]["violations"] =
                    Value::Array(vec![dirty_finding(&fixture, field)]);
            } else {
                value[field] = Value::Array(vec![dirty_finding(&fixture, field)]);
            }
            *target = pretty(&value);
            refresh_receipt(&fixture, &mut changed);
            let bundle = bind_fixture(&fixture, &changed);
            let report: Value = serde_json::from_str(bundle.report_json()).unwrap();
            assert_eq!(report["execution_status"], "completed");
            assert_eq!(report["all_pass"], false);
            for outcome in report["outcomes"].as_array().unwrap() {
                let capability = outcome["capability"].as_str().unwrap();
                let expected = if capability == failed_capability {
                    "fail"
                } else {
                    "pass"
                };
                assert_eq!(outcome["outcome"], expected);
                assert!(outcome["evidence_role"].is_string());
            }
        }
    }

    #[test]
    fn reordered_findings_and_items_fail_closed_after_receipt_refresh() {
        let fixture = fixture();
        let baseline = evidence(&fixture);

        let mut reordered_findings = baseline.clone();
        let mut drc: Value = serde_json::from_slice(&reordered_findings.drc_report_json).unwrap();
        let mut findings = vec![
            dirty_finding(&fixture, "clearance_a"),
            dirty_finding(&fixture, "clearance_b"),
        ];
        findings.sort_by_key(|finding| serde_json::to_string(finding).unwrap());
        findings.reverse();
        drc["violations"] = Value::Array(findings);
        reordered_findings.drc_report_json = pretty(&drc);
        refresh_receipt(&fixture, &mut reordered_findings);

        let mut reordered_items = baseline.clone();
        let mut drc: Value = serde_json::from_slice(&reordered_items.drc_report_json).unwrap();
        let mut finding = dirty_finding(&fixture, "clearance");
        let mut items = vec![
            finding_item(&fixture, 0, "item a"),
            finding_item(&fixture, 1, "item b"),
        ];
        items.sort_by_key(|item| serde_json::to_string(item).unwrap());
        items.reverse();
        finding["items"] = Value::Array(items);
        drc["violations"] = Value::Array(vec![finding]);
        reordered_items.drc_report_json = pretty(&drc);
        refresh_receipt(&fixture, &mut reordered_items);

        for evidence in [reordered_findings, reordered_items] {
            assert_eq!(
                bind_kicad10_board_analysis(
                    &fixture.design,
                    SNAPSHOT,
                    "production",
                    FabricationCompilerArtifacts::Static(&fixture.compiled),
                    &fixture.product,
                    ANALYSIS,
                    &fixture.identity_map,
                    &fixture.fabrication,
                    &evidence,
                )
                .unwrap_err()
                .code,
                "CC-BOARD-ANALYSIS-DRC-001"
            );
        }
    }

    #[test]
    fn erc_sheet_coverage_mutants_fail_closed_after_receipt_refresh() {
        let fixture = fixture();
        let baseline = evidence(&fixture);
        let erc: Value = serde_json::from_slice(&baseline.erc_report_json).unwrap();
        let sheet = erc["sheets"][0].clone();
        let mut substituted_path = sheet.clone();
        substituted_path["path"] = json!("/substituted");
        let mut substituted_uuid = sheet.clone();
        substituted_uuid["uuid_path"] = json!(format!(
            "/{}",
            fixture
                .compiled
                .kicad_identities
                .iter()
                .find(|identity| identity.semantic_path != "design.schematic")
                .unwrap()
                .uuid
        ));
        let mutants = [
            Vec::new(),
            vec![sheet.clone(), sheet],
            vec![substituted_path],
            vec![substituted_uuid],
        ];
        for sheets in mutants {
            let mut changed = baseline.clone();
            let mut erc: Value = serde_json::from_slice(&changed.erc_report_json).unwrap();
            erc["sheets"] = Value::Array(sheets);
            changed.erc_report_json = pretty(&erc);
            refresh_receipt(&fixture, &mut changed);
            assert_eq!(
                bind_kicad10_board_analysis(
                    &fixture.design,
                    SNAPSHOT,
                    "production",
                    FabricationCompilerArtifacts::Static(&fixture.compiled),
                    &fixture.product,
                    ANALYSIS,
                    &fixture.identity_map,
                    &fixture.fabrication,
                    &changed,
                )
                .unwrap_err()
                .code,
                "CC-BOARD-ANALYSIS-ERC-001"
            );
        }
    }

    #[test]
    fn allowlisted_library_warning_keeps_drc_clean() {
        let fixture = fixture();
        let mut evidence = evidence(&fixture);
        let mut drc: Value = serde_json::from_slice(&evidence.drc_report_json).unwrap();
        let mut warning = dirty_finding(&fixture, "lib_footprint_issues");
        warning["description"] =
            json!("The current configuration does not include the footprint library 'CircuitC'");
        warning["severity"] = json!("warning");
        drc["violations"] = Value::Array(vec![warning]);
        evidence.drc_report_json = pretty(&drc);
        refresh_receipt(&fixture, &mut evidence);
        let bundle = bind_fixture(&fixture, &evidence);
        let report: Value = serde_json::from_str(bundle.report_json()).unwrap();
        assert_eq!(report["all_pass"], true);
    }

    #[test]
    fn receipt_tool_report_and_identity_drift_fail_closed() {
        let fixture = fixture();
        let baseline = evidence(&fixture);
        for version in ["10.0.4", "10.0.6", "10.0.5-extra"] {
            let mut host_version = baseline.clone();
            host_version.host_version = version.to_owned();
            assert_eq!(
                bind_kicad10_board_analysis(
                    &fixture.design,
                    SNAPSHOT,
                    "production",
                    FabricationCompilerArtifacts::Static(&fixture.compiled),
                    &fixture.product,
                    ANALYSIS,
                    &fixture.identity_map,
                    &fixture.fabrication,
                    &host_version,
                )
                .unwrap_err()
                .code,
                "CC-BOARD-ANALYSIS-HOST-001",
                "version {version} must not weaken exact KiCad identity"
            );
        }

        let mut mutations = Vec::new();
        let mut executable = baseline.clone();
        executable.host_executable.push(b'x');
        mutations.push(executable);
        let mut normalizer = baseline.clone();
        normalizer.normalizer.push(b'x');
        mutations.push(normalizer);
        let mut host_runner = baseline.clone();
        host_runner.host_runner.push(b'x');
        mutations.push(host_runner);
        let mut receipt = baseline.clone();
        receipt.receipt_json = receipt
            .receipt_json
            .split_last()
            .unwrap()
            .1
            .iter()
            .copied()
            .chain(b" \n".iter().copied())
            .collect();
        mutations.push(receipt);
        for mutation in mutations {
            assert_eq!(
                bind_kicad10_board_analysis(
                    &fixture.design,
                    SNAPSHOT,
                    "production",
                    FabricationCompilerArtifacts::Static(&fixture.compiled),
                    &fixture.product,
                    ANALYSIS,
                    &fixture.identity_map,
                    &fixture.fabrication,
                    &mutation,
                )
                .unwrap_err()
                .code,
                "CC-BOARD-ANALYSIS-RECEIPT-001"
            );
        }

        let mut identity_map: Value = serde_json::from_str(&fixture.identity_map).unwrap();
        identity_map["identities"][0]["semantic_path"] = json!("wrong.path");
        let mut identity_map_json = serde_json::to_string_pretty(&identity_map).unwrap();
        identity_map_json.push('\n');
        assert_eq!(
            prepare_kicad10_board_analysis_request(
                &fixture.design,
                SNAPSHOT,
                "production",
                FabricationCompilerArtifacts::Static(&fixture.compiled),
                &fixture.product,
                ANALYSIS,
                &identity_map_json,
                &fixture.fabrication,
            )
            .unwrap_err()
            .code,
            "CC-BOARD-ANALYSIS-IDENTITY-001"
        );
    }

    #[test]
    fn failed_and_unsupported_results_are_complete_nonacceptance_records() {
        let fixture = fixture();
        for (kind, status, outcome) in [
            (
                BoardAnalysisNoncompletionKind::Failed,
                "failed",
                "unevaluated",
            ),
            (
                BoardAnalysisNoncompletionKind::Unsupported,
                "unsupported",
                "unsupported",
            ),
        ] {
            let noncompletion = BoardAnalysisNoncompletion {
                kind,
                code: "HOST-UNAVAILABLE".to_owned(),
                message: "host did not complete the requested capabilities".to_owned(),
            };
            let bundle = record_kicad10_board_analysis_noncompletion(
                &fixture.design,
                SNAPSHOT,
                "production",
                FabricationCompilerArtifacts::Static(&fixture.compiled),
                &fixture.product,
                ANALYSIS,
                &fixture.identity_map,
                &fixture.fabrication,
                &noncompletion,
            )
            .unwrap();
            verify_kicad10_board_analysis_noncompletion(
                &fixture.design,
                SNAPSHOT,
                "production",
                FabricationCompilerArtifacts::Static(&fixture.compiled),
                &fixture.product,
                ANALYSIS,
                &fixture.identity_map,
                &fixture.fabrication,
                &noncompletion,
                &bundle,
            )
            .expect("independent noncompletion recomputation must match");
            let mut changed = bundle.clone();
            changed.report_json.push('\n');
            assert_eq!(
                verify_kicad10_board_analysis_noncompletion(
                    &fixture.design,
                    SNAPSHOT,
                    "production",
                    FabricationCompilerArtifacts::Static(&fixture.compiled),
                    &fixture.product,
                    ANALYSIS,
                    &fixture.identity_map,
                    &fixture.fabrication,
                    &noncompletion,
                    &changed,
                )
                .unwrap_err()
                .code,
                "CC-BOARD-ANALYSIS-VERIFY-001"
            );
            assert!(bundle.files().is_empty());
            let result: Value = serde_json::from_str(bundle.result_json()).unwrap();
            assert_eq!(result["status"], status);
            assert!(result["evidence"].is_null());
            assert!(result["tool"].is_null());
            let report: Value = serde_json::from_str(bundle.report_json()).unwrap();
            assert_eq!(report["all_pass"], false);
            assert_eq!(report["outcomes"].as_array().unwrap().len(), 5);
            assert!(
                report["outcomes"]
                    .as_array()
                    .unwrap()
                    .iter()
                    .all(|entry| entry["outcome"] == outcome && entry["evidence_role"].is_null())
            );
        }
    }

    #[test]
    fn missing_capability_and_coordinated_bundle_rewrite_fail() {
        let fixture = fixture();
        let mut incomplete = fixture.design.clone();
        incomplete.product.manufacturability_analyses[0]
            .assertions
            .pop();
        assert_eq!(
            prepare_kicad10_board_analysis_request(
                &incomplete,
                SNAPSHOT,
                "production",
                FabricationCompilerArtifacts::Static(&fixture.compiled),
                &fixture.product,
                ANALYSIS,
                &fixture.identity_map,
                &fixture.fabrication,
            )
            .unwrap_err()
            .code,
            "CC-BOARD-ANALYSIS-CAPABILITY-001"
        );

        let evidence = evidence(&fixture);
        let mut bundle = bind_fixture(&fixture, &evidence);
        bundle.result_json = bundle
            .result_json
            .replace("\"completed\"", "\"unsupported\"");
        assert_eq!(
            verify_kicad10_board_analysis(
                &fixture.design,
                SNAPSHOT,
                "production",
                FabricationCompilerArtifacts::Static(&fixture.compiled),
                &fixture.product,
                ANALYSIS,
                &fixture.identity_map,
                &fixture.fabrication,
                &evidence,
                &bundle,
            )
            .unwrap_err()
            .code,
            "CC-BOARD-ANALYSIS-VERIFY-001"
        );
    }

    #[test]
    fn normalized_reports_reject_noncanonical_and_duplicate_json() {
        let fixture = fixture();
        let baseline = evidence(&fixture);
        let mut noncanonical = baseline.clone();
        noncanonical.erc_report_json = serde_json::to_vec(
            &serde_json::from_slice::<Value>(&noncanonical.erc_report_json).unwrap(),
        )
        .unwrap();
        noncanonical.erc_report_json.push(b'\n');
        assert_eq!(
            bind_kicad10_board_analysis(
                &fixture.design,
                SNAPSHOT,
                "production",
                FabricationCompilerArtifacts::Static(&fixture.compiled),
                &fixture.product,
                ANALYSIS,
                &fixture.identity_map,
                &fixture.fabrication,
                &noncanonical,
            )
            .unwrap_err()
            .code,
            "CC-BOARD-ANALYSIS-CONTRACT-001"
        );

        let mut duplicate = baseline.clone();
        let text = String::from_utf8(duplicate.drc_report_json).unwrap();
        duplicate.drc_report_json = text
            .replacen("{\n", "{\n  \"coordinate_units\": \"mm\",\n", 1)
            .into_bytes();
        assert_eq!(
            bind_kicad10_board_analysis(
                &fixture.design,
                SNAPSHOT,
                "production",
                FabricationCompilerArtifacts::Static(&fixture.compiled),
                &fixture.product,
                ANALYSIS,
                &fixture.identity_map,
                &fixture.fabrication,
                &duplicate,
            )
            .unwrap_err()
            .code,
            "CC-BOARD-ANALYSIS-CONTRACT-001"
        );
    }

    #[test]
    fn completed_aggregate_guards_accept_exact_limit_and_reject_one_over() {
        assert_eq!(
            completed_input_aggregate([MAX_AGGREGATE_BYTES]).unwrap(),
            MAX_AGGREGATE_BYTES
        );
        assert_eq!(
            completed_bundle_aggregate([MAX_AGGREGATE_BYTES]).unwrap(),
            MAX_AGGREGATE_BYTES
        );
        for error in [
            completed_input_aggregate([MAX_AGGREGATE_BYTES, 1]).unwrap_err(),
            completed_bundle_aggregate([MAX_AGGREGATE_BYTES, 1]).unwrap_err(),
        ] {
            assert_eq!(error.code, "CC-BOARD-ANALYSIS-RESOURCE-001");
        }
        for error in [
            completed_input_aggregate([usize::MAX, 1]).unwrap_err(),
            completed_bundle_aggregate([usize::MAX, 1]).unwrap_err(),
        ] {
            assert_eq!(error.code, "CC-BOARD-ANALYSIS-RESOURCE-001");
        }
    }

    #[test]
    fn completed_bind_enforces_input_and_emitted_bundle_aggregate_limits() {
        let fixture = fixture();
        let evidence = evidence(&fixture);
        for (input_limit, bundle_limit, expected_path) in [
            (0, MAX_AGGREGATE_BYTES, "inputs"),
            (MAX_AGGREGATE_BYTES, 0, "bundle"),
        ] {
            let error = bind_kicad10_board_analysis_with_limits(
                &fixture.design,
                SNAPSHOT,
                "production",
                FabricationCompilerArtifacts::Static(&fixture.compiled),
                &fixture.product,
                ANALYSIS,
                &fixture.identity_map,
                &fixture.fabrication,
                &evidence,
                input_limit,
                bundle_limit,
            )
            .unwrap_err();
            assert_eq!(error.code, "CC-BOARD-ANALYSIS-RESOURCE-001");
            assert_eq!(error.path, expected_path);
        }
    }

    #[test]
    fn diagnostic_rows_and_tool_bytes_accept_exact_limits_and_reject_one_over() {
        let fixture = fixture();
        let mut exact_rows = evidence(&fixture);
        let mut drc: Value = serde_json::from_slice(&exact_rows.drc_report_json).unwrap();
        drc["violations"] = Value::Array(
            (0..256)
                .map(|index| {
                    let mut finding = dirty_finding(&fixture, "clearance");
                    finding["description"] = json!(format!("finding {index:03}"));
                    finding
                })
                .collect(),
        );
        exact_rows.drc_report_json = pretty(&drc);
        refresh_receipt(&fixture, &mut exact_rows);
        bind_fixture(&fixture, &exact_rows);

        let mut one_over_rows = exact_rows.clone();
        let mut drc: Value = serde_json::from_slice(&one_over_rows.drc_report_json).unwrap();
        drc["violations"]
            .as_array_mut()
            .unwrap()
            .push(dirty_finding(&fixture, "one_over"));
        one_over_rows.drc_report_json = pretty(&drc);
        refresh_receipt(&fixture, &mut one_over_rows);
        assert_eq!(
            bind_kicad10_board_analysis(
                &fixture.design,
                SNAPSHOT,
                "production",
                FabricationCompilerArtifacts::Static(&fixture.compiled),
                &fixture.product,
                ANALYSIS,
                &fixture.identity_map,
                &fixture.fabrication,
                &one_over_rows,
            )
            .unwrap_err()
            .code,
            "CC-BOARD-ANALYSIS-DRC-001"
        );

        let mut exact_bytes = evidence(&fixture);
        exact_bytes.host_executable = vec![0; MAX_FILE_BYTES];
        refresh_receipt(&fixture, &mut exact_bytes);
        bind_fixture(&fixture, &exact_bytes);
        exact_bytes.host_executable.push(0);
        assert_eq!(
            bind_kicad10_board_analysis(
                &fixture.design,
                SNAPSHOT,
                "production",
                FabricationCompilerArtifacts::Static(&fixture.compiled),
                &fixture.product,
                ANALYSIS,
                &fixture.identity_map,
                &fixture.fabrication,
                &exact_bytes,
            )
            .unwrap_err()
            .code,
            "CC-BOARD-ANALYSIS-RESOURCE-001"
        );
    }
}
