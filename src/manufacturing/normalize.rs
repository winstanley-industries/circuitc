use std::collections::{BTreeMap, BTreeSet};
use std::fmt;

use serde::de::{Deserialize, Deserializer, MapAccess, SeqAccess, Visitor};
use serde_json::Value;

use super::contract::{
    FabricationDiagnostic, GerberLayerProfile, KICAD_VERSION, MAX_FILE_BYTES, MAX_POSITION_ROWS,
    PositionRow,
};

pub(crate) struct NormalizedNativeFile {
    pub contents: Vec<u8>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct ExpectedPosition {
    pub component_path: String,
    pub reference: String,
    pub host_value: String,
    pub host_package: String,
    pub x_nm: i64,
    pub y_nm: i64,
    pub rotation_degrees: i16,
    pub side: String,
    pub state: String,
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
) -> FabricationDiagnostic {
    FabricationDiagnostic {
        code,
        path: path.into(),
        message: message.into(),
    }
}

fn native_text<'a>(
    path: &str,
    contents: &'a [u8],
    code: &'static str,
) -> Result<&'a str, FabricationDiagnostic> {
    if contents.len() > MAX_FILE_BYTES {
        return Err(diagnostic(
            "CC-FABRICATION-RESOURCE-001",
            path,
            "native fabrication file exceeds the 64 MiB byte limit",
        ));
    }
    let text = std::str::from_utf8(contents)
        .map_err(|_| diagnostic(code, path, "native fabrication file is not valid UTF-8"))?;
    if !text.ends_with('\n') || text.contains('\r') || text.contains('\0') {
        return Err(diagnostic(
            code,
            path,
            "native fabrication text must use LF line endings, end in one LF, and contain no NUL",
        ));
    }
    Ok(text)
}

fn raw_iso_timestamp_is_valid(value: &str) -> bool {
    if value.len() != 25 {
        return false;
    }
    let bytes = value.as_bytes();
    for index in [
        0, 1, 2, 3, 5, 6, 8, 9, 11, 12, 14, 15, 17, 18, 20, 21, 23, 24,
    ] {
        if !bytes[index].is_ascii_digit() {
            return false;
        }
    }
    bytes[4] == b'-'
        && bytes[7] == b'-'
        && bytes[10] == b'T'
        && bytes[13] == b':'
        && bytes[16] == b':'
        && matches!(bytes[19], b'+' | b'-')
        && bytes[22] == b':'
        && raw_local_timestamp_is_valid(&value[..19], b'T')
        && decimal_pair(&bytes[20..22]).is_some_and(|hour| hour <= 23)
        && decimal_pair(&bytes[23..25]).is_some_and(|minute| minute <= 59)
}

fn raw_local_timestamp_is_valid(value: &str, separator: u8) -> bool {
    if value.len() != 19 {
        return false;
    }
    let bytes = value.as_bytes();
    for index in [0, 1, 2, 3, 5, 6, 8, 9, 11, 12, 14, 15, 17, 18] {
        if !bytes[index].is_ascii_digit() {
            return false;
        }
    }
    bytes[4] == b'-'
        && bytes[7] == b'-'
        && bytes[10] == separator
        && bytes[13] == b':'
        && bytes[16] == b':'
        && gregorian_date_is_valid(&value[..10])
        && decimal_pair(&bytes[11..13]).is_some_and(|hour| hour <= 23)
        && decimal_pair(&bytes[14..16]).is_some_and(|minute| minute <= 59)
        && decimal_pair(&bytes[17..19]).is_some_and(|second| second <= 59)
}

fn decimal_pair(bytes: &[u8]) -> Option<u32> {
    (bytes.len() == 2 && bytes.iter().all(u8::is_ascii_digit))
        .then(|| u32::from(bytes[0] - b'0') * 10 + u32::from(bytes[1] - b'0'))
}

