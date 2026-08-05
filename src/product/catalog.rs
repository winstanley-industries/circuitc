use std::collections::{BTreeMap, BTreeSet};
use std::fmt;

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use crate::design::{Component, ComponentValue, Design, LifecycleStatus, SourcingConstraints};

const SCHEMA_NAME: &str = "circuitc.product_catalog_snapshot";
const SCHEMA_VERSION: u32 = 1;
const MAX_BYTES: usize = 64 * 1024 * 1024;
const MAX_ENTRIES: usize = 10_000;
const MAX_RESOLUTION_ENTRIES: usize = 10_000;
const MAX_DIAGNOSTICS: usize = 256;

type PartKey<'a> = (&'a str, &'a str, &'a str, &'a str);
type PartIndex<'a> = BTreeMap<PartKey<'a>, &'a PartRecord>;

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct CatalogDiagnostic {
    pub code: &'static str,
    pub path: String,
    pub message: String,
}

impl fmt::Display for CatalogDiagnostic {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(formatter, "{} [{}]: {}", self.code, self.path, self.message)
    }
}

impl std::error::Error for CatalogDiagnostic {}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct CatalogResolution {
    pub snapshot_id: String,
    pub snapshot_sha256: String,
    pub evaluated_on: String,
    pub parts: Vec<ResolvedCatalogPart>,
}

#[derive(Clone, Debug, Eq, Ord, PartialEq, PartialOrd)]
pub struct ResolvedCatalogPart {
    pub component_path: String,
    pub alternate: bool,
    pub manufacturer: String,
    pub manufacturer_part_number: String,
    pub package: String,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Snapshot {
    schema_name: String,
    schema_version: u32,
    snapshot_id: String,
    observed_on: String,
    valid_through: String,
    source_uri: String,
    raw_source_sha256: String,
    parts: Vec<PartRecord>,
}

#[derive(Clone, Debug, Eq, Ord, PartialEq, PartialOrd, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct PartRecord {
    logical_function: String,
    manufacturer: String,
    manufacturer_part_number: String,
    package: String,
    value: CatalogValue,
    lifecycle: CatalogLifecycle,
    availability: Vec<Availability>,
}

#[derive(Clone, Debug, Eq, Ord, PartialEq, PartialOrd, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct CatalogValue {
    kind: ValueKind,
    coefficient: i64,
    exponent: i8,
    unit: ValueUnit,
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

#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
enum CatalogLifecycle {
    Active,
    NotRecommendedForNewDesigns,
    Obsolete,
}

#[derive(Clone, Debug, Eq, Ord, PartialEq, PartialOrd, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Availability {
    region: String,
    available_quantity: u64,
    lead_time_days: u32,
}

pub fn verify_product_catalog(
    design: &Design,
    snapshot_bytes: &[u8],
) -> Result<CatalogResolution, Vec<CatalogDiagnostic>> {
    if design.validate().is_err() {
        return Err(vec![diagnostic(
            "CC-CATALOG-CONTRACT-008",
            "design",
            "catalog resolution requires a valid canonical Design IR",
        )]);
    }
    let Some(reference) = design.product.catalog.as_ref() else {
        return Err(vec![diagnostic(
            "CC-CATALOG-AUTH-001",
            "design.product.catalog",
            "catalog resolution requires a pinned catalog reference",
        )]);
    };
    if snapshot_bytes.len() > MAX_BYTES {
        return Err(vec![diagnostic(
            "CC-CATALOG-CONTRACT-006",
            "document",
            format!("catalog snapshot exceeds the {MAX_BYTES}-byte limit"),
        )]);
    }
    let actual_sha256 = sha256_hex(snapshot_bytes);
    if actual_sha256 != reference.sha256 {
        return Err(vec![diagnostic(
            "CC-CATALOG-AUTH-002",
            "design.product.catalog.sha256",
            "catalog bytes do not match the Design IR SHA-256 reference",
        )]);
    }
    let snapshot = parse_snapshot(snapshot_bytes)?;
    let mut diagnostics = Vec::new();
    if snapshot.snapshot_id != reference.snapshot_id {
        push(
            &mut diagnostics,
            "CC-CATALOG-AUTH-003",
            "snapshot_id",
            "catalog snapshot identity does not match Design IR",
        );
    }
    if reference.evaluated_on < snapshot.observed_on
        || reference.evaluated_on > snapshot.valid_through
    {
        push(
            &mut diagnostics,
            "CC-CATALOG-AUTH-004",
            "design.product.catalog.evaluated_on",
            "catalog evaluation date is outside the inclusive snapshot validity interval",
        );
    }
    if !diagnostics.is_empty() {
        return Err(diagnostics);
    }

    let mut resolved = Vec::new();
    let mut components: Vec<_> = design
        .components
        .iter()
        .filter(|component| component.part.manufacturer.is_some())
        .collect();
    components.sort_by(|left, right| left.path.cmp(&right.path));
    let resolution_entries = resolution_entry_count(&components);
    if resolution_entries.is_none_or(|entries| entries > MAX_RESOLUTION_ENTRIES) {
        return Err(vec![diagnostic(
            "CC-CATALOG-RESOLVE-007",
            "design.components",
            format!(
                "catalog resolution exceeds the {MAX_RESOLUTION_ENTRIES}-entry primary-and-alternate limit"
            ),
        )]);
    }
    resolved.reserve(resolution_entries.unwrap_or_default());
    let records: PartIndex<'_> = snapshot
        .parts
        .iter()
        .map(|record| {
            (
                (
                    record.logical_function.as_str(),
                    record.manufacturer.as_str(),
                    record.manufacturer_part_number.as_str(),
                    record.package.as_str(),
                ),
                record,
            )
        })
        .collect();
    for component in components {
        let part = &component.part;
        if let (Some(manufacturer), Some(number), Some(package)) = (
            part.manufacturer.as_deref(),
            part.manufacturer_part_number.as_deref(),
            part.package.as_deref(),
        ) {
            resolve_one(
                component,
                false,
                manufacturer,
                number,
                package,
                &records,
                &mut resolved,
                &mut diagnostics,
            );
        }
        let mut alternates: Vec<_> = part.approved_substitutions.iter().collect();
        alternates.sort();
        for alternate in alternates {
            resolve_one(
                component,
                true,
                &alternate.manufacturer,
                &alternate.manufacturer_part_number,
                &alternate.package,
                &records,
                &mut resolved,
                &mut diagnostics,
            );
        }
    }
    if !diagnostics.is_empty() {
        return Err(diagnostics);
    }
    resolved.sort();
    Ok(CatalogResolution {
        snapshot_id: snapshot.snapshot_id,
        snapshot_sha256: actual_sha256,
        evaluated_on: reference.evaluated_on.clone(),
        parts: resolved,
    })
}

