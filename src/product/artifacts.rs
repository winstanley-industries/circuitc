use std::collections::{BTreeMap, BTreeSet};
use std::fmt;
use std::io::{self, Write};
use std::marker::PhantomData;

use serde::de::{DeserializeOwned, IgnoredAny, SeqAccess, Visitor};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use crate::RelativeArtifactPath;
use crate::design::{
    Component, ComponentValue, CopperLayer, Design, LifecycleStatus, PopulationState,
    ProductConfiguration, ProductVariant,
};

use super::catalog::{CatalogDiagnostic, CatalogResolution, verify_product_catalog};

const SCHEMA_VERSION: u32 = 1;
const RESOLUTION_SCHEMA: &str = "circuitc.product_resolution";
const BOM_SCHEMA: &str = "circuitc.bom";
const PLACEMENT_SCHEMA: &str = "circuitc.placement";
const ASSEMBLY_SCHEMA: &str = "circuitc.assembly";
const MAX_ARTIFACT_BYTES: usize = 64 * 1024 * 1024;
const MAX_ARTIFACT_ROWS: usize = 10_000;
const MAX_DIAGNOSTICS: usize = 256;
const ROW_LIMIT_MARKER: &str = "circuitc-product-artifact-row-limit";

const VARIANT_IDENTITY_DOMAIN: &[u8] = b"CIRCUITC-PRODUCT-VARIANT-IDENTITY-V1\0";
const PRODUCT_INPUT_DOMAIN: &[u8] = b"CIRCUITC-PRODUCT-INPUT-V1\0";

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ProductArtifactDiagnostic {
    pub code: &'static str,
    pub path: String,
    pub message: String,
}

impl fmt::Display for ProductArtifactDiagnostic {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(formatter, "{} [{}]: {}", self.code, self.path, self.message)
    }
}

impl std::error::Error for ProductArtifactDiagnostic {}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ProductArtifactBundle {
    pub variant_path: String,
    pub variant_identity_sha256: String,
    pub resolution_path: RelativeArtifactPath,
    pub bom_path: RelativeArtifactPath,
    pub placement_path: RelativeArtifactPath,
    pub assembly_path: RelativeArtifactPath,
    pub resolution_json: String,
    pub bom_json: String,
    pub placement_json: String,
    pub assembly_json: String,
}

#[derive(Clone, Debug, Eq, Ord, PartialEq, PartialOrd, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Configuration {
    key: String,
    value: String,
}

#[derive(Clone, Debug, Eq, Ord, PartialEq, PartialOrd, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct ExactPartIdentity {
    logical_function: String,
    manufacturer: String,
    manufacturer_part_number: String,
    package: String,
    value: ExactValue,
}

#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
enum ValueKind {
    Resistance,
    DcVoltage,
}

#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
enum ValueUnit {
    Ohm,
    Volt,
}

#[derive(Clone, Debug, Eq, Ord, PartialEq, PartialOrd, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct ExactValue {
    kind: ValueKind,
    coefficient: i64,
    exponent: i8,
    unit: ValueUnit,
}

#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
enum ArtifactPopulationState {
    Fitted,
    NotFitted,
    Alternate,
}

#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
enum BoardSide {
    Front,
    Back,
}

/// A nullable field whose presence is mandatory even when its value is null.
#[derive(Clone, Debug, Eq, Ord, PartialEq, PartialOrd, Serialize, Deserialize)]
#[serde(transparent)]
struct RequiredNullable<T>(Option<T>);

#[derive(Clone, Debug, Eq, Ord, PartialEq, PartialOrd, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct ResolutionRow {
    component_path: String,
    reference: String,
    state: ArtifactPopulationState,
    base_identity: ExactPartIdentity,
    #[serde(deserialize_with = "deserialize_required_nullable")]
    selected_identity: RequiredNullable<ExactPartIdentity>,
}

#[derive(Clone, Debug, Eq, Ord, PartialEq, PartialOrd, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct BomRow {
    selected_identity: ExactPartIdentity,
    per_board_quantity: u64,
    total_quantity: u64,
}

#[derive(Clone, Debug, Eq, Ord, PartialEq, PartialOrd, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct PlacementRow {
    component_path: String,
    reference: String,
    selected_identity: ExactPartIdentity,
    x_nm: i64,
    y_nm: i64,
    rotation_degrees: i16,
    side: BoardSide,
}

