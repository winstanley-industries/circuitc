use std::collections::{BTreeMap, BTreeSet};

use crate::compile::RelativeArtifactPath;
use crate::design::{
    Design, Diagnostic, SimulationAnalysis, SimulationAnalysisKind, SimulationAssertion,
    SimulationSample, ac_grid_index, transient_grid_location,
};
use crate::quantity::Quantity;
use crate::spice::{SpiceNameMap, lower_analysis_netlist};

use super::{
    AnalysisKind, AxisKind, BackendIdentity, CONTRACT_SCHEMA_VERSION, MAX_CONTRACT_BYTES,
    OHMNIVORE_BACKEND_CONTRACT, OHMNIVORE_BACKEND_NAME, OHMNIVORE_BACKEND_VERSION,
    OHMNIVORE_SOURCE_REVISION, REQUEST_SCHEMA_NAME, ReportSample, RequestAnalysis,
    RequestAssertion, ResultUnit, SPICE_MAP_SCHEMA_NAME, SignalKind, SimulationRequest,
    SpiceDeviceIdentity, SpiceIdentityMap, SpiceNetIdentity, canonical_f64, parse_request,
    parse_spice_identity_map, sha256_hex,
};

const LOWER_QUANTITY: &str = "CC-SIM-LOWER-001";
const LOWER_AXIS: &str = "CC-SIM-LOWER-002";
const LOWER_SAMPLE: &str = "CC-SIM-LOWER-003";
const LOWER_CONTRACT: &str = "CC-SIM-LOWER-004";
const LOWER_RESOURCE: &str = "CC-SIM-LOWER-005";
const MAX_SIMULATION_GENERATED_BYTES: usize = MAX_CONTRACT_BYTES;

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct SimulationInputBundle {
    pub analysis_path: String,
    pub analysis_kind: AnalysisKind,
    pub netlist_path: RelativeArtifactPath,
    pub request_path: RelativeArtifactPath,
    pub map_path: RelativeArtifactPath,
    pub netlist: String,
    pub request_json: String,
    pub spice_identity_map_json: String,
}

impl SimulationInputBundle {
    pub(crate) fn verify(&self) -> Result<(), Diagnostic> {
        let request = parse_request(&self.request_json)
            .map_err(|error| contract_diagnostic(&self.analysis_path, "request", error))?;
        request
            .verify_netlist_bytes(self.netlist.as_bytes())
            .map_err(|error| contract_diagnostic(&self.analysis_path, "netlist", error))?;
        let expected_stem = artifact_path_stem(&request.design, &request.analysis.path);
        let expected_netlist = format!("simulation/{expected_stem}/analysis.spice");
        let expected_request = format!("simulation/{expected_stem}/request.json");
        let expected_map = format!("simulation/{expected_stem}/spice-map.json");
        if request.analysis.path != self.analysis_path
            || request.analysis.kind != self.analysis_kind
            || self.netlist_path.as_str() != expected_netlist
            || self.request_path.as_str() != expected_request
            || self.map_path.as_str() != expected_map
            || request.analysis.netlist_path != self.netlist_path.as_str()
            || request.analysis.map_path != self.map_path.as_str()
        {
            return Err(lower_diagnostic(
                LOWER_CONTRACT,
                format!("design.analyses.{}", self.analysis_path),
                None,
                "simulation bundle paths or analysis identity do not match its request",
            ));
        }

        let map = parse_spice_identity_map(&self.spice_identity_map_json)
            .map_err(|error| contract_diagnostic(&self.analysis_path, "map", error))?;
        map.verify_request_bytes(self.request_json.as_bytes())
            .map_err(|error| contract_diagnostic(&self.analysis_path, "map binding", error))?;
        if map.analysis_path != self.analysis_path || map.design != request.design {
            return Err(lower_diagnostic(
                LOWER_CONTRACT,
                format!("design.analyses.{}", self.analysis_path),
                None,
                "simulation identity map does not match its bundle identity",
            ));
        }

        let expected_comments = identity_comment_lines(&map);
        let actual_comments: Vec<_> = self
            .netlist
            .lines()
            .filter(|line| {
                line.starts_with("* @circuitc-net ") || line.starts_with("* @circuitc-device ")
            })
            .map(str::to_owned)
            .collect();
        if actual_comments != expected_comments {
            return Err(lower_diagnostic(
                LOWER_CONTRACT,
                format!("design.analyses.{}", self.analysis_path),
                None,
                "SPICE identity comments do not exactly match the standalone identity map",
            ));
        }
        verify_netlist_identity_coverage(&self.netlist, &map, self.analysis_kind).map_err(
            |message| {
                lower_diagnostic(
                    LOWER_CONTRACT,
                    format!("design.analyses.{}", self.analysis_path),
                    None,
                    message,
                )
            },
        )?;
        Ok(())
    }
}

#[derive(Clone, Debug)]
enum BackendAxisPlan {
    Scalar,
    Ac(Vec<f64>),
    Transient { start: f64, stop: f64 },
}

pub(crate) fn lower_inputs(design: &Design) -> Result<Vec<SimulationInputBundle>, Vec<Diagnostic>> {
    lower_inputs_with_limit(design, MAX_SIMULATION_GENERATED_BYTES)
}

