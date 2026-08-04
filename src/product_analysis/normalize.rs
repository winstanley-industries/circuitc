use std::collections::{BTreeMap, BTreeSet};
use std::fmt;

use serde::Deserialize as DeriveDeserialize;
use serde::de::{Deserialize, Deserializer, MapAccess, SeqAccess, Visitor};
use serde_json::Value;

use crate::KicadIdentity;

use super::contract::{
    ADAPTER, ADAPTER_MAJOR, ADAPTER_VERSION, BoardAnalysisDiagnostic, ExpectedSheet, MAX_FILE_BYTES,
};

const INCLUDED_SEVERITIES: [&str; 3] = ["error", "exclusion", "warning"];
const ERC_IGNORED: [&str; 4] = [
    "footprint_filter",
    "four_way_junction",
    "simulation_model_issue",
    "single_global_label",
];
const DRC_IGNORED: [&str; 5] = [
    "footprint_filters_mismatch",
    "footprint_type_mismatch",
    "missing_courtyard",
    "track_not_centered_on_via",
    "tuning_profile_track_geometries",
];
const LIBRARY_WARNING: &str =
    "The current configuration does not include the footprint library 'CircuitC'";

pub(crate) struct ValidatedReports {
    pub erc: Vec<u8>,
    pub drc: Vec<u8>,
    pub erc_clean: bool,
    pub drc_clean: bool,
    pub unconnected_clean: bool,
    pub schematic_parity_clean: bool,
}

struct NoDuplicateJson;

impl<'de> Deserialize<'de> for NoDuplicateJson {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        deserializer.deserialize_any(NoDuplicateJsonVisitor)
    }
}

struct NoDuplicateJsonVisitor;

impl<'de> Visitor<'de> for NoDuplicateJsonVisitor {
    type Value = NoDuplicateJson;

    fn expecting(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("JSON without duplicate object keys")
    }

    fn visit_map<A>(self, mut map: A) -> Result<Self::Value, A::Error>
    where
        A: MapAccess<'de>,
    {
        let mut keys = BTreeSet::new();
        while let Some(key) = map.next_key::<String>()? {
            if !keys.insert(key) {
                return Err(serde::de::Error::custom("duplicate JSON object key"));
            }
            map.next_value::<NoDuplicateJson>()?;
        }
        Ok(NoDuplicateJson)
    }

    fn visit_seq<A>(self, mut sequence: A) -> Result<Self::Value, A::Error>
    where
        A: SeqAccess<'de>,
    {
        while sequence.next_element::<NoDuplicateJson>()?.is_some() {}
        Ok(NoDuplicateJson)
    }

    fn visit_bool<E>(self, _: bool) -> Result<Self::Value, E> {
        Ok(NoDuplicateJson)
    }

    fn visit_i64<E>(self, _: i64) -> Result<Self::Value, E> {
        Ok(NoDuplicateJson)
    }

    fn visit_u64<E>(self, _: u64) -> Result<Self::Value, E> {
        Ok(NoDuplicateJson)
    }

    fn visit_f64<E>(self, _: f64) -> Result<Self::Value, E> {
        Ok(NoDuplicateJson)
    }

    fn visit_str<E>(self, _: &str) -> Result<Self::Value, E> {
        Ok(NoDuplicateJson)
    }

    fn visit_string<E>(self, _: String) -> Result<Self::Value, E> {
        Ok(NoDuplicateJson)
    }

    fn visit_none<E>(self) -> Result<Self::Value, E> {
        Ok(NoDuplicateJson)
    }

    fn visit_unit<E>(self) -> Result<Self::Value, E> {
        Ok(NoDuplicateJson)
    }

    fn visit_some<D>(self, deserializer: D) -> Result<Self::Value, D::Error>
    where
        D: Deserializer<'de>,
    {
        NoDuplicateJson::deserialize(deserializer)
    }
}

fn diagnostic(
    code: &'static str,
    path: impl Into<String>,
    message: impl Into<String>,
) -> BoardAnalysisDiagnostic {
    BoardAnalysisDiagnostic {
        code,
        path: path.into(),
        message: message.into(),
    }
}

