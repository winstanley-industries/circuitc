//! Code-authored reference designs used for executable architecture tests.

use crate::design::{
    Board, Component, ComponentValue, Connection, ConnectionState, CopperLayer,
    DESIGN_SCHEMA_VERSION, Design, ElectricalPinType, ModuleInstance, ModulePort, Net,
    PartIdentity, PhysicalImplementation, PinPadBinding, Placement, PointNm, PortDirection, RectNm,
    RouteSegment, SchematicPlacement, SimulationModel, SizeNm, SymbolBinding, SymbolPinBinding,
};
use crate::quantity::{Quantity, Unit};

const MM: i64 = 1_000_000;

/// A two-resistor divider with one code-authored route and an analysis-only DC source.
pub fn voltage_divider() -> Design {
    let mut design = Design {
        schema_version: DESIGN_SCHEMA_VERSION,
        name: "voltage_divider".to_owned(),
        nets: vec![
            Net {
                name: "VIN".to_owned(),
                is_ground: false,
            },
            Net {
                name: "VOUT".to_owned(),
                is_ground: false,
            },
            Net {
                name: "GND".to_owned(),
                is_ground: true,
            },
        ],
        modules: vec![
            ModuleInstance {
                path: "divider".to_owned(),
                ports: vec![
                    module_port("VIN", PortDirection::Input, "VIN"),
                    module_port("VOUT", PortDirection::Output, "VOUT"),
                    module_port("GND", PortDirection::Input, "GND"),
                ],
            },
            ModuleInstance {
                path: "divider.analysis".to_owned(),
                ports: vec![
                    ModulePort {
                        name: "VIN".to_owned(),
                        direction: PortDirection::Output,
                        electrical_type: ElectricalPinType::PowerOutput,
                        state: ConnectionState::Connected("VIN".to_owned()),
                    },
                    module_port("GND", PortDirection::Input, "GND"),
                ],
            },
        ],
        components: vec![
            resistor("divider.r_top", "R1", 15 * MM, 81_280_000, "VIN", "VOUT"),
            resistor(
                "divider.r_bottom",
                "R2",
                25 * MM,
                101_600_000,
                "VOUT",
                "GND",
            ),
            Component {
                path: "divider.analysis.input".to_owned(),
                reference: "V1".to_owned(),
                part: PartIdentity {
                    logical_device: "dc_voltage_source".to_owned(),
                    manufacturer: None,
                    manufacturer_part_number: None,
                },
                symbol: two_pin_symbol("CircuitC:VDC", "p", "n"),
                schematic_placement: SchematicPlacement {
                    position: PointNm::new(60_960_000, 81_280_000),
                    rotation_degrees: 0,
                },
                value: ComponentValue::DcVoltage(Quantity::new(10, 0, Unit::Volt)),
                connections: vec![
                    Connection {
                        pin: "p".to_owned(),
                        state: ConnectionState::Connected("VIN".to_owned()),
                    },
                    Connection {
                        pin: "n".to_owned(),
                        state: ConnectionState::Connected("GND".to_owned()),
                    },
                ],
                physical: None,
                simulation: Some(SimulationModel::DcVoltageSource {
                    model_id: "spice:Vdc".to_owned(),
                    positive_pin: "p".to_owned(),
                    negative_pin: "n".to_owned(),
                }),
            },
        ],
        analyses: Vec::new(),
        assertions: Vec::new(),
        board: Board {
            outline: RectNm {
                origin: PointNm::new(0, 0),
                size: SizeNm::new(40 * MM, 20 * MM),
            },
            routes: vec![RouteSegment {
                path: "board.routes.vout_bridge".to_owned(),
                net: "VOUT".to_owned(),
                start: PointNm::new(16 * MM, 10 * MM),
                end: PointNm::new(24 * MM, 10 * MM),
                width_nm: 250_000,
                layer: CopperLayer::Front,
            }],
        },
    };
    design.canonicalize();
    design
}

fn module_port(name: &str, direction: PortDirection, net: &str) -> ModulePort {
    ModulePort {
        name: name.to_owned(),
        direction,
        electrical_type: ElectricalPinType::Passive,
        state: ConnectionState::Connected(net.to_owned()),
    }
}

fn two_pin_symbol(library_id: &str, first_pin: &str, second_pin: &str) -> SymbolBinding {
    SymbolBinding {
        library_id: library_id.to_owned(),
        pins: vec![
            SymbolPinBinding {
                pin: first_pin.to_owned(),
                symbol_pin: "1".to_owned(),
                electrical_type: ElectricalPinType::Passive,
            },
            SymbolPinBinding {
                pin: second_pin.to_owned(),
                symbol_pin: "2".to_owned(),
                electrical_type: ElectricalPinType::Passive,
            },
        ],
    }
}

fn resistor(
    path: &str,
    reference: &str,
    x_nm: i64,
    schematic_x_nm: i64,
    pin_1_net: &str,
    pin_2_net: &str,
) -> Component {
    Component {
        path: path.to_owned(),
        reference: reference.to_owned(),
        part: PartIdentity {
            logical_device: "resistor".to_owned(),
            manufacturer: Some("Yageo".to_owned()),
            manufacturer_part_number: Some("RC0603FR-0710KL".to_owned()),
        },
        symbol: two_pin_symbol("CircuitC:R", "1", "2"),
        schematic_placement: SchematicPlacement {
            position: PointNm::new(schematic_x_nm, 81_280_000),
            rotation_degrees: 0,
        },
        value: ComponentValue::Resistance(Quantity::new(10, 3, Unit::Ohm)),
        connections: vec![
            Connection {
                pin: "1".to_owned(),
                state: ConnectionState::Connected(pin_1_net.to_owned()),
            },
            Connection {
                pin: "2".to_owned(),
                state: ConnectionState::Connected(pin_2_net.to_owned()),
            },
        ],
        physical: Some(PhysicalImplementation {
            footprint: crate::library::footprint("CircuitC:R_0603_1608Metric")
                .expect("reference footprint must be in the vendored catalog"),
            placement: Placement {
                position: PointNm::new(x_nm, 10 * MM),
                rotation_degrees: 0,
                layer: CopperLayer::Front,
            },
            pin_pad_bindings: vec![
                PinPadBinding {
                    pin: "1".to_owned(),
                    pad: "1".to_owned(),
                },
                PinPadBinding {
                    pin: "2".to_owned(),
                    pad: "2".to_owned(),
                },
            ],
        }),
        simulation: Some(SimulationModel::Resistor {
            model_id: "spice:R".to_owned(),
            positive_pin: "1".to_owned(),
            negative_pin: "2".to_owned(),
        }),
    }
}
