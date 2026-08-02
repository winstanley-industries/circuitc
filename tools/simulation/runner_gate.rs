use std::env;
use std::path::PathBuf;

use circuitc::simulation::{
    AnalysisKind, AxisKind, BackendIdentity, CONTRACT_SCHEMA_VERSION, ExecutionStatus,
    OHMNIVORE_BACKEND_CONTRACT, OHMNIVORE_BACKEND_NAME, OHMNIVORE_BACKEND_VERSION,
    OHMNIVORE_SOURCE_REVISION, OhmnivoreRunner, REQUEST_SCHEMA_NAME, ReportSample, RequestAnalysis,
    RequestAssertion, ResultUnit, SPICE_MAP_SCHEMA_NAME, SignalKind, SimulationRequest,
    SpiceDeviceIdentity, SpiceIdentityMap, SpiceNetIdentity, canonical_f64, sha256_hex,
};

struct Fixture {
    netlist: String,
    request: String,
    map: String,
    sample: String,
    signal_kind: SignalKind,
    canonical_identity: String,
    expected: f64,
}

fn main() {
    let arguments: Vec<_> = env::args_os().collect();
    if arguments.len() != 1 {
        eprintln!("usage: simulation_runner_gate");
        std::process::exit(2);
    }
    let work_root = env::var_os("TEST_TMPDIR")
        .map(PathBuf::from)
        .unwrap_or_else(env::temp_dir)
        .join("circuitc-runner-gate");
    let runner = OhmnivoreRunner::from_bazel_runfiles(work_root)
        .expect("the gate must resolve only its fixed Bazel runfiles");

    for fixture in [
        dc_fixture(),
        ac_fixture(),
        transient_fixture(),
        resistor_fixture(AnalysisKind::DcOperatingPoint, ".OP", AxisKind::Scalar, 0.0),
        resistor_fixture(
            AnalysisKind::Transient,
            ".TRAN 125e-3 250e-3 0",
            AxisKind::TimeSeconds,
            0.25,
        ),
    ] {
        let first = runner
            .execute(
                fixture.netlist.as_bytes(),
                fixture.request.as_bytes(),
                fixture.map.as_bytes(),
            )
            .expect("real runner fixture contracts must be valid");
        let second = runner
            .execute(
                fixture.netlist.as_bytes(),
                fixture.request.as_bytes(),
                fixture.map.as_bytes(),
            )
            .expect("repeat runner fixture contracts must be valid");
        assert_eq!(first.status, ExecutionStatus::Completed, "{first:#?}");
        assert!(first.diagnostics.is_empty());
        first
            .verify_binding_bytes(fixture.request.as_bytes(), fixture.map.as_bytes())
            .expect("normalized result must verify its complete digest and identity binding");
        assert_eq!(
            first.to_canonical_json().unwrap(),
            second.to_canonical_json().unwrap(),
            "normalized execution must repeat byte-identically"
        );
        let sample_index = first
            .axis
            .samples
            .iter()
            .position(|sample| sample == &fixture.sample)
            .expect("authenticated sample must exist");
        let signal = first
            .signals
            .iter()
            .find(|signal| {
                signal.kind == fixture.signal_kind
                    && signal.canonical_identity == fixture.canonical_identity
            })
            .expect("expected canonical signal must exist");
        let actual = signal.values[sample_index].parse::<f64>().unwrap();
        assert!(
            (actual - fixture.expected).abs() <= 1e-6,
            "unexpected VOUT: {actual}"
        );
    }
}

