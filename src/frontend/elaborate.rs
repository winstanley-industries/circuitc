use std::collections::{BTreeMap, BTreeSet};

use crate::design::{
    Board, Component, Connection, CopperLayer, DESIGN_SCHEMA_VERSION, Design, Diagnostic,
    Footprint, Net, Pad, PadShape, PhysicalImplementation, PinPadBinding, Placement, PointNm,
    RectNm, RouteSegment, SimulationModel, SizeNm,
};
use crate::quantity::Unit;

use super::diagnostic::{SourceDiagnostic, sort_diagnostics};
use super::quantity::{lower_electrical, lower_length, lower_rotation};
use super::syntax::{
    BindingSyntax, BoardItemSyntax, BoardSyntax, ComponentItemSyntax, ComponentKindSyntax,
    ComponentSyntax, DeclarationSyntax, FootprintItemSyntax, FootprintSyntax, NetSyntax, PadSyntax,
    PlacementSyntax, PointSyntax, QuantitySyntax, RectangleSyntax, RouteSyntax, SourceFile, Span,
    SyntaxTree,
};

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ProvenanceMap {
    pub source_name: String,
    semantic_spans: BTreeMap<SemanticProvenanceKey, Span>,
    structural_spans: BTreeMap<String, Span>,
}

#[derive(Clone, Debug, Eq, Ord, PartialEq, PartialOrd)]
enum SemanticProvenanceKey {
    Component(String),
    Route(String),
    Footprint(String),
    Placement(String),
    Pad { component: String, pad: String },
}

impl SemanticProvenanceKey {
    fn rendered_path(&self) -> String {
        match self {
            Self::Component(path) | Self::Route(path) => path.clone(),
            Self::Footprint(component) => format!("{component}.footprint"),
            Self::Placement(component) => format!("{component}.placement"),
            Self::Pad { component, pad } => format!("{component}.footprint.pad.{pad}"),
        }
    }
}

impl ProvenanceMap {
    pub fn span_for(&self, semantic_path: &str) -> Option<Span> {
        let mut matching = self
            .semantic_spans
            .iter()
            .filter(|(key, _)| key.rendered_path() == semantic_path)
            .map(|(_, span)| *span);
        match (matching.next(), matching.next()) {
            (Some(span), None) => Some(span),
            (None, _) => self.structural_spans.get(semantic_path).copied(),
            (Some(_), Some(_)) => None,
        }
    }

    pub fn semantic_paths(&self) -> impl Iterator<Item = (String, Span)> + '_ {
        self.semantic_spans
            .iter()
            .map(|(key, span)| (key.rendered_path(), *span))
    }

    fn component_span(&self, path: &str) -> Option<Span> {
        self.semantic_spans
            .get(&SemanticProvenanceKey::Component(path.to_owned()))
            .copied()
    }

    fn kicad_span(&self, path: &str) -> Option<Span> {
        self.semantic_spans
            .iter()
            .filter(|(key, _)| {
                matches!(
                    key,
                    SemanticProvenanceKey::Footprint(_) | SemanticProvenanceKey::Pad { .. }
                ) && {
                    let rendered = key.rendered_path();
                    path == rendered
                        || path
                            .strip_prefix(&rendered)
                            .is_some_and(|suffix| suffix.starts_with('.'))
                }
            })
            .max_by_key(|(key, _)| key.rendered_path().len())
            .map(|(_, span)| *span)
    }

    fn best_structural_span(&self, path: &str) -> Option<Span> {
        best_span(&self.structural_spans, path)
    }

    fn insert_semantic(&mut self, key: SemanticProvenanceKey, span: Span) {
        self.semantic_spans.insert(key, span);
    }

    fn insert_structural(&mut self, path: impl Into<String>, span: Span) {
        self.structural_spans.entry(path.into()).or_insert(span);
    }
}

