use std::collections::{BTreeSet, HashMap};
use std::fmt;

use serde::de::DeserializeOwned;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

pub const REQUEST_SCHEMA_NAME: &str = "circuitc.simulation_request";
pub const SPICE_MAP_SCHEMA_NAME: &str = "circuitc.spice_identity_map";
pub const RESULT_SCHEMA_NAME: &str = "circuitc.simulation_result";
pub const REPORT_SCHEMA_NAME: &str = "circuitc.simulation_report";
pub const CONTRACT_SCHEMA_VERSION: u32 = 1;
pub const OHMNIVORE_BACKEND_NAME: &str = "ohmnivore";
pub const OHMNIVORE_BACKEND_VERSION: &str = "0.1.0";
pub const OHMNIVORE_BACKEND_CONTRACT: &str = "ohmnivore-cli-csv/v1";
pub const OHMNIVORE_SOURCE_REVISION: &str = env!("CIRCUITC_OHMNIVORE_SOURCE_REVISION");
pub const MAX_CONTRACT_BYTES: usize = 64 * 1024 * 1024;
pub const MAX_CONTRACT_ENTRIES: usize = 10_000;
pub const MAX_VALIDATION_DIAGNOSTICS: usize = 256;

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ContractDiagnostic {
    pub code: &'static str,
    pub path: String,
    pub message: String,
}

#[derive(Clone, Debug, Eq, Ord, PartialEq, PartialOrd, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct NormalizedDiagnostic {
    pub code: String,
    pub message: String,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
#[serde(transparent)]
pub struct RequiredNullable<T>(pub Option<T>);

impl<'de, T: DeserializeOwned> Deserialize<'de> for RequiredNullable<T> {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let value = serde_json::Value::deserialize(deserializer)?;
        if value.is_null() {
            return Ok(Self::none());
        }
        serde_json::from_value(value)
            .map(Self::some)
            .map_err(serde::de::Error::custom)
    }
}

impl<T> RequiredNullable<T> {
    pub const fn none() -> Self {
        Self(None)
    }

    pub const fn some(value: T) -> Self {
        Self(Some(value))
    }

    pub const fn as_ref(&self) -> Option<&T> {
        self.0.as_ref()
    }

    pub const fn is_some(&self) -> bool {
        self.0.is_some()
    }

    pub const fn is_none(&self) -> bool {
        self.0.is_none()
    }
}

impl fmt::Display for ContractDiagnostic {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(formatter, "{} [{}]: {}", self.code, self.path, self.message)
    }
}

impl std::error::Error for ContractDiagnostic {}

#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AnalysisKind {
    DcOperatingPoint,
    AcLinearSweep,
    Transient,
}

#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ExecutionStatus {
    Completed,
    Failed,
    Unsupported,
    Unevaluated,
}

#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AssertionStatus {
    Pass,
    Fail,
    Unsupported,
    Unevaluated,
}

#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AxisKind {
    Scalar,
    FrequencyHertz,
    TimeSeconds,
}

#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SignalKind {
    NetVoltage,
    NetVoltageMagnitude,
    NetVoltagePhaseDegrees,
    BranchCurrent,
    BranchCurrentMagnitude,
    BranchCurrentPhaseDegrees,
}

