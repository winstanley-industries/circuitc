use std::collections::BTreeSet;

use serde::Deserialize;
use serde_json::Value;
use sha2::{Digest, Sha256};

use crate::RelativeArtifactPath;
use crate::compile::{CheckedCompiledArtifacts, CompiledArtifacts};
use crate::manufacturing::{FabricationCompilerArtifacts, verify_kicad10_fabrication_manifest};
use crate::product::verify_product_artifact_bundle;
use crate::product_analysis::verify_kicad10_board_analysis;
use crate::simulation::{
    AssertionStatus, ExecutionStatus, OHMNIVORE_BACKEND_CONTRACT, OHMNIVORE_BACKEND_NAME,
    OHMNIVORE_BACKEND_VERSION, OHMNIVORE_SOURCE_REVISION, parse_report,
};

use super::contract::*;
use super::identity::canonical_design_identity;

const APGAR_PROVENANCE_HEADER: &str = "circuitc-apgar-route-provenance-v1";

struct CollectedArtifact {
    role: String,
    path: RelativeArtifactPath,
    contents: Vec<u8>,
}

struct Collector {
    artifacts: Vec<CollectedArtifact>,
    paths: BTreeSet<String>,
    folded_paths: BTreeSet<String>,
    path_bytes: usize,
    aggregate_bytes: usize,
}

impl Default for Collector {
    fn default() -> Self {
        Self {
            artifacts: Vec::new(),
            paths: BTreeSet::new(),
            folded_paths: BTreeSet::from(["manifest.json".to_owned(), "request.json".to_owned()]),
            path_bytes: "manifest.json".len() + "request.json".len(),
            aggregate_bytes: 0,
        }
    }
}

impl Collector {
    fn add(
        &mut self,
        role: impl Into<String>,
        path: impl Into<String>,
        contents: &[u8],
    ) -> Result<(), ReleaseDiagnostic> {
        let role = role.into();
        let path = path.into();
        validate_release_path(&path)?;
        enforce_resource_limit(
            &path,
            contents.len(),
            MAX_FILE_BYTES,
            "release artifact exceeds the 64 MiB per-file limit",
        )?;
        let next_file_count =
            self.artifacts.len().checked_add(3).ok_or_else(|| {
                resource_overflow("artifacts", "release artifact count overflowed")
            })?;
        enforce_resource_limit(
            "artifacts",
            next_file_count,
            MAX_FILES,
            "release artifact count exceeds 4096",
        )?;
        self.path_bytes = self.path_bytes.checked_add(path.len()).ok_or_else(|| {
            diagnostic(
                "CC-RELEASE-RESOURCE-001",
                "artifacts",
                "release path-byte count overflowed",
            )
        })?;
        enforce_resource_limit(
            "artifacts",
            self.path_bytes,
            MAX_PATH_BYTES,
            "release path-byte count exceeds 1 MiB",
        )?;
        self.aggregate_bytes = self
            .aggregate_bytes
            .checked_add(contents.len())
            .ok_or_else(|| {
                diagnostic(
                    "CC-RELEASE-RESOURCE-001",
                    "artifacts",
                    "release artifact aggregate overflowed",
                )
            })?;
        enforce_resource_limit(
            "artifacts",
            self.aggregate_bytes,
            MAX_AGGREGATE_BYTES,
            "release artifact aggregate exceeds 1 GiB",
        )?;
        let folded = path.to_ascii_lowercase();
        if self.folded_paths.iter().any(|existing| {
            existing == &folded
                || existing.starts_with(&(folded.clone() + "/"))
                || folded.starts_with(&(existing.clone() + "/"))
        }) {
            return Err(diagnostic(
                "CC-RELEASE-INVENTORY-001",
                &path,
                "release artifact path is duplicated or has a file/directory collision under ASCII case folding",
            ));
        }
        self.paths.insert(path.clone());
        self.folded_paths.insert(folded);
        let path = RelativeArtifactPath::try_new(path).map_err(|error| {
            diagnostic(
                "CC-RELEASE-INVENTORY-001",
                "artifacts.path",
                error.to_string(),
            )
        })?;
        self.artifacts.push(CollectedArtifact {
            role,
            path,
            contents: contents.to_vec(),
        });
        Ok(())
    }

    fn sort(&mut self) {
        self.artifacts.sort_by(|left, right| {
            left.path
                .as_str()
                .cmp(right.path.as_str())
                .then_with(|| left.role.cmp(&right.role))
        });
    }

    fn bindings(&self) -> Vec<ArtifactBinding> {
        self.artifacts
            .iter()
            .map(|artifact| {
                bind_artifact(&artifact.role, artifact.path.as_str(), &artifact.contents)
            })
            .collect()
    }
}

fn resource_overflow(path: &str, message: &str) -> ReleaseDiagnostic {
    diagnostic("CC-RELEASE-RESOURCE-001", path, message)
}

fn enforce_resource_limit(
    path: &str,
    value: usize,
    limit: usize,
    message: &str,
) -> Result<(), ReleaseDiagnostic> {
    if value > limit {
        return Err(diagnostic("CC-RELEASE-RESOURCE-001", path, message));
    }
    Ok(())
}

fn validate_release_path(path: &str) -> Result<(), ReleaseDiagnostic> {
    if path.len() > 4096 {
        return Err(diagnostic(
            "CC-RELEASE-INVENTORY-001",
            path,
            "release path is not a portable canonical ASCII relative path",
        ));
    }
    let folded = path.to_ascii_lowercase();
    if folded == "request.json"
        || folded.starts_with("request.json/")
        || folded == "manifest.json"
        || folded.starts_with("manifest.json/")
    {
        return Err(diagnostic(
            "CC-RELEASE-INVENTORY-001",
            path,
            "release request and manifest paths are reserved",
        ));
    }
    if !path.is_ascii()
        || path.starts_with('/')
        || path.contains(['\\', '\0'])
        || path.split('/').any(|component| {
            component.is_empty()
                || matches!(component, "." | "..")
                || component
                    .to_ascii_lowercase()
                    .starts_with(".circuitc-release-transaction-")
                || !component
                    .bytes()
                    .all(|byte| byte.is_ascii_alphanumeric() || b"._-".contains(&byte))
        })
    {
        return Err(diagnostic(
            "CC-RELEASE-INVENTORY-001",
            path,
            "release path is not a portable canonical ASCII relative path",
        ));
    }
    Ok(())
}

fn preflight_supplied_files(
    files: &[ReleaseFile],
    code: &'static str,
) -> Result<(), ReleaseDiagnostic> {
    if files.len() > MAX_FILES {
        return Err(diagnostic(
            code,
            "files",
            "release candidate exceeds the file-count limit",
        ));
    }
    let mut folded_paths = BTreeSet::new();
    let mut path_bytes = 0_usize;
    let mut aggregate_bytes = 0_usize;
    for file in files {
        let path = file.path.as_str();
        if path.len() > 4096 {
            return Err(diagnostic(
                code,
                "files",
                "release candidate path exceeds the per-path limit",
            ));
        }
        path_bytes = path_bytes.checked_add(path.len()).ok_or_else(|| {
            diagnostic(
                code,
                "files",
                "release candidate path-byte count overflowed",
            )
        })?;
        if path_bytes > MAX_PATH_BYTES {
            return Err(diagnostic(
                code,
                "files",
                "release candidate exceeds the path-byte limit",
            ));
        }
        let folded = path.to_ascii_lowercase();
        if matches!(folded.as_str(), "request.json" | "manifest.json") {
            if path != folded {
                return Err(diagnostic(
                    code,
                    path,
                    "release control paths must use exact lowercase spelling",
                ));
            }
        } else {
            validate_release_path(path).map_err(|error| {
                diagnostic(
                    code,
                    error.path,
                    format!("invalid candidate path: {}", error.message),
                )
            })?;
        }
        if file.contents.len() > MAX_FILE_BYTES {
            return Err(diagnostic(
                code,
                path,
                "release candidate file exceeds the per-file limit",
            ));
        }
        if folded_paths.iter().any(|existing: &String| {
            existing == &folded
                || existing.starts_with(&(folded.clone() + "/"))
                || folded.starts_with(&(existing.clone() + "/"))
        }) {
            return Err(diagnostic(
                code,
                path,
                "release candidate has a duplicate or file/directory collision under ASCII case folding",
            ));
        }
        folded_paths.insert(folded);
        aggregate_bytes = aggregate_bytes
            .checked_add(file.contents.len())
            .ok_or_else(|| diagnostic(code, "files", "release candidate aggregate overflowed"))?;
        if aggregate_bytes > MAX_AGGREGATE_BYTES {
            return Err(diagnostic(
                code,
                "files",
                "release candidate exceeds the aggregate byte limit",
            ));
        }
    }
    Ok(())
}

fn sha256(bytes: &[u8]) -> String {
    format!("{:x}", Sha256::digest(bytes))
}

fn bind_artifact(role: &str, path: &str, contents: &[u8]) -> ArtifactBinding {
    ArtifactBinding {
        role: role.to_owned(),
        path: path.to_owned(),
        byte_length: contents.len() as u64,
        sha256: sha256(contents),
    }
}

fn canonical_json<T: serde::Serialize>(value: &T) -> Result<String, ReleaseDiagnostic> {
    let mut json = serde_json::to_string(value).map_err(|error| {
        diagnostic(
            "CC-RELEASE-ENCODING-001",
            "release",
            format!("could not encode canonical release JSON: {error}"),
        )
    })?;
    json.push('\n');
    enforce_resource_limit(
        "release",
        json.len(),
        MAX_FILE_BYTES,
        "release contract exceeds the 64 MiB per-file limit",
    )?;
    Ok(json)
}