fn json_value(
    path: &str,
    bytes: &[u8],
    require_sorted_pretty: bool,
) -> Result<Value, BoardAnalysisDiagnostic> {
    if bytes.len() > MAX_FILE_BYTES {
        return Err(diagnostic(
            "CC-BOARD-ANALYSIS-RESOURCE-001",
            path,
            "analysis input exceeds the 64 MiB byte limit",
        ));
    }
    let text = std::str::from_utf8(bytes).map_err(|_| {
        diagnostic(
            "CC-BOARD-ANALYSIS-CONTRACT-001",
            path,
            "analysis input is not UTF-8",
        )
    })?;
    if !text.ends_with('\n') || text.contains(['\r', '\0']) {
        return Err(diagnostic(
            "CC-BOARD-ANALYSIS-CONTRACT-001",
            path,
            "analysis JSON must use LF, contain no NUL, and end in one LF",
        ));
    }
    let mut duplicate_parser = serde_json::Deserializer::from_str(text);
    NoDuplicateJson::deserialize(&mut duplicate_parser)
        .and_then(|_| duplicate_parser.end())
        .map_err(|error| {
            diagnostic(
                "CC-BOARD-ANALYSIS-CONTRACT-001",
                path,
                format!("analysis JSON is invalid or contains duplicate keys: {error}"),
            )
        })?;
    let value: Value = serde_json::from_str(text).map_err(|error| {
        diagnostic(
            "CC-BOARD-ANALYSIS-CONTRACT-001",
            path,
            format!("analysis JSON is invalid: {error}"),
        )
    })?;
    if require_sorted_pretty {
        let mut rendered = serde_json::to_string_pretty(&value).map_err(|error| {
            diagnostic(
                "CC-BOARD-ANALYSIS-CONTRACT-001",
                path,
                format!("analysis JSON cannot be rendered canonically: {error}"),
            )
        })?;
        rendered.push('\n');
        if rendered.as_bytes() != bytes {
            return Err(diagnostic(
                "CC-BOARD-ANALYSIS-CONTRACT-001",
                path,
                "normalized host evidence is not canonical sorted pretty JSON",
            ));
        }
    }
    Ok(value)
}

#[derive(DeriveDeserialize)]
#[serde(deny_unknown_fields)]
struct IdentityMap {
    schema_version: u32,
    source: String,
    identities: Vec<IdentityEntry>,
}

#[derive(DeriveDeserialize)]
#[serde(deny_unknown_fields)]
struct IdentityEntry {
    uuid: String,
    semantic_path: String,
    location: Option<SourceLocation>,
}

#[derive(DeriveDeserialize)]
#[serde(deny_unknown_fields)]
struct SourceLocation {
    start: usize,
    end: usize,
    line: usize,
    column: usize,
}

pub(crate) fn validate_identity_map(
    bytes: &[u8],
    design_name: &str,
    expected_identities: &[KicadIdentity],
) -> Result<(), BoardAnalysisDiagnostic> {
    let value = json_value("kicad_identity_map", bytes, false)?;
    let parsed: IdentityMap = serde_json::from_value(value).map_err(|error| {
        diagnostic(
            "CC-BOARD-ANALYSIS-IDENTITY-001",
            "kicad_identity_map",
            format!("identity map has an unsupported shape: {error}"),
        )
    })?;
    if parsed.schema_version != 1 || parsed.source != format!("{design_name}.circuitc") {
        return Err(diagnostic(
            "CC-BOARD-ANALYSIS-IDENTITY-001",
            "kicad_identity_map",
            "identity map schema or logical source does not match the Design",
        ));
    }
    if parsed.identities.len() != expected_identities.len() {
        return Err(diagnostic(
            "CC-BOARD-ANALYSIS-IDENTITY-001",
            "kicad_identity_map.identities",
            "identity map cardinality does not match compiled KiCad identities",
        ));
    }
    for (observed, expected) in parsed.identities.iter().zip(expected_identities) {
        if observed.uuid != expected.uuid || observed.semantic_path != expected.semantic_path {
            return Err(diagnostic(
                "CC-BOARD-ANALYSIS-IDENTITY-001",
                "kicad_identity_map.identities",
                "identity map order or identity does not match compiled KiCad identities",
            ));
        }
        if let Some(location) = &observed.location
            && (location.end < location.start || location.line == 0 || location.column == 0)
        {
            return Err(diagnostic(
                "CC-BOARD-ANALYSIS-IDENTITY-001",
                observed.semantic_path.clone(),
                "identity-map source location is invalid",
            ));
        }
    }
    Ok(())
}

fn exact_string_array(value: &Value, key: &str, expected: &[&str]) -> bool {
    value
        .get(key)
        .and_then(Value::as_array)
        .is_some_and(|items| {
            items.len() == expected.len()
                && items
                    .iter()
                    .zip(expected)
                    .all(|(item, expected)| item.as_str() == Some(expected))
        })
}