fn best_span(spans: &BTreeMap<String, Span>, path: &str) -> Option<Span> {
    spans.get(path).copied().or_else(|| {
        spans
            .iter()
            .filter(|(candidate, _)| {
                path.strip_prefix(candidate.as_str())
                    .is_some_and(|suffix| suffix.starts_with('.') || suffix.starts_with('['))
            })
            .max_by_key(|(candidate, _)| candidate.len())
            .map(|(_, span)| *span)
    })
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ElaboratedDesign {
    pub design: Design,
    pub provenance: ProvenanceMap,
}

pub(crate) fn elaborate(tree: &SyntaxTree) -> Result<ElaboratedDesign, Vec<SourceDiagnostic>> {
    let source = &tree.source;
    let mut diagnostics = Vec::new();
    let mut provenance = ProvenanceMap {
        source_name: source.name.clone(),
        semantic_spans: BTreeMap::new(),
        structural_spans: BTreeMap::new(),
    };
    provenance.insert_structural("design", tree.design.span);
    provenance.insert_structural("design.name", tree.design.name.span);

    if !artifact_name_is_valid(&tree.design.name.value) {
        diagnostics.push(SourceDiagnostic::new(
            "CC-LANG-DESIGN-001",
            source,
            tree.design.name.span,
            Some("design.name".to_owned()),
            "design name must start with an ASCII letter or underscore and contain only ASCII letters, digits, `_`, or `-`",
        ));
    }

    let mut net_syntax = Vec::new();
    let mut component_syntax = Vec::new();
    let mut board_syntax = Vec::new();
    for declaration in &tree.design.declarations {
        match declaration {
            DeclarationSyntax::Net(net) => net_syntax.push(net),
            DeclarationSyntax::Component(component) => component_syntax.push(component),
            DeclarationSyntax::Board(board) => board_syntax.push(board),
        }
    }

    let nets = elaborate_nets(source, &net_syntax, &mut provenance, &mut diagnostics);
    let component_index = index_components(source, &component_syntax, &mut diagnostics);
    let board = select_board(source, &board_syntax, &mut diagnostics);
    let board_parts = board.map(|board| {
        elaborate_board(
            source,
            board,
            &nets,
            &component_index,
            &mut provenance,
            &mut diagnostics,
        )
    });

    let mut components = Vec::new();
    let placements = board_parts
        .as_ref()
        .map_or_else(BTreeMap::new, |parts| parts.placements.clone());
    for component in component_index.by_path.values() {
        if let Some(lowered) = elaborate_component(
            source,
            component,
            &nets,
            &placements,
            &mut provenance,
            &mut diagnostics,
        ) {
            components.push(lowered);
        }
    }

    for (reference, placement) in &placements {
        if let Some(component) = component_index.by_reference.get(reference.as_str())
            && !component
                .items
                .iter()
                .any(|item| matches!(item, ComponentItemSyntax::Footprint(_)))
        {
            diagnostics.push(SourceDiagnostic::new(
                "CC-LANG-PLACE-004",
                source,
                placement.span,
                Some(component.path.value.clone()),
                format!("placement for component reference `{reference}` requires a footprint"),
            ));
        }
    }

    let ground_count = nets.values().filter(|net| net.is_ground).count();
    if !components.is_empty() && ground_count != 1 {
        diagnostics.push(SourceDiagnostic::new(
            "CC-LANG-GROUND-001",
            source,
            tree.design.span,
            Some("design.nets".to_owned()),
            format!("a simulated design requires exactly one ground; found {ground_count}"),
        ));
    }

    if !diagnostics.is_empty() {
        sort_diagnostics(&mut diagnostics);
        return Err(diagnostics);
    }

    let board_parts = board_parts.expect("a missing board produces a diagnostic");
    let mut design = Design {
        schema_version: DESIGN_SCHEMA_VERSION,
        name: tree.design.name.value.clone(),
        nets: nets.into_values().collect(),
        components,
        board: Board {
            outline: board_parts
                .outline
                .expect("a missing rectangle produces a diagnostic"),
            routes: board_parts.routes,
        },
    };
    design.canonicalize();
    register_indexed_provenance(&design, &mut provenance);
    Ok(ElaboratedDesign { design, provenance })
}

pub(crate) fn map_ir_diagnostics(
    source: &SourceFile,
    provenance: &ProvenanceMap,
    diagnostics: Vec<Diagnostic>,
) -> Vec<SourceDiagnostic> {
    let mut mapped: Vec<_> = diagnostics
        .into_iter()
        .map(|diagnostic| {
            let structural = diagnostic.code.starts_with("CC-IR-")
                || diagnostic.code.starts_with("CC-NET-")
                || diagnostic.code.starts_with("CC-BOARD-")
                || diagnostic.code.starts_with("CC-ROUTE-")
                || diagnostic.code == "CC-SIM-001";
            let semantic_span = if diagnostic.code.starts_with("CC-KICAD-") {
                provenance
                    .kicad_span(&diagnostic.path)
                    .or_else(|| provenance.component_span(&diagnostic.path))
            } else {
                provenance.component_span(&diagnostic.path)
            };
            let span = if structural {
                provenance
                    .best_structural_span(&diagnostic.path)
                    .or(semantic_span)
            } else if diagnostic.code.starts_with("CC-KICAD-") {
                provenance
                    .structural_spans
                    .get(&diagnostic.path)
                    .copied()
                    .or(semantic_span)
            } else {
                semantic_span.or_else(|| provenance.best_structural_span(&diagnostic.path))
            }
            .unwrap_or(Span::new(0, source.text.len()));
            SourceDiagnostic::new(
                diagnostic.code,
                source,
                span,
                Some(diagnostic.path),
                diagnostic.message,
            )
        })
        .collect();
    sort_diagnostics(&mut mapped);
    mapped
}

fn elaborate_nets(
    source: &SourceFile,
    syntax: &[&NetSyntax],
    provenance: &mut ProvenanceMap,
    diagnostics: &mut Vec<SourceDiagnostic>,
) -> BTreeMap<String, Net> {
    let mut nets = BTreeMap::new();
    let mut first_spans = BTreeMap::new();
    for net in syntax {
        let name = net.name.value.as_str();
        if !canonical_token_is_valid(name) {
            diagnostics.push(SourceDiagnostic::new(
                "CC-LANG-NET-001",
                source,
                net.name.span,
                Some(format!("design.nets.{name}")),
                "net name must be a non-empty canonical ASCII token",
            ));
        }
        if let Some(first) = first_spans.get(name).copied() {
            diagnostics.push(
                SourceDiagnostic::new(
                    "CC-LANG-NET-002",
                    source,
                    net.name.span,
                    Some(format!("design.nets.{name}")),
                    format!("duplicate net name `{name}`"),
                )
                .with_related(source, first, "first declaration is here"),
            );
            continue;
        }
        first_spans.insert(name, net.name.span);
        nets.insert(
            name.to_owned(),
            Net {
                name: name.to_owned(),
                is_ground: net.is_ground,
            },
        );
        provenance.insert_structural(format!("design.nets.{name}"), net.span);
    }
    nets
}

struct ComponentIndex<'a> {
    by_path: BTreeMap<&'a str, &'a ComponentSyntax>,
    by_reference: BTreeMap<&'a str, &'a ComponentSyntax>,
}