fn authenticate_source_design(inputs: &ReleaseInputs<'_>) -> Result<(), ReleaseDiagnostic> {
    let elaborated =
        crate::frontend::elaborate_source("release-input.circuitc", inputs.source.to_owned())
            .map_err(|errors| {
                let message = errors.first().map_or_else(
                    || "source elaboration failed".to_owned(),
                    ToString::to_string,
                );
                diagnostic("CC-RELEASE-SOURCE-001", "source", message)
            })?;
    if &elaborated.design != inputs.design {
        return Err(diagnostic(
            "CC-RELEASE-SOURCE-001",
            "source",
            "exact source bytes do not elaborate to the exact canonical supplied Design IR",
        ));
    }
    Ok(())
}

fn preflight_inputs(inputs: &ReleaseInputs<'_>) -> Result<(), ReleaseDiagnostic> {
    let mut total = 0_usize;
    let mut count = 0_usize;
    let mut consume = |path: &str, length: usize| -> Result<(), ReleaseDiagnostic> {
        enforce_resource_limit(
            path,
            length,
            MAX_FILE_BYTES,
            "release input exceeds the 64 MiB per-file limit",
        )?;
        count = count
            .checked_add(1)
            .ok_or_else(|| resource_overflow("inputs", "release input count overflowed"))?;
        enforce_resource_limit(
            "inputs",
            count,
            MAX_FILES,
            "release consumed-input count exceeds 4096",
        )?;
        total = total.checked_add(length).ok_or_else(|| {
            resource_overflow("inputs", "release consumed-input aggregate overflowed")
        })?;
        enforce_resource_limit(
            "inputs",
            total,
            MAX_AGGREGATE_BYTES,
            "release consumed-input aggregate exceeds 1 GiB",
        )?;
        Ok(())
    };

    consume("source", inputs.source.len())?;
    consume("catalog", inputs.catalog_snapshot.len())?;
    consume("kicad_identity_map", inputs.kicad_identity_map_json.len())?;
    for (path, bytes) in [
        ("fabrication.kicad", inputs.fabrication.host_executable),
        (
            "analysis.kicad",
            inputs.analysis.host.host_executable.as_slice(),
        ),
        (
            "analysis.normalizer",
            inputs.analysis.host.normalizer.as_slice(),
        ),
        (
            "analysis.host_runner",
            inputs.analysis.host.host_runner.as_slice(),
        ),
        (
            "analysis.erc",
            inputs.analysis.host.erc_report_json.as_slice(),
        ),
        (
            "analysis.drc",
            inputs.analysis.host.drc_report_json.as_slice(),
        ),
        (
            "analysis.receipt",
            inputs.analysis.host.receipt_json.as_slice(),
        ),
    ] {
        consume(path, bytes.len())?;
    }
    for (path, bytes) in [
        ("tools.ohmnivore", inputs.tools.ohmnivore_executable),
        (
            "tools.ohmnivore_provenance",
            inputs.tools.ohmnivore_provenance,
        ),
        ("tools.apgar", inputs.tools.apgar_executable),
        ("tools.apgar_provenance", inputs.tools.apgar_provenance),
    ] {
        if let Some(bytes) = bytes {
            consume(path, bytes.len())?;
        }
    }
    for file in inputs.fabrication.host_files {
        consume(file.path.as_str(), file.contents.len())?;
    }
    for (path, bytes) in [
        (
            inputs.product.resolution_path.as_str(),
            inputs.product.resolution_json.as_bytes(),
        ),
        (
            inputs.product.bom_path.as_str(),
            inputs.product.bom_json.as_bytes(),
        ),
        (
            inputs.product.placement_path.as_str(),
            inputs.product.placement_json.as_bytes(),
        ),
        (
            inputs.product.assembly_path.as_str(),
            inputs.product.assembly_json.as_bytes(),
        ),
        (
            inputs.fabrication.bundle.request_path().as_str(),
            inputs.fabrication.bundle.request_json().as_bytes(),
        ),
        (
            inputs.fabrication.bundle.manifest_path().as_str(),
            inputs.fabrication.bundle.manifest_json().as_bytes(),
        ),
        (
            inputs.analysis.bundle.request_path().as_str(),
            inputs.analysis.bundle.request_json().as_bytes(),
        ),
        (
            inputs.analysis.bundle.result_path().as_str(),
            inputs.analysis.bundle.result_json().as_bytes(),
        ),
        (
            inputs.analysis.bundle.report_path().as_str(),
            inputs.analysis.bundle.report_json().as_bytes(),
        ),
    ] {
        consume(path, bytes.len())?;
    }
    for file in inputs.fabrication.bundle.files() {
        consume(file.path.as_str(), file.contents.len())?;
    }
    for file in inputs.analysis.bundle.files() {
        consume(file.path.as_str(), file.contents.len())?;
    }
    let (static_artifacts, checked) = match inputs.compiler {
        FabricationCompilerArtifacts::Static(static_artifacts) => (static_artifacts, None),
        FabricationCompilerArtifacts::Checked(checked) => {
            (checked.static_artifacts(), Some(checked))
        }
    };
    for (path, bytes) in [
        (
            "compiler.kicad_schematic",
            static_artifacts.kicad_schematic.as_bytes(),
        ),
        ("compiler.kicad_pcb", static_artifacts.kicad_pcb.as_bytes()),
        (
            "compiler.kicad_project",
            static_artifacts.kicad_project.as_bytes(),
        ),
        (
            "compiler.kicad_symbol_table",
            static_artifacts.kicad_symbol_table.as_bytes(),
        ),
        (
            "compiler.kicad_footprint_table",
            static_artifacts.kicad_footprint_table.as_bytes(),
        ),
        ("compiler.spice", static_artifacts.spice.as_bytes()),
    ] {
        consume(path, bytes.len())?;
    }
    for file in &static_artifacts.kicad_library_files {
        consume(file.relative_path.as_str(), file.contents.len())?;
    }
    if let Some(checked) = checked {
        for simulation in checked.simulations() {
            for (path, bytes) in [
                (
                    simulation.netlist_path.as_str(),
                    simulation.netlist.as_bytes(),
                ),
                (
                    simulation.request_path.as_str(),
                    simulation.request_json.as_bytes(),
                ),
                (
                    simulation.map_path.as_str(),
                    simulation.spice_identity_map_json.as_bytes(),
                ),
                (
                    simulation.result_path.as_str(),
                    simulation.result_json.as_bytes(),
                ),
                (
                    simulation.report_path.as_str(),
                    simulation.report_json.as_bytes(),
                ),
            ] {
                consume(path, bytes.len())?;
            }
        }
        if let Some(routing) = checked.routing() {
            for (path, bytes) in [
                (
                    routing.request_path.as_str(),
                    routing.request_json.as_bytes(),
                ),
                (routing.result_path.as_str(), routing.result_json.as_bytes()),
                (
                    routing.projection_path.as_str(),
                    routing.projection_json.as_bytes(),
                ),
            ] {
                consume(path, bytes.len())?;
            }
        }
    }
    if let Some(routing) = inputs.routing {
        consume("routing.acceptance", routing.acceptance_json.len())?;
    }
    Ok(())
}

fn authenticate_source(
    inputs: &ReleaseInputs<'_>,
    static_artifacts: &CompiledArtifacts,
) -> Result<(), ReleaseDiagnostic> {
    if inputs.source.len() > MAX_FILE_BYTES {
        return Err(diagnostic(
            "CC-RELEASE-RESOURCE-001",
            "source",
            "CircuitC source exceeds the 64 MiB limit",
        ));
    }
    let logical_name = format!("{}.circuitc", inputs.design.name);
    let (elaborated, identity_map) = crate::frontend::elaborate_source_with_kicad_identity_map(
        logical_name,
        inputs.source.to_owned(),
        &static_artifacts.kicad_identities,
    )
    .map_err(|errors| {
        let message = errors.first().map_or_else(
            || "source elaboration failed".to_owned(),
            ToString::to_string,
        );
        diagnostic("CC-RELEASE-SOURCE-001", "source", message)
    })?;
    if &elaborated.design != inputs.design {
        return Err(diagnostic(
            "CC-RELEASE-SOURCE-001",
            "source",
            "exact source bytes do not elaborate to the exact canonical supplied Design IR",
        ));
    }
    if identity_map != inputs.kicad_identity_map_json {
        return Err(diagnostic(
            "CC-RELEASE-SOURCE-001",
            "kicad_identity_map_json",
            "KiCad identity map does not equal exact-source provenance and compiler identities",
        ));
    }
    Ok(())
}

fn map_predecessor_error(
    code: &'static str,
    path: &str,
    error: impl std::fmt::Display,
) -> ReleaseDiagnostic {
    diagnostic(code, path, error.to_string())
}

fn authenticated_compiler<'a>(
    inputs: &'a ReleaseInputs<'a>,
) -> Result<(&'a CompiledArtifacts, Option<&'a CheckedCompiledArtifacts>), ReleaseDiagnostic> {
    let requires_checked =
        !inputs.design.analyses.is_empty() || !inputs.design.board.routing_requests.is_empty();
    match inputs.compiler {
        FabricationCompilerArtifacts::Static(artifacts) => {
            if requires_checked {
                return Err(diagnostic(
                    "CC-RELEASE-APPLICABILITY-001",
                    "compiler",
                    "simulation or routing intent requires checked compiler evidence",
                ));
            }
            let expected = crate::compile(inputs.design).map_err(|error| {
                map_predecessor_error("CC-RELEASE-COMPILER-001", "compiler", error)
            })?;
            if artifacts != &expected {
                return Err(diagnostic(
                    "CC-RELEASE-COMPILER-001",
                    "compiler",
                    "static artifacts do not equal deterministic compilation of current Design IR",
                ));
            }
            Ok((artifacts, None))
        }
        FabricationCompilerArtifacts::Checked(checked) => {
            let static_artifacts =
                crate::compile::authenticate_checked_compilation(inputs.design, checked).map_err(
                    |diagnostics| {
                        let message = diagnostics.first().map_or_else(
                            || "checked compiler authentication failed".to_owned(),
                            ToString::to_string,
                        );
                        diagnostic("CC-RELEASE-COMPILER-001", "compiler", message)
                    },
                )?;
            if !requires_checked {
                return Err(diagnostic(
                    "CC-RELEASE-APPLICABILITY-001",
                    "compiler",
                    "checked compiler evidence is forbidden when simulation and routing are absent",
                ));
            }
            Ok((static_artifacts, Some(checked)))
        }
    }
}