fn resolution_entry_count(components: &[&Component]) -> Option<usize> {
    components.iter().try_fold(0_usize, |total, component| {
        total.checked_add(1 + component.part.approved_substitutions.len())
    })
}

#[allow(clippy::too_many_arguments)]
fn resolve_one(
    component: &Component,
    alternate: bool,
    manufacturer: &str,
    number: &str,
    package: &str,
    records: &PartIndex<'_>,
    resolved: &mut Vec<ResolvedCatalogPart>,
    diagnostics: &mut Vec<CatalogDiagnostic>,
) {
    let Some(record) = records.get(&(
        component.part.logical_function.as_str(),
        manufacturer,
        number,
        package,
    )) else {
        push(
            diagnostics,
            "CC-CATALOG-RESOLVE-001",
            &component.path,
            format!(
                "catalog identity {manufacturer} / {number} / {package} resolved to 0 records; expected exactly one"
            ),
        );
        return;
    };
    if !value_matches(component, &record.value) {
        push(
            diagnostics,
            "CC-CATALOG-RESOLVE-002",
            &component.path,
            "catalog part value is incompatible with the exact authored component value",
        );
    }
    if component.part.lifecycle.map(catalog_lifecycle) != Some(record.lifecycle) {
        push(
            diagnostics,
            "CC-CATALOG-RESOLVE-003",
            &component.path,
            "catalog lifecycle observation does not equal the authored requirement",
        );
    }
    if let Some(sourcing) = &component.part.sourcing {
        validate_sourcing(component, sourcing, record, diagnostics);
    }
    resolved.push(ResolvedCatalogPart {
        component_path: component.path.clone(),
        alternate,
        manufacturer: manufacturer.to_owned(),
        manufacturer_part_number: number.to_owned(),
        package: package.to_owned(),
    });
}

fn validate_sourcing(
    component: &Component,
    sourcing: &SourcingConstraints,
    record: &PartRecord,
    diagnostics: &mut Vec<CatalogDiagnostic>,
) {
    let Some(observation) = record
        .availability
        .iter()
        .find(|entry| entry.region == sourcing.required_region)
    else {
        push(
            diagnostics,
            "CC-CATALOG-RESOLVE-004",
            &component.path,
            format!(
                "catalog has no availability observation for required region {}",
                sourcing.required_region
            ),
        );
        return;
    };
    if observation.available_quantity < sourcing.minimum_available_quantity {
        push(
            diagnostics,
            "CC-CATALOG-RESOLVE-005",
            &component.path,
            "catalog available quantity is below the authored minimum",
        );
    }
    if observation.lead_time_days > sourcing.maximum_lead_time_days {
        push(
            diagnostics,
            "CC-CATALOG-RESOLVE-006",
            &component.path,
            "catalog lead time exceeds the authored maximum",
        );
    }
}

fn value_matches(component: &Component, value: &CatalogValue) -> bool {
    let quantity = component.value.quantity();
    let (kind, unit) = match component.value {
        ComponentValue::Resistance(_) => (ValueKind::Resistance, ValueUnit::Ohm),
        ComponentValue::DcVoltage(_) => (ValueKind::DcVoltage, ValueUnit::Volt),
    };
    value.kind == kind
        && value.unit == unit
        && value.coefficient == quantity.coefficient
        && value.exponent == quantity.exponent
}

fn catalog_lifecycle(value: LifecycleStatus) -> CatalogLifecycle {
    match value {
        LifecycleStatus::Active => CatalogLifecycle::Active,
        LifecycleStatus::NotRecommendedForNewDesigns => {
            CatalogLifecycle::NotRecommendedForNewDesigns
        }
        LifecycleStatus::Obsolete => CatalogLifecycle::Obsolete,
    }
}