fn gregorian_date_is_valid(value: &str) -> bool {
    let bytes = value.as_bytes();
    if bytes.len() != 10 || bytes[4] != b'-' || bytes[7] != b'-' {
        return false;
    }
    let Ok(year) = value[..4].parse::<u32>() else {
        return false;
    };
    let Some(month) = decimal_pair(&bytes[5..7]) else {
        return false;
    };
    let Some(day) = decimal_pair(&bytes[8..10]) else {
        return false;
    };
    let leap = year % 4 == 0 && (year % 100 != 0 || year % 400 == 0);
    let maximum = match month {
        1 | 3 | 5 | 7 | 8 | 10 | 12 => 31,
        4 | 6 | 9 | 11 => 30,
        2 if leap => 29,
        2 => 28,
        _ => return false,
    };
    (1..=maximum).contains(&day)
}

fn local_timestamp_values_match(left: &str, right: &str) -> bool {
    left.len() >= 19
        && right.len() >= 19
        && left[..10] == right[..10]
        && left[11..19] == right[11..19]
}

fn lowercase_uuid_is_valid(value: &str) -> bool {
    value.len() == 36
        && value.bytes().enumerate().all(|(index, byte)| match index {
            8 | 13 | 18 | 23 => byte == b'-',
            _ => byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte),
        })
}

fn count_json_key(value: &Value, expected_key: &str) -> usize {
    match value {
        Value::Array(items) => items
            .iter()
            .map(|item| count_json_key(item, expected_key))
            .sum(),
        Value::Object(object) => object
            .iter()
            .map(|(key, item)| {
                usize::from(key == expected_key) + count_json_key(item, expected_key)
            })
            .sum(),
        _ => 0,
    }
}

