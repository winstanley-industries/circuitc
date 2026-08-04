use std::collections::BTreeMap;
use std::fmt;
use std::path::Path;

use crate::design::{Design, Diagnostic};
use crate::simulation::assert::evaluate_assertions;
use crate::simulation::lower::{self, SimulationInputBundle};
use crate::simulation::{
    AnalysisKind, AssertionStatus, AxisKind, CONTRACT_SCHEMA_VERSION, ContractDiagnostic,
    ExecutionStatus, MAX_CONTRACT_BYTES, NormalizedDiagnostic, OhmnivoreRunner, RESULT_SCHEMA_NAME,
    ResultAxis, SimulationResult, parse_request, sha256_hex,
};
use crate::spice::SpiceNameMap;
use crate::{kicad, spice};

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct KicadIdentity {
    pub uuid: String,
    pub semantic_path: String,
}

#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd)]
pub enum KicadLibraryFileKind {
    Symbol,
    Footprint,
}

#[derive(Clone, Debug, Eq, Ord, PartialEq, PartialOrd)]
pub struct RelativeArtifactPath(String);

impl RelativeArtifactPath {
    pub fn try_new(path: impl Into<String>) -> Result<Self, InvalidRelativeArtifactPath> {
        let path = path.into();
        if path.is_empty()
            || path.starts_with('/')
            || path.contains(['\\', '\0'])
            || path
                .split('/')
                .any(|segment| segment.is_empty() || matches!(segment, "." | ".."))
        {
            return Err(InvalidRelativeArtifactPath { path });
        }
        Ok(Self(path))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }

    pub fn as_path(&self) -> &Path {
        Path::new(&self.0)
    }

    pub fn into_string(self) -> String {
        self.0
    }
}

impl fmt::Display for RelativeArtifactPath {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.0)
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct InvalidRelativeArtifactPath {
    path: String,
}

impl fmt::Display for InvalidRelativeArtifactPath {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            formatter,
            "artifact path must be a canonical portable relative path with non-empty '/'-separated normal components and no backslash or NUL: {:?}",
            self.path
        )
    }
}

impl std::error::Error for InvalidRelativeArtifactPath {}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct KicadLibraryFile {
    pub kind: KicadLibraryFileKind,
    pub nickname: String,
    pub relative_path: RelativeArtifactPath,
    pub table_relative_path: RelativeArtifactPath,
    pub contents: String,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct CompiledArtifacts {
    pub kicad_schematic: String,
    pub kicad_pcb: String,
    pub kicad_project: String,
    pub kicad_library_files: Vec<KicadLibraryFile>,
    pub kicad_symbol_table: String,
    pub kicad_footprint_table: String,
    pub kicad_identities: Vec<KicadIdentity>,
    pub spice: String,
    pub spice_name_map: SpiceNameMap,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct CompiledSimulation {
    pub analysis_path: String,
    pub netlist_path: RelativeArtifactPath,
    pub request_path: RelativeArtifactPath,
    pub map_path: RelativeArtifactPath,
    pub result_path: RelativeArtifactPath,
    pub report_path: RelativeArtifactPath,
    pub netlist: String,
    pub request_json: String,
    pub spice_identity_map_json: String,
    pub result_json: String,
    pub report_json: String,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct CheckedCompiledArtifacts {
    pub static_artifacts: CompiledArtifacts,
    pub simulations: Vec<CompiledSimulation>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct CheckedCompileError {
    pub diagnostics: Vec<Diagnostic>,
    pub simulations: Vec<CompiledSimulation>,
}

impl fmt::Display for CheckedCompileError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        for (index, diagnostic) in self.diagnostics.iter().enumerate() {
            if index != 0 {
                formatter.write_str("\n")?;
            }
            write!(formatter, "{diagnostic}")?;
        }
        Ok(())
    }
}

impl std::error::Error for CheckedCompileError {}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct CompileError {
    pub diagnostics: Vec<Diagnostic>,
}

impl fmt::Display for CompileError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        for (index, diagnostic) in self.diagnostics.iter().enumerate() {
            if index != 0 {
                formatter.write_str("\n")?;
            }
            write!(formatter, "{diagnostic}")?;
        }
        Ok(())
    }
}

impl std::error::Error for CompileError {}

/// Compile static artifacts for a design with no declared simulation intent.
///
/// Call [`compile_checked`] when the design declares analyses; this entry point
/// deliberately fails closed rather than executing or weakening them.
pub fn compile(design: &Design) -> Result<CompiledArtifacts, CompileError> {
    design
        .validate()
        .map_err(|diagnostics| CompileError { diagnostics })?;
    let simulation_inputs =
        lower::lower_inputs(design).map_err(|diagnostics| CompileError { diagnostics })?;
    if let Some(input) = simulation_inputs.first() {
        return Err(CompileError {
            diagnostics: vec![Diagnostic {
                code: "CC-SIM-PHASE-001",
                path: format!("design.analyses.{}", input.analysis_path),
                related_path: None,
                message: "the static-only compile entry point does not execute declared simulation analyses; use checked compilation"
                    .to_owned(),
            }],
        });
    }
    compile_static_validated(design)
}

pub(crate) fn compile_static_validated(design: &Design) -> Result<CompiledArtifacts, CompileError> {
    if let Some(request) = design.board.routing_requests.first() {
        return Err(CompileError {
            diagnostics: vec![Diagnostic {
                code: "CC-AUTOROUTE-PHASE-001",
                path: format!("design.board.routing_requests.{}", request.path),
                related_path: None,
                message: "the static compiler cannot emit authored routing intent before APGAR exact routing and authenticated import"
                    .to_owned(),
            }],
        });
    }
    let validated_kicad = kicad::validate(design);
    if !validated_kicad.diagnostics.is_empty() {
        return Err(CompileError {
            diagnostics: validated_kicad.diagnostics,
        });
    }
    let lowered_spice = spice::lower_netlist(design);
    let kicad_library_files = kicad_library_files(design);
    let project = kicad::emit_project(design, &kicad_library_files, validated_kicad.identities);
    Ok(CompiledArtifacts {
        kicad_schematic: project.schematic,
        kicad_pcb: project.board,
        kicad_project: project.project,
        kicad_library_files,
        kicad_symbol_table: project.symbol_table,
        kicad_footprint_table: project.footprint_table,
        kicad_identities: project.identities,
        spice: lowered_spice.netlist,
        spice_name_map: lowered_spice.name_map,
    })
}

const MAX_AGGREGATE_RESULT_BYTES: usize = MAX_CONTRACT_BYTES;
const CHECK_FAILURE: &str = "CC-SIM-CHECK-001";
const CHECK_EXECUTION: &str = "CC-SIM-CHECK-002";
const CHECK_RESOURCE: &str = "CC-SIM-CHECK-003";
const CHECK_INTERNAL: &str = "CC-SIM-CHECK-004";
const CHECK_RESOURCE_MESSAGE: &str =
    "aggregate normalized simulation results exceeded the 64 MiB checked-compilation budget";
const CHECK_INTERNAL_RESULT_MESSAGE: &str =
    "checked simulation result did not satisfy its authenticated contract";

/// Compile all static artifacts only after every declared simulation has
/// produced a complete, authenticated result and every assertion has passed.
pub fn compile_checked(
    design: &Design,
    work_root: &Path,
) -> Result<CheckedCompiledArtifacts, CheckedCompileError> {
    let (static_artifacts, bundles) = prepare_checked(design)?;
    if bundles.is_empty() {
        return Ok(CheckedCompiledArtifacts {
            static_artifacts,
            simulations: Vec::new(),
        });
    }

    match OhmnivoreRunner::from_bazel_runfiles(work_root) {
        Ok(runner) => execute_checked(static_artifacts, bundles, |bundle| {
            runner.execute(
                bundle.netlist.as_bytes(),
                bundle.request_json.as_bytes(),
                bundle.spice_identity_map_json.as_bytes(),
            )
        }),
        Err(error) => execute_checked(static_artifacts, bundles, |_| Err(error.clone())),
    }
}

fn prepare_checked(
    design: &Design,
) -> Result<(CompiledArtifacts, Vec<SimulationInputBundle>), CheckedCompileError> {
    design
        .validate()
        .map_err(|diagnostics| CheckedCompileError {
            diagnostics,
            simulations: Vec::new(),
        })?;
    let bundles = lower::lower_inputs(design).map_err(|diagnostics| CheckedCompileError {
        diagnostics,
        simulations: Vec::new(),
    })?;
    let static_artifacts =
        compile_static_validated(design).map_err(|error| CheckedCompileError {
            diagnostics: error.diagnostics,
            simulations: Vec::new(),
        })?;
    Ok((static_artifacts, bundles))
}

fn execute_checked<F>(
    static_artifacts: CompiledArtifacts,
    bundles: Vec<SimulationInputBundle>,
    execute: F,
) -> Result<CheckedCompiledArtifacts, CheckedCompileError>
where
    F: FnMut(&SimulationInputBundle) -> Result<SimulationResult, ContractDiagnostic>,
{
    execute_checked_with_result_limit(
        static_artifacts,
        bundles,
        MAX_AGGREGATE_RESULT_BYTES,
        execute,
    )
}