pub(crate) fn lower_inputs_with_limit(
    design: &Design,
    generated_byte_limit: usize,
) -> Result<Vec<SimulationInputBundle>, Vec<Diagnostic>> {
    design.validate()?;

    let mut analyses: Vec<_> = design.analyses.iter().collect();
    analyses.sort_by(|left, right| left.path.cmp(&right.path));
    let upper_bound = analyses.iter().try_fold(0_usize, |total, analysis| {
        total.checked_add(bundle_size_upper_bound(design, analysis)?)
    });
    if upper_bound.is_none_or(|upper_bound| upper_bound > generated_byte_limit) {
        return Err(vec![resource_diagnostic(generated_byte_limit)]);
    }

    let mut bundles = Vec::with_capacity(analyses.len());
    let mut diagnostics = Vec::new();
    let mut paths = BTreeSet::new();
    let mut generated_bytes = 0_usize;
    for analysis in analyses {
        match lower_one(design, analysis) {
            Ok(bundle) => {
                let Some(next_generated_bytes) = generated_bytes.checked_add(bundle.byte_len())
                else {
                    return Err(vec![resource_diagnostic(generated_byte_limit)]);
                };
                if next_generated_bytes > generated_byte_limit {
                    return Err(vec![resource_diagnostic(generated_byte_limit)]);
                }
                generated_bytes = next_generated_bytes;
                for path in [
                    bundle.netlist_path.as_str(),
                    bundle.request_path.as_str(),
                    bundle.map_path.as_str(),
                ] {
                    if !paths.insert(path.to_owned()) {
                        diagnostics.push(lower_diagnostic(
                            LOWER_CONTRACT,
                            format!("design.analyses.{}", analysis.path),
                            None,
                            format!("simulation artifact path collision at {path}"),
                        ));
                    }
                }
                bundles.push(bundle);
            }
            Err(diagnostic) => diagnostics.push(diagnostic),
        }
    }
    if diagnostics.is_empty() {
        Ok(bundles)
    } else {
        Err(diagnostics)
    }
}

impl SimulationInputBundle {
    fn byte_len(&self) -> usize {
        [
            self.analysis_path.len(),
            self.netlist_path.as_str().len(),
            self.request_path.as_str().len(),
            self.map_path.as_str().len(),
            self.netlist.len(),
            self.request_json.len(),
            self.spice_identity_map_json.len(),
        ]
        .into_iter()
        .sum()
    }
}

fn bundle_size_upper_bound(design: &Design, analysis: &SimulationAnalysis) -> Option<usize> {
    let mut bytes = 8_192_usize;
    add_scaled(&mut bytes, design.name.len(), 16)?;
    add_scaled(&mut bytes, analysis.path.len(), 16)?;
    for net in &design.nets {
        bytes = bytes.checked_add(512)?;
        add_scaled(&mut bytes, net.name.len(), 32)?;
    }
    for component in design
        .components
        .iter()
        .filter(|component| component.simulation.is_some())
    {
        bytes = bytes.checked_add(1_024)?;
        add_scaled(&mut bytes, component.path.len(), 32)?;
        add_scaled(&mut bytes, component.reference.len(), 32)?;
        for connection in &component.connections {
            if let crate::design::ConnectionState::Connected(net) = &connection.state {
                add_scaled(&mut bytes, net.len(), 16)?;
            }
        }
    }
    for assertion in design
        .assertions
        .iter()
        .filter(|assertion| assertion.analysis_path == analysis.path)
    {
        bytes = bytes.checked_add(2_048)?;
        add_scaled(&mut bytes, assertion.path.len(), 16)?;
        add_scaled(&mut bytes, assertion.analysis_path.len(), 16)?;
        add_scaled(&mut bytes, assertion.net.len(), 16)?;
    }
    Some(bytes)
}

fn add_scaled(bytes: &mut usize, value: usize, multiplier: usize) -> Option<()> {
    *bytes = bytes.checked_add(value.checked_mul(multiplier)?)?;
    Some(())
}

fn resource_diagnostic(limit: usize) -> Diagnostic {
    lower_diagnostic(
        LOWER_RESOURCE,
        "design.analyses".to_owned(),
        None,
        format!(
            "deterministic simulation inputs exceed the {limit}-byte aggregate generated-artifact budget"
        ),
    )
}

fn lower_one(
    design: &Design,
    analysis: &SimulationAnalysis,
) -> Result<SimulationInputBundle, Diagnostic> {
    let analysis_kind = contract_analysis_kind(&analysis.kind);
    let path_stem = artifact_path_stem(&design.name, &analysis.path);
    let netlist_path = relative_path(analysis, format!("simulation/{path_stem}/analysis.spice"))?;
    let request_path = relative_path(analysis, format!("simulation/{path_stem}/request.json"))?;
    let map_path = relative_path(analysis, format!("simulation/{path_stem}/spice-map.json"))?;

    let mut assertions: Vec<_> = design
        .assertions
        .iter()
        .filter(|assertion| assertion.analysis_path == analysis.path)
        .collect();
    assertions.sort_by(|left, right| left.path.cmp(&right.path));
    let axis = backend_axis_plan(analysis)?;
    validate_transient_sample_injectivity(analysis, &assertions)?;
    let lowered = lower_analysis_netlist(design, analysis);
    let netlist_sha256 = sha256_hex(lowered.netlist.as_bytes());
    let assertions = assertions
        .into_iter()
        .map(|assertion| lower_assertion(assertion, analysis, &axis))
        .collect::<Result<Vec<_>, _>>()?;

    let request = SimulationRequest {
        schema_name: REQUEST_SCHEMA_NAME.to_owned(),
        schema_version: CONTRACT_SCHEMA_VERSION,
        design: design.name.clone(),
        backend: BackendIdentity {
            name: OHMNIVORE_BACKEND_NAME.to_owned(),
            version: OHMNIVORE_BACKEND_VERSION.to_owned(),
            contract: OHMNIVORE_BACKEND_CONTRACT.to_owned(),
            source_revision: OHMNIVORE_SOURCE_REVISION.to_owned(),
        },
        analysis: RequestAnalysis {
            path: analysis.path.clone(),
            kind: analysis_kind,
            netlist_path: netlist_path.as_str().to_owned(),
            netlist_sha256,
            map_path: map_path.as_str().to_owned(),
        },
        assertions,
    };
    let request_json = request
        .to_canonical_json()
        .map_err(|error| contract_diagnostic(&analysis.path, "request", error))?;
    let identity_map = lower_identity_map(
        design,
        analysis,
        &lowered.name_map,
        sha256_hex(request_json.as_bytes()),
    );
    let spice_identity_map_json = identity_map
        .to_canonical_json()
        .map_err(|error| contract_diagnostic(&analysis.path, "map", error))?;
    let bundle = SimulationInputBundle {
        analysis_path: analysis.path.clone(),
        analysis_kind,
        netlist_path,
        request_path,
        map_path,
        netlist: lowered.netlist,
        request_json,
        spice_identity_map_json,
    };
    bundle.verify()?;
    Ok(bundle)
}