fn product_input_sha256(
    product: &crate::product::ProductArtifactBundle,
) -> Result<String, ReleaseDiagnostic> {
    let value: Value = serde_json::from_str(&product.resolution_json).map_err(|error| {
        diagnostic(
            "CC-RELEASE-PRODUCT-001",
            product.resolution_path.as_str(),
            format!("verified product resolution could not be decoded: {error}"),
        )
    })?;
    value
        .get("product_input_sha256")
        .and_then(Value::as_str)
        .map(ToOwned::to_owned)
        .ok_or_else(|| {
            diagnostic(
                "CC-RELEASE-PRODUCT-001",
                product.resolution_path.as_str(),
                "verified product resolution omitted product_input_sha256",
            )
        })
}

fn verify_predecessors(inputs: &ReleaseInputs<'_>) -> Result<(), ReleaseDiagnostic> {
    verify_product_artifact_bundle(
        inputs.design,
        inputs.catalog_snapshot,
        inputs.variant_path,
        inputs.product,
    )
    .map_err(|diagnostics| {
        let message = diagnostics.first().map_or_else(
            || "product verification failed".to_owned(),
            ToString::to_string,
        );
        diagnostic("CC-RELEASE-PRODUCT-001", "product", message)
    })?;
    verify_kicad10_fabrication_manifest(
        inputs.design,
        inputs.catalog_snapshot,
        inputs.variant_path,
        inputs.compiler,
        inputs.product,
        inputs.fabrication.analysis_path,
        inputs.fabrication.assertion_path,
        inputs.fabrication.host_version,
        inputs.fabrication.host_executable,
        inputs.fabrication.host_files,
        inputs.fabrication.bundle,
    )
    .map_err(|error| map_predecessor_error("CC-RELEASE-FABRICATION-001", "fabrication", error))?;
    if inputs.fabrication.analysis_path != inputs.analysis.analysis_path {
        return Err(diagnostic(
            "CC-RELEASE-ANALYSIS-001",
            "analysis_path",
            "fabrication and board-analysis evidence do not name the same analysis",
        ));
    }
    if inputs.fabrication.host_executable != inputs.analysis.host.host_executable
        || inputs.fabrication.host_version != inputs.analysis.host.host_version
    {
        return Err(diagnostic(
            "CC-RELEASE-TOOL-001",
            "tools.kicad",
            "fabrication and board analysis must authenticate the same KiCad executable and version",
        ));
    }
    verify_kicad10_board_analysis(
        inputs.design,
        inputs.catalog_snapshot,
        inputs.variant_path,
        inputs.compiler,
        inputs.product,
        inputs.analysis.analysis_path,
        inputs.kicad_identity_map_json,
        inputs.fabrication.bundle,
        inputs.analysis.host,
        inputs.analysis.bundle,
    )
    .map_err(|error| map_predecessor_error("CC-RELEASE-ANALYSIS-001", "analysis", error))?;
    let report: Value =
        serde_json::from_str(inputs.analysis.bundle.report_json()).map_err(|error| {
            diagnostic(
                "CC-RELEASE-ANALYSIS-001",
                inputs.analysis.bundle.report_path().as_str(),
                format!("verified board-analysis report could not be decoded: {error}"),
            )
        })?;
    let all_pass = report.get("all_pass").and_then(Value::as_bool) == Some(true);
    let completed = report.get("execution_status").and_then(Value::as_str) == Some("completed");
    let outcomes_pass = report
        .get("outcomes")
        .and_then(Value::as_array)
        .is_some_and(|outcomes| {
            outcomes.len() == 5
                && outcomes
                    .iter()
                    .all(|outcome| outcome.get("outcome").and_then(Value::as_str) == Some("pass"))
        });
    if !all_pass || !completed || !outcomes_pass {
        return Err(diagnostic(
            "CC-RELEASE-ANALYSIS-001",
            inputs.analysis.bundle.report_path().as_str(),
            "release requires a recomputed completed board analysis with all five capabilities passing",
        ));
    }
    Ok(())
}

fn verify_simulations(
    design: &crate::design::Design,
    checked: Option<&CheckedCompiledArtifacts>,
) -> Result<(), ReleaseDiagnostic> {
    if design.analyses.is_empty() {
        return Ok(());
    }
    let checked = checked.expect("checked applicability was authenticated");
    let expected = crate::simulation::lower::lower_inputs(design).map_err(|diagnostics| {
        let message = diagnostics.first().map_or_else(
            || "simulation lowering failed".to_owned(),
            ToString::to_string,
        );
        diagnostic("CC-RELEASE-SIMULATION-001", "simulation", message)
    })?;
    if expected.len() != checked.simulations().len() {
        return Err(diagnostic(
            "CC-RELEASE-SIMULATION-001",
            "simulation",
            "checked simulation inventory does not match current Design IR",
        ));
    }
    for (expected, supplied) in expected.iter().zip(checked.simulations()) {
        if expected.analysis_path != supplied.analysis_path
            || expected.netlist_path != supplied.netlist_path
            || expected.request_path != supplied.request_path
            || expected.map_path != supplied.map_path
            || expected.result_path != supplied.result_path
            || expected.report_path != supplied.report_path
            || expected.netlist != supplied.netlist
            || expected.request_json != supplied.request_json
            || expected.spice_identity_map_json != supplied.spice_identity_map_json
        {
            return Err(diagnostic(
                "CC-RELEASE-SIMULATION-001",
                &supplied.analysis_path,
                "checked simulation input chain is stale relative to current Design IR",
            ));
        }
        let report = parse_report(&supplied.report_json).map_err(|error| {
            map_predecessor_error(
                "CC-RELEASE-SIMULATION-001",
                supplied.report_path.as_str(),
                error,
            )
        })?;
        let (_, _, result) = report
            .verify_binding_bytes(
                supplied.request_json.as_bytes(),
                supplied.spice_identity_map_json.as_bytes(),
                supplied.result_json.as_bytes(),
            )
            .map_err(|error| {
                map_predecessor_error(
                    "CC-RELEASE-SIMULATION-001",
                    supplied.report_path.as_str(),
                    error,
                )
            })?;
        if result.status != ExecutionStatus::Completed
            || report
                .assertions
                .iter()
                .any(|assertion| assertion.status != AssertionStatus::Pass)
            || report.summary.fail != 0
            || report.summary.unsupported != 0
            || report.summary.unevaluated != 0
        {
            return Err(diagnostic(
                "CC-RELEASE-SIMULATION-001",
                supplied.report_path.as_str(),
                "release requires completed simulation evidence with every assertion passing",
            ));
        }
    }
    Ok(())
}

fn provenance_text<'a>(bytes: &'a [u8], path: &str) -> Result<&'a str, ReleaseDiagnostic> {
    std::str::from_utf8(bytes).map_err(|error| {
        diagnostic(
            "CC-RELEASE-TOOL-001",
            path,
            format!("tool provenance is not UTF-8: {error}"),
        )
    })
}