pub(crate) fn normalize_gerber(
    path: &str,
    contents: &[u8],
    design_name: &str,
    evaluated_on: &str,
    layer: &GerberLayerProfile,
) -> Result<NormalizedNativeFile, FabricationDiagnostic> {
    let text = native_text(path, contents, "CC-FABRICATION-GERBER-001")?;
    let generation = format!("%TF.GenerationSoftware,KiCad,Pcbnew,{KICAD_VERSION}*%");
    let project_prefix = format!("%TF.ProjectId,{design_name},");
    let function = format!("%TF.FileFunction,{}*%", layer.file_function);
    let polarity = format!("%TF.FilePolarity,{}*%", layer.file_polarity);
    let created_prefix = format!("G04 Created by KiCad (PCBNEW {KICAD_VERSION}) date ");
    let canonical_iso = format!("{evaluated_on}T00:00:00Z");
    let canonical_local = format!("{evaluated_on} 00:00:00");
    let mut lines = text.strip_suffix('\n').expect("checked suffix").split('\n');
    let invalid_envelope = || {
        diagnostic(
            "CC-FABRICATION-GERBER-001",
            path,
            "Gerber does not match the exact ordered KiCad 10.0.5 envelope",
        )
    };
    if lines.next() != Some(generation.as_str()) {
        return Err(invalid_envelope());
    }
    let creation_line = lines.next().ok_or_else(invalid_envelope)?;
    let creation = creation_line
        .strip_prefix("%TF.CreationDate,")
        .and_then(|value| value.strip_suffix("*%"))
        .ok_or_else(invalid_envelope)?;
    if !raw_iso_timestamp_is_valid(creation) {
        return Err(diagnostic(
            "CC-FABRICATION-GERBER-001",
            path,
            "Gerber CreationDate has an unsupported shape",
        ));
    }
    let project_line = lines.next().ok_or_else(invalid_envelope)?;
    let project_tail = project_line
        .strip_prefix(&project_prefix)
        .and_then(|value| value.strip_suffix("*%"))
        .ok_or_else(invalid_envelope)?;
    let Some((project_uuid, revision)) = project_tail.split_once(',') else {
        return Err(invalid_envelope());
    };
    if !lowercase_uuid_is_valid(project_uuid) || revision != "rev?" {
        return Err(invalid_envelope());
    }
    if lines.next() != Some("%TF.SameCoordinates,Original*%")
        || lines.next() != Some(function.as_str())
    {
        return Err(invalid_envelope());
    }
    if layer.layer_name != "Edge.Cuts" && lines.next() != Some(polarity.as_str()) {
        return Err(invalid_envelope());
    }
    if lines.next() != Some("%FSLAX46Y46*%")
        || lines.next() != Some("G04 Gerber Fmt 4.6, Leading zero omitted, Abs format (unit mm)*")
    {
        return Err(invalid_envelope());
    }
    let created_line = lines.next().ok_or_else(invalid_envelope)?;
    let created = created_line
        .strip_prefix(&created_prefix)
        .and_then(|value| value.strip_suffix('*'))
        .ok_or_else(invalid_envelope)?;
    if !raw_local_timestamp_is_valid(created, b' ')
        || !local_timestamp_values_match(creation, created)
    {
        return Err(diagnostic(
            "CC-FABRICATION-GERBER-001",
            path,
            "Gerber native creation timestamps are malformed or disagree",
        ));
    }
    if lines.next() != Some("%MOMM*%")
        || lines.next() != Some("%LPD*%")
        || lines.next() != Some("G01*")
    {
        return Err(invalid_envelope());
    }

    let mut payload = lines.peekable();
    let mut normalized = String::with_capacity(text.len());
    normalized.push_str(&generation);
    normalized.push('\n');
    normalized.push_str("%TF.CreationDate,");
    normalized.push_str(&canonical_iso);
    normalized.push_str("*%\n");
    normalized.push_str(project_line);
    normalized.push('\n');
    normalized.push_str("%TF.SameCoordinates,Original*%\n");
    normalized.push_str(&function);
    normalized.push('\n');
    if layer.layer_name != "Edge.Cuts" {
        normalized.push_str(&polarity);
        normalized.push('\n');
    }
    normalized.push_str("%FSLAX46Y46*%\n");
    normalized.push_str("G04 Gerber Fmt 4.6, Leading zero omitted, Abs format (unit mm)*\n");
    normalized.push_str(&created_prefix);
    normalized.push_str(&canonical_local);
    normalized.push_str("*\n%MOMM*%\n%LPD*%\nG01*\n");
    let controlled_fragments = [
        "%TF.GenerationSoftware,",
        "%TF.CreationDate,",
        "%TF.ProjectId,",
        "%TF.SameCoordinates,",
        "%TF.FileFunction,",
        "%TF.FilePolarity,",
        "%FS",
        "%MO",
        "G04 Created by KiCad (PCBNEW ",
        "M02*",
    ];
    let mut terminated = false;
    while let Some(line) = payload.next() {
        if payload.peek().is_none() {
            if line != "M02*" {
                return Err(invalid_envelope());
            }
            normalized.push_str("M02*\n");
            terminated = true;
            break;
        }
        if controlled_fragments
            .iter()
            .any(|fragment| line.contains(fragment))
        {
            return Err(diagnostic(
                "CC-FABRICATION-GERBER-001",
                path,
                "Gerber contains a relocated or additional controlled command",
            ));
        }
        normalized.push_str(line);
        normalized.push('\n');
    }
    if !terminated {
        return Err(invalid_envelope());
    }
    Ok(NormalizedNativeFile {
        contents: normalized.into_bytes(),
    })
}