fn backend_axis_plan(analysis: &SimulationAnalysis) -> Result<BackendAxisPlan, Diagnostic> {
    let base = format!("design.analyses.{}", analysis.path);
    match &analysis.kind {
        SimulationAnalysisKind::DcOperatingPoint => Ok(BackendAxisPlan::Scalar),
        SimulationAnalysisKind::AcLinearSweep {
            points,
            start_frequency,
            stop_frequency,
            ..
        } => {
            let start = quantity_to_f64(*start_frequency, format!("{base}.start_frequency"))?;
            let stop = quantity_to_f64(*stop_frequency, format!("{base}.stop_frequency"))?;
            if start >= stop {
                return Err(lower_diagnostic(
                    LOWER_AXIS,
                    format!("{base}.stop_frequency"),
                    Some(format!("{base}.start_frequency")),
                    "distinct exact AC sweep endpoints collapse or reverse at the backend f64 boundary",
                ));
            }
            let step = (stop - start) / f64::from(points - 1);
            let mut values = Vec::with_capacity(*points as usize);
            for index in 0..*points {
                let value = start + step * f64::from(index);
                if !value.is_finite()
                    || values
                        .last()
                        .is_some_and(|previous: &f64| value <= *previous)
                {
                    return Err(lower_diagnostic(
                        LOWER_AXIS,
                        format!("{base}.points"),
                        Some(format!("{base}.start_frequency")),
                        "the pinned backend AC schedule is non-finite, duplicate, or non-increasing",
                    ));
                }
                values.push(value);
            }
            Ok(BackendAxisPlan::Ac(values))
        }
        SimulationAnalysisKind::Transient {
            step, stop, start, ..
        } => {
            let step_value = quantity_to_f64(*step, format!("{base}.step"))?;
            let stop_value = quantity_to_f64(*stop, format!("{base}.stop"))?;
            let start_value = quantity_to_f64(*start, format!("{base}.start"))?;
            for (left_name, left_exact, left_value, right_name, right_exact, right_value) in [
                ("step", *step, step_value, "start", *start, start_value),
                ("step", *step, step_value, "stop", *stop, stop_value),
                ("start", *start, start_value, "stop", *stop, stop_value),
            ] {
                if left_exact.exact_cmp(right_exact) != Some(std::cmp::Ordering::Equal)
                    && left_value.to_bits() == right_value.to_bits()
                {
                    return Err(lower_diagnostic(
                        LOWER_AXIS,
                        format!("{base}.{right_name}"),
                        Some(format!("{base}.{left_name}")),
                        "distinct exact transient controls collapse to one value at the backend f64 boundary",
                    ));
                }
            }
            Ok(BackendAxisPlan::Transient {
                start: start_value,
                stop: stop_value,
            })
        }
    }
}

fn lower_assertion(
    assertion: &SimulationAssertion,
    analysis: &SimulationAnalysis,
    axis: &BackendAxisPlan,
) -> Result<RequestAssertion, Diagnostic> {
    let base = format!("design.assertions.{}", assertion.path);
    let (signal_kind, sample) = match (&analysis.kind, &assertion.sample, axis) {
        (
            SimulationAnalysisKind::DcOperatingPoint,
            SimulationSample::Scalar,
            BackendAxisPlan::Scalar,
        ) => (
            SignalKind::NetVoltage,
            ReportSample {
                kind: AxisKind::Scalar,
                value: canonical_f64(0.0)
                    .map_err(|error| contract_diagnostic(&analysis.path, "scalar sample", error))?,
            },
        ),
        (
            SimulationAnalysisKind::AcLinearSweep {
                points,
                start_frequency,
                stop_frequency,
                ..
            },
            SimulationSample::Frequency(exact_sample),
            BackendAxisPlan::Ac(values),
        ) => {
            let index = ac_grid_index(*exact_sample, *start_frequency, *stop_frequency, *points)
                .and_then(|index| usize::try_from(index).ok())
                .and_then(|index| values.get(index).copied())
                .ok_or_else(|| missing_sample(assertion, analysis))?;
            (
                SignalKind::NetVoltageMagnitude,
                ReportSample {
                    kind: AxisKind::FrequencyHertz,
                    value: canonical_f64(index).map_err(|error| {
                        contract_diagnostic(&analysis.path, "AC assertion sample", error)
                    })?,
                },
            )
        }
        (
            SimulationAnalysisKind::Transient { step, stop, .. },
            SimulationSample::Time(exact_sample),
            BackendAxisPlan::Transient {
                start,
                stop: backend_stop,
            },
        ) => {
            if transient_grid_location(*exact_sample, *step, *stop).is_none() {
                return Err(missing_sample(assertion, analysis));
            }
            let value = quantity_to_f64(*exact_sample, format!("{base}.sample"))?;
            if value < *start || value > *backend_stop {
                return Err(missing_sample(assertion, analysis));
            }
            (
                SignalKind::NetVoltage,
                ReportSample {
                    kind: AxisKind::TimeSeconds,
                    value: canonical_f64(value).map_err(|error| {
                        contract_diagnostic(&analysis.path, "transient assertion sample", error)
                    })?,
                },
            )
        }
        _ => return Err(missing_sample(assertion, analysis)),
    };

    Ok(RequestAssertion {
        path: assertion.path.clone(),
        signal_kind,
        canonical_identity: assertion.net.clone(),
        sample,
        unit: ResultUnit::Volt,
        expected: quantity_to_contract_number(assertion.expected, format!("{base}.expected"))?,
        absolute_tolerance: quantity_to_contract_number(
            assertion.absolute_tolerance,
            format!("{base}.absolute_tolerance"),
        )?,
        relative_tolerance: quantity_to_contract_number(
            assertion.relative_tolerance,
            format!("{base}.relative_tolerance"),
        )?,
    })
}