fn verify_ohmnivore_tools(inputs: &ReleaseInputs<'_>) -> Result<(), ReleaseDiagnostic> {
    let required = !inputs.design.analyses.is_empty();
    match (
        required,
        inputs.tools.ohmnivore_executable,
        inputs.tools.ohmnivore_provenance,
    ) {
        (false, None, None) => Ok(()),
        (true, Some(executable), Some(provenance)) => {
            let expected = format!(
                "circuitc-ohmnivore-provenance-v1\nname={OHMNIVORE_BACKEND_NAME}\nversion={OHMNIVORE_BACKEND_VERSION}\ncontract={OHMNIVORE_BACKEND_CONTRACT}\nsource_revision={OHMNIVORE_SOURCE_REVISION}\nexecutable_sha256={}\n",
                sha256(executable)
            );
            if provenance != expected.as_bytes() {
                return Err(diagnostic(
                    "CC-RELEASE-TOOL-001",
                    "tools.ohmnivore",
                    "Ohmnivore provenance does not authenticate the supplied pinned executable",
                ));
            }
            let executable_sha256 = sha256(executable);
            let checked = match inputs.compiler {
                FabricationCompilerArtifacts::Checked(checked) => checked,
                FabricationCompilerArtifacts::Static(_) => {
                    return Err(diagnostic(
                        "CC-RELEASE-APPLICABILITY-001",
                        "tools.ohmnivore",
                        "simulation release requires checked compiler evidence",
                    ));
                }
            };
            if checked.simulations().iter().any(|simulation| {
                simulation.tool_executable_sha256.as_deref() != Some(&executable_sha256)
            }) {
                return Err(diagnostic(
                    "CC-RELEASE-TOOL-001",
                    "tools.ohmnivore",
                    "Ohmnivore executable digest does not equal the execution identity retained by every checked simulation",
                ));
            }
            Ok(())
        }
        _ => Err(diagnostic(
            "CC-RELEASE-APPLICABILITY-001",
            "tools.ohmnivore",
            "Ohmnivore executable and provenance must be present exactly when simulation applies",
        )),
    }
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct RouteAcceptance {
    schema_name: String,
    schema_version: u32,
    design_name: String,
    request_path: String,
    request_identity_sha256: String,
    request_sha256: String,
    result_sha256: String,
    projection_sha256: String,
    tool_provenance_sha256: String,
    selected_candidate_id: String,
    candidate: RouteAcceptanceCandidate,
    tool: RouteAcceptanceTool,
    authorities: RouteAcceptanceAuthorities,
    kicad: RouteAcceptanceKicad,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct RouteAcceptanceCandidate {
    geometry_signature: String,
    resource_signature: String,
    payload_checksum: String,
}

#[derive(Debug, Deserialize, Eq, PartialEq)]
#[serde(deny_unknown_fields)]
struct RouteAcceptanceTool {
    name: String,
    version: String,
    contract_identity: String,
    source_revision: String,
    executable_sha256: String,
    device_class: String,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct RouteAcceptanceAuthorities {
    apgar_exact_admission: bool,
    kicad_erc_clean: bool,
    kicad_drc_clean: bool,
    kicad_schematic_parity_clean: bool,
    kicad_unconnected_clean: bool,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct RouteAcceptanceKicad {
    host: RouteAcceptanceHost,
    pcb_filename: String,
    pcb_sha256: String,
    schematic_filename: String,
    schematic_sha256: String,
    drc_filename: String,
    drc_sha256: String,
    erc_filename: String,
    erc_sha256: String,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct RouteAcceptanceHost {
    name: String,
    major: u32,
    version: String,
}

fn verify_routing(
    inputs: &ReleaseInputs<'_>,
    checked: Option<&CheckedCompiledArtifacts>,
    static_artifacts: &CompiledArtifacts,
) -> Result<(), ReleaseDiagnostic> {
    let required = !inputs.design.board.routing_requests.is_empty();
    match (
        required,
        checked.and_then(CheckedCompiledArtifacts::routing),
        inputs.routing,
        inputs.tools.apgar_executable,
        inputs.tools.apgar_provenance,
    ) {
        (false, None, None, None, None) => Ok(()),
        (true, Some(routing), Some(acceptance), Some(executable), Some(provenance)) => {
            if !inputs.design.board.routes.is_empty() {
                return Err(diagnostic(
                    "CC-RELEASE-ROUTING-001",
                    "design.board.routes",
                    "APGAR acceptance v1 requires the complete board segment set and therefore forbids source-authored routes",
                ));
            }
            let provenance = provenance_text(provenance, "tools.apgar.provenance")?;
            let executable_digest = sha256(executable);
            if !provenance.starts_with(&format!("{APGAR_PROVENANCE_HEADER}\n"))
                || !provenance.contains(&format!("\nexecutable_sha256={executable_digest}\n"))
            {
                return Err(diagnostic(
                    "CC-RELEASE-TOOL-001",
                    "tools.apgar",
                    "APGAR provenance does not authenticate the supplied executable",
                ));
            }
            let verified_json = crate::routing::evidence::verify(
                &routing.request_json,
                &routing.result_json,
                provenance,
            )
            .map_err(|error| {
                map_predecessor_error(
                    "CC-RELEASE-ROUTING-001",
                    routing.result_path.as_str(),
                    error,
                )
            })?;
            let verified: Value = serde_json::from_str(&verified_json).map_err(|error| {
                diagnostic(
                    "CC-RELEASE-ROUTING-001",
                    "routing.verified_evidence",
                    format!("internal verified APGAR evidence could not be decoded: {error}"),
                )
            })?;
            let projection: Value =
                serde_json::from_str(&routing.projection_json).map_err(|error| {
                    diagnostic(
                        "CC-RELEASE-ROUTING-001",
                        routing.projection_path.as_str(),
                        format!("authenticated APGAR projection could not be decoded: {error}"),
                    )
                })?;
            if verified["segments"] != projection["segments"] {
                return Err(diagnostic(
                    "CC-RELEASE-ROUTING-001",
                    "routing.projection.segments",
                    "verified APGAR segments do not equal the authenticated KiCad projection",
                ));
            }
            let acceptance_value: Value = serde_json::from_str(acceptance.acceptance_json)
                .map_err(|error| {
                    diagnostic(
                        "CC-RELEASE-ROUTING-001",
                        "routing.acceptance",
                        format!("route acceptance is not valid JSON: {error}"),
                    )
                })?;
            let mut canonical = serde_json::to_string(&acceptance_value).map_err(|error| {
                diagnostic(
                    "CC-RELEASE-ROUTING-001",
                    "routing.acceptance",
                    error.to_string(),
                )
            })?;
            canonical.push('\n');
            if canonical != acceptance.acceptance_json {
                return Err(diagnostic(
                    "CC-RELEASE-ROUTING-001",
                    "routing.acceptance",
                    "route acceptance is not canonical compact sorted JSON plus one LF",
                ));
            }
            let acceptance: RouteAcceptance = serde_json::from_str(acceptance.acceptance_json)
                .map_err(|error| {
                    diagnostic(
                        "CC-RELEASE-ROUTING-001",
                        "routing.acceptance",
                        format!("route acceptance violates the strict v1 schema: {error}"),
                    )
                })?;
            let verified_tool: RouteAcceptanceTool =
                serde_json::from_value(verified["tool"].clone()).map_err(|error| {
                    diagnostic(
                        "CC-RELEASE-ROUTING-001",
                        "routing.verified_evidence.tool",
                        error.to_string(),
                    )
                })?;
            if acceptance.schema_name != "circuitc.apgar_route_acceptance"
                || acceptance.schema_version != 1
                || acceptance.design_name != inputs.design.name
                || acceptance.request_path != inputs.design.board.routing_requests[0].path
                || acceptance.request_identity_sha256 != routing.request_identity_sha256
                || acceptance.request_sha256 != routing.request_sha256
                || acceptance.result_sha256 != routing.result_sha256
                || acceptance.projection_sha256 != routing.projection_sha256
                || acceptance.tool_provenance_sha256 != sha256(provenance.as_bytes())
                || acceptance.selected_candidate_id != routing.selected_candidate_id
                || verified["candidate_geometry_signature"].as_str()
                    != Some(acceptance.candidate.geometry_signature.as_str())
                || verified["candidate_resource_signature"].as_str()
                    != Some(acceptance.candidate.resource_signature.as_str())
                || verified["candidate_payload_checksum"].as_str()
                    != Some(acceptance.candidate.payload_checksum.as_str())
                || acceptance.tool != verified_tool
            {
                return Err(diagnostic(
                    "CC-RELEASE-ROUTING-001",
                    "routing.acceptance",
                    "route acceptance identity, candidate, or tool does not equal strict APGAR evidence and current Design",
                ));
            }
            let authorities = &acceptance.authorities;
            if !authorities.apgar_exact_admission
                || !authorities.kicad_erc_clean
                || !authorities.kicad_drc_clean
                || !authorities.kicad_schematic_parity_clean
                || !authorities.kicad_unconnected_clean
            {
                return Err(diagnostic(
                    "CC-RELEASE-ROUTING-001",
                    "routing.acceptance.authorities",
                    "all five exact route-acceptance authorities must be true",
                ));
            }
            let erc = inputs
                .analysis
                .bundle
                .files()
                .iter()
                .find(|file| {
                    file.path
                        .as_str()
                        .ends_with("/evidence/erc.normalized.json")
                })
                .ok_or_else(|| {
                    diagnostic(
                        "CC-RELEASE-ROUTING-001",
                        "analysis.erc",
                        "Layer-5 ERC evidence is missing",
                    )
                })?;
            let drc = inputs
                .analysis
                .bundle
                .files()
                .iter()
                .find(|file| {
                    file.path
                        .as_str()
                        .ends_with("/evidence/drc.normalized.json")
                })
                .ok_or_else(|| {
                    diagnostic(
                        "CC-RELEASE-ROUTING-001",
                        "analysis.drc",
                        "Layer-5 DRC evidence is missing",
                    )
                })?;
            let kicad = &acceptance.kicad;
            if kicad.host.name != "kicad"
                || kicad.host.major != 10
                || kicad.host.version != inputs.analysis.host.host_version
                || kicad.pcb_filename != format!("{}.kicad_pcb", inputs.design.name)
                || kicad.pcb_sha256 != sha256(static_artifacts.kicad_pcb.as_bytes())
                || kicad.schematic_filename != format!("{}.kicad_sch", inputs.design.name)
                || kicad.schematic_sha256 != sha256(static_artifacts.kicad_schematic.as_bytes())
                || kicad.erc_filename != "erc.normalized.json"
                || kicad.drc_filename != "drc.normalized.json"
                || kicad.erc_sha256 != sha256(&erc.contents)
                || kicad.drc_sha256 != sha256(&drc.contents)
            {
                return Err(diagnostic(
                    "CC-RELEASE-ROUTING-001",
                    "routing.acceptance.kicad",
                    "route acceptance does not bind the final board and Layer-5 ERC/DRC evidence",
                ));
            }
            Ok(())
        }
        _ => Err(diagnostic(
            "CC-RELEASE-APPLICABILITY-001",
            "routing",
            "checked routing, acceptance, APGAR executable, and provenance must be present exactly when an autoroute request applies",
        )),
    }
}

fn tool_binding(
    role: &str,
    name: &str,
    version: &str,
    source_revision: &str,
    bytes: &[u8],
) -> ToolBinding {
    ToolBinding {
        role: role.to_owned(),
        name: name.to_owned(),
        version: version.to_owned(),
        source_revision: source_revision.to_owned(),
        byte_length: bytes.len() as u64,
        sha256: sha256(bytes),
    }
}

fn collect_tools(inputs: &ReleaseInputs<'_>) -> Vec<ToolBinding> {
    let mut tools = vec![
        tool_binding(
            "kicad",
            "kicad-cli",
            inputs.fabrication.host_version,
            "",
            inputs.fabrication.host_executable,
        ),
        tool_binding(
            "analysis_normalizer",
            "circuitc-kicad-analysis-normalizer",
            "1",
            "",
            &inputs.analysis.host.normalizer,
        ),
        tool_binding(
            "analysis_host_runner",
            "circuitc-kicad-analysis-host-runner",
            "1",
            "",
            &inputs.analysis.host.host_runner,
        ),
    ];
    if let Some(executable) = inputs.tools.ohmnivore_executable {
        tools.push(tool_binding(
            "ohmnivore",
            OHMNIVORE_BACKEND_NAME,
            OHMNIVORE_BACKEND_VERSION,
            OHMNIVORE_SOURCE_REVISION,
            executable,
        ));
    }
    if let Some(executable) = inputs.tools.apgar_executable {
        tools.push(tool_binding(
            "apgar",
            crate::routing::APGAR_TOOL_NAME,
            crate::routing::APGAR_TOOL_VERSION,
            crate::routing::PINNED_APGAR_SOURCE_REVISION,
            executable,
        ));
    }
    tools
}

fn collect_static(
    collector: &mut Collector,
    design_name: &str,
    static_artifacts: &CompiledArtifacts,
    kicad_identity_map_json: &str,
) -> Result<(), ReleaseDiagnostic> {
    collector.add(
        "kicad_schematic",
        format!("{design_name}.kicad_sch"),
        static_artifacts.kicad_schematic.as_bytes(),
    )?;
    collector.add(
        "kicad_pcb",
        format!("{design_name}.kicad_pcb"),
        static_artifacts.kicad_pcb.as_bytes(),
    )?;
    collector.add(
        "kicad_project",
        format!("{design_name}.kicad_pro"),
        static_artifacts.kicad_project.as_bytes(),
    )?;
    for file in &static_artifacts.kicad_library_files {
        collector.add(
            "kicad_library",
            file.relative_path.as_str(),
            file.contents.as_bytes(),
        )?;
    }
    collector.add(
        "kicad_symbol_table",
        "sym-lib-table",
        static_artifacts.kicad_symbol_table.as_bytes(),
    )?;
    collector.add(
        "kicad_footprint_table",
        "fp-lib-table",
        static_artifacts.kicad_footprint_table.as_bytes(),
    )?;
    collector.add(
        "kicad_identity_map",
        format!("{design_name}.kicad-map.json"),
        kicad_identity_map_json.as_bytes(),
    )?;
    collector.add(
        "spice_netlist",
        format!("{design_name}.spice"),
        static_artifacts.spice.as_bytes(),
    )?;
    Ok(())
}

fn collect_predecessors(
    collector: &mut Collector,
    inputs: &ReleaseInputs<'_>,
    static_artifacts: &CompiledArtifacts,
    checked: Option<&CheckedCompiledArtifacts>,
) -> Result<(), ReleaseDiagnostic> {
    collector.add(
        "source",
        format!("source/{}.circuitc", inputs.design.name),
        inputs.source.as_bytes(),
    )?;
    let snapshot_id = inputs
        .design
        .product
        .catalog
        .as_ref()
        .ok_or_else(|| {
            diagnostic(
                "CC-RELEASE-CATALOG-001",
                "design.product.catalog",
                "release requires catalog evidence",
            )
        })?
        .snapshot_id
        .clone();
    collector.add(
        "catalog",
        format!("catalog/{snapshot_id}.json"),
        inputs.catalog_snapshot,
    )?;
    collect_static(
        collector,
        &inputs.design.name,
        static_artifacts,
        inputs.kicad_identity_map_json,
    )?;
    for (role, path, json) in [
        (
            "product_resolution",
            inputs.product.resolution_path.as_str(),
            inputs.product.resolution_json.as_bytes(),
        ),
        (
            "bom",
            inputs.product.bom_path.as_str(),
            inputs.product.bom_json.as_bytes(),
        ),
        (
            "placement",
            inputs.product.placement_path.as_str(),
            inputs.product.placement_json.as_bytes(),
        ),
        (
            "assembly",
            inputs.product.assembly_path.as_str(),
            inputs.product.assembly_json.as_bytes(),
        ),
    ] {
        collector.add(role, path, json)?;
    }
    collector.add(
        "fabrication_request",
        inputs.fabrication.bundle.request_path().as_str(),
        inputs.fabrication.bundle.request_json().as_bytes(),
    )?;
    collector.add(
        "fabrication_manifest",
        inputs.fabrication.bundle.manifest_path().as_str(),
        inputs.fabrication.bundle.manifest_json().as_bytes(),
    )?;
    for file in inputs.fabrication.bundle.files() {
        collector.add("fabrication_artifact", file.path.as_str(), &file.contents)?;
    }
    collector.add(
        "board_analysis_request",
        inputs.analysis.bundle.request_path().as_str(),
        inputs.analysis.bundle.request_json().as_bytes(),
    )?;
    collector.add(
        "board_analysis_result",
        inputs.analysis.bundle.result_path().as_str(),
        inputs.analysis.bundle.result_json().as_bytes(),
    )?;
    collector.add(
        "board_analysis_report",
        inputs.analysis.bundle.report_path().as_str(),
        inputs.analysis.bundle.report_json().as_bytes(),
    )?;
    for file in inputs.analysis.bundle.files() {
        let role = if file.path.as_str().ends_with("erc.normalized.json") {
            "erc_evidence"
        } else {
            "drc_evidence"
        };
        collector.add(role, file.path.as_str(), &file.contents)?;
    }
    if let Some(checked) = checked {
        for simulation in checked.simulations() {
            for (role, path, contents) in [
                (
                    "simulation_netlist",
                    simulation.netlist_path.as_str(),
                    simulation.netlist.as_bytes(),
                ),
                (
                    "simulation_request",
                    simulation.request_path.as_str(),
                    simulation.request_json.as_bytes(),
                ),
                (
                    "simulation_identity_map",
                    simulation.map_path.as_str(),
                    simulation.spice_identity_map_json.as_bytes(),
                ),
                (
                    "simulation_result",
                    simulation.result_path.as_str(),
                    simulation.result_json.as_bytes(),
                ),
                (
                    "simulation_report",
                    simulation.report_path.as_str(),
                    simulation.report_json.as_bytes(),
                ),
            ] {
                collector.add(role, path, contents)?;
            }
        }
        if let Some(routing) = checked.routing() {
            collector.add(
                "routing_request",
                routing.request_path.as_str(),
                routing.request_json.as_bytes(),
            )?;
            collector.add(
                "routing_result",
                routing.result_path.as_str(),
                routing.result_json.as_bytes(),
            )?;
            collector.add(
                "routing_projection",
                routing.projection_path.as_str(),
                routing.projection_json.as_bytes(),
            )?;
            collector.add(
                "routing_acceptance",
                format!(
                    "routing/{}/acceptance.json",
                    routing.request_identity_sha256
                ),
                inputs
                    .routing
                    .expect("authenticated routing applicability")
                    .acceptance_json
                    .as_bytes(),
            )?;
        }
    }
    if let Some(provenance) = inputs.tools.ohmnivore_provenance {
        collector.add(
            "ohmnivore_provenance",
            "toolchain/ohmnivore.provenance",
            provenance,
        )?;
    }
    if let Some(provenance) = inputs.tools.apgar_provenance {
        collector.add(
            "apgar_provenance",
            "toolchain/apgar-route.provenance",
            provenance,
        )?;
    }
    Ok(())
}

fn validation_outcomes(applicability: &Applicability) -> Vec<ValidationOutcome> {
    let mut outcomes = vec![
        ("source_elaboration", "source"),
        ("design_identity", "design_ir"),
        ("catalog_freshness", "catalog"),
        ("product_artifacts", "product_resolution"),
        ("fabrication_inventory", "fabrication_manifest"),
        ("erc", "erc_evidence"),
        ("drc", "drc_evidence"),
        ("unconnected", "drc_evidence"),
        ("schematic_parity", "drc_evidence"),
    ];
    if applicability.simulation {
        outcomes.push(("simulation", "simulation_report"));
    }
    if applicability.routing {
        outcomes.push(("routing", "routing_acceptance"));
    }
    outcomes.push(("artifact_inventory", "release_request"));
    outcomes
        .into_iter()
        .map(|(capability, evidence_role)| ValidationOutcome {
            capability: capability.to_owned(),
            evidence_role: evidence_role.to_owned(),
            outcome: "pass".to_owned(),
        })
        .collect()
}

/// Construct one complete accepted release closure from current authoritative inputs.
pub fn bind_release(inputs: &ReleaseInputs<'_>) -> Result<ReleaseBundle, ReleaseDiagnostic> {
    preflight_inputs(inputs)?;
    authenticate_source_design(inputs)?;
    let design_identity_sha256 = canonical_design_identity(inputs.design)?;
    if let FabricationCompilerArtifacts::Checked(checked) = inputs.compiler {
        verify_simulations(inputs.design, Some(checked))?;
    }
    let (static_artifacts, checked) = authenticated_compiler(inputs)?;
    authenticate_source(inputs, static_artifacts)?;
    verify_predecessors(inputs)?;
    verify_ohmnivore_tools(inputs)?;
    verify_routing(inputs, checked, static_artifacts)?;

    let mut collector = Collector::default();
    collect_predecessors(&mut collector, inputs, static_artifacts, checked)?;
    collector.sort();
    let artifacts = collector.bindings();
    let source = artifacts
        .iter()
        .find(|binding| binding.role == "source")
        .cloned()
        .expect("source is collected");
    let catalog = artifacts
        .iter()
        .find(|binding| binding.role == "catalog")
        .cloned()
        .expect("catalog is collected");
    let applicability = Applicability {
        simulation: !inputs.design.analyses.is_empty(),
        routing: !inputs.design.board.routing_requests.is_empty(),
    };
    let tools = collect_tools(inputs);
    let product_input_sha256 = product_input_sha256(inputs.product)?;
    let preimage = ReleaseIdentityPreimage {
        schema_name: REQUEST_SCHEMA.to_owned(),
        schema_version: SCHEMA_VERSION,
        design_name: inputs.design.name.clone(),
        variant_path: inputs.variant_path.to_owned(),
        variant_identity_sha256: inputs.product.variant_identity_sha256.clone(),
        product_input_sha256: product_input_sha256.clone(),
        source: source.clone(),
        design_identity_sha256: design_identity_sha256.clone(),
        catalog: catalog.clone(),
        applicability: applicability.clone(),
        tools: tools.clone(),
        artifacts: artifacts.clone(),
        resources: ResourcePolicy::default(),
    };
    let preimage_json = serde_json::to_vec(&preimage)
        .map_err(|error| diagnostic("CC-RELEASE-ENCODING-001", "request", error.to_string()))?;
    let mut identity = Sha256::new();
    identity.update(RELEASE_IDENTITY_DOMAIN);
    identity.update(preimage_json);
    let release_identity_sha256 = format!("{:x}", identity.finalize());
    let request = ReleaseRequest {
        schema_name: REQUEST_SCHEMA.to_owned(),
        schema_version: SCHEMA_VERSION,
        release_identity_sha256: release_identity_sha256.clone(),
        design_name: preimage.design_name,
        variant_path: preimage.variant_path,
        variant_identity_sha256: preimage.variant_identity_sha256,
        product_input_sha256: preimage.product_input_sha256,
        source: source.clone(),
        design_identity_sha256: design_identity_sha256.clone(),
        catalog: catalog.clone(),
        applicability: applicability.clone(),
        tools: tools.clone(),
        artifacts: artifacts.clone(),
        resources: preimage.resources,
    };
    let request_json = canonical_json(&request)?;
    let manifest = ReleaseManifest {
        schema_name: MANIFEST_SCHEMA.to_owned(),
        schema_version: SCHEMA_VERSION,
        release_identity_sha256: release_identity_sha256.clone(),
        request: RequestBinding {
            path: "request.json".to_owned(),
            byte_length: request_json.len() as u64,
            sha256: sha256(request_json.as_bytes()),
        },
        source,
        design_identity_sha256,
        applicability: applicability.clone(),
        tools,
        validations: validation_outcomes(&applicability),
        artifacts,
        all_pass: true,
    };
    let manifest_json = canonical_json(&manifest)?;
    let final_aggregate = collector
        .aggregate_bytes
        .checked_add(request_json.len())
        .and_then(|total| total.checked_add(manifest_json.len()))
        .ok_or_else(|| {
            diagnostic(
                "CC-RELEASE-RESOURCE-001",
                "release",
                "emitted release aggregate overflowed",
            )
        })?;
    if final_aggregate > MAX_AGGREGATE_BYTES || collector.artifacts.len() + 2 > MAX_FILES {
        return Err(diagnostic(
            "CC-RELEASE-RESOURCE-001",
            "release",
            "complete release exceeds its emitted aggregate or file-count limit",
        ));
    }
    let mut files = Vec::with_capacity(collector.artifacts.len() + 2);
    files.push(ReleaseFile {
        path: RelativeArtifactPath::try_new("request.json").expect("fixed request path is valid"),
        contents: request_json.as_bytes().to_vec(),
    });
    files.extend(collector.artifacts.into_iter().map(|artifact| ReleaseFile {
        path: artifact.path,
        contents: artifact.contents,
    }));
    files.push(ReleaseFile {
        path: RelativeArtifactPath::try_new("manifest.json").expect("fixed manifest path is valid"),
        contents: manifest_json.as_bytes().to_vec(),
    });
    let root = RelativeArtifactPath::try_new(format!("release/{release_identity_sha256}"))
        .expect("SHA-256 release root is valid");
    Ok(ReleaseBundle {
        release_identity_sha256,
        root,
        request_json,
        manifest_json,
        files,
    })
}

/// Assemble exact bytes read by a later materializer into an opaque candidate
/// bundle. This performs structural decoding only; [`verify_release`] supplies
/// the independent current-input authority.
pub fn assemble_release(
    root: RelativeArtifactPath,
    files: Vec<ReleaseFile>,
) -> Result<ReleaseBundle, ReleaseDiagnostic> {
    preflight_supplied_files(&files, "CC-RELEASE-DECODE-001")?;
    let identity = root
        .as_str()
        .strip_prefix("release/")
        .filter(|value| {
            value.len() == 64
                && value
                    .bytes()
                    .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
        })
        .ok_or_else(|| {
            diagnostic(
                "CC-RELEASE-DECODE-001",
                "root",
                "release root must be release/<lowercase-sha256>",
            )
        })?
        .to_owned();
    let request_files: Vec<_> = files
        .iter()
        .filter(|file| file.path.as_str() == "request.json")
        .collect();
    let manifest_files: Vec<_> = files
        .iter()
        .filter(|file| file.path.as_str() == "manifest.json")
        .collect();
    if request_files.len() != 1 || manifest_files.len() != 1 {
        return Err(diagnostic(
            "CC-RELEASE-DECODE-001",
            "files",
            "release candidate requires exactly one request.json and manifest.json",
        ));
    }
    let request_json = std::str::from_utf8(&request_files[0].contents)
        .map_err(|_| {
            diagnostic(
                "CC-RELEASE-DECODE-001",
                "request.json",
                "release request is not UTF-8",
            )
        })?
        .to_owned();
    let manifest_json = std::str::from_utf8(&manifest_files[0].contents)
        .map_err(|_| {
            diagnostic(
                "CC-RELEASE-DECODE-001",
                "manifest.json",
                "release manifest is not UTF-8",
            )
        })?
        .to_owned();
    Ok(ReleaseBundle {
        release_identity_sha256: identity,
        root,
        request_json,
        manifest_json,
        files,
    })
}

struct ExpectedPayload<'a> {
    role: &'static str,
    path: String,
    contents: &'a [u8],
}

fn independently_expected_payloads<'a>(
    inputs: &'a ReleaseInputs<'a>,
    static_artifacts: &'a CompiledArtifacts,
    checked: Option<&'a CheckedCompiledArtifacts>,
) -> Result<Vec<ExpectedPayload<'a>>, ReleaseDiagnostic> {
    let mut payloads = vec![
        ExpectedPayload {
            role: "source",
            path: format!("source/{}.circuitc", inputs.design.name),
            contents: inputs.source.as_bytes(),
        },
        ExpectedPayload {
            role: "catalog",
            path: format!(
                "catalog/{}.json",
                inputs
                    .design
                    .product
                    .catalog
                    .as_ref()
                    .ok_or_else(|| {
                        diagnostic(
                            "CC-RELEASE-CATALOG-001",
                            "design.product.catalog",
                            "release requires catalog evidence",
                        )
                    })?
                    .snapshot_id
            ),
            contents: inputs.catalog_snapshot,
        },
        ExpectedPayload {
            role: "kicad_schematic",
            path: format!("{}.kicad_sch", inputs.design.name),
            contents: static_artifacts.kicad_schematic.as_bytes(),
        },
        ExpectedPayload {
            role: "kicad_pcb",
            path: format!("{}.kicad_pcb", inputs.design.name),
            contents: static_artifacts.kicad_pcb.as_bytes(),
        },
        ExpectedPayload {
            role: "kicad_project",
            path: format!("{}.kicad_pro", inputs.design.name),
            contents: static_artifacts.kicad_project.as_bytes(),
        },
        ExpectedPayload {
            role: "kicad_symbol_table",
            path: "sym-lib-table".to_owned(),
            contents: static_artifacts.kicad_symbol_table.as_bytes(),
        },
        ExpectedPayload {
            role: "kicad_footprint_table",
            path: "fp-lib-table".to_owned(),
            contents: static_artifacts.kicad_footprint_table.as_bytes(),
        },
        ExpectedPayload {
            role: "kicad_identity_map",
            path: format!("{}.kicad-map.json", inputs.design.name),
            contents: inputs.kicad_identity_map_json.as_bytes(),
        },
        ExpectedPayload {
            role: "spice_netlist",
            path: format!("{}.spice", inputs.design.name),
            contents: static_artifacts.spice.as_bytes(),
        },
        ExpectedPayload {
            role: "product_resolution",
            path: inputs.product.resolution_path.as_str().to_owned(),
            contents: inputs.product.resolution_json.as_bytes(),
        },
        ExpectedPayload {
            role: "bom",
            path: inputs.product.bom_path.as_str().to_owned(),
            contents: inputs.product.bom_json.as_bytes(),
        },
        ExpectedPayload {
            role: "placement",
            path: inputs.product.placement_path.as_str().to_owned(),
            contents: inputs.product.placement_json.as_bytes(),
        },
        ExpectedPayload {
            role: "assembly",
            path: inputs.product.assembly_path.as_str().to_owned(),
            contents: inputs.product.assembly_json.as_bytes(),
        },
        ExpectedPayload {
            role: "fabrication_request",
            path: inputs.fabrication.bundle.request_path().as_str().to_owned(),
            contents: inputs.fabrication.bundle.request_json().as_bytes(),
        },
        ExpectedPayload {
            role: "fabrication_manifest",
            path: inputs
                .fabrication
                .bundle
                .manifest_path()
                .as_str()
                .to_owned(),
            contents: inputs.fabrication.bundle.manifest_json().as_bytes(),
        },
        ExpectedPayload {
            role: "board_analysis_request",
            path: inputs.analysis.bundle.request_path().as_str().to_owned(),
            contents: inputs.analysis.bundle.request_json().as_bytes(),
        },
        ExpectedPayload {
            role: "board_analysis_result",
            path: inputs.analysis.bundle.result_path().as_str().to_owned(),
            contents: inputs.analysis.bundle.result_json().as_bytes(),
        },
        ExpectedPayload {
            role: "board_analysis_report",
            path: inputs.analysis.bundle.report_path().as_str().to_owned(),
            contents: inputs.analysis.bundle.report_json().as_bytes(),
        },
    ];
    payloads.extend(
        static_artifacts
            .kicad_library_files
            .iter()
            .map(|file| ExpectedPayload {
                role: "kicad_library",
                path: file.relative_path.as_str().to_owned(),
                contents: file.contents.as_bytes(),
            }),
    );
    payloads.extend(
        inputs
            .fabrication
            .bundle
            .files()
            .iter()
            .map(|file| ExpectedPayload {
                role: "fabrication_artifact",
                path: file.path.as_str().to_owned(),
                contents: &file.contents,
            }),
    );
    payloads.extend(
        inputs
            .analysis
            .bundle
            .files()
            .iter()
            .map(|file| ExpectedPayload {
                role: if file.path.as_str().ends_with("erc.normalized.json") {
                    "erc_evidence"
                } else {
                    "drc_evidence"
                },
                path: file.path.as_str().to_owned(),
                contents: &file.contents,
            }),
    );
    if let Some(checked) = checked {
        for simulation in checked.simulations() {
            payloads.extend([
                ExpectedPayload {
                    role: "simulation_netlist",
                    path: simulation.netlist_path.as_str().to_owned(),
                    contents: simulation.netlist.as_bytes(),
                },
                ExpectedPayload {
                    role: "simulation_request",
                    path: simulation.request_path.as_str().to_owned(),
                    contents: simulation.request_json.as_bytes(),
                },
                ExpectedPayload {
                    role: "simulation_identity_map",
                    path: simulation.map_path.as_str().to_owned(),
                    contents: simulation.spice_identity_map_json.as_bytes(),
                },
                ExpectedPayload {
                    role: "simulation_result",
                    path: simulation.result_path.as_str().to_owned(),
                    contents: simulation.result_json.as_bytes(),
                },
                ExpectedPayload {
                    role: "simulation_report",
                    path: simulation.report_path.as_str().to_owned(),
                    contents: simulation.report_json.as_bytes(),
                },
            ]);
        }
        if let Some(routing) = checked.routing() {
            payloads.extend([
                ExpectedPayload {
                    role: "routing_request",
                    path: routing.request_path.as_str().to_owned(),
                    contents: routing.request_json.as_bytes(),
                },
                ExpectedPayload {
                    role: "routing_result",
                    path: routing.result_path.as_str().to_owned(),
                    contents: routing.result_json.as_bytes(),
                },
                ExpectedPayload {
                    role: "routing_projection",
                    path: routing.projection_path.as_str().to_owned(),
                    contents: routing.projection_json.as_bytes(),
                },
                ExpectedPayload {
                    role: "routing_acceptance",
                    path: format!(
                        "routing/{}/acceptance.json",
                        routing.request_identity_sha256
                    ),
                    contents: inputs
                        .routing
                        .ok_or_else(|| {
                            diagnostic(
                                "CC-RELEASE-APPLICABILITY-001",
                                "routing",
                                "routed compiler evidence requires route acceptance",
                            )
                        })?
                        .acceptance_json
                        .as_bytes(),
                },
            ]);
        }
    }
    if let Some(provenance) = inputs.tools.ohmnivore_provenance {
        payloads.push(ExpectedPayload {
            role: "ohmnivore_provenance",
            path: "toolchain/ohmnivore.provenance".to_owned(),
            contents: provenance,
        });
    }
    if let Some(provenance) = inputs.tools.apgar_provenance {
        payloads.push(ExpectedPayload {
            role: "apgar_provenance",
            path: "toolchain/apgar-route.provenance".to_owned(),
            contents: provenance,
        });
    }
    payloads.sort_by(|left, right| {
        left.path
            .cmp(&right.path)
            .then_with(|| left.role.cmp(right.role))
    });
    Ok(payloads)
}

fn independently_expected_tools(inputs: &ReleaseInputs<'_>) -> Vec<ToolBinding> {
    let mut tools = vec![
        tool_binding(
            "kicad",
            "kicad-cli",
            inputs.fabrication.host_version,
            "",
            inputs.fabrication.host_executable,
        ),
        tool_binding(
            "analysis_normalizer",
            "circuitc-kicad-analysis-normalizer",
            "1",
            "",
            &inputs.analysis.host.normalizer,
        ),
        tool_binding(
            "analysis_host_runner",
            "circuitc-kicad-analysis-host-runner",
            "1",
            "",
            &inputs.analysis.host.host_runner,
        ),
    ];
    if let Some(executable) = inputs.tools.ohmnivore_executable {
        tools.push(tool_binding(
            "ohmnivore",
            OHMNIVORE_BACKEND_NAME,
            OHMNIVORE_BACKEND_VERSION,
            OHMNIVORE_SOURCE_REVISION,
            executable,
        ));
    }
    if let Some(executable) = inputs.tools.apgar_executable {
        tools.push(tool_binding(
            "apgar",
            crate::routing::APGAR_TOOL_NAME,
            crate::routing::APGAR_TOOL_VERSION,
            crate::routing::PINNED_APGAR_SOURCE_REVISION,
            executable,
        ));
    }
    tools
}

fn independently_expected_validations(applicability: &Applicability) -> Vec<ValidationOutcome> {
    let mut validations = vec![
        ValidationOutcome {
            capability: "source_elaboration".to_owned(),
            evidence_role: "source".to_owned(),
            outcome: "pass".to_owned(),
        },
        ValidationOutcome {
            capability: "design_identity".to_owned(),
            evidence_role: "design_ir".to_owned(),
            outcome: "pass".to_owned(),
        },
        ValidationOutcome {
            capability: "catalog_freshness".to_owned(),
            evidence_role: "catalog".to_owned(),
            outcome: "pass".to_owned(),
        },
        ValidationOutcome {
            capability: "product_artifacts".to_owned(),
            evidence_role: "product_resolution".to_owned(),
            outcome: "pass".to_owned(),
        },
        ValidationOutcome {
            capability: "fabrication_inventory".to_owned(),
            evidence_role: "fabrication_manifest".to_owned(),
            outcome: "pass".to_owned(),
        },
        ValidationOutcome {
            capability: "erc".to_owned(),
            evidence_role: "erc_evidence".to_owned(),
            outcome: "pass".to_owned(),
        },
        ValidationOutcome {
            capability: "drc".to_owned(),
            evidence_role: "drc_evidence".to_owned(),
            outcome: "pass".to_owned(),
        },
        ValidationOutcome {
            capability: "unconnected".to_owned(),
            evidence_role: "drc_evidence".to_owned(),
            outcome: "pass".to_owned(),
        },
        ValidationOutcome {
            capability: "schematic_parity".to_owned(),
            evidence_role: "drc_evidence".to_owned(),
            outcome: "pass".to_owned(),
        },
    ];
    if applicability.simulation {
        validations.push(ValidationOutcome {
            capability: "simulation".to_owned(),
            evidence_role: "simulation_report".to_owned(),
            outcome: "pass".to_owned(),
        });
    }
    if applicability.routing {
        validations.push(ValidationOutcome {
            capability: "routing".to_owned(),
            evidence_role: "routing_acceptance".to_owned(),
            outcome: "pass".to_owned(),
        });
    }
    validations.push(ValidationOutcome {
        capability: "artifact_inventory".to_owned(),
        evidence_role: "release_request".to_owned(),
        outcome: "pass".to_owned(),
    });
    validations
}

fn independently_validate_emitted_inventory(
    payloads: &[ExpectedPayload<'_>],
    request_bytes: usize,
    manifest_bytes: usize,
) -> Result<(), ReleaseDiagnostic> {
    if payloads
        .len()
        .checked_add(2)
        .is_none_or(|count| count > MAX_FILES)
    {
        return Err(diagnostic(
            "CC-RELEASE-VERIFY-001",
            "artifacts",
            "independent emitted inventory exceeds the file-count limit",
        ));
    }
    let mut folded = BTreeSet::from(["manifest.json".to_owned(), "request.json".to_owned()]);
    let mut path_bytes = "manifest.json".len() + "request.json".len();
    let mut aggregate = request_bytes.checked_add(manifest_bytes).ok_or_else(|| {
        diagnostic(
            "CC-RELEASE-VERIFY-001",
            "release",
            "independent emitted aggregate overflowed",
        )
    })?;
    for payload in payloads {
        if payload.contents.len() > MAX_FILE_BYTES {
            return Err(diagnostic(
                "CC-RELEASE-VERIFY-001",
                &payload.path,
                "independent emitted artifact exceeds the per-file limit",
            ));
        }
        validate_release_path(&payload.path)?;
        let path = payload.path.to_ascii_lowercase();
        if folded.iter().any(|existing| {
            existing == &path
                || existing.starts_with(&(path.clone() + "/"))
                || path.starts_with(&(existing.clone() + "/"))
        }) {
            return Err(diagnostic(
                "CC-RELEASE-VERIFY-001",
                &payload.path,
                "independent emitted inventory has a case-folded path collision",
            ));
        }
        folded.insert(path);
        path_bytes = path_bytes.checked_add(payload.path.len()).ok_or_else(|| {
            diagnostic(
                "CC-RELEASE-VERIFY-001",
                "artifacts",
                "independent emitted path-byte count overflowed",
            )
        })?;
        aggregate = aggregate
            .checked_add(payload.contents.len())
            .ok_or_else(|| {
                diagnostic(
                    "CC-RELEASE-VERIFY-001",
                    "release",
                    "independent emitted aggregate overflowed",
                )
            })?;
    }
    if path_bytes > MAX_PATH_BYTES || aggregate > MAX_AGGREGATE_BYTES {
        return Err(diagnostic(
            "CC-RELEASE-VERIFY-001",
            "release",
            "independent emitted inventory exceeds its path-byte or aggregate limit",
        ));
    }
    Ok(())
}

/// Independently reconstruct and authenticate a supplied release closure.
pub fn verify_release(
    inputs: &ReleaseInputs<'_>,
    supplied: &ReleaseBundle,
) -> Result<VerifiedReleaseBundle, ReleaseDiagnostic> {
    preflight_supplied_files(&supplied.files, "CC-RELEASE-VERIFY-001")?;
    preflight_inputs(inputs)?;
    authenticate_source_design(inputs)?;
    let design_identity_sha256 = canonical_design_identity(inputs.design)?;
    let (static_artifacts, checked) = authenticated_compiler(inputs)?;
    authenticate_source(inputs, static_artifacts)?;
    verify_predecessors(inputs)?;
    verify_simulations(inputs.design, checked)?;
    verify_ohmnivore_tools(inputs)?;
    verify_routing(inputs, checked, static_artifacts)?;

    let request: ReleaseRequest =
        serde_json::from_str(&supplied.request_json).map_err(|error| {
            diagnostic(
                "CC-RELEASE-VERIFY-001",
                "request.json",
                format!("release request violates its strict schema: {error}"),
            )
        })?;
    if canonical_json(&request)? != supplied.request_json {
        return Err(diagnostic(
            "CC-RELEASE-VERIFY-001",
            "request.json",
            "release request is not exact canonical JSON plus one LF",
        ));
    }
    let manifest: ReleaseManifest =
        serde_json::from_str(&supplied.manifest_json).map_err(|error| {
            diagnostic(
                "CC-RELEASE-VERIFY-001",
                "manifest.json",
                format!("release manifest violates its strict schema: {error}"),
            )
        })?;
    if canonical_json(&manifest)? != supplied.manifest_json {
        return Err(diagnostic(
            "CC-RELEASE-VERIFY-001",
            "manifest.json",
            "release manifest is not exact canonical JSON plus one LF",
        ));
    }

    let payloads = independently_expected_payloads(inputs, static_artifacts, checked)?;
    let bindings: Vec<_> = payloads
        .iter()
        .map(|payload| bind_artifact(payload.role, &payload.path, payload.contents))
        .collect();
    let source = bindings
        .iter()
        .find(|binding| binding.role == "source")
        .cloned()
        .ok_or_else(|| {
            diagnostic(
                "CC-RELEASE-VERIFY-001",
                "artifacts",
                "independent release inventory omitted source",
            )
        })?;
    let catalog = bindings
        .iter()
        .find(|binding| binding.role == "catalog")
        .cloned()
        .ok_or_else(|| {
            diagnostic(
                "CC-RELEASE-VERIFY-001",
                "artifacts",
                "independent release inventory omitted catalog",
            )
        })?;
    let applicability = Applicability {
        simulation: !inputs.design.analyses.is_empty(),
        routing: !inputs.design.board.routing_requests.is_empty(),
    };
    let tools = independently_expected_tools(inputs);
    let product_input_sha256 = product_input_sha256(inputs.product)?;
    let preimage = ReleaseIdentityPreimage {
        schema_name: REQUEST_SCHEMA.to_owned(),
        schema_version: SCHEMA_VERSION,
        design_name: inputs.design.name.clone(),
        variant_path: inputs.variant_path.to_owned(),
        variant_identity_sha256: inputs.product.variant_identity_sha256.clone(),
        product_input_sha256: product_input_sha256.clone(),
        source: source.clone(),
        design_identity_sha256: design_identity_sha256.clone(),
        catalog: catalog.clone(),
        applicability: applicability.clone(),
        tools: tools.clone(),
        artifacts: bindings.clone(),
        resources: ResourcePolicy::default(),
    };
    let mut identity = Sha256::new();
    identity.update(RELEASE_IDENTITY_DOMAIN);
    identity.update(serde_json::to_vec(&preimage).map_err(|error| {
        diagnostic(
            "CC-RELEASE-VERIFY-001",
            "request.identity",
            error.to_string(),
        )
    })?);
    let release_identity_sha256 = format!("{:x}", identity.finalize());
    let expected_request = ReleaseRequest {
        schema_name: REQUEST_SCHEMA.to_owned(),
        schema_version: SCHEMA_VERSION,
        release_identity_sha256: release_identity_sha256.clone(),
        design_name: preimage.design_name,
        variant_path: preimage.variant_path,
        variant_identity_sha256: preimage.variant_identity_sha256,
        product_input_sha256: preimage.product_input_sha256,
        source: source.clone(),
        design_identity_sha256: design_identity_sha256.clone(),
        catalog,
        applicability: applicability.clone(),
        tools: tools.clone(),
        artifacts: bindings.clone(),
        resources: preimage.resources,
    };
    let expected_request_json = canonical_json(&expected_request)?;
    let expected_manifest = ReleaseManifest {
        schema_name: MANIFEST_SCHEMA.to_owned(),
        schema_version: SCHEMA_VERSION,
        release_identity_sha256: release_identity_sha256.clone(),
        request: RequestBinding {
            path: "request.json".to_owned(),
            byte_length: expected_request_json.len() as u64,
            sha256: sha256(expected_request_json.as_bytes()),
        },
        source,
        design_identity_sha256,
        applicability: applicability.clone(),
        tools,
        validations: independently_expected_validations(&applicability),
        artifacts: bindings,
        all_pass: true,
    };
    let expected_manifest_json = canonical_json(&expected_manifest)?;
    independently_validate_emitted_inventory(
        &payloads,
        expected_request_json.len(),
        expected_manifest_json.len(),
    )?;
    if request != expected_request
        || manifest != expected_manifest
        || supplied.release_identity_sha256 != release_identity_sha256
        || supplied.root.as_str() != format!("release/{release_identity_sha256}")
        || supplied.request_json != expected_request_json
        || supplied.manifest_json != expected_manifest_json
        || supplied.files.len() != payloads.len() + 2
        || supplied.files.first().is_none_or(|file| {
            file.path.as_str() != "request.json"
                || file.contents != expected_request_json.as_bytes()
        })
        || supplied.files.last().is_none_or(|file| {
            file.path.as_str() != "manifest.json"
                || file.contents != expected_manifest_json.as_bytes()
        })
        || supplied.files[1..supplied.files.len() - 1]
            .iter()
            .zip(&payloads)
            .any(|(file, payload)| {
                file.path.as_str() != payload.path || file.contents != payload.contents
            })
    {
        return Err(diagnostic(
            "CC-RELEASE-VERIFY-001",
            "release",
            "release request, manifest, inventory, or exact bytes do not match authoritative recomputation",
        ));
    }
    Ok(VerifiedReleaseBundle(supplied.clone()))
}

#[cfg(test)]
mod tests {
    use super::{
        Collector, enforce_resource_limit, preflight_supplied_files, validate_release_path,
    };
    use crate::RelativeArtifactPath;
    use crate::release::contract::{
        MAX_AGGREGATE_BYTES, MAX_FILE_BYTES, MAX_FILES, MAX_PATH_BYTES, ReleaseFile,
    };

    #[test]
    fn release_paths_reject_unsafe_or_reserved_forms() {
        for path in [
            "",
            "/absolute",
            "a/../b",
            "a\\b",
            "request.json",
            "manifest.json",
            "Request.JSON/child",
            "manifest.json/child",
            ".circuitc-release-transaction-x/file",
            ".CircuitC-Release-Transaction-x/file",
            "non-ascii-µ",
        ] {
            assert!(validate_release_path(path).is_err(), "accepted {path}");
        }
        assert!(validate_release_path("simulation/id/report.json").is_ok());
        assert!(validate_release_path(&"a".repeat(4096)).is_ok());
        assert!(validate_release_path(&"a".repeat(4097)).is_err());
    }

    #[test]
    fn release_paths_reject_case_folded_file_directory_collisions() {
        let mut collector = Collector::default();
        collector.add("test", "Foo", b"file").unwrap();
        assert_eq!(
            collector.add("test", "foo/bar", b"child").unwrap_err().code,
            "CC-RELEASE-INVENTORY-001"
        );
    }

    #[test]
    fn release_resource_limits_are_inclusive_and_reject_one_over() {
        for (name, limit) in [
            ("file", MAX_FILE_BYTES),
            ("count", MAX_FILES),
            ("path", MAX_PATH_BYTES),
            ("aggregate", MAX_AGGREGATE_BYTES),
        ] {
            enforce_resource_limit(name, limit, limit, "over limit").unwrap();
            assert_eq!(
                enforce_resource_limit(name, limit + 1, limit, "over limit")
                    .unwrap_err()
                    .code,
                "CC-RELEASE-RESOURCE-001"
            );
        }
    }

    #[test]
    fn supplied_candidate_preflight_rejects_count_and_folded_collisions() {
        let file = || ReleaseFile {
            path: RelativeArtifactPath::try_new("payload").unwrap(),
            contents: Vec::new(),
        };
        assert_eq!(
            preflight_supplied_files(&vec![file(); MAX_FILES + 1], "CC-RELEASE-DECODE-001")
                .unwrap_err()
                .code,
            "CC-RELEASE-DECODE-001"
        );
        let collision = vec![
            ReleaseFile {
                path: RelativeArtifactPath::try_new("Foo").unwrap(),
                contents: Vec::new(),
            },
            ReleaseFile {
                path: RelativeArtifactPath::try_new("foo/bar").unwrap(),
                contents: Vec::new(),
            },
        ];
        assert_eq!(
            preflight_supplied_files(&collision, "CC-RELEASE-DECODE-001")
                .unwrap_err()
                .code,
            "CC-RELEASE-DECODE-001"
        );
    }
}