pub(crate) fn normalize_gerber_job(
    path: &str,
    contents: &[u8],
    design_name: &str,
    evaluated_on: &str,
    layers: &[GerberLayerProfile],
) -> Result<NormalizedNativeFile, FabricationDiagnostic> {
    let text = native_text(path, contents, "CC-FABRICATION-GERBER-001")?;
    let mut duplicate_parser = serde_json::Deserializer::from_str(text);
    NoDuplicateJson::deserialize(&mut duplicate_parser)
        .and_then(|_| duplicate_parser.end())
        .map_err(|error| {
            diagnostic(
                "CC-FABRICATION-GERBER-001",
                path,
                format!("Gerber job contains invalid or duplicate JSON: {error}"),
            )
        })?;
    let value: Value = serde_json::from_str(text).map_err(|error| {
        diagnostic(
            "CC-FABRICATION-GERBER-001",
            path,
            format!("Gerber job is not valid JSON: {error}"),
        )
    })?;
    if count_json_key(&value, "CreationDate") != 1 {
        return Err(diagnostic(
            "CC-FABRICATION-GERBER-001",
            path,
            "Gerber job must contain exactly one CreationDate field at /Header/CreationDate",
        ));
    }
    let header = value
        .get("Header")
        .and_then(Value::as_object)
        .ok_or_else(|| {
            diagnostic(
                "CC-FABRICATION-GERBER-001",
                path,
                "Gerber job Header is missing",
            )
        })?;
    let software = header
        .get("GenerationSoftware")
        .and_then(Value::as_object)
        .ok_or_else(|| {
            diagnostic(
                "CC-FABRICATION-GERBER-001",
                path,
                "Gerber job generation software is missing",
            )
        })?;
    if software.get("Vendor").and_then(Value::as_str) != Some("KiCad")
        || software.get("Application").and_then(Value::as_str) != Some("Pcbnew")
        || software.get("Version").and_then(Value::as_str) != Some(KICAD_VERSION)
        || header
            .get("CreationDate")
            .and_then(Value::as_str)
            .is_none_or(|timestamp| !raw_iso_timestamp_is_valid(timestamp))
    {
        return Err(diagnostic(
            "CC-FABRICATION-GERBER-001",
            path,
            "Gerber job host identity or CreationDate is invalid",
        ));
    }
    if value
        .pointer("/GeneralSpecs/ProjectId/Name")
        .and_then(Value::as_str)
        != Some(design_name)
    {
        return Err(diagnostic(
            "CC-FABRICATION-GERBER-001",
            path,
            "Gerber job project identity does not match the Design",
        ));
    }

    let attributes = value
        .get("FilesAttributes")
        .and_then(Value::as_array)
        .ok_or_else(|| {
            diagnostic(
                "CC-FABRICATION-GERBER-001",
                path,
                "Gerber job file inventory is missing",
            )
        })?;
    if attributes.len() != layers.len() {
        return Err(diagnostic(
            "CC-FABRICATION-GERBER-001",
            path,
            "Gerber job file count does not match the fixed layer inventory",
        ));
    }
    let mut observed = BTreeMap::new();
    for attribute in attributes {
        let object = attribute.as_object().ok_or_else(|| {
            diagnostic(
                "CC-FABRICATION-GERBER-001",
                path,
                "Gerber job file entry is not an object",
            )
        })?;
        let native_path = object.get("Path").and_then(Value::as_str).ok_or_else(|| {
            diagnostic(
                "CC-FABRICATION-GERBER-001",
                path,
                "Gerber job file path is missing",
            )
        })?;
        let function = object
            .get("FileFunction")
            .and_then(Value::as_str)
            .ok_or_else(|| {
                diagnostic(
                    "CC-FABRICATION-GERBER-001",
                    path,
                    "Gerber job file function is missing",
                )
            })?;
        let polarity = object
            .get("FilePolarity")
            .and_then(Value::as_str)
            .ok_or_else(|| {
                diagnostic(
                    "CC-FABRICATION-GERBER-001",
                    path,
                    "Gerber job file polarity is missing",
                )
            })?;
        if native_path.contains('/') || native_path.contains('\\') || native_path == "." {
            return Err(diagnostic(
                "CC-FABRICATION-GERBER-001",
                path,
                "Gerber job contains a non-basename path",
            ));
        }
        if observed
            .insert(
                native_path.to_owned(),
                (function.to_owned(), polarity.to_owned()),
            )
            .is_some()
        {
            return Err(diagnostic(
                "CC-FABRICATION-GERBER-001",
                path,
                "Gerber job contains a duplicate path",
            ));
        }
    }
    let expected: BTreeMap<_, _> = layers
        .iter()
        .map(|layer| {
            let basename = layer.path.rsplit('/').next().expect("known output path");
            (
                basename.to_owned(),
                (layer.job_file_function.clone(), layer.file_polarity.clone()),
            )
        })
        .collect();
    if observed != expected {
        return Err(diagnostic(
            "CC-FABRICATION-GERBER-001",
            path,
            "Gerber job inventory does not match the Gerber files bidirectionally",
        ));
    }

    let canonical = format!("{evaluated_on}T00:00:00Z");
    let mut count = 0_usize;
    let mut normalized = String::with_capacity(text.len());
    for line in text.strip_suffix('\n').expect("checked suffix").split('\n') {
        let trimmed = line.trim_start();
        if let Some(tail) = trimmed.strip_prefix("\"CreationDate\": \"") {
            let (value, comma) = if let Some(value) = tail.strip_suffix("\",") {
                (value, true)
            } else if let Some(value) = tail.strip_suffix('\"') {
                (value, false)
            } else {
                return Err(diagnostic(
                    "CC-FABRICATION-GERBER-001",
                    path,
                    "Gerber job CreationDate line is malformed",
                ));
            };
            count += 1;
            if !raw_iso_timestamp_is_valid(value) {
                return Err(diagnostic(
                    "CC-FABRICATION-GERBER-001",
                    path,
                    "Gerber job CreationDate line has an unsupported shape",
                ));
            }
            let indent = &line[..line.len() - trimmed.len()];
            normalized.push_str(indent);
            normalized.push_str("\"CreationDate\": \"");
            normalized.push_str(&canonical);
            normalized.push('\"');
            if comma {
                normalized.push(',');
            }
            normalized.push('\n');
        } else {
            normalized.push_str(line);
            normalized.push('\n');
        }
    }
    if count != 1 {
        return Err(diagnostic(
            "CC-FABRICATION-GERBER-001",
            path,
            "Gerber job must contain exactly one CreationDate field",
        ));
    }
    let normalized_value = serde_json::from_str::<Value>(&normalized).map_err(|error| {
        diagnostic(
            "CC-FABRICATION-GERBER-001",
            path,
            format!("normalized Gerber job is invalid: {error}"),
        )
    })?;
    if count_json_key(&normalized_value, "CreationDate") != 1
        || normalized_value
            .pointer("/Header/CreationDate")
            .and_then(Value::as_str)
            != Some(canonical.as_str())
    {
        return Err(diagnostic(
            "CC-FABRICATION-GERBER-001",
            path,
            "normalized Gerber job did not retain the canonical CreationDate",
        ));
    }
    Ok(NormalizedNativeFile {
        contents: normalized.into_bytes(),
    })
}