fn execute_checked_with_result_limit<F>(
    static_artifacts: CompiledArtifacts,
    bundles: Vec<SimulationInputBundle>,
    result_byte_limit: usize,
    mut execute: F,
) -> Result<CheckedCompiledArtifacts, CheckedCompileError>
where
    F: FnMut(&SimulationInputBundle) -> Result<SimulationResult, ContractDiagnostic>,
{
    let resource_results = bundles
        .iter()
        .map(|bundle| canonical_failure_result(bundle, CHECK_RESOURCE, CHECK_RESOURCE_MESSAGE))
        .collect::<Result<Vec<_>, _>>()
        .map_err(|error| internal_checked_error(error, Vec::new()))?;
    let mut remaining_resource_bytes = resource_results
        .iter()
        .try_fold(0_usize, |total, (_, json)| total.checked_add(json.len()))
        .ok_or_else(|| internal_checked_error_message(Vec::new()))?;

    let mut result_bytes = 0_usize;
    let mut resource_exhausted = false;
    let mut all_checked = true;
    let mut simulations = Vec::with_capacity(bundles.len());
    let mut diagnostics = Vec::new();

    for (index, bundle) in bundles.iter().enumerate() {
        remaining_resource_bytes = remaining_resource_bytes
            .checked_sub(resource_results[index].1.len())
            .ok_or_else(|| internal_checked_error_message(simulations.clone()))?;

        let executed = execute(bundle);
        let candidate = if resource_exhausted {
            resource_results[index].clone()
        } else {
            canonical_executed_result(bundle, executed)
                .map_err(|error| internal_checked_error(error, simulations.clone()))?
        };
        let (mut result, mut result_json) = select_result_with_budget(
            candidate,
            &resource_results[index],
            result_bytes,
            remaining_resource_bytes,
            result_byte_limit,
            &mut resource_exhausted,
        );

        let evaluation = match evaluate_assertions(
            bundle.request_json.as_bytes(),
            bundle.spice_identity_map_json.as_bytes(),
            result_json.as_bytes(),
        ) {
            Ok(evaluation) => evaluation,
            Err(_) => {
                let fallback =
                    canonical_failure_result(bundle, CHECK_INTERNAL, CHECK_INTERNAL_RESULT_MESSAGE)
                        .map_err(|error| internal_checked_error(error, simulations.clone()))?;
                (result, result_json) = select_result_with_budget(
                    fallback,
                    &resource_results[index],
                    result_bytes,
                    remaining_resource_bytes,
                    result_byte_limit,
                    &mut resource_exhausted,
                );
                evaluate_assertions(
                    bundle.request_json.as_bytes(),
                    bundle.spice_identity_map_json.as_bytes(),
                    result_json.as_bytes(),
                )
                .map_err(|error| internal_checked_error(error, simulations.clone()))?
            }
        };
        result_bytes = result_bytes
            .checked_add(result_json.len())
            .ok_or_else(|| internal_checked_error_message(simulations.clone()))?;

        all_checked &= evaluation.checked_success;
        diagnostics.extend(checked_diagnostics(&result, &evaluation.report));
        simulations.push(CompiledSimulation {
            analysis_path: bundle.analysis_path.clone(),
            netlist_path: bundle.netlist_path.clone(),
            request_path: bundle.request_path.clone(),
            map_path: bundle.map_path.clone(),
            result_path: bundle.result_path.clone(),
            report_path: bundle.report_path.clone(),
            netlist: bundle.netlist.clone(),
            request_json: bundle.request_json.clone(),
            spice_identity_map_json: bundle.spice_identity_map_json.clone(),
            result_json,
            report_json: evaluation.report_json,
        });
    }

    if all_checked && diagnostics.is_empty() {
        Ok(CheckedCompiledArtifacts {
            static_artifacts,
            simulations,
        })
    } else {
        Err(CheckedCompileError {
            diagnostics,
            simulations,
        })
    }
}

fn select_result_with_budget(
    candidate: (SimulationResult, String),
    resource_result: &(SimulationResult, String),
    result_bytes: usize,
    remaining_resource_bytes: usize,
    result_byte_limit: usize,
    resource_exhausted: &mut bool,
) -> (SimulationResult, String) {
    if *resource_exhausted {
        return resource_result.clone();
    }
    let projected = result_bytes
        .checked_add(candidate.1.len())
        .and_then(|value| value.checked_add(remaining_resource_bytes));
    if projected.is_none_or(|projected| projected > result_byte_limit) {
        *resource_exhausted = true;
        resource_result.clone()
    } else {
        candidate
    }
}

fn canonical_executed_result(
    bundle: &SimulationInputBundle,
    executed: Result<SimulationResult, ContractDiagnostic>,
) -> Result<(SimulationResult, String), ContractDiagnostic> {
    match executed {
        Ok(result) => match result.to_canonical_json() {
            Ok(json) => Ok((result, json)),
            Err(_) => canonical_failure_result(
                bundle,
                CHECK_INTERNAL,
                "simulator adapter produced a non-canonical normalized result",
            ),
        },
        Err(error) => canonical_failure_result(bundle, error.code, &error.message),
    }
}

fn canonical_failure_result(
    bundle: &SimulationInputBundle,
    code: &str,
    message: &str,
) -> Result<(SimulationResult, String), ContractDiagnostic> {
    let request = parse_request(&bundle.request_json)?;
    let result = SimulationResult {
        schema_name: RESULT_SCHEMA_NAME.to_owned(),
        schema_version: CONTRACT_SCHEMA_VERSION,
        design: request.design,
        analysis_path: request.analysis.path,
        analysis_kind: request.analysis.kind,
        status: ExecutionStatus::Failed,
        request_sha256: sha256_hex(bundle.request_json.as_bytes()),
        map_sha256: sha256_hex(bundle.spice_identity_map_json.as_bytes()),
        axis: ResultAxis {
            kind: axis_kind(bundle.analysis_kind),
            samples: Vec::new(),
        },
        signals: Vec::new(),
        diagnostics: vec![NormalizedDiagnostic {
            code: code.to_owned(),
            message: message.to_owned(),
        }],
    };
    let json = result.to_canonical_json()?;
    Ok((result, json))
}

const fn axis_kind(kind: AnalysisKind) -> AxisKind {
    match kind {
        AnalysisKind::DcOperatingPoint => AxisKind::Scalar,
        AnalysisKind::AcLinearSweep => AxisKind::FrequencyHertz,
        AnalysisKind::Transient => AxisKind::TimeSeconds,
    }
}

fn checked_diagnostics(
    result: &SimulationResult,
    report: &crate::simulation::SimulationReport,
) -> Vec<Diagnostic> {
    let result_diagnostic = result
        .diagnostics
        .first()
        .map(|diagnostic| format!("{}: {}", diagnostic.code, diagnostic.message))
        .unwrap_or_else(|| "simulation did not complete".to_owned());
    let mut diagnostics = Vec::new();
    if result.status != ExecutionStatus::Completed {
        diagnostics.push(Diagnostic {
            code: CHECK_EXECUTION,
            path: format!("design.analyses.{}", result.analysis_path),
            related_path: None,
            message: format!("checked simulation did not complete: {result_diagnostic}"),
        });
    }
    for assertion in &report.assertions {
        let (code, message) = match assertion.status {
            AssertionStatus::Pass => continue,
            AssertionStatus::Fail => (
                CHECK_FAILURE,
                format!(
                    "assertion failed: actual {} is outside expected {} with absolute tolerance {} and relative tolerance {}",
                    assertion
                        .actual
                        .as_ref()
                        .map_or("<missing>", String::as_str),
                    assertion.expected,
                    assertion.absolute_tolerance,
                    assertion.relative_tolerance,
                ),
            ),
            AssertionStatus::Unsupported => (
                CHECK_EXECUTION,
                format!("assertion is unsupported: {result_diagnostic}"),
            ),
            AssertionStatus::Unevaluated => (
                CHECK_EXECUTION,
                format!("assertion was not evaluated: {result_diagnostic}"),
            ),
        };
        diagnostics.push(Diagnostic {
            code,
            path: format!("design.assertions.{}", assertion.path),
            related_path: Some(format!("design.analyses.{}", result.analysis_path)),
            message,
        });
    }
    diagnostics
}

fn internal_checked_error(
    error: ContractDiagnostic,
    simulations: Vec<CompiledSimulation>,
) -> CheckedCompileError {
    CheckedCompileError {
        diagnostics: vec![Diagnostic {
            code: CHECK_INTERNAL,
            path: "design.analyses".to_owned(),
            related_path: None,
            message: format!("checked simulation contract construction failed: {error}"),
        }],
        simulations,
    }
}

fn internal_checked_error_message(simulations: Vec<CompiledSimulation>) -> CheckedCompileError {
    CheckedCompileError {
        diagnostics: vec![Diagnostic {
            code: CHECK_INTERNAL,
            path: "design.analyses".to_owned(),
            related_path: None,
            message: "checked simulation evidence accounting overflowed".to_owned(),
        }],
        simulations,
    }
}

