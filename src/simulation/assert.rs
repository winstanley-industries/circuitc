use super::contract::{
    AssertionOutcome, AssertionStatus, CONTRACT_SCHEMA_VERSION, ContractDiagnostic,
    ExecutionStatus, REPORT_SCHEMA_NAME, ReportSummary, RequiredNullable, ResultIndex,
    SimulationReport, compare_assertion_values, parse_result, sha256_hex,
};

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct AssertionEvaluation {
    pub(crate) report: SimulationReport,
    pub(crate) report_json: String,
    pub(crate) checked_success: bool,
}

pub(crate) fn evaluate_assertions(
    request_json: &[u8],
    map_json: &[u8],
    result_json: &[u8],
) -> Result<AssertionEvaluation, ContractDiagnostic> {
    let result_text = std::str::from_utf8(result_json).map_err(|error| ContractDiagnostic {
        code: "CC-SIM-CONTRACT-001",
        path: "document".to_owned(),
        message: format!("simulation contract is not UTF-8: {error}"),
    })?;
    let result = parse_result(result_text)?;

    // Authenticate the complete canonical request -> map -> result chain before
    // using any result value in assertion arithmetic.
    let (request, _) = result.verify_binding_bytes(request_json, map_json)?;
    let result_index = ResultIndex::new(&result);
    let execution_diagnostic = result.diagnostics.first().cloned();
    let mut summary = ReportSummary {
        pass: 0,
        fail: 0,
        unsupported: 0,
        unevaluated: 0,
    };
    let mut outcomes = Vec::with_capacity(request.assertions.len());

    for (index, intent) in request.assertions.iter().enumerate() {
        let (status, actual, diagnostic) = match result.status {
            ExecutionStatus::Completed => {
                let actual = result_index
                    .actual(
                        intent.signal_kind,
                        &intent.canonical_identity,
                        intent.unit,
                        &intent.sample.value,
                    )
                    .ok_or_else(|| ContractDiagnostic {
                        code: "CC-SIM-CONTRACT-004",
                        path: format!("request.assertions[{index}]"),
                        message: "completed result does not contain the requested normalized signal and sample"
                            .to_owned(),
                    })?;
                let status = compare_assertion_values(
                    parse_authenticated_number(
                        actual,
                        &format!("result.assertions[{index}].actual"),
                    )?,
                    parse_authenticated_number(
                        &intent.expected,
                        &format!("request.assertions[{index}].expected"),
                    )?,
                    parse_authenticated_number(
                        &intent.absolute_tolerance,
                        &format!("request.assertions[{index}].absolute_tolerance"),
                    )?,
                    parse_authenticated_number(
                        &intent.relative_tolerance,
                        &format!("request.assertions[{index}].relative_tolerance"),
                    )?,
                    &format!("assertions[{index}].status"),
                )?;
                (
                    status,
                    RequiredNullable::some(actual.clone()),
                    RequiredNullable::none(),
                )
            }
            ExecutionStatus::Unsupported => (
                AssertionStatus::Unsupported,
                RequiredNullable::none(),
                RequiredNullable::some(
                    execution_diagnostic
                        .clone()
                        .ok_or_else(missing_execution_diagnostic)?,
                ),
            ),
            ExecutionStatus::Failed | ExecutionStatus::Unevaluated => (
                AssertionStatus::Unevaluated,
                RequiredNullable::none(),
                RequiredNullable::some(
                    execution_diagnostic
                        .clone()
                        .ok_or_else(missing_execution_diagnostic)?,
                ),
            ),
        };

        match status {
            AssertionStatus::Pass => summary.pass += 1,
            AssertionStatus::Fail => summary.fail += 1,
            AssertionStatus::Unsupported => summary.unsupported += 1,
            AssertionStatus::Unevaluated => summary.unevaluated += 1,
        }
        outcomes.push(AssertionOutcome {
            path: intent.path.clone(),
            status,
            signal_kind: intent.signal_kind,
            canonical_identity: intent.canonical_identity.clone(),
            sample: intent.sample.clone(),
            unit: intent.unit,
            expected: intent.expected.clone(),
            actual,
            absolute_tolerance: intent.absolute_tolerance.clone(),
            relative_tolerance: intent.relative_tolerance.clone(),
            diagnostic,
        });
    }

    let report = SimulationReport {
        schema_name: REPORT_SCHEMA_NAME.to_owned(),
        schema_version: CONTRACT_SCHEMA_VERSION,
        design: request.design.clone(),
        analysis_path: request.analysis.path.clone(),
        analysis_kind: request.analysis.kind,
        request_sha256: sha256_hex(request_json),
        map_sha256: sha256_hex(map_json),
        result_sha256: sha256_hex(result_json),
        assertions: outcomes,
        summary,
    };
    let report_json = report.to_canonical_json()?;

    // Verify the produced report against the same exact byte chain. This keeps
    // generation fail-closed if report construction ever diverges from the
    // independently validating contract implementation.
    report.verify_binding_bytes(request_json, map_json, result_json)?;
    let checked_success = result.status == ExecutionStatus::Completed
        && report
            .assertions
            .iter()
            .all(|outcome| outcome.status == AssertionStatus::Pass);

    Ok(AssertionEvaluation {
        report,
        report_json,
        checked_success,
    })
}

