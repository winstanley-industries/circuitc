use std::collections::{BTreeMap, BTreeSet};

use crate::design::{
    ApprovedSubstitution, Board, CatalogEvidenceRef, Component, ComponentValue, Connection,
    ConnectionState, CopperLayer, DESIGN_SCHEMA_VERSION, Design, Diagnostic, ElectricalPinType,
    LifecycleStatus, ManufacturabilityAnalysis, ManufacturabilityAssertion,
    ManufacturabilityCapability, ModuleInstance, ModulePort, Net, PartIdentity,
    PhysicalImplementation, PinPadBinding, Placement, PointNm, PopulationState, PortDirection,
    ProductConfiguration, ProductIntent, ProductVariant, RectNm, RouteSegment, RoutingRequest,
    SchematicPlacement, SimulationAnalysis, SimulationAnalysisKind, SimulationAssertion,
    SimulationModel, SimulationSample, SizeNm, SourcingConstraints, SymbolBinding,
    SymbolPinBinding, VariantComponent,
};
use crate::quantity::Unit;

use super::diagnostic::{SourceDiagnostic, sort_diagnostics};
use super::quantity::{lower_electrical, lower_length, lower_rotation};
use super::syntax::{
    AutorouteSyntax, BoardItemSyntax, BoardSyntax, CatalogSnapshotSyntax, ComponentItemSyntax,
    ComponentKindSyntax, ComponentSyntax, ConnectionStateSyntax, DeclarationSyntax,
    FootprintItemSyntax, FootprintSyntax, LifecycleSyntax, ManufacturabilitySyntax, ModuleSyntax,
    NetSyntax, PartSyntax, PlacementSyntax, PointSyntax, QuantitySyntax, RectangleSyntax,
    RouteSyntax, SchematicPlacementSyntax, SimulationAnalysisKindSyntax, SimulationAnalysisSyntax,
    SimulationAssertionSyntax, SimulationSampleSyntax, SourceFile, SourcingSyntax, Span,
    SubstituteSyntax, SymbolSyntax, SyntaxTree, VariantItemSyntax, VariantSyntax,
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
    Analysis(String),
    Assertion(String),
    Route(String),
    RoutingRequest(String),
    Footprint(String),
    Placement(String),
    Pad { component: String, pad: String },
}