fn validate_host(value: &Value) -> bool {
    value
        .get("host")
        .and_then(Value::as_object)
        .is_some_and(|host| {
            host.len() == 3
                && host.get("name").and_then(Value::as_str) == Some(ADAPTER)
                && host.get("major").and_then(Value::as_u64) == Some(u64::from(ADAPTER_MAJOR))
                && host.get("version").and_then(Value::as_str) == Some(ADAPTER_VERSION)
        })
}

fn validate_ignored_checks(value: &Value, expected: &[&str]) -> bool {
    value
        .get("ignored_checks")
        .and_then(Value::as_array)
        .is_some_and(|checks| {
            checks.len() == expected.len()
                && checks.iter().zip(expected).all(|(check, key)| {
                    check.as_object().is_some_and(|object| {
                        object.len() == 2
                            && object.get("key").and_then(Value::as_str) == Some(key)
                            && object
                                .get("description")
                                .and_then(Value::as_str)
                                .is_some_and(|description| !description.is_empty())
                    })
                })
        })
}

fn common_report_is_valid(
    value: &Value,
    kind: &str,
    source: &str,
    source_sha256: &str,
    ignored: &[&str],
) -> bool {
    value.get("schema_version").and_then(Value::as_u64) == Some(1)
        && value.get("report_kind").and_then(Value::as_str) == Some(kind)
        && value.get("source").and_then(Value::as_str) == Some(source)
        && value.get("source_sha256").and_then(Value::as_str) == Some(source_sha256)
        && value.get("coordinate_units").and_then(Value::as_str) == Some("mm")
        && validate_host(value)
        && exact_string_array(value, "included_severities", &INCLUDED_SEVERITIES)
        && validate_ignored_checks(value, ignored)
}

fn validate_circuitc_item(
    item: &Value,
    identity_paths: &BTreeMap<&str, &str>,
    source: &str,
) -> bool {
    let Some(object) = item.as_object() else {
        return false;
    };
    let Some(uuid) = object.get("uuid").and_then(Value::as_str) else {
        return false;
    };
    let Some(circuitc) = object.get("circuitc").and_then(Value::as_object) else {
        return false;
    };
    circuitc.get("source").and_then(Value::as_str) == Some(source)
        && circuitc.get("semantic_path").and_then(Value::as_str)
            == identity_paths.get(uuid).copied()
        && object
            .get("description")
            .and_then(Value::as_str)
            .is_some_and(|description| !description.is_empty())
}

fn compact_canonical_key(value: &Value) -> Option<String> {
    serde_json::to_string(value).ok()
}

fn is_strictly_canonical(values: &[Value]) -> bool {
    values.windows(2).all(|pair| {
        compact_canonical_key(&pair[0])
            .zip(compact_canonical_key(&pair[1]))
            .is_some_and(|(left, right)| left < right)
    })
}

fn validate_finding(finding: &Value, identity_paths: &BTreeMap<&str, &str>, source: &str) -> bool {
    let Some(object) = finding.as_object() else {
        return false;
    };
    object
        .get("description")
        .and_then(Value::as_str)
        .is_some_and(|description| !description.is_empty())
        && object
            .get("severity")
            .and_then(Value::as_str)
            .is_some_and(|severity| INCLUDED_SEVERITIES.contains(&severity))
        && object
            .get("type")
            .and_then(Value::as_str)
            .is_some_and(|kind| !kind.is_empty())
        && object
            .get("items")
            .and_then(Value::as_array)
            .is_some_and(|items| {
                is_strictly_canonical(items)
                    && items
                        .iter()
                        .all(|item| validate_circuitc_item(item, identity_paths, source))
            })
}

fn finding_item_count(findings: &[Value]) -> Option<usize> {
    findings.iter().try_fold(0_usize, |total, finding| {
        finding
            .get("items")
            .and_then(Value::as_array)
            .and_then(|items| total.checked_add(items.len()))
    })
}

fn sheet_inventory_is_exact(sheets: &[Value], expected: &[ExpectedSheet]) -> bool {
    is_strictly_canonical(sheets)
        && sheets.len() == expected.len()
        && sheets.iter().zip(expected).all(|(sheet, expected)| {
            sheet.as_object().is_some_and(|object| {
                object.len() == 3
                    && object.get("path").and_then(Value::as_str) == Some(expected.path.as_str())
                    && object.get("uuid_path").and_then(Value::as_str)
                        == Some(expected.uuid_path.as_str())
                    && object.get("violations").and_then(Value::as_array).is_some()
            })
        })
}