fn index_components<'a>(
    source: &SourceFile,
    components: &[&'a ComponentSyntax],
    diagnostics: &mut Vec<SourceDiagnostic>,
) -> ComponentIndex<'a> {
    let mut by_path: BTreeMap<&'a str, &'a ComponentSyntax> = BTreeMap::new();
    let mut by_reference: BTreeMap<&'a str, &'a ComponentSyntax> = BTreeMap::new();
    for component in components {
        let path = component.path.value.as_str();
        let reference = component.reference.value.as_str();
        if !semantic_path_is_valid(path) {
            diagnostics.push(SourceDiagnostic::new(
                "CC-LANG-COMP-001",
                source,
                component.path.span,
                Some(path.to_owned()),
                "component semantic path is invalid",
            ));
        }
        if !canonical_token_is_valid(reference) {
            diagnostics.push(SourceDiagnostic::new(
                "CC-LANG-COMP-002",
                source,
                component.reference.span,
                Some(path.to_owned()),
                "component reference must be a non-empty canonical ASCII token",
            ));
        }
        if let Some(first) = by_path.get(path) {
            diagnostics.push(
                SourceDiagnostic::new(
                    "CC-LANG-COMP-003",
                    source,
                    component.path.span,
                    Some(path.to_owned()),
                    format!("duplicate component semantic path `{path}`"),
                )
                .with_related(source, first.path.span, "first declaration is here"),
            );
        } else {
            by_path.insert(path, *component);
        }
        if let Some(first) = by_reference.get(reference) {
            diagnostics.push(
                SourceDiagnostic::new(
                    "CC-LANG-COMP-004",
                    source,
                    component.reference.span,
                    Some(path.to_owned()),
                    format!("duplicate component reference `{reference}`"),
                )
                .with_related(
                    source,
                    first.reference.span,
                    "first declaration is here",
                ),
            );
        } else {
            by_reference.insert(reference, *component);
        }
    }
    ComponentIndex {
        by_path,
        by_reference,
    }
}

fn select_board<'a>(
    source: &SourceFile,
    boards: &[&'a BoardSyntax],
    diagnostics: &mut Vec<SourceDiagnostic>,
) -> Option<&'a BoardSyntax> {
    let Some(first) = boards.first().copied() else {
        diagnostics.push(SourceDiagnostic::new(
            "CC-LANG-BOARD-001",
            source,
            Span::new(0, source.text.len()),
            Some("design.board".to_owned()),
            "design must contain one board declaration",
        ));
        return None;
    };
    for duplicate in &boards[1..] {
        diagnostics.push(
            SourceDiagnostic::new(
                "CC-LANG-BOARD-002",
                source,
                duplicate.span,
                Some("design.board".to_owned()),
                "duplicate board declaration",
            )
            .with_related(source, first.span, "first declaration is here"),
        );
    }
    Some(first)
}

#[derive(Clone)]
struct BoardParts {
    outline: Option<RectNm>,
    placements: BTreeMap<String, LoweredPlacement>,
    routes: Vec<RouteSegment>,
}

#[derive(Clone)]
struct LoweredPlacement {
    placement: Placement,
    span: Span,
}

fn elaborate_board(
    source: &SourceFile,
    board: &BoardSyntax,
    nets: &BTreeMap<String, Net>,
    components: &ComponentIndex<'_>,
    provenance: &mut ProvenanceMap,
    diagnostics: &mut Vec<SourceDiagnostic>,
) -> BoardParts {
    provenance.insert_structural("design.board", board.span);
    let mut rectangle_syntax: Option<&RectangleSyntax> = None;
    let mut placement_syntax = Vec::new();
    let mut route_syntax = Vec::new();
    for item in &board.items {
        match item {
            BoardItemSyntax::Rectangle(rectangle) => {
                if let Some(first) = rectangle_syntax {
                    diagnostics.push(
                        SourceDiagnostic::new(
                            "CC-LANG-BOARD-004",
                            source,
                            rectangle.span,
                            Some("design.board.outline".to_owned()),
                            "duplicate board rectangle",
                        )
                        .with_related(
                            source,
                            first.span,
                            "first rectangle is here",
                        ),
                    );
                } else {
                    rectangle_syntax = Some(rectangle);
                }
            }
            BoardItemSyntax::Placement(placement) => placement_syntax.push(placement),
            BoardItemSyntax::Route(route) => route_syntax.push(route),
        }
    }
    let outline = rectangle_syntax.and_then(|rectangle| {
        provenance.insert_structural("design.board.outline", rectangle.span);
        lower_rectangle(source, rectangle, diagnostics)
    });
    if rectangle_syntax.is_none() {
        diagnostics.push(SourceDiagnostic::new(
            "CC-LANG-BOARD-003",
            source,
            board.span,
            Some("design.board.outline".to_owned()),
            "board must contain one rectangle",
        ));
    }

    let mut placements: BTreeMap<String, LoweredPlacement> = BTreeMap::new();
    for syntax in placement_syntax {
        let reference = syntax.reference.value.as_str();
        let Some(component) = components.by_reference.get(reference).copied() else {
            diagnostics.push(SourceDiagnostic::new(
                "CC-LANG-RESOLVE-004",
                source,
                syntax.reference.span,
                Some("design.board".to_owned()),
                format!("placement references unknown component `{reference}`"),
            ));
            continue;
        };
        if let Some(first) = placements.get(reference) {
            diagnostics.push(
                SourceDiagnostic::new(
                    "CC-LANG-PLACE-001",
                    source,
                    syntax.reference.span,
                    Some(component.path.value.clone()),
                    format!("component `{reference}` is placed more than once"),
                )
                .with_related(source, first.span, "first placement is here"),
            );
            continue;
        }
        if let Some(placement) = lower_placement(source, syntax, &component.path.value, diagnostics)
        {
            provenance.insert_semantic(
                SemanticProvenanceKey::Placement(component.path.value.clone()),
                syntax.span,
            );
            placements.insert(
                reference.to_owned(),
                LoweredPlacement {
                    placement,
                    span: syntax.span,
                },
            );
        }
    }

    let mut routes = Vec::new();
    let mut route_paths = BTreeMap::new();
    for syntax in route_syntax {
        let path = syntax.path.value.as_str();
        if !semantic_path_is_valid(path) {
            diagnostics.push(SourceDiagnostic::new(
                "CC-LANG-ROUTE-001",
                source,
                syntax.path.span,
                Some(path.to_owned()),
                "route semantic path is invalid",
            ));
        }
        if let Some(first) = route_paths.get(path).copied() {
            diagnostics.push(
                SourceDiagnostic::new(
                    "CC-LANG-ROUTE-002",
                    source,
                    syntax.path.span,
                    Some(path.to_owned()),
                    format!("duplicate route semantic path `{path}`"),
                )
                .with_related(source, first, "first route is here"),
            );
            continue;
        }
        route_paths.insert(path, syntax.path.span);
        if let Some(route) = lower_route(source, syntax, nets, diagnostics) {
            provenance.insert_structural(format!("design.board.routes.{path}"), syntax.span);
            provenance.insert_semantic(SemanticProvenanceKey::Route(path.to_owned()), syntax.span);
            routes.push(route);
        }
    }
    BoardParts {
        outline,
        placements,
        routes,
    }
}

