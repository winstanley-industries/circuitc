//! Code-authored reference designs used for executable architecture tests.

use crate::design::{
    Board, Component, Connection, CopperLayer, DESIGN_SCHEMA_VERSION, Design, Footprint, Net, Pad,
    PadShape, PhysicalImplementation, PinPadBinding, Placement, PointNm, RectNm, RouteSegment,
    SimulationModel, SizeNm,
};
use crate::quantity::{Quantity, Unit};

const MM: i64 = 1_000_000;

/// A two-resistor divider with one code-authored route and an analysis-only DC source.
pub fn voltage_divider() -> Design {
    Design {
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
        components: vec![
            resistor("divider.r_top", "R1", 15 * MM, "VIN", "VOUT"),
            resistor("divider.r_bottom", "R2", 25 * MM, "VOUT", "GND"),
            Component {
                path: "analysis.input".to_owned(),
                reference: "V1".to_owned(),
                connections: vec![
                    Connection {
                        pin: "p".to_owned(),
                        net: "VIN".to_owned(),
                    },
                    Connection {
                        pin: "n".to_owned(),
                        net: "GND".to_owned(),
                    },
                ],
                physical: None,
                simulation: Some(SimulationModel::DcVoltageSource {
                    voltage: Quantity::new(10, 0, Unit::Volt),
                    positive_pin: "p".to_owned(),
                    negative_pin: "n".to_owned(),
                }),
            },
        ],
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
    }
}

fn resistor(path: &str, reference: &str, x_nm: i64, pin_1_net: &str, pin_2_net: &str) -> Component {
    Component {
        path: path.to_owned(),
        reference: reference.to_owned(),
        connections: vec![
            Connection {
                pin: "1".to_owned(),
                net: pin_1_net.to_owned(),
            },
            Connection {
                pin: "2".to_owned(),
                net: pin_2_net.to_owned(),
            },
        ],
        physical: Some(PhysicalImplementation {
            footprint: Footprint {
                library_id: "CircuitC:R_0603_1608Metric".to_owned(),
                pads: vec![
                    Pad {
                        number: "1".to_owned(),
                        offset: PointNm::new(-MM, 0),
                        size: SizeNm::new(900_000, 950_000),
                        shape: PadShape::RoundRect,
                    },
                    Pad {
                        number: "2".to_owned(),
                        offset: PointNm::new(MM, 0),
                        size: SizeNm::new(900_000, 950_000),
                        shape: PadShape::RoundRect,
                    },
                ],
            },
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
            resistance: Quantity::new(10, 3, Unit::Ohm),
            positive_pin: "1".to_owned(),
            negative_pin: "2".to_owned(),
        }),
    }
}