fn parse_snapshot(bytes: &[u8]) -> Result<Snapshot, Vec<CatalogDiagnostic>> {
    if bytes.len() > MAX_BYTES {
        return Err(vec![diagnostic(
            "CC-CATALOG-CONTRACT-006",
            "document",
            format!("catalog snapshot exceeds the {MAX_BYTES}-byte limit"),
        )]);
    }
    let input = std::str::from_utf8(bytes).map_err(|_| {
        vec![diagnostic(
            "CC-CATALOG-CONTRACT-001",
            "document",
            "catalog snapshot is not UTF-8",
        )]
    })?;
    let snapshot: Snapshot = serde_json::from_str(input).map_err(|_| {
        vec![diagnostic(
            "CC-CATALOG-CONTRACT-001",
            "document",
            "catalog snapshot is not strict JSON matching the v1 schema",
        )]
    })?;
    let mut diagnostics = Vec::new();
    validate_snapshot(&snapshot, &mut diagnostics);
    if diagnostics.is_empty() {
        let mut canonical = serde_json::to_string(&snapshot).map_err(|_| {
            vec![diagnostic(
                "CC-CATALOG-CONTRACT-001",
                "document",
                "catalog snapshot could not be canonically serialized",
            )]
        })?;
        canonical.push('\n');
        if canonical.as_bytes() != bytes {
            push(
                &mut diagnostics,
                "CC-CATALOG-CONTRACT-007",
                "document",
                "catalog snapshot bytes are not canonical compact JSON with one final LF",
            );
        }
    }
    if diagnostics.is_empty() {
        Ok(snapshot)
    } else {
        Err(diagnostics)
    }
}

fn validate_snapshot(snapshot: &Snapshot, diagnostics: &mut Vec<CatalogDiagnostic>) {
    if snapshot.schema_name != SCHEMA_NAME || snapshot.schema_version != SCHEMA_VERSION {
        push(
            diagnostics,
            "CC-CATALOG-CONTRACT-002",
            "schema",
            "unsupported product catalog schema name or version",
        );
    }
    if !token_is_valid(&snapshot.snapshot_id) {
        push(
            diagnostics,
            "CC-CATALOG-CONTRACT-003",
            "snapshot_id",
            "snapshot identity is not a canonical token",
        );
    }
    if !date_is_valid(&snapshot.observed_on)
        || !date_is_valid(&snapshot.valid_through)
        || snapshot.valid_through < snapshot.observed_on
    {
        push(
            diagnostics,
            "CC-CATALOG-CONTRACT-003",
            "validity",
            "snapshot validity dates must be real ordered Gregorian dates",
        );
    }
    if !uri_is_valid(&snapshot.source_uri) || !sha256_is_valid(&snapshot.raw_source_sha256) {
        push(
            diagnostics,
            "CC-CATALOG-CONTRACT-003",
            "provenance",
            "snapshot provenance URI or digest is not canonical",
        );
    }
    if snapshot.parts.len() > MAX_ENTRIES {
        push(
            diagnostics,
            "CC-CATALOG-CONTRACT-006",
            "parts",
            "catalog part collection exceeds 10000 entries",
        );
        return;
    }
    let availability_count = snapshot.parts.iter().try_fold(0_usize, |total, part| {
        total.checked_add(part.availability.len())
    });
    if availability_count.is_none_or(|count| count > MAX_ENTRIES) {
        push(
            diagnostics,
            "CC-CATALOG-CONTRACT-006",
            "parts.availability",
            "catalog availability collection exceeds 10000 aggregate entries",
        );
        return;
    }
    let mut previous = None;
    for (index, part) in snapshot.parts.iter().enumerate() {
        let path = format!("parts[{index}]");
        let key = (
            &part.logical_function,
            &part.manufacturer,
            &part.manufacturer_part_number,
            &part.package,
        );
        if previous.is_some_and(|prior| prior >= key) {
            push(
                diagnostics,
                "CC-CATALOG-CONTRACT-004",
                &path,
                "catalog parts must be strictly sorted and unique by exact identity",
            );
        }
        previous = Some(key);
        if !token_is_valid(&part.logical_function)
            || !text_is_valid(&part.manufacturer)
            || !text_is_valid(&part.manufacturer_part_number)
            || !text_is_valid(&part.package)
        {
            push(
                diagnostics,
                "CC-CATALOG-CONTRACT-003",
                &path,
                "catalog part identity is not canonical",
            );
        }
        if !catalog_value_is_valid(&part.logical_function, &part.value) {
            push(
                diagnostics,
                "CC-CATALOG-CONTRACT-003",
                format!("{path}.value"),
                "catalog part value is not a supported canonical exact value",
            );
        }
        let mut regions = BTreeSet::new();
        let mut prior_region: Option<&str> = None;
        for availability in &part.availability {
            if !token_is_valid(&availability.region)
                || prior_region.is_some_and(|prior| prior >= availability.region.as_str())
                || !regions.insert(availability.region.as_str())
            {
                push(
                    diagnostics,
                    "CC-CATALOG-CONTRACT-005",
                    format!("{path}.availability"),
                    "availability entries must be strictly sorted and unique by canonical region",
                );
            }
            prior_region = Some(&availability.region);
        }
    }
}