fn elaborate_component(
    source: &SourceFile,
    syntax: &ComponentSyntax,
    nets: &BTreeMap<String, Net>,
    placements: &BTreeMap<String, LoweredPlacement>,
    provenance: &mut ProvenanceMap,
    diagnostics: &mut Vec<SourceDiagnostic>,
) -> Option<Component> {
    let path = syntax.path.value.as_str();
    provenance.insert_semantic(
        SemanticProvenanceKey::Component(path.to_owned()),
        syntax.span,
    );

    let mut values: Vec<(&str, &QuantitySyntax, Span)> = Vec::new();
    let mut terminals: Vec<(
        &super::syntax::Spanned<String>,
        &super::syntax::Spanned<String>,
        Span,
    )> = Vec::new();
    let mut connections = Vec::new();
    let mut footprints = Vec::new();
    for item in &syntax.items {
        match item {
            ComponentItemSyntax::Value { keyword, quantity } => {
                values.push((&keyword.value, quantity, keyword.span))
            }
            ComponentItemSyntax::Terminals {
                positive,
                negative,
                span,
            } => terminals.push((positive, negative, *span)),
            ComponentItemSyntax::Connection { pin, net, span } => {
                connections.push((pin, net, *span))
            }
            ComponentItemSyntax::Footprint(footprint) => footprints.push(footprint),
        }
    }

    let expected_keyword = match syntax.kind {
        ComponentKindSyntax::Resistor => "resistance",
        ComponentKindSyntax::DcSource => "voltage",
    };
    let value = select_single(
        source,
        &values,
        syntax.span,
        path,
        "CC-LANG-COMP-005",
        "CC-LANG-COMP-006",
        &format!("component requires one `{expected_keyword}` declaration"),
        "component value is declared more than once",
        diagnostics,
    );
    if let Some((keyword, _, keyword_span)) = value
        && *keyword != expected_keyword
    {
        diagnostics.push(SourceDiagnostic::new(
            "CC-LANG-COMP-007",
            source,
            *keyword_span,
            Some(path.to_owned()),
            format!(
                "{} components require `{expected_keyword}`, not `{keyword}`",
                component_kind_name(syntax.kind)
            ),
        ));
    }
    let terminal = select_single(
        source,
        &terminals,
        syntax.span,
        path,
        "CC-LANG-COMP-008",
        "CC-LANG-COMP-009",
        "component requires one `terminals` declaration",
        "component terminals are declared more than once",
        diagnostics,
    );

    let mut connection_map = BTreeMap::new();
    let mut lowered_connections = Vec::new();
    for (pin, net, span) in connections {
        if !canonical_token_is_valid(&pin.value) {
            diagnostics.push(SourceDiagnostic::new(
                "CC-LANG-RESOLVE-002",
                source,
                pin.span,
                Some(path.to_owned()),
                "logical pin must be a non-empty canonical ASCII token",
            ));
        }
        if let Some(first) = connection_map.get(pin.value.as_str()).copied() {
            diagnostics.push(
                SourceDiagnostic::new(
                    "CC-LANG-CONNECT-001",
                    source,
                    pin.span,
                    Some(path.to_owned()),
                    format!("logical pin `{}` is connected more than once", pin.value),
                )
                .with_related(source, first, "first connection is here"),
            );
            continue;
        }
        connection_map.insert(pin.value.as_str(), span);
        if !nets.contains_key(net.value.as_str()) {
            diagnostics.push(SourceDiagnostic::new(
                "CC-LANG-RESOLVE-001",
                source,
                net.span,
                Some(path.to_owned()),
                format!("connection references unknown net `{}`", net.value),
            ));
        }
        lowered_connections.push(Connection {
            pin: pin.value.clone(),
            net: net.value.clone(),
        });
    }

    if let Some((positive, negative, _)) = terminal {
        if positive.value == negative.value {
            diagnostics.push(SourceDiagnostic::new(
                "CC-LANG-TERMINAL-001",
                source,
                negative.span,
                Some(path.to_owned()),
                "positive and negative terminals must be distinct",
            ));
        }
        for pin in [positive, negative] {
            if !connection_map.contains_key(pin.value.as_str()) {
                diagnostics.push(SourceDiagnostic::new(
                    "CC-LANG-RESOLVE-002",
                    source,
                    pin.span,
                    Some(path.to_owned()),
                    format!("terminal references unknown logical pin `{}`", pin.value),
                ));
            }
        }
    }

    let footprint = select_single(
        source,
        &footprints,
        syntax.span,
        path,
        "CC-LANG-FOOTPRINT-001",
        "CC-LANG-FOOTPRINT-002",
        "",
        "component footprint is declared more than once",
        diagnostics,
    );
    let physical = footprint.and_then(|footprint| {
        let placement = placements.get(syntax.reference.value.as_str());
        if placement.is_none() {
            diagnostics.push(SourceDiagnostic::new(
                "CC-LANG-PLACE-002",
                source,
                footprint.span,
                Some(path.to_owned()),
                format!(
                    "physical component `{}` requires one board placement",
                    syntax.reference.value
                ),
            ));
        }
        elaborate_footprint(
            source,
            footprint,
            path,
            &connection_map,
            placement.map(|placement| placement.placement),
            provenance,
            diagnostics,
        )
    });

    let simulation = match (value, terminal) {
        (Some((_, quantity, _)), Some((positive, negative, _))) => {
            let expected_unit = match syntax.kind {
                ComponentKindSyntax::Resistor => Unit::Ohm,
                ComponentKindSyntax::DcSource => Unit::Volt,
            };
            lower_electrical(source, quantity, expected_unit, Some(path), diagnostics).map(
                |lowered| match syntax.kind {
                    ComponentKindSyntax::Resistor => SimulationModel::Resistor {
                        resistance: lowered,
                        positive_pin: positive.value.clone(),
                        negative_pin: negative.value.clone(),
                    },
                    ComponentKindSyntax::DcSource => SimulationModel::DcVoltageSource {
                        voltage: lowered,
                        positive_pin: positive.value.clone(),
                        negative_pin: negative.value.clone(),
                    },
                },
            )
        }
        _ => None,
    };

    if value.is_none() || terminal.is_none() || simulation.is_none() {
        return None;
    }
    Some(Component {
        path: path.to_owned(),
        reference: syntax.reference.value.clone(),
        connections: lowered_connections,
        physical,
        simulation,
    })
}

