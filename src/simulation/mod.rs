//! Versioned simulation process contracts.

pub mod contract;
pub(crate) mod lower;
pub mod runner;

pub use contract::{
    AnalysisKind, AssertionOutcome, AssertionStatus, AxisKind, BackendIdentity,
    CONTRACT_SCHEMA_VERSION, ContractDiagnostic, ExecutionStatus, MAX_CONTRACT_BYTES,
    MAX_CONTRACT_ENTRIES, MAX_VALIDATION_DIAGNOSTICS, NormalizedDiagnostic,
    OHMNIVORE_BACKEND_CONTRACT, OHMNIVORE_BACKEND_NAME, OHMNIVORE_BACKEND_VERSION,
    OHMNIVORE_SOURCE_REVISION, REPORT_SCHEMA_NAME, REQUEST_SCHEMA_NAME, RESULT_SCHEMA_NAME,
    ReportSample, ReportSummary, RequestAnalysis, RequestAssertion, RequiredNullable, ResultAxis,
    ResultSignal, ResultUnit, SPICE_MAP_SCHEMA_NAME, SignalKind, SimulationReport,
    SimulationRequest, SimulationResult, SpiceDeviceIdentity, SpiceIdentityMap, SpiceNetIdentity,
    canonical_f64, parse_report, parse_request, parse_result, parse_spice_identity_map, sha256_hex,
};
pub use runner::OhmnivoreRunner;