#[derive(Clone, Debug, Eq, Ord, PartialEq, PartialOrd, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct AssemblyRow {
    component_path: String,
    reference: String,
    state: ArtifactPopulationState,
    #[serde(deserialize_with = "deserialize_required_nullable")]
    selected_identity: RequiredNullable<ExactPartIdentity>,
    per_board_quantity: u64,
    total_quantity: u64,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct ResolutionArtifact {
    schema_name: String,
    schema_version: u32,
    design_name: String,
    variant_path: String,
    variant_identity_sha256: String,
    product_input_sha256: String,
    catalog_snapshot_id: String,
    catalog_snapshot_sha256: String,
    catalog_evaluated_on: String,
    build_quantity: u64,
    configurations: Vec<Configuration>,
    #[serde(deserialize_with = "deserialize_bounded_rows")]
    components: Vec<ResolutionRow>,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct BomArtifact {
    schema_name: String,
    schema_version: u32,
    design_name: String,
    variant_path: String,
    variant_identity_sha256: String,
    product_input_sha256: String,
    product_resolution_sha256: String,
    build_quantity: u64,
    #[serde(deserialize_with = "deserialize_bounded_rows")]
    items: Vec<BomRow>,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct PlacementArtifact {
    schema_name: String,
    schema_version: u32,
    design_name: String,
    variant_path: String,
    variant_identity_sha256: String,
    product_input_sha256: String,
    product_resolution_sha256: String,
    #[serde(deserialize_with = "deserialize_bounded_rows")]
    placements: Vec<PlacementRow>,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct AssemblyArtifact {
    schema_name: String,
    schema_version: u32,
    design_name: String,
    variant_path: String,
    variant_identity_sha256: String,
    product_input_sha256: String,
    product_resolution_sha256: String,
    build_quantity: u64,
    configurations: Vec<Configuration>,
    #[serde(deserialize_with = "deserialize_bounded_rows")]
    components: Vec<AssemblyRow>,
}

#[derive(Clone)]
struct Header {
    design_name: String,
    variant_path: String,
    variant_identity_sha256: String,
    product_input_sha256: String,
    catalog_snapshot_id: String,
    catalog_snapshot_sha256: String,
    catalog_evaluated_on: String,
    build_quantity: u64,
    configurations: Vec<Configuration>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
struct HeaderRef<'a> {
    design_name: &'a str,
    variant_path: &'a str,
    variant_identity_sha256: &'a str,
    product_input_sha256: &'a str,
}

trait ArtifactContract: Serialize + DeserializeOwned {
    const SCHEMA_NAME: &'static str;

    fn header(&self) -> HeaderRef<'_>;
    fn schema_name(&self) -> &str;
    fn schema_version(&self) -> u32;
    fn primary_row_count(&self) -> usize;
    fn validate_root(&self, artifact: &str, diagnostics: &mut Vec<ProductArtifactDiagnostic>);
    fn validate_rows(&self, diagnostics: &mut Vec<ProductArtifactDiagnostic>);
}

#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd, Serialize)]
#[serde(rename_all = "snake_case")]
enum ProjectionLifecycle {
    Active,
    NotRecommendedForNewDesigns,
    Obsolete,
}

#[derive(Clone, Debug, Eq, Ord, PartialEq, PartialOrd, Serialize)]
struct ProjectionSourcing {
    minimum_available_quantity: u64,
    maximum_lead_time_days: u32,
    required_region: String,
}

#[derive(Clone, Debug, Eq, Ord, PartialEq, PartialOrd, Serialize)]
struct ProjectionSubstitution {
    manufacturer: String,
    manufacturer_part_number: String,
    package: String,
}

#[derive(Clone, Debug, Eq, Ord, PartialEq, PartialOrd, Serialize)]
struct ProjectionComponent {
    component_path: String,
    reference: String,
    logical_function: String,
    manufacturer: String,
    manufacturer_part_number: String,
    package: String,
    value: ExactValue,
    lifecycle_requirement: ProjectionLifecycle,
    sourcing: ProjectionSourcing,
    approved_substitutions: Vec<ProjectionSubstitution>,
    x_nm: i64,
    y_nm: i64,
    rotation_degrees: i16,
    side: BoardSide,
    state: ArtifactPopulationState,
    alternate_identity: RequiredNullable<ProjectionSubstitution>,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
struct ProductInputProjection {
    design_name: String,
    catalog_snapshot_id: String,
    catalog_snapshot_sha256: String,
    catalog_evaluated_on: String,
    variant_path: String,
    build_quantity: u64,
    configurations: Vec<Configuration>,
    components: Vec<ProjectionComponent>,
}

fn deserialize_required_nullable<'de, D, T>(
    deserializer: D,
) -> Result<RequiredNullable<T>, D::Error>
where
    D: serde::Deserializer<'de>,
    T: Deserialize<'de>,
{
    RequiredNullable::deserialize(deserializer)
}

fn deserialize_bounded_rows<'de, D, T>(deserializer: D) -> Result<Vec<T>, D::Error>
where
    D: serde::Deserializer<'de>,
    T: Deserialize<'de>,
{
    struct BoundedRows<T>(PhantomData<T>);

    impl<'de, T: Deserialize<'de>> Visitor<'de> for BoundedRows<T> {
        type Value = Vec<T>;

        fn expecting(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
            write!(
                formatter,
                "at most {MAX_ARTIFACT_ROWS} product artifact rows"
            )
        }

        fn visit_seq<A>(self, mut sequence: A) -> Result<Self::Value, A::Error>
        where
            A: SeqAccess<'de>,
        {
            let capacity = sequence.size_hint().unwrap_or(0).min(MAX_ARTIFACT_ROWS);
            let mut rows = Vec::with_capacity(capacity);
            while rows.len() < MAX_ARTIFACT_ROWS {
                match sequence.next_element()? {
                    Some(row) => rows.push(row),
                    None => return Ok(rows),
                }
            }
            if sequence.next_element::<IgnoredAny>()?.is_some() {
                return Err(<A::Error as serde::de::Error>::custom(ROW_LIMIT_MARKER));
            }
            Ok(rows)
        }
    }

    deserializer.deserialize_seq(BoundedRows(PhantomData))
}

/// Compile deterministic variant-specific product artifacts from authenticated
/// catalog evidence. The catalog is always reverified inside this API.
pub fn compile_product_artifacts(
    design: &Design,
    snapshot_bytes: &[u8],
    variant_path: &str,
) -> Result<ProductArtifactBundle, Vec<ProductArtifactDiagnostic>> {
    let resolution = verify_product_catalog(design, snapshot_bytes).map_err(map_catalog_errors)?;
    let Some(variant) = design
        .product
        .variants
        .iter()
        .find(|variant| variant.path == variant_path)
    else {
        return Err(vec![diagnostic(
            "CC-PRODUCT-ARTIFACT-VARIANT-001",
            "variant_path",
            format!("Design IR does not declare product variant `{variant_path}`"),
        )]);
    };
    compile_verified(design, variant, &resolution)
}

/// Strictly parse, join, and independently recompute a complete product
/// artifact bundle. No caller-supplied catalog resolution is trusted.
pub fn verify_product_artifact_bundle(
    design: &Design,
    snapshot_bytes: &[u8],
    variant_path: &str,
    bundle: &ProductArtifactBundle,
) -> Result<(), Vec<ProductArtifactDiagnostic>> {
    enforce_aggregate_byte_budget([
        bundle.resolution_json.len(),
        bundle.bom_json.len(),
        bundle.placement_json.len(),
        bundle.assembly_json.len(),
    ])?;
    let catalog = verify_product_catalog(design, snapshot_bytes).map_err(map_catalog_errors)?;
    let Some(variant) = design
        .product
        .variants
        .iter()
        .find(|variant| variant.path == variant_path)
    else {
        return Err(vec![diagnostic(
            "CC-PRODUCT-ARTIFACT-VARIANT-001",
            "variant_path",
            format!("Design IR does not declare product variant `{variant_path}`"),
        )]);
    };
    let mut diagnostics = Vec::new();
    let resolution = parse_artifact::<ResolutionArtifact>(
        &bundle.resolution_json,
        "resolution.json",
        &mut diagnostics,
    );
    let bom = parse_artifact::<BomArtifact>(&bundle.bom_json, "bom.json", &mut diagnostics);
    let placement = parse_artifact::<PlacementArtifact>(
        &bundle.placement_json,
        "placement.json",
        &mut diagnostics,
    );
    let assembly = parse_artifact::<AssemblyArtifact>(
        &bundle.assembly_json,
        "assembly.json",
        &mut diagnostics,
    );
    if !diagnostics.is_empty() {
        return Err(diagnostics);
    }
    let (Some(resolution), Some(bom), Some(placement), Some(assembly)) =
        (resolution, bom, placement, assembly)
    else {
        return Err(vec![diagnostic(
            "CC-PRODUCT-ARTIFACT-CONTRACT-001",
            "bundle",
            "product artifact bundle could not be parsed",
        )]);
    };

    let resolution_sha256 = sha256_hex(bundle.resolution_json.as_bytes());
    validate_bundle_joins(
        &resolution,
        &bom,
        &placement,
        &assembly,
        &resolution_sha256,
        &mut diagnostics,
    );
    if !diagnostics.is_empty() {
        return Err(diagnostics);
    }

    let (expected_resolution, expected_bom, expected_placement, expected_assembly) =
        recompute_expected_for_verification(design, variant, &catalog)?;
    let variant_identity = variant_identity_sha256(variant_path)?;
    let base_path = format!("product/{variant_identity}");
    let paths_match = bundle.variant_path == variant_path
        && bundle.variant_identity_sha256 == variant_identity
        && bundle.resolution_path == artifact_path(&base_path, "resolution.json")?
        && bundle.bom_path == artifact_path(&base_path, "bom.json")?
        && bundle.placement_path == artifact_path(&base_path, "placement.json")?
        && bundle.assembly_path == artifact_path(&base_path, "assembly.json")?;
    if !paths_match
        || resolution != expected_resolution
        || bom != expected_bom
        || placement != expected_placement
        || assembly != expected_assembly
    {
        return Err(vec![diagnostic(
            "CC-PRODUCT-ARTIFACT-VERIFY-001",
            "bundle",
            "product artifact bundle does not equal the independently recomputed canonical bundle",
        )]);
    }
    Ok(())
}

fn compile_verified(
    design: &Design,
    variant: &ProductVariant,
    catalog: &CatalogResolution,
) -> Result<ProductArtifactBundle, Vec<ProductArtifactDiagnostic>> {
    let variant_identity_sha256 = variant_identity_sha256(&variant.path)?;
    let product_input_sha256 = compile_product_input_sha256(design, variant, catalog)?;
    let header = Header {
        design_name: design.name.clone(),
        variant_path: variant.path.clone(),
        variant_identity_sha256: variant_identity_sha256.clone(),
        product_input_sha256,
        catalog_snapshot_id: catalog.snapshot_id.clone(),
        catalog_snapshot_sha256: catalog.snapshot_sha256.clone(),
        catalog_evaluated_on: catalog.evaluated_on.clone(),
        build_quantity: variant.build_quantity,
        configurations: canonical_configurations(&variant.configurations),
    };

    let assignments: BTreeMap<_, _> = variant
        .components
        .iter()
        .map(|assignment| (assignment.component_path.as_str(), &assignment.state))
        .collect();
    let resolved_catalog: BTreeSet<_> = catalog
        .parts
        .iter()
        .map(|part| {
            (
                part.component_path.as_str(),
                part.alternate,
                part.manufacturer.as_str(),
                part.manufacturer_part_number.as_str(),
                part.package.as_str(),
            )
        })
        .collect();
    let mut components: Vec<_> = design
        .components
        .iter()
        .filter(|component| component.physical.is_some())
        .collect();
    components.sort_by(|left, right| left.path.cmp(&right.path));
    if components.len() > MAX_ARTIFACT_ROWS {
        return Err(vec![resource_diagnostic("design.components")]);
    }

    let mut resolution_rows = Vec::with_capacity(components.len());
    let mut placement_rows = Vec::with_capacity(components.len());
    let mut assembly_rows = Vec::with_capacity(components.len());
    let mut bom_groups: BTreeMap<ExactPartIdentity, u64> = BTreeMap::new();

    for component in components {
        let Some(state) = assignments.get(component.path.as_str()).copied() else {
            return Err(vec![diagnostic(
                "CC-PRODUCT-ARTIFACT-JOIN-001",
                &component.path,
                "validated variant is missing a physical-component assignment",
            )]);
        };
        let base = base_identity(component)?;
        let (artifact_state, selected, alternate) = selected_identity(component, state, &base);
        if let Some(selected) = &selected {
            let key = (
                component.path.as_str(),
                alternate,
                selected.manufacturer.as_str(),
                selected.manufacturer_part_number.as_str(),
                selected.package.as_str(),
            );
            if !resolved_catalog.contains(&key) {
                return Err(vec![diagnostic(
                    "CC-PRODUCT-ARTIFACT-JOIN-001",
                    &component.path,
                    "selected identity is absent from authenticated catalog resolution",
                )]);
            }
        }

        resolution_rows.push(ResolutionRow {
            component_path: component.path.clone(),
            reference: component.reference.clone(),
            state: artifact_state,
            base_identity: base,
            selected_identity: RequiredNullable(selected.clone()),
        });

        let quantity_per_board = u64::from(selected.is_some());
        let quantity_total = variant
            .build_quantity
            .checked_mul(quantity_per_board)
            .ok_or_else(|| vec![quantity_overflow(&component.path)])?;
        assembly_rows.push(AssemblyRow {
            component_path: component.path.clone(),
            reference: component.reference.clone(),
            state: artifact_state,
            selected_identity: RequiredNullable(selected.clone()),
            per_board_quantity: quantity_per_board,
            total_quantity: quantity_total,
        });

        if let Some(selected) = selected {
            let count = bom_groups.entry(selected.clone()).or_default();
            *count = count
                .checked_add(1)
                .ok_or_else(|| vec![quantity_overflow("bom.items")])?;
            let physical = component
                .physical
                .as_ref()
                .expect("validated physical component remains physical");
            placement_rows.push(PlacementRow {
                component_path: component.path.clone(),
                reference: component.reference.clone(),
                selected_identity: selected,
                x_nm: physical.placement.position.x,
                y_nm: physical.placement.position.y,
                rotation_degrees: physical.placement.rotation_degrees,
                side: match physical.placement.layer {
                    CopperLayer::Front => BoardSide::Front,
                    CopperLayer::Back => BoardSide::Back,
                },
            });
        }
    }

    let bom_rows = bom_groups
        .into_iter()
        .map(|(selected_identity, per_board_quantity)| {
            let quantity_total = variant
                .build_quantity
                .checked_mul(per_board_quantity)
                .ok_or_else(|| vec![quantity_overflow("bom.items")])?;
            Ok(BomRow {
                selected_identity,
                per_board_quantity,
                total_quantity: quantity_total,
            })
        })
        .collect::<Result<Vec<_>, Vec<ProductArtifactDiagnostic>>>()?;

    let resolution = ResolutionArtifact {
        schema_name: RESOLUTION_SCHEMA.to_owned(),
        schema_version: SCHEMA_VERSION,
        design_name: header.design_name.clone(),
        variant_path: header.variant_path.clone(),
        variant_identity_sha256: header.variant_identity_sha256.clone(),
        product_input_sha256: header.product_input_sha256.clone(),
        catalog_snapshot_id: header.catalog_snapshot_id.clone(),
        catalog_snapshot_sha256: header.catalog_snapshot_sha256.clone(),
        catalog_evaluated_on: header.catalog_evaluated_on.clone(),
        build_quantity: header.build_quantity,
        configurations: header.configurations.clone(),
        components: resolution_rows,
    };
    let resolution_json = render_artifact(&resolution, "resolution.json")?;
    let product_resolution_sha256 = sha256_hex(resolution_json.as_bytes());
    let bom = BomArtifact {
        schema_name: BOM_SCHEMA.to_owned(),
        schema_version: SCHEMA_VERSION,
        design_name: header.design_name.clone(),
        variant_path: header.variant_path.clone(),
        variant_identity_sha256: header.variant_identity_sha256.clone(),
        product_input_sha256: header.product_input_sha256.clone(),
        product_resolution_sha256: product_resolution_sha256.clone(),
        build_quantity: header.build_quantity,
        items: bom_rows,
    };
    let placement = PlacementArtifact {
        schema_name: PLACEMENT_SCHEMA.to_owned(),
        schema_version: SCHEMA_VERSION,
        design_name: header.design_name.clone(),
        variant_path: header.variant_path.clone(),
        variant_identity_sha256: header.variant_identity_sha256.clone(),
        product_input_sha256: header.product_input_sha256.clone(),
        product_resolution_sha256: product_resolution_sha256.clone(),
        placements: placement_rows,
    };
    let assembly = AssemblyArtifact {
        schema_name: ASSEMBLY_SCHEMA.to_owned(),
        schema_version: SCHEMA_VERSION,
        design_name: header.design_name,
        variant_path: header.variant_path,
        variant_identity_sha256: header.variant_identity_sha256,
        product_input_sha256: header.product_input_sha256,
        product_resolution_sha256,
        build_quantity: header.build_quantity,
        configurations: header.configurations,
        components: assembly_rows,
    };

    let base_path = format!("product/{variant_identity_sha256}");
    let bom_json = render_artifact(&bom, "bom.json")?;
    let placement_json = render_artifact(&placement, "placement.json")?;
    let assembly_json = render_artifact(&assembly, "assembly.json")?;
    enforce_aggregate_byte_budget([
        resolution_json.len(),
        bom_json.len(),
        placement_json.len(),
        assembly_json.len(),
    ])?;
    Ok(ProductArtifactBundle {
        variant_path: variant.path.clone(),
        variant_identity_sha256,
        resolution_path: artifact_path(&base_path, "resolution.json")?,
        bom_path: artifact_path(&base_path, "bom.json")?,
        placement_path: artifact_path(&base_path, "placement.json")?,
        assembly_path: artifact_path(&base_path, "assembly.json")?,
        resolution_json,
        bom_json,
        placement_json,
        assembly_json,
    })
}

/// Independent verifier-side reconstruction. This deliberately does not call
/// the emitter or its identity-selection and BOM-grouping helpers.
fn recompute_expected_for_verification(
    design: &Design,
    variant: &ProductVariant,
    catalog: &CatalogResolution,
) -> Result<
    (
        ResolutionArtifact,
        BomArtifact,
        PlacementArtifact,
        AssemblyArtifact,
    ),
    Vec<ProductArtifactDiagnostic>,
> {
    let assignments: BTreeMap<_, _> = variant
        .components
        .iter()
        .map(|assignment| (assignment.component_path.as_str(), &assignment.state))
        .collect();
    let catalog_index: BTreeSet<_> = catalog
        .parts
        .iter()
        .map(|part| {
            (
                part.component_path.as_str(),
                part.alternate,
                part.manufacturer.as_str(),
                part.manufacturer_part_number.as_str(),
                part.package.as_str(),
            )
        })
        .collect();
    let mut components: Vec<_> = design
        .components
        .iter()
        .filter(|component| component.physical.is_some())
        .collect();
    components.sort_by(|left, right| left.path.cmp(&right.path));
    if components.len() > MAX_ARTIFACT_ROWS {
        return Err(vec![resource_diagnostic("design.components")]);
    }

    let mut resolution_rows = Vec::with_capacity(components.len());
    let mut placement_rows = Vec::with_capacity(components.len());
    let mut assembly_rows = Vec::with_capacity(components.len());
    let mut projected_components = Vec::with_capacity(components.len());
    let mut grouped: BTreeMap<ExactPartIdentity, u64> = BTreeMap::new();

    for component in components {
        let Some(manufacturer) = component.part.manufacturer.as_ref() else {
            return Err(vec![diagnostic(
                "CC-PRODUCT-ARTIFACT-JOIN-001",
                &component.path,
                "physical component lacks verifier-side base manufacturer",
            )]);
        };
        let Some(number) = component.part.manufacturer_part_number.as_ref() else {
            return Err(vec![diagnostic(
                "CC-PRODUCT-ARTIFACT-JOIN-001",
                &component.path,
                "physical component lacks verifier-side base part number",
            )]);
        };
        let Some(package) = component.part.package.as_ref() else {
            return Err(vec![diagnostic(
                "CC-PRODUCT-ARTIFACT-JOIN-001",
                &component.path,
                "physical component lacks verifier-side package",
            )]);
        };
        let quantity = component.value.quantity();
        let value = match component.value {
            ComponentValue::Resistance(_) => ExactValue {
                kind: ValueKind::Resistance,
                coefficient: quantity.coefficient,
                exponent: quantity.exponent,
                unit: ValueUnit::Ohm,
            },
            ComponentValue::DcVoltage(_) => ExactValue {
                kind: ValueKind::DcVoltage,
                coefficient: quantity.coefficient,
                exponent: quantity.exponent,
                unit: ValueUnit::Volt,
            },
        };
        let base = ExactPartIdentity {
            logical_function: component.part.logical_function.clone(),
            manufacturer: manufacturer.clone(),
            manufacturer_part_number: number.clone(),
            package: package.clone(),
            value: value.clone(),
        };
        let Some(population) = assignments.get(component.path.as_str()).copied() else {
            return Err(vec![diagnostic(
                "CC-PRODUCT-ARTIFACT-JOIN-001",
                &component.path,
                "verifier-side variant assignment is missing",
            )]);
        };
        let (state, selected, alternate) = match population {
            PopulationState::Fitted => (ArtifactPopulationState::Fitted, Some(base.clone()), false),
            PopulationState::NotFitted => (ArtifactPopulationState::NotFitted, None, false),
            PopulationState::Alternate(alternate) => (
                ArtifactPopulationState::Alternate,
                Some(ExactPartIdentity {
                    logical_function: component.part.logical_function.clone(),
                    manufacturer: alternate.manufacturer.clone(),
                    manufacturer_part_number: alternate.manufacturer_part_number.clone(),
                    package: alternate.package.clone(),
                    value: value.clone(),
                }),
                true,
            ),
        };
        if let Some(identity) = &selected {
            let key = (
                component.path.as_str(),
                alternate,
                identity.manufacturer.as_str(),
                identity.manufacturer_part_number.as_str(),
                identity.package.as_str(),
            );
            if !catalog_index.contains(&key) {
                return Err(vec![diagnostic(
                    "CC-PRODUCT-ARTIFACT-JOIN-001",
                    &component.path,
                    "verifier-side selected identity is absent from authenticated catalog resolution",
                )]);
            }
        }

        let physical = component
            .physical
            .as_ref()
            .expect("verifier filtered physical components");
        let side = match physical.placement.layer {
            CopperLayer::Front => BoardSide::Front,
            CopperLayer::Back => BoardSide::Back,
        };
        resolution_rows.push(ResolutionRow {
            component_path: component.path.clone(),
            reference: component.reference.clone(),
            state,
            base_identity: base.clone(),
            selected_identity: RequiredNullable(selected.clone()),
        });
        let per_board_quantity = u64::from(selected.is_some());
        let total_quantity = variant
            .build_quantity
            .checked_mul(per_board_quantity)
            .ok_or_else(|| vec![quantity_overflow(&component.path)])?;
        assembly_rows.push(AssemblyRow {
            component_path: component.path.clone(),
            reference: component.reference.clone(),
            state,
            selected_identity: RequiredNullable(selected.clone()),
            per_board_quantity,
            total_quantity,
        });
        if let Some(identity) = selected {
            let count = grouped.entry(identity.clone()).or_default();
            *count = count
                .checked_add(1)
                .ok_or_else(|| vec![quantity_overflow("bom.items")])?;
            placement_rows.push(PlacementRow {
                component_path: component.path.clone(),
                reference: component.reference.clone(),
                selected_identity: identity,
                x_nm: physical.placement.position.x,
                y_nm: physical.placement.position.y,
                rotation_degrees: physical.placement.rotation_degrees,
                side,
            });
        }

        let sourcing = component
            .part
            .sourcing
            .as_ref()
            .expect("validated physical component has sourcing constraints");
        let lifecycle_requirement = match component
            .part
            .lifecycle
            .expect("validated physical component has lifecycle intent")
        {
            LifecycleStatus::Active => ProjectionLifecycle::Active,
            LifecycleStatus::NotRecommendedForNewDesigns => {
                ProjectionLifecycle::NotRecommendedForNewDesigns
            }
            LifecycleStatus::Obsolete => ProjectionLifecycle::Obsolete,
        };
        let mut approved_substitutions: Vec<_> = component
            .part
            .approved_substitutions
            .iter()
            .map(|substitution| ProjectionSubstitution {
                manufacturer: substitution.manufacturer.clone(),
                manufacturer_part_number: substitution.manufacturer_part_number.clone(),
                package: substitution.package.clone(),
            })
            .collect();
        approved_substitutions.sort();
        let alternate_identity = match population {
            PopulationState::Alternate(alternate) => Some(ProjectionSubstitution {
                manufacturer: alternate.manufacturer.clone(),
                manufacturer_part_number: alternate.manufacturer_part_number.clone(),
                package: alternate.package.clone(),
            }),
            PopulationState::Fitted | PopulationState::NotFitted => None,
        };
        projected_components.push(ProjectionComponent {
            component_path: component.path.clone(),
            reference: component.reference.clone(),
            logical_function: component.part.logical_function.clone(),
            manufacturer: manufacturer.clone(),
            manufacturer_part_number: number.clone(),
            package: package.clone(),
            value,
            lifecycle_requirement,
            sourcing: ProjectionSourcing {
                minimum_available_quantity: sourcing.minimum_available_quantity,
                maximum_lead_time_days: sourcing.maximum_lead_time_days,
                required_region: sourcing.required_region.clone(),
            },
            approved_substitutions,
            x_nm: physical.placement.position.x,
            y_nm: physical.placement.position.y,
            rotation_degrees: physical.placement.rotation_degrees,
            side,
            state,
            alternate_identity: RequiredNullable(alternate_identity),
        });
    }

    let bom_rows = grouped
        .into_iter()
        .map(|(selected_identity, per_board_quantity)| {
            let total_quantity = variant
                .build_quantity
                .checked_mul(per_board_quantity)
                .ok_or_else(|| vec![quantity_overflow("bom.items")])?;
            Ok(BomRow {
                selected_identity,
                per_board_quantity,
                total_quantity,
            })
        })
        .collect::<Result<Vec<_>, Vec<ProductArtifactDiagnostic>>>()?;

    let mut configurations: Vec<_> = variant
        .configurations
        .iter()
        .map(|configuration| Configuration {
            key: configuration.key.clone(),
            value: configuration.value.clone(),
        })
        .collect();
    configurations.sort();
    let product_input_sha256 = hash_product_input_projection(&ProductInputProjection {
        design_name: design.name.clone(),
        catalog_snapshot_id: catalog.snapshot_id.clone(),
        catalog_snapshot_sha256: catalog.snapshot_sha256.clone(),
        catalog_evaluated_on: catalog.evaluated_on.clone(),
        variant_path: variant.path.clone(),
        build_quantity: variant.build_quantity,
        configurations: configurations.clone(),
        components: projected_components,
    })?;
    let variant_identity_sha256 = variant_identity_sha256(&variant.path)?;

    let resolution = ResolutionArtifact {
        schema_name: RESOLUTION_SCHEMA.to_owned(),
        schema_version: SCHEMA_VERSION,
        design_name: design.name.clone(),
        variant_path: variant.path.clone(),
        variant_identity_sha256: variant_identity_sha256.clone(),
        product_input_sha256: product_input_sha256.clone(),
        catalog_snapshot_id: catalog.snapshot_id.clone(),
        catalog_snapshot_sha256: catalog.snapshot_sha256.clone(),
        catalog_evaluated_on: catalog.evaluated_on.clone(),
        build_quantity: variant.build_quantity,
        configurations: configurations.clone(),
        components: resolution_rows,
    };
    let resolution_json =
        serialize_canonical(&resolution, "resolution.json").map_err(|error| vec![error])?;
    let product_resolution_sha256 = sha256_hex(resolution_json.as_bytes());
    let bom = BomArtifact {
        schema_name: BOM_SCHEMA.to_owned(),
        schema_version: SCHEMA_VERSION,
        design_name: design.name.clone(),
        variant_path: variant.path.clone(),
        variant_identity_sha256: variant_identity_sha256.clone(),
        product_input_sha256: product_input_sha256.clone(),
        product_resolution_sha256: product_resolution_sha256.clone(),
        build_quantity: variant.build_quantity,
        items: bom_rows,
    };
    let placement = PlacementArtifact {
        schema_name: PLACEMENT_SCHEMA.to_owned(),
        schema_version: SCHEMA_VERSION,
        design_name: design.name.clone(),
        variant_path: variant.path.clone(),
        variant_identity_sha256: variant_identity_sha256.clone(),
        product_input_sha256: product_input_sha256.clone(),
        product_resolution_sha256: product_resolution_sha256.clone(),
        placements: placement_rows,
    };
    let assembly = AssemblyArtifact {
        schema_name: ASSEMBLY_SCHEMA.to_owned(),
        schema_version: SCHEMA_VERSION,
        design_name: design.name.clone(),
        variant_path: variant.path.clone(),
        variant_identity_sha256,
        product_input_sha256,
        product_resolution_sha256,
        build_quantity: variant.build_quantity,
        configurations,
        components: assembly_rows,
    };
    Ok((resolution, bom, placement, assembly))
}

fn artifact_path(
    base: &str,
    filename: &str,
) -> Result<RelativeArtifactPath, Vec<ProductArtifactDiagnostic>> {
    RelativeArtifactPath::try_new(format!("{base}/{filename}")).map_err(|error| {
        vec![diagnostic(
            "CC-PRODUCT-ARTIFACT-PATH-001",
            "variant_path",
            format!("could not derive a portable product artifact path: {error}"),
        )]
    })
}

fn variant_identity_sha256(variant_path: &str) -> Result<String, Vec<ProductArtifactDiagnostic>> {
    let mut hasher = Sha256::new();
    hasher.update(VARIANT_IDENTITY_DOMAIN);
    hasher.update(variant_path.as_bytes());
    Ok(hex_digest(hasher.finalize().as_slice()))
}

fn compile_product_input_sha256(
    design: &Design,
    variant: &ProductVariant,
    catalog: &CatalogResolution,
) -> Result<String, Vec<ProductArtifactDiagnostic>> {
    let assignments: BTreeMap<_, _> = variant
        .components
        .iter()
        .map(|assignment| (assignment.component_path.as_str(), &assignment.state))
        .collect();
    let mut components: Vec<_> = design
        .components
        .iter()
        .filter(|component| component.physical.is_some())
        .collect();
    components.sort_by(|left, right| left.path.cmp(&right.path));
    let mut projected = Vec::with_capacity(components.len());
    for component in components {
        let base = base_identity(component)?;
        let Some(state) = assignments.get(component.path.as_str()).copied() else {
            return Err(vec![diagnostic(
                "CC-PRODUCT-ARTIFACT-JOIN-001",
                &component.path,
                "validated variant is missing a physical-component assignment",
            )]);
        };
        let (artifact_state, _, _) = selected_identity(component, state, &base);
        let physical = component
            .physical
            .as_ref()
            .expect("validated physical component remains physical");
        let sourcing = component
            .part
            .sourcing
            .as_ref()
            .expect("validated physical component has sourcing constraints");
        let lifecycle = component
            .part
            .lifecycle
            .expect("validated physical component has lifecycle intent");
        let mut approved_substitutions: Vec<_> = component
            .part
            .approved_substitutions
            .iter()
            .map(|substitution| ProjectionSubstitution {
                manufacturer: substitution.manufacturer.clone(),
                manufacturer_part_number: substitution.manufacturer_part_number.clone(),
                package: substitution.package.clone(),
            })
            .collect();
        approved_substitutions.sort();
        projected.push(ProjectionComponent {
            component_path: component.path.clone(),
            reference: component.reference.clone(),
            logical_function: component.part.logical_function.clone(),
            manufacturer: base.manufacturer,
            manufacturer_part_number: base.manufacturer_part_number,
            package: base.package,
            value: base.value,
            lifecycle_requirement: projection_lifecycle(lifecycle),
            sourcing: ProjectionSourcing {
                minimum_available_quantity: sourcing.minimum_available_quantity,
                maximum_lead_time_days: sourcing.maximum_lead_time_days,
                required_region: sourcing.required_region.clone(),
            },
            approved_substitutions,
            x_nm: physical.placement.position.x,
            y_nm: physical.placement.position.y,
            rotation_degrees: physical.placement.rotation_degrees,
            side: match physical.placement.layer {
                CopperLayer::Front => BoardSide::Front,
                CopperLayer::Back => BoardSide::Back,
            },
            state: artifact_state,
            alternate_identity: RequiredNullable(match state {
                PopulationState::Alternate(alternate) => Some(ProjectionSubstitution {
                    manufacturer: alternate.manufacturer.clone(),
                    manufacturer_part_number: alternate.manufacturer_part_number.clone(),
                    package: alternate.package.clone(),
                }),
                PopulationState::Fitted | PopulationState::NotFitted => None,
            }),
        });
    }
    hash_product_input_projection(&ProductInputProjection {
        design_name: design.name.clone(),
        catalog_snapshot_id: catalog.snapshot_id.clone(),
        catalog_snapshot_sha256: catalog.snapshot_sha256.clone(),
        catalog_evaluated_on: catalog.evaluated_on.clone(),
        variant_path: variant.path.clone(),
        build_quantity: variant.build_quantity,
        configurations: canonical_configurations(&variant.configurations),
        components: projected,
    })
}

fn hash_product_input_projection(
    projection: &ProductInputProjection,
) -> Result<String, Vec<ProductArtifactDiagnostic>> {
    let mut hasher = Sha256::new();
    hasher.update(PRODUCT_INPUT_DOMAIN);
    let mut writer = Sha256Writer(hasher);
    serde_json::to_writer(&mut writer, projection).map_err(|error| {
        vec![diagnostic(
            "CC-PRODUCT-ARTIFACT-CONTRACT-001",
            "product_input",
            format!("could not serialize canonical product-input projection: {error}"),
        )]
    })?;
    Ok(hex_digest(writer.0.finalize().as_slice()))
}

struct Sha256Writer(Sha256);

impl Write for Sha256Writer {
    fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
        self.0.update(bytes);
        Ok(bytes.len())
    }

    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

fn projection_lifecycle(lifecycle: LifecycleStatus) -> ProjectionLifecycle {
    match lifecycle {
        LifecycleStatus::Active => ProjectionLifecycle::Active,
        LifecycleStatus::NotRecommendedForNewDesigns => {
            ProjectionLifecycle::NotRecommendedForNewDesigns
        }
        LifecycleStatus::Obsolete => ProjectionLifecycle::Obsolete,
    }
}

fn enforce_aggregate_byte_budget(sizes: [usize; 4]) -> Result<(), Vec<ProductArtifactDiagnostic>> {
    let total = sizes
        .into_iter()
        .try_fold(0_usize, usize::checked_add)
        .ok_or_else(|| vec![resource_diagnostic("bundle")])?;
    if total > MAX_ARTIFACT_BYTES {
        return Err(vec![resource_diagnostic("bundle")]);
    }
    Ok(())
}

fn canonical_configurations(configurations: &[ProductConfiguration]) -> Vec<Configuration> {
    let mut result: Vec<_> = configurations
        .iter()
        .map(|configuration| Configuration {
            key: configuration.key.clone(),
            value: configuration.value.clone(),
        })
        .collect();
    result.sort();
    result
}

fn base_identity(
    component: &Component,
) -> Result<ExactPartIdentity, Vec<ProductArtifactDiagnostic>> {
    let (Some(manufacturer), Some(number), Some(package)) = (
        component.part.manufacturer.as_ref(),
        component.part.manufacturer_part_number.as_ref(),
        component.part.package.as_ref(),
    ) else {
        return Err(vec![diagnostic(
            "CC-PRODUCT-ARTIFACT-JOIN-001",
            &component.path,
            "physical component does not carry a complete base identity",
        )]);
    };
    Ok(ExactPartIdentity {
        logical_function: component.part.logical_function.clone(),
        manufacturer: manufacturer.clone(),
        manufacturer_part_number: number.clone(),
        package: package.clone(),
        value: exact_value(component),
    })
}

fn selected_identity(
    component: &Component,
    state: &PopulationState,
    base: &ExactPartIdentity,
) -> (ArtifactPopulationState, Option<ExactPartIdentity>, bool) {
    match state {
        PopulationState::Fitted => (ArtifactPopulationState::Fitted, Some(base.clone()), false),
        PopulationState::NotFitted => (ArtifactPopulationState::NotFitted, None, false),
        PopulationState::Alternate(alternate) => (
            ArtifactPopulationState::Alternate,
            Some(ExactPartIdentity {
                logical_function: component.part.logical_function.clone(),
                manufacturer: alternate.manufacturer.clone(),
                manufacturer_part_number: alternate.manufacturer_part_number.clone(),
                package: alternate.package.clone(),
                value: base.value.clone(),
            }),
            true,
        ),
    }
}

fn exact_value(component: &Component) -> ExactValue {
    let quantity = component.value.quantity();
    let (kind, unit) = match component.value {
        ComponentValue::Resistance(_) => (ValueKind::Resistance, ValueUnit::Ohm),
        ComponentValue::DcVoltage(_) => (ValueKind::DcVoltage, ValueUnit::Volt),
    };
    ExactValue {
        kind,
        coefficient: quantity.coefficient,
        exponent: quantity.exponent,
        unit,
    }
}

fn parse_artifact<T: ArtifactContract>(
    input: &str,
    artifact: &str,
    diagnostics: &mut Vec<ProductArtifactDiagnostic>,
) -> Option<T> {
    if input.len() > MAX_ARTIFACT_BYTES {
        push(diagnostics, resource_diagnostic(artifact));
        return None;
    }
    let value: T = match serde_json::from_str(input) {
        Ok(value) => value,
        Err(error) => {
            if error.to_string().starts_with(ROW_LIMIT_MARKER) {
                push(diagnostics, resource_diagnostic(artifact));
                return None;
            }
            push(
                diagnostics,
                diagnostic(
                    "CC-PRODUCT-ARTIFACT-CONTRACT-001",
                    artifact,
                    format!("invalid strict JSON product artifact: {error}"),
                ),
            );
            return None;
        }
    };
    validate_artifact(&value, artifact, diagnostics);
    if diagnostics.is_empty() {
        match serialize_canonical(&value, artifact) {
            Ok(canonical) if canonical == input => {}
            Ok(_) => push(
                diagnostics,
                diagnostic(
                    "CC-PRODUCT-ARTIFACT-CONTRACT-007",
                    artifact,
                    "product artifact bytes are not canonical compact JSON with one final LF",
                ),
            ),
            Err(error) => push(diagnostics, error),
        }
    }
    Some(value)
}

fn render_artifact<T: ArtifactContract>(
    value: &T,
    artifact: &str,
) -> Result<String, Vec<ProductArtifactDiagnostic>> {
    let mut diagnostics = Vec::new();
    validate_artifact(value, artifact, &mut diagnostics);
    if diagnostics.is_empty() {
        serialize_canonical(value, artifact).map_err(|error| vec![error])
    } else {
        Err(diagnostics)
    }
}

fn serialize_canonical<T: Serialize>(
    value: &T,
    artifact: &str,
) -> Result<String, ProductArtifactDiagnostic> {
    let mut json = serde_json::to_string(value).map_err(|error| {
        diagnostic(
            "CC-PRODUCT-ARTIFACT-CONTRACT-001",
            artifact,
            format!("could not serialize canonical product artifact: {error}"),
        )
    })?;
    json.push('\n');
    if json.len() > MAX_ARTIFACT_BYTES {
        return Err(resource_diagnostic(artifact));
    }
    Ok(json)
}

fn validate_artifact<T: ArtifactContract>(
    value: &T,
    artifact: &str,
    diagnostics: &mut Vec<ProductArtifactDiagnostic>,
) {
    if value.schema_name() != T::SCHEMA_NAME || value.schema_version() != SCHEMA_VERSION {
        push(
            diagnostics,
            diagnostic(
                "CC-PRODUCT-ARTIFACT-CONTRACT-002",
                artifact,
                "unsupported product artifact schema name or version",
            ),
        );
    }
    validate_header(value.header(), artifact, diagnostics);
    if value.primary_row_count() > MAX_ARTIFACT_ROWS {
        push(diagnostics, resource_diagnostic(artifact));
        return;
    }
    value.validate_root(artifact, diagnostics);
    value.validate_rows(diagnostics);
}

fn validate_header(
    header: HeaderRef<'_>,
    artifact: &str,
    diagnostics: &mut Vec<ProductArtifactDiagnostic>,
) {
    if header.design_name.is_empty()
        || header.variant_path.is_empty()
        || !sha256_is_valid(header.variant_identity_sha256)
        || !sha256_is_valid(header.product_input_sha256)
    {
        push(
            diagnostics,
            diagnostic(
                "CC-PRODUCT-ARTIFACT-CONTRACT-003",
                artifact,
                "product artifact header is not canonical",
            ),
        );
    }
    match variant_identity_sha256(header.variant_path) {
        Ok(expected) if expected == header.variant_identity_sha256 => {}
        _ => push(
            diagnostics,
            diagnostic(
                "CC-PRODUCT-ARTIFACT-CONTRACT-003",
                artifact,
                "variant identity digest does not bind the exact variant path",
            ),
        ),
    }
}

fn validate_bundle_joins(
    resolution: &ResolutionArtifact,
    bom: &BomArtifact,
    placement: &PlacementArtifact,
    assembly: &AssemblyArtifact,
    resolution_sha256: &str,
    diagnostics: &mut Vec<ProductArtifactDiagnostic>,
) {
    let header = resolution.header();
    if bom.header() != header || placement.header() != header || assembly.header() != header {
        push(
            diagnostics,
            diagnostic(
                "CC-PRODUCT-ARTIFACT-JOIN-001",
                "bundle",
                "product artifact roots do not bind the same Design, variant, and authoritative-input digest",
            ),
        );
        return;
    }
    if bom.product_resolution_sha256 != resolution_sha256
        || placement.product_resolution_sha256 != resolution_sha256
        || assembly.product_resolution_sha256 != resolution_sha256
        || bom.build_quantity != resolution.build_quantity
        || assembly.build_quantity != resolution.build_quantity
        || assembly.configurations != resolution.configurations
    {
        push(
            diagnostics,
            diagnostic(
                "CC-PRODUCT-ARTIFACT-JOIN-001",
                "bundle",
                "derived artifacts do not bind the exact resolution bytes, quantity, and configuration",
            ),
        );
    }

    let assembly_by_path: BTreeMap<_, _> = assembly
        .components
        .iter()
        .map(|row| (row.component_path.as_str(), row))
        .collect();
    let placement_by_path: BTreeMap<_, _> = placement
        .placements
        .iter()
        .map(|row| (row.component_path.as_str(), row))
        .collect();
    for row in &resolution.components {
        let Some(assembly_row) = assembly_by_path.get(row.component_path.as_str()) else {
            push(
                diagnostics,
                diagnostic(
                    "CC-PRODUCT-ARTIFACT-JOIN-001",
                    &row.component_path,
                    "resolution component is absent from assembly artifact",
                ),
            );
            continue;
        };
        let expected_per_board = u64::from(row.selected_identity.0.is_some());
        let expected_total = resolution.build_quantity.checked_mul(expected_per_board);
        if assembly_row.state != row.state
            || assembly_row.reference != row.reference
            || assembly_row.selected_identity != row.selected_identity
            || assembly_row.per_board_quantity != expected_per_board
            || expected_total != Some(assembly_row.total_quantity)
        {
            push(
                diagnostics,
                diagnostic(
                    "CC-PRODUCT-ARTIFACT-JOIN-001",
                    &row.component_path,
                    "assembly population state, selected identity, or quantities do not join resolution",
                ),
            );
        }
        match &row.selected_identity.0 {
            Some(selected)
                if placement_by_path
                    .get(row.component_path.as_str())
                    .is_some_and(|placement| {
                        placement.reference == row.reference
                            && &placement.selected_identity == selected
                    }) => {}
            Some(_) => push(
                diagnostics,
                diagnostic(
                    "CC-PRODUCT-ARTIFACT-JOIN-001",
                    &row.component_path,
                    "fitted resolution component does not join exact placement identity",
                ),
            ),
            None if placement_by_path.contains_key(row.component_path.as_str()) => push(
                diagnostics,
                diagnostic(
                    "CC-PRODUCT-ARTIFACT-JOIN-001",
                    &row.component_path,
                    "not-fitted resolution component unexpectedly has a placement",
                ),
            ),
            None => {}
        }
    }
    if assembly.components.len() != resolution.components.len()
        || placement.placements.len()
            != resolution
                .components
                .iter()
                .filter(|row| row.selected_identity.0.is_some())
                .count()
    {
        push(
            diagnostics,
            diagnostic(
                "CC-PRODUCT-ARTIFACT-JOIN-001",
                "bundle",
                "resolution, placement, and assembly component inventories do not reconcile",
            ),
        );
    }

    for item in &bom.items {
        if item.per_board_quantity == 0
            || resolution
                .build_quantity
                .checked_mul(item.per_board_quantity)
                != Some(item.total_quantity)
        {
            push(
                diagnostics,
                diagnostic(
                    "CC-PRODUCT-ARTIFACT-JOIN-001",
                    "bom.items",
                    "BOM quantities are zero, overflowed, or inconsistent with build quantity",
                ),
            );
        }
    }
}

fn validate_rows_sorted<T, K: Ord>(
    rows: &[T],
    key: impl Fn(&T) -> K,
    path: &str,
    diagnostics: &mut Vec<ProductArtifactDiagnostic>,
) {
    if rows.len() > MAX_ARTIFACT_ROWS {
        push(diagnostics, resource_diagnostic(path));
    } else if !strictly_sorted_unique_by(rows, key) {
        push(
            diagnostics,
            diagnostic(
                "CC-PRODUCT-ARTIFACT-CONTRACT-004",
                path,
                "product artifact rows must be strictly sorted and unique by canonical key",
            ),
        );
    }
}

fn strictly_sorted_unique_by<T, K: Ord>(rows: &[T], key: impl Fn(&T) -> K) -> bool {
    rows.windows(2)
        .all(|window| key(&window[0]) < key(&window[1]))
}

fn validate_configurations(
    configurations: &[Configuration],
    artifact: &str,
    diagnostics: &mut Vec<ProductArtifactDiagnostic>,
) {
    if !strictly_sorted_unique_by(configurations, |configuration| configuration.key.clone()) {
        push(
            diagnostics,
            diagnostic(
                "CC-PRODUCT-ARTIFACT-CONTRACT-004",
                artifact,
                "product configurations must be strictly sorted and unique by key",
            ),
        );
    }
}

fn validate_resolution_root(
    value: &ResolutionArtifact,
    artifact: &str,
    diagnostics: &mut Vec<ProductArtifactDiagnostic>,
) {
    if value.catalog_snapshot_id.is_empty()
        || !sha256_is_valid(&value.catalog_snapshot_sha256)
        || value.catalog_evaluated_on.is_empty()
        || value.build_quantity == 0
    {
        push(
            diagnostics,
            diagnostic(
                "CC-PRODUCT-ARTIFACT-CONTRACT-003",
                artifact,
                "resolution authority fields are not canonical",
            ),
        );
    }
    validate_configurations(&value.configurations, artifact, diagnostics);
    for row in &value.components {
        let selection_is_valid = match row.state {
            ArtifactPopulationState::Fitted => {
                row.selected_identity.0.as_ref() == Some(&row.base_identity)
            }
            ArtifactPopulationState::NotFitted => row.selected_identity.0.is_none(),
            ArtifactPopulationState::Alternate => {
                row.selected_identity.0.as_ref().is_some_and(|selected| {
                    selected.logical_function == row.base_identity.logical_function
                        && selected.value == row.base_identity.value
                        && selected != &row.base_identity
                })
            }
        };
        if row.reference.is_empty() || !selection_is_valid {
            push(
                diagnostics,
                diagnostic(
                    "CC-PRODUCT-ARTIFACT-CONTRACT-005",
                    &row.component_path,
                    "resolution state, reference, or selected identity is invalid",
                ),
            );
        }
    }
}

fn validate_bom_root(
    value: &BomArtifact,
    artifact: &str,
    diagnostics: &mut Vec<ProductArtifactDiagnostic>,
) {
    if !sha256_is_valid(&value.product_resolution_sha256) || value.build_quantity == 0 {
        push(
            diagnostics,
            diagnostic(
                "CC-PRODUCT-ARTIFACT-CONTRACT-003",
                artifact,
                "BOM resolution digest or build quantity is not canonical",
            ),
        );
    }
}

fn validate_placement_root(
    value: &PlacementArtifact,
    artifact: &str,
    diagnostics: &mut Vec<ProductArtifactDiagnostic>,
) {
    if !sha256_is_valid(&value.product_resolution_sha256) {
        push(
            diagnostics,
            diagnostic(
                "CC-PRODUCT-ARTIFACT-CONTRACT-003",
                artifact,
                "placement resolution digest is not canonical",
            ),
        );
    }
}

fn validate_assembly_root(
    value: &AssemblyArtifact,
    artifact: &str,
    diagnostics: &mut Vec<ProductArtifactDiagnostic>,
) {
    if !sha256_is_valid(&value.product_resolution_sha256) || value.build_quantity == 0 {
        push(
            diagnostics,
            diagnostic(
                "CC-PRODUCT-ARTIFACT-CONTRACT-003",
                artifact,
                "assembly resolution digest or build quantity is not canonical",
            ),
        );
    }
    validate_configurations(&value.configurations, artifact, diagnostics);
}

macro_rules! impl_artifact_contract {
    ($type:ty, $schema:expr, $rows:ident, $key:expr, $root:expr) => {
        impl ArtifactContract for $type {
            const SCHEMA_NAME: &'static str = $schema;

            fn header(&self) -> HeaderRef<'_> {
                HeaderRef {
                    design_name: &self.design_name,
                    variant_path: &self.variant_path,
                    variant_identity_sha256: &self.variant_identity_sha256,
                    product_input_sha256: &self.product_input_sha256,
                }
            }

            fn schema_name(&self) -> &str {
                &self.schema_name
            }

            fn schema_version(&self) -> u32 {
                self.schema_version
            }

            fn primary_row_count(&self) -> usize {
                self.$rows.len()
            }

            fn validate_root(
                &self,
                artifact: &str,
                diagnostics: &mut Vec<ProductArtifactDiagnostic>,
            ) {
                ($root)(self, artifact, diagnostics);
            }

            fn validate_rows(&self, diagnostics: &mut Vec<ProductArtifactDiagnostic>) {
                validate_rows_sorted(&self.$rows, $key, concat!(stringify!($rows)), diagnostics);
            }
        }
    };
}

impl_artifact_contract!(
    ResolutionArtifact,
    RESOLUTION_SCHEMA,
    components,
    |row: &ResolutionRow| row.component_path.clone(),
    validate_resolution_root
);
impl_artifact_contract!(
    BomArtifact,
    BOM_SCHEMA,
    items,
    |row: &BomRow| row.selected_identity.clone(),
    validate_bom_root
);
impl_artifact_contract!(
    PlacementArtifact,
    PLACEMENT_SCHEMA,
    placements,
    |row: &PlacementRow| row.component_path.clone(),
    validate_placement_root
);
impl_artifact_contract!(
    AssemblyArtifact,
    ASSEMBLY_SCHEMA,
    components,
    |row: &AssemblyRow| row.component_path.clone(),
    validate_assembly_root
);

fn sha256_is_valid(value: &str) -> bool {
    value.len() == 64
        && value
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
}

fn hex_digest(bytes: &[u8]) -> String {
    bytes.iter().map(|byte| format!("{byte:02x}")).collect()
}

fn sha256_hex(bytes: &[u8]) -> String {
    let mut hasher = Sha256::new();
    hasher.update(bytes);
    hex_digest(hasher.finalize().as_slice())
}

fn map_catalog_errors(errors: Vec<CatalogDiagnostic>) -> Vec<ProductArtifactDiagnostic> {
    errors
        .into_iter()
        .map(|error| ProductArtifactDiagnostic {
            code: error.code,
            path: error.path,
            message: error.message,
        })
        .collect()
}

fn resource_diagnostic(path: impl Into<String>) -> ProductArtifactDiagnostic {
    diagnostic(
        "CC-PRODUCT-ARTIFACT-RESOURCE-001",
        path,
        format!(
            "product artifact exceeds the {MAX_ARTIFACT_BYTES}-byte or {MAX_ARTIFACT_ROWS}-row limit"
        ),
    )
}

fn quantity_overflow(path: impl Into<String>) -> ProductArtifactDiagnostic {
    diagnostic(
        "CC-PRODUCT-ARTIFACT-QUANTITY-001",
        path,
        "product artifact quantity arithmetic overflowed u64",
    )
}

fn diagnostic(
    code: &'static str,
    path: impl Into<String>,
    message: impl Into<String>,
) -> ProductArtifactDiagnostic {
    ProductArtifactDiagnostic {
        code,
        path: path.into(),
        message: message.into(),
    }
}

fn push(diagnostics: &mut Vec<ProductArtifactDiagnostic>, diagnostic: ProductArtifactDiagnostic) {
    if diagnostics.len() < MAX_DIAGNOSTICS {
        diagnostics.push(diagnostic);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::demo::voltage_divider;

    const SNAPSHOT: &[u8] = include_bytes!("../../catalogs/reference-catalog.json");

    fn compile(variant: &str) -> ProductArtifactBundle {
        compile_product_artifacts(&voltage_divider(), SNAPSHOT, variant)
            .expect("reference product artifacts compile")
    }

    fn parse<T: DeserializeOwned>(json: &str) -> T {
        serde_json::from_str(json).expect("emitted product artifact parses")
    }

    fn render_value(value: &serde_json::Value) -> String {
        let mut json = serde_json::to_string(value).unwrap();
        json.push('\n');
        json
    }

    #[test]
    fn production_and_alternate_not_fitted_variants_are_exact_and_joined() {
        let design = voltage_divider();
        let production = compile_product_artifacts(&design, SNAPSHOT, "production").unwrap();
        let resolution: ResolutionArtifact = parse(&production.resolution_json);
        let bom: BomArtifact = parse(&production.bom_json);
        let placement: PlacementArtifact = parse(&production.placement_json);
        let assembly: AssemblyArtifact = parse(&production.assembly_json);
        assert_eq!(resolution.components.len(), 2);
        assert!(resolution.components.iter().all(|row| {
            row.state == ArtifactPopulationState::Fitted && row.selected_identity.0.is_some()
        }));
        assert_eq!(bom.items.len(), 1);
        assert_eq!(bom.items[0].selected_identity.manufacturer, "Yageo");
        assert_eq!(bom.items[0].per_board_quantity, 2);
        assert_eq!(bom.items[0].total_quantity, 20);
        assert_eq!(placement.placements.len(), 2);
        assert_eq!(assembly.components.len(), 2);
        for artifact in [
            &production.resolution_json,
            &production.bom_json,
            &production.placement_json,
            &production.assembly_json,
        ] {
            assert!(artifact.ends_with('\n'));
            assert!(!artifact.ends_with("\n\n"));
        }
        assert_eq!(
            production.variant_identity_sha256,
            "195439caa3cb617eeaccdcacf243a13dd8194cba296b2a52085355611236b915"
        );
        assert_eq!(resolution.schema_name, "circuitc.product_resolution");
        assert_eq!(bom.schema_name, "circuitc.bom");
        assert_eq!(placement.schema_name, "circuitc.placement");
        assert_eq!(assembly.schema_name, "circuitc.assembly");
        assert_eq!(resolution.product_input_sha256, bom.product_input_sha256);
        assert_eq!(
            resolution.product_input_sha256,
            placement.product_input_sha256
        );
        assert_eq!(
            resolution.product_input_sha256,
            assembly.product_input_sha256
        );
        let resolution_sha256 = sha256_hex(production.resolution_json.as_bytes());
        assert_eq!(bom.product_resolution_sha256, resolution_sha256);
        assert_eq!(placement.product_resolution_sha256, resolution_sha256);
        assert_eq!(assembly.product_resolution_sha256, resolution_sha256);
        verify_product_artifact_bundle(&design, SNAPSHOT, "production", &production).unwrap();

        let alternate =
            compile_product_artifacts(&design, SNAPSHOT, "prototype_alternate").unwrap();
        let resolution: ResolutionArtifact = parse(&alternate.resolution_json);
        let bom: BomArtifact = parse(&alternate.bom_json);
        let placement: PlacementArtifact = parse(&alternate.placement_json);
        let assembly: AssemblyArtifact = parse(&alternate.assembly_json);
        assert_eq!(resolution.components.len(), 2);
        assert_eq!(
            resolution.components[0].state,
            ArtifactPopulationState::NotFitted
        );
        assert_eq!(resolution.components[0].selected_identity.0, None);
        assert_eq!(
            resolution.components[1].state,
            ArtifactPopulationState::Alternate
        );
        assert_eq!(
            resolution.components[1]
                .selected_identity
                .0
                .as_ref()
                .unwrap()
                .manufacturer,
            "Panasonic"
        );
        assert_eq!(bom.items.len(), 1);
        assert_eq!(bom.items[0].selected_identity.manufacturer, "Panasonic");
        assert_eq!(bom.items[0].per_board_quantity, 1);
        assert_eq!(bom.items[0].total_quantity, 2);
        assert_eq!(placement.placements.len(), 1);
        assert_eq!(assembly.components[0].per_board_quantity, 0);
        assert_eq!(assembly.components[0].total_quantity, 0);
        verify_product_artifact_bundle(&design, SNAPSHOT, "prototype_alternate", &alternate)
            .unwrap();
    }

    #[test]
    fn placement_and_assembly_preserve_exact_back_side_and_configuration_intent() {
        let mut design = voltage_divider();
        let component = design
            .components
            .iter_mut()
            .find(|component| component.path == "divider.r_bottom")
            .unwrap();
        let physical = component.physical.as_mut().unwrap();
        physical.placement.rotation_degrees = 90;
        physical.placement.layer = CopperLayer::Back;
        assert_eq!(design.validate(), Ok(()));

        let bundle = compile_product_artifacts(&design, SNAPSHOT, "production").unwrap();
        let placement: PlacementArtifact = parse(&bundle.placement_json);
        let row = placement
            .placements
            .iter()
            .find(|row| row.component_path == "divider.r_bottom")
            .unwrap();
        assert_eq!(row.reference, "R2");
        assert_eq!(row.x_nm, 25_000_000);
        assert_eq!(row.y_nm, 10_000_000);
        assert_eq!(row.rotation_degrees, 90);
        assert_eq!(row.side, BoardSide::Back);
        assert_eq!(row.selected_identity.value.coefficient, 1);
        assert_eq!(row.selected_identity.value.exponent, 4);

        let assembly: AssemblyArtifact = parse(&bundle.assembly_json);
        assert_eq!(assembly.build_quantity, 10);
        assert_eq!(
            assembly.configurations,
            vec![Configuration {
                key: "assembly_revision".to_owned(),
                value: "A".to_owned(),
            }]
        );
        let assembly_row = assembly
            .components
            .iter()
            .find(|row| row.component_path == "divider.r_bottom")
            .unwrap();
        assert_eq!(assembly_row.reference, "R2");
        assert_eq!(assembly_row.state, ArtifactPopulationState::Fitted);
        assert_eq!(
            assembly_row.selected_identity.0,
            Some(row.selected_identity.clone())
        );
        assert_eq!(assembly_row.per_board_quantity, 1);
        assert_eq!(assembly_row.total_quantity, 10);
    }

    #[test]
    fn repeat_and_permuted_inputs_emit_identical_bytes_and_safe_hashed_paths() {
        let mut canonical = voltage_divider();
        canonical.product.variants[0].path = "release/eu.production".to_owned();
        canonical.canonicalize();
        assert_eq!(canonical.validate(), Ok(()));
        let expected =
            compile_product_artifacts(&canonical, SNAPSHOT, "release/eu.production").unwrap();
        assert_eq!(
            expected,
            compile_product_artifacts(&canonical, SNAPSHOT, "release/eu.production").unwrap()
        );
        assert!(expected.resolution_path.as_str().starts_with("product/"));
        assert!(
            expected
                .resolution_path
                .as_str()
                .ends_with("/resolution.json")
        );
        assert!(!expected.resolution_path.as_str().contains("release/eu"));
        let base_path = format!("product/{}", expected.variant_identity_sha256);
        assert_eq!(
            expected.resolution_path.as_str(),
            format!("{base_path}/resolution.json")
        );
        assert_eq!(expected.bom_path.as_str(), format!("{base_path}/bom.json"));
        assert_eq!(
            expected.placement_path.as_str(),
            format!("{base_path}/placement.json")
        );
        assert_eq!(
            expected.assembly_path.as_str(),
            format!("{base_path}/assembly.json")
        );

        let mut permuted = canonical.clone();
        permuted.components.reverse();
        permuted.product.variants.reverse();
        for variant in &mut permuted.product.variants {
            variant.components.reverse();
            variant.configurations.reverse();
        }
        assert_eq!(permuted.validate(), Ok(()));
        assert_eq!(
            compile_product_artifacts(&permuted, SNAPSHOT, "release/eu.production").unwrap(),
            expected
        );

        let mut collided = expected.clone();
        collided.bom_path = collided.resolution_path.clone();
        assert_eq!(
            verify_product_artifact_bundle(
                &canonical,
                SNAPSHOT,
                "release/eu.production",
                &collided,
            )
            .unwrap_err()[0]
                .code,
            "CC-PRODUCT-ARTIFACT-VERIFY-001"
        );
    }

    #[test]
    fn unknown_variant_and_checked_bom_overflow_fail_closed() {
        let unknown =
            compile_product_artifacts(&voltage_divider(), SNAPSHOT, "missing").unwrap_err();
        assert_eq!(unknown[0].code, "CC-PRODUCT-ARTIFACT-VARIANT-001");

        let mut boundary = voltage_divider();
        let boundary_variant = boundary
            .product
            .variants
            .iter_mut()
            .find(|variant| variant.path == "production")
            .unwrap();
        boundary_variant.build_quantity = u64::MAX;
        boundary_variant.components[0].state = PopulationState::NotFitted;
        assert_eq!(boundary.validate(), Ok(()));
        let bundle = compile_product_artifacts(&boundary, SNAPSHOT, "production").unwrap();
        let bom: BomArtifact = parse(&bundle.bom_json);
        assert_eq!(bom.items.len(), 1);
        assert_eq!(bom.items[0].per_board_quantity, 1);
        assert_eq!(bom.items[0].total_quantity, u64::MAX);

        let mut overflow = voltage_divider();
        overflow
            .product
            .variants
            .iter_mut()
            .find(|variant| variant.path == "production")
            .unwrap()
            .build_quantity = u64::MAX;
        assert_eq!(overflow.validate(), Ok(()));
        let diagnostics = compile_product_artifacts(&overflow, SNAPSHOT, "production").unwrap_err();
        assert_eq!(diagnostics[0].code, "CC-PRODUCT-ARTIFACT-QUANTITY-001");
    }

    #[test]
    fn strict_parser_rejects_unknown_missing_duplicate_removed_and_extra_rows() {
        let design = voltage_divider();

        let mut unknown = compile("production");
        unknown.resolution_json.insert_str(1, "\"extra\":0,");
        assert_eq!(
            verify_product_artifact_bundle(&design, SNAPSHOT, "production", &unknown).unwrap_err()
                [0]
            .code,
            "CC-PRODUCT-ARTIFACT-CONTRACT-001"
        );

        let mut missing = compile("production");
        let mut value: serde_json::Value = serde_json::from_str(&missing.resolution_json).unwrap();
        value["components"][0]
            .as_object_mut()
            .unwrap()
            .remove("selected_identity");
        missing.resolution_json = render_value(&value);
        assert_eq!(
            verify_product_artifact_bundle(&design, SNAPSHOT, "production", &missing).unwrap_err()
                [0]
            .code,
            "CC-PRODUCT-ARTIFACT-CONTRACT-001"
        );

        let mut duplicate = compile("production");
        let mut resolution: ResolutionArtifact = parse(&duplicate.resolution_json);
        resolution
            .components
            .insert(1, resolution.components[0].clone());
        duplicate.resolution_json = serialize_canonical(&resolution, "resolution.json").unwrap();
        assert_eq!(
            verify_product_artifact_bundle(&design, SNAPSHOT, "production", &duplicate)
                .unwrap_err()[0]
                .code,
            "CC-PRODUCT-ARTIFACT-CONTRACT-004"
        );

        let mut removed = compile("production");
        let mut placement: PlacementArtifact = parse(&removed.placement_json);
        placement.placements.pop();
        removed.placement_json = serialize_canonical(&placement, "placement.json").unwrap();
        assert_eq!(
            verify_product_artifact_bundle(&design, SNAPSHOT, "production", &removed).unwrap_err()
                [0]
            .code,
            "CC-PRODUCT-ARTIFACT-JOIN-001"
        );

        let mut extra = compile("production");
        let mut assembly: AssemblyArtifact = parse(&extra.assembly_json);
        let mut row = assembly.components[1].clone();
        row.component_path = "divider.stale".to_owned();
        assembly.components.push(row);
        extra.assembly_json = serialize_canonical(&assembly, "assembly.json").unwrap();
        assert_eq!(
            verify_product_artifact_bundle(&design, SNAPSHOT, "production", &extra).unwrap_err()[0]
                .code,
            "CC-PRODUCT-ARTIFACT-JOIN-001"
        );

        let mut wrong_bom = compile("production");
        let mut bom: BomArtifact = parse(&wrong_bom.bom_json);
        bom.items[0].per_board_quantity = 3;
        bom.items[0].total_quantity = 30;
        wrong_bom.bom_json = serialize_canonical(&bom, "bom.json").unwrap();
        assert_eq!(
            verify_product_artifact_bundle(&design, SNAPSHOT, "production", &wrong_bom)
                .unwrap_err()[0]
                .code,
            "CC-PRODUCT-ARTIFACT-VERIFY-001"
        );
    }

    #[test]
    fn strict_parser_rejects_noncanonical_and_duplicate_json_forms() {
        let design = voltage_divider();

        let mut missing_lf = compile("production");
        missing_lf.resolution_json.pop();
        assert_eq!(
            verify_product_artifact_bundle(&design, SNAPSHOT, "production", &missing_lf)
                .unwrap_err()[0]
                .code,
            "CC-PRODUCT-ARTIFACT-CONTRACT-007"
        );

        let mut double_lf = compile("production");
        double_lf.resolution_json.push('\n');
        assert_eq!(
            verify_product_artifact_bundle(&design, SNAPSHOT, "production", &double_lf)
                .unwrap_err()[0]
                .code,
            "CC-PRODUCT-ARTIFACT-CONTRACT-007"
        );

        let mut pretty = compile("production");
        let value: serde_json::Value = serde_json::from_str(&pretty.resolution_json).unwrap();
        pretty.resolution_json = serde_json::to_string_pretty(&value).unwrap();
        pretty.resolution_json.push('\n');
        assert_eq!(
            verify_product_artifact_bundle(&design, SNAPSHOT, "production", &pretty).unwrap_err()
                [0]
            .code,
            "CC-PRODUCT-ARTIFACT-CONTRACT-007"
        );

        let mut duplicate_field = compile("production");
        duplicate_field.resolution_json = duplicate_field.resolution_json.replacen(
            "{\"schema_name\":",
            "{\"schema_name\":\"circuitc.product_resolution\",\"schema_name\":",
            1,
        );
        assert_eq!(
            verify_product_artifact_bundle(&design, SNAPSHOT, "production", &duplicate_field)
                .unwrap_err()[0]
                .code,
            "CC-PRODUCT-ARTIFACT-CONTRACT-001"
        );

        let mut trailing = compile("production");
        trailing.resolution_json.push('x');
        assert_eq!(
            verify_product_artifact_bundle(&design, SNAPSHOT, "production", &trailing).unwrap_err()
                [0]
            .code,
            "CC-PRODUCT-ARTIFACT-CONTRACT-001"
        );

        let mut marker_collision = compile("production");
        marker_collision.resolution_json = marker_collision.resolution_json.replacen(
            "\"state\":\"fitted\"",
            &format!("\"state\":\"{ROW_LIMIT_MARKER}\""),
            1,
        );
        assert_eq!(
            verify_product_artifact_bundle(&design, SNAPSHOT, "production", &marker_collision,)
                .unwrap_err()[0]
                .code,
            "CC-PRODUCT-ARTIFACT-CONTRACT-001"
        );
    }

    #[test]
    fn primary_row_limit_accepts_exactly_ten_thousand_and_preflights_one_over() {
        fn synthetic_resolution(row_count: usize, invalid_first_reference: bool) -> String {
            let mut resolution: ResolutionArtifact = parse(&compile("production").resolution_json);
            let template = resolution.components[0].clone();
            resolution.components = (0..row_count)
                .map(|index| {
                    let mut row = template.clone();
                    row.component_path = format!("fixture.component.{index:05}");
                    row.reference = if invalid_first_reference && index == 0 {
                        String::new()
                    } else {
                        format!("R{index}")
                    };
                    row
                })
                .collect();
            let mut json = serde_json::to_string(&resolution).unwrap();
            json.push('\n');
            json
        }

        let mut diagnostics = Vec::new();
        assert!(
            parse_artifact::<ResolutionArtifact>(
                &synthetic_resolution(MAX_ARTIFACT_ROWS, false),
                "resolution.json",
                &mut diagnostics,
            )
            .is_some()
        );
        assert!(diagnostics.is_empty());

        let mut diagnostics = Vec::new();
        assert!(
            parse_artifact::<ResolutionArtifact>(
                &synthetic_resolution(MAX_ARTIFACT_ROWS + 1, true),
                "resolution.json",
                &mut diagnostics,
            )
            .is_none()
        );
        assert_eq!(diagnostics.len(), 1);
        assert_eq!(diagnostics[0].code, "CC-PRODUCT-ARTIFACT-RESOURCE-001");
    }

    #[test]
    fn coordinated_semantic_staleness_and_input_fingerprint_drift_are_rejected() {
        let design = voltage_divider();
        let mut stale = compile("production");
        for json in [
            &mut stale.resolution_json,
            &mut stale.bom_json,
            &mut stale.placement_json,
            &mut stale.assembly_json,
        ] {
            *json = json.replace("Yageo", "Panasonic");
        }
        let rebound_resolution_sha256 = sha256_hex(stale.resolution_json.as_bytes());
        let mut bom: BomArtifact = parse(&stale.bom_json);
        bom.product_resolution_sha256 = rebound_resolution_sha256.clone();
        stale.bom_json = serialize_canonical(&bom, "bom.json").unwrap();
        let mut placement: PlacementArtifact = parse(&stale.placement_json);
        placement.product_resolution_sha256 = rebound_resolution_sha256.clone();
        stale.placement_json = serialize_canonical(&placement, "placement.json").unwrap();
        let mut assembly: AssemblyArtifact = parse(&stale.assembly_json);
        assembly.product_resolution_sha256 = rebound_resolution_sha256;
        stale.assembly_json = serialize_canonical(&assembly, "assembly.json").unwrap();
        assert_eq!(
            verify_product_artifact_bundle(&design, SNAPSHOT, "production", &stale).unwrap_err()[0]
                .code,
            "CC-PRODUCT-ARTIFACT-VERIFY-001"
        );

        let mut omitted = compile("production");
        let mut resolution: ResolutionArtifact = parse(&omitted.resolution_json);
        let removed_path = resolution.components.remove(0).component_path;
        omitted.resolution_json = serialize_canonical(&resolution, "resolution.json").unwrap();
        let rebound_resolution_sha256 = sha256_hex(omitted.resolution_json.as_bytes());
        let mut bom: BomArtifact = parse(&omitted.bom_json);
        bom.items[0].per_board_quantity = 1;
        bom.items[0].total_quantity = 10;
        bom.product_resolution_sha256 = rebound_resolution_sha256.clone();
        omitted.bom_json = serialize_canonical(&bom, "bom.json").unwrap();
        let mut placement: PlacementArtifact = parse(&omitted.placement_json);
        placement
            .placements
            .retain(|row| row.component_path != removed_path);
        placement.product_resolution_sha256 = rebound_resolution_sha256.clone();
        omitted.placement_json = serialize_canonical(&placement, "placement.json").unwrap();
        let mut assembly: AssemblyArtifact = parse(&omitted.assembly_json);
        assembly
            .components
            .retain(|row| row.component_path != removed_path);
        assembly.product_resolution_sha256 = rebound_resolution_sha256;
        omitted.assembly_json = serialize_canonical(&assembly, "assembly.json").unwrap();
        assert_eq!(
            verify_product_artifact_bundle(&design, SNAPSHOT, "production", &omitted).unwrap_err()
                [0]
            .code,
            "CC-PRODUCT-ARTIFACT-VERIFY-001"
        );

        let original = compile("production");
        let original_root: ResolutionArtifact = parse(&original.resolution_json);
        let mut sourcing_changed = design.clone();
        for component in &mut sourcing_changed.components {
            if let Some(sourcing) = &mut component.part.sourcing {
                sourcing.minimum_available_quantity = 2;
            }
        }
        assert_eq!(sourcing_changed.validate(), Ok(()));

        let mut placement_changed = design.clone();
        placement_changed.components[0]
            .physical
            .as_mut()
            .unwrap()
            .placement
            .position
            .x += 1;
        assert_eq!(placement_changed.validate(), Ok(()));

        let mut configuration_changed = design.clone();
        configuration_changed.product.variants[0].configurations[0].value = "B".to_owned();
        assert_eq!(configuration_changed.validate(), Ok(()));

        let mut build_changed = design.clone();
        build_changed.product.variants[0].build_quantity += 1;
        assert_eq!(build_changed.validate(), Ok(()));

        let mut population_changed = design;
        population_changed.product.variants[0].components[0].state = PopulationState::NotFitted;
        assert_eq!(population_changed.validate(), Ok(()));

        for changed in [
            sourcing_changed,
            placement_changed,
            configuration_changed,
            build_changed,
            population_changed,
        ] {
            let changed = compile_product_artifacts(&changed, SNAPSHOT, "production").unwrap();
            let changed_root: ResolutionArtifact = parse(&changed.resolution_json);
            assert_ne!(
                original_root.product_input_sha256,
                changed_root.product_input_sha256
            );
        }
    }

    #[test]
    fn aggregate_bundle_budget_is_checked_without_overflow() {
        assert!(enforce_aggregate_byte_budget([MAX_ARTIFACT_BYTES, 0, 0, 0]).is_ok());
        assert_eq!(
            enforce_aggregate_byte_budget([MAX_ARTIFACT_BYTES, 1, 0, 0]).unwrap_err()[0].code,
            "CC-PRODUCT-ARTIFACT-RESOURCE-001"
        );
        assert!(enforce_aggregate_byte_budget([usize::MAX, 1, 0, 0]).is_err());
    }

    #[test]
    fn product_input_fingerprint_covers_every_documented_authority_field() {
        fn digest(mutate: impl FnOnce(&mut Design, &mut CatalogResolution)) -> String {
            let mut design = voltage_divider();
            let mut catalog = verify_product_catalog(&design, SNAPSHOT).unwrap();
            mutate(&mut design, &mut catalog);
            let variant = &design.product.variants[0];
            compile_product_input_sha256(&design, variant, &catalog).unwrap()
        }

        let original = digest(|_, _| {});
        let changed = vec![
            digest(|design, _| design.name.push_str("_changed")),
            digest(|_, catalog| catalog.snapshot_id.push_str("-changed")),
            digest(|_, catalog| catalog.snapshot_sha256 = "b".repeat(64)),
            digest(|_, catalog| catalog.evaluated_on = "2026-08-05".to_owned()),
            digest(|design, _| design.product.variants[0].path.push_str(".changed")),
            digest(|design, _| design.product.variants[0].build_quantity += 1),
            digest(|design, _| {
                design.product.variants[0].configurations[0]
                    .value
                    .push_str("-changed");
            }),
            digest(|design, _| {
                design.product.variants[0].configurations[0]
                    .key
                    .push_str("_changed");
            }),
            digest(|design, _| {
                let old_path = design.components[0].path.clone();
                design.components[0].path.push_str(".changed");
                let new_path = design.components[0].path.clone();
                for variant in &mut design.product.variants {
                    variant
                        .components
                        .iter_mut()
                        .find(|assignment| assignment.component_path == old_path)
                        .unwrap()
                        .component_path = new_path.clone();
                }
            }),
            digest(|design, _| design.components[0].reference.push_str("_CHANGED")),
            digest(|design, _| {
                design.components[0]
                    .part
                    .logical_function
                    .push_str("_changed");
            }),
            digest(|design, _| {
                design.components[0]
                    .part
                    .manufacturer
                    .as_mut()
                    .unwrap()
                    .push_str("_changed");
            }),
            digest(|design, _| {
                design.components[0]
                    .part
                    .manufacturer_part_number
                    .as_mut()
                    .unwrap()
                    .push_str("_changed");
            }),
            digest(|design, _| {
                design.components[0]
                    .part
                    .package
                    .as_mut()
                    .unwrap()
                    .push_str("_changed");
            }),
            digest(|design, _| {
                design.components[0].value = ComponentValue::Resistance(
                    crate::quantity::Quantity::new(11, 3, crate::quantity::Unit::Ohm),
                );
            }),
            digest(|design, _| {
                design.components[0].part.lifecycle =
                    Some(LifecycleStatus::NotRecommendedForNewDesigns);
            }),
            digest(|design, _| {
                design.components[0]
                    .part
                    .sourcing
                    .as_mut()
                    .unwrap()
                    .maximum_lead_time_days -= 1;
            }),
            digest(|design, _| {
                design.components[0]
                    .part
                    .sourcing
                    .as_mut()
                    .unwrap()
                    .required_region
                    .push_str("-changed");
            }),
            digest(|design, _| {
                design.components[0].part.approved_substitutions[0]
                    .manufacturer
                    .push_str("-changed");
            }),
            digest(|design, _| {
                design.components[0].part.approved_substitutions[0]
                    .manufacturer_part_number
                    .push_str("-changed");
            }),
            digest(|design, _| {
                design.components[0].part.approved_substitutions[0]
                    .package
                    .push_str("-changed");
            }),
            digest(|design, _| {
                design.components[0]
                    .physical
                    .as_mut()
                    .unwrap()
                    .placement
                    .position
                    .y += 1;
            }),
            digest(|design, _| {
                design.components[0]
                    .physical
                    .as_mut()
                    .unwrap()
                    .placement
                    .rotation_degrees = 90;
            }),
            digest(|design, _| {
                design.components[0]
                    .physical
                    .as_mut()
                    .unwrap()
                    .placement
                    .layer = CopperLayer::Back;
            }),
            digest(|design, _| {
                design.product.variants[0].components[0].state = PopulationState::Alternate(
                    design.components[0].part.approved_substitutions[0].clone(),
                );
            }),
        ];
        for fingerprint in changed {
            assert_ne!(fingerprint, original);
        }

        let alternate_base = digest(|design, _| {
            design.product.variants[0].components[0].state = PopulationState::Alternate(
                design.components[0].part.approved_substitutions[0].clone(),
            );
        });
        let alternate_fields_changed = vec![
            digest(|design, _| {
                let mut alternate = design.components[0].part.approved_substitutions[0].clone();
                alternate.manufacturer.push_str("-selected");
                design.product.variants[0].components[0].state =
                    PopulationState::Alternate(alternate);
            }),
            digest(|design, _| {
                let mut alternate = design.components[0].part.approved_substitutions[0].clone();
                alternate.manufacturer_part_number.push_str("-selected");
                design.product.variants[0].components[0].state =
                    PopulationState::Alternate(alternate);
            }),
            digest(|design, _| {
                let mut alternate = design.components[0].part.approved_substitutions[0].clone();
                alternate.package.push_str("-selected");
                design.product.variants[0].components[0].state =
                    PopulationState::Alternate(alternate);
            }),
        ];
        for fingerprint in alternate_fields_changed {
            assert_ne!(fingerprint, alternate_base);
        }
    }
}