#[allow(clippy::too_many_arguments)]
fn elaborate_footprint(
    source: &SourceFile,
    footprint: &FootprintSyntax,
    component_path: &str,
    connections: &BTreeMap<&str, Span>,
    placement: Option<Placement>,
    provenance: &mut ProvenanceMap,
    diagnostics: &mut Vec<SourceDiagnostic>,
) -> Option<PhysicalImplementation> {
    provenance.insert_semantic(
        SemanticProvenanceKey::Footprint(component_path.to_owned()),
        footprint.span,
    );
    let mut pads = Vec::new();
    let mut pad_spans = BTreeMap::new();
    let mut bindings = Vec::new();
    for item in &footprint.items {
        match item {
            FootprintItemSyntax::Pad(pad) => {
                let number = pad.number.value.as_str();
                if let Some(first) = pad_spans.get(number).copied() {
                    diagnostics.push(
                        SourceDiagnostic::new(
                            "CC-LANG-PAD-001",
                            source,
                            pad.number.span,
                            Some(component_path.to_owned()),
                            format!("duplicate physical pad `{number}`"),
                        )
                        .with_related(source, first, "first pad is here"),
                    );
                    continue;
                }
                pad_spans.insert(number, pad.span);
                if let Some(lowered) = lower_pad(source, pad, component_path, diagnostics) {
                    provenance.insert_semantic(
                        SemanticProvenanceKey::Pad {
                            component: component_path.to_owned(),
                            pad: number.to_owned(),
                        },
                        pad.span,
                    );
                    pads.push(lowered);
                }
            }
            FootprintItemSyntax::Binding(binding) => bindings.push(binding),
        }
    }
    if pad_spans.is_empty() {
        diagnostics.push(SourceDiagnostic::new(
            "CC-LANG-PAD-002",
            source,
            footprint.span,
            Some(component_path.to_owned()),
            "footprint must declare at least one pad",
        ));
    }

    let mut bound_pads = BTreeMap::new();
    let mut bound_pins = BTreeSet::new();
    let mut lowered_bindings = Vec::new();
    for binding in bindings {
        validate_binding(
            source,
            binding,
            component_path,
            connections,
            &pad_spans,
            &mut bound_pads,
            &mut bound_pins,
            &mut lowered_bindings,
            diagnostics,
        );
    }
    for (pad, span) in &pad_spans {
        if !bound_pads.contains_key(pad) {
            diagnostics.push(SourceDiagnostic::new(
                "CC-LANG-BIND-003",
                source,
                *span,
                Some(component_path.to_owned()),
                format!("physical pad `{pad}` has no logical-pin binding"),
            ));
        }
    }
    for (pin, span) in connections {
        if !bound_pins.contains(pin) {
            diagnostics.push(SourceDiagnostic::new(
                "CC-LANG-BIND-004",
                source,
                *span,
                Some(component_path.to_owned()),
                format!("connected logical pin `{pin}` has no physical pad binding"),
            ));
        }
    }
    placement.map(|placement| PhysicalImplementation {
        footprint: Footprint {
            library_id: footprint.library_id.value.clone(),
            pads,
        },
        placement,
        pin_pad_bindings: lowered_bindings,
    })
}

#[allow(clippy::too_many_arguments)]
fn validate_binding<'a>(
    source: &SourceFile,
    binding: &'a BindingSyntax,
    component_path: &str,
    connections: &BTreeMap<&str, Span>,
    pad_spans: &BTreeMap<&str, Span>,
    bound_pads: &mut BTreeMap<&'a str, Span>,
    bound_pins: &mut BTreeSet<&'a str>,
    lowered: &mut Vec<PinPadBinding>,
    diagnostics: &mut Vec<SourceDiagnostic>,
) {
    if !connections.contains_key(binding.pin.value.as_str()) {
        diagnostics.push(SourceDiagnostic::new(
            "CC-LANG-RESOLVE-002",
            source,
            binding.pin.span,
            Some(component_path.to_owned()),
            format!(
                "pin-to-pad binding references unknown logical pin `{}`",
                binding.pin.value
            ),
        ));
    }
    if !pad_spans.contains_key(binding.pad.value.as_str()) {
        diagnostics.push(SourceDiagnostic::new(
            "CC-LANG-RESOLVE-003",
            source,
            binding.pad.span,
            Some(component_path.to_owned()),
            format!(
                "pin-to-pad binding references unknown physical pad `{}`",
                binding.pad.value
            ),
        ));
    }
    if let Some(first) = bound_pads.get(binding.pad.value.as_str()).copied() {
        diagnostics.push(
            SourceDiagnostic::new(
                "CC-LANG-BIND-002",
                source,
                binding.pad.span,
                Some(component_path.to_owned()),
                format!(
                    "physical pad `{}` is bound more than once",
                    binding.pad.value
                ),
            )
            .with_related(source, first, "first binding is here"),
        );
    } else {
        bound_pads.insert(binding.pad.value.as_str(), binding.span);
    }
    bound_pins.insert(binding.pin.value.as_str());
    lowered.push(PinPadBinding {
        pin: binding.pin.value.clone(),
        pad: binding.pad.value.clone(),
    });
}