pub(crate) fn normalize_excellon(
    path: &str,
    contents: &[u8],
    evaluated_on: &str,
    plated: bool,
) -> Result<NormalizedNativeFile, FabricationDiagnostic> {
    let text = native_text(path, contents, "CC-FABRICATION-DRILL-001")?;
    let expected_function = if plated {
        "; #@! TF.FileFunction,Plated,1,2,PTH"
    } else {
        "; #@! TF.FileFunction,NonPlated,1,2,NPTH"
    };
    let generation = format!("; #@! TF.GenerationSoftware,Kicad,Pcbnew,{KICAD_VERSION}");
    let mut lines = text.strip_suffix('\n').expect("checked suffix").split('\n');
    let invalid_envelope = || {
        diagnostic(
            "CC-FABRICATION-DRILL-001",
            path,
            "Excellon does not match the exact ordered KiCad 10.0.5 zero-hit form",
        )
    };
    if lines.next() != Some("M48") {
        return Err(invalid_envelope());
    }
    let native_prefix = format!("; DRILL file KiCad {KICAD_VERSION} date ");
    let native_timestamp = lines
        .next()
        .and_then(|line| line.strip_prefix(&native_prefix))
        .ok_or_else(invalid_envelope)?;
    if !raw_local_timestamp_is_valid(native_timestamp, b'T')
        || lines.next() != Some("; FORMAT={-:-/ absolute / metric / decimal}")
    {
        return Err(invalid_envelope());
    }
    let creation_timestamp = lines
        .next()
        .and_then(|line| line.strip_prefix("; #@! TF.CreationDate,"))
        .ok_or_else(invalid_envelope)?;
    if !raw_iso_timestamp_is_valid(creation_timestamp)
        || !local_timestamp_values_match(creation_timestamp, native_timestamp)
        || lines.next() != Some(generation.as_str())
        || lines.next() != Some(expected_function)
        || lines.next() != Some("FMAT,2")
        || lines.next() != Some("METRIC")
        || lines.next() != Some("%")
        || lines.next() != Some("G90")
        || lines.next() != Some("G05")
        || lines.next() != Some("M30")
        || lines.next().is_some()
    {
        return Err(invalid_envelope());
    }
    let mut normalized = String::with_capacity(text.len());
    normalized.push_str("M48\n");
    normalized.push_str(&native_prefix);
    normalized.push_str(evaluated_on);
    normalized.push_str("T00:00:00\n; FORMAT={-:-/ absolute / metric / decimal}\n");
    normalized.push_str("; #@! TF.CreationDate,");
    normalized.push_str(evaluated_on);
    normalized.push_str("T00:00:00Z\n");
    normalized.push_str(&generation);
    normalized.push('\n');
    normalized.push_str(expected_function);
    normalized.push_str("\nFMAT,2\nMETRIC\n%\nG90\nG05\nM30\n");
    Ok(NormalizedNativeFile {
        contents: normalized.into_bytes(),
    })
}

