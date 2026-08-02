use crate::design::{ElectricalPinType, Footprint, Pad, PadShape, PartIdentity, PointNm, SizeNm};

pub(crate) const SYMBOL_LIBRARY: &str = include_str!("../libraries/CircuitC.kicad_sym");
pub(crate) const RESISTOR_FOOTPRINT_LIBRARY: &str =
    include_str!("../libraries/CircuitC.pretty/R_0603_1608Metric.kicad_mod");

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) struct SymbolPinDefinition {
    pub number: &'static str,
    pub electrical_type: ElectricalPinType,
    pub offset: PointNm,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) struct SymbolDefinition {
    pub library_id: &'static str,
    pub name: &'static str,
    pub pins: &'static [SymbolPinDefinition],
    pub on_board: bool,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) struct PartDefinition {
    pub logical_device: &'static str,
    pub manufacturer: Option<&'static str>,
    pub manufacturer_part_number: Option<&'static str>,
    pub symbol_library_id: &'static str,
    pub footprint_library_id: Option<&'static str>,
}

pub(crate) fn part(identity: &PartIdentity) -> Option<PartDefinition> {
    match (
        identity.logical_device.as_str(),
        identity.manufacturer.as_deref(),
        identity.manufacturer_part_number.as_deref(),
    ) {
        ("resistor", Some("Yageo"), Some("RC0603FR-0710KL")) => Some(PartDefinition {
            logical_device: "resistor",
            manufacturer: Some("Yageo"),
            manufacturer_part_number: Some("RC0603FR-0710KL"),
            symbol_library_id: "CircuitC:R",
            footprint_library_id: Some("CircuitC:R_0603_1608Metric"),
        }),
        ("dc_voltage_source", None, None) => Some(PartDefinition {
            logical_device: "dc_voltage_source",
            manufacturer: None,
            manufacturer_part_number: None,
            symbol_library_id: "CircuitC:VDC",
            footprint_library_id: None,
        }),
        _ => None,
    }
}

const TWO_PASSIVE_PINS: &[SymbolPinDefinition] = &[
    SymbolPinDefinition {
        number: "1",
        electrical_type: ElectricalPinType::Passive,
        offset: PointNm::new(0, -3_810_000),
    },
    SymbolPinDefinition {
        number: "2",
        electrical_type: ElectricalPinType::Passive,
        offset: PointNm::new(0, 3_810_000),
    },
];

pub(crate) fn symbol(library_id: &str) -> Option<SymbolDefinition> {
    match library_id {
        "CircuitC:R" => Some(SymbolDefinition {
            library_id: "CircuitC:R",
            name: "R",
            pins: TWO_PASSIVE_PINS,
            on_board: true,
        }),
        "CircuitC:VDC" => Some(SymbolDefinition {
            library_id: "CircuitC:VDC",
            name: "VDC",
            pins: TWO_PASSIVE_PINS,
            on_board: false,
        }),
        _ => None,
    }
}

pub(crate) fn footprint(library_id: &str) -> Option<Footprint> {
    match library_id {
        "CircuitC:R_0603_1608Metric" => Some(Footprint {
            library_id: library_id.to_owned(),
            pads: vec![
                Pad {
                    number: "1".to_owned(),
                    offset: PointNm::new(-1_000_000, 0),
                    size: SizeNm::new(900_000, 950_000),
                    shape: PadShape::RoundRect,
                },
                Pad {
                    number: "2".to_owned(),
                    offset: PointNm::new(1_000_000, 0),
                    size: SizeNm::new(900_000, 950_000),
                    shape: PadShape::RoundRect,
                },
            ],
        }),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use crate::design::{PadShape, PartIdentity, PointNm, SizeNm};

    use super::{RESISTOR_FOOTPRINT_LIBRARY, SYMBOL_LIBRARY, footprint, part, symbol};

    #[test]
    fn vendored_assets_match_the_catalog() {
        let resistor_identity = PartIdentity {
            logical_device: "resistor".to_owned(),
            manufacturer: Some("Yageo".to_owned()),
            manufacturer_part_number: Some("RC0603FR-0710KL".to_owned()),
        };
        let resistor = part(&resistor_identity).expect("resistor part binding must exist");
        assert_eq!(resistor.logical_device, "resistor");
        assert_eq!(resistor.manufacturer, Some("Yageo"));
        assert_eq!(resistor.manufacturer_part_number, Some("RC0603FR-0710KL"));
        assert_eq!(resistor.symbol_library_id, "CircuitC:R");
        assert_eq!(
            resistor.footprint_library_id,
            Some("CircuitC:R_0603_1608Metric")
        );
        assert_eq!(
            part(&PartIdentity {
                logical_device: "dc_voltage_source".to_owned(),
                manufacturer: None,
                manufacturer_part_number: None,
            })
            .expect("voltage source part binding must exist")
            .footprint_library_id,
            None
        );
        for id in ["CircuitC:R", "CircuitC:VDC"] {
            let definition = symbol(id).expect("catalog symbol must exist");
            assert_eq!(definition.library_id, id);
            assert!(
                SYMBOL_LIBRARY.contains(&format!("(symbol \"{}\"", definition.name)),
                "vendored symbol library must contain {}",
                definition.name
            );
        }
        let footprint =
            footprint("CircuitC:R_0603_1608Metric").expect("catalog footprint must exist");
        assert!(RESISTOR_FOOTPRINT_LIBRARY.contains("(footprint \"R_0603_1608Metric\""));
        assert_eq!(footprint.pads.len(), 2);
        assert_eq!(footprint.pads[0].number, "1");
        assert_eq!(footprint.pads[0].offset, PointNm::new(-1_000_000, 0));
        assert_eq!(footprint.pads[0].size, SizeNm::new(900_000, 950_000));
        assert_eq!(footprint.pads[0].shape, PadShape::RoundRect);
        assert_eq!(footprint.pads[1].number, "2");
        assert_eq!(footprint.pads[1].offset, PointNm::new(1_000_000, 0));
        assert!(
            RESISTOR_FOOTPRINT_LIBRARY
                .contains("(pad \"1\" smd roundrect\n    (at -1 0)\n    (size 0.9 0.95)")
        );
        assert!(
            RESISTOR_FOOTPRINT_LIBRARY
                .contains("(pad \"2\" smd roundrect\n    (at 1 0)\n    (size 0.9 0.95)")
        );
        assert!(
            SYMBOL_LIBRARY
                .contains("(pin passive line\n        (at 0 3.81 270)\n        (length 1.27)")
        );
        assert!(
            SYMBOL_LIBRARY
                .contains("(pin passive line\n        (at 0 -3.81 90)\n        (length 1.27)")
        );
    }

    #[test]
    fn catalog_lookup_rejects_incoherent_exact_part_identities() {
        for identity in [
            PartIdentity {
                logical_device: "resistor".to_owned(),
                manufacturer: Some("Acme".to_owned()),
                manufacturer_part_number: Some("banana".to_owned()),
            },
            PartIdentity {
                logical_device: "dc_voltage_source".to_owned(),
                manufacturer: Some("Acme".to_owned()),
                manufacturer_part_number: Some("VIRTUAL".to_owned()),
            },
        ] {
            assert_eq!(part(&identity), None);
        }
    }
}