fn catalog_value_is_valid(function: &str, value: &CatalogValue) -> bool {
    if !(-18..=18).contains(&value.exponent) {
        return false;
    }
    let pair = matches!(
        (function, value.kind, value.unit),
        ("resistor", ValueKind::Resistance, ValueUnit::Ohm)
            | ("dc_voltage_source", ValueKind::DcVoltage, ValueUnit::Volt)
    );
    let canonical = value.coefficient == 0 && value.exponent == 0
        || value.coefficient != 0 && (value.coefficient % 10 != 0 || value.exponent == 18);
    pair && canonical && (value.kind != ValueKind::Resistance || value.coefficient > 0)
}

fn token_is_valid(value: &str) -> bool {
    !value.is_empty()
        && value
            .chars()
            .all(|character| character.is_ascii_alphanumeric() || "_+-./".contains(character))
}

fn text_is_valid(value: &str) -> bool {
    !value.is_empty() && value.trim() == value && !value.chars().any(char::is_control)
}

fn uri_is_valid(value: &str) -> bool {
    if value.is_empty()
        || !value.is_ascii()
        || !value.starts_with("https://")
        || value.chars().any(|character| {
            matches!(character, '#' | '\\') || character.is_whitespace() || character.is_control()
        })
    {
        return false;
    }
    let remainder = &value[8..];
    let authority_end = remainder.find(['/', '?']).unwrap_or(remainder.len());
    let authority = &remainder[..authority_end];
    let (host, port) = authority
        .rsplit_once(':')
        .map_or((authority, None), |(host, port)| (host, Some(port)));
    if host.is_empty()
        || host != host.to_ascii_lowercase()
        || host.starts_with('.')
        || host.ends_with('.')
        || host.split('.').any(|label| {
            label.is_empty()
                || label.starts_with('-')
                || label.ends_with('-')
                || !label
                    .bytes()
                    .all(|byte| byte.is_ascii_alphanumeric() || byte == b'-')
        })
        || port.is_some_and(|port| {
            port.is_empty()
                || (port.len() > 1 && port.starts_with('0'))
                || !port.bytes().all(|byte| byte.is_ascii_digit())
                || port
                    .parse::<u16>()
                    .map_or(true, |port| port == 0 || port == 443)
        })
    {
        return false;
    }
    let path_and_query = &remainder[authority_end..];
    let path = path_and_query
        .split_once('?')
        .map_or(path_and_query, |(path, _)| path);
    if path.split('/').any(|segment| matches!(segment, "." | "..")) {
        return false;
    }
    let bytes = value.as_bytes();
    let mut index = 0;
    while index < bytes.len() {
        if bytes[index] != b'%' {
            if !uri_character_is_allowed(bytes[index]) {
                return false;
            }
            index += 1;
            continue;
        }
        let Some(escape) = bytes.get(index + 1..index + 3) else {
            return false;
        };
        if !escape
            .iter()
            .all(|byte| byte.is_ascii_digit() || (b'A'..=b'F').contains(byte))
        {
            return false;
        }
        let decoded = (hex_value(escape[0]) << 4) | hex_value(escape[1]);
        if decoded.is_ascii_alphanumeric() || b"-._~".contains(&decoded) {
            return false;
        }
        index += 3;
    }
    true
}

fn uri_character_is_allowed(byte: u8) -> bool {
    byte.is_ascii_alphanumeric() || b"-._~:/?@!$&'()*+,;=".contains(&byte)
}

fn hex_value(byte: u8) -> u8 {
    match byte {
        b'0'..=b'9' => byte - b'0',
        b'A'..=b'F' => byte - b'A' + 10,
        _ => unreachable!("URI escapes are validated before decoding"),
    }
}

fn sha256_is_valid(value: &str) -> bool {
    value.len() == 64
        && value
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
}

fn sha256_hex(bytes: &[u8]) -> String {
    Sha256::digest(bytes)
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect()
}

fn date_is_valid(value: &str) -> bool {
    let bytes = value.as_bytes();
    if bytes.len() != 10 || bytes[4] != b'-' || bytes[7] != b'-' {
        return false;
    }
    let parse = |range: std::ops::Range<usize>| value[range].parse::<u32>().ok();
    let (Some(year), Some(month), Some(day)) = (parse(0..4), parse(5..7), parse(8..10)) else {
        return false;
    };
    if year == 0 || !(1..=12).contains(&month) {
        return false;
    }
    let leap = year % 4 == 0 && (year % 100 != 0 || year % 400 == 0);
    let days = [
        31,
        if leap { 29 } else { 28 },
        31,
        30,
        31,
        30,
        31,
        31,
        30,
        31,
        30,
        31,
    ];
    day != 0 && day <= days[(month - 1) as usize]
}

fn diagnostic(
    code: &'static str,
    path: impl Into<String>,
    message: impl Into<String>,
) -> CatalogDiagnostic {
    CatalogDiagnostic {
        code,
        path: path.into(),
        message: message.into(),
    }
}