fn lower_pad(
    source: &SourceFile,
    syntax: &PadSyntax,
    component_path: &str,
    diagnostics: &mut Vec<SourceDiagnostic>,
) -> Option<Pad> {
    let offset = lower_point(source, &syntax.offset, Some(component_path), diagnostics);
    let size = lower_size(source, &syntax.size, Some(component_path), diagnostics);
    let shape = match syntax.shape.value.as_str() {
        "rect" => Some(PadShape::Rect),
        "roundrect" => Some(PadShape::RoundRect),
        other => {
            diagnostics.push(SourceDiagnostic::new(
                "CC-LANG-PAD-003",
                source,
                syntax.shape.span,
                Some(component_path.to_owned()),
                format!("unsupported pad shape `{other}`; expected `rect` or `roundrect`"),
            ));
            None
        }
    };
    match (offset, size, shape) {
        (Some(offset), Some(size), Some(shape)) => Some(Pad {
            number: syntax.number.value.clone(),
            offset,
            size,
            shape,
        }),
        _ => None,
    }
}

fn lower_rectangle(
    source: &SourceFile,
    syntax: &RectangleSyntax,
    diagnostics: &mut Vec<SourceDiagnostic>,
) -> Option<RectNm> {
    let origin = lower_point(
        source,
        &syntax.origin,
        Some("design.board.outline"),
        diagnostics,
    );
    let size = lower_size(
        source,
        &syntax.size,
        Some("design.board.outline"),
        diagnostics,
    );
    match (origin, size) {
        (Some(origin), Some(size)) => Some(RectNm { origin, size }),
        _ => None,
    }
}

fn lower_placement(
    source: &SourceFile,
    syntax: &PlacementSyntax,
    component_path: &str,
    diagnostics: &mut Vec<SourceDiagnostic>,
) -> Option<Placement> {
    let position = lower_point(source, &syntax.position, Some(component_path), diagnostics);
    let rotation = lower_rotation(
        source,
        &syntax.rotation.value,
        syntax.rotation.span,
        Some(component_path),
        diagnostics,
    );
    let layer = lower_layer(
        source,
        &syntax.layer.value,
        syntax.layer.span,
        component_path,
        diagnostics,
    );
    match (position, rotation, layer) {
        (Some(position), Some(rotation_degrees), Some(layer)) => Some(Placement {
            position,
            rotation_degrees,
            layer,
        }),
        _ => None,
    }
}

fn lower_route(
    source: &SourceFile,
    syntax: &RouteSyntax,
    nets: &BTreeMap<String, Net>,
    diagnostics: &mut Vec<SourceDiagnostic>,
) -> Option<RouteSegment> {
    let path = syntax.path.value.as_str();
    if !nets.contains_key(syntax.net.value.as_str()) {
        diagnostics.push(SourceDiagnostic::new(
            "CC-LANG-RESOLVE-001",
            source,
            syntax.net.span,
            Some(path.to_owned()),
            format!("route references unknown net `{}`", syntax.net.value),
        ));
    }
    let start = lower_point(source, &syntax.start, Some(path), diagnostics);
    let end = lower_point(source, &syntax.end, Some(path), diagnostics);
    let width = lower_length(source, &syntax.width, Some(path), diagnostics);
    if width.is_some_and(|width| width <= 0) {
        diagnostics.push(SourceDiagnostic::new(
            "CC-LANG-ROUTE-003",
            source,
            syntax.width.span,
            Some(path.to_owned()),
            "route width must be positive",
        ));
    }
    let layer = lower_layer(
        source,
        &syntax.layer.value,
        syntax.layer.span,
        path,
        diagnostics,
    );
    match (start, end, width, layer) {
        (Some(start), Some(end), Some(width), Some(layer)) if width > 0 => Some(RouteSegment {
            path: path.to_owned(),
            net: syntax.net.value.clone(),
            start,
            end,
            width_nm: width,
            layer,
        }),
        _ => None,
    }
}

fn lower_point(
    source: &SourceFile,
    syntax: &PointSyntax,
    semantic_path: Option<&str>,
    diagnostics: &mut Vec<SourceDiagnostic>,
) -> Option<PointNm> {
    let x = lower_length(source, &syntax.x, semantic_path, diagnostics);
    let y = lower_length(source, &syntax.y, semantic_path, diagnostics);
    match (x, y) {
        (Some(x), Some(y)) => Some(PointNm::new(x, y)),
        _ => None,
    }
}

fn lower_size(
    source: &SourceFile,
    syntax: &PointSyntax,
    semantic_path: Option<&str>,
    diagnostics: &mut Vec<SourceDiagnostic>,
) -> Option<SizeNm> {
    let width = lower_length(source, &syntax.x, semantic_path, diagnostics);
    let height = lower_length(source, &syntax.y, semantic_path, diagnostics);
    for (name, value, span) in [
        ("width", width, syntax.x.span),
        ("height", height, syntax.y.span),
    ] {
        if value.is_some_and(|value| value <= 0) {
            diagnostics.push(SourceDiagnostic::new(
                "CC-LANG-SIZE-001",
                source,
                span,
                semantic_path.map(str::to_owned),
                format!("{name} must be positive"),
            ));
        }
    }
    match (width, height) {
        (Some(width), Some(height)) if width > 0 && height > 0 => Some(SizeNm::new(width, height)),
        _ => None,
    }
}

fn lower_layer(
    source: &SourceFile,
    value: &str,
    span: Span,
    semantic_path: &str,
    diagnostics: &mut Vec<SourceDiagnostic>,
) -> Option<CopperLayer> {
    match value {
        "front" => Some(CopperLayer::Front),
        "back" => Some(CopperLayer::Back),
        other => {
            diagnostics.push(SourceDiagnostic::new(
                "CC-LANG-LAYER-001",
                source,
                span,
                Some(semantic_path.to_owned()),
                format!("unsupported copper layer `{other}`; expected `front` or `back`"),
            ));
            None
        }
    }
}

