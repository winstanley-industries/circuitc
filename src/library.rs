use crate::KicadLibraryFileKind;
use crate::design::{ElectricalPinType, Footprint, Pad, PadShape, PartIdentity, PointNm, SizeNm};

pub(crate) const SYMBOL_LIBRARY: &str = include_str!("../libraries/CircuitC.kicad_sym");
pub(crate) const RESISTOR_FOOTPRINT_LIBRARY: &str =
    include_str!("../libraries/CircuitC.pretty/R_0603_1608Metric.kicad_mod");

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) struct LibraryFileDefinition {
    pub kind: KicadLibraryFileKind,
    pub nickname: &'static str,
    pub relative_path: &'static str,
    pub table_relative_path: &'static str,
    pub contents: &'static str,
}

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

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) struct GraphicLineDefinition {
    pub semantic_name: &'static str,
    pub start: PointNm,
    pub end: PointNm,
    pub width_nm: i64,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) struct FootprintGraphicsDefinition {
    pub silkscreen_lines: &'static [GraphicLineDefinition],
    pub courtyard_start: PointNm,
    pub courtyard_end: PointNm,
    pub courtyard_width_nm: i64,
}

#[derive(Clone, Debug, Eq, PartialEq)]
struct FootprintCatalogDefinition {
    footprint: Footprint,
    graphics: FootprintGraphicsDefinition,
    library_file: LibraryFileDefinition,
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

pub(crate) fn symbol_library_file(library_id: &str) -> Option<LibraryFileDefinition> {
    symbol(library_id).map(|_| LibraryFileDefinition {
        kind: KicadLibraryFileKind::Symbol,
        nickname: "CircuitC",
        relative_path: "CircuitC.kicad_sym",
        table_relative_path: "CircuitC.kicad_sym",
        contents: SYMBOL_LIBRARY,
    })
}

pub(crate) fn footprint(library_id: &str) -> Option<Footprint> {
    footprint_catalog(library_id).map(|definition| definition.footprint)
}

const RESISTOR_SILKSCREEN: &[GraphicLineDefinition] = &[
    GraphicLineDefinition {
        semantic_name: "top",
        start: PointNm::new(-450_000, -500_000),
        end: PointNm::new(450_000, -500_000),
        width_nm: 120_000,
    },
    GraphicLineDefinition {
        semantic_name: "bottom",
        start: PointNm::new(-450_000, 500_000),
        end: PointNm::new(450_000, 500_000),
        width_nm: 120_000,
    },
];

pub(crate) fn footprint_graphics(library_id: &str) -> Option<FootprintGraphicsDefinition> {
    footprint_catalog(library_id).map(|definition| definition.graphics)
}

pub(crate) fn footprint_library_file(library_id: &str) -> Option<LibraryFileDefinition> {
    footprint_catalog(library_id).map(|definition| definition.library_file)
}

fn footprint_catalog(library_id: &str) -> Option<FootprintCatalogDefinition> {
    match library_id {
        "CircuitC:R_0603_1608Metric" => Some(FootprintCatalogDefinition {
            footprint: Footprint {
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
            },
            graphics: FootprintGraphicsDefinition {
                silkscreen_lines: RESISTOR_SILKSCREEN,
                courtyard_start: PointNm::new(-1_700_000, -750_000),
                courtyard_end: PointNm::new(1_700_000, 750_000),
                courtyard_width_nm: 50_000,
            },
            library_file: LibraryFileDefinition {
                kind: KicadLibraryFileKind::Footprint,
                nickname: "CircuitC",
                relative_path: "CircuitC.pretty/R_0603_1608Metric.kicad_mod",
                table_relative_path: "CircuitC.pretty",
                contents: RESISTOR_FOOTPRINT_LIBRARY,
            },
        }),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use crate::design::{PadShape, PartIdentity, PointNm, SizeNm};

    use super::{
        RESISTOR_FOOTPRINT_LIBRARY, SYMBOL_LIBRARY, footprint, footprint_graphics,
        footprint_library_file, part, symbol, symbol_library_file,
    };

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
        let graphics = footprint_graphics("CircuitC:R_0603_1608Metric")
            .expect("catalog footprint graphics must exist");
        assert_eq!(graphics.silkscreen_lines.len(), 2);
        assert_eq!(graphics.courtyard_start, PointNm::new(-1_700_000, -750_000));
        assert_eq!(graphics.courtyard_end, PointNm::new(1_700_000, 750_000));
        assert!(RESISTOR_FOOTPRINT_LIBRARY.contains("(layer \"F.CrtYd\")"));
        assert!(RESISTOR_FOOTPRINT_LIBRARY.contains("(fp_line\n    (start -0.45 -0.5)"));
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
    fn every_catalog_entry_has_a_publishable_library_file_and_footprint_graphics() {
        let mut library_bindings = std::collections::BTreeMap::new();
        for library_id in ["CircuitC:R", "CircuitC:VDC"] {
            assert!(symbol(library_id).is_some());
            let file = symbol_library_file(library_id)
                .expect("every catalog symbol must have a publishable library file");
            assert_eq!(file.kind, crate::KicadLibraryFileKind::Symbol);
            assert_eq!(file.nickname, "CircuitC");
            assert_eq!(file.relative_path, "CircuitC.kicad_sym");
            assert_eq!(file.table_relative_path, file.relative_path);
            assert!(!file.contents.is_empty());
            if let Some(first_path) =
                library_bindings.insert((file.kind, file.nickname), file.table_relative_path)
            {
                assert_eq!(
                    first_path, file.table_relative_path,
                    "each symbol-library nickname must identify exactly one table path"
                );
            }
        }
        for library_id in ["CircuitC:R_0603_1608Metric"] {
            assert!(footprint(library_id).is_some());
            assert!(footprint_graphics(library_id).is_some());
            let file = footprint_library_file(library_id)
                .expect("every catalog footprint must have a publishable library file");
            assert_eq!(file.kind, crate::KicadLibraryFileKind::Footprint);
            assert_eq!(file.nickname, "CircuitC");
            assert!(file.relative_path.ends_with(".kicad_mod"));
            assert_eq!(file.table_relative_path, "CircuitC.pretty");
            assert!(
                file.relative_path
                    .strip_prefix(file.table_relative_path)
                    .is_some_and(|suffix| suffix.starts_with('/') && suffix.ends_with(".kicad_mod"))
            );
            assert!(!file.contents.is_empty());
            if let Some(first_path) =
                library_bindings.insert((file.kind, file.nickname), file.table_relative_path)
            {
                assert_eq!(
                    first_path, file.table_relative_path,
                    "each footprint-library nickname must identify exactly one table path"
                );
            }
        }
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