fn validate_transient_sample_injectivity(
    analysis: &SimulationAnalysis,
    assertions: &[&SimulationAssertion],
) -> Result<(), Diagnostic> {
    let SimulationAnalysisKind::Transient {
        step, stop, start, ..
    } = &analysis.kind
    else {
        return Ok(());
    };
    let analysis_base = format!("design.analyses.{}", analysis.path);
    let mut lowered = BTreeMap::<u64, (String, Quantity)>::new();
    for (field, quantity) in [("step", *step), ("start", *start), ("stop", *stop)] {
        let path = format!("{analysis_base}.{field}");
        let value = quantity_to_f64(quantity, path.clone())?;
        lowered.entry(value.to_bits()).or_insert((path, quantity));
    }
    for assertion in assertions {
        let SimulationSample::Time(sample) = assertion.sample else {
            continue;
        };
        let path = format!("design.assertions.{}.sample", assertion.path);
        let value = quantity_to_f64(sample, path.clone())?;
        match lowered.entry(value.to_bits()) {
            std::collections::btree_map::Entry::Occupied(entry) => {
                let (other_path, other) = entry.get();
                if sample.exact_cmp(*other) != Some(std::cmp::Ordering::Equal) {
                    return Err(lower_diagnostic(
                        LOWER_SAMPLE,
                        path,
                        Some(other_path.clone()),
                        "distinct exact transient assertion or control times collapse to one value at the backend f64 boundary",
                    ));
                }
            }
            std::collections::btree_map::Entry::Vacant(entry) => {
                entry.insert((path, sample));
            }
        }
    }
    Ok(())
}

fn lower_identity_map(
    design: &Design,
    analysis: &SimulationAnalysis,
    names: &SpiceNameMap,
    request_sha256: String,
) -> SpiceIdentityMap {
    let ground: BTreeMap<_, _> = design
        .nets
        .iter()
        .map(|net| (net.name.as_str(), net.is_ground))
        .collect();
    SpiceIdentityMap {
        schema_name: SPICE_MAP_SCHEMA_NAME.to_owned(),
        schema_version: CONTRACT_SCHEMA_VERSION,
        design: design.name.clone(),
        analysis_path: analysis.path.clone(),
        request_sha256,
        nets: names
            .nets
            .iter()
            .map(|mapping| SpiceNetIdentity {
                canonical: mapping.net.clone(),
                backend: mapping.node.clone(),
                is_ground: ground[&mapping.net.as_str()],
            })
            .collect(),
        devices: names
            .components
            .iter()
            .map(|mapping| SpiceDeviceIdentity {
                semantic_path: mapping.path.clone(),
                reference: mapping.reference.clone(),
                backend: mapping.device.clone(),
            })
            .collect(),
    }
}

fn contract_analysis_kind(kind: &SimulationAnalysisKind) -> AnalysisKind {
    match kind {
        SimulationAnalysisKind::DcOperatingPoint => AnalysisKind::DcOperatingPoint,
        SimulationAnalysisKind::AcLinearSweep { .. } => AnalysisKind::AcLinearSweep,
        SimulationAnalysisKind::Transient { .. } => AnalysisKind::Transient,
    }
}

fn quantity_to_contract_number(quantity: Quantity, path: String) -> Result<String, Diagnostic> {
    let value = quantity_to_f64(quantity, path.clone())?;
    canonical_f64(value).map_err(|error| contract_diagnostic(&path, "number", error))
}

fn quantity_to_f64(quantity: Quantity, path: String) -> Result<f64, Diagnostic> {
    let literal = quantity.spice_literal();
    let value = literal.parse::<f64>().map_err(|_| {
        lower_diagnostic(
            LOWER_QUANTITY,
            path.clone(),
            None,
            format!("exact quantity `{literal}` is not representable by the pinned backend parser"),
        )
    })?;
    if value.is_finite() {
        Ok(value)
    } else {
        Err(lower_diagnostic(
            LOWER_QUANTITY,
            path,
            None,
            format!("exact quantity `{literal}` becomes non-finite at the pinned backend boundary"),
        ))
    }
}

fn missing_sample(assertion: &SimulationAssertion, analysis: &SimulationAnalysis) -> Diagnostic {
    lower_diagnostic(
        LOWER_SAMPLE,
        format!("design.assertions.{}.sample", assertion.path),
        Some(format!("design.analyses.{}", analysis.path)),
        "the exact assertion sample is not emitted by the pinned backend schedule",
    )
}

fn relative_path(
    analysis: &SimulationAnalysis,
    path: String,
) -> Result<RelativeArtifactPath, Diagnostic> {
    RelativeArtifactPath::try_new(path).map_err(|error| {
        lower_diagnostic(
            LOWER_CONTRACT,
            format!("design.analyses.{}", analysis.path),
            None,
            error.to_string(),
        )
    })
}

fn artifact_path_stem(design: &str, analysis: &str) -> String {
    let source = format!("circuitc-simulation-path-v1\0{design}\0{analysis}");
    sha256_hex(source.as_bytes())
}