#[allow(clippy::too_many_arguments)]
fn select_single<'a, T>(
    source: &SourceFile,
    values: &'a [T],
    owner_span: Span,
    semantic_path: &str,
    missing_code: &'static str,
    duplicate_code: &'static str,
    missing_message: &str,
    duplicate_message: &str,
    diagnostics: &mut Vec<SourceDiagnostic>,
) -> Option<&'a T>
where
    T: HasSpan,
{
    let Some(first) = values.first() else {
        if !missing_message.is_empty() {
            diagnostics.push(SourceDiagnostic::new(
                missing_code,
                source,
                owner_span,
                Some(semantic_path.to_owned()),
                missing_message,
            ));
        }
        return None;
    };
    for duplicate in &values[1..] {
        diagnostics.push(
            SourceDiagnostic::new(
                duplicate_code,
                source,
                duplicate.span(),
                Some(semantic_path.to_owned()),
                duplicate_message,
            )
            .with_related(source, first.span(), "first declaration is here"),
        );
    }
    Some(first)
}

trait HasSpan {
    fn span(&self) -> Span;
}

impl HasSpan for (&str, &QuantitySyntax, Span) {
    fn span(&self) -> Span {
        self.2
    }
}

impl HasSpan
    for (
        &super::syntax::Spanned<String>,
        &super::syntax::Spanned<String>,
        Span,
    )
{
    fn span(&self) -> Span {
        self.2
    }
}

impl HasSpan for &FootprintSyntax {
    fn span(&self) -> Span {
        self.span
    }
}

fn register_indexed_provenance(design: &Design, provenance: &mut ProvenanceMap) {
    for (index, net) in design.nets.iter().enumerate() {
        if let Some(span) = provenance
            .structural_spans
            .get(&format!("design.nets.{}", net.name))
            .copied()
        {
            provenance.insert_structural(format!("design.nets[{index}]"), span);
        }
    }
    for (index, route) in design.board.routes.iter().enumerate() {
        if let Some(span) = provenance
            .structural_spans
            .get(&format!("design.board.routes.{}", route.path))
            .copied()
        {
            provenance.insert_structural(format!("design.board.routes[{index}]"), span);
        }
    }
}

fn component_kind_name(kind: ComponentKindSyntax) -> &'static str {
    match kind {
        ComponentKindSyntax::Resistor => "resistor",
        ComponentKindSyntax::DcSource => "dc_source",
    }
}

fn canonical_token_is_valid(value: &str) -> bool {
    !value.is_empty()
        && value
            .chars()
            .all(|character| character.is_ascii_alphanumeric() || "_+-./".contains(character))
}

fn semantic_path_is_valid(value: &str) -> bool {
    !value.is_empty()
        && value.split('.').all(canonical_token_is_valid)
        && !value.starts_with('.')
        && !value.ends_with('.')
}

fn artifact_name_is_valid(value: &str) -> bool {
    value
        .chars()
        .next()
        .is_some_and(|first| first.is_ascii_alphabetic() || first == '_')
        && value
            .chars()
            .all(|character| character.is_ascii_alphanumeric() || matches!(character, '_' | '-'))
}

#[cfg(test)]
mod tests {
    use super::elaborate;
    use crate::demo::voltage_divider;
    use crate::frontend::parser::parse;
    use crate::frontend::syntax::SourceFile;

    const REFERENCE: &str = r#"
design voltage_divider {
  net VIN;
  net VOUT;
  ground GND;
  resistor divider.r_top R1 {
    resistance 10 kohm;
    terminals 1 2;
    connect 1 VIN;
    connect 2 VOUT;
    footprint "CircuitC:R_0603_1608Metric" {
      pad 1 at (-1 mm, 0 mm) size (0.9 mm, 0.95 mm) shape roundrect;
      pad 2 at (1 mm, 0 mm) size (0.9 mm, 0.95 mm) shape roundrect;
      bind 1 1;
      bind 2 2;
    }
  }
  resistor divider.r_bottom R2 {
    resistance 10 kohm;
    terminals 1 2;
    connect 1 VOUT;
    connect 2 GND;
    footprint "CircuitC:R_0603_1608Metric" {
      pad 1 at (-1 mm, 0 mm) size (0.9 mm, 0.95 mm) shape roundrect;
      pad 2 at (1 mm, 0 mm) size (0.9 mm, 0.95 mm) shape roundrect;
      bind 1 1;
      bind 2 2;
    }
  }
  dc_source analysis.input V1 {
    voltage 10 V;
    terminals p n;
    connect p VIN;
    connect n GND;
  }
  board {
    rectangle at (0 mm, 0 mm) size (40 mm, 20 mm);
    place R1 at (15 mm, 10 mm) rotation 0 deg layer front;
    place R2 at (25 mm, 10 mm) rotation 0 deg layer front;
    route board.routes.vout_bridge net VOUT from (16 mm, 10 mm) to (24 mm, 10 mm) width 0.25 mm layer front;
  }
}
"#;

    fn elaborate_source(
        source: &str,
    ) -> Result<super::ElaboratedDesign, Vec<crate::frontend::SourceDiagnostic>> {
        let tree = parse(SourceFile::new("test.circuitc", source))?;
        elaborate(&tree)
    }

    #[test]
    fn reference_source_equals_rust_fixture_at_ir_boundary() {
        let elaborated = elaborate_source(REFERENCE).expect("reference source must elaborate");
        assert_eq!(elaborated.design, voltage_divider());
        assert!(elaborated.provenance.span_for("divider.r_top").is_some());
        assert!(
            elaborated
                .provenance
                .span_for("divider.r_top.footprint.pad.1")
                .is_some()
        );
    }

    #[test]
    fn declaration_order_does_not_change_ir() {
        let expected = elaborate_source(REFERENCE).expect("reference source must elaborate");
        let mut syntax = parse(SourceFile::new("reordered.circuitc", REFERENCE))
            .expect("reference source must parse");
        syntax.design.declarations.reverse();
        for declaration in &mut syntax.design.declarations {
            match declaration {
                crate::frontend::syntax::DeclarationSyntax::Component(component) => {
                    component.items.reverse();
                    for item in &mut component.items {
                        if let crate::frontend::syntax::ComponentItemSyntax::Footprint(footprint) =
                            item
                        {
                            footprint.items.reverse();
                        }
                    }
                }
                crate::frontend::syntax::DeclarationSyntax::Board(board) => board.items.reverse(),
                crate::frontend::syntax::DeclarationSyntax::Net(_) => {}
            }
        }
        let reordered = elaborate(&syntax).expect("reordered syntax must elaborate");
        assert_eq!(reordered.design, expected.design);
        assert_eq!(
            crate::compile(&reordered.design).expect("reordered design must compile"),
            crate::compile(&expected.design).expect("reference design must compile")
        );
    }