fn missing_execution_diagnostic() -> ContractDiagnostic {
    ContractDiagnostic {
        code: "CC-SIM-CONTRACT-002",
        path: "diagnostics".to_owned(),
        message: "a non-completed result requires at least one normalized diagnostic".to_owned(),
    }
}

fn parse_authenticated_number(value: &str, path: &str) -> Result<f64, ContractDiagnostic> {
    value
        .parse::<f64>()
        .ok()
        .filter(|value| value.is_finite())
        .ok_or_else(|| ContractDiagnostic {
            code: "CC-SIM-CONTRACT-005",
            path: path.to_owned(),
            message: "authenticated simulation number is not finite".to_owned(),
        })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::simulation::contract::{
        AnalysisKind, AxisKind, BackendIdentity, NormalizedDiagnostic, OHMNIVORE_BACKEND_CONTRACT,
        OHMNIVORE_BACKEND_NAME, OHMNIVORE_BACKEND_VERSION, OHMNIVORE_SOURCE_REVISION,
        REQUEST_SCHEMA_NAME, ReportSample, RequestAnalysis, RequestAssertion, ResultAxis,
        ResultSignal, ResultUnit, SPICE_MAP_SCHEMA_NAME, SignalKind, SimulationRequest,
        SimulationResult, SpiceIdentityMap, SpiceNetIdentity, canonical_f64, parse_report,
    };

    fn number(value: f64) -> String {
        canonical_f64(value).unwrap()
    }

    fn assertion(
        path: &str,
        identity: &str,
        expected: f64,
        absolute_tolerance: f64,
        relative_tolerance: f64,
    ) -> RequestAssertion {
        RequestAssertion {
            path: path.to_owned(),
            signal_kind: SignalKind::NetVoltage,
            canonical_identity: identity.to_owned(),
            sample: ReportSample {
                kind: AxisKind::Scalar,
                value: number(0.0),
            },
            unit: ResultUnit::Volt,
            expected: number(expected),
            absolute_tolerance: number(absolute_tolerance),
            relative_tolerance: number(relative_tolerance),
        }
    }

    fn request(assertions: Vec<RequestAssertion>) -> SimulationRequest {
        SimulationRequest {
            schema_name: REQUEST_SCHEMA_NAME.to_owned(),
            schema_version: CONTRACT_SCHEMA_VERSION,
            design: "assertion_evaluator".to_owned(),
            backend: BackendIdentity {
                name: OHMNIVORE_BACKEND_NAME.to_owned(),
                version: OHMNIVORE_BACKEND_VERSION.to_owned(),
                contract: OHMNIVORE_BACKEND_CONTRACT.to_owned(),
                source_revision: OHMNIVORE_SOURCE_REVISION.to_owned(),
            },
            analysis: RequestAnalysis {
                path: "simulation.dc".to_owned(),
                kind: AnalysisKind::DcOperatingPoint,
                netlist_path: "simulation/dc.spice".to_owned(),
                netlist_sha256: sha256_hex(b"deterministic netlist\n"),
                map_path: "simulation/dc.spice-map.json".to_owned(),
            },
            assertions,
        }
    }

    fn canonical_chain(
        assertions: Vec<RequestAssertion>,
        status: ExecutionStatus,
        values: Vec<(&str, f64)>,
    ) -> (String, String, String) {
        let request = request(assertions);
        let request_json = request.to_canonical_json().unwrap();
        let map = SpiceIdentityMap {
            schema_name: SPICE_MAP_SCHEMA_NAME.to_owned(),
            schema_version: CONTRACT_SCHEMA_VERSION,
            design: request.design.clone(),
            analysis_path: request.analysis.path.clone(),
            request_sha256: sha256_hex(request_json.as_bytes()),
            nets: vec![
                SpiceNetIdentity {
                    canonical: "GND".to_owned(),
                    backend: "0".to_owned(),
                    is_ground: true,
                },
                SpiceNetIdentity {
                    canonical: "NEG".to_owned(),
                    backend: "N_NEG".to_owned(),
                    is_ground: false,
                },
                SpiceNetIdentity {
                    canonical: "VOUT".to_owned(),
                    backend: "N_VOUT".to_owned(),
                    is_ground: false,
                },
            ],
            devices: Vec::new(),
        };
        let map_json = map.to_canonical_json().unwrap();
        let completed = status == ExecutionStatus::Completed;
        let result = SimulationResult {
            schema_name: super::super::contract::RESULT_SCHEMA_NAME.to_owned(),
            schema_version: CONTRACT_SCHEMA_VERSION,
            design: request.design.clone(),
            analysis_path: request.analysis.path.clone(),
            analysis_kind: request.analysis.kind,
            status,
            request_sha256: sha256_hex(request_json.as_bytes()),
            map_sha256: sha256_hex(map_json.as_bytes()),
            axis: ResultAxis {
                kind: AxisKind::Scalar,
                samples: if completed {
                    vec![number(0.0)]
                } else {
                    Vec::new()
                },
            },
            signals: if completed {
                values
                    .into_iter()
                    .map(|(identity, value)| ResultSignal {
                        kind: SignalKind::NetVoltage,
                        canonical_identity: identity.to_owned(),
                        unit: ResultUnit::Volt,
                        values: vec![number(value)],
                    })
                    .collect()
            } else {
                Vec::new()
            },
            diagnostics: if completed {
                Vec::new()
            } else {
                vec![NormalizedDiagnostic {
                    code: "CC-SIM-EXECUTION-TEST".to_owned(),
                    message: "deterministic test execution failure".to_owned(),
                }]
            },
        };
        let result_json = result.to_canonical_json().unwrap();
        (request_json, map_json, result_json)
    }

    fn evaluate(chain: &(String, String, String)) -> AssertionEvaluation {
        evaluate_assertions(chain.0.as_bytes(), chain.1.as_bytes(), chain.2.as_bytes()).unwrap()
    }

    #[test]
    fn inclusive_boundary_passes_and_one_ulp_beyond_fails() {
        let beyond = f64::from_bits(6.0_f64.to_bits() + 1);
        let chain = canonical_chain(
            vec![
                assertion("checks.a_boundary", "GND", 5.0, 1.0, 0.0),
                assertion("checks.b_beyond", "VOUT", 5.0, 1.0, 0.0),
            ],
            ExecutionStatus::Completed,
            vec![("GND", 6.0), ("VOUT", beyond)],
        );

        let evaluation = evaluate(&chain);

        assert_eq!(
            evaluation
                .report
                .assertions
                .iter()
                .map(|outcome| outcome.status)
                .collect::<Vec<_>>(),
            vec![AssertionStatus::Pass, AssertionStatus::Fail]
        );
        assert_eq!(evaluation.report.summary.pass, 1);
        assert_eq!(evaluation.report.summary.fail, 1);
        assert!(!evaluation.checked_success);
        assert_eq!(
            parse_report(&evaluation.report_json).unwrap(),
            evaluation.report
        );
        evaluation
            .report
            .verify_binding_bytes(chain.0.as_bytes(), chain.1.as_bytes(), chain.2.as_bytes())
            .unwrap();
    }

    #[test]
    fn relative_tolerance_uses_absolute_negative_expected_value() {
        let chain = canonical_chain(
            vec![assertion("checks.negative", "NEG", -4.0, 1.0, 0.5)],
            ExecutionStatus::Completed,
            vec![("NEG", -7.0)],
        );

        let evaluation = evaluate(&chain);

        assert_eq!(
            evaluation.report.assertions[0].status,
            AssertionStatus::Pass
        );
        assert!(evaluation.checked_success);
    }

    #[test]
    fn non_completed_results_map_every_assertion_to_the_required_status() {
        for (execution_status, assertion_status) in [
            (ExecutionStatus::Unsupported, AssertionStatus::Unsupported),
            (ExecutionStatus::Failed, AssertionStatus::Unevaluated),
            (ExecutionStatus::Unevaluated, AssertionStatus::Unevaluated),
        ] {
            let chain = canonical_chain(
                vec![assertion("checks.value", "VOUT", 5.0, 0.0, 0.0)],
                execution_status,
                Vec::new(),
            );

            let evaluation = evaluate(&chain);

            let outcome = &evaluation.report.assertions[0];
            assert_eq!(outcome.status, assertion_status);
            assert!(outcome.actual.is_none());
            assert_eq!(
                outcome.diagnostic.as_ref().unwrap().code,
                "CC-SIM-EXECUTION-TEST"
            );
            assert!(!evaluation.checked_success);
        }
    }

    #[test]
    fn completed_zero_assertions_succeeds_but_non_completed_zero_assertions_fails() {
        let completed =
            canonical_chain(Vec::new(), ExecutionStatus::Completed, vec![("VOUT", 5.0)]);
        let unsupported = canonical_chain(Vec::new(), ExecutionStatus::Unsupported, Vec::new());

        let completed_evaluation = evaluate(&completed);
        let unsupported_evaluation = evaluate(&unsupported);

        assert!(completed_evaluation.report.assertions.is_empty());
        assert!(completed_evaluation.checked_success);
        assert!(unsupported_evaluation.report.assertions.is_empty());
        assert!(!unsupported_evaluation.checked_success);
    }

    #[test]
    fn stale_request_binding_is_rejected_before_evaluation() {
        let chain = canonical_chain(
            vec![assertion("checks.value", "VOUT", -f64::MAX, 0.0, 0.0)],
            ExecutionStatus::Completed,
            vec![("VOUT", -f64::MAX)],
        );
        let stale_request = request(vec![assertion("checks.value", "VOUT", f64::MAX, 0.0, 0.0)])
            .to_canonical_json()
            .unwrap();

        let diagnostic = evaluate_assertions(
            stale_request.as_bytes(),
            chain.1.as_bytes(),
            chain.2.as_bytes(),
        )
        .unwrap_err();

        assert_eq!(diagnostic.code, "CC-SIM-CONTRACT-004");
        assert_eq!(diagnostic.path, "request_sha256");
    }

    #[test]
    fn authenticated_assertion_overflow_fails_with_a_contract_diagnostic() {
        let chain = canonical_chain(
            vec![assertion("checks.value", "VOUT", f64::MAX, 0.0, 0.0)],
            ExecutionStatus::Completed,
            vec![("VOUT", -f64::MAX)],
        );

        let diagnostic =
            evaluate_assertions(chain.0.as_bytes(), chain.1.as_bytes(), chain.2.as_bytes())
                .unwrap_err();

        assert_eq!(diagnostic.code, "CC-SIM-CONTRACT-005");
        assert_eq!(diagnostic.path, "assertions[0].status");
    }
}