fn resistor_fixture(
    kind: AnalysisKind,
    directive: &str,
    sample_kind: AxisKind,
    sample: f64,
) -> Fixture {
    let netlist = format!(
        "* CircuitC resistor-only runner gate\n\
         * @circuitc-net 474E44 0\n\
         * @circuitc-net 4E N\n\
         * @circuitc-device 7265736973746F725F6F6E6C792E72 5231 R1\n\
         R1 N 0 1e3\n{directive}\n.END\n"
    );
    let analysis_path = match kind {
        AnalysisKind::DcOperatingPoint => "resistor_only.simulation.dc",
        AnalysisKind::Transient => "resistor_only.simulation.tran",
        AnalysisKind::AcLinearSweep => unreachable!(),
    };
    let request = SimulationRequest {
        schema_name: REQUEST_SCHEMA_NAME.to_owned(),
        schema_version: CONTRACT_SCHEMA_VERSION,
        design: "runner_gate_resistor_only".to_owned(),
        backend: BackendIdentity {
            name: OHMNIVORE_BACKEND_NAME.to_owned(),
            version: OHMNIVORE_BACKEND_VERSION.to_owned(),
            contract: OHMNIVORE_BACKEND_CONTRACT.to_owned(),
            source_revision: OHMNIVORE_SOURCE_REVISION.to_owned(),
        },
        analysis: RequestAnalysis {
            path: analysis_path.to_owned(),
            kind,
            netlist_path: "simulation/gate/analysis.spice".to_owned(),
            netlist_sha256: sha256_hex(netlist.as_bytes()),
            map_path: "simulation/gate/spice-map.json".to_owned(),
        },
        assertions: vec![RequestAssertion {
            path: format!("{analysis_path}.assertion"),
            signal_kind: SignalKind::NetVoltage,
            canonical_identity: "N".to_owned(),
            sample: ReportSample {
                kind: sample_kind,
                value: canonical_f64(sample).unwrap(),
            },
            unit: ResultUnit::Volt,
            expected: canonical_f64(0.0).unwrap(),
            absolute_tolerance: canonical_f64(1e-6).unwrap(),
            relative_tolerance: canonical_f64(0.0).unwrap(),
        }],
    }
    .to_canonical_json()
    .unwrap();
    let map = SpiceIdentityMap {
        schema_name: SPICE_MAP_SCHEMA_NAME.to_owned(),
        schema_version: CONTRACT_SCHEMA_VERSION,
        design: "runner_gate_resistor_only".to_owned(),
        analysis_path: analysis_path.to_owned(),
        request_sha256: sha256_hex(request.as_bytes()),
        nets: vec![
            SpiceNetIdentity {
                canonical: "GND".to_owned(),
                backend: "0".to_owned(),
                is_ground: true,
            },
            SpiceNetIdentity {
                canonical: "N".to_owned(),
                backend: "N".to_owned(),
                is_ground: false,
            },
        ],
        devices: vec![SpiceDeviceIdentity {
            semantic_path: "resistor_only.r".to_owned(),
            reference: "R1".to_owned(),
            backend: "R1".to_owned(),
        }],
    }
    .to_canonical_json()
    .unwrap();
    Fixture {
        netlist,
        request,
        map,
        sample: canonical_f64(sample).unwrap(),
        signal_kind: SignalKind::NetVoltage,
        canonical_identity: "N".to_owned(),
        expected: 0.0,
    }
}

fn dc_fixture() -> Fixture {
    fixture(FixtureSpec {
        analysis_path: "divider.simulation.dc",
        kind: AnalysisKind::DcOperatingPoint,
        directive: ".OP",
        sample_kind: AxisKind::Scalar,
        signal_kind: SignalKind::NetVoltage,
        sample: 0.0,
        expected: 5.0,
        source: "V1 VIN 0 DC 10",
    })
}

fn ac_fixture() -> Fixture {
    fixture(FixtureSpec {
        analysis_path: "divider.simulation.ac",
        kind: AnalysisKind::AcLinearSweep,
        directive: ".AC LIN 4 1 4",
        sample_kind: AxisKind::FrequencyHertz,
        signal_kind: SignalKind::NetVoltageMagnitude,
        sample: 3.0,
        expected: 0.5,
        source: "V1 VIN 0 DC 10 AC 1 0",
    })
}

fn transient_fixture() -> Fixture {
    fixture(FixtureSpec {
        analysis_path: "divider.simulation.tran",
        kind: AnalysisKind::Transient,
        directive: ".TRAN 125e-3 500e-3 0",
        sample_kind: AxisKind::TimeSeconds,
        signal_kind: SignalKind::NetVoltage,
        sample: 0.5,
        expected: 5.0,
        source: "V1 VIN 0 DC 10",
    })
}

