use std::collections::{BTreeMap, BTreeSet};

use crate::design::{
    Board, Component, ComponentValue, Connection, ConnectionState, CopperLayer,
    DESIGN_SCHEMA_VERSION, Design, Diagnostic, ElectricalPinType, ModuleInstance, ModulePort, Net,
    PartIdentity, PhysicalImplementation, PinPadBinding, Placement, PointNm, PortDirection, RectNm,
    RouteSegment, SchematicPlacement, SimulationModel, SizeNm, SymbolBinding, SymbolPinBinding,
};
use crate::quantity::Unit;

use super::diagnostic::{SourceDiagnostic, sort_diagnostics};
use super::quantity::{lower_electrical, lower_length, lower_rotation};
use super::syntax::{
    BoardItemSyntax, BoardSyntax, ComponentItemSyntax, ComponentKindSyntax, ComponentSyntax,
    ConnectionStateSyntax, DeclarationSyntax, FootprintItemSyntax, FootprintSyntax, ModuleSyntax,
    NetSyntax, PartSyntax, PlacementSyntax, PointSyntax, QuantitySyntax, RectangleSyntax,
    RouteSyntax, SchematicPlacementSyntax, SourceFile, Span, SymbolSyntax, SyntaxTree,
};

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ProvenanceMap {
    semantic_spans: BTreeMap<SemanticProvenanceKey, Span>,
    rendered_semantic_spans: BTreeMap<String, Option<Span>>,
    identity_owner_spans: BTreeMap<String, Option<Span>>,
    route_spans: BTreeMap<String, Span>,
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
        match self.rendered_semantic_spans.get(semantic_path) {
            Some(span) => *span,
            None => self.structural_spans.get(semantic_path).copied(),
        }
    }

    pub fn semantic_paths(&self) -> impl Iterator<Item = (String, Span)> + '_ {
        self.semantic_spans
            .iter()
            .map(|(key, span)| (key.rendered_path(), *span))
    }

    pub(crate) fn span_for_identity(&self, semantic_path: &str) -> Option<Span> {
        if let Some(span) = self.route_spans.get(semantic_path) {
            return Some(*span);
        }
        let mut candidate = semantic_path;
        loop {
            if let Some(span) = self.identity_owner_spans.get(candidate) {
                return *span;
            }
            let Some((parent, _)) = candidate.rsplit_once('.') else {
                return self.structural_spans.get(semantic_path).copied();
            };
            candidate = parent;
        }
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
        let rendered = key.rendered_path();
        self.rendered_semantic_spans
            .entry(rendered.clone())
            .and_modify(|existing| *existing = None)
            .or_insert(Some(span));
        if matches!(&key, SemanticProvenanceKey::Route(_)) {
            self.route_spans.insert(rendered, span);
        } else {
            self.identity_owner_spans
                .entry(rendered)
                .and_modify(|existing| *existing = None)
                .or_insert(Some(span));
        }
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
        semantic_spans: BTreeMap::new(),
        rendered_semantic_spans: BTreeMap::new(),
        identity_owner_spans: BTreeMap::new(),
        route_spans: BTreeMap::new(),
        structural_spans: BTreeMap::new(),
    };
    provenance.insert_structural("design", tree.design.span);
    provenance.insert_structural("design.name", tree.design.name.span);

    if !crate::design::artifact_name_is_valid(&tree.design.name.value) {
        diagnostics.push(SourceDiagnostic::new(
            "CC-LANG-DESIGN-001",
            source,
            tree.design.name.span,
            Some("design.name".to_owned()),
            "design name must start with an ASCII letter or underscore and contain only ASCII letters, digits, `_`, or `-`",
        ));
    }

    let mut net_syntax = Vec::new();
    let mut module_syntax = Vec::new();
    let mut component_syntax = Vec::new();
    let mut board_syntax = Vec::new();
    for declaration in &tree.design.declarations {
        match declaration {
            DeclarationSyntax::Net(net) => net_syntax.push(net),
            DeclarationSyntax::Module(module) => module_syntax.push(module),
            DeclarationSyntax::Component(component) => component_syntax.push(component),
            DeclarationSyntax::Board(board) => board_syntax.push(board),
        }
    }

    let nets = elaborate_nets(source, &net_syntax, &mut provenance, &mut diagnostics);
    let modules = elaborate_modules(
        source,
        &module_syntax,
        &nets,
        &mut provenance,
        &mut diagnostics,
    );
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
            &modules,
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
    if components
        .iter()
        .any(|component| component.simulation.is_some())
        && ground_count != 1
    {
        diagnostics.push(SourceDiagnostic::new(
            "CC-LANG-GROUND-001",
            source,
            tree.design.span,
            Some("design.nets".to_owned()),
            format!("a simulated design requires exactly one ground; found {ground_count}"),
        ));
    }

    if diagnostics.is_empty()
        && let Some(board_parts) = &board_parts
    {
        validate_kicad_semantic_paths(
            source,
            &components,
            board_parts,
            &provenance,
            &mut diagnostics,
        );
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
        modules: modules.into_values().collect(),
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

fn validate_kicad_semantic_paths(
    source: &SourceFile,
    components: &[Component],
    board: &BoardParts,
    provenance: &ProvenanceMap,
    diagnostics: &mut Vec<SourceDiagnostic>,
) {
    let mut origins: BTreeMap<String, (&'static str, Span)> = BTreeMap::new();
    let mut register = |path: String, kind: &'static str, span: Span| {
        if let Some((first_kind, first_span)) = origins.get(&path).copied() {
            diagnostics.push(
                SourceDiagnostic::new(
                    "CC-KICAD-ID-002",
                    source,
                    span,
                    Some(path.clone()),
                    format!(
                        "generated KiCad semantic path `{path}` is shared by {first_kind} and {kind}"
                    ),
                )
                .with_related(
                    source,
                    first_span,
                    format!("first generated {first_kind} identity is here"),
                ),
            );
        } else {
            origins.insert(path, (kind, span));
        }
    };

    for descriptor in crate::kicad::generated_kicad_identity_descriptors(components, &board.routes)
    {
        let span = match descriptor.origin {
            crate::kicad::GeneratedKicadIdentityOrigin::Design => {
                provenance.structural_spans.get("design").copied()
            }
            crate::kicad::GeneratedKicadIdentityOrigin::BoardOutline => provenance
                .structural_spans
                .get("design.board.outline")
                .copied(),
            crate::kicad::GeneratedKicadIdentityOrigin::Component(component) => provenance
                .semantic_spans
                .get(&SemanticProvenanceKey::Component(component.to_owned()))
                .copied(),
            crate::kicad::GeneratedKicadIdentityOrigin::Footprint(component) => provenance
                .semantic_spans
                .get(&SemanticProvenanceKey::Footprint(component.to_owned()))
                .copied()
                .or_else(|| provenance.component_span(component)),
            crate::kicad::GeneratedKicadIdentityOrigin::Pad { component, pad } => provenance
                .semantic_spans
                .get(&SemanticProvenanceKey::Pad {
                    component: component.to_owned(),
                    pad: pad.to_owned(),
                })
                .copied()
                .or_else(|| {
                    provenance
                        .semantic_spans
                        .get(&SemanticProvenanceKey::Footprint(component.to_owned()))
                        .copied()
                })
                .or_else(|| provenance.component_span(component)),
            crate::kicad::GeneratedKicadIdentityOrigin::Route(route) => provenance
                .semantic_spans
                .get(&SemanticProvenanceKey::Route(route.to_owned()))
                .copied(),
        }
        .unwrap_or(Span::new(0, source.text.len()));
        register(descriptor.semantic_path, descriptor.diagnostic_kind, span);
    }
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
                || diagnostic.code.starts_with("CC-MODULE-")
                || diagnostic.code.starts_with("CC-PORT-")
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
                    .or_else(|| provenance.span_for_identity(&diagnostic.path))
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

fn elaborate_modules(
    source: &SourceFile,
    syntax: &[&ModuleSyntax],
    nets: &BTreeMap<String, Net>,
    provenance: &mut ProvenanceMap,
    diagnostics: &mut Vec<SourceDiagnostic>,
) -> BTreeMap<String, ModuleInstance> {
    let mut modules = BTreeMap::new();
    let mut first_spans = BTreeMap::new();
    for module in syntax {
        let path = module.path.value.as_str();
        if !semantic_path_is_valid(path) {
            diagnostics.push(SourceDiagnostic::new(
                "CC-LANG-MODULE-001",
                source,
                module.path.span,
                Some(path.to_owned()),
                "module instance path is invalid",
            ));
        }
        if let Some(first) = first_spans.get(path).copied() {
            diagnostics.push(
                SourceDiagnostic::new(
                    "CC-LANG-MODULE-002",
                    source,
                    module.path.span,
                    Some(path.to_owned()),
                    format!("duplicate module instance path `{path}`"),
                )
                .with_related(source, first, "first declaration is here"),
            );
            continue;
        }
        first_spans.insert(path, module.path.span);

        let mut ports = Vec::new();
        let mut port_spans = BTreeMap::new();
        for port in &module.ports {
            if !canonical_token_is_valid(&port.name.value) {
                diagnostics.push(SourceDiagnostic::new(
                    "CC-LANG-PORT-001",
                    source,
                    port.name.span,
                    Some(path.to_owned()),
                    "module port name must be a non-empty canonical ASCII token",
                ));
            }
            if let Some(first) = port_spans.get(port.name.value.as_str()).copied() {
                diagnostics.push(
                    SourceDiagnostic::new(
                        "CC-LANG-PORT-002",
                        source,
                        port.name.span,
                        Some(path.to_owned()),
                        format!("duplicate module port `{}`", port.name.value),
                    )
                    .with_related(source, first, "first declaration is here"),
                );
                continue;
            }
            port_spans.insert(port.name.value.as_str(), port.name.span);
            let direction = lower_port_direction(source, &port.direction, path, diagnostics);
            let electrical_type =
                lower_electrical_type(source, &port.electrical_type, path, diagnostics);
            let state = match &port.state {
                ConnectionStateSyntax::Connected(net) => {
                    if !nets.contains_key(net.value.as_str()) {
                        diagnostics.push(SourceDiagnostic::new(
                            "CC-LANG-RESOLVE-001",
                            source,
                            net.span,
                            Some(path.to_owned()),
                            format!("module port references unknown net `{}`", net.value),
                        ));
                    }
                    ConnectionState::Connected(net.value.clone())
                }
                ConnectionStateSyntax::NoConnect(_) => ConnectionState::NoConnect,
            };
            if let (Some(direction), Some(electrical_type)) = (direction, electrical_type) {
                ports.push(ModulePort {
                    name: port.name.value.clone(),
                    direction,
                    electrical_type,
                    state,
                });
            }
        }
        provenance.insert_structural(format!("design.modules.{path}"), module.span);
        modules.insert(
            path.to_owned(),
            ModuleInstance {
                path: path.to_owned(),
                ports,
            },
        );
    }

    for module in syntax {
        let path = module.path.value.as_str();
        if let Some((parent, _)) = path.rsplit_once('.')
            && !modules.contains_key(parent)
        {
            diagnostics.push(SourceDiagnostic::new(
                "CC-LANG-MODULE-003",
                source,
                module.path.span,
                Some(path.to_owned()),
                format!("module `{path}` requires parent module `{parent}`"),
            ));
        }
    }
    modules
}

fn lower_port_direction(
    source: &SourceFile,
    syntax: &super::syntax::Spanned<String>,
    path: &str,
    diagnostics: &mut Vec<SourceDiagnostic>,
) -> Option<PortDirection> {
    match syntax.value.as_str() {
        "input" => Some(PortDirection::Input),
        "output" => Some(PortDirection::Output),
        "inout" => Some(PortDirection::InOut),
        other => {
            diagnostics.push(SourceDiagnostic::new(
                "CC-LANG-PORT-003",
                source,
                syntax.span,
                Some(path.to_owned()),
                format!(
                    "unsupported port direction `{other}`; expected `input`, `output`, or `inout`"
                ),
            ));
            None
        }
    }
}

fn lower_electrical_type(
    source: &SourceFile,
    syntax: &super::syntax::Spanned<String>,
    path: &str,
    diagnostics: &mut Vec<SourceDiagnostic>,
) -> Option<ElectricalPinType> {
    let lowered = match syntax.value.as_str() {
        "input" => ElectricalPinType::Input,
        "output" => ElectricalPinType::Output,
        "bidirectional" => ElectricalPinType::Bidirectional,
        "passive" => ElectricalPinType::Passive,
        "power_input" => ElectricalPinType::PowerInput,
        "power_output" => ElectricalPinType::PowerOutput,
        "open_collector" => ElectricalPinType::OpenCollector,
        "open_emitter" => ElectricalPinType::OpenEmitter,
        other => {
            diagnostics.push(SourceDiagnostic::new(
                "CC-LANG-PIN-TYPE-001",
                source,
                syntax.span,
                Some(path.to_owned()),
                format!("unsupported electrical pin type `{other}`"),
            ));
            return None;
        }
    };
    Some(lowered)
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
    modules: &BTreeMap<String, ModuleInstance>,
    placements: &BTreeMap<String, LoweredPlacement>,
    provenance: &mut ProvenanceMap,
    diagnostics: &mut Vec<SourceDiagnostic>,
) -> Option<Component> {
    let path = syntax.path.value.as_str();
    provenance.insert_semantic(
        SemanticProvenanceKey::Component(path.to_owned()),
        syntax.span,
    );

    let module_path = match path.rsplit_once('.') {
        Some((module, _)) if modules.contains_key(module) => Some(module.to_owned()),
        Some((module, _)) => {
            diagnostics.push(SourceDiagnostic::new(
                "CC-LANG-COMP-010",
                source,
                syntax.path.span,
                Some(path.to_owned()),
                format!("component requires declared parent module `{module}`"),
            ));
            None
        }
        None => {
            diagnostics.push(SourceDiagnostic::new(
                "CC-LANG-COMP-010",
                source,
                syntax.path.span,
                Some(path.to_owned()),
                "component path must include its parent module",
            ));
            None
        }
    };

    let mut parts = Vec::new();
    let mut symbols = Vec::new();
    let mut models: Vec<(&super::syntax::Spanned<String>, Span)> = Vec::new();
    let mut schematic_placements = Vec::new();
    let mut values: Vec<(&str, &QuantitySyntax, Span)> = Vec::new();
    let mut terminals: Vec<(
        &super::syntax::Spanned<String>,
        &super::syntax::Spanned<String>,
        Span,
    )> = Vec::new();
    type ConnectionSyntax<'a> = (
        &'a super::syntax::Spanned<String>,
        Option<&'a super::syntax::Spanned<String>>,
        Span,
    );
    let mut connections: Vec<ConnectionSyntax<'_>> = Vec::new();
    let mut footprints = Vec::new();
    for item in &syntax.items {
        match item {
            ComponentItemSyntax::Part(part) => parts.push(part),
            ComponentItemSyntax::Symbol(symbol) => symbols.push(symbol),
            ComponentItemSyntax::Model { library_id, span } => models.push((library_id, *span)),
            ComponentItemSyntax::SchematicPlacement(placement) => {
                schematic_placements.push(placement)
            }
            ComponentItemSyntax::Value { keyword, quantity } => {
                values.push((&keyword.value, quantity, keyword.span))
            }
            ComponentItemSyntax::Terminals {
                positive,
                negative,
                span,
            } => terminals.push((positive, negative, *span)),
            ComponentItemSyntax::Connection { pin, net, span } => {
                connections.push((pin, Some(net), *span))
            }
            ComponentItemSyntax::NoConnect { pin, span } => connections.push((pin, None, *span)),
            ComponentItemSyntax::Footprint(footprint) => footprints.push(footprint),
        }
    }

    let part = select_single(
        source,
        &parts,
        syntax.path.span,
        path,
        "CC-LANG-PART-001",
        "CC-LANG-PART-002",
        "component requires one `part` declaration",
        "component part is declared more than once",
        diagnostics,
    )
    .copied();
    let symbol = select_single(
        source,
        &symbols,
        syntax.path.span,
        path,
        "CC-LANG-SYMBOL-001",
        "CC-LANG-SYMBOL-002",
        "component requires one `symbol` declaration",
        "component symbol is declared more than once",
        diagnostics,
    )
    .copied();
    let model = select_single(
        source,
        &models,
        syntax.span,
        path,
        "CC-LANG-MODEL-001",
        "CC-LANG-MODEL-002",
        "",
        "component model is declared more than once",
        diagnostics,
    );
    let schematic_placement = select_single(
        source,
        &schematic_placements,
        syntax.path.span,
        path,
        "CC-LANG-SCHEMATIC-001",
        "CC-LANG-SCHEMATIC-002",
        "component requires one `schematic` placement",
        "component schematic placement is declared more than once",
        diagnostics,
    )
    .copied();

    let lowered_part = part.map(lower_part);
    let lowered_symbol =
        symbol.and_then(|symbol| lower_symbol_binding(source, symbol, path, diagnostics));
    let lowered_schematic_placement = schematic_placement
        .and_then(|placement| lower_schematic_placement(source, placement, path, diagnostics));

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
        "",
        "component terminals are declared more than once",
        diagnostics,
    );

    let mut connection_map = BTreeMap::new();
    let mut lowered_connections = Vec::new();
    for (pin, net, _) in connections {
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
        connection_map.insert(pin.value.as_str(), pin.span);
        let state = match net {
            Some(net) => {
                if !nets.contains_key(net.value.as_str()) {
                    diagnostics.push(SourceDiagnostic::new(
                        "CC-LANG-RESOLVE-001",
                        source,
                        net.span,
                        Some(path.to_owned()),
                        format!("connection references unknown net `{}`", net.value),
                    ));
                }
                ConnectionState::Connected(net.value.clone())
            }
            None => ConnectionState::NoConnect,
        };
        lowered_connections.push(Connection {
            pin: pin.value.clone(),
            state,
        });
    }

    if let Some(lowered_symbol_binding) = &lowered_symbol {
        let symbol_pins: BTreeSet<_> = lowered_symbol_binding
            .pins
            .iter()
            .map(|pin| pin.pin.as_str())
            .collect();
        for connection in &lowered_connections {
            if !symbol_pins.contains(connection.pin.as_str()) {
                diagnostics.push(SourceDiagnostic::new(
                    "CC-LANG-SYMBOL-007",
                    source,
                    connection_map[connection.pin.as_str()],
                    Some(path.to_owned()),
                    format!(
                        "connection references logical pin `{}` absent from the symbol binding",
                        connection.pin
                    ),
                ));
            }
        }
        for pin in &lowered_symbol_binding.pins {
            if !connection_map.contains_key(pin.pin.as_str()) {
                let span = symbol
                    .and_then(|syntax| {
                        syntax
                            .pins
                            .iter()
                            .find(|candidate| candidate.pin.value == pin.pin)
                    })
                    .map_or(syntax.path.span, |pin| pin.pin.span);
                diagnostics.push(SourceDiagnostic::new(
                    "CC-LANG-CONNECT-002",
                    source,
                    span,
                    Some(path.to_owned()),
                    format!(
                        "symbol logical pin `{}` requires `connect` or `no_connect`",
                        pin.pin
                    ),
                ));
            }
        }
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
            let connection = lowered_connections
                .iter()
                .find(|connection| connection.pin == pin.value);
            if connection.is_none() {
                diagnostics.push(SourceDiagnostic::new(
                    "CC-LANG-RESOLVE-002",
                    source,
                    pin.span,
                    Some(path.to_owned()),
                    format!("terminal references unknown logical pin `{}`", pin.value),
                ));
            } else if connection
                .is_some_and(|connection| matches!(connection.state, ConnectionState::NoConnect))
            {
                diagnostics.push(SourceDiagnostic::new(
                    "CC-LANG-SIM-002",
                    source,
                    pin.span,
                    Some(path.to_owned()),
                    format!(
                        "simulation terminal references unconnected logical pin `{}`",
                        pin.value
                    ),
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
            &lowered_connections,
            placement.map(|placement| placement.placement),
            provenance,
            diagnostics,
        )
    });

    let expected_unit = match syntax.kind {
        ComponentKindSyntax::Resistor => Unit::Ohm,
        ComponentKindSyntax::DcSource => Unit::Volt,
    };
    let lowered_value = value.and_then(|(_, quantity, _)| {
        lower_electrical(source, quantity, expected_unit, Some(path), diagnostics).map(|quantity| {
            match syntax.kind {
                ComponentKindSyntax::Resistor => ComponentValue::Resistance(quantity),
                ComponentKindSyntax::DcSource => ComponentValue::DcVoltage(quantity),
            }
        })
    });

    let simulation_configuration_valid = terminal.is_some() == model.is_some();
    if !simulation_configuration_valid {
        let span = terminal
            .map(|(_, _, span)| *span)
            .or_else(|| model.map(|(_, span)| *span))
            .unwrap_or(syntax.span);
        diagnostics.push(SourceDiagnostic::new(
            "CC-LANG-SIM-001",
            source,
            span,
            Some(path.to_owned()),
            "`model` and `terminals` must be declared together",
        ));
    }
    let simulation = match (terminal, model) {
        (Some((positive, negative, _)), Some((model, _))) => Some(match syntax.kind {
            ComponentKindSyntax::Resistor => SimulationModel::Resistor {
                model_id: model.value.clone(),
                positive_pin: positive.value.clone(),
                negative_pin: negative.value.clone(),
            },
            ComponentKindSyntax::DcSource => SimulationModel::DcVoltageSource {
                model_id: model.value.clone(),
                positive_pin: positive.value.clone(),
                negative_pin: negative.value.clone(),
            },
        }),
        _ => None,
    };

    if lowered_value.is_none()
        || !simulation_configuration_valid
        || module_path.is_none()
        || lowered_part.is_none()
        || lowered_symbol.is_none()
        || lowered_schematic_placement.is_none()
    {
        return None;
    }
    Some(Component {
        path: path.to_owned(),
        reference: syntax.reference.value.clone(),
        part: lowered_part.expect("checked above"),
        value: lowered_value.expect("checked above"),
        symbol: lowered_symbol.expect("checked above"),
        schematic_placement: lowered_schematic_placement.expect("checked above"),
        connections: lowered_connections,
        physical,
        simulation,
    })
}

fn lower_part(syntax: &PartSyntax) -> PartIdentity {
    PartIdentity {
        logical_device: syntax.logical_device.value.clone(),
        manufacturer: syntax
            .manufacturer
            .as_ref()
            .map(|manufacturer| manufacturer.value.clone()),
        manufacturer_part_number: syntax
            .manufacturer_part_number
            .as_ref()
            .map(|number| number.value.clone()),
    }
}

fn lower_symbol_binding(
    source: &SourceFile,
    syntax: &SymbolSyntax,
    component_path: &str,
    diagnostics: &mut Vec<SourceDiagnostic>,
) -> Option<SymbolBinding> {
    let Some(definition) = crate::library::symbol(&syntax.library_id.value) else {
        diagnostics.push(SourceDiagnostic::new(
            "CC-LANG-SYMBOL-003",
            source,
            syntax.library_id.span,
            Some(component_path.to_owned()),
            format!(
                "symbol `{}` is not present in the vendored CircuitC catalog",
                syntax.library_id.value
            ),
        ));
        return None;
    };
    let mut logical_pins = BTreeMap::new();
    let mut library_pins = BTreeMap::new();
    let mut lowered = Vec::new();
    for pin in &syntax.pins {
        if let Some(first) = logical_pins.get(pin.pin.value.as_str()).copied() {
            diagnostics.push(
                SourceDiagnostic::new(
                    "CC-LANG-SYMBOL-004",
                    source,
                    pin.pin.span,
                    Some(component_path.to_owned()),
                    format!("logical pin `{}` is bound more than once", pin.pin.value),
                )
                .with_related(source, first, "first binding is here"),
            );
            continue;
        }
        logical_pins.insert(pin.pin.value.as_str(), pin.pin.span);
        if let Some(first) = library_pins.get(pin.symbol_pin.value.as_str()).copied() {
            diagnostics.push(
                SourceDiagnostic::new(
                    "CC-LANG-SYMBOL-005",
                    source,
                    pin.symbol_pin.span,
                    Some(component_path.to_owned()),
                    format!(
                        "library symbol pin `{}` is bound more than once",
                        pin.symbol_pin.value
                    ),
                )
                .with_related(source, first, "first binding is here"),
            );
            continue;
        }
        library_pins.insert(pin.symbol_pin.value.as_str(), pin.symbol_pin.span);
        let Some(catalog_pin) = definition
            .pins
            .iter()
            .find(|candidate| candidate.number == pin.symbol_pin.value)
        else {
            diagnostics.push(SourceDiagnostic::new(
                "CC-LANG-SYMBOL-006",
                source,
                pin.symbol_pin.span,
                Some(component_path.to_owned()),
                format!(
                    "symbol `{}` has no pin `{}`",
                    syntax.library_id.value, pin.symbol_pin.value
                ),
            ));
            continue;
        };
        let Some(electrical_type) =
            lower_electrical_type(source, &pin.electrical_type, component_path, diagnostics)
        else {
            continue;
        };
        if electrical_type != catalog_pin.electrical_type {
            diagnostics.push(SourceDiagnostic::new(
                "CC-LANG-SYMBOL-008",
                source,
                pin.electrical_type.span,
                Some(component_path.to_owned()),
                format!(
                    "electrical type for symbol pin `{}` does not match the vendored library",
                    pin.symbol_pin.value
                ),
            ));
        }
        lowered.push(SymbolPinBinding {
            pin: pin.pin.value.clone(),
            symbol_pin: pin.symbol_pin.value.clone(),
            electrical_type,
        });
    }
    for catalog_pin in definition.pins {
        if !library_pins.contains_key(catalog_pin.number) {
            diagnostics.push(SourceDiagnostic::new(
                "CC-LANG-SYMBOL-009",
                source,
                syntax.span,
                Some(component_path.to_owned()),
                format!(
                    "symbol pin `{}` has no explicit logical binding",
                    catalog_pin.number
                ),
            ));
        }
    }
    Some(SymbolBinding {
        library_id: definition.library_id.to_owned(),
        pins: lowered,
    })
}

fn lower_schematic_placement(
    source: &SourceFile,
    syntax: &SchematicPlacementSyntax,
    component_path: &str,
    diagnostics: &mut Vec<SourceDiagnostic>,
) -> Option<SchematicPlacement> {
    let position = lower_point(source, &syntax.position, Some(component_path), diagnostics);
    let rotation = lower_rotation(
        source,
        &syntax.rotation.value,
        syntax.rotation.span,
        Some(component_path),
        diagnostics,
    );
    match (position, rotation) {
        (Some(position), Some(rotation_degrees)) => Some(SchematicPlacement {
            position,
            rotation_degrees,
        }),
        _ => None,
    }
}

#[allow(clippy::too_many_arguments)]
fn elaborate_footprint(
    source: &SourceFile,
    footprint: &FootprintSyntax,
    component_path: &str,
    connections: &[Connection],
    placement: Option<Placement>,
    provenance: &mut ProvenanceMap,
    diagnostics: &mut Vec<SourceDiagnostic>,
) -> Option<PhysicalImplementation> {
    provenance.insert_semantic(
        SemanticProvenanceKey::Footprint(component_path.to_owned()),
        footprint.span,
    );
    let Some(catalog_footprint) = crate::library::footprint(&footprint.library_id.value) else {
        diagnostics.push(SourceDiagnostic::new(
            "CC-LANG-FOOTPRINT-003",
            source,
            footprint.library_id.span,
            Some(component_path.to_owned()),
            format!(
                "footprint `{}` is not present in the vendored CircuitC catalog",
                footprint.library_id.value
            ),
        ));
        return None;
    };
    let pad_numbers: BTreeSet<_> = catalog_footprint
        .pads
        .iter()
        .map(|pad| pad.number.as_str())
        .collect();
    let logical_pins: BTreeSet<_> = connections
        .iter()
        .map(|connection| connection.pin.as_str())
        .collect();

    let mut bound_pads = BTreeMap::new();
    let mut bound_pins = BTreeSet::new();
    let mut lowered_bindings = Vec::new();
    for item in &footprint.items {
        let FootprintItemSyntax::Binding(binding) = item;
        if !logical_pins.contains(binding.pin.value.as_str()) {
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
        if !pad_numbers.contains(binding.pad.value.as_str()) {
            diagnostics.push(SourceDiagnostic::new(
                "CC-LANG-RESOLVE-003",
                source,
                binding.pad.span,
                Some(component_path.to_owned()),
                format!(
                    "pin-to-pad binding references unknown catalog pad `{}`",
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
        provenance.insert_semantic(
            SemanticProvenanceKey::Pad {
                component: component_path.to_owned(),
                pad: binding.pad.value.clone(),
            },
            binding.span,
        );
        lowered_bindings.push(PinPadBinding {
            pin: binding.pin.value.clone(),
            pad: binding.pad.value.clone(),
        });
    }
    for pad in &catalog_footprint.pads {
        if !bound_pads.contains_key(pad.number.as_str()) {
            diagnostics.push(SourceDiagnostic::new(
                "CC-LANG-BIND-003",
                source,
                footprint.span,
                Some(component_path.to_owned()),
                format!("catalog pad `{}` has no logical-pin binding", pad.number),
            ));
        }
    }
    for connection in connections {
        if matches!(&connection.state, ConnectionState::Connected(_))
            && !bound_pins.contains(connection.pin.as_str())
        {
            diagnostics.push(SourceDiagnostic::new(
                "CC-LANG-BIND-004",
                source,
                footprint.span,
                Some(component_path.to_owned()),
                format!(
                    "connected logical pin `{}` has no physical pad binding",
                    connection.pin
                ),
            ));
        }
    }
    placement.map(|placement| PhysicalImplementation {
        footprint: catalog_footprint,
        placement,
        pin_pad_bindings: lowered_bindings,
    })
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

impl HasSpan for &PartSyntax {
    fn span(&self) -> Span {
        self.span
    }
}

impl HasSpan for &SymbolSyntax {
    fn span(&self) -> Span {
        self.span
    }
}

impl HasSpan for (&super::syntax::Spanned<String>, Span) {
    fn span(&self) -> Span {
        self.1
    }
}

impl HasSpan for &SchematicPlacementSyntax {
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
    for (index, module) in design.modules.iter().enumerate() {
        if let Some(span) = provenance
            .structural_spans
            .get(&format!("design.modules.{}", module.path))
            .copied()
        {
            provenance.insert_structural(format!("design.modules[{index}]"), span);
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

#[cfg(test)]
mod tests {
    use super::{ProvenanceMap, SemanticProvenanceKey, elaborate};
    use crate::demo::voltage_divider;
    use crate::frontend::parser::parse;
    use crate::frontend::syntax::SourceFile;

    const REFERENCE: &str = include_str!("../../examples/voltage_divider.circuitc");
    const PHYSICAL_NO_CONNECT: &str = include_str!("../../examples/physical_no_connect.circuitc");

    fn elaborate_source(
        source: &str,
    ) -> Result<super::ElaboratedDesign, Vec<crate::frontend::SourceDiagnostic>> {
        let tree = parse(SourceFile::new("test.circuitc", source))?;
        elaborate(&tree)
    }

    fn assert_source_diagnostic(
        source: &str,
        code: &str,
        message: &str,
        expected_start: usize,
        expected_text: &str,
        related_count: usize,
    ) {
        let diagnostics = elaborate_source(source).expect_err("mutated source must fail");
        let diagnostic = diagnostics
            .iter()
            .find(|diagnostic| diagnostic.code == code)
            .unwrap_or_else(|| panic!("missing {code}: {diagnostics:#?}"));
        assert_eq!(diagnostic.message, message);
        assert_eq!(diagnostic.start, expected_start);
        assert_eq!(diagnostic.end, expected_start + expected_text.len());
        assert_eq!(&source[diagnostic.start..diagnostic.end], expected_text);
        assert_eq!(diagnostic.related.len(), related_count);
    }

    #[test]
    fn large_provenance_index_resolves_derived_identity_paths() {
        let mut provenance = ProvenanceMap {
            semantic_spans: std::collections::BTreeMap::new(),
            rendered_semantic_spans: std::collections::BTreeMap::new(),
            identity_owner_spans: std::collections::BTreeMap::new(),
            route_spans: std::collections::BTreeMap::new(),
            structural_spans: std::collections::BTreeMap::new(),
        };
        for index in 0..4096 {
            provenance.insert_semantic(
                SemanticProvenanceKey::Component(format!("root.c{index}")),
                crate::frontend::Span {
                    start: index,
                    end: index + 1,
                },
            );
        }
        for index in 0..4096 {
            assert_eq!(
                provenance
                    .span_for_identity(&format!("root.c{index}.symbol.pin.1"))
                    .map(|span| span.start),
                Some(index)
            );
        }
    }

    #[test]
    fn generated_identity_provenance_ignores_route_prefixes() {
        let source = REFERENCE.replacen(
            "route board.routes.vout_bridge",
            "route divider.r_top.symbol",
            1,
        );
        let compiled = crate::frontend::compile_source("route-prefix.circuitc", &source)
            .expect("a route that is only a prefix of an identity may compile");
        let component_start = source
            .find("resistor divider.r_top")
            .expect("component declaration exists");
        let route_start = source
            .find("route divider.r_top.symbol")
            .expect("route declaration exists");
        let semantic_path = "divider.r_top.symbol.pin.1";
        let span = compiled
            .elaborated
            .provenance
            .span_for_identity(semantic_path)
            .expect("derived symbol-pin identity must resolve to its component");
        assert_eq!(span.start, component_start);
        assert_ne!(span.start, route_start);

        let exact_route_span = compiled
            .elaborated
            .provenance
            .span_for_identity("divider.r_top.symbol")
            .expect("an exact route identity must resolve to its route declaration");
        assert_eq!(exact_route_span.start, route_start);

        let marker = format!("\"semantic_path\": \"{semantic_path}\"");
        let identity = compiled
            .kicad_identity_map
            .split(&marker)
            .nth(1)
            .and_then(|tail| tail.split("\n    }").next())
            .expect("identity-map entry must exist");
        assert!(
            identity.contains(&format!("\"location\": {{\"start\": {component_start},")),
            "identity map attributed {semantic_path} to the wrong owner: {identity}"
        );

        let route_marker = "\"semantic_path\": \"divider.r_top.symbol\"";
        let route_identity = compiled
            .kicad_identity_map
            .split(route_marker)
            .nth(1)
            .and_then(|tail| tail.split("\n    }").next())
            .expect("route identity-map entry must exist");
        assert!(
            route_identity.contains(&format!("\"location\": {{\"start\": {route_start},")),
            "identity map lost the exact route location: {route_identity}"
        );
    }

    #[test]
    fn kicad_connection_diagnostics_map_to_the_owning_component() {
        let source = REFERENCE
            .replacen(
                "schematic at (101.6 mm, 81.28 mm)",
                "schematic at (81.28 mm, 88.9 mm)",
                1,
            )
            .replacen("connect 1 VOUT;", "connect 1 GND;", 1);
        let component_start = source
            .find("resistor divider.r_top")
            .expect("colliding component declaration exists");
        let diagnostic = crate::frontend::compile_source("collision.circuitc", &source)
            .expect_err("different connections at one schematic point must fail")
            .into_iter()
            .find(|diagnostic| diagnostic.code == "CC-KICAD-SCHEMATIC-002")
            .expect("schematic collision diagnostic must exist");
        assert_eq!(
            diagnostic.semantic_path.as_deref(),
            Some("divider.r_top.connection.2")
        );
        assert_eq!(diagnostic.start, component_start);
        assert_ne!(diagnostic.start, 0);
        assert_eq!(
            &source[diagnostic.start..diagnostic.start + "resistor".len()],
            "resistor"
        );
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
    fn module_ports_accept_inout_with_an_explicit_no_connect_state() {
        let source = REFERENCE.replacen(
            "port input GND passive connect GND;",
            "port inout GND passive no_connect;",
            1,
        );
        let elaborated = elaborate_source(&source)
            .expect("inout no-connect module port must elaborate successfully");
        let port = elaborated
            .design
            .modules
            .iter()
            .flat_map(|module| &module.ports)
            .find(|port| {
                port.name == "GND"
                    && port.direction == crate::design::PortDirection::InOut
                    && port.state == crate::design::ConnectionState::NoConnect
            })
            .expect("lowered inout no-connect port must exist");
        assert_eq!(port.direction, crate::design::PortDirection::InOut);
        assert_eq!(port.state, crate::design::ConnectionState::NoConnect);
        elaborated
            .design
            .validate()
            .expect("inout no-connect module port must satisfy the Design IR contract");
    }

    #[test]
    fn source_can_author_a_physical_only_component_with_connected_and_no_connect_pins() {
        let elaborated = elaborate_source(PHYSICAL_NO_CONNECT)
            .expect("physical-only no-connect source must elaborate");
        let component = elaborated
            .design
            .components
            .iter()
            .find(|component| component.reference == "R1")
            .expect("physical-only resistor must exist");
        assert!(component.physical.is_some());
        assert!(component.simulation.is_none());
        assert!(matches!(
            component.value,
            crate::design::ComponentValue::Resistance(_)
        ));
        assert!(component.connections.iter().any(|connection| {
            matches!(&connection.state, crate::design::ConnectionState::Connected(net) if net == "TEST")
        }));
        assert!(component.connections.iter().any(|connection| matches!(
            connection.state,
            crate::design::ConnectionState::NoConnect
        )));
    }

    #[test]
    fn ground_is_optional_only_when_every_component_is_physical_only() {
        const PHYSICAL_ONLY: &str = r#"design physical_only {
  net TEST;
  module board_only {}

  resistor board_only.unused R1 {
    part "resistor" manufacturer "Yageo" number "RC0603FR-0710KL";
    symbol "CircuitC:R" {
      bind 1 1 passive;
      bind 2 2 passive;
    }
    schematic at (81.28 mm, 81.28 mm) rotation 0 deg;
    resistance 10 kohm;
    connect 1 TEST;
    no_connect 2;
    footprint "CircuitC:R_0603_1608Metric" {
      bind 1 1;
      bind 2 2;
    }
  }

  board {
    rectangle at (0 mm, 0 mm) size (10 mm, 10 mm);
    place R1 at (5 mm, 5 mm) rotation 0 deg layer front;
  }
}
"#;

        let compiled = crate::frontend::compile_source("physical_only.circuitc", PHYSICAL_ONLY)
            .expect("a ground-less physical-only design must compile");
        assert!(
            compiled
                .elaborated
                .design
                .nets
                .iter()
                .all(|net| !net.is_ground)
        );
        assert!(!compiled.artifacts.spice.contains("@circuitc-device"));

        let simulated = PHYSICAL_ONLY.replacen(
            "    schematic at",
            "    model \"spice:R\";\n    terminals 1 2;\n    schematic at",
            1,
        );
        let diagnostics = crate::frontend::compile_source("simulated.circuitc", simulated)
            .expect_err("adding simulation restores the single-ground requirement");
        assert!(
            diagnostics
                .iter()
                .any(|diagnostic| diagnostic.code == "CC-LANG-GROUND-001"),
            "missing ground diagnostic: {diagnostics:#?}"
        );
    }

    #[test]
    fn simulation_model_and_terminals_are_an_optional_pair() {
        for source in [
            REFERENCE.replacen("    model \"spice:R\";\n", "", 1),
            REFERENCE.replacen("    terminals 1 2;\n", "", 1),
        ] {
            let diagnostics = elaborate_source(&source)
                .expect_err("half of a simulation declaration pair must fail");
            assert!(
                diagnostics
                    .iter()
                    .any(|diagnostic| diagnostic.code == "CC-LANG-SIM-001"),
                "missing optional-pair diagnostic: {diagnostics:#?}"
            );
        }
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
                crate::frontend::syntax::DeclarationSyntax::Module(module) => {
                    module.ports.reverse()
                }
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
            (
                "footprint \"CircuitC:R_0603_1608Metric\" {\n      bind 1 1;",
                "footprint \"CircuitC:R_0603_1608Metric\" {\n      bind 1 missing;",
                "CC-LANG-RESOLVE-003",
            ),
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
    fn rejects_unknown_catalog_bindings_models_and_orphan_modules() {
        let unknown_symbol = REFERENCE.replacen("CircuitC:R\"", "CircuitC:UNKNOWN\"", 1);
        let diagnostics = elaborate_source(&unknown_symbol).expect_err("unknown symbol must fail");
        assert!(
            diagnostics
                .iter()
                .any(|diagnostic| diagnostic.code == "CC-LANG-SYMBOL-003")
        );

        let unknown_footprint =
            REFERENCE.replacen("CircuitC:R_0603_1608Metric", "CircuitC:UNKNOWN", 1);
        let diagnostics =
            elaborate_source(&unknown_footprint).expect_err("unknown footprint must fail");
        assert!(
            diagnostics
                .iter()
                .any(|diagnostic| diagnostic.code == "CC-LANG-FOOTPRINT-003")
        );

        let orphan_module = REFERENCE.replacen("module divider.analysis", "module orphan.child", 1);
        let diagnostics = elaborate_source(&orphan_module).expect_err("orphan module must fail");
        assert!(
            diagnostics
                .iter()
                .any(|diagnostic| diagnostic.code == "CC-LANG-MODULE-003")
        );

        let unknown_model = REFERENCE.replacen("model \"spice:R\"", "model \"spice:X\"", 1);
        let diagnostics = crate::frontend::compile_source("model.circuitc", unknown_model)
            .expect_err("unsupported simulator model must fail");
        assert!(
            diagnostics
                .iter()
                .any(|diagnostic| diagnostic.code == "CC-SIM-010")
        );
    }

    #[test]
    fn rejects_symbol_catalog_electrical_type_drift_and_missing_bindings() {
        let electrical_drift = REFERENCE.replacen("bind 1 1 passive;", "bind 1 1 power_output;", 1);
        let expected_start = electrical_drift
            .find("bind 1 1 power_output")
            .map(|start| start + "bind 1 1 ".len())
            .expect("mutated electrical type token exists");
        let diagnostics = elaborate_source(&electrical_drift)
            .expect_err("catalog electrical-type drift must fail");
        let diagnostic = diagnostics
            .iter()
            .find(|diagnostic| diagnostic.code == "CC-LANG-SYMBOL-008")
            .expect("electrical-type catalog diagnostic must exist");
        assert_eq!(diagnostic.start, expected_start);
        assert_eq!(diagnostic.end, expected_start + "power_output".len());

        let missing_binding = REFERENCE.replacen("      bind 2 2 passive;\n", "", 1);
        let diagnostics =
            elaborate_source(&missing_binding).expect_err("missing catalog pin binding must fail");
        assert!(
            diagnostics
                .iter()
                .any(|diagnostic| diagnostic.code == "CC-LANG-SYMBOL-009"),
            "missing CC-LANG-SYMBOL-009: {diagnostics:#?}"
        );
    }

    #[test]
    fn rejects_invalid_and_duplicate_modules_and_ports_with_precise_diagnostics() {
        let invalid_module = REFERENCE.replacen("module divider {", "module .divider {", 1);
        let start = invalid_module
            .find(".divider")
            .expect("invalid module token exists");
        assert_source_diagnostic(
            &invalid_module,
            "CC-LANG-MODULE-001",
            "module instance path is invalid",
            start,
            ".divider",
            0,
        );

        let duplicate_module = REFERENCE.replacen(
            "  module divider.analysis {",
            "  module divider {}\n\n  module divider.analysis {",
            1,
        );
        let start = duplicate_module
            .match_indices("module divider {}")
            .nth(0)
            .map(|(start, _)| start + "module ".len())
            .expect("duplicate module token exists");
        assert_source_diagnostic(
            &duplicate_module,
            "CC-LANG-MODULE-002",
            "duplicate module instance path `divider`",
            start,
            "divider",
            1,
        );

        let invalid_port = REFERENCE.replacen(
            "port input VIN passive connect VIN;",
            "port input café passive connect VIN;",
            1,
        );
        let start = invalid_port
            .find("café")
            .expect("invalid port token exists");
        assert_source_diagnostic(
            &invalid_port,
            "CC-LANG-PORT-001",
            "module port name must be a non-empty canonical ASCII token",
            start,
            "café",
            0,
        );

        let duplicate_port = REFERENCE.replacen(
            "port output VOUT passive connect VOUT;",
            "port output VIN passive connect VOUT;",
            1,
        );
        let start = duplicate_port
            .find("port output VIN")
            .map(|start| start + "port output ".len())
            .expect("duplicate port token exists");
        assert_source_diagnostic(
            &duplicate_port,
            "CC-LANG-PORT-002",
            "duplicate module port `VIN`",
            start,
            "VIN",
            1,
        );

        let invalid_direction = REFERENCE.replacen(
            "port input VIN passive connect VIN;",
            "port sideways VIN passive connect VIN;",
            1,
        );
        let start = invalid_direction
            .find("sideways")
            .expect("invalid direction token exists");
        assert_source_diagnostic(
            &invalid_direction,
            "CC-LANG-PORT-003",
            "unsupported port direction `sideways`; expected `input`, `output`, or `inout`",
            start,
            "sideways",
            0,
        );

        let invalid_pin_type = REFERENCE.replacen(
            "port input VIN passive connect VIN;",
            "port input VIN plasma connect VIN;",
            1,
        );
        let start = invalid_pin_type
            .find("plasma")
            .expect("invalid electrical type token exists");
        assert_source_diagnostic(
            &invalid_pin_type,
            "CC-LANG-PIN-TYPE-001",
            "unsupported electrical pin type `plasma`",
            start,
            "plasma",
            0,
        );
    }

    #[test]
    fn rejects_missing_duplicate_and_incoherent_component_items_with_precise_diagnostics() {
        let missing_parent =
            REFERENCE.replacen("resistor divider.r_top R1", "resistor r_top R1", 1);
        let start = missing_parent
            .find("r_top R1")
            .expect("component path exists");
        assert_source_diagnostic(
            &missing_parent,
            "CC-LANG-COMP-010",
            "component path must include its parent module",
            start,
            "r_top",
            0,
        );

        const PART: &str = "part \"resistor\" manufacturer \"Yageo\" number \"RC0603FR-0710KL\";";
        let missing_part = REFERENCE.replacen(&format!("    {PART}\n"), "", 1);
        let start = missing_part
            .find("divider.r_top")
            .expect("component path exists");
        assert_source_diagnostic(
            &missing_part,
            "CC-LANG-PART-001",
            "component requires one `part` declaration",
            start,
            "divider.r_top",
            0,
        );

        let duplicate_part = REFERENCE.replacen(
            &format!("    {PART}\n"),
            &format!("    {PART}\n    {PART}\n"),
            1,
        );
        let start = duplicate_part
            .match_indices(PART)
            .nth(1)
            .map(|(start, _)| start)
            .expect("duplicate part declaration exists");
        assert_source_diagnostic(
            &duplicate_part,
            "CC-LANG-PART-002",
            "component part is declared more than once",
            start,
            PART,
            1,
        );

        const SYMBOL: &str =
            "symbol \"CircuitC:R\" {\n      bind 1 1 passive;\n      bind 2 2 passive;\n    }";
        let missing_symbol = REFERENCE.replacen(&format!("    {SYMBOL}\n"), "", 1);
        let start = missing_symbol
            .find("divider.r_top")
            .expect("component path exists");
        assert_source_diagnostic(
            &missing_symbol,
            "CC-LANG-SYMBOL-001",
            "component requires one `symbol` declaration",
            start,
            "divider.r_top",
            0,
        );

        let duplicate_symbol = REFERENCE.replacen(
            &format!("    {SYMBOL}\n"),
            &format!("    {SYMBOL}\n    {SYMBOL}\n"),
            1,
        );
        let start = duplicate_symbol
            .match_indices(SYMBOL)
            .nth(1)
            .map(|(start, _)| start)
            .expect("duplicate symbol declaration exists");
        assert_source_diagnostic(
            &duplicate_symbol,
            "CC-LANG-SYMBOL-002",
            "component symbol is declared more than once",
            start,
            SYMBOL,
            1,
        );

        const MODEL: &str = "model \"spice:R\";";
        let duplicate_model = REFERENCE.replacen(
            &format!("    {MODEL}\n"),
            &format!("    {MODEL}\n    {MODEL}\n"),
            1,
        );
        let start = duplicate_model
            .match_indices(MODEL)
            .nth(1)
            .map(|(start, _)| start)
            .expect("duplicate model declaration exists");
        assert_source_diagnostic(
            &duplicate_model,
            "CC-LANG-MODEL-002",
            "component model is declared more than once",
            start,
            MODEL,
            1,
        );

        let duplicate_library_pin = REFERENCE.replacen("bind 2 2 passive;", "bind 2 1 passive;", 1);
        let start = duplicate_library_pin
            .find("bind 2 1 passive;")
            .map(|start| start + "bind 2 ".len())
            .expect("duplicate library pin exists");
        assert_source_diagnostic(
            &duplicate_library_pin,
            "CC-LANG-SYMBOL-005",
            "library symbol pin `1` is bound more than once",
            start,
            "1",
            1,
        );

        let unknown_library_pin = REFERENCE.replacen("bind 2 2 passive;", "bind 2 9 passive;", 1);
        let start = unknown_library_pin
            .find("bind 2 9 passive;")
            .map(|start| start + "bind 2 ".len())
            .expect("unknown library pin exists");
        assert_source_diagnostic(
            &unknown_library_pin,
            "CC-LANG-SYMBOL-006",
            "symbol `CircuitC:R` has no pin `9`",
            start,
            "9",
            0,
        );

        let unknown_logical_pin = REFERENCE.replacen("connect 2 VOUT;", "connect 3 VOUT;", 1);
        let start = unknown_logical_pin
            .find("connect 3 VOUT;")
            .map(|start| start + "connect ".len())
            .expect("unknown logical pin exists");
        assert_source_diagnostic(
            &unknown_logical_pin,
            "CC-LANG-SYMBOL-007",
            "connection references logical pin `3` absent from the symbol binding",
            start,
            "3",
            0,
        );

        const SCHEMATIC: &str = "schematic at (81.28 mm, 81.28 mm) rotation 0 deg;";
        let missing_schematic = REFERENCE.replacen(&format!("    {SCHEMATIC}\n"), "", 1);
        let start = missing_schematic
            .find("divider.r_top")
            .expect("component path exists");
        assert_source_diagnostic(
            &missing_schematic,
            "CC-LANG-SCHEMATIC-001",
            "component requires one `schematic` placement",
            start,
            "divider.r_top",
            0,
        );

        let duplicate_schematic = REFERENCE.replacen(
            &format!("    {SCHEMATIC}\n"),
            &format!("    {SCHEMATIC}\n    {SCHEMATIC}\n"),
            1,
        );
        let start = duplicate_schematic
            .match_indices(SCHEMATIC)
            .nth(1)
            .map(|(start, _)| start)
            .expect("duplicate schematic placement exists");
        assert_source_diagnostic(
            &duplicate_schematic,
            "CC-LANG-SCHEMATIC-002",
            "component schematic placement is declared more than once",
            start,
            SCHEMATIC,
            1,
        );

        let missing_connection = REFERENCE.replacen("    connect 2 VOUT;\n", "", 1);
        let start = missing_connection
            .find("bind 2 2 passive;")
            .map(|start| start + "bind ".len())
            .expect("unconnected symbol pin exists");
        assert_source_diagnostic(
            &missing_connection,
            "CC-LANG-CONNECT-002",
            "symbol logical pin `2` requires `connect` or `no_connect`",
            start,
            "2",
            0,
        );
    }

    #[test]
    fn distinguishes_unknown_and_explicitly_unconnected_simulation_terminals() {
        let unconnected_terminal =
            REFERENCE.replacen("    connect 2 VOUT;", "    no_connect 2;", 1);
        let start = unconnected_terminal
            .find("terminals 1 2;")
            .map(|start| start + "terminals 1 ".len())
            .expect("unconnected terminal token exists");
        assert_source_diagnostic(
            &unconnected_terminal,
            "CC-LANG-SIM-002",
            "simulation terminal references unconnected logical pin `2`",
            start,
            "2",
            0,
        );
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
    fn reports_duplicate_semantic_identities_references_and_symbol_pins() {
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

        let duplicate_symbol_pin = REFERENCE.replacen("bind 2 2 passive;", "bind 1 2 passive;", 1);
        let diagnostics = elaborate_source(&duplicate_symbol_pin)
            .expect_err("duplicate logical symbol pin identities must fail");
        assert!(
            diagnostics
                .iter()
                .any(|diagnostic| diagnostic.code == "CC-LANG-SYMBOL-004")
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