fn push(
    diagnostics: &mut Vec<CatalogDiagnostic>,
    code: &'static str,
    path: impl Into<String>,
    message: impl Into<String>,
) {
    if diagnostics.len() < MAX_DIAGNOSTICS {
        diagnostics.push(diagnostic(code, path, message));
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::demo::voltage_divider;
    use crate::design::{ApprovedSubstitution, PopulationState, VariantComponent};

    const SNAPSHOT: &[u8] = include_bytes!("../../catalogs/reference-catalog.json");

    fn design() -> Design {
        voltage_divider()
    }

    fn verify_rebound(bytes: &[u8]) -> Result<CatalogResolution, Vec<CatalogDiagnostic>> {
        let mut design = design();
        design.product.catalog.as_mut().unwrap().sha256 = sha256_hex(bytes);
        verify_product_catalog(&design, bytes)
    }

    fn snapshot() -> Snapshot {
        serde_json::from_slice(SNAPSHOT).unwrap()
    }

    fn render_snapshot(snapshot: &Snapshot) -> Vec<u8> {
        let mut bytes = serde_json::to_vec(snapshot).unwrap();
        bytes.push(b'\n');
        bytes
    }

    fn missing_substitution(index: usize) -> ApprovedSubstitution {
        ApprovedSubstitution {
            manufacturer: "Missing".to_owned(),
            manufacturer_part_number: format!("MISSING-{index:03}"),
            package: "0603_1608Metric".to_owned(),
        }
    }

    fn design_with_missing_alternates(count: usize, reverse: bool) -> Design {
        let mut design = design();
        let mut substitutions: Vec<_> = (0..count).map(missing_substitution).collect();
        let selected = substitutions[0].clone();
        if reverse {
            substitutions.reverse();
        }
        for component in &mut design.components {
            if component.part.manufacturer.is_some() {
                component.part.approved_substitutions = substitutions.clone();
            }
        }
        for variant in &mut design.product.variants {
            for component in &mut variant.components {
                if matches!(component.state, PopulationState::Alternate(_)) {
                    component.state = PopulationState::Alternate(selected.clone());
                }
            }
        }
        design
    }

    #[test]
    fn canonical_snapshot_resolves_every_base_and_approved_alternate_offline() {
        let resolution = verify_product_catalog(&design(), SNAPSHOT).expect("snapshot resolves");
        assert_eq!(resolution.parts.len(), 4);
        assert_eq!(resolution.snapshot_sha256, sha256_hex(SNAPSHOT));
        assert_eq!(verify_product_catalog(&design(), SNAPSHOT), Ok(resolution));
    }

    #[test]
    fn manufacturer_identified_parts_resolve_without_board_placement() {
        let mut design = design();
        let component_path = design.components[0].path.clone();
        design.components[0].physical = None;
        assert_eq!(design.validate(), Ok(()));

        let resolution = verify_product_catalog(&design, SNAPSHOT).unwrap();
        assert_eq!(resolution.parts.len(), 4);
        assert_eq!(
            resolution
                .parts
                .iter()
                .filter(|part| part.component_path == component_path)
                .count(),
            2
        );
    }

    #[test]
    fn exact_catalog_value_rules_cover_signed_voltage_and_positive_resistance() {
        for coefficient in [-1, 0, 1] {
            assert!(catalog_value_is_valid(
                "dc_voltage_source",
                &CatalogValue {
                    kind: ValueKind::DcVoltage,
                    coefficient,
                    exponent: 0,
                    unit: ValueUnit::Volt,
                }
            ));
        }
        assert!(catalog_value_is_valid(
            "resistor",
            &CatalogValue {
                kind: ValueKind::Resistance,
                coefficient: 1,
                exponent: 0,
                unit: ValueUnit::Ohm,
            }
        ));
        for coefficient in [-1, 0] {
            let mut invalid = snapshot();
            invalid.parts[0].value.coefficient = coefficient;
            invalid.parts[0].value.exponent = 0;
            assert_eq!(
                verify_rebound(&render_snapshot(&invalid)).unwrap_err()[0].code,
                "CC-CATALOG-CONTRACT-003"
            );
        }
        assert!(!catalog_value_is_valid(
            "resistor",
            &CatalogValue {
                kind: ValueKind::DcVoltage,
                coefficient: -1,
                exponent: 0,
                unit: ValueUnit::Volt,
            }
        ));
    }

    #[test]
    fn exact_bytes_and_canonical_encoding_are_authenticated() {
        let mut stale = design();
        stale.product.catalog.as_mut().unwrap().sha256 = "0".repeat(64);
        assert_eq!(
            verify_product_catalog(&stale, b"not json").unwrap_err()[0].code,
            "CC-CATALOG-AUTH-002"
        );

        let mut noncanonical = SNAPSHOT.to_vec();
        noncanonical.insert(0, b' ');
        let mut rebound = design();
        rebound.product.catalog.as_mut().unwrap().sha256 = sha256_hex(&noncanonical);
        assert_eq!(
            verify_product_catalog(&rebound, &noncanonical).unwrap_err()[0].code,
            "CC-CATALOG-CONTRACT-007"
        );
    }

    #[test]
    fn invalid_design_uses_the_declared_catalog_contract_family() {
        let mut invalid = design();
        invalid.name.clear();
        assert!(invalid.validate().is_err());
        assert_eq!(
            verify_product_catalog(&invalid, SNAPSHOT).unwrap_err()[0].code,
            "CC-CATALOG-CONTRACT-008"
        );
    }

    #[test]
    fn snapshot_mutations_fail_closed() {
        for (needle, replacement, expected) in [
            ("\"active\"", "\"obsolete\"", "CC-CATALOG-RESOLVE-003"),
            (
                "\"available_quantity\":1000",
                "\"available_quantity\":0",
                "CC-CATALOG-RESOLVE-005",
            ),
            (
                "\"lead_time_days\":14",
                "\"lead_time_days\":999",
                "CC-CATALOG-RESOLVE-006",
            ),
            (
                "\"coefficient\":1",
                "\"coefficient\":2",
                "CC-CATALOG-RESOLVE-002",
            ),
        ] {
            let bytes = String::from_utf8(SNAPSHOT.to_vec())
                .unwrap()
                .replacen(needle, replacement, 1)
                .into_bytes();
            let mut design = design();
            design.product.catalog.as_mut().unwrap().sha256 = sha256_hex(&bytes);
            let diagnostics =
                verify_product_catalog(&design, &bytes).expect_err("mutation rejected");
            assert!(
                diagnostics
                    .iter()
                    .any(|diagnostic| diagnostic.code == expected),
                "missing {expected}: {diagnostics:#?}"
            );
        }
    }

    #[test]
    fn strict_structure_dates_provenance_and_identity_fail_closed() {
        let snapshot_json = std::str::from_utf8(SNAPSHOT).unwrap();
        for (bytes, expected) in [
            (
                snapshot_json
                    .replacen("\"schema_version\":1", "\"schema_version\":1,\"extra\":0", 1)
                    .into_bytes(),
                "CC-CATALOG-CONTRACT-001",
            ),
            (
                snapshot_json
                    .replacen(
                        "\"schema_name\":\"circuitc.product_catalog_snapshot\",\"schema_version\":1",
                        "\"schema_version\":1,\"schema_name\":\"circuitc.product_catalog_snapshot\"",
                        1,
                    )
                    .into_bytes(),
                "CC-CATALOG-CONTRACT-007",
            ),
            (
                snapshot_json
                    .replacen(
                        "https://example.com/catalog/2026-08-04.json",
                        "https://example.com/catalog/%7e.json",
                        1,
                    )
                    .into_bytes(),
                "CC-CATALOG-CONTRACT-003",
            ),
            (
                snapshot_json
                    .replacen(
                        "https://example.com/catalog/2026-08-04.json",
                        "https://example.com/\\u0001catalog",
                        1,
                    )
                    .into_bytes(),
                "CC-CATALOG-CONTRACT-003",
            ),
            (
                snapshot_json
                    .replacen(
                        "https://example.com/catalog/2026-08-04.json",
                        "https://example.com/<catalog>",
                        1,
                    )
                    .into_bytes(),
                "CC-CATALOG-CONTRACT-003",
            ),
            (
                snapshot_json
                    .replacen(
                        "https://example.com/catalog/2026-08-04.json",
                        "https://example.com/[catalog]",
                        1,
                    )
                    .into_bytes(),
                "CC-CATALOG-CONTRACT-003",
            ),
            (
                snapshot_json
                    .replacen(
                        "https://example.com/catalog/2026-08-04.json",
                        "https://example.com/catalog/../source",
                        1,
                    )
                    .into_bytes(),
                "CC-CATALOG-CONTRACT-003",
            ),
            (
                snapshot_json
                    .replacen(
                        "https://example.com/catalog/2026-08-04.json",
                        "https://user@example.com/catalog",
                        1,
                    )
                    .into_bytes(),
                "CC-CATALOG-CONTRACT-003",
            ),
            (
                snapshot_json
                    .replacen(
                        "https://example.com/catalog/2026-08-04.json",
                        "https://example.com:0080/catalog",
                        1,
                    )
                    .into_bytes(),
                "CC-CATALOG-CONTRACT-003",
            ),
            (
                snapshot_json
                    .replacen("\"observed_on\":\"2026-08-04\"", "\"observed_on\":\"2026-02-30\"", 1)
                    .into_bytes(),
                "CC-CATALOG-CONTRACT-003",
            ),
            (
                snapshot_json
                    .replacen("ERJ-3EKF1002V", "ERJ-3EKF9999V", 1)
                    .into_bytes(),
                "CC-CATALOG-RESOLVE-001",
            ),
            (
                snapshot_json
                    .replacen("\"global\"", "\"eu\"", 1)
                    .into_bytes(),
                "CC-CATALOG-RESOLVE-004",
            ),
        ] {
            let diagnostics = verify_rebound(&bytes).expect_err("invalid snapshot rejected");
            assert!(
                diagnostics
                    .iter()
                    .any(|diagnostic| diagnostic.code == expected),
                "missing {expected}: {diagnostics:#?}"
            );
        }

        let mut canonical_uri = snapshot();
        canonical_uri.source_uri = "https://example.com/catalog/@scope?part=%2F".to_owned();
        assert!(verify_rebound(&render_snapshot(&canonical_uri)).is_ok());
    }

    #[test]
    fn snapshot_and_region_order_and_uniqueness_are_authenticated() {
        let mut reordered = snapshot();
        reordered.parts.swap(0, 1);
        assert_eq!(
            verify_rebound(&render_snapshot(&reordered)).unwrap_err()[0].code,
            "CC-CATALOG-CONTRACT-004"
        );

        let mut duplicate_part = snapshot();
        duplicate_part.parts.push(duplicate_part.parts[0].clone());
        duplicate_part.parts.sort();
        assert_eq!(
            verify_rebound(&render_snapshot(&duplicate_part)).unwrap_err()[0].code,
            "CC-CATALOG-CONTRACT-004"
        );

        let mut duplicate_region = snapshot();
        let duplicate = duplicate_region.parts[0].availability[0].clone();
        duplicate_region.parts[0].availability.push(duplicate);
        assert_eq!(
            verify_rebound(&render_snapshot(&duplicate_region)).unwrap_err()[0].code,
            "CC-CATALOG-CONTRACT-005"
        );

        let mut reordered_regions = snapshot();
        let mut second = reordered_regions.parts[0].availability[0].clone();
        second.region = "z-region".to_owned();
        reordered_regions.parts[0].availability.push(second);
        reordered_regions.parts[0].availability.swap(0, 1);
        assert_eq!(
            verify_rebound(&render_snapshot(&reordered_regions)).unwrap_err()[0].code,
            "CC-CATALOG-CONTRACT-005"
        );
    }

    #[test]
    fn snapshot_collection_and_byte_bounds_reject_one_over() {
        let mut exact_parts = snapshot();
        let template = exact_parts.parts[0].clone();
        for index in 0..MAX_ENTRIES - 2 {
            let mut extra = template.clone();
            extra.manufacturer = "Generated".to_owned();
            extra.manufacturer_part_number = format!("GENERATED-{index:05}");
            extra.availability.clear();
            exact_parts.parts.push(extra);
        }
        exact_parts.parts.sort();
        assert_eq!(exact_parts.parts.len(), MAX_ENTRIES);
        assert!(verify_rebound(&render_snapshot(&exact_parts)).is_ok());
        let mut extra = template.clone();
        extra.manufacturer = "Generated".to_owned();
        extra.manufacturer_part_number = "GENERATED-OVER".to_owned();
        extra.availability.clear();
        exact_parts.parts.push(extra);
        exact_parts.parts.sort();
        assert_eq!(
            verify_rebound(&render_snapshot(&exact_parts)).unwrap_err()[0].code,
            "CC-CATALOG-CONTRACT-006"
        );

        let mut exact_availability = snapshot();
        let observation = exact_availability.parts[0].availability[0].clone();
        for index in 0..MAX_ENTRIES - 2 {
            let mut extra = observation.clone();
            extra.region = format!("region-{index:05}");
            exact_availability.parts[0].availability.push(extra);
        }
        exact_availability.parts[0].availability.sort();
        assert_eq!(
            exact_availability
                .parts
                .iter()
                .map(|part| part.availability.len())
                .sum::<usize>(),
            MAX_ENTRIES
        );
        assert!(verify_rebound(&render_snapshot(&exact_availability)).is_ok());
        let mut extra = observation;
        extra.region = "region-over".to_owned();
        exact_availability.parts[0].availability.push(extra);
        exact_availability.parts[0].availability.sort();
        assert_eq!(
            verify_rebound(&render_snapshot(&exact_availability)).unwrap_err()[0].code,
            "CC-CATALOG-CONTRACT-006"
        );

        drop(exact_parts);
        drop(exact_availability);

        let mut exact_bytes = snapshot();
        exact_bytes.source_uri = "https://example.com/catalog?pad=".to_owned();
        let unpadded_length = render_snapshot(&exact_bytes).len();
        exact_bytes
            .source_uri
            .push_str(&"a".repeat(MAX_BYTES - unpadded_length));
        let mut exact_bytes = render_snapshot(&exact_bytes);
        assert_eq!(exact_bytes.len(), MAX_BYTES);
        verify_rebound(&exact_bytes).expect("the exact byte ceiling is accepted");
        exact_bytes.push(b' ');
        assert_eq!(
            verify_product_catalog(&design(), &exact_bytes).unwrap_err()[0].code,
            "CC-CATALOG-CONTRACT-006"
        );
    }

    #[test]
    fn resolution_work_is_bounded_before_lookup_and_result_allocation() {
        let component = design().components[0].clone();
        let mut maximum = component.clone();
        maximum.part.approved_substitutions = (0..256).map(missing_substitution).collect();
        let mut remainder = component.clone();
        remainder.part.approved_substitutions = (0..233).map(missing_substitution).collect();
        let mut exact: Vec<_> = std::iter::repeat_n(&maximum, 38).collect();
        exact.push(&remainder);
        assert_eq!(resolution_entry_count(&exact), Some(MAX_RESOLUTION_ENTRIES));
        remainder
            .part
            .approved_substitutions
            .push(missing_substitution(233));
        let mut one_over: Vec<_> = std::iter::repeat_n(&maximum, 38).collect();
        one_over.push(&remainder);
        assert_eq!(
            resolution_entry_count(&one_over),
            Some(MAX_RESOLUTION_ENTRIES + 1)
        );

        let mut oversized = design();
        let template = oversized.components[0].clone();
        oversized.components = (0..39)
            .map(|index| {
                let mut component = template.clone();
                component.path = format!("divider.bulk_{index:02}");
                component.reference = format!("RB{index:02}");
                component.schematic_placement.position.x = 40_000_000 + index * 5_000_000;
                component.part.approved_substitutions =
                    (0..256).map(missing_substitution).collect();
                component
            })
            .collect();
        oversized.product.variants.truncate(1);
        oversized.product.variants[0].components = oversized
            .components
            .iter()
            .map(|component| VariantComponent {
                component_path: component.path.clone(),
                state: PopulationState::Fitted,
            })
            .collect();
        oversized.canonicalize();
        assert_eq!(oversized.validate(), Ok(()));
        assert_eq!(
            verify_product_catalog(&oversized, SNAPSHOT).unwrap_err()[0].code,
            "CC-CATALOG-RESOLVE-007"
        );

        let mut exact_design = design();
        let substitutions: Vec<_> = (0..256).map(missing_substitution).collect();
        let template = exact_design.components[0].clone();
        exact_design.components = (0..39)
            .map(|index| {
                let mut component = template.clone();
                component.path = format!("divider.exact_{index:02}");
                component.reference = format!("RE{index:02}");
                component.schematic_placement.position.x = 40_000_000 + (index as i64) * 5_000_000;
                component.part.approved_substitutions = if index == 38 {
                    substitutions[..233].to_vec()
                } else {
                    substitutions.clone()
                };
                component
            })
            .collect();
        exact_design.product.variants.truncate(1);
        exact_design.product.variants[0].components = exact_design
            .components
            .iter()
            .map(|component| VariantComponent {
                component_path: component.path.clone(),
                state: PopulationState::Fitted,
            })
            .collect();
        exact_design.canonicalize();
        assert_eq!(exact_design.validate(), Ok(()));

        let mut exact_catalog = snapshot();
        let template = exact_catalog.parts[0].clone();
        for substitution in substitutions {
            let mut record = template.clone();
            record.manufacturer = substitution.manufacturer;
            record.manufacturer_part_number = substitution.manufacturer_part_number;
            record.package = substitution.package;
            exact_catalog.parts.push(record);
        }
        exact_catalog.parts.sort();
        let exact_catalog = render_snapshot(&exact_catalog);
        exact_design.product.catalog.as_mut().unwrap().sha256 = sha256_hex(&exact_catalog);
        let resolution = verify_product_catalog(&exact_design, &exact_catalog)
            .expect("the exact resolution-entry ceiling is accepted");
        assert_eq!(resolution.parts.len(), MAX_RESOLUTION_ENTRIES);
    }

    #[test]
    fn diagnostics_are_canonical_and_capped_independent_of_substitution_order() {
        let forward = verify_product_catalog(&design_with_missing_alternates(256, false), SNAPSHOT)
            .unwrap_err();
        let reverse = verify_product_catalog(&design_with_missing_alternates(256, true), SNAPSHOT)
            .unwrap_err();
        assert_eq!(forward.len(), MAX_DIAGNOSTICS);
        assert_eq!(forward, reverse);
    }

    #[test]
    fn lifecycle_compatibility_is_exact_not_an_ordering() {
        for (required, observed) in [
            (LifecycleStatus::Active, "active"),
            (LifecycleStatus::Active, "not_recommended_for_new_designs"),
            (LifecycleStatus::Active, "obsolete"),
            (LifecycleStatus::NotRecommendedForNewDesigns, "active"),
            (
                LifecycleStatus::NotRecommendedForNewDesigns,
                "not_recommended_for_new_designs",
            ),
            (LifecycleStatus::NotRecommendedForNewDesigns, "obsolete"),
            (LifecycleStatus::Obsolete, "active"),
            (LifecycleStatus::Obsolete, "not_recommended_for_new_designs"),
            (LifecycleStatus::Obsolete, "obsolete"),
        ] {
            let bytes = std::str::from_utf8(SNAPSHOT)
                .unwrap()
                .replace("\"active\"", &format!("\"{observed}\""))
                .into_bytes();
            let mut design = design();
            for component in &mut design.components {
                if component.part.manufacturer.is_some() {
                    component.part.lifecycle = Some(required);
                }
            }
            design.product.catalog.as_mut().unwrap().sha256 = sha256_hex(&bytes);
            let result = verify_product_catalog(&design, &bytes);
            assert_eq!(
                result.is_ok(),
                catalog_lifecycle(required)
                    == serde_json::from_str::<CatalogLifecycle>(&format!("\"{observed}\""))
                        .unwrap(),
                "required {required:?}, observed {observed}: {result:#?}"
            );
        }
    }

    #[test]
    fn sourcing_comparisons_accept_exact_inclusive_boundaries() {
        let bytes = std::str::from_utf8(SNAPSHOT)
            .unwrap()
            .replace("\"available_quantity\":1000", "\"available_quantity\":1")
            .replace("\"lead_time_days\":14", "\"lead_time_days\":365")
            .into_bytes();
        assert!(verify_rebound(&bytes).is_ok());
    }

    #[test]
    fn validity_interval_is_inclusive_and_never_reads_the_clock() {
        for date in ["2026-08-04", "2026-12-31"] {
            let mut design = design();
            design.product.catalog.as_mut().unwrap().evaluated_on = date.to_owned();
            assert!(verify_product_catalog(&design, SNAPSHOT).is_ok());
        }
        let mut outside = design();
        outside.product.catalog.as_mut().unwrap().evaluated_on = "2027-01-01".to_owned();
        assert_eq!(
            verify_product_catalog(&outside, SNAPSHOT).unwrap_err()[0].code,
            "CC-CATALOG-AUTH-004"
        );
        let mut before = design();
        before.product.catalog.as_mut().unwrap().evaluated_on = "2026-08-03".to_owned();
        assert_eq!(
            verify_product_catalog(&before, SNAPSHOT).unwrap_err()[0].code,
            "CC-CATALOG-AUTH-004"
        );

        let mut wrong_id = snapshot();
        wrong_id.snapshot_id = "other-snapshot".to_owned();
        assert_eq!(
            verify_rebound(&render_snapshot(&wrong_id)).unwrap_err()[0].code,
            "CC-CATALOG-AUTH-003"
        );
    }
}