fn kicad_library_files(design: &Design) -> Vec<KicadLibraryFile> {
    let mut files = BTreeMap::new();
    for component in &design.components {
        let symbol = crate::library::symbol_library_file(&component.symbol.library_id)
            .expect("validated catalog symbol must have a publishable library file");
        files.insert(symbol.relative_path, symbol);
        if let Some(physical) = &component.physical {
            let footprint = crate::library::footprint_library_file(&physical.footprint.library_id)
                .expect("validated catalog footprint must have a publishable library file");
            files.insert(footprint.relative_path, footprint);
        }
    }
    files
        .into_iter()
        .map(|(relative_path, definition)| KicadLibraryFile {
            kind: definition.kind,
            nickname: definition.nickname.to_owned(),
            relative_path: RelativeArtifactPath::try_new(relative_path)
                .expect("catalog library file path must be a safe relative artifact path"),
            table_relative_path: RelativeArtifactPath::try_new(definition.table_relative_path)
                .expect("catalog table path must be a safe relative artifact path"),
            contents: definition.contents.to_owned(),
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeSet;
    use std::panic::catch_unwind;

    use crate::demo::voltage_divider;
    use crate::design::{
        ComponentValue, ConnectionState, CopperLayer, ModuleInstance, SimulationAnalysis,
        SimulationAnalysisKind, SimulationAssertion, SimulationSample,
    };
    use crate::quantity::{Quantity, Unit};
    use crate::simulation::{
        AnalysisKind, CONTRACT_SCHEMA_VERSION, ExecutionStatus, NormalizedDiagnostic,
        RESULT_SCHEMA_NAME, ResultAxis, ResultSignal, ResultUnit, SignalKind, SimulationResult,
        canonical_f64, parse_report, parse_request, parse_result, sha256_hex,
    };

    use super::{
        CHECK_EXECUTION, CHECK_FAILURE, CHECK_INTERNAL, CHECK_INTERNAL_RESULT_MESSAGE,
        CHECK_RESOURCE, CHECK_RESOURCE_MESSAGE, CompiledArtifacts, SimulationInputBundle,
        axis_kind, canonical_failure_result, compile, execute_checked,
        execute_checked_with_result_limit, prepare_checked,
    };

    fn checked_dc_design(paths: &[&str], assertions: bool) -> crate::design::Design {
        let mut design = voltage_divider();
        design.analyses = paths
            .iter()
            .map(|path| SimulationAnalysis {
                path: (*path).to_owned(),
                kind: SimulationAnalysisKind::DcOperatingPoint,
            })
            .collect();
        design.assertions = if assertions {
            paths
                .iter()
                .map(|path| SimulationAssertion {
                    path: format!("checks.{}", path.rsplit('.').next().unwrap()),
                    analysis_path: (*path).to_owned(),
                    net: "VOUT".to_owned(),
                    sample: SimulationSample::Scalar,
                    expected: Quantity::new(5, 0, Unit::Volt),
                    absolute_tolerance: Quantity::new(0, 0, Unit::Volt),
                    relative_tolerance: Quantity::new(0, 0, Unit::Dimensionless),
                })
                .collect()
        } else {
            Vec::new()
        };
        design.canonicalize();
        design
    }

    fn checked_all_kinds_design() -> crate::design::Design {
        let mut design = voltage_divider();
        design.analyses = vec![
            SimulationAnalysis {
                path: "simulation.transient".to_owned(),
                kind: SimulationAnalysisKind::Transient {
                    step: Quantity::new(125, -3, Unit::Second),
                    stop: Quantity::new(500, -3, Unit::Second),
                    start: Quantity::new(0, 0, Unit::Second),
                    uic: false,
                },
            },
            SimulationAnalysis {
                path: "simulation.dc".to_owned(),
                kind: SimulationAnalysisKind::DcOperatingPoint,
            },
            SimulationAnalysis {
                path: "simulation.ac".to_owned(),
                kind: SimulationAnalysisKind::AcLinearSweep {
                    source: "divider.analysis.input".to_owned(),
                    points: 4,
                    start_frequency: Quantity::new(1, 0, Unit::Hertz),
                    stop_frequency: Quantity::new(4, 0, Unit::Hertz),
                    magnitude: Quantity::new(1, 0, Unit::Volt),
                    phase: Quantity::new(0, 0, Unit::Degree),
                },
            },
        ];
        design.assertions = vec![
            SimulationAssertion {
                path: "checks.transient".to_owned(),
                analysis_path: "simulation.transient".to_owned(),
                net: "VOUT".to_owned(),
                sample: SimulationSample::Time(Quantity::new(500, -3, Unit::Second)),
                expected: Quantity::new(5, 0, Unit::Volt),
                absolute_tolerance: Quantity::new(0, 0, Unit::Volt),
                relative_tolerance: Quantity::new(0, 0, Unit::Dimensionless),
            },
            SimulationAssertion {
                path: "checks.dc".to_owned(),
                analysis_path: "simulation.dc".to_owned(),
                net: "VOUT".to_owned(),
                sample: SimulationSample::Scalar,
                expected: Quantity::new(5, 0, Unit::Volt),
                absolute_tolerance: Quantity::new(0, 0, Unit::Volt),
                relative_tolerance: Quantity::new(0, 0, Unit::Dimensionless),
            },
            SimulationAssertion {
                path: "checks.ac".to_owned(),
                analysis_path: "simulation.ac".to_owned(),
                net: "VOUT".to_owned(),
                sample: SimulationSample::Frequency(Quantity::new(3, 0, Unit::Hertz)),
                expected: Quantity::new(5, 0, Unit::Volt),
                absolute_tolerance: Quantity::new(0, 0, Unit::Volt),
                relative_tolerance: Quantity::new(0, 0, Unit::Dimensionless),
            },
        ];
        design.canonicalize();
        design
    }

    fn completed_dc_result(bundle: &SimulationInputBundle, actual: f64) -> SimulationResult {
        let request = parse_request(&bundle.request_json).unwrap();
        SimulationResult {
            schema_name: RESULT_SCHEMA_NAME.to_owned(),
            schema_version: CONTRACT_SCHEMA_VERSION,
            design: request.design,
            analysis_path: request.analysis.path,
            analysis_kind: request.analysis.kind,
            status: ExecutionStatus::Completed,
            request_sha256: sha256_hex(bundle.request_json.as_bytes()),
            map_sha256: sha256_hex(bundle.spice_identity_map_json.as_bytes()),
            axis: ResultAxis {
                kind: axis_kind(bundle.analysis_kind),
                samples: vec![crate::simulation::canonical_f64(0.0).unwrap()],
            },
            signals: vec![ResultSignal {
                kind: SignalKind::NetVoltage,
                canonical_identity: "VOUT".to_owned(),
                unit: ResultUnit::Volt,
                values: vec![crate::simulation::canonical_f64(actual).unwrap()],
            }],
            diagnostics: Vec::new(),
        }
    }

    fn completed_result_for_kind(bundle: &SimulationInputBundle, actual: f64) -> SimulationResult {
        let request = parse_request(&bundle.request_json).unwrap();
        let (samples, signal_kind) = match request.analysis.kind {
            AnalysisKind::DcOperatingPoint => (vec![0.0], SignalKind::NetVoltage),
            AnalysisKind::AcLinearSweep => {
                (vec![1.0, 2.0, 3.0, 4.0], SignalKind::NetVoltageMagnitude)
            }
            AnalysisKind::Transient => (vec![0.0, 0.125, 0.25, 0.375, 0.5], SignalKind::NetVoltage),
        };
        let value_count = samples.len();
        SimulationResult {
            schema_name: RESULT_SCHEMA_NAME.to_owned(),
            schema_version: CONTRACT_SCHEMA_VERSION,
            design: request.design,
            analysis_path: request.analysis.path,
            analysis_kind: request.analysis.kind,
            status: ExecutionStatus::Completed,
            request_sha256: sha256_hex(bundle.request_json.as_bytes()),
            map_sha256: sha256_hex(bundle.spice_identity_map_json.as_bytes()),
            axis: ResultAxis {
                kind: axis_kind(bundle.analysis_kind),
                samples: samples
                    .into_iter()
                    .map(|value| canonical_f64(value).unwrap())
                    .collect(),
            },
            signals: vec![ResultSignal {
                kind: signal_kind,
                canonical_identity: "VOUT".to_owned(),
                unit: ResultUnit::Volt,
                values: (0..value_count)
                    .map(|_| canonical_f64(actual).unwrap())
                    .collect(),
            }],
            diagnostics: Vec::new(),
        }
    }

    fn failed_dc_result(bundle: &SimulationInputBundle) -> SimulationResult {
        let request = parse_request(&bundle.request_json).unwrap();
        SimulationResult {
            schema_name: RESULT_SCHEMA_NAME.to_owned(),
            schema_version: CONTRACT_SCHEMA_VERSION,
            design: request.design,
            analysis_path: request.analysis.path,
            analysis_kind: request.analysis.kind,
            status: ExecutionStatus::Failed,
            request_sha256: sha256_hex(bundle.request_json.as_bytes()),
            map_sha256: sha256_hex(bundle.spice_identity_map_json.as_bytes()),
            axis: ResultAxis {
                kind: axis_kind(bundle.analysis_kind),
                samples: Vec::new(),
            },
            signals: Vec::new(),
            diagnostics: vec![NormalizedDiagnostic {
                code: "CC-SIM-EXECUTION-TEST".to_owned(),
                message: "deterministic injected execution failure".to_owned(),
            }],
        }
    }

    #[test]
    fn checked_execution_publishes_a_fully_bound_five_file_chain() {
        let design = checked_dc_design(&["simulation.dc"], true);
        let (static_artifacts, bundles) = prepare_checked(&design).unwrap();
        let checked = execute_checked(static_artifacts, bundles, |bundle| {
            Ok(completed_dc_result(bundle, 5.0))
        })
        .unwrap();

        assert_eq!(checked.simulations.len(), 1);
        let simulation = &checked.simulations[0];
        let directory = simulation
            .netlist_path
            .as_str()
            .strip_suffix("/analysis.spice")
            .unwrap();
        assert_eq!(
            simulation.request_path.as_str(),
            format!("{directory}/request.json")
        );
        assert_eq!(
            simulation.map_path.as_str(),
            format!("{directory}/spice-map.json")
        );
        assert_eq!(
            simulation.result_path.as_str(),
            format!("{directory}/result.json")
        );
        assert_eq!(
            simulation.report_path.as_str(),
            format!("{directory}/report.json")
        );

        let result = parse_result(&simulation.result_json).unwrap();
        result
            .verify_binding_bytes(
                simulation.request_json.as_bytes(),
                simulation.spice_identity_map_json.as_bytes(),
            )
            .unwrap();
        let report = parse_report(&simulation.report_json).unwrap();
        report
            .verify_binding_bytes(
                simulation.request_json.as_bytes(),
                simulation.spice_identity_map_json.as_bytes(),
                simulation.result_json.as_bytes(),
            )
            .unwrap();
        assert_eq!(report.summary.pass, 1);
        assert_eq!(report.summary.fail, 0);
        assert!(
            checked
                .static_artifacts
                .kicad_schematic
                .starts_with("(kicad_sch")
        );
    }

    #[test]
    fn checked_execution_evaluates_dc_ac_and_transient_in_canonical_order() {
        let design = checked_all_kinds_design();
        let (static_artifacts, bundles) = prepare_checked(&design).unwrap();
        assert_eq!(
            bundles
                .iter()
                .map(|bundle| bundle.analysis_path.as_str())
                .collect::<Vec<_>>(),
            vec!["simulation.ac", "simulation.dc", "simulation.transient"]
        );

        let checked = execute_checked(static_artifacts, bundles, |bundle| {
            Ok(completed_result_for_kind(bundle, 5.0))
        })
        .unwrap();

        assert_eq!(checked.simulations.len(), 3);
        assert_eq!(
            checked
                .simulations
                .iter()
                .map(|simulation| {
                    let result = parse_result(&simulation.result_json).unwrap();
                    let report = parse_report(&simulation.report_json).unwrap();
                    assert_eq!(result.status, ExecutionStatus::Completed);
                    assert_eq!(report.summary.pass, 1);
                    assert_eq!(report.summary.fail, 0);
                    result.analysis_kind
                })
                .collect::<Vec<_>>(),
            vec![
                AnalysisKind::AcLinearSweep,
                AnalysisKind::DcOperatingPoint,
                AnalysisKind::Transient,
            ]
        );
    }

    #[test]
    fn checked_execution_retains_noncompleted_ac_and_transient_evidence() {
        let design = checked_all_kinds_design();
        let (static_artifacts, bundles) = prepare_checked(&design).unwrap();
        let error = execute_checked(static_artifacts, bundles, |bundle| {
            if bundle.analysis_kind == AnalysisKind::DcOperatingPoint {
                Ok(completed_result_for_kind(bundle, 5.0))
            } else {
                Ok(failed_dc_result(bundle))
            }
        })
        .unwrap_err();

        assert_eq!(error.simulations.len(), 3);
        assert_eq!(
            error
                .simulations
                .iter()
                .map(|simulation| {
                    let result = parse_result(&simulation.result_json).unwrap();
                    let report = parse_report(&simulation.report_json).unwrap();
                    (
                        result.analysis_kind,
                        result.status,
                        report.summary.unevaluated,
                    )
                })
                .collect::<Vec<_>>(),
            vec![
                (AnalysisKind::AcLinearSweep, ExecutionStatus::Failed, 1),
                (
                    AnalysisKind::DcOperatingPoint,
                    ExecutionStatus::Completed,
                    0
                ),
                (AnalysisKind::Transient, ExecutionStatus::Failed, 1),
            ]
        );
    }

    #[test]
    fn checked_failure_retains_bound_evidence_and_machine_diagnostics() {
        let design = checked_dc_design(&["simulation.dc"], true);
        let (static_artifacts, bundles) = prepare_checked(&design).unwrap();
        let error = execute_checked(static_artifacts, bundles, |bundle| {
            Ok(completed_dc_result(bundle, 6.0))
        })
        .unwrap_err();

        assert_eq!(error.simulations.len(), 1);
        assert_eq!(error.diagnostics.len(), 1);
        assert_eq!(error.diagnostics[0].code, CHECK_FAILURE);
        assert_eq!(error.diagnostics[0].path, "design.assertions.checks.dc");
        let report = parse_report(&error.simulations[0].report_json).unwrap();
        assert_eq!(report.summary.fail, 1);
        report
            .verify_binding_bytes(
                error.simulations[0].request_json.as_bytes(),
                error.simulations[0].spice_identity_map_json.as_bytes(),
                error.simulations[0].result_json.as_bytes(),
            )
            .unwrap();
    }

    #[test]
    fn checked_execution_runs_every_analysis_in_canonical_order_after_failure() {
        use std::cell::RefCell;

        let design = checked_dc_design(&["simulation.z", "simulation.a"], true);
        let (static_artifacts, bundles) = prepare_checked(&design).unwrap();
        let executed = RefCell::new(Vec::new());
        let error = execute_checked(static_artifacts, bundles, |bundle| {
            executed.borrow_mut().push(bundle.analysis_path.clone());
            if bundle.analysis_path == "simulation.a" {
                Ok(failed_dc_result(bundle))
            } else {
                Ok(completed_dc_result(bundle, 5.0))
            }
        })
        .unwrap_err();

        assert_eq!(
            executed.into_inner(),
            vec!["simulation.a".to_owned(), "simulation.z".to_owned()]
        );
        assert_eq!(
            error
                .simulations
                .iter()
                .map(|simulation| simulation.analysis_path.as_str())
                .collect::<Vec<_>>(),
            vec!["simulation.a", "simulation.z"]
        );
        assert!(
            error
                .diagnostics
                .iter()
                .any(|item| item.code == CHECK_EXECUTION)
        );
    }

    #[test]
    fn non_completed_zero_assertion_analysis_fails_explicitly() {
        let design = checked_dc_design(&["simulation.dc"], false);
        let (static_artifacts, bundles) = prepare_checked(&design).unwrap();
        let error = execute_checked(static_artifacts, bundles, |bundle| {
            Ok(failed_dc_result(bundle))
        })
        .unwrap_err();

        assert_eq!(error.diagnostics.len(), 1);
        assert_eq!(error.diagnostics[0].code, CHECK_EXECUTION);
        assert_eq!(error.diagnostics[0].path, "design.analyses.simulation.dc");
        assert!(
            parse_report(&error.simulations[0].report_json)
                .unwrap()
                .assertions
                .is_empty()
        );
    }

    #[test]
    fn aggregate_result_budget_reserves_future_failure_evidence_without_omission() {
        use std::cell::Cell;

        let design = checked_dc_design(&["simulation.a", "simulation.b"], true);
        let (static_artifacts, bundles) = prepare_checked(&design).unwrap();
        let first_completed_bytes = completed_dc_result(&bundles[0], 5.0)
            .to_canonical_json()
            .unwrap()
            .len();
        let second_resource_bytes =
            canonical_failure_result(&bundles[1], CHECK_RESOURCE, CHECK_RESOURCE_MESSAGE)
                .unwrap()
                .1
                .len();
        let result_limit = first_completed_bytes
            .checked_add(second_resource_bytes)
            .unwrap()
            - 1;
        let first_resource_bytes =
            canonical_failure_result(&bundles[0], CHECK_RESOURCE, CHECK_RESOURCE_MESSAGE)
                .unwrap()
                .1
                .len();
        assert!(first_completed_bytes <= result_limit);
        assert!(
            first_resource_bytes + second_resource_bytes <= result_limit,
            "the injected budget must retain canonical resource evidence for every analysis"
        );
        assert!(
            first_completed_bytes + second_resource_bytes > result_limit,
            "the boundary must fit the current result alone but not reserved future evidence"
        );
        let executions = Cell::new(0_usize);
        let error =
            execute_checked_with_result_limit(static_artifacts, bundles, result_limit, |bundle| {
                executions.set(executions.get() + 1);
                Ok(completed_dc_result(bundle, 5.0))
            })
            .unwrap_err();

        assert_eq!(executions.get(), 2);
        assert_eq!(error.simulations.len(), 2);
        for simulation in &error.simulations {
            let result = parse_result(&simulation.result_json).unwrap();
            assert_eq!(result.status, ExecutionStatus::Failed);
            assert_eq!(result.diagnostics[0].code, CHECK_RESOURCE);
            assert_eq!(
                parse_report(&simulation.report_json)
                    .unwrap()
                    .summary
                    .unevaluated,
                1
            );
        }
    }

    #[test]
    fn evaluator_fallback_is_rebudgeted_before_result_accounting() {
        let design = checked_dc_design(&["simulation.dc"], true);
        let (static_artifacts, bundles) = prepare_checked(&design).unwrap();
        let mut stale = failed_dc_result(&bundles[0]);
        stale.request_sha256 = "0".repeat(64);
        stale.diagnostics = vec![NormalizedDiagnostic {
            code: "CC-SIM-TEST".to_owned(),
            message: "x".to_owned(),
        }];
        let stale_bytes = stale.to_canonical_json().unwrap().len();
        let internal_bytes =
            canonical_failure_result(&bundles[0], CHECK_INTERNAL, CHECK_INTERNAL_RESULT_MESSAGE)
                .unwrap()
                .1
                .len();
        assert!(
            stale_bytes < internal_bytes,
            "the injected boundary must distinguish pre- and post-fallback accounting"
        );

        let error =
            execute_checked_with_result_limit(static_artifacts, bundles, stale_bytes, |_| {
                Ok(stale.clone())
            })
            .unwrap_err();

        let result = parse_result(&error.simulations[0].result_json).unwrap();
        assert_eq!(result.status, ExecutionStatus::Failed);
        assert_eq!(result.diagnostics[0].code, CHECK_RESOURCE);
    }

    #[test]
    fn smaller_evaluator_fallback_releases_budget_for_later_completed_result() {
        let design = checked_dc_design(&["simulation.a", "simulation.b"], true);
        let (static_artifacts, bundles) = prepare_checked(&design).unwrap();
        let internal_bytes =
            canonical_failure_result(&bundles[0], CHECK_INTERNAL, CHECK_INTERNAL_RESULT_MESSAGE)
                .unwrap()
                .1
                .len();
        let second_resource_bytes =
            canonical_failure_result(&bundles[1], CHECK_RESOURCE, CHECK_RESOURCE_MESSAGE)
                .unwrap()
                .1
                .len();
        let second_completed_bytes = completed_dc_result(&bundles[1], 5.0)
            .to_canonical_json()
            .unwrap()
            .len();
        let result_limit = internal_bytes + second_completed_bytes;

        let stale = (1..=256)
            .find_map(|message_len| {
                let mut candidate = failed_dc_result(&bundles[0]);
                candidate.request_sha256 = "0".repeat(64);
                candidate.diagnostics = vec![NormalizedDiagnostic {
                    code: "CC-SIM-TEST".to_owned(),
                    message: "x".repeat(message_len),
                }];
                let bytes = candidate.to_canonical_json().unwrap().len();
                (bytes > internal_bytes && bytes + second_resource_bytes <= result_limit)
                    .then_some(candidate)
            })
            .expect("construct a stale result that distinguishes fallback accounting");

        let error =
            execute_checked_with_result_limit(static_artifacts, bundles, result_limit, |bundle| {
                if bundle.analysis_path == "simulation.a" {
                    Ok(stale.clone())
                } else {
                    Ok(completed_dc_result(bundle, 5.0))
                }
            })
            .unwrap_err();

        assert_eq!(error.simulations.len(), 2);
        assert_eq!(
            parse_result(&error.simulations[0].result_json)
                .unwrap()
                .diagnostics[0]
                .code,
            CHECK_INTERNAL
        );
        assert_eq!(
            parse_result(&error.simulations[1].result_json)
                .unwrap()
                .status,
            ExecutionStatus::Completed,
            "the smaller canonical fallback must release its unused bytes"
        );
    }

    #[test]
    fn compiles_reference_design_deterministically() {
        let design = voltage_divider();
        let first = compile(&design).expect("reference design must compile");
        let second = compile(&design).expect("reference design must compile repeatedly");
        assert_eq!(first, second);

        assert!(first.kicad_pcb.starts_with("(kicad_pcb\n"));
        assert!(first.kicad_pcb.contains("(generator \"circuitc\")"));
        assert!(first.kicad_pcb.contains("(net \"VOUT\")"));
        assert!(first.spice.contains("R1 VIN VOUT 10e3"));
        assert!(first.spice.contains("V1 VIN 0 DC 10"));
        assert_eq!(
            first
                .kicad_library_files
                .iter()
                .map(|file| file.relative_path.as_str())
                .collect::<Vec<_>>(),
            vec![
                "CircuitC.kicad_sym",
                "CircuitC.pretty/R_0603_1608Metric.kicad_mod"
            ]
        );
    }

    #[test]
    fn declared_simulation_intent_fails_closed_before_legacy_backend_lowering() {
        let mut design = voltage_divider();
        design.analyses = vec![
            SimulationAnalysis {
                path: "divider.simulation.transient".to_owned(),
                kind: SimulationAnalysisKind::DcOperatingPoint,
            },
            SimulationAnalysis {
                path: "divider.simulation.op".to_owned(),
                kind: SimulationAnalysisKind::DcOperatingPoint,
            },
        ];

        for candidate in [design.clone(), {
            let mut reversed = design;
            reversed.analyses.reverse();
            reversed
        }] {
            let error = compile(&candidate)
                .expect_err("declared analysis must not be silently lowered by the legacy backend");
            assert_eq!(error.diagnostics.len(), 1);
            assert_eq!(error.diagnostics[0].code, "CC-SIM-PHASE-001");
            assert_eq!(
                error.diagnostics[0].path,
                "design.analyses.divider.simulation.op"
            );
            assert_eq!(
                error.diagnostics[0].message,
                "the static-only compile entry point does not execute declared simulation analyses; use checked compilation"
            );
        }
    }

    #[test]
    fn no_connect_simulation_terminal_returns_diagnostics_without_panicking() {
        let mut design = voltage_divider();
        let component = design
            .components
            .iter_mut()
            .find(|component| component.reference == "R1")
            .expect("reference resistor must exist");
        component
            .connections
            .iter_mut()
            .find(|connection| connection.pin == "1")
            .expect("positive simulation terminal must exist")
            .state = ConnectionState::NoConnect;

        let result = catch_unwind(|| compile(&design));
        let diagnostics = result
            .expect("an unconnected simulation terminal must not panic in SPICE lowering")
            .expect_err("an unconnected simulation terminal must fail validation")
            .diagnostics;
        assert!(
            diagnostics
                .iter()
                .any(|diagnostic| diagnostic.code == "CC-SIM-003"),
            "missing CC-SIM-003: {diagnostics:#?}"
        );
    }

    #[test]
    fn schematic_participation_and_board_sheetfile_are_design_derived() {
        let design = voltage_divider();
        let artifacts = compile(&design).expect("reference design must compile");
        let physical_uuid = artifacts
            .kicad_identities
            .iter()
            .find(|identity| identity.semantic_path == "divider.r_top")
            .expect("physical symbol identity must exist")
            .uuid
            .as_str();
        let virtual_uuid = artifacts
            .kicad_identities
            .iter()
            .find(|identity| identity.semantic_path == "divider.analysis.input")
            .expect("virtual symbol identity must exist")
            .uuid
            .as_str();
        let physical_symbol = balanced_block_containing(
            &artifacts.kicad_schematic,
            "  (symbol\n",
            &format!("    (uuid \"{physical_uuid}\")"),
        );
        let virtual_symbol = balanced_block_containing(
            &artifacts.kicad_schematic,
            "  (symbol\n",
            &format!("    (uuid \"{virtual_uuid}\")"),
        );
        for expected in ["    (in_bom yes)", "    (on_board yes)"] {
            assert!(physical_symbol.contains(expected));
        }
        for expected in ["    (in_bom no)", "    (on_board no)"] {
            assert!(virtual_symbol.contains(expected));
        }

        let footprint = balanced_block_containing(
            &artifacts.kicad_pcb,
            "  (footprint ",
            "(property \"Reference\" \"R1\"",
        );
        assert!(footprint.contains("    (sheetfile \"voltage_divider.kicad_sch\")"));

        let mut renamed = design;
        renamed.name = "renamed_divider".to_owned();
        let renamed_artifacts = compile(&renamed).expect("renamed design must compile");
        let renamed_footprint = balanced_block_containing(
            &renamed_artifacts.kicad_pcb,
            "  (footprint ",
            "(property \"Reference\" \"R1\"",
        );
        assert!(renamed_footprint.contains("    (sheetfile \"renamed_divider.kicad_sch\")"));
        assert!(!renamed_footprint.contains("voltage_divider.kicad_sch"));
    }

    #[test]
    fn schematic_symbol_pins_use_catalog_symbol_pin_numbers() {
        let artifacts = compile(&voltage_divider()).expect("reference design must compile");
        let virtual_uuid = artifacts
            .kicad_identities
            .iter()
            .find(|identity| identity.semantic_path == "divider.analysis.input")
            .expect("virtual symbol identity must exist")
            .uuid
            .as_str();
        let virtual_symbol = balanced_block_containing(
            &artifacts.kicad_schematic,
            "  (symbol\n",
            &format!("    (uuid \"{virtual_uuid}\")"),
        );

        for pin in ["1", "2"] {
            assert!(
                virtual_symbol.contains(&format!("    (pin \"{pin}\"")),
                "virtual source symbol is missing catalog pin {pin}"
            );
        }
        for logical_pin in ["p", "n"] {
            assert!(
                !virtual_symbol.contains(&format!("    (pin \"{logical_pin}\"")),
                "virtual source symbol leaked logical pin {logical_pin} into KiCad"
            );
        }
    }

    #[test]
    fn schematic_instances_pin_the_project_root_and_references() {
        let artifacts = compile(&voltage_divider()).expect("reference design must compile");
        let root_uuid = artifacts
            .kicad_identities
            .iter()
            .find(|identity| identity.semantic_path == "design.schematic")
            .expect("schematic root identity must exist")
            .uuid
            .as_str();

        for (semantic_path, reference) in
            [("divider.r_top", "R1"), ("divider.analysis.input", "V1")]
        {
            let symbol_uuid = artifacts
                .kicad_identities
                .iter()
                .find(|identity| identity.semantic_path == semantic_path)
                .unwrap_or_else(|| panic!("symbol identity must exist for {semantic_path}"))
                .uuid
                .as_str();
            let symbol = balanced_block_containing(
                &artifacts.kicad_schematic,
                "  (symbol\n",
                &format!("    (uuid \"{symbol_uuid}\")"),
            );
            let expected = format!(
                concat!(
                    "    (instances\n",
                    "      (project \"voltage_divider\"\n",
                    "        (path \"/{}\"\n",
                    "          (reference \"{}\")\n",
                    "          (unit 1)\n"
                ),
                root_uuid, reference
            );
            assert!(
                symbol.contains(&expected),
                "{reference} is missing its project/root/reference instance stanza"
            );
        }

        assert!(
            artifacts
                .kicad_schematic
                .contains("  (sheet_instances\n    (path \"/\" (page \"1\"))\n  )")
        );
    }

    #[test]
    fn part_identity_values_round_trip_into_schematic_and_board_properties() {
        let artifacts = compile(&voltage_divider()).expect("reference design must compile");
        let physical_identity = artifacts
            .kicad_identities
            .iter()
            .find(|identity| identity.semantic_path == "divider.r_top")
            .expect("physical symbol identity must exist");
        let physical_symbol = balanced_block_containing(
            &artifacts.kicad_schematic,
            "  (symbol\n",
            &format!("    (uuid \"{}\")", physical_identity.uuid),
        );
        for expected in [
            "(property \"Footprint\" \"CircuitC:R_0603_1608Metric\"",
            "(property \"Description\" \"resistor\"",
            "(property \"Manufacturer\" \"Yageo\"",
            "(property \"MPN\" \"RC0603FR-0710KL\"",
        ] {
            assert!(
                physical_symbol.contains(expected),
                "physical schematic symbol is missing exact part identity property {expected}"
            );
        }

        let footprint = balanced_block_containing(
            &artifacts.kicad_pcb,
            "  (footprint \"CircuitC:R_0603_1608Metric\"",
            "(property \"Reference\" \"R1\"",
        );
        for expected in [
            "(property \"Manufacturer\" \"Yageo\"",
            "(property \"MPN\" \"RC0603FR-0710KL\"",
            "(property \"Description\" \"resistor\"",
        ] {
            assert!(
                footprint.contains(expected),
                "board footprint is missing exact part identity property {expected}"
            );
        }

        let virtual_identity = artifacts
            .kicad_identities
            .iter()
            .find(|identity| identity.semantic_path == "divider.analysis.input")
            .expect("virtual symbol identity must exist");
        let virtual_symbol = balanced_block_containing(
            &artifacts.kicad_schematic,
            "  (symbol\n",
            &format!("    (uuid \"{}\")", virtual_identity.uuid),
        );
        assert!(virtual_symbol.contains("(property \"Footprint\" \"\""));
        assert!(virtual_symbol.contains("(property \"Description\" \"dc_voltage_source\""));
        assert!(!virtual_symbol.contains("(property \"Manufacturer\""));
        assert!(!virtual_symbol.contains("(property \"MPN\""));
    }

    #[test]
    fn every_emitted_kicad_uuid_has_exactly_one_identity() {
        let reference = compile(&voltage_divider()).expect("reference design must compile");
        assert_identity_map_is_total(&reference);

        let source = include_str!("../examples/physical_no_connect.circuitc");
        let physical_no_connect =
            crate::frontend::compile_source("physical_no_connect.circuitc", source)
                .expect("physical no-connect fixture must compile");
        let physical_no_connect_repeat =
            crate::frontend::compile_source("physical_no_connect.circuitc", source)
                .expect("physical no-connect fixture must compile repeatedly");
        assert_eq!(
            physical_no_connect.artifacts, physical_no_connect_repeat.artifacts,
            "physical-only/no-connect artifacts must be byte-stable across repeat builds"
        );
        assert_eq!(
            physical_no_connect.kicad_identity_map, physical_no_connect_repeat.kicad_identity_map,
            "physical-only/no-connect identity maps must be byte-stable across repeat builds"
        );
        assert_identity_map_is_total(&physical_no_connect.artifacts);
    }

    #[test]
    fn relative_artifact_paths_reject_unsafe_components_at_construction() {
        for path in [
            "",
            "/absolute",
            "../escape",
            "nested/../escape",
            "CircuitC.pretty/./R.kicad_mod",
            "CircuitC.pretty//R.kicad_mod",
            "CircuitC.pretty/R.kicad_mod/",
            "CircuitC.pretty\\R.kicad_mod",
            "CircuitC.pretty/R\0.kicad_mod",
        ] {
            assert!(
                super::RelativeArtifactPath::try_new(path).is_err(),
                "unsafe artifact path must be rejected: {path:?}"
            );
        }
        let path =
            super::RelativeArtifactPath::try_new("CircuitC.pretty/R_0603_1608Metric.kicad_mod")
                .expect("catalog path must be valid");
        assert_eq!(path.as_str(), "CircuitC.pretty/R_0603_1608Metric.kicad_mod");
        assert_eq!(path.as_path(), std::path::Path::new(path.as_str()));
    }

    #[test]
    fn reference_library_tables_pin_the_complete_kicad_structure() {
        let artifacts = compile(&voltage_divider()).expect("reference design must compile");
        assert_eq!(
            artifacts.kicad_symbol_table,
            concat!(
                "(sym_lib_table\n",
                "  (version 7)\n",
                "  (lib (name \"CircuitC\")(type \"KiCad\")(uri \"${KIPRJMOD}/CircuitC.kicad_sym\")(options \"\")(descr \"CircuitC vendored symbols\"))\n",
                ")\n",
            )
        );
        assert_eq!(
            artifacts.kicad_footprint_table,
            concat!(
                "(fp_lib_table\n",
                "  (version 7)\n",
                "  (lib (name \"CircuitC\")(type \"KiCad\")(uri \"${KIPRJMOD}/CircuitC.pretty\")(options \"\")(descr \"CircuitC vendored footprints\"))\n",
                ")\n",
            )
        );
    }

    #[test]
    fn schematic_embeds_catalog_symbols_and_links_board_footprints_by_uuid() {
        let artifacts = compile(&voltage_divider()).expect("reference design must compile");
        let component_identity = artifacts
            .kicad_identities
            .iter()
            .find(|identity| identity.semantic_path == "divider.r_top")
            .expect("reference component identity must exist");
        assert!(
            artifacts
                .kicad_schematic
                .contains(&format!("    (uuid \"{}\")", component_identity.uuid)),
            "schematic symbol must carry the component identity UUID"
        );
        assert!(
            artifacts
                .kicad_pcb
                .contains(&format!("    (path \"/{}\")", component_identity.uuid)),
            "board footprint must link back to the schematic symbol UUID"
        );

        let embedded = balanced_block(&artifacts.kicad_schematic, "  (lib_symbols\n");
        assert!(embedded.contains("(symbol \"CircuitC:R\""));
        assert!(
            embedded.matches("(pin passive line").count() >= 2,
            "embedded resistor definition must retain both catalog pins"
        );
    }

    #[test]
    fn board_footprint_graphics_match_catalog_geometry_and_identity_map() {
        let artifacts = compile(&voltage_divider()).expect("reference design must compile");
        let footprint = balanced_block(
            &artifacts.kicad_pcb,
            "  (footprint \"CircuitC:R_0603_1608Metric\"",
        );
        for (semantic_path, marker, required) in [
            (
                "divider.r_top.footprint.graphic.silkscreen.top",
                "    (fp_line\n      (start -0.45 -0.5)",
                [
                    "(end 0.45 -0.5)",
                    "(stroke (width 0.12) (type default))",
                    "(layer \"F.SilkS\")",
                ],
            ),
            (
                "divider.r_top.footprint.graphic.silkscreen.bottom",
                "    (fp_line\n      (start -0.45 0.5)",
                [
                    "(end 0.45 0.5)",
                    "(stroke (width 0.12) (type default))",
                    "(layer \"F.SilkS\")",
                ],
            ),
            (
                "divider.r_top.footprint.graphic.courtyard",
                "    (fp_rect\n      (start -1.7 -0.75)",
                [
                    "(end 1.7 0.75)",
                    "(stroke (width 0.05) (type default))",
                    "(layer \"F.CrtYd\")",
                ],
            ),
        ] {
            let graphic = balanced_block(footprint, marker);
            for expected in required {
                assert!(
                    graphic.contains(expected),
                    "{semantic_path} is missing {expected}: {graphic}"
                );
            }
            let identity = artifacts
                .kicad_identities
                .iter()
                .find(|identity| identity.semantic_path == semantic_path)
                .unwrap_or_else(|| panic!("missing identity {semantic_path}"));
            assert!(
                graphic.contains(&format!("(uuid \"{}\")", identity.uuid)),
                "{semantic_path} graphic and identity UUID diverged: {graphic}"
            );
        }
    }

    #[test]
    fn back_layer_footprint_graphics_and_pads_use_back_layers() {
        let mut design = voltage_divider();
        design.components[0]
            .physical
            .as_mut()
            .expect("reference resistor must be physical")
            .placement
            .layer = CopperLayer::Back;
        let artifacts = compile(&design).expect("back-layer design must compile");
        let footprint = balanced_block_containing(
            &artifacts.kicad_pcb,
            "  (footprint ",
            "(property \"Reference\" \"R1\"",
        );
        for marker in [
            "    (fp_line\n      (start -0.45 -0.5)",
            "    (fp_line\n      (start -0.45 0.5)",
        ] {
            assert!(
                balanced_block(footprint, marker).contains("(layer \"B.SilkS\")"),
                "back-layer silkscreen graphic has the wrong layer"
            );
        }
        assert!(
            balanced_block(footprint, "    (fp_rect\n      (start -1.7 -0.75)")
                .contains("(layer \"B.CrtYd\")"),
            "back-layer courtyard graphic has the wrong layer"
        );
        for pad in ["1", "2"] {
            assert!(
                pad_stanza(footprint, pad).contains("(layers \"B.Cu\" \"B.Paste\" \"B.Mask\")"),
                "back-layer pad {pad} has the wrong copper/paste/mask layers"
            );
        }
    }

    #[test]
    fn schematic_connectivity_labels_cover_every_connected_symbol_pin() {
        let design = voltage_divider();
        let connected_pin_count = design
            .components
            .iter()
            .flat_map(|component| &component.connections)
            .filter(|connection| matches!(connection.state, ConnectionState::Connected(_)))
            .count();
        let artifacts = compile(&design).expect("reference design must compile");
        let label_count = artifacts
            .kicad_schematic
            .lines()
            .filter(|line| line.trim_start().starts_with("(global_label "))
            .count();

        assert_eq!(label_count, connected_pin_count);
        for net in ["VIN", "VOUT", "GND"] {
            assert!(
                artifacts
                    .kicad_schematic
                    .contains(&format!("  (global_label \"{net}\"")),
                "missing schematic label for {net}"
            );
        }
        assert!(global_label_at(
            &artifacts.kicad_schematic,
            "VIN",
            "81.28 77.47"
        ));
    }

    #[test]
    fn schematic_pin_coordinates_cover_every_orthogonal_rotation() {
        for (rotation, no_connect_at, connected_at) in [
            (90, "77.47 81.28", "85.09 81.28"),
            (180, "81.28 85.09", "81.28 77.47"),
            (270, "85.09 81.28", "77.47 81.28"),
        ] {
            let mut design = voltage_divider();
            let component = design
                .components
                .iter_mut()
                .find(|component| component.reference == "R1")
                .expect("reference resistor exists");
            component.simulation = None;
            component.schematic_placement.rotation_degrees = rotation;
            component
                .connections
                .iter_mut()
                .find(|connection| connection.pin == "1")
                .expect("pin 1 connection exists")
                .state = ConnectionState::NoConnect;

            let artifacts = compile(&design).expect("orthogonally rotated design must compile");
            assert!(
                artifacts
                    .kicad_schematic
                    .contains(&format!("  (no_connect (at {no_connect_at})")),
                "rotation {rotation} emitted the wrong no-connect coordinate"
            );
            assert!(
                global_label_at(&artifacts.kicad_schematic, "VOUT", connected_at),
                "rotation {rotation} emitted the wrong connected-pin coordinate"
            );
        }
    }

    #[test]
    fn schematic_connection_point_collisions_fail_before_emission() {
        let mut design = voltage_divider();
        let r1_position = design.components[0].schematic_placement.position;
        let r2 = design
            .components
            .iter_mut()
            .find(|component| component.reference == "R2")
            .expect("reference resistor exists");
        r2.schematic_placement.position =
            crate::design::PointNm::new(r1_position.x, r1_position.y + 7_620_000);
        r2.connections
            .iter_mut()
            .find(|connection| connection.pin == "1")
            .expect("R2 pin 1 connection exists")
            .state = ConnectionState::Connected("GND".to_owned());

        let diagnostics = compile(&design)
            .expect_err("differently connected schematic pins may not share a point")
            .diagnostics;
        let diagnostic = diagnostics
            .iter()
            .find(|diagnostic| diagnostic.code == "CC-KICAD-SCHEMATIC-002")
            .unwrap_or_else(|| panic!("missing schematic collision diagnostic: {diagnostics:#?}"));
        assert_eq!(
            diagnostic.related_path.as_deref(),
            Some("divider.r_bottom.connection.1"),
            "schematic collision diagnostic lost its counterpart: {diagnostic:#?}"
        );
    }

    #[test]
    fn coincident_no_connect_pins_fail_before_emission() {
        let mut design = voltage_divider();
        let r1_position = design.components[0].schematic_placement.position;
        design.components[0].simulation = None;
        design.components[0]
            .connections
            .iter_mut()
            .find(|connection| connection.pin == "2")
            .expect("R1 pin 2 connection exists")
            .state = ConnectionState::NoConnect;
        let r2 = design
            .components
            .iter_mut()
            .find(|component| component.reference == "R2")
            .expect("reference resistor exists");
        r2.schematic_placement.position =
            crate::design::PointNm::new(r1_position.x, r1_position.y + 7_620_000);
        r2.simulation = None;
        r2.connections
            .iter_mut()
            .find(|connection| connection.pin == "1")
            .expect("R2 pin 1 connection exists")
            .state = ConnectionState::NoConnect;

        let diagnostics = compile(&design)
            .expect_err("distinct no-connect pins may not share a schematic point")
            .diagnostics;
        let diagnostic = diagnostics
            .iter()
            .find(|diagnostic| diagnostic.code == "CC-KICAD-SCHEMATIC-002")
            .unwrap_or_else(|| panic!("missing schematic collision diagnostic: {diagnostics:#?}"));
        assert!(
            diagnostic
                .message
                .contains("no-connect pins may not share a connection point"),
            "unexpected no-connect collision diagnostic: {diagnostic:#?}"
        );
    }

    #[test]
    fn declaration_permutations_do_not_change_artifacts() {
        let design = voltage_divider();
        let expected = compile(&design).expect("reference design must compile");
        let mut permuted = design;
        permuted.nets.reverse();
        permuted.modules.reverse();
        permuted.components.reverse();
        permuted.board.routes.reverse();
        for module in &mut permuted.modules {
            module.ports.reverse();
        }
        for component in &mut permuted.components {
            component.connections.reverse();
            component.symbol.pins.reverse();
            if let Some(physical) = &mut component.physical {
                physical.footprint.pads.reverse();
                physical.pin_pad_bindings.reverse();
            }
        }
        assert_eq!(
            compile(&permuted).expect("permuted design must compile"),
            expected
        );
    }

    #[test]
    fn explicit_no_connect_emits_schematic_and_kicad_board_intent() {
        let mut design = voltage_divider();
        let component = design
            .components
            .iter_mut()
            .find(|component| component.reference == "R1")
            .expect("reference resistor exists");
        component.simulation = None;
        component.connections[0].state = ConnectionState::NoConnect;
        design.canonicalize();

        let artifacts = compile(&design).expect("physical-only no-connect must compile");
        assert_component_value(&artifacts, "divider.r_top", "R1", "10kΩ");
        assert!(artifacts.kicad_schematic.contains("  (no_connect (at "));
        assert!(artifacts.kicad_identities.iter().any(|identity| {
            identity.semantic_path == "divider.r_top.connection.1"
                && artifacts.kicad_schematic.contains(&identity.uuid)
        }));

        let footprint_start = artifacts
            .kicad_pcb
            .find("(property \"Reference\" \"R1\"")
            .expect("R1 footprint must exist");
        let footprint_end = artifacts.kicad_pcb[footprint_start..]
            .find("\n  )")
            .map(|offset| footprint_start + offset)
            .expect("R1 footprint must terminate");
        let footprint = &artifacts.kicad_pcb[footprint_start..footprint_end];
        let pad = pad_stanza(footprint, "1");
        assert!(
            pad.contains("(net \"unconnected-(R1-Pad1)\")"),
            "no-connect pad must receive KiCad's deterministic parity-only net"
        );
        assert!(
            !design
                .nets
                .iter()
                .any(|net| net.name.contains("unconnected-"))
        );

        let connected_pad = pad_stanza(footprint, "2");
        assert!(connected_pad.contains("(net \"VOUT\")"));

        design
            .components
            .iter_mut()
            .find(|component| component.reference == "R1")
            .expect("reference resistor exists")
            .value = ComponentValue::Resistance(Quantity::new(22, 3, Unit::Ohm));
        let changed = compile(&design).expect("changed exact value must compile");
        assert_component_value(&changed, "divider.r_top", "R1", "22kΩ");
    }

    #[test]
    fn source_authored_physical_no_connect_fixture_compiles() {
        let source = include_str!("../examples/physical_no_connect.circuitc");
        let compiled = crate::frontend::compile_source("physical_no_connect.circuitc", source)
            .expect("source-authored physical no-connect fixture must compile");
        let component = compiled
            .elaborated
            .design
            .components
            .iter()
            .find(|component| component.reference == "R1")
            .expect("physical-only resistor must exist");
        assert!(component.simulation.is_none());
        assert!(
            component
                .connections
                .iter()
                .any(|connection| matches!(&connection.state, ConnectionState::Connected(net) if net == "TEST"))
        );
        assert!(
            component
                .connections
                .iter()
                .any(|connection| connection.state == ConnectionState::NoConnect)
        );
        assert!(!compiled.artifacts.spice.contains("R1 "));
        assert!(compiled.artifacts.spice.contains("V1 TEST 0 DC 1"));
        assert!(compiled.artifacts.spice.contains("R2 TEST 0 1e3"));
        assert_eq!(
            compiled
                .artifacts
                .spice
                .lines()
                .filter(|line| line.starts_with('V'))
                .count(),
            1,
            "fixture SPICE netlist must contain one ideal voltage source"
        );
        assert_component_value(&compiled.artifacts, "board_only.unused", "R1", "10kΩ");
        assert!(global_label_at(
            &compiled.artifacts.kicad_schematic,
            "TEST",
            "77.47 81.28"
        ));
        assert!(
            compiled
                .artifacts
                .kicad_schematic
                .contains("  (no_connect (at 85.09 81.28)")
        );

        let footprint_start = compiled
            .artifacts
            .kicad_pcb
            .find("(property \"Reference\" \"R1\"")
            .expect("R1 footprint must exist");
        let footprint = &compiled.artifacts.kicad_pcb[footprint_start..];
        assert!(pad_stanza(footprint, "1").contains("(net \"TEST\")"));
        assert!(pad_stanza(footprint, "2").contains("(net \"unconnected-(R1-Pad2)\")"));
    }

    #[test]
    fn compile_returns_diagnostics_instead_of_panicking_on_extreme_coordinates() {
        let mut design = voltage_divider();
        let physical = design.components[0]
            .physical
            .as_mut()
            .expect("reference resistor is physical");
        physical.placement.rotation_degrees = 90;
        physical.footprint.pads[0].offset.x = i64::MIN;
        let result = catch_unwind(|| compile(&design));
        assert!(
            result.is_ok(),
            "compile must be total over public IR values"
        );
        assert!(result.expect("checked above").is_err());

        let mut design = voltage_divider();
        design.board.outline.origin.x = i64::MAX;
        design.board.outline.size.width = i64::MAX;
        let result = catch_unwind(|| compile(&design));
        assert!(result.is_ok(), "outline overflow must not panic");
        assert!(result.expect("checked above").is_err());

        let mut design = voltage_divider();
        design.components[0].schematic_placement.position.y = crate::design::MAX_ABS_COORDINATE_NM;
        let result = catch_unwind(|| compile(&design));
        let diagnostics = result
            .expect("derived schematic pin overflow must not panic")
            .expect_err("derived schematic pin beyond the envelope must fail")
            .diagnostics;
        assert!(
            diagnostics
                .iter()
                .any(|diagnostic| diagnostic.code == "CC-KICAD-SCHEMATIC-001")
        );
    }

    #[test]
    fn invalid_kicad_catalog_bindings_return_diagnostics_without_panicking() {
        let mut design = voltage_divider();
        design.components[0].part.manufacturer = Some("Texas Instruments".to_owned());
        design.components[0].part.manufacturer_part_number = Some("CC3551EN0UNRGER".to_owned());
        let diagnostics = compile(&design)
            .expect_err("incoherent manufacturer part identity must fail")
            .diagnostics;
        assert!(
            diagnostics
                .iter()
                .any(|diagnostic| diagnostic.code == "CC-KICAD-PART-001")
        );

        let mut design = voltage_divider();
        design.components[0].symbol.library_id = "CircuitC:VDC".to_owned();
        let diagnostics = compile(&design)
            .expect_err("part and symbol catalog drift must fail")
            .diagnostics;
        assert!(
            diagnostics
                .iter()
                .any(|diagnostic| diagnostic.code == "CC-KICAD-SYMBOL-001")
        );

        let mut design = voltage_divider();
        design.components[0].symbol.library_id = "CircuitC:UNKNOWN".to_owned();
        let result = catch_unwind(|| compile(&design));
        let diagnostics = result
            .expect("unknown symbols must not panic")
            .expect_err("unknown symbols must fail")
            .diagnostics;
        assert!(
            diagnostics
                .iter()
                .any(|diagnostic| diagnostic.code == "CC-KICAD-SYMBOL-006")
        );

        let mut design = voltage_divider();
        design.components[0].symbol.pins[0].electrical_type =
            crate::design::ElectricalPinType::PowerOutput;
        let diagnostics = compile(&design)
            .expect_err("catalog electrical-type drift must fail")
            .diagnostics;
        assert!(
            diagnostics
                .iter()
                .any(|diagnostic| diagnostic.code == "CC-KICAD-SYMBOL-002")
        );

        let mut design = voltage_divider();
        design.components[0].symbol.pins[0].symbol_pin = "3".to_owned();
        let diagnostics = compile(&design)
            .expect_err("missing catalog pin binding must fail")
            .diagnostics;
        assert!(
            diagnostics
                .iter()
                .any(|diagnostic| diagnostic.code == "CC-KICAD-SYMBOL-003")
        );
        assert!(
            diagnostics
                .iter()
                .any(|diagnostic| diagnostic.code == "CC-KICAD-SYMBOL-004")
        );

        let mut design = voltage_divider();
        design.components[0].physical = None;
        let diagnostics = compile(&design)
            .expect_err("physical catalog parts require physical implementations")
            .diagnostics;
        for code in ["CC-KICAD-SYMBOL-005", "CC-KICAD-FOOTPRINT-004"] {
            assert!(
                diagnostics.iter().any(|diagnostic| diagnostic.code == code),
                "missing {code}: {diagnostics:#?}"
            );
        }

        let mut design = voltage_divider();
        let physical = design.components[0]
            .physical
            .clone()
            .expect("reference resistor is physical");
        design.components[2].physical = Some(physical);
        let diagnostics = crate::kicad::validate(&design).diagnostics;
        assert!(
            diagnostics
                .iter()
                .any(|diagnostic| diagnostic.code == "CC-KICAD-FOOTPRINT-003"),
            "missing CC-KICAD-FOOTPRINT-003: {diagnostics:#?}"
        );

        let mut design = voltage_divider();
        design.components[0]
            .physical
            .as_mut()
            .expect("reference resistor is physical")
            .footprint
            .library_id = "CircuitC:R_0805_2012Metric".to_owned();
        let diagnostics = compile(&design)
            .expect_err("footprint identity drift from the part catalog must fail")
            .diagnostics;
        assert!(
            diagnostics
                .iter()
                .any(|diagnostic| diagnostic.code == "CC-KICAD-FOOTPRINT-002"),
            "missing CC-KICAD-FOOTPRINT-002: {diagnostics:#?}"
        );

        let mut design = voltage_divider();
        let physical = design.components[0]
            .physical
            .as_mut()
            .expect("reference resistor is physical");
        physical.footprint.pads[0].size.width += 1;
        let diagnostics = compile(&design)
            .expect_err("catalog geometry drift must fail")
            .diagnostics;
        assert!(
            diagnostics
                .iter()
                .any(|diagnostic| diagnostic.code == "CC-KICAD-FOOTPRINT-001")
        );

        let mut design = voltage_divider();
        let component = &mut design.components[0];
        let physical = component
            .physical
            .as_mut()
            .expect("reference resistor is physical");
        physical.pin_pad_bindings[0].pad = "2".to_owned();
        physical.pin_pad_bindings[1].pad = "1".to_owned();
        let diagnostics = compile(&design)
            .expect_err("cross-mapped symbol pins and pads must fail closed")
            .diagnostics;
        assert!(
            diagnostics
                .iter()
                .any(|diagnostic| diagnostic.code == "CC-KICAD-BIND-001")
        );
    }

    #[test]
    fn coordinate_boundary_matrix_never_panics() {
        let values = [
            i64::MIN,
            -crate::design::MAX_ABS_COORDINATE_NM - 1,
            -crate::design::MAX_ABS_COORDINATE_NM,
            0,
            crate::design::MAX_ABS_COORDINATE_NM,
            crate::design::MAX_ABS_COORDINATE_NM + 1,
            i64::MAX,
        ];
        for rotation in [0, 90, 180, 270] {
            for &value in &values {
                let mut design = voltage_divider();
                let physical = design.components[0]
                    .physical
                    .as_mut()
                    .expect("reference resistor is physical");
                physical.placement.rotation_degrees = rotation;
                physical.placement.position.x = value;
                physical.footprint.pads[0].offset.y = value;
                let result = catch_unwind(|| compile(&design));
                assert!(
                    result.is_ok(),
                    "compile panicked for rotation {rotation} and coordinate {value}"
                );
            }
        }
    }

    #[test]
    fn rejects_component_paths_that_collide_with_generated_kicad_paths() {
        let mut design = voltage_divider();
        design.modules.extend([
            ModuleInstance {
                path: "root".to_owned(),
                ports: Vec::new(),
            },
            ModuleInstance {
                path: "root.x".to_owned(),
                ports: Vec::new(),
            },
            ModuleInstance {
                path: "root.x.footprint".to_owned(),
                ports: Vec::new(),
            },
            ModuleInstance {
                path: "root.x.footprint.pad".to_owned(),
                ports: Vec::new(),
            },
        ]);
        design.components[0].path = "root.x".to_owned();
        design.components[1].path = "root.x.footprint.pad.1".to_owned();

        let diagnostics = compile(&design)
            .expect_err("rendered KiCad semantic paths must be globally unique")
            .diagnostics;
        assert!(diagnostics.iter().any(|diagnostic| {
            diagnostic.code == "CC-KICAD-ID-002" && diagnostic.path == "root.x.footprint.pad.1"
        }));
    }

    #[test]
    fn rejects_route_paths_that_collide_with_component_paths() {
        let mut design = voltage_divider();
        design.board.routes[0].path = design.components[0].path.clone();

        let diagnostics = compile(&design)
            .expect_err("component and route semantic paths must not collide")
            .diagnostics;
        assert!(diagnostics.iter().any(|diagnostic| {
            diagnostic.code == "CC-KICAD-ID-002" && diagnostic.path == "divider.r_top"
        }));
    }

    #[test]
    fn route_uuid_is_stable_when_geometry_changes() {
        let design = voltage_divider();
        let first = compile(&design).expect("reference design must compile");
        let first_uuid = segment_uuid(&first.kicad_pcb);

        let mut moved = design;
        moved.board.routes[0].start.x += 1;
        let second = compile(&moved).expect("moved route must compile");
        assert_eq!(first_uuid, segment_uuid(&second.kicad_pcb));
    }

    fn assert_identity_map_is_total(artifacts: &CompiledArtifacts) {
        let emitted: Vec<_> = [&artifacts.kicad_schematic, &artifacts.kicad_pcb]
            .into_iter()
            .flat_map(|artifact| emitted_kicad_uuids(artifact))
            .collect();
        let emitted_set: BTreeSet<_> = emitted.iter().copied().collect();
        assert_eq!(
            emitted.len(),
            emitted_set.len(),
            "every emitted KiCad UUID must be globally unique"
        );

        let identity_uuids: BTreeSet<_> = artifacts
            .kicad_identities
            .iter()
            .map(|identity| identity.uuid.as_str())
            .collect();
        let identity_paths: BTreeSet<_> = artifacts
            .kicad_identities
            .iter()
            .map(|identity| identity.semantic_path.as_str())
            .collect();
        assert_eq!(
            identity_uuids.len(),
            artifacts.kicad_identities.len(),
            "identity-map UUIDs must be unique"
        );
        assert_eq!(
            identity_paths.len(),
            artifacts.kicad_identities.len(),
            "identity-map semantic paths must be unique"
        );
        assert_eq!(
            emitted_set, identity_uuids,
            "emitted KiCad UUIDs and identity-map UUIDs must have exact set equality"
        );
    }

    fn emitted_kicad_uuids(mut artifact: &str) -> Vec<&str> {
        let mut uuids = Vec::new();
        while let Some((_, after_marker)) = artifact.split_once("(uuid \"") {
            let Some((uuid, remainder)) = after_marker.split_once("\")") else {
                break;
            };
            uuids.push(uuid);
            artifact = remainder;
        }
        uuids
    }

    fn assert_component_value(
        artifacts: &CompiledArtifacts,
        semantic_path: &str,
        reference: &str,
        value: &str,
    ) {
        let identity = artifacts
            .kicad_identities
            .iter()
            .find(|identity| identity.semantic_path == semantic_path)
            .unwrap_or_else(|| panic!("missing component identity {semantic_path}"));
        let schematic = balanced_block_containing(
            &artifacts.kicad_schematic,
            "  (symbol\n",
            &format!("    (uuid \"{}\")", identity.uuid),
        );
        assert!(schematic.contains(&format!("(property \"Reference\" \"{reference}\"")));
        assert!(schematic.contains(&format!("(property \"Value\" \"{value}\"")));

        let footprint = balanced_block_containing(
            &artifacts.kicad_pcb,
            "  (footprint ",
            &format!("(property \"Reference\" \"{reference}\""),
        );
        assert!(footprint.contains(&format!("(property \"Value\" \"{value}\"")));
    }

    fn pad_stanza<'a>(footprint: &'a str, pad: &str) -> &'a str {
        let marker = format!("    (pad \"{pad}\"");
        let start = footprint.find(&marker).expect("pad stanza must exist");
        let end = footprint[start..]
            .find("\n    )")
            .map(|offset| start + offset + "\n    )".len())
            .expect("pad stanza must terminate");
        &footprint[start..end]
    }

    fn segment_uuid(board: &str) -> &str {
        board
            .split("  (segment\n")
            .nth(1)
            .and_then(|segment| emitted_kicad_uuids(segment).into_iter().next())
            .expect("board must contain a routed segment UUID")
    }

    fn global_label_at(schematic: &str, net: &str, coordinates: &str) -> bool {
        schematic.contains(&format!(
            "  (global_label \"{net}\"\n    (shape bidirectional)\n    (at {coordinates} 0)"
        ))
    }

    fn balanced_block<'a>(text: &'a str, needle: &str) -> &'a str {
        let start = text.find(needle).expect("requested block must exist");
        let mut depth = 0_i32;
        let mut in_string = false;
        let mut escaped = false;
        for (offset, character) in text[start..].char_indices() {
            if in_string {
                if escaped {
                    escaped = false;
                } else if character == '\\' {
                    escaped = true;
                } else if character == '"' {
                    in_string = false;
                }
                continue;
            }
            match character {
                '"' => in_string = true,
                '(' => depth += 1,
                ')' => {
                    depth -= 1;
                    if depth == 0 {
                        return &text[start..start + offset + character.len_utf8()];
                    }
                }
                _ => {}
            }
        }
        panic!("requested block must be balanced")
    }

    fn balanced_block_containing<'a>(text: &'a str, block_marker: &str, needle: &str) -> &'a str {
        let needle_start = text.find(needle).expect("contained marker must exist");
        let block_start = text[..needle_start]
            .rfind(block_marker)
            .expect("enclosing block must exist");
        let block = balanced_block(&text[block_start..], block_marker);
        assert!(block.contains(needle), "marker escaped its enclosing block");
        block
    }
}