fn is_allowed_library_warning(
    violation: &Value,
    identity_paths: &BTreeMap<&str, &str>,
    source: &str,
) -> bool {
    violation.as_object().is_some_and(|object| {
        object.len() == 4
            && object.get("description").and_then(Value::as_str) == Some(LIBRARY_WARNING)
            && object.get("severity").and_then(Value::as_str) == Some("warning")
            && object.get("type").and_then(Value::as_str) == Some("lib_footprint_issues")
            && object
                .get("items")
                .and_then(Value::as_array)
                .is_some_and(|items| {
                    !items.is_empty()
                        && items
                            .iter()
                            .all(|item| validate_circuitc_item(item, identity_paths, source))
                })
    })
}

pub(crate) fn validate_reports(
    erc_bytes: &[u8],
    drc_bytes: &[u8],
    design_name: &str,
    schematic_sha256: &str,
    pcb_sha256: &str,
    identities: &[KicadIdentity],
    expected_sheets: &[ExpectedSheet],
) -> Result<ValidatedReports, BoardAnalysisDiagnostic> {
    let erc = json_value("erc", erc_bytes, true)?;
    let drc = json_value("drc", drc_bytes, true)?;
    let source_name = format!("{design_name}.circuitc");
    let identity_paths: BTreeMap<_, _> = identities
        .iter()
        .map(|identity| (identity.uuid.as_str(), identity.semantic_path.as_str()))
        .collect();

    if erc.as_object().is_none_or(|object| object.len() != 9)
        || !common_report_is_valid(
            &erc,
            "erc",
            &format!("{design_name}.kicad_sch"),
            schematic_sha256,
            &ERC_IGNORED,
        )
    {
        return Err(diagnostic(
            "CC-BOARD-ANALYSIS-ERC-001",
            "erc",
            "normalized ERC report identity or policy is invalid",
        ));
    }
    let sheets = erc.get("sheets").and_then(Value::as_array).ok_or_else(|| {
        diagnostic(
            "CC-BOARD-ANALYSIS-ERC-001",
            "erc.sheets",
            "normalized ERC report has no sheet inventory",
        )
    })?;
    if !sheet_inventory_is_exact(sheets, expected_sheets) {
        return Err(diagnostic(
            "CC-BOARD-ANALYSIS-ERC-001",
            "erc.sheets",
            "ERC sheet inventory does not exactly match the authenticated request",
        ));
    }
    let erc_violations: Vec<&Value> = sheets
        .iter()
        .flat_map(|sheet| {
            sheet
                .get("violations")
                .and_then(Value::as_array)
                .expect("sheet inventory validation established violations")
        })
        .collect();
    let erc_item_count = erc_violations.iter().try_fold(0_usize, |total, finding| {
        finding
            .get("items")
            .and_then(Value::as_array)
            .and_then(|items| total.checked_add(items.len()))
    });
    if erc_violations.len() > 256
        || erc_item_count.is_none_or(|count| count > 10_000)
        || sheets.iter().any(|sheet| {
            sheet
                .get("violations")
                .and_then(Value::as_array)
                .is_none_or(|violations| !is_strictly_canonical(violations))
        })
        || erc_violations
            .iter()
            .any(|finding| !validate_finding(finding, &identity_paths, &source_name))
    {
        return Err(diagnostic(
            "CC-BOARD-ANALYSIS-ERC-001",
            "erc.sheets.violations",
            "ERC findings are malformed, unauthenticated, or exceed resource limits",
        ));
    }
    let erc_clean = erc_violations.is_empty();

    if drc.as_object().is_none_or(|object| object.len() != 11)
        || !common_report_is_valid(
            &drc,
            "drc",
            &format!("{design_name}.kicad_pcb"),
            pcb_sha256,
            &DRC_IGNORED,
        )
    {
        return Err(diagnostic(
            "CC-BOARD-ANALYSIS-DRC-001",
            "drc",
            "normalized DRC report identity or policy is invalid",
        ));
    }
    let unconnected = drc
        .get("unconnected_items")
        .and_then(Value::as_array)
        .ok_or_else(|| {
            diagnostic(
                "CC-BOARD-ANALYSIS-UNCONNECTED-001",
                "drc.unconnected_items",
                "normalized DRC report has no unconnected-item inventory",
            )
        })?;
    let parity = drc
        .get("schematic_parity")
        .and_then(Value::as_array)
        .ok_or_else(|| {
            diagnostic(
                "CC-BOARD-ANALYSIS-PARITY-001",
                "drc.schematic_parity",
                "normalized DRC report has no schematic-parity inventory",
            )
        })?;
    for (key, code, findings) in [
        (
            "unconnected_items",
            "CC-BOARD-ANALYSIS-UNCONNECTED-001",
            unconnected,
        ),
        ("schematic_parity", "CC-BOARD-ANALYSIS-PARITY-001", parity),
    ] {
        if !is_strictly_canonical(findings)
            || findings
                .iter()
                .any(|finding| !validate_finding(finding, &identity_paths, &source_name))
        {
            return Err(diagnostic(
                code,
                format!("drc.{key}"),
                "findings are malformed or do not join to the authenticated identity map",
            ));
        }
    }
    let violations = drc
        .get("violations")
        .and_then(Value::as_array)
        .ok_or_else(|| {
            diagnostic(
                "CC-BOARD-ANALYSIS-DRC-001",
                "drc.violations",
                "normalized DRC report has no violation inventory",
            )
        })?;
    let diagnostic_count = violations
        .len()
        .checked_add(unconnected.len())
        .and_then(|count| count.checked_add(parity.len()));
    let item_count = finding_item_count(violations)
        .and_then(|count| finding_item_count(unconnected).and_then(|next| count.checked_add(next)))
        .and_then(|count| finding_item_count(parity).and_then(|next| count.checked_add(next)));
    if diagnostic_count.is_none_or(|count| count > 256)
        || item_count.is_none_or(|count| count > 10_000)
        || !is_strictly_canonical(violations)
        || violations
            .iter()
            .any(|finding| !validate_finding(finding, &identity_paths, &source_name))
    {
        return Err(diagnostic(
            "CC-BOARD-ANALYSIS-DRC-001",
            "drc.violations",
            "DRC findings are malformed, unauthenticated, or exceed resource limits",
        ));
    }
    let drc_clean = violations
        .iter()
        .all(|violation| is_allowed_library_warning(violation, &identity_paths, &source_name));
    Ok(ValidatedReports {
        erc: erc_bytes.to_vec(),
        drc: drc_bytes.to_vec(),
        erc_clean,
        drc_clean,
        unconnected_clean: unconnected.is_empty(),
        schematic_parity_clean: parity.is_empty(),
    })
}