struct FixtureSpec<'a> {
    analysis_path: &'a str,
    kind: AnalysisKind,
    directive: &'a str,
    sample_kind: AxisKind,
    signal_kind: SignalKind,
    sample: f64,
    expected: f64,
    source: &'a str,
}

fn fixture(spec: FixtureSpec<'_>) -> Fixture {
    let FixtureSpec {
        analysis_path,
        kind,
        directive,
        sample_kind,
        signal_kind,
        sample,
        expected,
        source,
    } = spec;
    let netlist = format!(
        "* CircuitC real runner gate\n\
         * @circuitc-net 474E44 0\n\
         * @circuitc-net 56494E VIN\n\
         * @circuitc-net 564F5554 VOUT\n\
         * @circuitc-device 646976696465722E696E707574 5631 V1\n\
         * @circuitc-device 646976696465722E725F626F74746F6D 5232 R2\n\
         * @circuitc-device 646976696465722E725F746F70 5231 R1\n\
         {source}\nR2 VOUT 0 10e3\nR1 VIN VOUT 10e3\n{directive}\n.END\n"
    );
    let request = SimulationRequest {
        schema_name: REQUEST_SCHEMA_NAME.to_owned(),
        schema_version: CONTRACT_SCHEMA_VERSION,
        design: "runner_gate".to_owned(),
        backend: BackendIdentity {
            name: OHMNIVORE_BACKEND_NAME.to_owned(),
            version: OHMNIVORE_BACKEND_VERSION.to_owned(),
            contract: OHMNIVORE_BACKEND_CONTRACT.to_owned(),
            source_revision: OHMNIVORE_SOURCE_REVISION.to_owned(),
        },
        analysis: RequestAnalysis {
            path: analysis_path.to_owned(),
            kind,
            netlist_path: "simulation/gate/analysis.spice".to_owned(),
            netlist_sha256: sha256_hex(netlist.as_bytes()),
            map_path: "simulation/gate/spice-map.json".to_owned(),
        },
        assertions: vec![RequestAssertion {
            path: format!("{analysis_path}.assertion"),
            signal_kind,
            canonical_identity: "VOUT".to_owned(),
            sample: ReportSample {
                kind: sample_kind,
                value: canonical_f64(sample).unwrap(),
            },
            unit: ResultUnit::Volt,
            expected: canonical_f64(expected).unwrap(),
            absolute_tolerance: canonical_f64(1e-6).unwrap(),
            relative_tolerance: canonical_f64(0.0).unwrap(),
        }],
    };
    let request = request.to_canonical_json().unwrap();
    let map = SpiceIdentityMap {
        schema_name: SPICE_MAP_SCHEMA_NAME.to_owned(),
        schema_version: CONTRACT_SCHEMA_VERSION,
        design: "runner_gate".to_owned(),
        analysis_path: analysis_path.to_owned(),
        request_sha256: sha256_hex(request.as_bytes()),
        nets: vec![
            SpiceNetIdentity {
                canonical: "GND".to_owned(),
                backend: "0".to_owned(),
                is_ground: true,
            },
            SpiceNetIdentity {
                canonical: "VIN".to_owned(),
                backend: "VIN".to_owned(),
                is_ground: false,
            },
            SpiceNetIdentity {
                canonical: "VOUT".to_owned(),
                backend: "VOUT".to_owned(),
                is_ground: false,
            },
        ],
        devices: vec![
            SpiceDeviceIdentity {
                semantic_path: "divider.input".to_owned(),
                reference: "V1".to_owned(),
                backend: "V1".to_owned(),
            },
            SpiceDeviceIdentity {
                semantic_path: "divider.r_bottom".to_owned(),
                reference: "R2".to_owned(),
                backend: "R2".to_owned(),
            },
            SpiceDeviceIdentity {
                semantic_path: "divider.r_top".to_owned(),
                reference: "R1".to_owned(),
                backend: "R1".to_owned(),
            },
        ],
    }
    .to_canonical_json()
    .unwrap();
    Fixture {
        netlist,
        request,
        map,
        sample: canonical_f64(sample).unwrap(),
        signal_kind,
        canonical_identity: "VOUT".to_owned(),
        expected,
    }
}