fn parse_csv_line(line: &str) -> Option<Vec<String>> {
    let bytes = line.as_bytes();
    let mut fields = Vec::new();
    let mut index = 0_usize;
    loop {
        let mut field = String::new();
        if index < bytes.len() && bytes[index] == b'"' {
            index += 1;
            loop {
                if index >= bytes.len() {
                    return None;
                }
                if bytes[index] == b'"' {
                    if index + 1 < bytes.len() && bytes[index + 1] == b'"' {
                        field.push('"');
                        index += 2;
                    } else {
                        index += 1;
                        break;
                    }
                } else {
                    let tail = std::str::from_utf8(&bytes[index..]).ok()?;
                    let ch = tail.chars().next()?;
                    field.push(ch);
                    index += ch.len_utf8();
                }
            }
            if index < bytes.len() && bytes[index] != b',' {
                return None;
            }
        } else {
            let start = index;
            while index < bytes.len() && bytes[index] != b',' {
                if bytes[index] == b'"' {
                    return None;
                }
                index += 1;
            }
            field.push_str(std::str::from_utf8(&bytes[start..index]).ok()?);
        }
        fields.push(field);
        if index == bytes.len() {
            break;
        }
        index += 1;
        if index == bytes.len() {
            return None;
        }
    }
    Some(fields)
}

fn decimal_mm_to_nm(value: &str) -> Option<i64> {
    let (negative, unsigned) = value
        .strip_prefix('-')
        .map_or((false, value), |tail| (true, tail));
    let (whole, fraction) = unsigned.split_once('.')?;
    if whole.is_empty()
        || fraction.len() != 6
        || !whole.bytes().all(|byte| byte.is_ascii_digit())
        || !fraction.bytes().all(|byte| byte.is_ascii_digit())
    {
        return None;
    }
    let whole: i128 = whole.parse().ok()?;
    let fraction: i128 = fraction.parse().ok()?;
    let magnitude = whole.checked_mul(1_000_000)?.checked_add(fraction)?;
    let signed = if negative {
        magnitude.checked_neg()?
    } else {
        magnitude
    };
    i64::try_from(signed).ok()
}

fn rotation(value: &str) -> Option<i16> {
    let nm = decimal_mm_to_nm(value)?;
    if nm % 1_000_000 != 0 {
        return None;
    }
    let degrees = i16::try_from(nm / 1_000_000).ok()?;
    matches!(degrees, 0 | 90 | 180 | 270).then_some(degrees)
}