impl SemanticProvenanceKey {
    fn rendered_path(&self) -> String {
        match self {
            Self::Component(path) | Self::Route(path) | Self::RoutingRequest(path) => path.clone(),
            Self::Analysis(path) => format!("design.analyses.{path}"),
            Self::Assertion(path) => format!("design.assertions.{path}"),
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
        if !matches!(
            &key,
            SemanticProvenanceKey::Analysis(_) | SemanticProvenanceKey::Assertion(_)
        ) {
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
    let mut catalog_syntax = Vec::new();
    let mut variant_syntax = Vec::new();
    let mut manufacturability_syntax = Vec::new();
    let mut analysis_syntax = Vec::new();
    let mut assertion_syntax = Vec::new();
    let mut board_syntax = Vec::new();
    for declaration in &tree.design.declarations {
        match declaration {
            DeclarationSyntax::Net(net) => net_syntax.push(net),
            DeclarationSyntax::Module(module) => module_syntax.push(module),
            DeclarationSyntax::Component(component) => component_syntax.push(component),
            DeclarationSyntax::CatalogSnapshot(snapshot) => catalog_syntax.push(snapshot),
            DeclarationSyntax::Variant(variant) => variant_syntax.push(variant),
            DeclarationSyntax::Manufacturability(analysis) => {
                manufacturability_syntax.push(analysis)
            }
            DeclarationSyntax::SimulationAnalysis(analysis) => analysis_syntax.push(analysis),
            DeclarationSyntax::SimulationAssertion(assertion) => assertion_syntax.push(assertion),
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
    let analyses = elaborate_analyses(source, &analysis_syntax, &mut provenance, &mut diagnostics);
    let assertions =
        elaborate_assertions(source, &assertion_syntax, &mut provenance, &mut diagnostics);
    provenance.insert_structural("design.product", tree.design.span);
    provenance.insert_structural("design.product.catalog", tree.design.span);
    provenance.insert_structural("design.product.variants", tree.design.span);
    provenance.insert_structural(
        "design.product.manufacturability_analyses",
        tree.design.span,
    );
    let product = elaborate_product_intent(
        source,
        &catalog_syntax,
        &variant_syntax,
        &manufacturability_syntax,
        &mut provenance,
        &mut diagnostics,
    );
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
        analyses,
        assertions,
        board: Board {
            outline: board_parts
                .outline
                .expect("a missing rectangle produces a diagnostic"),
            routes: board_parts.routes,
            routing_requests: board_parts.routing_requests,
        },
        product,
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
            let span_for_path = |path: &str| {
                let structural = diagnostic.code.starts_with("CC-IR-")
                    || diagnostic.code.starts_with("CC-NET-")
                    || diagnostic.code.starts_with("CC-MODULE-")
                    || diagnostic.code.starts_with("CC-PORT-")
                    || diagnostic.code.starts_with("CC-BOARD-")
                    || diagnostic.code.starts_with("CC-ROUTE-")
                    || diagnostic.code.starts_with("CC-AUTOROUTE-")
                    || diagnostic.code.starts_with("CC-SIM-")
                    || diagnostic.code.starts_with("CC-PRODUCT-");
                let semantic_span = if diagnostic.code.starts_with("CC-KICAD-") {
                    provenance
                        .kicad_span(path)
                        .or_else(|| provenance.component_span(path))
                } else {
                    provenance.component_span(path)
                };
                if structural {
                    provenance
                        .best_structural_span(path)
                        .or(semantic_span)
                        .or_else(|| provenance.span_for_identity(path))
                } else if diagnostic.code.starts_with("CC-KICAD-") {
                    provenance
                        .structural_spans
                        .get(path)
                        .copied()
                        .or(semantic_span)
                        .or_else(|| provenance.span_for_identity(path))
                } else {
                    semantic_span.or_else(|| provenance.best_structural_span(path))
                }
            };
            let span = span_for_path(&diagnostic.path).unwrap_or(Span::new(0, source.text.len()));
            let related = diagnostic
                .related_path
                .as_deref()
                .and_then(|path| span_for_path(path).map(|span| (path, span)));
            let mut mapped = SourceDiagnostic::new(
                diagnostic.code,
                source,
                span,
                Some(diagnostic.path),
                diagnostic.message,
            );
            if let Some((related_path, related_span)) = related {
                mapped = mapped.with_related(
                    source,
                    related_span,
                    format!("related entity `{related_path}` is here"),
                );
            }
            mapped
        })
        .collect();
    sort_diagnostics(&mut mapped);
    mapped
}

fn elaborate_product_intent(
    source: &SourceFile,
    catalogs: &[&CatalogSnapshotSyntax],
    variants: &[&VariantSyntax],
    analyses: &[&ManufacturabilitySyntax],
    provenance: &mut ProvenanceMap,
    diagnostics: &mut Vec<SourceDiagnostic>,
) -> ProductIntent {
    let catalog = catalogs.first().map(|syntax| {
        provenance.insert_structural("design.product.catalog", syntax.span);
        provenance.insert_structural("design.product.catalog.snapshot_id", syntax.id.span);
        provenance.insert_structural("design.product.catalog.sha256", syntax.sha256.span);
        provenance.insert_structural(
            "design.product.catalog.evaluated_on",
            syntax.evaluated_on.span,
        );
        CatalogEvidenceRef {
            snapshot_id: syntax.id.value.clone(),
            sha256: syntax.sha256.value.clone(),
            evaluated_on: syntax.evaluated_on.value.clone(),
        }
    });
    if let Some(first) = catalogs.first() {
        for duplicate in &catalogs[1..] {
            diagnostics.push(
                SourceDiagnostic::new(
                    "CC-LANG-CATALOG-001",
                    source,
                    duplicate.span,
                    Some("design.product.catalog".to_owned()),
                    "catalog snapshot is declared more than once",
                )
                .with_related(source, first.span, "first declaration is here"),
            );
        }
    }

    let variants = elaborate_variants(source, variants, provenance, diagnostics);
    let manufacturability_analyses =
        elaborate_manufacturability(source, analyses, provenance, diagnostics);
    ProductIntent {
        catalog,
        variants,
        manufacturability_analyses,
    }
}

fn elaborate_variants(
    source: &SourceFile,
    variants: &[&VariantSyntax],
    provenance: &mut ProvenanceMap,
    diagnostics: &mut Vec<SourceDiagnostic>,
) -> Vec<ProductVariant> {
    let mut first_paths = BTreeMap::new();
    variants
        .iter()
        .filter_map(|syntax| {
            let path = syntax.path.value.as_str();
            if let Some(first) = first_paths.get(path).copied() {
                diagnostics.push(
                    SourceDiagnostic::new(
                        "CC-LANG-VARIANT-001",
                        source,
                        syntax.path.span,
                        Some(path.to_owned()),
                        format!("duplicate product variant path `{path}`"),
                    )
                    .with_related(source, first, "first declaration is here"),
                );
                return None;
            }
            first_paths.insert(path, syntax.path.span);
            provenance.insert_structural("design.product.variants", syntax.span);
            provenance.insert_structural(path, syntax.span);
            provenance.insert_structural(format!("{path}.path"), syntax.path.span);
            provenance
                .insert_structural(format!("{path}.build_quantity"), syntax.build_quantity.span);

            let build_quantity = parse_unsigned::<u64>(
                source,
                &syntax.build_quantity,
                "CC-LANG-VARIANT-002",
                path,
                "product variant build quantity must be an exact unsigned 64-bit integer",
                diagnostics,
            )?;

            let mut components = Vec::new();
            let mut configurations = Vec::new();
            let mut component_spans = BTreeMap::new();
            let mut configuration_spans = BTreeMap::new();
            for item in &syntax.items {
                match item {
                    VariantItemSyntax::Fit {
                        component_path,
                        span,
                    }
                    | VariantItemSyntax::NotFitted {
                        component_path,
                        span,
                    } => {
                        if duplicate_variant_component(
                            source,
                            path,
                            component_path,
                            &mut component_spans,
                            diagnostics,
                        ) {
                            continue;
                        }
                        provenance.insert_structural(
                            format!("{path}.components.{}", component_path.value),
                            *span,
                        );
                        components.push(VariantComponent {
                            component_path: component_path.value.clone(),
                            state: if matches!(item, VariantItemSyntax::Fit { .. }) {
                                PopulationState::Fitted
                            } else {
                                PopulationState::NotFitted
                            },
                        });
                    }
                    VariantItemSyntax::Alternate {
                        component_path,
                        manufacturer,
                        manufacturer_part_number,
                        package,
                        span,
                    } => {
                        if duplicate_variant_component(
                            source,
                            path,
                            component_path,
                            &mut component_spans,
                            diagnostics,
                        ) {
                            continue;
                        }
                        provenance.insert_structural(
                            format!("{path}.components.{}", component_path.value),
                            *span,
                        );
                        components.push(VariantComponent {
                            component_path: component_path.value.clone(),
                            state: PopulationState::Alternate(ApprovedSubstitution {
                                manufacturer: manufacturer.value.clone(),
                                manufacturer_part_number: manufacturer_part_number.value.clone(),
                                package: package.value.clone(),
                            }),
                        });
                    }
                    VariantItemSyntax::Configure { key, value, span } => {
                        if let Some(first) = configuration_spans.get(key.value.as_str()).copied() {
                            diagnostics.push(
                                SourceDiagnostic::new(
                                    "CC-LANG-VARIANT-004",
                                    source,
                                    key.span,
                                    Some(path.to_owned()),
                                    format!("duplicate product configuration key `{}`", key.value),
                                )
                                .with_related(
                                    source,
                                    first,
                                    "first configuration is here",
                                ),
                            );
                            continue;
                        }
                        configuration_spans.insert(key.value.as_str(), key.span);
                        provenance.insert_structural(
                            format!("{path}.configurations.{}", key.value),
                            *span,
                        );
                        configurations.push(ProductConfiguration {
                            key: key.value.clone(),
                            value: value.value.clone(),
                        });
                    }
                }
            }

            Some(ProductVariant {
                path: path.to_owned(),
                build_quantity,
                components,
                configurations,
            })
        })
        .collect()
}

fn duplicate_variant_component<'a>(
    source: &SourceFile,
    variant_path: &str,
    component_path: &'a super::syntax::Spanned<String>,
    first_spans: &mut BTreeMap<&'a str, Span>,
    diagnostics: &mut Vec<SourceDiagnostic>,
) -> bool {
    if let Some(first) = first_spans.get(component_path.value.as_str()).copied() {
        diagnostics.push(
            SourceDiagnostic::new(
                "CC-LANG-VARIANT-003",
                source,
                component_path.span,
                Some(variant_path.to_owned()),
                format!(
                    "component `{}` is assigned more than once in product variant `{variant_path}`",
                    component_path.value
                ),
            )
            .with_related(source, first, "first assignment is here"),
        );
        true
    } else {
        first_spans.insert(component_path.value.as_str(), component_path.span);
        false
    }
}

fn elaborate_manufacturability(
    source: &SourceFile,
    analyses: &[&ManufacturabilitySyntax],
    provenance: &mut ProvenanceMap,
    diagnostics: &mut Vec<SourceDiagnostic>,
) -> Vec<ManufacturabilityAnalysis> {
    let mut first_analyses = BTreeMap::new();
    let mut first_assertions = BTreeMap::new();
    analyses
        .iter()
        .filter_map(|syntax| {
            let path = syntax.path.value.as_str();
            if let Some(first) = first_analyses.get(path).copied() {
                diagnostics.push(
                    SourceDiagnostic::new(
                        "CC-LANG-MANUFACTURABILITY-001",
                        source,
                        syntax.path.span,
                        Some(path.to_owned()),
                        format!("duplicate manufacturability analysis path `{path}`"),
                    )
                    .with_related(source, first, "first declaration is here"),
                );
                return None;
            }
            first_analyses.insert(path, syntax.path.span);
            provenance.insert_structural("design.product.manufacturability_analyses", syntax.span);
            provenance.insert_structural(path, syntax.span);
            provenance.insert_structural(format!("{path}.path"), syntax.path.span);
            provenance.insert_structural(format!("{path}.adapter"), syntax.adapter.span);
            provenance.insert_structural(format!("{path}.version"), syntax.version.span);

            let mut assertions = Vec::new();
            for assertion in &syntax.assertions {
                let assertion_path = assertion.path.value.as_str();
                if let Some(first) = first_assertions.get(assertion_path).copied() {
                    diagnostics.push(
                        SourceDiagnostic::new(
                            "CC-LANG-MANUFACTURABILITY-002",
                            source,
                            assertion.path.span,
                            Some(path.to_owned()),
                            format!(
                                "duplicate manufacturability assertion path `{assertion_path}`"
                            ),
                        )
                        .with_related(
                            source,
                            first,
                            "first declaration is here",
                        ),
                    );
                    continue;
                }
                first_assertions.insert(assertion_path, assertion.path.span);
                provenance.insert_structural(assertion_path, assertion.span);
                let capability = match assertion.capability.value.as_str() {
                    "erc_clean" => ManufacturabilityCapability::ErcClean,
                    "drc_clean" => ManufacturabilityCapability::DrcClean,
                    "unconnected_clean" => ManufacturabilityCapability::UnconnectedClean,
                    "schematic_parity_clean" => ManufacturabilityCapability::SchematicParityClean,
                    "fabrication_inventory_complete" => {
                        ManufacturabilityCapability::FabricationInventoryComplete
                    }
                    unsupported => {
                        diagnostics.push(SourceDiagnostic::new(
                            "CC-LANG-MANUFACTURABILITY-003",
                            source,
                            assertion.capability.span,
                            Some(path.to_owned()),
                            format!("unsupported manufacturability capability `{unsupported}`"),
                        ));
                        continue;
                    }
                };
                assertions.push(ManufacturabilityAssertion {
                    path: assertion_path.to_owned(),
                    capability,
                });
            }
            Some(ManufacturabilityAnalysis {
                path: path.to_owned(),
                adapter: syntax.adapter.value.clone(),
                version: syntax.version.value.clone(),
                assertions,
            })
        })
        .collect()
}

fn parse_unsigned<T>(
    source: &SourceFile,
    syntax: &super::syntax::Spanned<String>,
    code: &'static str,
    semantic_path: &str,
    message: &str,
    diagnostics: &mut Vec<SourceDiagnostic>,
) -> Option<T>
where
    T: std::str::FromStr,
{
    match syntax.value.parse() {
        Ok(value) => Some(value),
        Err(_) => {
            diagnostics.push(SourceDiagnostic::new(
                code,
                source,
                syntax.span,
                Some(semantic_path.to_owned()),
                message,
            ));
            None
        }
    }
}

fn elaborate_analyses(
    source: &SourceFile,
    syntax: &[&SimulationAnalysisSyntax],
    provenance: &mut ProvenanceMap,
    diagnostics: &mut Vec<SourceDiagnostic>,
) -> Vec<SimulationAnalysis> {
    let mut first_spans = BTreeMap::new();
    syntax
        .iter()
        .filter_map(|analysis| {
            let path = analysis.path.value.as_str();
            let base = format!("design.analyses.{path}");
            if !semantic_path_is_valid(path) {
                diagnostics.push(SourceDiagnostic::new(
                    "CC-LANG-ANALYSIS-003",
                    source,
                    analysis.path.span,
                    Some(base.clone()),
                    "simulation analysis path is invalid",
                ));
            }
            if let Some(first) = first_spans.get(path).copied() {
                diagnostics.push(
                    SourceDiagnostic::new(
                        "CC-LANG-ANALYSIS-004",
                        source,
                        analysis.path.span,
                        Some(base),
                        format!("duplicate simulation analysis path `{path}`"),
                    )
                    .with_related(source, first, "first declaration is here"),
                );
                return None;
            }
            first_spans.insert(path, analysis.path.span);
            provenance.insert_structural("design.analyses", analysis.span);
            provenance.insert_structural(&base, analysis.span);
            provenance.insert_structural(format!("{base}.path"), analysis.path.span);
            provenance.insert_semantic(
                SemanticProvenanceKey::Analysis(path.to_owned()),
                analysis.span,
            );
            let kind = match &analysis.kind {
                SimulationAnalysisKindSyntax::DcOperatingPoint => {
                    SimulationAnalysisKind::DcOperatingPoint
                }
                SimulationAnalysisKindSyntax::AcLinearSweep {
                    source: ac_source,
                    points,
                    start_frequency,
                    stop_frequency,
                    magnitude,
                    phase,
                } => {
                    provenance.insert_structural(format!("{base}.source"), ac_source.span);
                    provenance.insert_structural(format!("{base}.points"), points.span);
                    provenance
                        .insert_structural(format!("{base}.start_frequency"), start_frequency.span);
                    provenance
                        .insert_structural(format!("{base}.stop_frequency"), stop_frequency.span);
                    provenance.insert_structural(format!("{base}.magnitude"), magnitude.span);
                    provenance.insert_structural(format!("{base}.phase"), phase.span);
                    let points = lower_sweep_points(source, points, &base, diagnostics);
                    let start_frequency = lower_electrical(
                        source,
                        start_frequency,
                        Unit::Hertz,
                        Some(&format!("{base}.start_frequency")),
                        diagnostics,
                    );
                    let stop_frequency = lower_electrical(
                        source,
                        stop_frequency,
                        Unit::Hertz,
                        Some(&format!("{base}.stop_frequency")),
                        diagnostics,
                    );
                    let magnitude = lower_electrical(
                        source,
                        magnitude,
                        Unit::Volt,
                        Some(&format!("{base}.magnitude")),
                        diagnostics,
                    );
                    let phase = lower_electrical(
                        source,
                        phase,
                        Unit::Degree,
                        Some(&format!("{base}.phase")),
                        diagnostics,
                    );
                    let (
                        Some(points),
                        Some(start_frequency),
                        Some(stop_frequency),
                        Some(magnitude),
                        Some(phase),
                    ) = (points, start_frequency, stop_frequency, magnitude, phase)
                    else {
                        return None;
                    };
                    SimulationAnalysisKind::AcLinearSweep {
                        source: ac_source.value.clone(),
                        points,
                        start_frequency,
                        stop_frequency,
                        magnitude,
                        phase,
                    }
                }
                SimulationAnalysisKindSyntax::Transient {
                    step,
                    stop,
                    start,
                    uic,
                } => {
                    provenance.insert_structural(format!("{base}.step"), step.span);
                    provenance.insert_structural(format!("{base}.stop"), stop.span);
                    provenance.insert_structural(format!("{base}.start"), start.span);
                    provenance.insert_structural(format!("{base}.uic"), uic.span);
                    let step = lower_electrical(
                        source,
                        step,
                        Unit::Second,
                        Some(&format!("{base}.step")),
                        diagnostics,
                    );
                    let stop = lower_electrical(
                        source,
                        stop,
                        Unit::Second,
                        Some(&format!("{base}.stop")),
                        diagnostics,
                    );
                    let start = lower_electrical(
                        source,
                        start,
                        Unit::Second,
                        Some(&format!("{base}.start")),
                        diagnostics,
                    );
                    let uic = lower_uic(source, uic, &base, diagnostics);
                    let (Some(step), Some(stop), Some(start), Some(uic)) = (step, stop, start, uic)
                    else {
                        return None;
                    };
                    SimulationAnalysisKind::Transient {
                        step,
                        stop,
                        start,
                        uic,
                    }
                }
            };
            Some(SimulationAnalysis {
                path: analysis.path.value.clone(),
                kind,
            })
        })
        .collect()
}

fn elaborate_assertions(
    source: &SourceFile,
    syntax: &[&SimulationAssertionSyntax],
    provenance: &mut ProvenanceMap,
    diagnostics: &mut Vec<SourceDiagnostic>,
) -> Vec<SimulationAssertion> {
    let mut first_spans = BTreeMap::new();
    syntax
        .iter()
        .filter_map(|assertion| {
            let path = assertion.path.value.as_str();
            let base = format!("design.assertions.{path}");
            if !semantic_path_is_valid(path) {
                diagnostics.push(SourceDiagnostic::new(
                    "CC-LANG-ASSERTION-001",
                    source,
                    assertion.path.span,
                    Some(base.clone()),
                    "simulation assertion path is invalid",
                ));
            }
            if let Some(first) = first_spans.get(path).copied() {
                diagnostics.push(
                    SourceDiagnostic::new(
                        "CC-LANG-ASSERTION-002",
                        source,
                        assertion.path.span,
                        Some(base),
                        format!("duplicate simulation assertion path `{path}`"),
                    )
                    .with_related(source, first, "first declaration is here"),
                );
                return None;
            }
            first_spans.insert(path, assertion.path.span);
            provenance.insert_structural("design.assertions", assertion.span);
            provenance.insert_structural(&base, assertion.span);
            provenance.insert_structural(format!("{base}.path"), assertion.path.span);
            provenance.insert_semantic(
                SemanticProvenanceKey::Assertion(path.to_owned()),
                assertion.span,
            );
            provenance.insert_structural(
                format!("{base}.analysis_path"),
                assertion.analysis_path.span,
            );
            provenance.insert_structural(format!("{base}.net"), assertion.net.span);
            provenance.insert_structural(format!("{base}.sample"), sample_span(&assertion.sample));
            provenance.insert_structural(format!("{base}.expected"), assertion.expected.span);
            provenance.insert_structural(
                format!("{base}.absolute_tolerance"),
                assertion.absolute_tolerance.span,
            );
            provenance.insert_structural(
                format!("{base}.relative_tolerance"),
                assertion.relative_tolerance.span,
            );

            let sample = match &assertion.sample {
                SimulationSampleSyntax::Scalar(_) => Some(SimulationSample::Scalar),
                SimulationSampleSyntax::Frequency {
                    quantity: frequency,
                    ..
                } => lower_electrical(
                    source,
                    frequency,
                    Unit::Hertz,
                    Some(&format!("{base}.sample")),
                    diagnostics,
                )
                .map(SimulationSample::Frequency),
                SimulationSampleSyntax::Time { quantity: time, .. } => lower_electrical(
                    source,
                    time,
                    Unit::Second,
                    Some(&format!("{base}.sample")),
                    diagnostics,
                )
                .map(SimulationSample::Time),
            };
            let expected = lower_electrical(
                source,
                &assertion.expected,
                Unit::Volt,
                Some(&format!("{base}.expected")),
                diagnostics,
            );
            let absolute_tolerance = lower_electrical(
                source,
                &assertion.absolute_tolerance,
                Unit::Volt,
                Some(&format!("{base}.absolute_tolerance")),
                diagnostics,
            );
            let relative_tolerance = lower_electrical(
                source,
                &assertion.relative_tolerance,
                Unit::Dimensionless,
                Some(&format!("{base}.relative_tolerance")),
                diagnostics,
            );
            let (Some(sample), Some(expected), Some(absolute_tolerance), Some(relative_tolerance)) =
                (sample, expected, absolute_tolerance, relative_tolerance)
            else {
                return None;
            };
            Some(SimulationAssertion {
                path: assertion.path.value.clone(),
                analysis_path: assertion.analysis_path.value.clone(),
                net: assertion.net.value.clone(),
                sample,
                expected,
                absolute_tolerance,
                relative_tolerance,
            })
        })
        .collect()
}

fn sample_span(sample: &SimulationSampleSyntax) -> Span {
    match sample {
        SimulationSampleSyntax::Scalar(span) => *span,
        SimulationSampleSyntax::Frequency { span, .. }
        | SimulationSampleSyntax::Time { span, .. } => *span,
    }
}

fn lower_sweep_points(
    source: &SourceFile,
    points: &super::syntax::Spanned<String>,
    base: &str,
    diagnostics: &mut Vec<SourceDiagnostic>,
) -> Option<u32> {
    match points.value.parse::<u32>() {
        Ok(points) => Some(points),
        Err(_) => {
            diagnostics.push(SourceDiagnostic::new(
                "CC-LANG-ANALYSIS-001",
                source,
                points.span,
                Some(format!("{base}.points")),
                "AC linear sweep point count must be an exact unsigned 32-bit integer",
            ));
            None
        }
    }
}

fn lower_uic(
    source: &SourceFile,
    uic: &super::syntax::Spanned<String>,
    base: &str,
    diagnostics: &mut Vec<SourceDiagnostic>,
) -> Option<bool> {
    match uic.value.as_str() {
        "true" => Some(true),
        "false" => Some(false),
        _ => {
            diagnostics.push(SourceDiagnostic::new(
                "CC-LANG-ANALYSIS-002",
                source,
                uic.span,
                Some(format!("{base}.uic")),
                "transient `uic` must be `true` or `false`",
            ));
            None
        }
    }
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
    routing_requests: Vec<RoutingRequest>,
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
    let mut autoroute_syntax = Vec::new();
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
            BoardItemSyntax::Autoroute(request) => autoroute_syntax.push(request),
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

    let mut routing_requests = Vec::new();
    let mut request_paths = BTreeMap::new();
    for syntax in autoroute_syntax {
        let path = syntax.path.value.as_str();
        if !semantic_path_is_valid(path) {
            diagnostics.push(SourceDiagnostic::new(
                "CC-LANG-AUTOROUTE-001",
                source,
                syntax.path.span,
                Some(path.to_owned()),
                "routing request semantic path is invalid",
            ));
        }
        if let Some(first) = request_paths.get(path).copied() {
            diagnostics.push(
                SourceDiagnostic::new(
                    "CC-LANG-AUTOROUTE-002",
                    source,
                    syntax.path.span,
                    Some(path.to_owned()),
                    format!("duplicate routing request semantic path `{path}`"),
                )
                .with_related(source, first, "first routing request is here"),
            );
            continue;
        }
        request_paths.insert(path, syntax.path.span);
        if let Some(request) = lower_routing_request(source, syntax, nets, diagnostics) {
            provenance
                .insert_structural(format!("design.board.routing_requests.{path}"), syntax.span);
            provenance.insert_semantic(
                SemanticProvenanceKey::RoutingRequest(path.to_owned()),
                syntax.span,
            );
            routing_requests.push(request);
        }
    }
    BoardParts {
        outline,
        placements,
        routes,
        routing_requests,
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
    let mut lifecycles = Vec::new();
    let mut sourcing_constraints = Vec::new();
    let mut substitutions = Vec::new();
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
            ComponentItemSyntax::Lifecycle(lifecycle) => lifecycles.push(lifecycle),
            ComponentItemSyntax::Sourcing(sourcing) => sourcing_constraints.push(sourcing),
            ComponentItemSyntax::Substitute(substitute) => substitutions.push(substitute),
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
    let physical_part = part.is_some_and(|part| part.manufacturer.is_some());
    let lifecycle = select_single(
        source,
        &lifecycles,
        syntax.path.span,
        path,
        "CC-LANG-LIFECYCLE-001",
        "CC-LANG-LIFECYCLE-002",
        if physical_part {
            "physical component requires one `lifecycle` declaration"
        } else {
            ""
        },
        "component lifecycle is declared more than once",
        diagnostics,
    )
    .copied();
    let sourcing = select_single(
        source,
        &sourcing_constraints,
        syntax.path.span,
        path,
        "CC-LANG-SOURCING-001",
        "CC-LANG-SOURCING-002",
        if physical_part {
            "physical component requires one `sourcing` declaration"
        } else {
            ""
        },
        "component sourcing constraints are declared more than once",
        diagnostics,
    )
    .copied();
    if !physical_part {
        for lifecycle in &lifecycles {
            diagnostics.push(SourceDiagnostic::new(
                "CC-LANG-LIFECYCLE-003",
                source,
                lifecycle.span,
                Some(path.to_owned()),
                "virtual component must not declare lifecycle intent",
            ));
        }
        for sourcing in &sourcing_constraints {
            diagnostics.push(SourceDiagnostic::new(
                "CC-LANG-SOURCING-003",
                source,
                sourcing.span,
                Some(path.to_owned()),
                "virtual component must not declare sourcing constraints",
            ));
        }
    }
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

    let lowered_part = part.and_then(|part| {
        lower_part(
            source,
            part,
            lifecycle,
            sourcing,
            &substitutions,
            path,
            provenance,
            diagnostics,
        )
    });
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

#[allow(clippy::too_many_arguments)]
fn lower_part(
    source: &SourceFile,
    syntax: &PartSyntax,
    lifecycle: Option<&LifecycleSyntax>,
    sourcing: Option<&SourcingSyntax>,
    substitutions: &[&SubstituteSyntax],
    component_path: &str,
    provenance: &mut ProvenanceMap,
    diagnostics: &mut Vec<SourceDiagnostic>,
) -> Option<PartIdentity> {
    provenance.insert_structural(format!("{component_path}.part"), syntax.span);
    provenance.insert_structural(
        format!("{component_path}.part.logical_function"),
        syntax.logical_function.span,
    );
    if let Some(manufacturer) = &syntax.manufacturer {
        provenance.insert_structural(
            format!("{component_path}.part.manufacturer"),
            manufacturer.span,
        );
    }
    if let Some(number) = &syntax.manufacturer_part_number {
        provenance.insert_structural(
            format!("{component_path}.part.manufacturer_part_number"),
            number.span,
        );
    }
    if let Some(package) = &syntax.package {
        provenance.insert_structural(format!("{component_path}.part.package"), package.span);
    }

    let lifecycle = match lifecycle {
        Some(lifecycle) => {
            provenance
                .insert_structural(format!("{component_path}.part.lifecycle"), lifecycle.span);
            match lifecycle.status.value.as_str() {
                "active" => Some(LifecycleStatus::Active),
                "not_recommended_for_new_designs" => {
                    Some(LifecycleStatus::NotRecommendedForNewDesigns)
                }
                "obsolete" => Some(LifecycleStatus::Obsolete),
                unsupported => {
                    diagnostics.push(SourceDiagnostic::new(
                        "CC-LANG-LIFECYCLE-004",
                        source,
                        lifecycle.status.span,
                        Some(component_path.to_owned()),
                        format!("unsupported component lifecycle status `{unsupported}`"),
                    ));
                    None
                }
            }
        }
        None => None,
    };

    let sourcing = match sourcing {
        Some(sourcing) => {
            provenance.insert_structural(format!("{component_path}.part.sourcing"), sourcing.span);
            let minimum_available_quantity = parse_unsigned::<u64>(
                source,
                &sourcing.minimum_available,
                "CC-LANG-SOURCING-004",
                component_path,
                "sourcing minimum available quantity must be an exact unsigned 64-bit integer",
                diagnostics,
            );
            let maximum_lead_time_days = parse_unsigned::<u32>(
                source,
                &sourcing.maximum_lead_time_days,
                "CC-LANG-SOURCING-005",
                component_path,
                "sourcing maximum lead time days must be an exact unsigned 32-bit integer",
                diagnostics,
            );
            match (minimum_available_quantity, maximum_lead_time_days) {
                (Some(minimum_available_quantity), Some(maximum_lead_time_days)) => {
                    Some(SourcingConstraints {
                        minimum_available_quantity,
                        maximum_lead_time_days,
                        required_region: sourcing.region.value.clone(),
                    })
                }
                _ => None,
            }
        }
        None => None,
    };

    let approved_substitutions = substitutions
        .iter()
        .map(|substitution| {
            provenance.insert_structural(
                format!("{component_path}.part.approved_substitutions"),
                substitution.span,
            );
            ApprovedSubstitution {
                manufacturer: substitution.manufacturer.value.clone(),
                manufacturer_part_number: substitution.manufacturer_part_number.value.clone(),
                package: substitution.package.value.clone(),
            }
        })
        .collect();

    let physical = syntax.manufacturer.is_some();
    if physical && (lifecycle.is_none() || sourcing.is_none()) {
        return None;
    }
    Some(PartIdentity {
        logical_function: syntax.logical_function.value.clone(),
        manufacturer: syntax
            .manufacturer
            .as_ref()
            .map(|manufacturer| manufacturer.value.clone()),
        manufacturer_part_number: syntax
            .manufacturer_part_number
            .as_ref()
            .map(|number| number.value.clone()),
        package: syntax.package.as_ref().map(|package| package.value.clone()),
        lifecycle,
        sourcing,
        approved_substitutions,
    })
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

fn lower_routing_request(
    source: &SourceFile,
    syntax: &AutorouteSyntax,
    nets: &BTreeMap<String, Net>,
    diagnostics: &mut Vec<SourceDiagnostic>,
) -> Option<RoutingRequest> {
    let path = syntax.path.value.as_str();
    if !nets.contains_key(syntax.net.value.as_str()) {
        diagnostics.push(SourceDiagnostic::new(
            "CC-LANG-AUTOROUTE-003",
            source,
            syntax.net.span,
            Some(path.to_owned()),
            format!(
                "routing request references unknown net `{}`",
                syntax.net.value
            ),
        ));
    }
    let width = lower_length(source, &syntax.width, Some(path), diagnostics);
    let clearance = lower_length(source, &syntax.clearance, Some(path), diagnostics);
    let grid_step = lower_length(source, &syntax.grid_step, Some(path), diagnostics);
    for (value, code, span, message) in [
        (
            width,
            "CC-LANG-AUTOROUTE-004",
            syntax.width.span,
            "routing request width must be positive",
        ),
        (
            clearance,
            "CC-LANG-AUTOROUTE-005",
            syntax.clearance.span,
            "routing request clearance must be positive",
        ),
        (
            grid_step,
            "CC-LANG-AUTOROUTE-006",
            syntax.grid_step.span,
            "routing request grid step must be positive",
        ),
    ] {
        if value.is_some_and(|value| value <= 0) {
            diagnostics.push(SourceDiagnostic::new(
                code,
                source,
                span,
                Some(path.to_owned()),
                message,
            ));
        }
    }
    let layer = lower_layer(
        source,
        &syntax.layer.value,
        syntax.layer.span,
        path,
        diagnostics,
    );
    match (width, clearance, grid_step, layer) {
        (Some(width_nm), Some(clearance_nm), Some(grid_step_nm), Some(layer))
            if width_nm > 0 && clearance_nm > 0 && grid_step_nm > 0 =>
        {
            Some(RoutingRequest {
                path: path.to_owned(),
                net: syntax.net.value.clone(),
                width_nm,
                clearance_nm,
                grid_step_nm,
                layer,
            })
        }
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

impl HasSpan for &LifecycleSyntax {
    fn span(&self) -> Span {
        self.span
    }
}

impl HasSpan for &SourcingSyntax {
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
    for (index, analysis) in design.analyses.iter().enumerate() {
        register_indexed_fields(
            provenance,
            &format!("design.analyses.{}", analysis.path),
            &format!("design.analyses[{index}]"),
            &[
                "path",
                "source",
                "points",
                "start_frequency",
                "stop_frequency",
                "magnitude",
                "phase",
                "step",
                "stop",
                "start",
                "uic",
            ],
        );
    }
    for (index, assertion) in design.assertions.iter().enumerate() {
        register_indexed_fields(
            provenance,
            &format!("design.assertions.{}", assertion.path),
            &format!("design.assertions[{index}]"),
            &[
                "path",
                "analysis_path",
                "net",
                "sample",
                "expected",
                "absolute_tolerance",
                "relative_tolerance",
            ],
        );
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
    for (index, request) in design.board.routing_requests.iter().enumerate() {
        if let Some(span) = provenance
            .structural_spans
            .get(&format!("design.board.routing_requests.{}", request.path))
            .copied()
        {
            provenance.insert_structural(format!("design.board.routing_requests[{index}]"), span);
        }
    }
}

fn register_indexed_fields(
    provenance: &mut ProvenanceMap,
    authored_base: &str,
    indexed_base: &str,
    fields: &[&str],
) {
    let authored: Vec<_> = std::iter::once("")
        .chain(fields.iter().copied())
        .filter_map(|field| {
            let suffix = if field.is_empty() {
                String::new()
            } else {
                format!(".{field}")
            };
            provenance
                .structural_spans
                .get(&format!("{authored_base}{suffix}"))
                .copied()
                .map(|span| (suffix, span))
        })
        .collect();
    for (suffix, span) in authored {
        provenance.insert_structural(format!("{indexed_base}{suffix}"), span);
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
    const AUTHORED_ROUTE: &str = "route board.routes.vout_bridge net VOUT from (16 mm, 10 mm) to (24 mm, 10 mm) width 0.25 mm layer front;";
    const AUTOROUTE: &str = "autoroute board.autoroute.vout net VOUT width 0.25 mm clearance 0.2 mm grid 0.25 mm layer front;";

    fn elaborate_source(
        source: &str,
    ) -> Result<super::ElaboratedDesign, Vec<crate::frontend::SourceDiagnostic>> {
        let tree = parse(SourceFile::new("test.circuitc", source))?;
        elaborate(&tree)
    }

    fn with_intent(source: &str, intent: &str) -> String {
        let closing = source.rfind("\n}").expect("reference design closes");
        let mut result = source.to_owned();
        result.insert_str(closing, intent);
        result
    }

    fn autoroute_source() -> String {
        REFERENCE.replacen(AUTHORED_ROUTE, AUTOROUTE, 1)
    }

    fn valid_simulation_source() -> String {
        with_intent(
            REFERENCE,
            r#"
  analysis dc_operating_point sim.dc;
  analysis ac_linear_sweep sim.ac source divider.analysis.input points 11 start_frequency 10 Hz stop_frequency 2.5 kHz magnitude 1 V phase -90 deg;
  analysis transient sim.tran step 2 us stop 10 ms start 0 s uic true;
  assert net_voltage checks.dc analysis sim.dc net VOUT sample scalar expected -5 V absolute_tolerance 0.01 V relative_tolerance 0.001 ratio;
  assert net_voltage checks.ac analysis sim.ac net VOUT sample frequency 1006 Hz expected 5 V absolute_tolerance 0.01 V relative_tolerance 0 ratio;
  assert net_voltage checks.tran analysis sim.tran net VOUT sample time 2 ms expected 5 V absolute_tolerance 0.01 V relative_tolerance 0 ratio;
"#,
        )
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
        assert_eq!(diagnostic.related.len(), 1);
        assert!(source[diagnostic.related[0].start..].starts_with("resistor divider.r_bottom"));
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
    fn product_and_manufacturing_intent_lower_to_typed_design_ir() {
        let elaborated = elaborate_source(REFERENCE).expect("reference source must elaborate");
        let product = &elaborated.design.product;
        let catalog = product
            .catalog
            .as_ref()
            .expect("physical reference design carries pinned catalog evidence");
        assert_eq!(catalog.snapshot_id, "layer1-contract-fixture");
        assert_eq!(catalog.evaluated_on, "2026-08-04");
        assert_eq!(product.variants.len(), 2);
        let alternate = product
            .variants
            .iter()
            .find(|variant| variant.path == "prototype_alternate")
            .expect("alternate-population variant must exist");
        assert_eq!(alternate.build_quantity, 2);
        assert!(alternate.components.iter().any(|component| {
            component.component_path == "divider.r_top"
                && matches!(
                    &component.state,
                    crate::design::PopulationState::Alternate(substitution)
                        if substitution.manufacturer == "Panasonic"
                            && substitution.manufacturer_part_number == "ERJ-3EKF1002V"
                            && substitution.package == "0603_1608Metric"
                )
        }));
        assert_eq!(product.manufacturability_analyses.len(), 1);
        assert!(
            product.manufacturability_analyses[0]
                .assertions
                .iter()
                .any(|assertion| {
                    assertion.capability
                        == crate::design::ManufacturabilityCapability::FabricationInventoryComplete
                })
        );

        let resistor = elaborated
            .design
            .components
            .iter()
            .find(|component| component.path == "divider.r_top")
            .expect("physical resistor must exist");
        assert_eq!(resistor.part.logical_function, "resistor");
        assert_eq!(resistor.part.package.as_deref(), Some("0603_1608Metric"));
        assert_eq!(
            resistor.part.lifecycle,
            Some(crate::design::LifecycleStatus::Active)
        );
        assert_eq!(
            resistor
                .part
                .sourcing
                .as_ref()
                .map(|sourcing| sourcing.minimum_available_quantity),
            Some(1)
        );
        assert_eq!(resistor.part.approved_substitutions.len(), 1);

        let virtual_component = elaborated
            .design
            .components
            .iter()
            .find(|component| component.path == "divider.analysis.input")
            .expect("virtual analysis source must exist");
        assert!(virtual_component.part.manufacturer.is_none());
        assert!(virtual_component.part.package.is_none());
        assert!(virtual_component.part.lifecycle.is_none());
        assert!(virtual_component.part.sourcing.is_none());
    }

    #[test]
    fn physical_product_declarations_are_exactly_once_and_virtual_parts_reject_them() {
        let missing_lifecycle = REFERENCE.replacen("    lifecycle active;\n", "", 1);
        let component_path = missing_lifecycle
            .find("divider.r_top R1")
            .expect("component path exists");
        assert_source_diagnostic(
            &missing_lifecycle,
            "CC-LANG-LIFECYCLE-001",
            "physical component requires one `lifecycle` declaration",
            component_path,
            "divider.r_top",
            0,
        );

        let duplicate_lifecycle = REFERENCE.replacen(
            "    lifecycle active;\n",
            "    lifecycle active;\n    lifecycle obsolete;\n",
            1,
        );
        let duplicate_start = duplicate_lifecycle
            .find("lifecycle obsolete;")
            .expect("duplicate lifecycle exists");
        assert_source_diagnostic(
            &duplicate_lifecycle,
            "CC-LANG-LIFECYCLE-002",
            "component lifecycle is declared more than once",
            duplicate_start,
            "lifecycle obsolete;",
            1,
        );

        let duplicate_sourcing = REFERENCE.replacen(
            "    sourcing minimum_available 1 maximum_lead_time_days 365 region \"global\";\n",
            "    sourcing minimum_available 1 maximum_lead_time_days 365 region \"global\";\n    sourcing minimum_available 2 maximum_lead_time_days 30 region \"US\";\n",
            1,
        );
        let duplicate_start = duplicate_sourcing
            .find("sourcing minimum_available 2")
            .expect("duplicate sourcing exists");
        let duplicate_text =
            "sourcing minimum_available 2 maximum_lead_time_days 30 region \"US\";";
        assert_source_diagnostic(
            &duplicate_sourcing,
            "CC-LANG-SOURCING-002",
            "component sourcing constraints are declared more than once",
            duplicate_start,
            duplicate_text,
            1,
        );

        let virtual_lifecycle = REFERENCE.replacen(
            "    part \"dc_voltage_source\" virtual;\n",
            "    part \"dc_voltage_source\" virtual;\n    lifecycle active;\n",
            1,
        );
        let virtual_component_start = virtual_lifecycle
            .find("dc_source")
            .expect("virtual component exists");
        let forbidden_start = virtual_component_start
            + virtual_lifecycle[virtual_component_start..]
                .find("lifecycle active;")
                .expect("virtual lifecycle exists");
        assert_source_diagnostic(
            &virtual_lifecycle,
            "CC-LANG-LIFECYCLE-003",
            "virtual component must not declare lifecycle intent",
            forbidden_start,
            "lifecycle active;",
            0,
        );
    }

    #[test]
    fn product_literal_failures_have_stable_field_spans() {
        let invalid_quantity = REFERENCE.replacen("build_quantity 10", "build_quantity 1.5", 1);
        let start = invalid_quantity
            .find("1.5")
            .expect("invalid build quantity exists");
        assert_source_diagnostic(
            &invalid_quantity,
            "CC-LANG-VARIANT-002",
            "product variant build quantity must be an exact unsigned 64-bit integer",
            start,
            "1.5",
            0,
        );

        let invalid_lead_time = REFERENCE.replacen(
            "maximum_lead_time_days 365",
            "maximum_lead_time_days 4294967296",
            1,
        );
        let start = invalid_lead_time
            .find("4294967296")
            .expect("out-of-range lead time exists");
        assert_source_diagnostic(
            &invalid_lead_time,
            "CC-LANG-SOURCING-005",
            "sourcing maximum lead time days must be an exact unsigned 32-bit integer",
            start,
            "4294967296",
            0,
        );

        let unsupported = REFERENCE.replacen("capability erc_clean", "capability dfm_clean", 1);
        let start = unsupported
            .find("dfm_clean")
            .expect("unsupported capability exists");
        assert_source_diagnostic(
            &unsupported,
            "CC-LANG-MANUFACTURABILITY-003",
            "unsupported manufacturability capability `dfm_clean`",
            start,
            "dfm_clean",
            0,
        );
    }

    #[test]
    fn design_product_validation_maps_back_to_authored_declarations() {
        let invalid_digest = REFERENCE.replacen(
            "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855",
            "ABC",
            1,
        );
        let diagnostics = crate::frontend::compile_source("catalog.circuitc", &invalid_digest)
            .expect_err("invalid catalog digest must fail Design validation");
        let digest = diagnostics
            .iter()
            .find(|diagnostic| diagnostic.code == "CC-PRODUCT-CATALOG-003")
            .unwrap_or_else(|| panic!("missing catalog digest diagnostic: {diagnostics:#?}"));
        assert_eq!(&invalid_digest[digest.start..digest.end], "\"ABC\"");

        let invalid_adapter = REFERENCE.replacen(
            "adapter \"kicad\" version \"10\"",
            "adapter \"other\" version \"1\"",
            1,
        );
        let diagnostics = crate::frontend::compile_source("adapter.circuitc", &invalid_adapter)
            .expect_err("unsupported adapter must fail Design validation");
        let adapter = diagnostics
            .iter()
            .find(|diagnostic| diagnostic.code == "CC-PRODUCT-ANALYSIS-003")
            .unwrap_or_else(|| panic!("missing adapter diagnostic: {diagnostics:#?}"));
        assert!(invalid_adapter[adapter.start..adapter.end].starts_with("manufacturability "));
    }

    #[test]
    fn source_rejects_self_and_package_incompatible_alternates() {
        const SUBSTITUTE: &str = "substitute manufacturer \"Panasonic\" number \"ERJ-3EKF1002V\" package \"0603_1608Metric\";";
        const ALTERNATE: &str = "alternate divider.r_top manufacturer \"Panasonic\" number \"ERJ-3EKF1002V\" package \"0603_1608Metric\";";

        let self_substitution = REFERENCE
            .replacen(
                SUBSTITUTE,
                "substitute manufacturer \"Yageo\" number \"RC0603FR-0710KL\" package \"0603_1608Metric\";",
                1,
            )
            .replacen(
                ALTERNATE,
                "alternate divider.r_top manufacturer \"Yageo\" number \"RC0603FR-0710KL\" package \"0603_1608Metric\";",
                1,
            );
        let diagnostics =
            crate::frontend::compile_source("self-substitution.circuitc", &self_substitution)
                .expect_err("a primary part must not be represented as an alternate");
        let diagnostic = diagnostics
            .iter()
            .find(|diagnostic| diagnostic.code == "CC-PRODUCT-PART-011")
            .unwrap_or_else(|| panic!("missing self-substitution diagnostic: {diagnostics:#?}"));
        assert_eq!(diagnostic.semantic_path.as_deref(), Some("divider.r_top"));

        let wrong_package = REFERENCE
            .replacen(
                SUBSTITUTE,
                "substitute manufacturer \"Panasonic\" number \"ERJ-3EKF1002V\" package \"0805_2012Metric\";",
                1,
            )
            .replacen(
                ALTERNATE,
                "alternate divider.r_top manufacturer \"Panasonic\" number \"ERJ-3EKF1002V\" package \"0805_2012Metric\";",
                1,
            );
        let diagnostics = crate::frontend::compile_source("wrong-package.circuitc", &wrong_package)
            .expect_err("an alternate package must match the component's fixed package");
        let diagnostic = diagnostics
            .iter()
            .find(|diagnostic| diagnostic.code == "CC-PRODUCT-PART-010")
            .unwrap_or_else(|| {
                panic!("missing package-compatibility diagnostic: {diagnostics:#?}")
            });
        assert_eq!(diagnostic.semantic_path.as_deref(), Some("divider.r_top"));
    }

    #[test]
    fn source_product_configuration_resource_limits_fail_closed() {
        let configurations = (0..257)
            .map(|index| format!("    configure option_{index} \"enabled\";"))
            .collect::<Vec<_>>()
            .join("\n");
        let oversized_collection =
            REFERENCE.replacen("    configure assembly_revision \"A\";", &configurations, 1);
        let diagnostics =
            crate::frontend::compile_source("configuration-count.circuitc", &oversized_collection)
                .expect_err("a variant configuration collection above its limit must fail");
        assert!(
            diagnostics
                .iter()
                .any(|diagnostic| diagnostic.code == "CC-PRODUCT-CONFIG-004"),
            "missing configuration-count diagnostic: {diagnostics:#?}"
        );

        let oversized_key = "k".repeat(129);
        let source = REFERENCE.replacen(
            "configure assembly_revision \"A\";",
            &format!("configure {oversized_key} \"A\";"),
            1,
        );
        let diagnostics = crate::frontend::compile_source("configuration-key.circuitc", &source)
            .expect_err("an oversized configuration key must fail");
        assert!(
            diagnostics
                .iter()
                .any(|diagnostic| diagnostic.code == "CC-PRODUCT-CONFIG-005"),
            "missing configuration-key diagnostic: {diagnostics:#?}"
        );

        let oversized_value = "v".repeat(4097);
        let source = REFERENCE.replacen(
            "configure assembly_revision \"A\";",
            &format!("configure assembly_revision \"{oversized_value}\";"),
            1,
        );
        let diagnostics = crate::frontend::compile_source("configuration-value.circuitc", &source)
            .expect_err("an oversized configuration value must fail");
        assert!(
            diagnostics
                .iter()
                .any(|diagnostic| diagnostic.code == "CC-PRODUCT-CONFIG-006"),
            "missing configuration-value diagnostic: {diagnostics:#?}"
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
    part "resistor" manufacturer "Yageo" number "RC0603FR-0710KL" package "0603_1608Metric";
    lifecycle active;
    sourcing minimum_available 1 maximum_lead_time_days 365 region "global";
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

  catalog_snapshot "physical-only" sha256 "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855" evaluated_on "2026-08-04";
  variant production build_quantity 1 {
    fit board_only.unused;
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
                crate::frontend::syntax::DeclarationSyntax::Net(_)
                | crate::frontend::syntax::DeclarationSyntax::CatalogSnapshot(_)
                | crate::frontend::syntax::DeclarationSyntax::Variant(_)
                | crate::frontend::syntax::DeclarationSyntax::Manufacturability(_)
                | crate::frontend::syntax::DeclarationSyntax::SimulationAnalysis(_)
                | crate::frontend::syntax::DeclarationSyntax::SimulationAssertion(_) => {}
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

        const PART: &str = "part \"resistor\" manufacturer \"Yageo\" number \"RC0603FR-0710KL\" package \"0603_1608Metric\";";
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
    fn reports_duplicate_catalog_snapshots_with_related_location() {
        let first_start = REFERENCE
            .find("catalog_snapshot ")
            .expect("catalog declaration exists");
        let declaration_end = REFERENCE[first_start..]
            .find(';')
            .map(|offset| first_start + offset + 1)
            .expect("catalog declaration ends");
        let declaration = &REFERENCE[first_start..declaration_end];
        let source = REFERENCE.replacen(declaration, &format!("{declaration}\n  {declaration}"), 1);
        let duplicate_start = source.rfind(declaration).expect("duplicate catalog exists");
        let diagnostics = elaborate_source(&source).expect_err("duplicate catalog must fail");
        let duplicate = diagnostics
            .iter()
            .find(|diagnostic| diagnostic.code == "CC-LANG-CATALOG-001")
            .expect("duplicate catalog diagnostic must exist");

        assert_eq!(
            duplicate.message,
            "catalog snapshot is declared more than once"
        );
        assert_eq!(duplicate.start, duplicate_start);
        assert_eq!(&source[duplicate.start..duplicate.end], declaration);
        assert_eq!(duplicate.related.len(), 1);
        assert_eq!(duplicate.related[0].start, first_start);
        assert_eq!(duplicate.related[0].end, declaration_end);
        assert_eq!(
            &source[duplicate.related[0].start..duplicate.related[0].end],
            declaration
        );
        assert_eq!(duplicate.related[0].message, "first declaration is here");
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
    fn reports_every_autoroute_elaboration_diagnostic() {
        let reference = autoroute_source();
        let duplicate = reference.replacen(AUTOROUTE, &format!("{AUTOROUTE}\n    {AUTOROUTE}"), 1);
        let mutants = [
            (
                reference.replacen("autoroute board.autoroute.vout", "autoroute .invalid", 1),
                "CC-LANG-AUTOROUTE-001",
            ),
            (duplicate, "CC-LANG-AUTOROUTE-002"),
            (
                reference.replacen(
                    "autoroute board.autoroute.vout net VOUT",
                    "autoroute board.autoroute.vout net UNKNOWN",
                    1,
                ),
                "CC-LANG-AUTOROUTE-003",
            ),
            (
                reference.replacen("width 0.25 mm", "width 0 mm", 1),
                "CC-LANG-AUTOROUTE-004",
            ),
            (
                reference.replacen("clearance 0.2 mm", "clearance 0 mm", 1),
                "CC-LANG-AUTOROUTE-005",
            ),
            (
                reference.replacen("grid 0.25 mm", "grid 0 mm", 1),
                "CC-LANG-AUTOROUTE-006",
            ),
        ];
        for (source, code) in mutants {
            let diagnostics = match elaborate_source(&source) {
                Ok(_) => panic!("mutant {code} unexpectedly elaborated"),
                Err(diagnostics) => diagnostics,
            };
            assert!(
                diagnostics.iter().any(|diagnostic| diagnostic.code == code),
                "missing {code}: {diagnostics:#?}"
            );
        }
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

    #[test]
    fn elaborates_all_explicit_simulation_intent_without_floating_point() {
        let source = valid_simulation_source();
        let elaborated = elaborate_source(&source).expect("simulation intent must elaborate");
        assert_eq!(elaborated.design.analyses.len(), 3);
        assert_eq!(elaborated.design.assertions.len(), 3);

        let ac = elaborated
            .design
            .analyses
            .iter()
            .find(|analysis| analysis.path == "sim.ac")
            .expect("AC analysis exists");
        let crate::design::SimulationAnalysisKind::AcLinearSweep {
            points,
            start_frequency,
            stop_frequency,
            magnitude,
            phase,
            ..
        } = &ac.kind
        else {
            panic!("sim.ac must lower to a linear AC sweep");
        };
        assert_eq!(*points, 11);
        assert_eq!(
            *start_frequency,
            crate::quantity::Quantity::new(10, 0, crate::quantity::Unit::Hertz)
        );
        assert_eq!(
            *stop_frequency,
            crate::quantity::Quantity::new(25, 2, crate::quantity::Unit::Hertz)
        );
        assert_eq!(
            *magnitude,
            crate::quantity::Quantity::new(1, 0, crate::quantity::Unit::Volt)
        );
        assert_eq!(
            *phase,
            crate::quantity::Quantity::new(-90, 0, crate::quantity::Unit::Degree)
        );

        assert!(matches!(
            &elaborated.design.assertions[0].sample,
            crate::design::SimulationSample::Frequency(_)
                | crate::design::SimulationSample::Scalar
                | crate::design::SimulationSample::Time(_)
        ));
    }

    #[test]
    fn legacy_source_does_not_gain_an_implicit_analysis() {
        let elaborated = elaborate_source(REFERENCE).expect("reference source must elaborate");
        assert!(elaborated.design.analyses.is_empty());
        assert!(elaborated.design.assertions.is_empty());
    }

    #[test]
    fn simulation_declaration_order_and_equivalent_suffixes_do_not_change_ir() {
        let source = with_intent(
            REFERENCE,
            r#"
  analysis ac_linear_sweep sim.ac source divider.analysis.input points 11 start_frequency 1 kHz stop_frequency 2.5 kHz magnitude 1 V phase 0 deg;
  analysis transient sim.tran step 1 ms stop 10 ms start 0 s uic false;
  assert net_voltage checks.ac analysis sim.ac net VOUT sample frequency 1 kHz expected 5 V absolute_tolerance 0.01 V relative_tolerance 0.01 ratio;
"#,
        );
        let expected = elaborate_source(&source).expect("intent must elaborate");
        let equivalent = source
            .replace("start_frequency 1 kHz", "start_frequency 1000 Hz")
            .replace("step 1 ms", "step 1000 us")
            .replace("0.01 ratio", "0.0100 ratio");
        assert_eq!(
            elaborate_source(&equivalent)
                .expect("equivalent exact suffixes must elaborate")
                .design,
            expected.design
        );

        let mut syntax =
            parse(SourceFile::new("permuted.circuitc", &source)).expect("intent source must parse");
        syntax.design.declarations.reverse();
        assert_eq!(
            elaborate(&syntax)
                .expect("permuted intent must elaborate")
                .design,
            expected.design
        );
    }

    #[test]
    fn simulation_quantity_diagnostics_render_stably_for_humans_and_json() {
        let source = with_intent(
            REFERENCE,
            "\n  analysis ac_linear_sweep sim.ac source divider.analysis.input points 11 start_frequency 1 ms stop_frequency 1 kHz magnitude 1 V phase 0 deg;\n",
        );
        let diagnostics = elaborate_source(&source)
            .expect_err("a time-valued start frequency must fail elaboration");
        let diagnostic = diagnostics
            .iter()
            .find(|diagnostic| diagnostic.code == "CC-LANG-QUANTITY-005")
            .expect("dimension diagnostic exists");
        assert_eq!(
            diagnostic.semantic_path.as_deref(),
            Some("design.analyses.sim.ac.start_frequency")
        );
        assert_eq!(&source[diagnostic.start..diagnostic.end], "ms");

        let human = crate::frontend::render_diagnostics(
            &diagnostics,
            crate::frontend::DiagnosticFormat::Human,
        );
        let json = crate::frontend::render_diagnostics(
            &diagnostics,
            crate::frontend::DiagnosticFormat::Json,
        );
        assert!(human.contains("CC-LANG-QUANTITY-005 [design.analyses.sim.ac.start_frequency]"));
        assert!(json.contains("\"code\": \"CC-LANG-QUANTITY-005\""));
        assert!(json.contains("\"semantic_path\": \"design.analyses.sim.ac.start_frequency\""));
    }

    fn full_simulation_diagnostic_source() -> &'static str {
        r#"design sim_gold {
  net N;
  ground GND;
  module root {}
  resistor root.r R1 {
    part "resistor" manufacturer "X" number "Y" package "0603_1608Metric";
    lifecycle active;
    sourcing minimum_available 1 maximum_lead_time_days 365 region "global";
    symbol "CircuitC:R" {
      bind 1 1 passive;
      bind 2 2 passive;
    }
    schematic at (1 mm, 0 mm) rotation 0 deg;
    resistance 1 kohm;
    connect 1 N;
    connect 2 GND;
    footprint "CircuitC:R_0603_1608Metric" {
      bind 1 1;
      bind 2 2;
    }
  }
  dc_source root.v V1 {
    part "dc_voltage_source" virtual;
    symbol "CircuitC:VDC" {
      bind p 1 passive;
      bind n 2 passive;
    }
    model "spice:Vdc";
    schematic at (0 mm, 0 mm) rotation 0 deg;
    voltage 1 V;
    terminals p n;
    connect p N;
    connect n GND;
  }
  catalog_snapshot "simulation-golden" sha256 "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855" evaluated_on "2026-08-04";
  variant production build_quantity 1 {
    fit root.r;
  }
  board {
    rectangle at (0 mm, 0 mm) size (2 mm, 2 mm);
    place R1 at (1 mm, 1 mm) rotation 0 deg layer front;
  }
  analysis ac_linear_sweep sim.points source root.v points 1 start_frequency 10 Hz stop_frequency 100 Hz magnitude 1 V phase 0 deg;
  analysis ac_linear_sweep sim.unknown_source source root.missing points 2 start_frequency 10 Hz stop_frequency 100 Hz magnitude 1 V phase 0 deg;
  analysis ac_linear_sweep sim.non_voltage source root.r points 2 start_frequency 10 Hz stop_frequency 100 Hz magnitude 1 V phase 0 deg;
  analysis ac_linear_sweep sim.start_zero source root.v points 2 start_frequency 0 Hz stop_frequency 100 Hz magnitude 1 V phase 0 deg;
  analysis ac_linear_sweep sim.stop_zero source root.v points 2 start_frequency -2 Hz stop_frequency -1 Hz magnitude 1 V phase 0 deg;
  analysis ac_linear_sweep sim.reversed source root.v points 2 start_frequency 100 Hz stop_frequency 10 Hz magnitude 1 V phase 0 deg;
  analysis ac_linear_sweep sim.negative_magnitude source root.v points 2 start_frequency 10 Hz stop_frequency 100 Hz magnitude -1 V phase 0 deg;
  analysis ac_linear_sweep sim.ac_grid source root.v points 3 start_frequency 10 Hz stop_frequency 100 Hz magnitude 1 V phase 0 deg;
  analysis transient sim.zero_step step 0 s stop 1 s start 0 s uic false;
  analysis transient sim.zero_stop step 1 s stop 0 s start 0 s uic false;
  analysis transient sim.negative_start step 1 s stop 1 s start -1 s uic false;
  analysis transient sim.reversed_tran step 1 s stop 1 s start 2 s uic false;
  analysis transient sim.oversized_grid step 1 us stop 11 ms start 0 s uic false;
  analysis transient sim.tran_grid step 0.3 s stop 1 s start 0.1 s uic false;
  analysis dc_operating_point sim.dc;
  assert net_voltage checks.invalid_analysis_path analysis .bad net N sample scalar expected 0 V absolute_tolerance 0 V relative_tolerance 0 ratio;
  assert net_voltage checks.unknown_analysis analysis sim.missing net N sample scalar expected 0 V absolute_tolerance 0 V relative_tolerance 0 ratio;
  assert net_voltage checks.unknown_net analysis sim.dc net MISSING sample scalar expected 0 V absolute_tolerance 0 V relative_tolerance 0 ratio;
  assert net_voltage checks.kind_mismatch analysis sim.ac_grid net N sample scalar expected 0 V absolute_tolerance 0 V relative_tolerance 0 ratio;
  assert net_voltage checks.ac_outside analysis sim.ac_grid net N sample frequency 5 Hz expected 0 V absolute_tolerance 0 V relative_tolerance 0 ratio;
  assert net_voltage checks.ac_off_grid analysis sim.ac_grid net N sample frequency 56 Hz expected 0 V absolute_tolerance 0 V relative_tolerance 0 ratio;
  assert net_voltage checks.tran_outside analysis sim.tran_grid net N sample time 1.2 s expected 0 V absolute_tolerance 0 V relative_tolerance 0 ratio;
  assert net_voltage checks.tran_off_grid analysis sim.tran_grid net N sample time 0.4 s expected 0 V absolute_tolerance 0 V relative_tolerance 0 ratio;
  assert net_voltage checks.negative_absolute analysis sim.dc net N sample scalar expected 0 V absolute_tolerance -1 V relative_tolerance 0 ratio;
  assert net_voltage checks.negative_relative analysis sim.dc net N sample scalar expected 0 V absolute_tolerance 0 V relative_tolerance -1 ratio;
}
"#
    }

    fn simulation_count_limit_source(analysis_count: usize, assertion_count: usize) -> String {
        use std::fmt::Write as _;

        let mut source = String::from(
            "design count_limits {\n  net N;\n  ground GND;\n  module root {}\n  board {\n    rectangle at (0 mm, 0 mm) size (1 mm, 1 mm);\n  }\n",
        );
        if assertion_count != 0 {
            source.push_str("  analysis dc_operating_point sim.dc;\n");
        }
        for index in 0..analysis_count {
            writeln!(source, "  analysis dc_operating_point sim.a{index:03};").unwrap();
        }
        for index in 0..assertion_count {
            writeln!(
                source,
                "  assert net_voltage checks.a{index:05} analysis sim.dc net N sample scalar expected 0 V absolute_tolerance 0 V relative_tolerance 0 ratio;"
            )
            .unwrap();
        }
        source.push_str("}\n");
        source
    }

    #[test]
    fn source_reachable_simulation_diagnostics_have_full_exact_goldens() {
        let source = full_simulation_diagnostic_source();
        let diagnostics = crate::frontend::compile_source("i10-main.circuitc", source)
            .expect_err("the comprehensive invalid simulation fixture must fail");

        // Unit/path shape failures are rejected as CC-LANG diagnostics before Design IR exists;
        // this fixture exhausts the CC-SIM analysis/assertion/capability branches reachable from
        // well-typed source, including every distinct reachable message for shared codes.
        let expected_codes = [
            "CC-SIM-CAPABILITY-001",
            "CC-SIM-ANALYSIS-012",
            "CC-SIM-ANALYSIS-003",
            "CC-SIM-ANALYSIS-004",
            "CC-SIM-ANALYSIS-004",
            "CC-SIM-ANALYSIS-005",
            "CC-SIM-ANALYSIS-005",
            "CC-SIM-ANALYSIS-005",
            "CC-SIM-ANALYSIS-005",
            "CC-SIM-ANALYSIS-006",
            "CC-SIM-ANALYSIS-009",
            "CC-SIM-ANALYSIS-009",
            "CC-SIM-ANALYSIS-009",
            "CC-SIM-ANALYSIS-009",
            "CC-SIM-ANALYSIS-010",
            "CC-SIM-ASSERTION-003",
            "CC-SIM-ASSERTION-003",
            "CC-SIM-ASSERTION-004",
            "CC-SIM-ASSERTION-005",
            "CC-SIM-ASSERTION-007",
            "CC-SIM-ASSERTION-007",
            "CC-SIM-ASSERTION-007",
            "CC-SIM-ASSERTION-007",
            "CC-SIM-ASSERTION-009",
            "CC-SIM-ASSERTION-010",
        ];
        assert_eq!(
            diagnostics
                .iter()
                .map(|diagnostic| diagnostic.code)
                .collect::<Vec<_>>(),
            expected_codes
        );

        let tree = parse(SourceFile::new("i10-main.circuitc", source))
            .expect("the golden fixture must parse");
        let elaborated = elaborate(&tree).expect("the golden fixture must elaborate");
        let mut permuted = elaborated.design.clone();
        permuted.analyses.reverse();
        permuted.assertions.reverse();
        let permuted_diagnostics = super::map_ir_diagnostics(
            &tree.source,
            &elaborated.provenance,
            permuted
                .validate()
                .expect_err("permuted invalid intent must still fail"),
        );
        assert_eq!(permuted_diagnostics, diagnostics);

        assert_eq!(
            crate::frontend::render_diagnostics(
                &diagnostics,
                crate::frontend::DiagnosticFormat::Human,
            ),
            r###"i10-main.circuitc:5:3: CC-SIM-CAPABILITY-001 [root.r]: electrically participating component has no supported explicit simulation model (bytes 60..526)
  related i10-main.circuitc:50:3: related entity `design.analyses.sim.ac_grid` is here (bytes 2101..2231)
i10-main.circuitc:43:3: CC-SIM-ANALYSIS-012 [design.analyses]: aggregate declared simulation grid exceeds 10000 nominal samples (bytes 1138..1267)
i10-main.circuitc:43:60: CC-SIM-ANALYSIS-003 [design.analyses.sim.points.points]: AC sweep points must be in 2..=10000; found 1 (bytes 1195..1196)
i10-main.circuitc:44:54: CC-SIM-ANALYSIS-004 [design.analyses.sim.unknown_source.source]: AC source references unknown component root.missing (bytes 1321..1333)
i10-main.circuitc:45:51: CC-SIM-ANALYSIS-004 [design.analyses.sim.non_voltage.source]: AC source root.r must select a component with a DC voltage-source simulation model (bytes 1464..1470)
  related i10-main.circuitc:5:3: related entity `root.r` is here (bytes 60..526)
i10-main.circuitc:46:82: CC-SIM-ANALYSIS-005 [design.analyses.sim.start_zero.start_frequency]: AC start frequency must be positive (bytes 1632..1636)
i10-main.circuitc:47:81: CC-SIM-ANALYSIS-005 [design.analyses.sim.stop_zero.start_frequency]: AC start frequency must be positive (bytes 1766..1771)
i10-main.circuitc:47:102: CC-SIM-ANALYSIS-005 [design.analyses.sim.stop_zero.stop_frequency]: AC stop frequency must be positive (bytes 1787..1792)
i10-main.circuitc:48:102: CC-SIM-ANALYSIS-005 [design.analyses.sim.reversed.stop_frequency]: AC stop frequency must be greater than its start frequency (bytes 1921..1926)
i10-main.circuitc:49:128: CC-SIM-ANALYSIS-006 [design.analyses.sim.negative_magnitude.magnitude]: AC source magnitude must be non-negative (bytes 2081..2085)
i10-main.circuitc:51:41: CC-SIM-ANALYSIS-009 [design.analyses.sim.zero_step.step]: transient step must be positive (bytes 2272..2275)
i10-main.circuitc:52:50: CC-SIM-ANALYSIS-009 [design.analyses.sim.zero_stop.stop]: transient stop must be positive (bytes 2355..2358)
i10-main.circuitc:53:65: CC-SIM-ANALYSIS-009 [design.analyses.sim.negative_start.start]: transient start must be non-negative (bytes 2444..2448)
i10-main.circuitc:54:64: CC-SIM-ANALYSIS-009 [design.analyses.sim.reversed_tran.start]: transient start must not be greater than its stop (bytes 2523..2526)
i10-main.circuitc:55:46: CC-SIM-ANALYSIS-010 [design.analyses.sim.oversized_grid.step]: inclusive transient grid exceeds 10000 samples (bytes 2583..2587)
i10-main.circuitc:58:60: CC-SIM-ASSERTION-003 [design.assertions.checks.invalid_analysis_path.analysis_path]: assertion analysis path must be a canonical semantic path (bytes 2795..2799)
i10-main.circuitc:59:55: CC-SIM-ASSERTION-003 [design.assertions.checks.unknown_analysis.analysis_path]: assertion references unknown analysis sim.missing (bytes 2938..2949)
i10-main.circuitc:60:61: CC-SIM-ASSERTION-004 [design.assertions.checks.unknown_net.net]: assertion references unknown or invalid net MISSING (bytes 3094..3101)
i10-main.circuitc:61:77: CC-SIM-ASSERTION-005 [design.assertions.checks.kind_mismatch.sample]: assertion sample kind does not match its analysis kind (bytes 3256..3262)
  related i10-main.circuitc:50:3: related entity `design.analyses.sim.ac_grid` is here (bytes 2101..2231)
i10-main.circuitc:62:74: CC-SIM-ASSERTION-007 [design.assertions.checks.ac_outside.sample]: AC assertion sample must lie inside the inclusive sweep range (bytes 3400..3414)
  related i10-main.circuitc:50:3: related entity `design.analyses.sim.ac_grid` is here (bytes 2101..2231)
i10-main.circuitc:63:75: CC-SIM-ASSERTION-007 [design.assertions.checks.ac_off_grid.sample]: AC assertion sample must exactly equal a generated linear-grid sample (bytes 3553..3568)
  related i10-main.circuitc:50:3: related entity `design.analyses.sim.ac_grid` is here (bytes 2101..2231)
i10-main.circuitc:64:78: CC-SIM-ASSERTION-007 [design.assertions.checks.tran_outside.sample]: transient assertion sample must lie inside the inclusive analysis interval (bytes 3710..3720)
  related i10-main.circuitc:56:3: related entity `design.analyses.sim.tran_grid` is here (bytes 2622..2697)
i10-main.circuitc:65:79: CC-SIM-ASSERTION-007 [design.assertions.checks.tran_off_grid.sample]: transient assertion sample must exactly equal a zero-anchored step sample or the forced stop endpoint (bytes 3863..3873)
  related i10-main.circuitc:56:3: related entity `design.analyses.sim.tran_grid` is here (bytes 2622..2697)
i10-main.circuitc:66:115: CC-SIM-ASSERTION-009 [design.assertions.checks.negative_absolute.absolute_tolerance]: assertion absolute tolerance must be non-negative (bytes 4052..4056)
i10-main.circuitc:67:138: CC-SIM-ASSERTION-010 [design.assertions.checks.negative_relative.relative_tolerance]: assertion relative tolerance must be non-negative (bytes 4222..4230)"###
        );
        assert_eq!(
            crate::frontend::render_diagnostics(
                &diagnostics,
                crate::frontend::DiagnosticFormat::Json,
            ),
            r###"[
  {
    "code": "CC-SIM-CAPABILITY-001",
    "filename": "i10-main.circuitc",
    "start": 60,
    "end": 526,
    "line": 5,
    "column": 3,
    "semantic_path": "root.r",
    "message": "electrically participating component has no supported explicit simulation model",
    "related": [
      {
        "filename": "i10-main.circuitc",
        "start": 2101,
        "end": 2231,
        "line": 50,
        "column": 3,
        "message": "related entity `design.analyses.sim.ac_grid` is here"
      }
    ]
  },
  {
    "code": "CC-SIM-ANALYSIS-012",
    "filename": "i10-main.circuitc",
    "start": 1138,
    "end": 1267,
    "line": 43,
    "column": 3,
    "semantic_path": "design.analyses",
    "message": "aggregate declared simulation grid exceeds 10000 nominal samples",
    "related": []
  },
  {
    "code": "CC-SIM-ANALYSIS-003",
    "filename": "i10-main.circuitc",
    "start": 1195,
    "end": 1196,
    "line": 43,
    "column": 60,
    "semantic_path": "design.analyses.sim.points.points",
    "message": "AC sweep points must be in 2..=10000; found 1",
    "related": []
  },
  {
    "code": "CC-SIM-ANALYSIS-004",
    "filename": "i10-main.circuitc",
    "start": 1321,
    "end": 1333,
    "line": 44,
    "column": 54,
    "semantic_path": "design.analyses.sim.unknown_source.source",
    "message": "AC source references unknown component root.missing",
    "related": []
  },
  {
    "code": "CC-SIM-ANALYSIS-004",
    "filename": "i10-main.circuitc",
    "start": 1464,
    "end": 1470,
    "line": 45,
    "column": 51,
    "semantic_path": "design.analyses.sim.non_voltage.source",
    "message": "AC source root.r must select a component with a DC voltage-source simulation model",
    "related": [
      {
        "filename": "i10-main.circuitc",
        "start": 60,
        "end": 526,
        "line": 5,
        "column": 3,
        "message": "related entity `root.r` is here"
      }
    ]
  },
  {
    "code": "CC-SIM-ANALYSIS-005",
    "filename": "i10-main.circuitc",
    "start": 1632,
    "end": 1636,
    "line": 46,
    "column": 82,
    "semantic_path": "design.analyses.sim.start_zero.start_frequency",
    "message": "AC start frequency must be positive",
    "related": []
  },
  {
    "code": "CC-SIM-ANALYSIS-005",
    "filename": "i10-main.circuitc",
    "start": 1766,
    "end": 1771,
    "line": 47,
    "column": 81,
    "semantic_path": "design.analyses.sim.stop_zero.start_frequency",
    "message": "AC start frequency must be positive",
    "related": []
  },
  {
    "code": "CC-SIM-ANALYSIS-005",
    "filename": "i10-main.circuitc",
    "start": 1787,
    "end": 1792,
    "line": 47,
    "column": 102,
    "semantic_path": "design.analyses.sim.stop_zero.stop_frequency",
    "message": "AC stop frequency must be positive",
    "related": []
  },
  {
    "code": "CC-SIM-ANALYSIS-005",
    "filename": "i10-main.circuitc",
    "start": 1921,
    "end": 1926,
    "line": 48,
    "column": 102,
    "semantic_path": "design.analyses.sim.reversed.stop_frequency",
    "message": "AC stop frequency must be greater than its start frequency",
    "related": []
  },
  {
    "code": "CC-SIM-ANALYSIS-006",
    "filename": "i10-main.circuitc",
    "start": 2081,
    "end": 2085,
    "line": 49,
    "column": 128,
    "semantic_path": "design.analyses.sim.negative_magnitude.magnitude",
    "message": "AC source magnitude must be non-negative",
    "related": []
  },
  {
    "code": "CC-SIM-ANALYSIS-009",
    "filename": "i10-main.circuitc",
    "start": 2272,
    "end": 2275,
    "line": 51,
    "column": 41,
    "semantic_path": "design.analyses.sim.zero_step.step",
    "message": "transient step must be positive",
    "related": []
  },
  {
    "code": "CC-SIM-ANALYSIS-009",
    "filename": "i10-main.circuitc",
    "start": 2355,
    "end": 2358,
    "line": 52,
    "column": 50,
    "semantic_path": "design.analyses.sim.zero_stop.stop",
    "message": "transient stop must be positive",
    "related": []
  },
  {
    "code": "CC-SIM-ANALYSIS-009",
    "filename": "i10-main.circuitc",
    "start": 2444,
    "end": 2448,
    "line": 53,
    "column": 65,
    "semantic_path": "design.analyses.sim.negative_start.start",
    "message": "transient start must be non-negative",
    "related": []
  },
  {
    "code": "CC-SIM-ANALYSIS-009",
    "filename": "i10-main.circuitc",
    "start": 2523,
    "end": 2526,
    "line": 54,
    "column": 64,
    "semantic_path": "design.analyses.sim.reversed_tran.start",
    "message": "transient start must not be greater than its stop",
    "related": []
  },
  {
    "code": "CC-SIM-ANALYSIS-010",
    "filename": "i10-main.circuitc",
    "start": 2583,
    "end": 2587,
    "line": 55,
    "column": 46,
    "semantic_path": "design.analyses.sim.oversized_grid.step",
    "message": "inclusive transient grid exceeds 10000 samples",
    "related": []
  },
  {
    "code": "CC-SIM-ASSERTION-003",
    "filename": "i10-main.circuitc",
    "start": 2795,
    "end": 2799,
    "line": 58,
    "column": 60,
    "semantic_path": "design.assertions.checks.invalid_analysis_path.analysis_path",
    "message": "assertion analysis path must be a canonical semantic path",
    "related": []
  },
  {
    "code": "CC-SIM-ASSERTION-003",
    "filename": "i10-main.circuitc",
    "start": 2938,
    "end": 2949,
    "line": 59,
    "column": 55,
    "semantic_path": "design.assertions.checks.unknown_analysis.analysis_path",
    "message": "assertion references unknown analysis sim.missing",
    "related": []
  },
  {
    "code": "CC-SIM-ASSERTION-004",
    "filename": "i10-main.circuitc",
    "start": 3094,
    "end": 3101,
    "line": 60,
    "column": 61,
    "semantic_path": "design.assertions.checks.unknown_net.net",
    "message": "assertion references unknown or invalid net MISSING",
    "related": []
  },
  {
    "code": "CC-SIM-ASSERTION-005",
    "filename": "i10-main.circuitc",
    "start": 3256,
    "end": 3262,
    "line": 61,
    "column": 77,
    "semantic_path": "design.assertions.checks.kind_mismatch.sample",
    "message": "assertion sample kind does not match its analysis kind",
    "related": [
      {
        "filename": "i10-main.circuitc",
        "start": 2101,
        "end": 2231,
        "line": 50,
        "column": 3,
        "message": "related entity `design.analyses.sim.ac_grid` is here"
      }
    ]
  },
  {
    "code": "CC-SIM-ASSERTION-007",
    "filename": "i10-main.circuitc",
    "start": 3400,
    "end": 3414,
    "line": 62,
    "column": 74,
    "semantic_path": "design.assertions.checks.ac_outside.sample",
    "message": "AC assertion sample must lie inside the inclusive sweep range",
    "related": [
      {
        "filename": "i10-main.circuitc",
        "start": 2101,
        "end": 2231,
        "line": 50,
        "column": 3,
        "message": "related entity `design.analyses.sim.ac_grid` is here"
      }
    ]
  },
  {
    "code": "CC-SIM-ASSERTION-007",
    "filename": "i10-main.circuitc",
    "start": 3553,
    "end": 3568,
    "line": 63,
    "column": 75,
    "semantic_path": "design.assertions.checks.ac_off_grid.sample",
    "message": "AC assertion sample must exactly equal a generated linear-grid sample",
    "related": [
      {
        "filename": "i10-main.circuitc",
        "start": 2101,
        "end": 2231,
        "line": 50,
        "column": 3,
        "message": "related entity `design.analyses.sim.ac_grid` is here"
      }
    ]
  },
  {
    "code": "CC-SIM-ASSERTION-007",
    "filename": "i10-main.circuitc",
    "start": 3710,
    "end": 3720,
    "line": 64,
    "column": 78,
    "semantic_path": "design.assertions.checks.tran_outside.sample",
    "message": "transient assertion sample must lie inside the inclusive analysis interval",
    "related": [
      {
        "filename": "i10-main.circuitc",
        "start": 2622,
        "end": 2697,
        "line": 56,
        "column": 3,
        "message": "related entity `design.analyses.sim.tran_grid` is here"
      }
    ]
  },
  {
    "code": "CC-SIM-ASSERTION-007",
    "filename": "i10-main.circuitc",
    "start": 3863,
    "end": 3873,
    "line": 65,
    "column": 79,
    "semantic_path": "design.assertions.checks.tran_off_grid.sample",
    "message": "transient assertion sample must exactly equal a zero-anchored step sample or the forced stop endpoint",
    "related": [
      {
        "filename": "i10-main.circuitc",
        "start": 2622,
        "end": 2697,
        "line": 56,
        "column": 3,
        "message": "related entity `design.analyses.sim.tran_grid` is here"
      }
    ]
  },
  {
    "code": "CC-SIM-ASSERTION-009",
    "filename": "i10-main.circuitc",
    "start": 4052,
    "end": 4056,
    "line": 66,
    "column": 115,
    "semantic_path": "design.assertions.checks.negative_absolute.absolute_tolerance",
    "message": "assertion absolute tolerance must be non-negative",
    "related": []
  },
  {
    "code": "CC-SIM-ASSERTION-010",
    "filename": "i10-main.circuitc",
    "start": 4222,
    "end": 4230,
    "line": 67,
    "column": 138,
    "semantic_path": "design.assertions.checks.negative_relative.relative_tolerance",
    "message": "assertion relative tolerance must be non-negative",
    "related": []
  }
]
"###
        );
    }

    #[test]
    fn simulation_collection_limits_have_full_exact_goldens() {
        for (filename, source, expected_human, expected_json) in [
            (
                "i10-analysis-limit.circuitc",
                simulation_count_limit_source(257, 0),
                r###"i10-analysis-limit.circuitc:8:3: CC-SIM-ANALYSIS-011 [design.analyses]: design declares 257 analyses; the maximum is 256 (bytes 127..164)"###,
                r###"[
  {
    "code": "CC-SIM-ANALYSIS-011",
    "filename": "i10-analysis-limit.circuitc",
    "start": 127,
    "end": 164,
    "line": 8,
    "column": 3,
    "semantic_path": "design.analyses",
    "message": "design declares 257 analyses; the maximum is 256",
    "related": []
  }
]
"###,
            ),
            (
                "i10-assertion-limit.circuitc",
                simulation_count_limit_source(0, 10_001),
                r###"i10-assertion-limit.circuitc:9:3: CC-SIM-ASSERTION-011 [design.assertions]: design declares 10001 assertions; the maximum is 10000 (bytes 165..297)"###,
                r###"[
  {
    "code": "CC-SIM-ASSERTION-011",
    "filename": "i10-assertion-limit.circuitc",
    "start": 165,
    "end": 297,
    "line": 9,
    "column": 3,
    "semantic_path": "design.assertions",
    "message": "design declares 10001 assertions; the maximum is 10000",
    "related": []
  }
]
"###,
            ),
        ] {
            let diagnostics = crate::frontend::compile_source(filename, &source)
                .expect_err("over-limit source intent must fail");
            assert_eq!(
                crate::frontend::render_diagnostics(
                    &diagnostics,
                    crate::frontend::DiagnosticFormat::Human,
                ),
                expected_human
            );
            assert_eq!(
                crate::frontend::render_diagnostics(
                    &diagnostics,
                    crate::frontend::DiagnosticFormat::Json,
                ),
                expected_json
            );
        }
    }

    #[test]
    fn valid_authored_dc_analysis_maps_the_fail_closed_phase_guard() {
        let declaration = "analysis dc_operating_point sim.dc;";
        let source = with_intent(REFERENCE, &format!("\n  {declaration}\n"));
        let diagnostics = crate::frontend::compile_source("phase.circuitc", &source)
            .expect_err("declared intent must stop before the legacy backend");
        assert_eq!(diagnostics.len(), 1, "phase guard must be the sole failure");
        let diagnostic = &diagnostics[0];
        assert_eq!(diagnostic.code, "CC-SIM-PHASE-001");
        assert_eq!(
            diagnostic.semantic_path.as_deref(),
            Some("design.analyses.sim.dc")
        );
        assert_eq!(
            diagnostic.message,
            "the static-only compile entry point does not execute declared simulation analyses; use checked compilation"
        );
        assert_eq!(&source[diagnostic.start..diagnostic.end], declaration);
    }

    #[test]
    fn backend_schedule_diagnostics_map_to_exact_authored_fields() {
        for (declaration, semantic_path, primary, related, expected_human, expected_json) in [
            (
                "analysis ac_linear_sweep sim.ac source divider.analysis.input points 2 start_frequency 9007199254740992 Hz stop_frequency 9007199254740993 Hz magnitude 1 V phase 0 deg;",
                "design.analyses.sim.ac.stop_frequency",
                "9007199254740993 Hz",
                "9007199254740992 Hz",
                r###"schedule.circuitc:101:125: CC-SIM-LOWER-002 [design.analyses.sim.ac.stop_frequency]: distinct exact AC sweep endpoints collapse or reverse at the backend f64 boundary (bytes 3311..3330)
  related schedule.circuitc:101:90: related entity `design.analyses.sim.ac.start_frequency` is here (bytes 3276..3295)"###,
                r###"[
  {
    "code": "CC-SIM-LOWER-002",
    "filename": "schedule.circuitc",
    "start": 3311,
    "end": 3330,
    "line": 101,
    "column": 125,
    "semantic_path": "design.analyses.sim.ac.stop_frequency",
    "message": "distinct exact AC sweep endpoints collapse or reverse at the backend f64 boundary",
    "related": [
      {
        "filename": "schedule.circuitc",
        "start": 3276,
        "end": 3295,
        "line": 101,
        "column": 90,
        "message": "related entity `design.analyses.sim.ac.start_frequency` is here"
      }
    ]
  }
]
"###,
            ),
            (
                "analysis transient sim.tran step 9007199254740992 s stop 9007199254740993 s start 0 s uic false;",
                "design.analyses.sim.tran.stop",
                "9007199254740993 s",
                "9007199254740992 s",
                r###"schedule.circuitc:101:60: CC-SIM-LOWER-002 [design.analyses.sim.tran.stop]: distinct exact transient controls collapse to one value at the backend f64 boundary (bytes 3246..3264)
  related schedule.circuitc:101:36: related entity `design.analyses.sim.tran.step` is here (bytes 3222..3240)"###,
                r###"[
  {
    "code": "CC-SIM-LOWER-002",
    "filename": "schedule.circuitc",
    "start": 3246,
    "end": 3264,
    "line": 101,
    "column": 60,
    "semantic_path": "design.analyses.sim.tran.stop",
    "message": "distinct exact transient controls collapse to one value at the backend f64 boundary",
    "related": [
      {
        "filename": "schedule.circuitc",
        "start": 3222,
        "end": 3240,
        "line": 101,
        "column": 36,
        "message": "related entity `design.analyses.sim.tran.step` is here"
      }
    ]
  }
]
"###,
            ),
            (
                "analysis ac_linear_sweep sim.ac source divider.analysis.input points 3 start_frequency 9007199254740992 Hz stop_frequency 9007199254740994 Hz magnitude 1 V phase 0 deg;",
                "design.analyses.sim.ac.points",
                "3",
                "9007199254740992 Hz",
                r###"schedule.circuitc:101:72: CC-SIM-LOWER-002 [design.analyses.sim.ac.points]: the pinned backend AC schedule is non-finite, duplicate, or non-increasing (bytes 3258..3259)
  related schedule.circuitc:101:90: related entity `design.analyses.sim.ac.start_frequency` is here (bytes 3276..3295)"###,
                r###"[
  {
    "code": "CC-SIM-LOWER-002",
    "filename": "schedule.circuitc",
    "start": 3258,
    "end": 3259,
    "line": 101,
    "column": 72,
    "semantic_path": "design.analyses.sim.ac.points",
    "message": "the pinned backend AC schedule is non-finite, duplicate, or non-increasing",
    "related": [
      {
        "filename": "schedule.circuitc",
        "start": 3276,
        "end": 3295,
        "line": 101,
        "column": 90,
        "message": "related entity `design.analyses.sim.ac.start_frequency` is here"
      }
    ]
  }
]
"###,
            ),
        ] {
            let source = with_intent(REFERENCE, &format!("\n  {declaration}\n"));
            let diagnostics = crate::frontend::compile_source("schedule.circuitc", &source)
                .expect_err("a backend-unrepresentable schedule must fail before execution");
            assert_eq!(diagnostics.len(), 1);
            let diagnostic = &diagnostics[0];
            assert_eq!(diagnostic.code, "CC-SIM-LOWER-002");
            assert_eq!(diagnostic.semantic_path.as_deref(), Some(semantic_path));
            assert_eq!(&source[diagnostic.start..diagnostic.end], primary);
            assert_eq!(diagnostic.related.len(), 1);
            let related_span = &diagnostic.related[0];
            assert_eq!(&source[related_span.start..related_span.end], related);
            assert_eq!(
                crate::frontend::render_diagnostics(
                    &diagnostics,
                    crate::frontend::DiagnosticFormat::Human,
                ),
                expected_human
            );
            assert_eq!(
                crate::frontend::render_diagnostics(
                    &diagnostics,
                    crate::frontend::DiagnosticFormat::Json,
                ),
                expected_json
            );
        }
    }

    #[test]
    fn collapsed_transient_assertion_diagnostic_has_an_exact_golden() {
        let source = with_intent(
            REFERENCE,
            r#"
  analysis transient sim.tran step 4503599627370496 s stop 9007199254740993 s start 0 s uic false;
  assert net_voltage checks.multiple analysis sim.tran net VOUT sample time 9007199254740992 s expected 5 V absolute_tolerance 0.01 V relative_tolerance 0 ratio;
"#,
        );
        let diagnostics = crate::frontend::compile_source("sample-collapse.circuitc", &source)
            .expect_err("distinct authored times may not alias at the backend boundary");
        assert_eq!(diagnostics.len(), 1);
        assert_eq!(diagnostics[0].code, "CC-SIM-LOWER-003");
        assert_eq!(
            crate::frontend::render_diagnostics(
                &diagnostics,
                crate::frontend::DiagnosticFormat::Human,
            ),
            r###"sample-collapse.circuitc:102:72: CC-SIM-LOWER-003 [design.assertions.checks.multiple.sample]: distinct exact transient assertion or control times collapse to one value at the backend f64 boundary (bytes 3357..3380)
  related sample-collapse.circuitc:101:60: related entity `design.analyses.sim.tran.stop` is here (bytes 3246..3264)"###
        );
        assert_eq!(
            crate::frontend::render_diagnostics(
                &diagnostics,
                crate::frontend::DiagnosticFormat::Json,
            ),
            r###"[
  {
    "code": "CC-SIM-LOWER-003",
    "filename": "sample-collapse.circuitc",
    "start": 3357,
    "end": 3380,
    "line": 102,
    "column": 72,
    "semantic_path": "design.assertions.checks.multiple.sample",
    "message": "distinct exact transient assertion or control times collapse to one value at the backend f64 boundary",
    "related": [
      {
        "filename": "sample-collapse.circuitc",
        "start": 3246,
        "end": 3264,
        "line": 101,
        "column": 60,
        "message": "related entity `design.analyses.sim.tran.stop` is here"
      }
    ]
  }
]
"###
        );
    }

    #[test]
    fn aggregate_simulation_resource_diagnostic_has_exact_source_goldens() {
        let source = with_intent(
            REFERENCE,
            r#"
  analysis dc_operating_point sim.dc;
"#,
        );
        let tree = parse(SourceFile::new("resource.circuitc", &source))
            .expect("resource fixture must parse");
        let elaborated = elaborate(&tree).expect("resource fixture must elaborate");
        let diagnostics = crate::simulation::lower::lower_inputs_with_limit(&elaborated.design, 0)
            .expect_err("a zero generated-artifact limit must reject the simulation bundle");
        let diagnostics =
            super::map_ir_diagnostics(&tree.source, &elaborated.provenance, diagnostics);
        assert_eq!(diagnostics.len(), 1);
        assert_eq!(diagnostics[0].code, "CC-SIM-LOWER-005");
        assert_eq!(
            crate::frontend::render_diagnostics(
                &diagnostics,
                crate::frontend::DiagnosticFormat::Human,
            ),
            r###"resource.circuitc:101:3: CC-SIM-LOWER-005 [design.analyses]: deterministic simulation inputs exceed the 0-byte aggregate generated-artifact budget (bytes 3189..3224)"###
        );
        assert_eq!(
            crate::frontend::render_diagnostics(
                &diagnostics,
                crate::frontend::DiagnosticFormat::Json,
            ),
            r###"[
  {
    "code": "CC-SIM-LOWER-005",
    "filename": "resource.circuitc",
    "start": 3189,
    "end": 3224,
    "line": 101,
    "column": 3,
    "semantic_path": "design.analyses",
    "message": "deterministic simulation inputs exceed the 0-byte aggregate generated-artifact budget",
    "related": []
  }
]
"###
        );
    }

    #[test]
    fn simulation_capability_diagnostic_maps_component_and_first_analysis_spans() {
        let declaration = "analysis dc_operating_point sim.dc;";
        let source = with_intent(REFERENCE, &format!("\n  {declaration}\n"))
            .replacen("    model \"spice:R\";\n", "", 1)
            .replacen("    terminals 1 2;\n", "", 1);
        let diagnostics = crate::frontend::compile_source("capability.circuitc", &source)
            .expect_err("connected physical-only component must fail static capability validation");
        let diagnostic = diagnostics
            .iter()
            .find(|diagnostic| diagnostic.code == "CC-SIM-CAPABILITY-001")
            .unwrap_or_else(|| panic!("missing capability diagnostic: {diagnostics:#?}"));
        assert_eq!(diagnostic.semantic_path.as_deref(), Some("divider.r_top"));
        assert_eq!(
            diagnostic.message,
            "electrically participating component has no supported explicit simulation model"
        );
        let component_start = source
            .find("resistor divider.r_top R1 {")
            .expect("fixture contains first resistor");
        let component_end = source
            .find("\n\n  resistor divider.r_bottom R2 {")
            .expect("fixture contains second resistor separator");
        assert_eq!(
            (diagnostic.start, diagnostic.end),
            (component_start, component_end)
        );
        assert_eq!(diagnostic.related.len(), 1);
        let related = &diagnostic.related[0];
        let analysis_start = source.find(declaration).expect("fixture contains analysis");
        assert_eq!(
            (related.start, related.end),
            (analysis_start, analysis_start + declaration.len())
        );
        assert_eq!(
            related.message,
            "related entity `design.analyses.sim.dc` is here"
        );
    }

    #[test]
    fn simulation_paths_reject_invalid_and_duplicate_identities_with_related_spans() {
        let source = with_intent(
            REFERENCE,
            r#"
  analysis dc_operating_point .invalid;
  analysis dc_operating_point sim.same;
  analysis dc_operating_point sim.same;
  assert net_voltage .invalid analysis sim.same net VOUT sample scalar expected 5 V absolute_tolerance 0 V relative_tolerance 0 ratio;
  assert net_voltage checks.same analysis sim.same net VOUT sample scalar expected 5 V absolute_tolerance 0 V relative_tolerance 0 ratio;
  assert net_voltage checks.same analysis sim.same net VOUT sample scalar expected 5 V absolute_tolerance 0 V relative_tolerance 0 ratio;
"#,
        );
        let diagnostics = elaborate_source(&source).expect_err("invalid and duplicate paths fail");
        for code in [
            "CC-LANG-ANALYSIS-003",
            "CC-LANG-ANALYSIS-004",
            "CC-LANG-ASSERTION-001",
            "CC-LANG-ASSERTION-002",
        ] {
            let diagnostic = diagnostics
                .iter()
                .find(|diagnostic| diagnostic.code == code)
                .unwrap_or_else(|| panic!("missing {code}: {diagnostics:#?}"));
            if code.ends_with("004") || code.ends_with("002") {
                assert_eq!(
                    diagnostic.related.len(),
                    1,
                    "{code} must retain first declaration"
                );
            }
        }

        let expected = [
            (
                "CC-LANG-ANALYSIS-003",
                ".invalid",
                "simulation analysis path is invalid",
            ),
            (
                "CC-LANG-ANALYSIS-004",
                "sim.same",
                "duplicate simulation analysis path `sim.same`",
            ),
            (
                "CC-LANG-ASSERTION-001",
                ".invalid",
                "simulation assertion path is invalid",
            ),
            (
                "CC-LANG-ASSERTION-002",
                "checks.same",
                "duplicate simulation assertion path `checks.same`",
            ),
        ];
        for (code, authored_path, message) in expected {
            let diagnostic = diagnostics
                .iter()
                .find(|diagnostic| diagnostic.code == code)
                .expect("diagnostic exists");
            assert_eq!(&source[diagnostic.start..diagnostic.end], authored_path);
            assert_eq!(diagnostic.message, message);
            assert!(diagnostic.semantic_path.is_some());
        }
    }

    #[test]
    fn simulation_literal_lowering_diagnostics_are_exact_and_deterministic() {
        let source = with_intent(
            REFERENCE,
            r#"
  analysis ac_linear_sweep sim.ac source divider.analysis.input points 4294967296 start_frequency 10 Hz stop_frequency 1 kHz magnitude 1 V phase 0 deg;
  analysis transient sim.tran step 2 us stop 10 ms start 0 s uic maybe;
"#,
        );
        let diagnostics = elaborate_source(&source)
            .expect_err("out-of-range points and non-boolean uic must fail elaboration");
        let expected = [
            (
                "CC-LANG-ANALYSIS-001",
                "design.analyses.sim.ac.points",
                "4294967296",
                "AC linear sweep point count must be an exact unsigned 32-bit integer",
            ),
            (
                "CC-LANG-ANALYSIS-002",
                "design.analyses.sim.tran.uic",
                "maybe",
                "transient `uic` must be `true` or `false`",
            ),
        ];
        for (code, path, authored, message) in expected {
            let diagnostic = diagnostics
                .iter()
                .find(|diagnostic| diagnostic.code == code)
                .unwrap_or_else(|| panic!("missing {code}: {diagnostics:#?}"));
            assert_eq!(diagnostic.semantic_path.as_deref(), Some(path));
            assert_eq!(&source[diagnostic.start..diagnostic.end], authored);
            assert_eq!(diagnostic.message, message);
        }
        let first = crate::frontend::render_diagnostics(
            &diagnostics,
            crate::frontend::DiagnosticFormat::Json,
        );
        let second = crate::frontend::render_diagnostics(
            &diagnostics,
            crate::frontend::DiagnosticFormat::Json,
        );
        assert_eq!(first, second);
    }

    #[test]
    fn simulation_path_collision_does_not_erase_component_kicad_provenance() {
        let source = with_intent(
            REFERENCE,
            "\n  analysis dc_operating_point divider.r_top;\n",
        );
        let elaborated = elaborate_source(&source).expect("cross-kind identity is namespaced");
        let component_start = source
            .find("resistor divider.r_top")
            .expect("fixture contains component");
        let span = elaborated
            .provenance
            .span_for_identity("divider.r_top")
            .expect("component KiCad provenance remains unique");
        assert_eq!(span.start, component_start);
        assert!(
            elaborated
                .provenance
                .span_for("design.analyses.divider.r_top")
                .is_some()
        );

        let mut adversarial = ProvenanceMap {
            semantic_spans: std::collections::BTreeMap::new(),
            rendered_semantic_spans: std::collections::BTreeMap::new(),
            identity_owner_spans: std::collections::BTreeMap::new(),
            route_spans: std::collections::BTreeMap::new(),
            structural_spans: std::collections::BTreeMap::new(),
        };
        let component_span = crate::frontend::Span::new(10, 20);
        adversarial.insert_semantic(
            SemanticProvenanceKey::Component("design.analyses.foo".to_owned()),
            component_span,
        );
        adversarial.insert_semantic(
            SemanticProvenanceKey::Analysis("foo".to_owned()),
            crate::frontend::Span::new(30, 40),
        );
        assert_eq!(
            adversarial.span_for_identity("design.analyses.foo"),
            Some(component_span),
            "analysis namespace must not participate in KiCad identity ownership"
        );
    }
}