    #[test]
    fn full_turn_rotations_canonicalize_to_the_reference_ir() {
        let expected = elaborate_source(REFERENCE).expect("reference source must elaborate");
        for rotation in ["360", "-360"] {
            let source = REFERENCE.replace("rotation 0 deg", &format!("rotation {rotation} deg"));
            let rotated = elaborate_source(&source).expect("full turns must elaborate");
            assert_eq!(rotated.design, expected.design);
            assert_eq!(
                crate::compile(&rotated.design).expect("rotated design must compile"),
                crate::compile(&expected.design).expect("reference design must compile")
            );
        }
    }

    #[test]
    fn reports_required_resolution_and_binding_categories() {
        for (needle, replacement, code) in [
            ("connect 1 VIN", "connect 1 MISSING", "CC-LANG-RESOLVE-001"),
            (
                "terminals 1 2",
                "terminals 1 missing",
                "CC-LANG-RESOLVE-002",
            ),
            ("bind 1 1", "bind 1 missing", "CC-LANG-RESOLVE-003"),
            ("place R1 at", "place RX at", "CC-LANG-RESOLVE-004"),
        ] {
            let diagnostics = elaborate_source(&REFERENCE.replacen(needle, replacement, 1))
                .expect_err("invalid reference must fail");
            assert!(
                diagnostics.iter().any(|diagnostic| diagnostic.code == code),
                "missing {code}: {diagnostics:#?}"
            );
        }
    }

    #[test]
    fn reports_duplicate_names_and_related_locations() {
        let source = REFERENCE.replace("  net VOUT;", "  net VIN;");
        let diagnostics = elaborate_source(&source).expect_err("duplicate net must fail");
        let duplicate = diagnostics
            .iter()
            .find(|diagnostic| diagnostic.code == "CC-LANG-NET-002")
            .expect("duplicate diagnostic must exist");
        assert_eq!(duplicate.related.len(), 1);
    }

    #[test]
    fn reports_duplicate_semantic_identities_references_and_pads() {
        let duplicate_component =
            REFERENCE.replace("resistor divider.r_bottom R2", "resistor divider.r_top R1");
        let diagnostics = elaborate_source(&duplicate_component)
            .expect_err("duplicate component identities must fail");
        for code in ["CC-LANG-COMP-003", "CC-LANG-COMP-004"] {
            assert!(
                diagnostics.iter().any(|diagnostic| diagnostic.code == code),
                "missing {code}: {diagnostics:#?}"
            );
        }

        let duplicate_pad = REFERENCE.replacen("pad 2 at (1 mm, 0 mm)", "pad 1 at (1 mm, 0 mm)", 1);
        let diagnostics =
            elaborate_source(&duplicate_pad).expect_err("duplicate pad identities must fail");
        assert!(
            diagnostics
                .iter()
                .any(|diagnostic| diagnostic.code == "CC-LANG-PAD-001")
        );
    }

    #[test]
    fn reports_missing_and_duplicate_pin_to_pad_bindings() {
        let missing = REFERENCE.replacen("      bind 2 2;\n", "", 1);
        let diagnostics = elaborate_source(&missing).expect_err("missing binding must fail");
        for code in ["CC-LANG-BIND-003", "CC-LANG-BIND-004"] {
            assert!(
                diagnostics.iter().any(|diagnostic| diagnostic.code == code),
                "missing {code}: {diagnostics:#?}"
            );
        }

        let duplicate = REFERENCE.replacen("bind 2 2;", "bind 2 1;", 1);
        let diagnostics = elaborate_source(&duplicate).expect_err("duplicate binding must fail");
        assert!(
            diagnostics
                .iter()
                .any(|diagnostic| diagnostic.code == "CC-LANG-BIND-002")
        );
    }

    #[test]
    fn reports_invalid_route_identity_geometry_and_coordinate_overflow() {
        let invalid_identity =
            REFERENCE.replace("route board.routes.vout_bridge", "route .invalid");
        let diagnostics =
            elaborate_source(&invalid_identity).expect_err("invalid route identity must fail");
        assert!(
            diagnostics
                .iter()
                .any(|diagnostic| diagnostic.code == "CC-LANG-ROUTE-001")
        );

        let zero_length = REFERENCE.replace("to (24 mm, 10 mm)", "to (16 mm, 10 mm)");
        let diagnostics = crate::frontend::compile_source("route.circuitc", zero_length)
            .expect_err("zero-length route must fail Design validation");
        assert!(
            diagnostics
                .iter()
                .any(|diagnostic| diagnostic.code == "CC-ROUTE-003")
        );

        let overflow = REFERENCE.replace("size (40 mm, 20 mm)", "size (1000001 mm, 20 mm)");
        let diagnostics = elaborate_source(&overflow).expect_err("coordinate overflow must fail");
        assert!(
            diagnostics
                .iter()
                .any(|diagnostic| diagnostic.code == "CC-LANG-QUANTITY-004")
        );
    }

    #[test]
    fn reports_zero_and_multiple_grounds() {
        for (source, expected_count) in [
            (REFERENCE.replace("  ground GND;", "  net GND;"), "0"),
            (REFERENCE.replace("  net VIN;", "  ground VIN;"), "2"),
        ] {
            let diagnostics = elaborate_source(&source).expect_err("bad ground count must fail");
            assert!(diagnostics.iter().any(|diagnostic| {
                diagnostic.code == "CC-LANG-GROUND-001"
                    && diagnostic.message.contains(expected_count)
            }));
        }
    }
}