fn identity_comment_lines(map: &SpiceIdentityMap) -> Vec<String> {
    map.nets
        .iter()
        .map(|net| {
            format!(
                "* @circuitc-net {} {}",
                hex_encode(net.canonical.as_bytes()),
                net.backend
            )
        })
        .chain(map.devices.iter().map(|device| {
            format!(
                "* @circuitc-device {} {} {}",
                hex_encode(device.semantic_path.as_bytes()),
                hex_encode(device.reference.as_bytes()),
                device.backend
            )
        }))
        .collect()
}

fn verify_netlist_identity_coverage(
    netlist: &str,
    map: &SpiceIdentityMap,
    analysis_kind: AnalysisKind,
) -> Result<(), &'static str> {
    let mapped_nets: BTreeSet<_> = map.nets.iter().map(|net| net.backend.as_str()).collect();
    let mapped_devices: BTreeSet<_> = map
        .devices
        .iter()
        .map(|device| device.backend.as_str())
        .collect();
    let mut used_nets = BTreeSet::new();
    let mut used_devices = BTreeSet::new();
    let mut directives = Vec::new();

    for line in netlist.lines() {
        if line.is_empty() || line.starts_with('*') {
            continue;
        }
        if line.starts_with('.') {
            if !line.eq_ignore_ascii_case(".END") {
                directives.push(line);
            }
            continue;
        }
        let fields: Vec<_> = line.split_ascii_whitespace().collect();
        if fields.len() < 3 {
            return Err("SPICE device lines must contain one mapped device and two mapped nodes");
        }
        let device = fields[0];
        if !mapped_devices.contains(&device) || !used_devices.insert(device) {
            return Err(
                "SPICE device tokens must map exactly once through the standalone identity map",
            );
        }
        for node in &fields[1..=2] {
            let node = *node;
            if !mapped_nets.contains(&node) {
                return Err(
                    "every emitted SPICE node token must resolve through the standalone identity map",
                );
            }
            used_nets.insert(node);
        }
    }

    if used_devices != mapped_devices {
        return Err("the standalone identity map must contain exactly every emitted SPICE device");
    }
    if map
        .nets
        .iter()
        .any(|net| !net.is_ground && !used_nets.contains(net.backend.as_str()))
    {
        return Err(
            "the standalone identity map may not contain a non-ground net absent from emitted SPICE device lines",
        );
    }
    let matching_directive = directives.first().is_some_and(|directive| {
        let keyword = directive
            .split_ascii_whitespace()
            .next()
            .unwrap_or_default();
        match analysis_kind {
            AnalysisKind::DcOperatingPoint => directive.eq_ignore_ascii_case(".OP"),
            AnalysisKind::AcLinearSweep => keyword.eq_ignore_ascii_case(".AC"),
            AnalysisKind::Transient => keyword.eq_ignore_ascii_case(".TRAN"),
        }
    });
    if directives.len() != 1 || !matching_directive {
        return Err("a simulation netlist must contain exactly one matching analysis directive");
    }
    Ok(())
}

fn hex_encode(bytes: &[u8]) -> String {
    use std::fmt::Write as _;

    let mut encoded = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        write!(encoded, "{byte:02X}").unwrap();
    }
    encoded
}

fn contract_diagnostic(
    analysis_path: &str,
    context: &str,
    error: super::ContractDiagnostic,
) -> Diagnostic {
    lower_diagnostic(
        LOWER_CONTRACT,
        format!("design.analyses.{analysis_path}"),
        None,
        format!("invalid generated simulation {context}: {error}"),
    )
}

fn lower_diagnostic(
    code: &'static str,
    path: String,
    related_path: Option<String>,
    message: impl Into<String>,
) -> Diagnostic {
    Diagnostic {
        code,
        path,
        related_path,
        message: message.into(),
    }
}

#[cfg(test)]
mod tests {
    use crate::demo::voltage_divider;
    use crate::design::{
        SimulationAnalysis, SimulationAnalysisKind, SimulationAssertion, SimulationSample,
    };
    use crate::quantity::{Quantity, Unit};
    use crate::simulation::{
        AxisKind, SignalKind, canonical_f64, parse_request, parse_spice_identity_map, sha256_hex,
    };

    use super::{
        LOWER_AXIS, LOWER_RESOURCE, LOWER_SAMPLE, bundle_size_upper_bound, lower_inputs,
        lower_inputs_with_limit, quantity_to_f64,
    };

    #[test]
    fn lowers_sorted_bound_request_map_bundles_and_backend_scheduled_samples() {
        let design = simulation_design();
        let bundles = lower_inputs(&design).expect("supported analyses must lower");
        assert_eq!(
            bundles
                .iter()
                .map(|bundle| bundle.analysis_path.as_str())
                .collect::<Vec<_>>(),
            vec![
                "divider.simulation.ac",
                "divider.simulation.dc",
                "divider.simulation.tran"
            ]
        );
        for bundle in &bundles {
            bundle
                .verify()
                .expect("generated bundle must be self-consistent");
            assert!(bundle.netlist_path.as_str().starts_with("simulation/"));
            assert!(bundle.request_path.as_str().ends_with("/request.json"));
            assert!(bundle.map_path.as_str().ends_with("/spice-map.json"));
        }

        let ac = parse_request(&bundles[0].request_json).unwrap();
        assert_eq!(ac.assertions.len(), 1);
        assert_eq!(
            ac.assertions[0].signal_kind,
            SignalKind::NetVoltageMagnitude
        );
        assert_eq!(ac.assertions[0].sample.kind, AxisKind::FrequencyHertz);
        let backend_ac = 0.1_f64 + ((0.4_f64 - 0.1_f64) / 3.0) * 2.0;
        assert_eq!(
            ac.assertions[0].sample.value,
            canonical_f64(backend_ac).unwrap()
        );
        assert_ne!(ac.assertions[0].sample.value, canonical_f64(0.3).unwrap());

        let transient = parse_request(&bundles[2].request_json).unwrap();
        assert_eq!(
            transient.assertions[0].sample.value,
            canonical_f64(0.3).unwrap()
        );

        let mut permuted = design;
        permuted.analyses.reverse();
        permuted.assertions.reverse();
        assert_eq!(lower_inputs(&permuted).unwrap(), bundles);
    }