pub(crate) fn analysis_policy() -> super::contract::AnalysisPolicy {
    super::contract::AnalysisPolicy {
        included_severities: INCLUDED_SEVERITIES
            .iter()
            .map(|value| (*value).to_owned())
            .collect(),
        erc_ignored_checks: ERC_IGNORED
            .iter()
            .map(|value| (*value).to_owned())
            .collect(),
        drc_ignored_checks: DRC_IGNORED
            .iter()
            .map(|value| (*value).to_owned())
            .collect(),
        drc_library_warning: LIBRARY_WARNING.to_owned(),
    }
}

#[cfg(test)]
mod tests {
    use serde_json::{Value, json};

    use super::*;

    fn sheet(path: &str, uuid_path: &str) -> Value {
        json!({"path": path, "uuid_path": uuid_path, "violations": []})
    }

    #[test]
    fn exact_sheet_inventory_rejects_missing_duplicate_reordered_and_substituted_rows() {
        let expected = vec![
            ExpectedSheet {
                path: "/".to_owned(),
                uuid_path: "/root".to_owned(),
            },
            ExpectedSheet {
                path: "/child".to_owned(),
                uuid_path: "/root/child".to_owned(),
            },
        ];
        assert!(sheet_inventory_is_exact(
            &[sheet("/", "/root"), sheet("/child", "/root/child")],
            &expected
        ));
        let mutants = [
            vec![sheet("/", "/root")],
            vec![sheet("/", "/root"), sheet("/", "/root")],
            vec![sheet("/child", "/root/child"), sheet("/", "/root")],
            vec![sheet("/", "/root"), sheet("/other", "/root/child")],
        ];
        for mutant in mutants {
            assert!(!sheet_inventory_is_exact(&mutant, &expected));
        }
    }

    #[test]
    fn canonical_sequences_reject_reordered_and_duplicate_entries() {
        let first = json!({"description": "a"});
        let second = json!({"description": "b"});
        assert!(is_strictly_canonical(&[first.clone(), second.clone()]));
        assert!(!is_strictly_canonical(&[second.clone(), first.clone()]));
        assert!(!is_strictly_canonical(&[first.clone(), first]));

        let ascii = json!({"description": "z"});
        let unicode = json!({"description": "é"});
        assert!(is_strictly_canonical(&[ascii.clone(), unicode.clone()]));
        assert!(!is_strictly_canonical(&[unicode, ascii]));
    }
}