#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ResultUnit {
    Volt,
    Ampere,
    Degree,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct BackendIdentity {
    pub name: String,
    pub version: String,
    pub contract: String,
    pub source_revision: String,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ReportSample {
    pub kind: AxisKind,
    pub value: String,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RequestAssertion {
    pub path: String,
    pub signal_kind: SignalKind,
    pub canonical_identity: String,
    pub sample: ReportSample,
    pub unit: ResultUnit,
    pub expected: String,
    pub absolute_tolerance: String,
    pub relative_tolerance: String,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RequestAnalysis {
    pub path: String,
    pub kind: AnalysisKind,
    pub netlist_path: String,
    pub netlist_sha256: String,
    pub map_path: String,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SimulationRequest {
    pub schema_name: String,
    pub schema_version: u32,
    pub design: String,
    pub backend: BackendIdentity,
    pub analysis: RequestAnalysis,
    pub assertions: Vec<RequestAssertion>,
}

impl SimulationRequest {
    pub fn validate(&self) -> Result<(), Vec<ContractDiagnostic>> {
        let mut diagnostics = Vec::new();
        validate_header(
            &self.schema_name,
            self.schema_version,
            REQUEST_SCHEMA_NAME,
            &mut diagnostics,
        );
        validate_design_name(&self.design, &mut diagnostics);
        if self.backend.name != OHMNIVORE_BACKEND_NAME
            || self.backend.version != OHMNIVORE_BACKEND_VERSION
            || self.backend.contract != OHMNIVORE_BACKEND_CONTRACT
            || self.backend.source_revision != OHMNIVORE_SOURCE_REVISION
        {
            push(
                &mut diagnostics,
                "CC-SIM-CONTRACT-007",
                "backend",
                format!(
                    "unsupported backend identity; expected {OHMNIVORE_BACKEND_NAME} {OHMNIVORE_BACKEND_VERSION} contract {OHMNIVORE_BACKEND_CONTRACT} at {OHMNIVORE_SOURCE_REVISION}"
                ),
            );
        }
        validate_semantic_path(&self.analysis.path, "analysis.path", &mut diagnostics);
        validate_relative_path(
            &self.analysis.netlist_path,
            "analysis.netlist_path",
            &mut diagnostics,
        );
        validate_digest(
            &self.analysis.netlist_sha256,
            "analysis.netlist_sha256",
            &mut diagnostics,
        );
        validate_relative_path(
            &self.analysis.map_path,
            "analysis.map_path",
            &mut diagnostics,
        );
        validate_entry_count(self.assertions.len(), "assertions", &mut diagnostics);
        let assertion_keys: Vec<_> = self
            .assertions
            .iter()
            .map(|assertion| assertion.path.as_str())
            .collect();
        validate_sorted_unique(&assertion_keys, "assertions", &mut diagnostics);
        for (index, assertion) in self.assertions.iter().enumerate() {
            validate_assertion_intent(
                assertion,
                self.analysis.kind,
                &format!("assertions[{index}]"),
                &mut diagnostics,
            );
        }
        finish(diagnostics)
    }

    pub fn to_canonical_json(&self) -> Result<String, ContractDiagnostic> {
        validated_json(self)
    }

    pub fn verify_netlist_bytes(&self, netlist: &[u8]) -> Result<(), ContractDiagnostic> {
        ensure_valid(self.validate())?;
        verify_digest(
            &self.analysis.netlist_sha256,
            netlist,
            "analysis.netlist_sha256",
        )
    }
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SpiceNetIdentity {
    pub canonical: String,
    pub backend: String,
    pub is_ground: bool,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SpiceDeviceIdentity {
    pub semantic_path: String,
    pub reference: String,
    pub backend: String,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SpiceIdentityMap {
    pub schema_name: String,
    pub schema_version: u32,
    pub design: String,
    pub analysis_path: String,
    pub request_sha256: String,
    pub nets: Vec<SpiceNetIdentity>,
    pub devices: Vec<SpiceDeviceIdentity>,
}

impl SpiceIdentityMap {
    pub fn validate(&self) -> Result<(), Vec<ContractDiagnostic>> {
        let mut diagnostics = Vec::new();
        validate_header(
            &self.schema_name,
            self.schema_version,
            SPICE_MAP_SCHEMA_NAME,
            &mut diagnostics,
        );
        validate_design_name(&self.design, &mut diagnostics);
        validate_semantic_path(&self.analysis_path, "analysis_path", &mut diagnostics);
        validate_digest(&self.request_sha256, "request_sha256", &mut diagnostics);
        validate_entry_count(self.nets.len(), "nets", &mut diagnostics);
        validate_entry_count(self.devices.len(), "devices", &mut diagnostics);

        let net_keys: Vec<_> = self.nets.iter().map(|net| net.canonical.as_str()).collect();
        validate_sorted_unique(&net_keys, "nets", &mut diagnostics);
        let mut backend_nets = BTreeSet::new();
        let mut ground_count = 0_usize;
        for (index, net) in self.nets.iter().enumerate() {
            validate_canonical_token(
                &net.canonical,
                &format!("nets[{index}].canonical"),
                &mut diagnostics,
            );
            if net.backend.is_empty() {
                push(
                    &mut diagnostics,
                    "CC-SIM-CONTRACT-002",
                    format!("nets[{index}].backend"),
                    "backend net identity must be non-empty",
                );
            }
            if !backend_nets.insert(net.backend.to_ascii_uppercase()) {
                push(
                    &mut diagnostics,
                    "CC-SIM-CONTRACT-003",
                    format!("nets[{index}].backend"),
                    "backend net identities must be injective under SPICE case-folding",
                );
            }
            if net.is_ground {
                ground_count += 1;
                if net.backend != "0" {
                    push(
                        &mut diagnostics,
                        "CC-SIM-CONTRACT-002",
                        format!("nets[{index}].backend"),
                        "the canonical ground net must map to backend node `0`",
                    );
                }
            } else if net.backend == "0" || net.backend.eq_ignore_ascii_case("GND") {
                push(
                    &mut diagnostics,
                    "CC-SIM-CONTRACT-002",
                    format!("nets[{index}].backend"),
                    "a non-ground net may not map to a reserved simulator ground alias",
                );
            }
        }
        if ground_count != 1 {
            push(
                &mut diagnostics,
                "CC-SIM-CONTRACT-002",
                "nets",
                format!("a simulation map requires exactly one ground net; found {ground_count}"),
            );
        }

        let device_keys: Vec<_> = self
            .devices
            .iter()
            .map(|device| device.semantic_path.as_str())
            .collect();
        validate_sorted_unique(&device_keys, "devices", &mut diagnostics);
        let mut references = BTreeSet::new();
        let mut backend_devices = BTreeSet::new();
        for (index, device) in self.devices.iter().enumerate() {
            validate_semantic_path(
                &device.semantic_path,
                &format!("devices[{index}].semantic_path"),
                &mut diagnostics,
            );
            validate_canonical_token(
                &device.reference,
                &format!("devices[{index}].reference"),
                &mut diagnostics,
            );
            if device.backend.is_empty() {
                push(
                    &mut diagnostics,
                    "CC-SIM-CONTRACT-002",
                    format!("devices[{index}].backend"),
                    "backend device identity must be non-empty",
                );
            }
            if !references.insert(device.reference.as_str()) {
                push(
                    &mut diagnostics,
                    "CC-SIM-CONTRACT-003",
                    format!("devices[{index}].reference"),
                    "canonical component references must be unique",
                );
            }
            if !backend_devices.insert(device.backend.to_ascii_uppercase()) {
                push(
                    &mut diagnostics,
                    "CC-SIM-CONTRACT-003",
                    format!("devices[{index}].backend"),
                    "backend device identities must be injective under SPICE case-folding",
                );
            }
        }
        finish(diagnostics)
    }

    pub fn to_canonical_json(&self) -> Result<String, ContractDiagnostic> {
        validated_json(self)
    }

    pub fn verify_request_bytes(
        &self,
        request_bytes: &[u8],
    ) -> Result<SimulationRequest, ContractDiagnostic> {
        ensure_valid(self.validate())?;
        let request = parse_canonical_bytes(request_bytes, parse_request)?;
        verify_digest(&self.request_sha256, request_bytes, "request_sha256")?;
        if self.design != request.design || self.analysis_path != request.analysis.path {
            return Err(binding_error(
                "request",
                "SPICE identity map design and analysis path must match the bound request",
            ));
        }
        Ok(request)
    }
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ResultAxis {
    pub kind: AxisKind,
    pub samples: Vec<String>,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ResultSignal {
    pub kind: SignalKind,
    pub canonical_identity: String,
    pub unit: ResultUnit,
    pub values: Vec<String>,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SimulationResult {
    pub schema_name: String,
    pub schema_version: u32,
    pub design: String,
    pub analysis_path: String,
    pub analysis_kind: AnalysisKind,
    pub status: ExecutionStatus,
    pub request_sha256: String,
    pub map_sha256: String,
    pub axis: ResultAxis,
    pub signals: Vec<ResultSignal>,
    pub diagnostics: Vec<NormalizedDiagnostic>,
}

impl SimulationResult {
    pub fn validate(&self) -> Result<(), Vec<ContractDiagnostic>> {
        let mut diagnostics = Vec::new();
        validate_header(
            &self.schema_name,
            self.schema_version,
            RESULT_SCHEMA_NAME,
            &mut diagnostics,
        );
        validate_design_name(&self.design, &mut diagnostics);
        validate_semantic_path(&self.analysis_path, "analysis_path", &mut diagnostics);
        validate_digest(&self.request_sha256, "request_sha256", &mut diagnostics);
        validate_digest(&self.map_sha256, "map_sha256", &mut diagnostics);
        validate_entry_count(self.axis.samples.len(), "axis.samples", &mut diagnostics);
        validate_entry_count(self.signals.len(), "signals", &mut diagnostics);
        validate_entry_count(self.diagnostics.len(), "diagnostics", &mut diagnostics);

        let expected_axis = match self.analysis_kind {
            AnalysisKind::DcOperatingPoint => AxisKind::Scalar,
            AnalysisKind::AcLinearSweep => AxisKind::FrequencyHertz,
            AnalysisKind::Transient => AxisKind::TimeSeconds,
        };
        if self.axis.kind != expected_axis {
            push(
                &mut diagnostics,
                "CC-SIM-CONTRACT-002",
                "axis.kind",
                format!("analysis requires axis kind {expected_axis:?}"),
            );
        }

        if self.status == ExecutionStatus::Completed {
            if !self.diagnostics.is_empty() {
                push(
                    &mut diagnostics,
                    "CC-SIM-CONTRACT-002",
                    "diagnostics",
                    "a completed result may not contain execution diagnostics",
                );
            }
            validate_axis(&self.axis, &mut diagnostics);
            if self.signals.is_empty() {
                push(
                    &mut diagnostics,
                    "CC-SIM-CONTRACT-002",
                    "signals",
                    "a completed result must contain at least one signal",
                );
            }
        } else {
            if !self.axis.samples.is_empty() || !self.signals.is_empty() {
                push(
                    &mut diagnostics,
                    "CC-SIM-CONTRACT-002",
                    "status",
                    "a non-completed result may not publish partial samples or signals",
                );
            }
            if self.diagnostics.is_empty() {
                push(
                    &mut diagnostics,
                    "CC-SIM-CONTRACT-002",
                    "diagnostics",
                    "a non-completed result requires at least one normalized diagnostic",
                );
            }
        }
        let diagnostic_keys: Vec<_> = self
            .diagnostics
            .iter()
            .map(|diagnostic| (diagnostic.code.as_str(), diagnostic.message.as_str()))
            .collect();
        validate_sorted_unique(&diagnostic_keys, "diagnostics", &mut diagnostics);
        for (index, diagnostic) in self.diagnostics.iter().enumerate() {
            validate_normalized_diagnostic(
                diagnostic,
                &format!("diagnostics[{index}]"),
                &mut diagnostics,
            );
        }

        let signal_keys: Vec<_> = self
            .signals
            .iter()
            .map(|signal| (signal.kind, signal.canonical_identity.as_str()))
            .collect();
        validate_sorted_unique(&signal_keys, "signals", &mut diagnostics);
        for (index, signal) in self.signals.iter().enumerate() {
            validate_entry_count(
                signal.values.len(),
                &format!("signals[{index}].values"),
                &mut diagnostics,
            );
            validate_signal_identity(
                signal.kind,
                &signal.canonical_identity,
                &format!("signals[{index}].canonical_identity"),
                &mut diagnostics,
            );
            if signal.unit != expected_unit(signal.kind) {
                push(
                    &mut diagnostics,
                    "CC-SIM-CONTRACT-002",
                    format!("signals[{index}].unit"),
                    "signal unit does not match its signal kind",
                );
            }
            if signal.values.len() != self.axis.samples.len() {
                push(
                    &mut diagnostics,
                    "CC-SIM-CONTRACT-002",
                    format!("signals[{index}].values"),
                    "every signal must contain exactly one value per axis sample",
                );
            }
            for (value_index, value) in signal.values.iter().enumerate() {
                validate_number(
                    value,
                    &format!("signals[{index}].values[{value_index}]"),
                    &mut diagnostics,
                );
            }
        }
        finish(diagnostics)
    }

    pub fn to_canonical_json(&self) -> Result<String, ContractDiagnostic> {
        validated_json(self)
    }

    pub fn verify_binding_bytes(
        &self,
        request_bytes: &[u8],
        map_bytes: &[u8],
    ) -> Result<(SimulationRequest, SpiceIdentityMap), ContractDiagnostic> {
        ensure_valid(self.validate())?;
        let map = parse_canonical_bytes(map_bytes, parse_spice_identity_map)?;
        let request = map.verify_request_bytes(request_bytes)?;
        verify_digest(&self.request_sha256, request_bytes, "request_sha256")?;
        verify_digest(&self.map_sha256, map_bytes, "map_sha256")?;
        if self.design != request.design
            || self.design != map.design
            || self.analysis_path != request.analysis.path
            || self.analysis_path != map.analysis_path
            || self.analysis_kind != request.analysis.kind
        {
            return Err(binding_error(
                "result",
                "simulation result design, analysis path, and kind must match its bound request and map",
            ));
        }
        verify_result_signal_mappings(self, &map)?;
        verify_request_result_coverage(self, &request)?;
        Ok((request, map))
    }
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AssertionOutcome {
    pub path: String,
    pub status: AssertionStatus,
    pub signal_kind: SignalKind,
    pub canonical_identity: String,
    pub sample: ReportSample,
    pub unit: ResultUnit,
    pub expected: String,
    pub actual: RequiredNullable<String>,
    pub absolute_tolerance: String,
    pub relative_tolerance: String,
    pub diagnostic: RequiredNullable<NormalizedDiagnostic>,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ReportSummary {
    pub pass: u32,
    pub fail: u32,
    pub unsupported: u32,
    pub unevaluated: u32,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SimulationReport {
    pub schema_name: String,
    pub schema_version: u32,
    pub design: String,
    pub analysis_path: String,
    pub analysis_kind: AnalysisKind,
    pub request_sha256: String,
    pub map_sha256: String,
    pub result_sha256: String,
    pub assertions: Vec<AssertionOutcome>,
    pub summary: ReportSummary,
}

impl SimulationReport {
    pub fn validate(&self) -> Result<(), Vec<ContractDiagnostic>> {
        let mut diagnostics = Vec::new();
        validate_header(
            &self.schema_name,
            self.schema_version,
            REPORT_SCHEMA_NAME,
            &mut diagnostics,
        );
        validate_design_name(&self.design, &mut diagnostics);
        validate_semantic_path(&self.analysis_path, "analysis_path", &mut diagnostics);
        validate_digest(&self.request_sha256, "request_sha256", &mut diagnostics);
        validate_digest(&self.map_sha256, "map_sha256", &mut diagnostics);
        validate_digest(&self.result_sha256, "result_sha256", &mut diagnostics);
        validate_entry_count(self.assertions.len(), "assertions", &mut diagnostics);

        let assertion_keys: Vec<_> = self
            .assertions
            .iter()
            .map(|assertion| assertion.path.as_str())
            .collect();
        validate_sorted_unique(&assertion_keys, "assertions", &mut diagnostics);
        let mut actual_summary = ReportSummary {
            pass: 0,
            fail: 0,
            unsupported: 0,
            unevaluated: 0,
        };
        for (index, assertion) in self.assertions.iter().enumerate() {
            validate_semantic_path(
                &assertion.path,
                &format!("assertions[{index}].path"),
                &mut diagnostics,
            );
            validate_signal_identity(
                assertion.signal_kind,
                &assertion.canonical_identity,
                &format!("assertions[{index}].canonical_identity"),
                &mut diagnostics,
            );
            if assertion.unit != expected_unit(assertion.signal_kind) {
                push(
                    &mut diagnostics,
                    "CC-SIM-CONTRACT-002",
                    format!("assertions[{index}].unit"),
                    "assertion unit does not match its signal kind",
                );
            }
            let expected_axis = match self.analysis_kind {
                AnalysisKind::DcOperatingPoint => AxisKind::Scalar,
                AnalysisKind::AcLinearSweep => AxisKind::FrequencyHertz,
                AnalysisKind::Transient => AxisKind::TimeSeconds,
            };
            if assertion.sample.kind != expected_axis {
                push(
                    &mut diagnostics,
                    "CC-SIM-CONTRACT-002",
                    format!("assertions[{index}].sample.kind"),
                    "assertion sample kind does not match its analysis kind",
                );
            }
            validate_scalar_sample(
                &assertion.sample,
                &format!("assertions[{index}].sample"),
                &mut diagnostics,
            );
            for (field, value) in [
                ("sample.value", &assertion.sample.value),
                ("expected", &assertion.expected),
                ("absolute_tolerance", &assertion.absolute_tolerance),
                ("relative_tolerance", &assertion.relative_tolerance),
            ] {
                validate_number(
                    value,
                    &format!("assertions[{index}].{field}"),
                    &mut diagnostics,
                );
            }
            if let Some(actual) = assertion.actual.as_ref() {
                validate_number(
                    actual,
                    &format!("assertions[{index}].actual"),
                    &mut diagnostics,
                );
            }
            for (field, value) in [
                ("absolute_tolerance", &assertion.absolute_tolerance),
                ("relative_tolerance", &assertion.relative_tolerance),
            ] {
                if value.parse::<f64>().is_ok_and(|value| value < 0.0) {
                    push(
                        &mut diagnostics,
                        "CC-SIM-CONTRACT-002",
                        format!("assertions[{index}].{field}"),
                        "assertion tolerances must be non-negative",
                    );
                }
            }
            if let Some(diagnostic) = assertion.diagnostic.as_ref() {
                validate_normalized_diagnostic(
                    diagnostic,
                    &format!("assertions[{index}].diagnostic"),
                    &mut diagnostics,
                );
            }
            match assertion.status {
                AssertionStatus::Pass => actual_summary.pass += 1,
                AssertionStatus::Fail => actual_summary.fail += 1,
                AssertionStatus::Unsupported => actual_summary.unsupported += 1,
                AssertionStatus::Unevaluated => actual_summary.unevaluated += 1,
            }
            match assertion.status {
                AssertionStatus::Pass | AssertionStatus::Fail => {
                    if assertion.actual.is_none() {
                        push(
                            &mut diagnostics,
                            "CC-SIM-CONTRACT-002",
                            format!("assertions[{index}].actual"),
                            "pass and fail outcomes require an actual value",
                        );
                    } else {
                        validate_assertion_status(assertion, index, &mut diagnostics);
                    }
                }
                AssertionStatus::Unsupported | AssertionStatus::Unevaluated => {
                    if assertion.actual.is_some() || assertion.diagnostic.is_none() {
                        push(
                            &mut diagnostics,
                            "CC-SIM-CONTRACT-002",
                            format!("assertions[{index}]"),
                            "unsupported and unevaluated outcomes require a diagnostic and no actual value",
                        );
                    }
                }
            }
        }
        if actual_summary != self.summary {
            push(
                &mut diagnostics,
                "CC-SIM-CONTRACT-002",
                "summary",
                "report summary counts must exactly match assertion statuses",
            );
        }
        finish(diagnostics)
    }

    pub fn to_canonical_json(&self) -> Result<String, ContractDiagnostic> {
        validated_json(self)
    }

    pub fn verify_binding_bytes(
        &self,
        request_bytes: &[u8],
        map_bytes: &[u8],
        result_bytes: &[u8],
    ) -> Result<(SimulationRequest, SpiceIdentityMap, SimulationResult), ContractDiagnostic> {
        ensure_valid(self.validate())?;
        let result = parse_canonical_bytes(result_bytes, parse_result)?;
        let (request, map) = result.verify_binding_bytes(request_bytes, map_bytes)?;
        verify_digest(&self.request_sha256, request_bytes, "request_sha256")?;
        verify_digest(&self.map_sha256, map_bytes, "map_sha256")?;
        verify_digest(&self.result_sha256, result_bytes, "result_sha256")?;
        if self.design != request.design
            || self.analysis_path != request.analysis.path
            || self.analysis_kind != request.analysis.kind
            || self.design != result.design
            || self.analysis_path != result.analysis_path
            || self.analysis_kind != result.analysis_kind
        {
            return Err(binding_error(
                "report",
                "simulation report design, analysis path, and kind must match its bound request and result",
            ));
        }
        verify_report_assertions(self, &request, &result)?;
        Ok((request, map, result))
    }
}

pub fn parse_request(input: &str) -> Result<SimulationRequest, ContractDiagnostic> {
    parse_validated(input)
}

pub fn parse_spice_identity_map(input: &str) -> Result<SpiceIdentityMap, ContractDiagnostic> {
    parse_validated(input)
}

pub fn parse_result(input: &str) -> Result<SimulationResult, ContractDiagnostic> {
    parse_validated(input)
}

pub fn parse_report(input: &str) -> Result<SimulationReport, ContractDiagnostic> {
    parse_validated(input)
}

pub fn sha256_hex(bytes: &[u8]) -> String {
    let digest = Sha256::digest(bytes);
    digest.iter().map(|byte| format!("{byte:02x}")).collect()
}

pub fn canonical_f64(value: f64) -> Result<String, ContractDiagnostic> {
    if !value.is_finite() {
        return Err(ContractDiagnostic {
            code: "CC-SIM-CONTRACT-005",
            path: "number".to_owned(),
            message: "simulation contract numbers must be finite".to_owned(),
        });
    }
    let value = if value == 0.0 { 0.0 } else { value };
    Ok(format!("{value:.17e}"))
}

trait ValidatedContract: Serialize + DeserializeOwned {
    fn validate_contract(&self) -> Result<(), Vec<ContractDiagnostic>>;
}

impl ValidatedContract for SimulationRequest {
    fn validate_contract(&self) -> Result<(), Vec<ContractDiagnostic>> {
        self.validate()
    }
}

impl ValidatedContract for SpiceIdentityMap {
    fn validate_contract(&self) -> Result<(), Vec<ContractDiagnostic>> {
        self.validate()
    }
}

impl ValidatedContract for SimulationResult {
    fn validate_contract(&self) -> Result<(), Vec<ContractDiagnostic>> {
        self.validate()
    }
}

impl ValidatedContract for SimulationReport {
    fn validate_contract(&self) -> Result<(), Vec<ContractDiagnostic>> {
        self.validate()
    }
}

fn parse_validated<T: ValidatedContract>(input: &str) -> Result<T, ContractDiagnostic> {
    if input.len() > MAX_CONTRACT_BYTES {
        return Err(ContractDiagnostic {
            code: "CC-SIM-CONTRACT-006",
            path: "document".to_owned(),
            message: format!(
                "simulation contract exceeds the {MAX_CONTRACT_BYTES}-byte input limit"
            ),
        });
    }
    let value: T = serde_json::from_str(input).map_err(|error| ContractDiagnostic {
        code: "CC-SIM-CONTRACT-001",
        path: "document".to_owned(),
        message: format!("invalid strict JSON contract: {error}"),
    })?;
    value
        .validate_contract()
        .map_err(|diagnostics| diagnostics.into_iter().next().expect("validation failed"))?;
    let canonical = serialize_canonical(&value)?;
    if canonical.as_bytes() != input.as_bytes() {
        return Err(ContractDiagnostic {
            code: "CC-SIM-CONTRACT-006",
            path: "document".to_owned(),
            message: "simulation contract bytes are not in canonical JSON encoding".to_owned(),
        });
    }
    Ok(value)
}

fn validated_json<T: ValidatedContract>(value: &T) -> Result<String, ContractDiagnostic> {
    value
        .validate_contract()
        .map_err(|diagnostics| diagnostics.into_iter().next().expect("validation failed"))?;
    serialize_canonical(value)
}

fn serialize_canonical<T: Serialize>(value: &T) -> Result<String, ContractDiagnostic> {
    let mut json = serde_json::to_string_pretty(value).map_err(|error| ContractDiagnostic {
        code: "CC-SIM-CONTRACT-001",
        path: "document".to_owned(),
        message: format!("could not serialize simulation contract: {error}"),
    })?;
    json.push('\n');
    if json.len() > MAX_CONTRACT_BYTES {
        return Err(ContractDiagnostic {
            code: "CC-SIM-CONTRACT-006",
            path: "document".to_owned(),
            message: format!(
                "canonical simulation contract exceeds the {MAX_CONTRACT_BYTES}-byte output limit"
            ),
        });
    }
    Ok(json)
}

fn parse_canonical_bytes<T>(
    bytes: &[u8],
    parser: fn(&str) -> Result<T, ContractDiagnostic>,
) -> Result<T, ContractDiagnostic> {
    let input = std::str::from_utf8(bytes).map_err(|error| ContractDiagnostic {
        code: "CC-SIM-CONTRACT-001",
        path: "document".to_owned(),
        message: format!("simulation contract is not UTF-8: {error}"),
    })?;
    parser(input)
}

fn verify_digest(expected: &str, bytes: &[u8], path: &str) -> Result<(), ContractDiagnostic> {
    let actual = sha256_hex(bytes);
    if actual != expected {
        return Err(ContractDiagnostic {
            code: "CC-SIM-CONTRACT-004",
            path: path.to_owned(),
            message: format!("artifact SHA-256 mismatch: expected `{expected}`; found `{actual}`"),
        });
    }
    Ok(())
}

fn binding_error(path: &str, message: &str) -> ContractDiagnostic {
    ContractDiagnostic {
        code: "CC-SIM-CONTRACT-004",
        path: path.to_owned(),
        message: message.to_owned(),
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

fn verify_result_signal_mappings(
    result: &SimulationResult,
    map: &SpiceIdentityMap,
) -> Result<(), ContractDiagnostic> {
    let nets: BTreeSet<_> = map
        .nets
        .iter()
        .map(|identity| identity.canonical.as_str())
        .collect();
    let devices: BTreeSet<_> = map
        .devices
        .iter()
        .map(|identity| identity.semantic_path.as_str())
        .collect();
    for (index, signal) in result.signals.iter().enumerate() {
        let mapped = match signal.kind {
            SignalKind::NetVoltage
            | SignalKind::NetVoltageMagnitude
            | SignalKind::NetVoltagePhaseDegrees => {
                nets.contains(signal.canonical_identity.as_str())
            }
            SignalKind::BranchCurrent
            | SignalKind::BranchCurrentMagnitude
            | SignalKind::BranchCurrentPhaseDegrees => {
                devices.contains(signal.canonical_identity.as_str())
            }
        };
        if !mapped {
            return Err(binding_error(
                &format!("signals[{index}].canonical_identity"),
                "normalized signal identity is absent from the matching SPICE identity-map namespace",
            ));
        }
    }
    Ok(())
}

pub(super) struct ResultIndex<'a> {
    samples: HashMap<&'a str, usize>,
    signals: HashMap<(SignalKind, &'a str, ResultUnit), &'a ResultSignal>,
}

impl<'a> ResultIndex<'a> {
    pub(super) fn new(result: &'a SimulationResult) -> Self {
        Self {
            samples: result
                .axis
                .samples
                .iter()
                .enumerate()
                .map(|(index, sample)| (sample.as_str(), index))
                .collect(),
            signals: result
                .signals
                .iter()
                .map(|signal| {
                    (
                        (signal.kind, signal.canonical_identity.as_str(), signal.unit),
                        signal,
                    )
                })
                .collect(),
        }
    }

    pub(super) fn actual(
        &self,
        signal_kind: SignalKind,
        canonical_identity: &str,
        unit: ResultUnit,
        sample: &str,
    ) -> Option<&'a String> {
        let sample_index = self.samples.get(sample)?;
        self.signals
            .get(&(signal_kind, canonical_identity, unit))?
            .values
            .get(*sample_index)
    }
}

fn verify_request_result_coverage(
    result: &SimulationResult,
    request: &SimulationRequest,
) -> Result<(), ContractDiagnostic> {
    if result.status != ExecutionStatus::Completed {
        return Ok(());
    }
    let index = ResultIndex::new(result);
    for (assertion_index, assertion) in request.assertions.iter().enumerate() {
        if index
            .actual(
                assertion.signal_kind,
                &assertion.canonical_identity,
                assertion.unit,
                &assertion.sample.value,
            )
            .is_none()
        {
            return Err(binding_error(
                &format!("request.assertions[{assertion_index}]"),
                "completed result does not contain the requested normalized signal and sample",
            ));
        }
    }
    Ok(())
}

fn verify_report_assertions(
    report: &SimulationReport,
    request: &SimulationRequest,
    result: &SimulationResult,
) -> Result<(), ContractDiagnostic> {
    if report.assertions.len() != request.assertions.len() {
        return Err(binding_error(
            "assertions",
            "report must contain exactly one outcome for every request assertion",
        ));
    }

    let result_index = ResultIndex::new(result);
    for (index, (outcome, intent)) in report
        .assertions
        .iter()
        .zip(&request.assertions)
        .enumerate()
    {
        if outcome.path != intent.path
            || outcome.signal_kind != intent.signal_kind
            || outcome.canonical_identity != intent.canonical_identity
            || outcome.sample != intent.sample
            || outcome.unit != intent.unit
            || outcome.expected != intent.expected
            || outcome.absolute_tolerance != intent.absolute_tolerance
            || outcome.relative_tolerance != intent.relative_tolerance
        {
            return Err(binding_error(
                &format!("assertions[{index}]"),
                "report assertion intent must exactly match the digest-bound request assertion",
            ));
        }

        let authenticated_actual = result_index.actual(
            outcome.signal_kind,
            &outcome.canonical_identity,
            outcome.unit,
            &outcome.sample.value,
        );

        match outcome.status {
            AssertionStatus::Pass | AssertionStatus::Fail => {
                if result.status != ExecutionStatus::Completed {
                    return Err(binding_error(
                        &format!("assertions[{index}].status"),
                        "a non-completed simulation result cannot authenticate a pass or fail outcome",
                    ));
                }
                let Some(authenticated_actual) = authenticated_actual else {
                    return Err(binding_error(
                        &format!("assertions[{index}]"),
                        "pass/fail outcome does not resolve to an exact normalized result signal and sample",
                    ));
                };
                if outcome.actual.as_ref() != Some(authenticated_actual) {
                    return Err(binding_error(
                        &format!("assertions[{index}].actual"),
                        "report actual value does not equal the authenticated normalized result value",
                    ));
                }
            }
            AssertionStatus::Unsupported | AssertionStatus::Unevaluated => {
                let expected_status = match result.status {
                    ExecutionStatus::Completed => None,
                    ExecutionStatus::Unsupported => Some(AssertionStatus::Unsupported),
                    ExecutionStatus::Failed | ExecutionStatus::Unevaluated => {
                        Some(AssertionStatus::Unevaluated)
                    }
                };
                if expected_status != Some(outcome.status) || authenticated_actual.is_some() {
                    return Err(binding_error(
                        &format!("assertions[{index}].status"),
                        "unsupported/unevaluated assertion status must match the non-completed result status and contain no authenticated sample",
                    ));
                }
            }
        }
    }
    Ok(())
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
            "CC-SIM-CONTRACT-001",
            "schema_name",
            format!("expected schema name `{expected_name}`; found `{actual_name}`"),
        );
    }
    if actual_version != CONTRACT_SCHEMA_VERSION {
        push(
            diagnostics,
            "CC-SIM-CONTRACT-001",
            "schema_version",
            format!("expected schema version {CONTRACT_SCHEMA_VERSION}; found {actual_version}"),
        );
    }
}

fn validate_entry_count(count: usize, path: &str, diagnostics: &mut Vec<ContractDiagnostic>) {
    if count > MAX_CONTRACT_ENTRIES {
        push(
            diagnostics,
            "CC-SIM-CONTRACT-006",
            path,
            format!(
                "contract collection contains {count} entries; maximum is {MAX_CONTRACT_ENTRIES}"
            ),
        );
    }
}

fn validate_assertion_intent(
    assertion: &RequestAssertion,
    analysis_kind: AnalysisKind,
    path: &str,
    diagnostics: &mut Vec<ContractDiagnostic>,
) {
    validate_semantic_path(&assertion.path, &format!("{path}.path"), diagnostics);
    validate_signal_identity(
        assertion.signal_kind,
        &assertion.canonical_identity,
        &format!("{path}.canonical_identity"),
        diagnostics,
    );
    if assertion.unit != expected_unit(assertion.signal_kind) {
        push(
            diagnostics,
            "CC-SIM-CONTRACT-002",
            format!("{path}.unit"),
            "assertion unit does not match its signal kind",
        );
    }
    let expected_axis = match analysis_kind {
        AnalysisKind::DcOperatingPoint => AxisKind::Scalar,
        AnalysisKind::AcLinearSweep => AxisKind::FrequencyHertz,
        AnalysisKind::Transient => AxisKind::TimeSeconds,
    };
    if assertion.sample.kind != expected_axis {
        push(
            diagnostics,
            "CC-SIM-CONTRACT-002",
            format!("{path}.sample.kind"),
            "assertion sample kind does not match its analysis kind",
        );
    }
    validate_scalar_sample(&assertion.sample, &format!("{path}.sample"), diagnostics);
    for (field, value) in [
        ("sample.value", &assertion.sample.value),
        ("expected", &assertion.expected),
        ("absolute_tolerance", &assertion.absolute_tolerance),
        ("relative_tolerance", &assertion.relative_tolerance),
    ] {
        validate_number(value, &format!("{path}.{field}"), diagnostics);
    }
    for (field, value) in [
        ("absolute_tolerance", &assertion.absolute_tolerance),
        ("relative_tolerance", &assertion.relative_tolerance),
    ] {
        if value.parse::<f64>().is_ok_and(|value| value < 0.0) {
            push(
                diagnostics,
                "CC-SIM-CONTRACT-002",
                format!("{path}.{field}"),
                "assertion tolerances must be non-negative",
            );
        }
    }
}

fn validate_design_name(name: &str, diagnostics: &mut Vec<ContractDiagnostic>) {
    let mut bytes = name.bytes();
    let valid_start = bytes
        .next()
        .is_some_and(|byte| byte.is_ascii_alphabetic() || byte == b'_');
    if !valid_start
        || !bytes.all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'_' | b'-'))
    {
        push(
            diagnostics,
            "CC-SIM-CONTRACT-002",
            "design",
            "design must be a safe CircuitC artifact stem",
        );
    }
}

fn validate_canonical_token(value: &str, path: &str, diagnostics: &mut Vec<ContractDiagnostic>) {
    if value.is_empty()
        || !value.bytes().all(|byte| {
            byte.is_ascii_alphanumeric() || matches!(byte, b'_' | b'+' | b'-' | b'.' | b'/')
        })
    {
        push(
            diagnostics,
            "CC-SIM-CONTRACT-002",
            path,
            "canonical identity must be a non-empty CircuitC ASCII token",
        );
    }
}

fn validate_signal_identity(
    kind: SignalKind,
    value: &str,
    path: &str,
    diagnostics: &mut Vec<ContractDiagnostic>,
) {
    match kind {
        SignalKind::NetVoltage
        | SignalKind::NetVoltageMagnitude
        | SignalKind::NetVoltagePhaseDegrees => {
            validate_canonical_token(value, path, diagnostics);
        }
        SignalKind::BranchCurrent
        | SignalKind::BranchCurrentMagnitude
        | SignalKind::BranchCurrentPhaseDegrees => {
            validate_semantic_path(value, path, diagnostics);
        }
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
        push(
            diagnostics,
            "CC-SIM-CONTRACT-002",
            path,
            "semantic path must be a non-empty canonical CircuitC path",
        );
    }
}

fn validate_relative_path(value: &str, path: &str, diagnostics: &mut Vec<ContractDiagnostic>) {
    if value.is_empty()
        || value.starts_with('/')
        || value.contains(['\\', '\0'])
        || value
            .split('/')
            .any(|segment| segment.is_empty() || matches!(segment, "." | ".."))
    {
        push(
            diagnostics,
            "CC-SIM-CONTRACT-002",
            path,
            "artifact path must be a canonical portable relative path",
        );
    }
}

fn validate_digest(value: &str, path: &str, diagnostics: &mut Vec<ContractDiagnostic>) {
    if !is_lower_hex(value, 64) {
        push(
            diagnostics,
            "CC-SIM-CONTRACT-004",
            path,
            "SHA-256 digest must be exactly 64 lowercase hexadecimal characters",
        );
    }
}

fn is_lower_hex(value: &str, length: usize) -> bool {
    value.len() == length
        && value
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
}

fn validate_sorted_unique<T: Ord + fmt::Debug>(
    values: &[T],
    path: &str,
    diagnostics: &mut Vec<ContractDiagnostic>,
) {
    if values.windows(2).any(|pair| pair[0] >= pair[1]) {
        push(
            diagnostics,
            "CC-SIM-CONTRACT-003",
            path,
            "entries must be strictly sorted and unique by their canonical key",
        );
    }
}

fn validate_axis(axis: &ResultAxis, diagnostics: &mut Vec<ContractDiagnostic>) {
    if axis.samples.is_empty() {
        push(
            diagnostics,
            "CC-SIM-CONTRACT-002",
            "axis.samples",
            "a completed result requires at least one axis sample",
        );
        return;
    }
    let mut previous = None;
    for (index, sample) in axis.samples.iter().enumerate() {
        validate_number(sample, &format!("axis.samples[{index}]"), diagnostics);
        if let Ok(value) = sample.parse::<f64>() {
            if axis.kind == AxisKind::Scalar && (axis.samples.len() != 1 || value != 0.0) {
                push(
                    diagnostics,
                    "CC-SIM-CONTRACT-002",
                    "axis.samples",
                    "a scalar axis contains exactly one canonical zero sample",
                );
            }
            if axis.kind != AxisKind::Scalar && previous.is_some_and(|previous| value <= previous) {
                push(
                    diagnostics,
                    "CC-SIM-CONTRACT-003",
                    format!("axis.samples[{index}]"),
                    "frequency and time samples must be strictly increasing",
                );
            }
            previous = Some(value);
        }
    }
}

fn validate_scalar_sample(
    sample: &ReportSample,
    path: &str,
    diagnostics: &mut Vec<ContractDiagnostic>,
) {
    if sample.kind == AxisKind::Scalar
        && sample.value.parse::<f64>().is_ok_and(|value| value != 0.0)
    {
        push(
            diagnostics,
            "CC-SIM-CONTRACT-002",
            format!("{path}.value"),
            "a scalar assertion sample must be canonical zero",
        );
    }
}

fn validate_number(value: &str, path: &str, diagnostics: &mut Vec<ContractDiagnostic>) {
    let Ok(parsed) = value.parse::<f64>() else {
        push(
            diagnostics,
            "CC-SIM-CONTRACT-005",
            path,
            "simulation number must be a finite canonical scientific-decimal string",
        );
        return;
    };
    let Ok(canonical) = canonical_f64(parsed) else {
        push(
            diagnostics,
            "CC-SIM-CONTRACT-005",
            path,
            "simulation number must be finite",
        );
        return;
    };
    if canonical != value {
        push(
            diagnostics,
            "CC-SIM-CONTRACT-005",
            path,
            format!("simulation number is not canonical; expected `{canonical}`"),
        );
    }
}

fn validate_normalized_diagnostic(
    diagnostic: &NormalizedDiagnostic,
    path: &str,
    diagnostics: &mut Vec<ContractDiagnostic>,
) {
    let valid_code = diagnostic.code.split('-').count() >= 2
        && diagnostic
            .code
            .split('-')
            .all(|part| !part.is_empty() && part.bytes().all(|byte| byte.is_ascii_alphanumeric()))
        && diagnostic
            .code
            .bytes()
            .all(|byte| byte.is_ascii_uppercase() || byte.is_ascii_digit() || byte == b'-')
        && diagnostic
            .code
            .as_bytes()
            .first()
            .is_some_and(u8::is_ascii_alphanumeric)
        && diagnostic
            .code
            .as_bytes()
            .last()
            .is_some_and(u8::is_ascii_alphanumeric);
    if !valid_code {
        push(
            diagnostics,
            "CC-SIM-CONTRACT-002",
            format!("{path}.code"),
            "diagnostic code must be a non-empty uppercase machine code with hyphen-separated parts",
        );
    }
    if diagnostic.message.trim().is_empty()
        || diagnostic.message.trim() != diagnostic.message
        || diagnostic
            .message
            .chars()
            .any(|character| character.is_control())
    {
        push(
            diagnostics,
            "CC-SIM-CONTRACT-002",
            format!("{path}.message"),
            "diagnostic message must be trimmed, non-empty, and single-line without control characters",
        );
    }
}

fn validate_assertion_status(
    assertion: &AssertionOutcome,
    index: usize,
    diagnostics: &mut Vec<ContractDiagnostic>,
) {
    let Some(actual) = assertion
        .actual
        .as_ref()
        .and_then(|value| value.parse::<f64>().ok())
    else {
        return;
    };
    let Ok(expected) = assertion.expected.parse::<f64>() else {
        return;
    };
    let Ok(absolute_tolerance) = assertion.absolute_tolerance.parse::<f64>() else {
        return;
    };
    let Ok(relative_tolerance) = assertion.relative_tolerance.parse::<f64>() else {
        return;
    };
    if !actual.is_finite()
        || !expected.is_finite()
        || !absolute_tolerance.is_finite()
        || !relative_tolerance.is_finite()
        || absolute_tolerance < 0.0
        || relative_tolerance < 0.0
    {
        return;
    }

    let comparison_path = format!("assertions[{index}].status");
    let expected_status = match compare_assertion_values(
        actual,
        expected,
        absolute_tolerance,
        relative_tolerance,
        &comparison_path,
    ) {
        Ok(status) => status,
        Err(diagnostic) => {
            push(
                diagnostics,
                diagnostic.code,
                diagnostic.path,
                diagnostic.message,
            );
            return;
        }
    };
    if assertion.status != expected_status {
        push(
            diagnostics,
            "CC-SIM-CONTRACT-002",
            format!("assertions[{index}].status"),
            "pass/fail status contradicts the inclusive absolute-plus-relative tolerance formula",
        );
    }
}

pub(super) fn compare_assertion_values(
    actual: f64,
    expected: f64,
    absolute_tolerance: f64,
    relative_tolerance: f64,
    path: &str,
) -> Result<AssertionStatus, ContractDiagnostic> {
    let difference = (actual - expected).abs();
    let allowed = absolute_tolerance + relative_tolerance * expected.abs();
    if !difference.is_finite() || !allowed.is_finite() {
        return Err(ContractDiagnostic {
            code: "CC-SIM-CONTRACT-005",
            path: path.to_owned(),
            message: "assertion comparison overflowed the finite floating-point result boundary"
                .to_owned(),
        });
    }

    Ok(if difference <= allowed {
        AssertionStatus::Pass
    } else {
        AssertionStatus::Fail
    })
}

fn expected_unit(kind: SignalKind) -> ResultUnit {
    match kind {
        SignalKind::NetVoltage | SignalKind::NetVoltageMagnitude => ResultUnit::Volt,
        SignalKind::NetVoltagePhaseDegrees | SignalKind::BranchCurrentPhaseDegrees => {
            ResultUnit::Degree
        }
        SignalKind::BranchCurrent | SignalKind::BranchCurrentMagnitude => ResultUnit::Ampere,
    }
}

fn push(
    diagnostics: &mut Vec<ContractDiagnostic>,
    code: &'static str,
    path: impl Into<String>,
    message: impl Into<String>,
) {
    if diagnostics.len() >= MAX_VALIDATION_DIAGNOSTICS {
        return;
    }
    diagnostics.push(ContractDiagnostic {
        code,
        path: path.into(),
        message: message.into(),
    });
}

fn finish(diagnostics: Vec<ContractDiagnostic>) -> Result<(), Vec<ContractDiagnostic>> {
    if diagnostics.is_empty() {
        Ok(())
    } else {
        Err(diagnostics)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const DIGEST: &str = "0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef";
    fn request() -> SimulationRequest {
        SimulationRequest {
            schema_name: REQUEST_SCHEMA_NAME.to_owned(),
            schema_version: CONTRACT_SCHEMA_VERSION,
            design: "divider".to_owned(),
            backend: BackendIdentity {
                name: OHMNIVORE_BACKEND_NAME.to_owned(),
                version: OHMNIVORE_BACKEND_VERSION.to_owned(),
                contract: OHMNIVORE_BACKEND_CONTRACT.to_owned(),
                source_revision: OHMNIVORE_SOURCE_REVISION.to_owned(),
            },
            analysis: RequestAnalysis {
                path: "divider.simulation.op".to_owned(),
                kind: AnalysisKind::DcOperatingPoint,
                netlist_path: "simulation/op.spice".to_owned(),
                netlist_sha256: DIGEST.to_owned(),
                map_path: "simulation/op.spice-map.json".to_owned(),
            },
            assertions: vec![RequestAssertion {
                path: "divider.simulation.op.vout".to_owned(),
                signal_kind: SignalKind::NetVoltage,
                canonical_identity: "VOUT".to_owned(),
                sample: ReportSample {
                    kind: AxisKind::Scalar,
                    value: canonical_f64(0.0).unwrap(),
                },
                unit: ResultUnit::Volt,
                expected: canonical_f64(5.0).unwrap(),
                absolute_tolerance: canonical_f64(1e-6).unwrap(),
                relative_tolerance: canonical_f64(0.0).unwrap(),
            }],
        }
    }

    fn identity_map() -> SpiceIdentityMap {
        SpiceIdentityMap {
            schema_name: SPICE_MAP_SCHEMA_NAME.to_owned(),
            schema_version: CONTRACT_SCHEMA_VERSION,
            design: "divider".to_owned(),
            analysis_path: "divider.simulation.op".to_owned(),
            request_sha256: DIGEST.to_owned(),
            nets: vec![
                SpiceNetIdentity {
                    canonical: "GND".to_owned(),
                    backend: "0".to_owned(),
                    is_ground: true,
                },
                SpiceNetIdentity {
                    canonical: "VOUT".to_owned(),
                    backend: "VOUT".to_owned(),
                    is_ground: false,
                },
            ],
            devices: vec![SpiceDeviceIdentity {
                semantic_path: "divider.input".to_owned(),
                reference: "V1".to_owned(),
                backend: "V1".to_owned(),
            }],
        }
    }

    fn result() -> SimulationResult {
        SimulationResult {
            schema_name: RESULT_SCHEMA_NAME.to_owned(),
            schema_version: CONTRACT_SCHEMA_VERSION,
            design: "divider".to_owned(),
            analysis_path: "divider.simulation.op".to_owned(),
            analysis_kind: AnalysisKind::DcOperatingPoint,
            status: ExecutionStatus::Completed,
            request_sha256: DIGEST.to_owned(),
            map_sha256: DIGEST.to_owned(),
            axis: ResultAxis {
                kind: AxisKind::Scalar,
                samples: vec![canonical_f64(0.0).unwrap()],
            },
            signals: vec![ResultSignal {
                kind: SignalKind::NetVoltage,
                canonical_identity: "VOUT".to_owned(),
                unit: ResultUnit::Volt,
                values: vec![canonical_f64(5.0).unwrap()],
            }],
            diagnostics: Vec::new(),
        }
    }

    fn report() -> SimulationReport {
        SimulationReport {
            schema_name: REPORT_SCHEMA_NAME.to_owned(),
            schema_version: CONTRACT_SCHEMA_VERSION,
            design: "divider".to_owned(),
            analysis_path: "divider.simulation.op".to_owned(),
            analysis_kind: AnalysisKind::DcOperatingPoint,
            request_sha256: DIGEST.to_owned(),
            map_sha256: DIGEST.to_owned(),
            result_sha256: DIGEST.to_owned(),
            assertions: vec![AssertionOutcome {
                path: "divider.simulation.op.vout".to_owned(),
                status: AssertionStatus::Pass,
                signal_kind: SignalKind::NetVoltage,
                canonical_identity: "VOUT".to_owned(),
                sample: ReportSample {
                    kind: AxisKind::Scalar,
                    value: canonical_f64(0.0).unwrap(),
                },
                unit: ResultUnit::Volt,
                expected: canonical_f64(5.0).unwrap(),
                actual: RequiredNullable::some(canonical_f64(5.0).unwrap()),
                absolute_tolerance: canonical_f64(1e-6).unwrap(),
                relative_tolerance: canonical_f64(0.0).unwrap(),
                diagnostic: RequiredNullable::none(),
            }],
            summary: ReportSummary {
                pass: 1,
                fail: 0,
                unsupported: 0,
                unevaluated: 0,
            },
        }
    }

    #[test]
    fn request_is_strict_versioned_and_canonical() {
        let json = request().to_canonical_json().unwrap();
        assert!(json.ends_with('\n'));
        assert_eq!(parse_request(&json).unwrap(), request());
        let unknown = json.replacen(
            "  \"schema_name\"",
            "  \"unknown\": true,\n  \"schema_name\"",
            1,
        );
        assert_eq!(
            parse_request(&unknown).unwrap_err().code,
            "CC-SIM-CONTRACT-001"
        );
        let mut invalid = request();
        invalid.schema_version = 2;
        invalid.analysis.netlist_path = "../escape.spice".to_owned();
        let diagnostics = invalid.validate().unwrap_err();
        assert!(diagnostics.iter().any(|item| item.path == "schema_version"));
        assert!(
            diagnostics
                .iter()
                .any(|item| item.path == "analysis.netlist_path")
        );

        let minified = serde_json::to_string(&request()).unwrap();
        assert_eq!(
            parse_request(&minified).unwrap_err().code,
            "CC-SIM-CONTRACT-006"
        );
        assert_eq!(
            parse_request(json.trim_end()).unwrap_err().code,
            "CC-SIM-CONTRACT-006"
        );
        assert_eq!(
            parse_request(&" ".repeat(MAX_CONTRACT_BYTES + 1))
                .unwrap_err()
                .code,
            "CC-SIM-CONTRACT-006"
        );

        let backend_mutations: [fn(&mut BackendIdentity); 4] = [
            |backend: &mut BackendIdentity| backend.name = "other".to_owned(),
            |backend: &mut BackendIdentity| backend.version = "0.1.1".to_owned(),
            |backend: &mut BackendIdentity| backend.contract = "other/v1".to_owned(),
            |backend: &mut BackendIdentity| {
                backend.source_revision = "0000000000000000000000000000000000000000".to_owned()
            },
        ];
        for mutate in backend_mutations {
            let mut unsupported_backend = request();
            mutate(&mut unsupported_backend.backend);
            assert!(
                unsupported_backend
                    .validate()
                    .unwrap_err()
                    .iter()
                    .any(|item| item.code == "CC-SIM-CONTRACT-007")
            );
        }

        let mut oversized = request();
        oversized.design = "A".repeat(MAX_CONTRACT_BYTES);
        assert_eq!(
            oversized.to_canonical_json().unwrap_err().code,
            "CC-SIM-CONTRACT-006"
        );

        let mut nonzero_scalar = request();
        nonzero_scalar.assertions[0].sample.value = canonical_f64(1.0).unwrap();
        assert!(
            nonzero_scalar
                .validate()
                .unwrap_err()
                .iter()
                .any(|item| item.path == "assertions[0].sample.value")
        );

        for invalid_identity in ["BAD NET", "BAD\nNET", "µV"] {
            let mut invalid = request();
            invalid.assertions[0].canonical_identity = invalid_identity.to_owned();
            assert!(
                invalid
                    .validate()
                    .unwrap_err()
                    .iter()
                    .any(|item| item.path == "assertions[0].canonical_identity")
            );
        }
    }

    #[test]
    fn identity_map_rejects_order_collisions_and_reserved_ground_aliases() {
        let map = identity_map();
        let json = map.to_canonical_json().unwrap();
        assert_eq!(parse_spice_identity_map(&json).unwrap(), map);

        let mut invalid = identity_map();
        invalid.nets.reverse();
        invalid.nets[0].backend = "gnd".to_owned();
        invalid.devices.push(invalid.devices[0].clone());
        let diagnostics = invalid.validate().unwrap_err();
        assert!(diagnostics.iter().any(|item| item.path == "nets"));
        assert!(diagnostics.iter().any(|item| item.path.contains("backend")));
        assert!(diagnostics.iter().any(|item| item.path == "devices"));

        for invalid_identity in ["BAD NET", "BAD\nNET", "µV"] {
            let mut invalid = identity_map();
            invalid.nets[1].canonical = invalid_identity.to_owned();
            invalid.devices[0].reference = invalid_identity.to_owned();
            let diagnostics = invalid.validate().unwrap_err();
            assert!(
                diagnostics
                    .iter()
                    .any(|item| item.path == "nets[1].canonical")
            );
            assert!(
                diagnostics
                    .iter()
                    .any(|item| item.path == "devices[0].reference")
            );
        }
    }

    #[test]
    fn result_rejects_noncanonical_nonfinite_and_partial_data() {
        let valid_result = result();
        let json = valid_result.to_canonical_json().unwrap();
        assert_eq!(parse_result(&json).unwrap(), valid_result);

        let mut nonfinite = result();
        nonfinite.signals[0].values[0] = "NaN".to_owned();
        assert!(
            nonfinite
                .validate()
                .unwrap_err()
                .iter()
                .any(|item| item.code == "CC-SIM-CONTRACT-005")
        );

        let mut failed = result();
        failed.status = ExecutionStatus::Failed;
        failed.diagnostics.push(NormalizedDiagnostic {
            code: "CC-SIM-EXEC-001".to_owned(),
            message: "solver failed".to_owned(),
        });
        assert!(failed.validate().is_err());
        failed.axis.samples.clear();
        failed.signals.clear();
        assert!(failed.validate().is_ok());

        failed.diagnostics.push(failed.diagnostics[0].clone());
        assert!(
            failed
                .validate()
                .unwrap_err()
                .iter()
                .any(|item| item.path == "diagnostics")
        );

        failed.diagnostics.truncate(1);
        failed.diagnostics[0].code = "CC--SIM".to_owned();
        failed.diagnostics[0].message = " padded ".to_owned();
        let diagnostics = failed.validate().unwrap_err();
        assert!(
            diagnostics
                .iter()
                .any(|item| item.path == "diagnostics[0].code")
        );
        assert!(
            diagnostics
                .iter()
                .any(|item| item.path == "diagnostics[0].message")
        );

        let mut duplicate_signal = result();
        duplicate_signal
            .signals
            .push(duplicate_signal.signals[0].clone());
        assert!(
            duplicate_signal
                .validate()
                .unwrap_err()
                .iter()
                .any(|item| item.path == "signals")
        );

        let mut duplicate_sample = result();
        duplicate_sample.analysis_kind = AnalysisKind::Transient;
        duplicate_sample.axis.kind = AxisKind::TimeSeconds;
        duplicate_sample
            .axis
            .samples
            .push(canonical_f64(0.0).unwrap());
        duplicate_sample.signals[0]
            .values
            .push(canonical_f64(5.0).unwrap());
        assert!(
            duplicate_sample
                .validate()
                .unwrap_err()
                .iter()
                .any(|item| item.path == "axis.samples[1]")
        );

        let mut invalid_identity = result();
        invalid_identity.signals[0].canonical_identity = "BAD NET".to_owned();
        assert!(
            invalid_identity
                .validate()
                .unwrap_err()
                .iter()
                .any(|item| item.path == "signals[0].canonical_identity")
        );
    }

    #[test]
    fn report_status_summary_and_value_presence_are_explicit() {
        let valid_report = report();
        let json = valid_report.to_canonical_json().unwrap();
        assert_eq!(parse_report(&json).unwrap(), valid_report);

        let mut invalid = report();
        invalid.assertions[0].status = AssertionStatus::Unevaluated;
        invalid.assertions[0].sample.kind = AxisKind::TimeSeconds;
        invalid.assertions[0].absolute_tolerance = canonical_f64(-1.0).unwrap();
        let diagnostics = invalid.validate().unwrap_err();
        assert!(diagnostics.iter().any(|item| item.path == "summary"));
        assert!(diagnostics.iter().any(|item| item.path == "assertions[0]"));
        assert!(
            diagnostics
                .iter()
                .any(|item| item.path == "assertions[0].sample.kind")
        );
        assert!(
            diagnostics
                .iter()
                .any(|item| item.path == "assertions[0].absolute_tolerance")
        );

        let mut contradiction = report();
        contradiction.assertions[0].actual = RequiredNullable::some(canonical_f64(7.0).unwrap());
        assert!(
            contradiction
                .validate()
                .unwrap_err()
                .iter()
                .any(|item| item.path == "assertions[0].status")
        );

        let mut inclusive_boundary = report();
        inclusive_boundary.assertions[0].actual =
            RequiredNullable::some(canonical_f64(6.0).unwrap());
        inclusive_boundary.assertions[0].absolute_tolerance = canonical_f64(1.0).unwrap();
        assert!(inclusive_boundary.validate().is_ok());

        let mut relative_boundary = report();
        relative_boundary.assertions[0].expected = canonical_f64(4.0).unwrap();
        relative_boundary.assertions[0].actual =
            RequiredNullable::some(canonical_f64(7.0).unwrap());
        relative_boundary.assertions[0].absolute_tolerance = canonical_f64(1.0).unwrap();
        relative_boundary.assertions[0].relative_tolerance = canonical_f64(0.5).unwrap();
        assert!(
            relative_boundary.validate().is_ok(),
            "difference 3 must pass at 1 absolute + 0.5 * abs(4) relative"
        );

        let mut relative_beyond = relative_boundary;
        relative_beyond.assertions[0].actual = RequiredNullable::some(canonical_f64(8.0).unwrap());
        relative_beyond.assertions[0].status = AssertionStatus::Fail;
        relative_beyond.summary.pass = 0;
        relative_beyond.summary.fail = 1;
        assert!(
            relative_beyond.validate().is_ok(),
            "difference 4 must fail beyond 1 absolute + 0.5 * abs(4) relative"
        );

        let without_actual = json
            .lines()
            .filter(|line| !line.contains("\"actual\":"))
            .collect::<Vec<_>>()
            .join("\n")
            + "\n";
        assert_eq!(
            parse_report(&without_actual).unwrap_err().code,
            "CC-SIM-CONTRACT-001"
        );
        let without_diagnostic = json
            .lines()
            .filter(|line| !line.contains("\"diagnostic\":"))
            .collect::<Vec<_>>()
            .join("\n")
            + "\n";
        assert_eq!(
            parse_report(&without_diagnostic).unwrap_err().code,
            "CC-SIM-CONTRACT-001"
        );

        let mut all_statuses = report();
        let base = all_statuses.assertions[0].clone();
        let mut failed = base.clone();
        failed.path = "divider.simulation.op.z_fail".to_owned();
        failed.status = AssertionStatus::Fail;
        failed.expected = canonical_f64(0.0).unwrap();
        let mut unevaluated = base.clone();
        unevaluated.path = "divider.simulation.op.z_unevaluated".to_owned();
        unevaluated.status = AssertionStatus::Unevaluated;
        unevaluated.actual = RequiredNullable::none();
        unevaluated.diagnostic = RequiredNullable::some(NormalizedDiagnostic {
            code: "CC-SIM-EVAL-001".to_owned(),
            message: "sample unavailable".to_owned(),
        });
        let mut unsupported = unevaluated.clone();
        unsupported.path = "divider.simulation.op.z_unsupported".to_owned();
        unsupported.status = AssertionStatus::Unsupported;
        all_statuses.assertions = vec![base, failed, unevaluated, unsupported];
        all_statuses
            .assertions
            .sort_by(|left, right| left.path.cmp(&right.path));
        all_statuses.summary = ReportSummary {
            pass: 1,
            fail: 1,
            unsupported: 1,
            unevaluated: 1,
        };
        assert!(all_statuses.validate().is_ok());
    }

    #[test]
    fn digest_chain_verifies_exact_canonical_predecessor_bytes() {
        let netlist =
            b"CircuitC divider\nV1 VIN 0 10\nR1 VIN VOUT 1000\nR2 VOUT 0 1000\n.op\n.end\n";
        let mut bound_request = request();
        bound_request.analysis.netlist_sha256 = sha256_hex(netlist);
        let request_json = bound_request.to_canonical_json().unwrap();

        let mut bound_map = identity_map();
        bound_map.request_sha256 = sha256_hex(request_json.as_bytes());
        let map_json = bound_map.to_canonical_json().unwrap();

        let mut bound_result = result();
        bound_result.request_sha256 = sha256_hex(request_json.as_bytes());
        bound_result.map_sha256 = sha256_hex(map_json.as_bytes());
        let result_json = bound_result.to_canonical_json().unwrap();

        let mut bound_report = report();
        bound_report.request_sha256 = sha256_hex(request_json.as_bytes());
        bound_report.map_sha256 = sha256_hex(map_json.as_bytes());
        bound_report.result_sha256 = sha256_hex(result_json.as_bytes());

        bound_request.verify_netlist_bytes(netlist).unwrap();
        bound_map
            .verify_request_bytes(request_json.as_bytes())
            .unwrap();
        bound_result
            .verify_binding_bytes(request_json.as_bytes(), map_json.as_bytes())
            .unwrap();
        bound_report
            .verify_binding_bytes(
                request_json.as_bytes(),
                map_json.as_bytes(),
                result_json.as_bytes(),
            )
            .unwrap();

        assert_eq!(
            bound_request
                .verify_netlist_bytes(b"stale")
                .unwrap_err()
                .code,
            "CC-SIM-CONTRACT-004"
        );

        let mut stale_request = bound_request.clone();
        stale_request.design = "other".to_owned();
        let stale_request_json = stale_request.to_canonical_json().unwrap();
        assert_eq!(
            bound_map
                .verify_request_bytes(stale_request_json.as_bytes())
                .unwrap_err()
                .code,
            "CC-SIM-CONTRACT-004"
        );

        let request_metadata_mutations: [fn(&mut SimulationRequest); 2] = [
            |request: &mut SimulationRequest| request.design = "other".to_owned(),
            |request: &mut SimulationRequest| request.analysis.path = "other.analysis".to_owned(),
        ];
        for mutate in request_metadata_mutations {
            let mut mismatched_request = bound_request.clone();
            mutate(&mut mismatched_request);
            let mismatched_request_json = mismatched_request.to_canonical_json().unwrap();
            let mut map_bound_to_mismatched_request = bound_map.clone();
            map_bound_to_mismatched_request.request_sha256 =
                sha256_hex(mismatched_request_json.as_bytes());
            assert_eq!(
                map_bound_to_mismatched_request
                    .verify_request_bytes(mismatched_request_json.as_bytes())
                    .unwrap_err()
                    .path,
                "request"
            );
        }

        let mut stale_map = bound_map.clone();
        stale_map.devices[0].backend = "V2".to_owned();
        let stale_map_json = stale_map.to_canonical_json().unwrap();
        assert_eq!(
            bound_result
                .verify_binding_bytes(request_json.as_bytes(), stale_map_json.as_bytes())
                .unwrap_err()
                .code,
            "CC-SIM-CONTRACT-004"
        );

        let mut stale_result = bound_result.clone();
        stale_result.signals[0].values[0] = canonical_f64(6.0).unwrap();
        let stale_result_json = stale_result.to_canonical_json().unwrap();
        assert_eq!(
            bound_report
                .verify_binding_bytes(
                    request_json.as_bytes(),
                    map_json.as_bytes(),
                    stale_result_json.as_bytes(),
                )
                .unwrap_err()
                .code,
            "CC-SIM-CONTRACT-004"
        );

        let map_metadata_mutations: [fn(&mut SpiceIdentityMap); 2] = [
            |map: &mut SpiceIdentityMap| map.design = "other".to_owned(),
            |map: &mut SpiceIdentityMap| map.analysis_path = "other.analysis".to_owned(),
        ];
        for mutate in map_metadata_mutations {
            let mut mismatched_map = bound_map.clone();
            mutate(&mut mismatched_map);
            let mismatched_map_json = mismatched_map.to_canonical_json().unwrap();
            let mut result_bound_to_mismatched_map = bound_result.clone();
            result_bound_to_mismatched_map.map_sha256 = sha256_hex(mismatched_map_json.as_bytes());
            assert_eq!(
                result_bound_to_mismatched_map
                    .verify_binding_bytes(request_json.as_bytes(), mismatched_map_json.as_bytes())
                    .unwrap_err()
                    .path,
                "request"
            );
        }

        let result_metadata_mutations: [fn(&mut SimulationResult); 2] = [
            |result: &mut SimulationResult| result.design = "other".to_owned(),
            |result: &mut SimulationResult| result.analysis_path = "other.analysis".to_owned(),
        ];
        for mutate in result_metadata_mutations {
            let mut mismatched_result = bound_result.clone();
            mutate(&mut mismatched_result);
            assert_eq!(
                mismatched_result
                    .verify_binding_bytes(request_json.as_bytes(), map_json.as_bytes())
                    .unwrap_err()
                    .path,
                "result"
            );
        }
        let mut mismatched_kind = bound_result.clone();
        mismatched_kind.analysis_kind = AnalysisKind::AcLinearSweep;
        mismatched_kind.axis.kind = AxisKind::FrequencyHertz;
        mismatched_kind.axis.samples[0] = canonical_f64(1.0).unwrap();
        assert_eq!(
            mismatched_kind
                .verify_binding_bytes(request_json.as_bytes(), map_json.as_bytes())
                .unwrap_err()
                .path,
            "result"
        );

        let mut unknown_signal = bound_result.clone();
        unknown_signal.signals[0].canonical_identity = "UNKNOWN".to_owned();
        assert_eq!(
            unknown_signal
                .verify_binding_bytes(request_json.as_bytes(), map_json.as_bytes())
                .unwrap_err()
                .path,
            "signals[0].canonical_identity"
        );
        let mut wrong_namespace = bound_result.clone();
        wrong_namespace.signals[0].kind = SignalKind::BranchCurrent;
        wrong_namespace.signals[0].unit = ResultUnit::Ampere;
        assert_eq!(
            wrong_namespace
                .verify_binding_bytes(request_json.as_bytes(), map_json.as_bytes())
                .unwrap_err()
                .path,
            "signals[0].canonical_identity"
        );

        let mut request_with_uncovered_assertion = bound_request.clone();
        let mut ground_assertion = request_with_uncovered_assertion.assertions[0].clone();
        ground_assertion.path = "divider.simulation.op.z_ground".to_owned();
        ground_assertion.canonical_identity = "GND".to_owned();
        ground_assertion.expected = canonical_f64(0.0).unwrap();
        request_with_uncovered_assertion
            .assertions
            .push(ground_assertion);
        let uncovered_request_json = request_with_uncovered_assertion
            .to_canonical_json()
            .unwrap();
        let mut map_for_uncovered_request = bound_map.clone();
        map_for_uncovered_request.request_sha256 = sha256_hex(uncovered_request_json.as_bytes());
        let uncovered_map_json = map_for_uncovered_request.to_canonical_json().unwrap();
        let mut uncovered_result = bound_result.clone();
        uncovered_result.request_sha256 = sha256_hex(uncovered_request_json.as_bytes());
        uncovered_result.map_sha256 = sha256_hex(uncovered_map_json.as_bytes());
        assert_eq!(
            uncovered_result
                .verify_binding_bytes(
                    uncovered_request_json.as_bytes(),
                    uncovered_map_json.as_bytes(),
                )
                .unwrap_err()
                .path,
            "request.assertions[1]"
        );

        let mut missing_outcome = bound_report.clone();
        missing_outcome.assertions.clear();
        missing_outcome.summary = ReportSummary {
            pass: 0,
            fail: 0,
            unsupported: 0,
            unevaluated: 0,
        };
        assert!(missing_outcome.validate().is_ok());
        assert_eq!(
            missing_outcome
                .verify_binding_bytes(
                    request_json.as_bytes(),
                    map_json.as_bytes(),
                    result_json.as_bytes(),
                )
                .unwrap_err()
                .path,
            "assertions"
        );

        let mut rewritten_path = bound_report.clone();
        rewritten_path.assertions[0].path = "divider.simulation.op.other".to_owned();
        assert_eq!(
            rewritten_path
                .verify_binding_bytes(
                    request_json.as_bytes(),
                    map_json.as_bytes(),
                    result_json.as_bytes(),
                )
                .unwrap_err()
                .path,
            "assertions[0]"
        );

        let mut rewritten_expected = bound_report.clone();
        rewritten_expected.assertions[0].status = AssertionStatus::Fail;
        rewritten_expected.assertions[0].expected = canonical_f64(6.0).unwrap();
        rewritten_expected.summary.pass = 0;
        rewritten_expected.summary.fail = 1;
        assert!(rewritten_expected.validate().is_ok());
        assert_eq!(
            rewritten_expected
                .verify_binding_bytes(
                    request_json.as_bytes(),
                    map_json.as_bytes(),
                    result_json.as_bytes(),
                )
                .unwrap_err()
                .path,
            "assertions[0]"
        );

        let tolerance_mutations: [fn(&mut AssertionOutcome); 2] = [
            |outcome: &mut AssertionOutcome| {
                outcome.absolute_tolerance = canonical_f64(1.0).unwrap()
            },
            |outcome: &mut AssertionOutcome| {
                outcome.relative_tolerance = canonical_f64(1.0).unwrap()
            },
        ];
        for mutate in tolerance_mutations {
            let mut rewritten_tolerance = bound_report.clone();
            mutate(&mut rewritten_tolerance.assertions[0]);
            assert!(rewritten_tolerance.validate().is_ok());
            assert_eq!(
                rewritten_tolerance
                    .verify_binding_bytes(
                        request_json.as_bytes(),
                        map_json.as_bytes(),
                        result_json.as_bytes(),
                    )
                    .unwrap_err()
                    .path,
                "assertions[0]"
            );
        }

        let mut mismatched_actual = bound_report.clone();
        mismatched_actual.assertions[0].status = AssertionStatus::Fail;
        mismatched_actual.assertions[0].actual =
            RequiredNullable::some(canonical_f64(6.0).unwrap());
        mismatched_actual.summary.pass = 0;
        mismatched_actual.summary.fail = 1;
        assert!(mismatched_actual.validate().is_ok());
        assert_eq!(
            mismatched_actual
                .verify_binding_bytes(
                    request_json.as_bytes(),
                    map_json.as_bytes(),
                    result_json.as_bytes(),
                )
                .unwrap_err()
                .path,
            "assertions[0].actual"
        );

        let report_metadata_mutations: [fn(&mut SimulationReport); 2] = [
            |report: &mut SimulationReport| report.design = "other".to_owned(),
            |report: &mut SimulationReport| report.analysis_path = "other.analysis".to_owned(),
        ];
        for mutate in report_metadata_mutations {
            let mut mismatched_report = bound_report.clone();
            mutate(&mut mismatched_report);
            assert_eq!(
                mismatched_report
                    .verify_binding_bytes(
                        request_json.as_bytes(),
                        map_json.as_bytes(),
                        result_json.as_bytes(),
                    )
                    .unwrap_err()
                    .path,
                "report"
            );
        }
        let mut mismatched_report_kind = bound_report.clone();
        mismatched_report_kind.analysis_kind = AnalysisKind::AcLinearSweep;
        mismatched_report_kind.assertions[0].sample.kind = AxisKind::FrequencyHertz;
        mismatched_report_kind.assertions[0].sample.value = canonical_f64(1.0).unwrap();
        assert_eq!(
            mismatched_report_kind
                .verify_binding_bytes(
                    request_json.as_bytes(),
                    map_json.as_bytes(),
                    result_json.as_bytes(),
                )
                .unwrap_err()
                .path,
            "report"
        );

        let mut failed_result = bound_result.clone();
        failed_result.status = ExecutionStatus::Failed;
        failed_result.axis.samples.clear();
        failed_result.signals.clear();
        failed_result.diagnostics = vec![NormalizedDiagnostic {
            code: "CC-SIM-EXEC-001".to_owned(),
            message: "solver failed".to_owned(),
        }];
        let failed_result_json = failed_result.to_canonical_json().unwrap();
        let mut unevaluated_report = bound_report.clone();
        unevaluated_report.result_sha256 = sha256_hex(failed_result_json.as_bytes());
        unevaluated_report.assertions[0].status = AssertionStatus::Unevaluated;
        unevaluated_report.assertions[0].actual = RequiredNullable::none();
        unevaluated_report.assertions[0].diagnostic =
            RequiredNullable::some(NormalizedDiagnostic {
                code: "CC-SIM-EVAL-001".to_owned(),
                message: "solver failed before assertion evaluation".to_owned(),
            });
        unevaluated_report.summary.pass = 0;
        unevaluated_report.summary.unevaluated = 1;
        unevaluated_report
            .verify_binding_bytes(
                request_json.as_bytes(),
                map_json.as_bytes(),
                failed_result_json.as_bytes(),
            )
            .unwrap();

        let mut unsupported_result = failed_result.clone();
        unsupported_result.status = ExecutionStatus::Unsupported;
        unsupported_result.diagnostics[0] = NormalizedDiagnostic {
            code: "CC-SIM-CAP-001".to_owned(),
            message: "analysis is unsupported".to_owned(),
        };
        let unsupported_result_json = unsupported_result.to_canonical_json().unwrap();
        let mut unsupported_report = bound_report.clone();
        unsupported_report.result_sha256 = sha256_hex(unsupported_result_json.as_bytes());
        unsupported_report.assertions[0].status = AssertionStatus::Unsupported;
        unsupported_report.assertions[0].actual = RequiredNullable::none();
        unsupported_report.assertions[0].diagnostic =
            RequiredNullable::some(NormalizedDiagnostic {
                code: "CC-SIM-CAP-001".to_owned(),
                message: "analysis is unsupported".to_owned(),
            });
        unsupported_report.summary.pass = 0;
        unsupported_report.summary.unsupported = 1;
        unsupported_report
            .verify_binding_bytes(
                request_json.as_bytes(),
                map_json.as_bytes(),
                unsupported_result_json.as_bytes(),
            )
            .unwrap();

        let mut pass_from_failed_result = bound_report.clone();
        pass_from_failed_result.result_sha256 = sha256_hex(failed_result_json.as_bytes());
        assert_eq!(
            pass_from_failed_result
                .verify_binding_bytes(
                    request_json.as_bytes(),
                    map_json.as_bytes(),
                    failed_result_json.as_bytes(),
                )
                .unwrap_err()
                .path,
            "assertions[0].status"
        );
    }

    #[test]
    fn ac_binding_uses_exact_authenticated_signal_identity_unit_and_sample() {
        let netlist = b"CircuitC divider AC\nV1 VIN 0 DC 10 AC 1 0\nR1 VIN VOUT 1000\nR2 VOUT 0 1000\n.ac lin 2 10 20\n.end\n";
        let mut ac_request = request();
        ac_request.analysis.path = "divider.simulation.ac".to_owned();
        ac_request.analysis.kind = AnalysisKind::AcLinearSweep;
        ac_request.analysis.netlist_path = "simulation/ac.spice".to_owned();
        ac_request.analysis.netlist_sha256 = sha256_hex(netlist);
        ac_request.analysis.map_path = "simulation/ac.spice-map.json".to_owned();
        ac_request.assertions[0].path = "divider.simulation.ac.vout".to_owned();
        ac_request.assertions[0].signal_kind = SignalKind::NetVoltageMagnitude;
        ac_request.assertions[0].sample.kind = AxisKind::FrequencyHertz;
        ac_request.assertions[0].sample.value = canonical_f64(10.0).unwrap();
        let request_json = ac_request.to_canonical_json().unwrap();

        let mut ac_map = identity_map();
        ac_map.analysis_path = "divider.simulation.ac".to_owned();
        ac_map.request_sha256 = sha256_hex(request_json.as_bytes());
        let map_json = ac_map.to_canonical_json().unwrap();

        let mut ac_result = result();
        ac_result.analysis_path = "divider.simulation.ac".to_owned();
        ac_result.analysis_kind = AnalysisKind::AcLinearSweep;
        ac_result.request_sha256 = sha256_hex(request_json.as_bytes());
        ac_result.map_sha256 = sha256_hex(map_json.as_bytes());
        ac_result.axis = ResultAxis {
            kind: AxisKind::FrequencyHertz,
            samples: vec![canonical_f64(10.0).unwrap(), canonical_f64(20.0).unwrap()],
        };
        ac_result.signals = vec![
            ResultSignal {
                kind: SignalKind::NetVoltage,
                canonical_identity: "VOUT".to_owned(),
                unit: ResultUnit::Volt,
                values: vec![canonical_f64(4.0).unwrap(), canonical_f64(5.0).unwrap()],
            },
            ResultSignal {
                kind: SignalKind::NetVoltageMagnitude,
                canonical_identity: "GND".to_owned(),
                unit: ResultUnit::Volt,
                values: vec![canonical_f64(0.0).unwrap(), canonical_f64(0.0).unwrap()],
            },
            ResultSignal {
                kind: SignalKind::NetVoltageMagnitude,
                canonical_identity: "VOUT".to_owned(),
                unit: ResultUnit::Volt,
                values: vec![canonical_f64(5.0).unwrap(), canonical_f64(6.0).unwrap()],
            },
        ];
        let result_json = ac_result.to_canonical_json().unwrap();

        let mut ac_report = report();
        ac_report.analysis_path = "divider.simulation.ac".to_owned();
        ac_report.analysis_kind = AnalysisKind::AcLinearSweep;
        ac_report.request_sha256 = sha256_hex(request_json.as_bytes());
        ac_report.map_sha256 = sha256_hex(map_json.as_bytes());
        ac_report.result_sha256 = sha256_hex(result_json.as_bytes());
        ac_report.assertions[0].path = "divider.simulation.ac.vout".to_owned();
        ac_report.assertions[0].signal_kind = SignalKind::NetVoltageMagnitude;
        ac_report.assertions[0].sample.kind = AxisKind::FrequencyHertz;
        ac_report.assertions[0].sample.value = canonical_f64(10.0).unwrap();
        ac_report
            .verify_binding_bytes(
                request_json.as_bytes(),
                map_json.as_bytes(),
                result_json.as_bytes(),
            )
            .unwrap();

        let intent_mutations: [fn(&mut AssertionOutcome); 3] = [
            |outcome| {
                outcome.signal_kind = SignalKind::NetVoltage;
                outcome.actual = RequiredNullable::some(canonical_f64(4.0).unwrap());
                outcome.status = AssertionStatus::Fail;
            },
            |outcome| {
                outcome.canonical_identity = "GND".to_owned();
                outcome.actual = RequiredNullable::some(canonical_f64(0.0).unwrap());
                outcome.status = AssertionStatus::Fail;
            },
            |outcome| {
                outcome.sample.value = canonical_f64(20.0).unwrap();
                outcome.actual = RequiredNullable::some(canonical_f64(6.0).unwrap());
                outcome.status = AssertionStatus::Fail;
            },
        ];
        for mutate in intent_mutations {
            let mut rewritten = ac_report.clone();
            mutate(&mut rewritten.assertions[0]);
            rewritten.summary.pass = 0;
            rewritten.summary.fail = 1;
            assert!(rewritten.validate().is_ok());
            assert_eq!(
                rewritten
                    .verify_binding_bytes(
                        request_json.as_bytes(),
                        map_json.as_bytes(),
                        result_json.as_bytes(),
                    )
                    .unwrap_err()
                    .path,
                "assertions[0]"
            );
        }

        let mut invalid_unit = ac_report.clone();
        invalid_unit.assertions[0].unit = ResultUnit::Ampere;
        assert!(
            invalid_unit
                .validate()
                .unwrap_err()
                .iter()
                .any(|diagnostic| diagnostic.path == "assertions[0].unit")
        );

        let mut second_sample_request = ac_request.clone();
        second_sample_request.assertions[0].sample.value = canonical_f64(20.0).unwrap();
        let second_request_json = second_sample_request.to_canonical_json().unwrap();
        let mut second_map = ac_map.clone();
        second_map.request_sha256 = sha256_hex(second_request_json.as_bytes());
        let second_map_json = second_map.to_canonical_json().unwrap();
        let mut second_result = ac_result.clone();
        second_result.request_sha256 = sha256_hex(second_request_json.as_bytes());
        second_result.map_sha256 = sha256_hex(second_map_json.as_bytes());
        let second_result_json = second_result.to_canonical_json().unwrap();
        let mut second_report = ac_report.clone();
        second_report.request_sha256 = sha256_hex(second_request_json.as_bytes());
        second_report.map_sha256 = sha256_hex(second_map_json.as_bytes());
        second_report.result_sha256 = sha256_hex(second_result_json.as_bytes());
        second_report.assertions[0].sample.value = canonical_f64(20.0).unwrap();
        second_report.assertions[0].actual = RequiredNullable::some(canonical_f64(6.0).unwrap());
        second_report.assertions[0].status = AssertionStatus::Fail;
        second_report.summary.pass = 0;
        second_report.summary.fail = 1;
        second_report
            .verify_binding_bytes(
                second_request_json.as_bytes(),
                second_map_json.as_bytes(),
                second_result_json.as_bytes(),
            )
            .unwrap();

        let mut missing_sample_request = ac_request.clone();
        missing_sample_request.assertions[0].sample.value = canonical_f64(15.0).unwrap();
        let missing_request_json = missing_sample_request.to_canonical_json().unwrap();
        let mut missing_map = ac_map.clone();
        missing_map.request_sha256 = sha256_hex(missing_request_json.as_bytes());
        let missing_map_json = missing_map.to_canonical_json().unwrap();
        let mut missing_result = ac_result.clone();
        missing_result.request_sha256 = sha256_hex(missing_request_json.as_bytes());
        missing_result.map_sha256 = sha256_hex(missing_map_json.as_bytes());
        assert_eq!(
            missing_result
                .verify_binding_bytes(missing_request_json.as_bytes(), missing_map_json.as_bytes(),)
                .unwrap_err()
                .path,
            "request.assertions[0]"
        );

        let mut concealed = ac_report.clone();
        concealed.assertions[0].status = AssertionStatus::Unevaluated;
        concealed.assertions[0].actual = RequiredNullable::none();
        concealed.assertions[0].diagnostic = RequiredNullable::some(NormalizedDiagnostic {
            code: "CC-SIM-EVAL-001".to_owned(),
            message: "sample concealed".to_owned(),
        });
        concealed.summary.pass = 0;
        concealed.summary.unevaluated = 1;
        assert_eq!(
            concealed
                .verify_binding_bytes(
                    request_json.as_bytes(),
                    map_json.as_bytes(),
                    result_json.as_bytes(),
                )
                .unwrap_err()
                .path,
            "assertions[0].status"
        );
    }

    #[test]
    fn digests_and_numbers_have_one_canonical_encoding() {
        assert_eq!(
            sha256_hex(b"abc"),
            "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad"
        );
        assert_eq!(canonical_f64(-0.0).unwrap(), canonical_f64(0.0).unwrap());
        assert!(canonical_f64(f64::INFINITY).is_err());
        let json = request().to_canonical_json().unwrap();
        assert_ne!(
            sha256_hex(json.as_bytes()),
            sha256_hex(json.trim_end().as_bytes())
        );

        let mut diagnostics = Vec::new();
        validate_entry_count(MAX_CONTRACT_ENTRIES + 1, "entries", &mut diagnostics);
        assert_eq!(diagnostics[0].code, "CC-SIM-CONTRACT-006");

        let mut capped = Vec::new();
        for _ in 0..(MAX_VALIDATION_DIAGNOSTICS + 10) {
            push(&mut capped, "CC-SIM-CONTRACT-002", "field", "invalid");
        }
        assert_eq!(capped.len(), MAX_VALIDATION_DIAGNOSTICS);
    }

    #[test]
    fn every_public_collection_enforces_the_structural_entry_cap() {
        let assert_limit = |diagnostics: Vec<ContractDiagnostic>, path: &str| {
            assert!(diagnostics.iter().any(|diagnostic| {
                diagnostic.code == "CC-SIM-CONTRACT-006" && diagnostic.path == path
            }));
        };
        let over_limit = MAX_CONTRACT_ENTRIES + 1;

        {
            let mut value = request();
            value.assertions = vec![value.assertions[0].clone(); over_limit];
            assert_limit(value.validate().unwrap_err(), "assertions");
        }
        {
            let mut value = identity_map();
            value.nets = vec![value.nets[0].clone(); over_limit];
            assert_limit(value.validate().unwrap_err(), "nets");
        }
        {
            let mut value = identity_map();
            value.devices = vec![value.devices[0].clone(); over_limit];
            assert_limit(value.validate().unwrap_err(), "devices");
        }
        {
            let mut value = result();
            value.axis.samples = vec![canonical_f64(0.0).unwrap(); over_limit];
            assert_limit(value.validate().unwrap_err(), "axis.samples");
        }
        {
            let mut value = result();
            value.signals = vec![value.signals[0].clone(); over_limit];
            assert_limit(value.validate().unwrap_err(), "signals");
        }
        {
            let mut value = result();
            value.status = ExecutionStatus::Failed;
            value.axis.samples.clear();
            value.signals.clear();
            value.diagnostics = vec![
                NormalizedDiagnostic {
                    code: "CC-SIM-EXEC-001".to_owned(),
                    message: "solver failed".to_owned(),
                };
                over_limit
            ];
            assert_limit(value.validate().unwrap_err(), "diagnostics");
        }
        {
            let mut value = result();
            value.signals[0].values = vec![canonical_f64(5.0).unwrap(); over_limit];
            assert_limit(value.validate().unwrap_err(), "signals[0].values");
        }
        {
            let mut value = report();
            value.assertions = vec![value.assertions[0].clone(); over_limit];
            assert_limit(value.validate().unwrap_err(), "assertions");
        }
    }

    #[test]
    fn contract_json_has_exact_golden_bytes() {
        let request_golden = r#"{
  "schema_name": "circuitc.simulation_request",
  "schema_version": 1,
  "design": "divider",
  "backend": {
    "name": "ohmnivore",
    "version": "0.1.0",
    "contract": "ohmnivore-cli-csv/v1",
    "source_revision": "c2189a651d4879211019e109b2136dee836a5c5d"
  },
  "analysis": {
    "path": "divider.simulation.op",
    "kind": "dc_operating_point",
    "netlist_path": "simulation/op.spice",
    "netlist_sha256": "0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef",
    "map_path": "simulation/op.spice-map.json"
  },
  "assertions": [
    {
      "path": "divider.simulation.op.vout",
      "signal_kind": "net_voltage",
      "canonical_identity": "VOUT",
      "sample": {
        "kind": "scalar",
        "value": "0.00000000000000000e0"
      },
      "unit": "volt",
      "expected": "5.00000000000000000e0",
      "absolute_tolerance": "9.99999999999999955e-7",
      "relative_tolerance": "0.00000000000000000e0"
    }
  ]
}
"#;
        let map_golden = r#"{
  "schema_name": "circuitc.spice_identity_map",
  "schema_version": 1,
  "design": "divider",
  "analysis_path": "divider.simulation.op",
  "request_sha256": "0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef",
  "nets": [
    {
      "canonical": "GND",
      "backend": "0",
      "is_ground": true
    },
    {
      "canonical": "VOUT",
      "backend": "VOUT",
      "is_ground": false
    }
  ],
  "devices": [
    {
      "semantic_path": "divider.input",
      "reference": "V1",
      "backend": "V1"
    }
  ]
}
"#;
        let result_golden = r#"{
  "schema_name": "circuitc.simulation_result",
  "schema_version": 1,
  "design": "divider",
  "analysis_path": "divider.simulation.op",
  "analysis_kind": "dc_operating_point",
  "status": "completed",
  "request_sha256": "0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef",
  "map_sha256": "0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef",
  "axis": {
    "kind": "scalar",
    "samples": [
      "0.00000000000000000e0"
    ]
  },
  "signals": [
    {
      "kind": "net_voltage",
      "canonical_identity": "VOUT",
      "unit": "volt",
      "values": [
        "5.00000000000000000e0"
      ]
    }
  ],
  "diagnostics": []
}
"#;
        let report_golden = r#"{
  "schema_name": "circuitc.simulation_report",
  "schema_version": 1,
  "design": "divider",
  "analysis_path": "divider.simulation.op",
  "analysis_kind": "dc_operating_point",
  "request_sha256": "0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef",
  "map_sha256": "0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef",
  "result_sha256": "0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef",
  "assertions": [
    {
      "path": "divider.simulation.op.vout",
      "status": "pass",
      "signal_kind": "net_voltage",
      "canonical_identity": "VOUT",
      "sample": {
        "kind": "scalar",
        "value": "0.00000000000000000e0"
      },
      "unit": "volt",
      "expected": "5.00000000000000000e0",
      "actual": "5.00000000000000000e0",
      "absolute_tolerance": "9.99999999999999955e-7",
      "relative_tolerance": "0.00000000000000000e0",
      "diagnostic": null
    }
  ],
  "summary": {
    "pass": 1,
    "fail": 0,
    "unsupported": 0,
    "unevaluated": 0
  }
}
"#;

        assert_eq!(request().to_canonical_json().unwrap(), request_golden);
        assert_eq!(identity_map().to_canonical_json().unwrap(), map_golden);
        assert_eq!(result().to_canonical_json().unwrap(), result_golden);
        assert_eq!(report().to_canonical_json().unwrap(), report_golden);
    }
}