    #[test]
    fn dc_bundle_has_exact_canonical_paths_and_bytes() {
        let bundle = lower_inputs(&simulation_design()).unwrap().remove(1);
        let directory =
            "simulation/5a1ce7fefedf5496a28f8a63cdba0f83185fa3e19e771da222f3807fed34a97f";
        assert_eq!(
            bundle.netlist_path.as_str(),
            format!("{directory}/analysis.spice")
        );
        assert_eq!(
            bundle.request_path.as_str(),
            format!("{directory}/request.json")
        );
        assert_eq!(
            bundle.map_path.as_str(),
            format!("{directory}/spice-map.json")
        );
        assert_eq!(
            bundle.netlist,
            r#"* Generated by CircuitC from voltage_divider
* @circuitc-net 474E44 0
* @circuitc-net 56494E VIN
* @circuitc-net 564F5554 VOUT
* @circuitc-device 646976696465722E616E616C797369732E696E707574 5631 V1
* @circuitc-device 646976696465722E725F626F74746F6D 5232 R2
* @circuitc-device 646976696465722E725F746F70 5231 R1
V1 VIN 0 DC 10
R2 VOUT 0 10e3
R1 VIN VOUT 10e3
.OP
.END
"#
        );
        assert_eq!(
            bundle.request_json,
            r#"{
  "schema_name": "circuitc.simulation_request",
  "schema_version": 1,
  "design": "voltage_divider",
  "backend": {
    "name": "ohmnivore",
    "version": "0.1.0",
    "contract": "ohmnivore-cli-csv/v1",
    "source_revision": "c2189a651d4879211019e109b2136dee836a5c5d"
  },
  "analysis": {
    "path": "divider.simulation.dc",
    "kind": "dc_operating_point",
    "netlist_path": "simulation/5a1ce7fefedf5496a28f8a63cdba0f83185fa3e19e771da222f3807fed34a97f/analysis.spice",
    "netlist_sha256": "43a5f70c8f1e4bbdf428027a1b88e450f02ea6eacf9015f2cd953d65b174c0a8",
    "map_path": "simulation/5a1ce7fefedf5496a28f8a63cdba0f83185fa3e19e771da222f3807fed34a97f/spice-map.json"
  },
  "assertions": [
    {
      "path": "divider.assertions.dc",
      "signal_kind": "net_voltage",
      "canonical_identity": "VOUT",
      "sample": {
        "kind": "scalar",
        "value": "0.00000000000000000e0"
      },
      "unit": "volt",
      "expected": "5.00000000000000000e0",
      "absolute_tolerance": "9.99999999999999955e-7",
      "relative_tolerance": "1.00000000000000002e-3"
    }
  ]
}
"#
        );
        assert_eq!(
            bundle.spice_identity_map_json,
            r#"{
  "schema_name": "circuitc.spice_identity_map",
  "schema_version": 1,
  "design": "voltage_divider",
  "analysis_path": "divider.simulation.dc",
  "request_sha256": "e701d0ad46a2c434127f6aff9fb8d60cdf4b3b0cc94b7bd245037bff6bcab62c",
  "nets": [
    {
      "canonical": "GND",
      "backend": "0",
      "is_ground": true
    },
    {
      "canonical": "VIN",
      "backend": "VIN",
      "is_ground": false
    },
    {
      "canonical": "VOUT",
      "backend": "VOUT",
      "is_ground": false
    }
  ],
  "devices": [
    {
      "semantic_path": "divider.analysis.input",
      "reference": "V1",
      "backend": "V1"
    },
    {
      "semantic_path": "divider.r_bottom",
      "reference": "R2",
      "backend": "R2"
    },
    {
      "semantic_path": "divider.r_top",
      "reference": "R1",
      "backend": "R1"
    }
  ]
}
"#
        );
    }

    #[test]
    fn bundle_verifier_rejects_corrupted_predecessors_and_comment_parity() {
        let bundle = lower_inputs(&simulation_design()).unwrap().remove(0);

        let mut stale_netlist = bundle.clone();
        stale_netlist.netlist.push_str("* stale\n");
        assert_eq!(stale_netlist.verify().unwrap_err().code, "CC-SIM-LOWER-004");

        let mut stale_request = bundle.clone();
        stale_request.request_json = stale_request
            .request_json
            .replace("\"design\": \"voltage_divider\"", "\"design\": \"other\"");
        assert_eq!(stale_request.verify().unwrap_err().code, "CC-SIM-LOWER-004");

        let mut stale_map = bundle.clone();
        stale_map.spice_identity_map_json = stale_map.spice_identity_map_json.replacen(
            "\"backend\": \"VIN\"",
            "\"backend\": \"OTHER\"",
            1,
        );
        assert_eq!(stale_map.verify().unwrap_err().code, "CC-SIM-LOWER-004");

        let mut wrong_path = bundle;
        wrong_path.netlist_path =
            crate::RelativeArtifactPath::try_new("simulation/wrong.spice").unwrap();
        assert_eq!(wrong_path.verify().unwrap_err().code, "CC-SIM-LOWER-004");

        let mut wrong_request_path = lower_inputs(&simulation_design()).unwrap().remove(0);
        wrong_request_path.request_path =
            crate::RelativeArtifactPath::try_new("simulation/wrong-request.json").unwrap();
        assert_eq!(
            wrong_request_path.verify().unwrap_err().code,
            "CC-SIM-LOWER-004"
        );

        let mut unmapped_token = lower_inputs(&simulation_design()).unwrap().remove(1);
        unmapped_token.netlist =
            unmapped_token
                .netlist
                .replacen("V1 VIN 0 DC 10", "V1 UNMAPPED 0 DC 10", 1);
        rebind_bundle(&mut unmapped_token);
        let diagnostic = unmapped_token.verify().unwrap_err();
        assert_eq!(diagnostic.code, "CC-SIM-LOWER-004");
        assert_eq!(
            diagnostic.message,
            "every emitted SPICE node token must resolve through the standalone identity map"
        );

        for (from, to) in [
            ("V1 VIN 0 DC 10", "V1 vin 0 DC 10"),
            ("V1 VIN 0 DC 10", "v1 VIN 0 DC 10"),
        ] {
            let mut case_drift = lower_inputs(&simulation_design()).unwrap().remove(1);
            case_drift.netlist = case_drift.netlist.replacen(from, to, 1);
            rebind_bundle(&mut case_drift);
            assert_eq!(case_drift.verify().unwrap_err().code, "CC-SIM-LOWER-004");
        }

        for replacement in [".OPTION TEMP=27", ".OPERATOR", ".OP extra"] {
            let mut directive_drift = lower_inputs(&simulation_design()).unwrap().remove(1);
            directive_drift.netlist = directive_drift.netlist.replacen(".OP", replacement, 1);
            rebind_bundle(&mut directive_drift);
            let diagnostic = directive_drift.verify().unwrap_err();
            assert_eq!(diagnostic.code, "CC-SIM-LOWER-004");
            assert_eq!(
                diagnostic.message,
                "a simulation netlist must contain exactly one matching analysis directive"
            );
        }

        for (bundle_index, from, to) in [(0, ".AC", ".ACCURACY"), (2, ".TRAN", ".TRANSIENT")] {
            let mut directive_drift = lower_inputs(&simulation_design())
                .unwrap()
                .remove(bundle_index);
            directive_drift.netlist = directive_drift.netlist.replacen(from, to, 1);
            rebind_bundle(&mut directive_drift);
            assert_eq!(
                directive_drift.verify().unwrap_err().code,
                "CC-SIM-LOWER-004"
            );
        }
    }

    #[test]
    fn rejects_collapsed_ac_axes_and_transient_controls() {
        for (start, stop, points) in [
            (9_007_199_254_740_992, 9_007_199_254_740_993, 2),
            (9_007_199_254_740_992, 9_007_199_254_740_994, 3),
        ] {
            let mut design = voltage_divider();
            design.analyses = vec![SimulationAnalysis {
                path: "divider.simulation.ac".to_owned(),
                kind: SimulationAnalysisKind::AcLinearSweep {
                    source: "divider.analysis.input".to_owned(),
                    points,
                    start_frequency: Quantity::new(start, 0, Unit::Hertz),
                    stop_frequency: Quantity::new(stop, 0, Unit::Hertz),
                    magnitude: Quantity::new(1, 0, Unit::Volt),
                    phase: Quantity::new(0, 0, Unit::Degree),
                },
            }];
            let error = lower_inputs(&design).unwrap_err();
            assert_eq!(error.len(), 1);
            assert_eq!(error[0].code, LOWER_AXIS);
        }

        let mut design = voltage_divider();
        design.analyses = vec![transient_analysis(
            Quantity::new(9_007_199_254_740_992, 0, Unit::Second),
            Quantity::new(9_007_199_254_740_993, 0, Unit::Second),
            Quantity::new(0, 0, Unit::Second),
        )];
        let error = lower_inputs(&design).unwrap_err();
        assert_eq!(error.len(), 1);
        assert_eq!(error[0].code, LOWER_AXIS);
        assert_eq!(
            error[0].path,
            "design.analyses.divider.simulation.tran.stop"
        );
    }

    #[test]
    fn rejects_distinct_transient_assertion_samples_that_collapse() {
        let mut design = voltage_divider();
        design.analyses = vec![transient_analysis(
            Quantity::new(4_503_599_627_370_496, 0, Unit::Second),
            Quantity::new(9_007_199_254_740_993, 0, Unit::Second),
            Quantity::new(0, 0, Unit::Second),
        )];
        design.assertions = vec![assertion(
            "divider.assertions.multiple",
            "divider.simulation.tran",
            SimulationSample::Time(Quantity::new(9_007_199_254_740_992, 0, Unit::Second)),
        )];
        let error = lower_inputs(&design).unwrap_err();
        assert_eq!(error.len(), 1);
        assert_eq!(error[0].code, LOWER_SAMPLE);
        assert_eq!(
            error[0].path,
            "design.assertions.divider.assertions.multiple.sample"
        );
        assert_eq!(
            error[0].related_path.as_deref(),
            Some("design.analyses.divider.simulation.tran.stop")
        );
    }

    #[test]
    fn transient_request_authenticates_authored_samples_without_predicting_adaptive_rows() {
        let mut design = voltage_divider();
        design.analyses = vec![transient_analysis(
            Quantity::new(1, -1, Unit::Second),
            Quantity::new(8, -1, Unit::Second),
            Quantity::new(0, 0, Unit::Second),
        )];
        design.assertions = vec![assertion(
            "divider.assertions.stop",
            "divider.simulation.tran",
            SimulationSample::Time(Quantity::new(8, -1, Unit::Second)),
        )];
        let bundle = lower_inputs(&design).unwrap().remove(0);
        let request = parse_request(&bundle.request_json).unwrap();
        assert_eq!(
            request.assertions[0].sample.value,
            canonical_f64(0.8).unwrap()
        );
    }

    #[test]
    fn aggregate_generated_artifact_budget_is_checked_before_retention() {
        let mut design = voltage_divider();
        design.analyses = (0..3)
            .map(|index| SimulationAnalysis {
                path: format!("divider.simulation.dc{index}"),
                kind: SimulationAnalysisKind::DcOperatingPoint,
            })
            .collect();
        let upper_bound = design
            .analyses
            .iter()
            .try_fold(0_usize, |total, analysis| {
                total.checked_add(bundle_size_upper_bound(&design, analysis)?)
            })
            .unwrap();
        let diagnostic = lower_inputs_with_limit(&design, upper_bound - 1).unwrap_err();
        assert_eq!(diagnostic.len(), 1);
        assert_eq!(diagnostic[0].code, LOWER_RESOURCE);
        assert_eq!(diagnostic[0].path, "design.analyses");
        assert_eq!(
            diagnostic[0].message,
            format!(
                "deterministic simulation inputs exceed the {}-byte aggregate generated-artifact budget",
                upper_bound - 1
            )
        );

        let bundles = lower_inputs_with_limit(&design, upper_bound).unwrap();
        assert_eq!(bundles.len(), 3);
        assert!(
            bundles
                .iter()
                .map(|bundle| bundle.byte_len())
                .sum::<usize>()
                < upper_bound
        );
    }

    #[test]
    fn ac_assertion_uses_the_pinned_backend_operation_order() {
        let mut design = voltage_divider();
        design.analyses = vec![SimulationAnalysis {
            path: "divider.simulation.ac".to_owned(),
            kind: SimulationAnalysisKind::AcLinearSweep {
                source: "divider.analysis.input".to_owned(),
                points: 6,
                start_frequency: Quantity::new(1, -1, Unit::Hertz),
                stop_frequency: Quantity::new(2, -1, Unit::Hertz),
                magnitude: Quantity::new(1, 0, Unit::Volt),
                phase: Quantity::new(0, 0, Unit::Degree),
            },
        }];
        design.assertions = vec![assertion(
            "divider.assertions.ac",
            "divider.simulation.ac",
            SimulationSample::Frequency(Quantity::new(12, -2, Unit::Hertz)),
        )];

        let bundle = lower_inputs(&design).unwrap().remove(0);
        let request = parse_request(&bundle.request_json).unwrap();
        let scheduled = 0.1_f64 + (0.2_f64 - 0.1_f64) / 5.0;
        assert_eq!(
            request.assertions[0].sample.value,
            canonical_f64(scheduled).unwrap()
        );
        assert_ne!(
            request.assertions[0].sample.value,
            canonical_f64(0.12).unwrap()
        );
    }

    #[test]
    fn backend_quantity_conversion_parses_the_exact_emitted_literal() {
        let quantity = Quantity::new(8_904_841_857_247_764_027, -18, Unit::Second);
        let converted = quantity_to_f64(quantity, "test.quantity".to_owned()).unwrap();
        assert_eq!(converted.to_bits(), 0x4021_cf47_6e91_dcb2);
        assert_ne!(
            converted.to_bits(),
            ((quantity.coefficient as f64) * 10_f64.powi(i32::from(quantity.exponent))).to_bits()
        );
    }

    fn simulation_design() -> crate::design::Design {
        let mut design = voltage_divider();
        design.analyses = vec![
            SimulationAnalysis {
                path: "divider.simulation.tran".to_owned(),
                kind: SimulationAnalysisKind::Transient {
                    step: Quantity::new(1, -1, Unit::Second),
                    stop: Quantity::new(4, -1, Unit::Second),
                    start: Quantity::new(0, 0, Unit::Second),
                    uic: false,
                },
            },
            SimulationAnalysis {
                path: "divider.simulation.dc".to_owned(),
                kind: SimulationAnalysisKind::DcOperatingPoint,
            },
            SimulationAnalysis {
                path: "divider.simulation.ac".to_owned(),
                kind: SimulationAnalysisKind::AcLinearSweep {
                    source: "divider.analysis.input".to_owned(),
                    points: 4,
                    start_frequency: Quantity::new(1, -1, Unit::Hertz),
                    stop_frequency: Quantity::new(4, -1, Unit::Hertz),
                    magnitude: Quantity::new(1, 0, Unit::Volt),
                    phase: Quantity::new(0, 0, Unit::Degree),
                },
            },
        ];
        design.assertions = vec![
            assertion(
                "divider.assertions.tran",
                "divider.simulation.tran",
                SimulationSample::Time(Quantity::new(3, -1, Unit::Second)),
            ),
            assertion(
                "divider.assertions.dc",
                "divider.simulation.dc",
                SimulationSample::Scalar,
            ),
            assertion(
                "divider.assertions.ac",
                "divider.simulation.ac",
                SimulationSample::Frequency(Quantity::new(3, -1, Unit::Hertz)),
            ),
        ];
        design
    }

    fn transient_analysis(step: Quantity, stop: Quantity, start: Quantity) -> SimulationAnalysis {
        SimulationAnalysis {
            path: "divider.simulation.tran".to_owned(),
            kind: SimulationAnalysisKind::Transient {
                step,
                stop,
                start,
                uic: false,
            },
        }
    }

    fn assertion(path: &str, analysis_path: &str, sample: SimulationSample) -> SimulationAssertion {
        SimulationAssertion {
            path: path.to_owned(),
            analysis_path: analysis_path.to_owned(),
            net: "VOUT".to_owned(),
            sample,
            expected: Quantity::new(5, 0, Unit::Volt),
            absolute_tolerance: Quantity::new(1, -6, Unit::Volt),
            relative_tolerance: Quantity::new(1, -3, Unit::Dimensionless),
        }
    }

    fn rebind_bundle(bundle: &mut super::SimulationInputBundle) {
        let mut request = parse_request(&bundle.request_json).unwrap();
        request.analysis.netlist_sha256 = sha256_hex(bundle.netlist.as_bytes());
        bundle.request_json = request.to_canonical_json().unwrap();
        let mut map = parse_spice_identity_map(&bundle.spice_identity_map_json).unwrap();
        map.request_sha256 = sha256_hex(bundle.request_json.as_bytes());
        bundle.spice_identity_map_json = map.to_canonical_json().unwrap();
    }
}