pub(crate) fn parse_position_csv(
    path: &str,
    contents: &[u8],
    expected: &[ExpectedPosition],
) -> Result<Vec<PositionRow>, FabricationDiagnostic> {
    let text = native_text(path, contents, "CC-FABRICATION-POSITION-001")?;
    let mut lines = text.strip_suffix('\n').expect("checked suffix").split('\n');
    if lines.next() != Some("Ref,Val,Package,PosX,PosY,Rot,Side") {
        return Err(diagnostic(
            "CC-FABRICATION-POSITION-001",
            path,
            "position CSV header does not match KiCad 10",
        ));
    }
    let expected_by_reference: BTreeMap<_, _> = expected
        .iter()
        .map(|entry| (entry.reference.as_str(), entry))
        .collect();
    if expected_by_reference.len() != expected.len() {
        return Err(diagnostic(
            "CC-FABRICATION-POSITION-001",
            path,
            "authoritative physical references are not unique",
        ));
    }
    let mut observed = BTreeMap::new();
    for (index, line) in lines.enumerate() {
        if index >= MAX_POSITION_ROWS {
            return Err(diagnostic(
                "CC-FABRICATION-RESOURCE-001",
                path,
                "position CSV exceeds the 10,000-row limit",
            ));
        }
        let fields = parse_csv_line(line).ok_or_else(|| {
            diagnostic(
                "CC-FABRICATION-POSITION-001",
                format!("{path}.rows[{index}]"),
                "position CSV row is malformed",
            )
        })?;
        if fields.len() != 7 {
            return Err(diagnostic(
                "CC-FABRICATION-POSITION-001",
                format!("{path}.rows[{index}]"),
                "position CSV row must contain exactly seven fields",
            ));
        }
        let reference = &fields[0];
        let authoritative = expected_by_reference
            .get(reference.as_str())
            .ok_or_else(|| {
                diagnostic(
                    "CC-FABRICATION-POSITION-001",
                    format!("{path}.rows[{index}].reference"),
                    "position CSV contains an unknown physical reference",
                )
            })?;
        let x_nm = decimal_mm_to_nm(&fields[3]).ok_or_else(|| {
            diagnostic(
                "CC-FABRICATION-POSITION-001",
                format!("{path}.rows[{index}].x"),
                "position X is not an exact six-decimal millimetre value",
            )
        })?;
        let raw_y_nm = decimal_mm_to_nm(&fields[4]).ok_or_else(|| {
            diagnostic(
                "CC-FABRICATION-POSITION-001",
                format!("{path}.rows[{index}].y"),
                "position Y is not an exact six-decimal millimetre value",
            )
        })?;
        let y_nm = raw_y_nm.checked_neg().ok_or_else(|| {
            diagnostic(
                "CC-FABRICATION-POSITION-001",
                format!("{path}.rows[{index}].y"),
                "position Y cannot be converted to the Design coordinate convention",
            )
        })?;
        let rotation_degrees = rotation(&fields[5]).ok_or_else(|| {
            diagnostic(
                "CC-FABRICATION-POSITION-001",
                format!("{path}.rows[{index}].rotation"),
                "position rotation is not an exact supported orthogonal value",
            )
        })?;
        let side = match fields[6].as_str() {
            "top" => "front",
            "bottom" => "back",
            _ => {
                return Err(diagnostic(
                    "CC-FABRICATION-POSITION-001",
                    format!("{path}.rows[{index}].side"),
                    "position side is not top or bottom",
                ));
            }
        };
        if fields[1] != authoritative.host_value
            || fields[2] != authoritative.host_package
            || x_nm != authoritative.x_nm
            || y_nm != authoritative.y_nm
            || rotation_degrees != authoritative.rotation_degrees
            || side != authoritative.side
        {
            return Err(diagnostic(
                "CC-FABRICATION-POSITION-001",
                format!("{path}.rows[{index}]"),
                "position CSV row does not match the exact Design footprint lowering",
            ));
        }
        let row = PositionRow {
            component_path: authoritative.component_path.clone(),
            reference: reference.clone(),
            host_value: fields[1].clone(),
            host_package: fields[2].clone(),
            x_nm,
            y_nm,
            rotation_degrees,
            side: side.to_owned(),
            state: authoritative.state.clone(),
        };
        if observed.insert(reference.clone(), row).is_some() {
            return Err(diagnostic(
                "CC-FABRICATION-POSITION-001",
                format!("{path}.rows[{index}].reference"),
                "position CSV contains a duplicate reference",
            ));
        }
    }
    if observed.len() != expected.len() {
        return Err(diagnostic(
            "CC-FABRICATION-POSITION-001",
            path,
            "position CSV does not contain every physical Design component exactly once",
        ));
    }
    let mut rows: Vec<_> = observed.into_values().collect();
    rows.sort_by(|left, right| left.component_path.cmp(&right.component_path));
    Ok(rows)
}
