use crate::design::MAX_ABS_COORDINATE_NM;
use crate::quantity::{NormalizationError, Quantity, Unit, normalize_decimal};

use super::diagnostic::SourceDiagnostic;
use super::syntax::{QuantitySyntax, SourceFile, Span};

pub(crate) fn lower_length(
    source: &SourceFile,
    quantity: &QuantitySyntax,
    semantic_path: Option<&str>,
    diagnostics: &mut Vec<SourceDiagnostic>,
) -> Option<i64> {
    let unit_exponent_nm = match quantity.unit.value.as_str() {
        "nm" => 0,
        "um" => 3,
        "mm" => 6,
        other => {
            diagnostics.push(SourceDiagnostic::new(
                "CC-LANG-QUANTITY-002",
                source,
                quantity.unit.span,
                semantic_path.map(str::to_owned),
                format!("expected a length unit (nm, um, or mm); found `{other}`"),
            ));
            return None;
        }
    };
    let decimal = parse_decimal(source, quantity, semantic_path, diagnostics)?;
    let exponent = unit_exponent_nm - decimal.fractional_digits;
    let value = if exponent >= 0 {
        let Some(multiplier) = power_of_ten(exponent as u32) else {
            overflow(source, quantity.span, semantic_path, diagnostics);
            return None;
        };
        decimal.coefficient.checked_mul(multiplier)
    } else {
        let Some(divisor) = power_of_ten((-exponent) as u32) else {
            overflow(source, quantity.span, semantic_path, diagnostics);
            return None;
        };
        if decimal.coefficient % divisor != 0 {
            diagnostics.push(SourceDiagnostic::new(
                "CC-LANG-QUANTITY-003",
                source,
                quantity.span,
                semantic_path.map(str::to_owned),
                "length has precision finer than one integer nanometre",
            ));
            return None;
        }
        Some(decimal.coefficient / divisor)
    };
    let Some(value) = value.and_then(|value| i64::try_from(value).ok()) else {
        overflow(source, quantity.span, semantic_path, diagnostics);
        return None;
    };
    if value.unsigned_abs() > MAX_ABS_COORDINATE_NM as u64 {
        diagnostics.push(SourceDiagnostic::new(
            "CC-LANG-QUANTITY-004",
            source,
            quantity.span,
            semantic_path.map(str::to_owned),
            format!(
                "coordinate {value} nm exceeds the Design IR envelope of +/−{MAX_ABS_COORDINATE_NM} nm"
            ),
        ));
        return None;
    }
    Some(value)
}

pub(crate) fn lower_electrical(
    source: &SourceFile,
    quantity: &QuantitySyntax,
    expected: Unit,
    semantic_path: Option<&str>,
    diagnostics: &mut Vec<SourceDiagnostic>,
) -> Option<Quantity> {
    let (unit, prefix_exponent) = match quantity.unit.value.as_str() {
        "ohm" => (Unit::Ohm, 0_i32),
        "kohm" => (Unit::Ohm, 3),
        "V" => (Unit::Volt, 0),
        "Hz" => (Unit::Hertz, 0),
        "kHz" => (Unit::Hertz, 3),
        "s" => (Unit::Second, 0),
        "ms" => (Unit::Second, -3),
        "us" => (Unit::Second, -6),
        "deg" => (Unit::Degree, 0),
        "ratio" => (Unit::Dimensionless, 0),
        other => {
            diagnostics.push(SourceDiagnostic::new(
                "CC-LANG-QUANTITY-002",
                source,
                quantity.unit.span,
                semantic_path.map(str::to_owned),
                format!("unsupported exact quantity unit `{other}`"),
            ));
            return None;
        }
    };
    if unit != expected {
        diagnostics.push(SourceDiagnostic::new(
            "CC-LANG-QUANTITY-005",
            source,
            quantity.unit.span,
            semantic_path.map(str::to_owned),
            format!(
                "dimensionally incorrect quantity: expected {}, found {}",
                expected.symbol(),
                unit.symbol()
            ),
        ));
        return None;
    }
    let decimal = parse_decimal(source, quantity, semantic_path, diagnostics)?;
    let exponent = prefix_exponent - decimal.fractional_digits;
    match normalize_decimal(decimal.coefficient, exponent, unit) {
        Ok(quantity) => Some(quantity),
        Err(NormalizationError::Exponent(exponent)) => {
            diagnostics.push(SourceDiagnostic::new(
                "CC-LANG-QUANTITY-006",
                source,
                quantity.span,
                semantic_path.map(str::to_owned),
                format!("quantity exponent {exponent} is outside [-18, 18]"),
            ));
            None
        }
        Err(NormalizationError::Coefficient) => {
            overflow(source, quantity.span, semantic_path, diagnostics);
            None
        }
    }
}

pub(crate) fn lower_rotation(
    source: &SourceFile,
    value: &str,
    span: Span,
    semantic_path: Option<&str>,
    diagnostics: &mut Vec<SourceDiagnostic>,
) -> Option<i16> {
    if value.contains('.') {
        diagnostics.push(SourceDiagnostic::new(
            "CC-LANG-QUANTITY-007",
            source,
            span,
            semantic_path.map(str::to_owned),
            "rotation must be an exact integer number of degrees",
        ));
        return None;
    }
    let Some(rotation) = value.parse::<i16>().ok() else {
        overflow(source, span, semantic_path, diagnostics);
        return None;
    };
    if !matches!(rotation.rem_euclid(360), 0 | 90 | 180 | 270) {
        diagnostics.push(SourceDiagnostic::new(
            "CC-LANG-QUANTITY-007",
            source,
            span,
            semantic_path.map(str::to_owned),
            "placement rotation must be a multiple of 90 degrees",
        ));
        return None;
    }
    Some(rotation.rem_euclid(360))
}

struct ExactDecimal {
    coefficient: i128,
    fractional_digits: i32,
}

fn parse_decimal(
    source: &SourceFile,
    quantity: &QuantitySyntax,
    semantic_path: Option<&str>,
    diagnostics: &mut Vec<SourceDiagnostic>,
) -> Option<ExactDecimal> {
    let raw = quantity.number.value.as_str();
    let (negative, unsigned) = raw
        .strip_prefix('-')
        .map_or((false, raw), |unsigned| (true, unsigned));
    let (whole, fractional) = unsigned.split_once('.').unwrap_or((unsigned, ""));
    if whole.is_empty()
        || (raw.contains('.') && fractional.is_empty())
        || !whole.bytes().all(|byte| byte.is_ascii_digit())
        || !fractional.bytes().all(|byte| byte.is_ascii_digit())
    {
        diagnostics.push(SourceDiagnostic::new(
            "CC-LANG-QUANTITY-001",
            source,
            quantity.number.span,
            semantic_path.map(str::to_owned),
            format!("invalid exact decimal literal `{raw}`"),
        ));
        return None;
    }
    let digits = format!("{whole}{fractional}");
    let significant = digits.trim_start_matches('0');
    if significant.is_empty() {
        return Some(ExactDecimal {
            coefficient: 0,
            fractional_digits: 0,
        });
    }
    let without_trailing_zeros = significant.trim_end_matches('0');
    let trailing_zeros = significant.len() - without_trailing_zeros.len();
    let Some(mut coefficient) = without_trailing_zeros.parse::<i128>().ok() else {
        overflow(source, quantity.number.span, semantic_path, diagnostics);
        return None;
    };
    if negative {
        coefficient = -coefficient;
    }
    let Some(fractional_digits) = i32::try_from(fractional.len()).ok() else {
        overflow(source, quantity.number.span, semantic_path, diagnostics);
        return None;
    };
    let Some(trailing_zeros) = i32::try_from(trailing_zeros).ok() else {
        overflow(source, quantity.number.span, semantic_path, diagnostics);
        return None;
    };
    Some(ExactDecimal {
        coefficient,
        fractional_digits: fractional_digits - trailing_zeros,
    })
}

fn power_of_ten(exponent: u32) -> Option<i128> {
    10_i128.checked_pow(exponent)
}

fn overflow(
    source: &SourceFile,
    span: Span,
    semantic_path: Option<&str>,
    diagnostics: &mut Vec<SourceDiagnostic>,
) {
    diagnostics.push(SourceDiagnostic::new(
        "CC-LANG-QUANTITY-004",
        source,
        span,
        semantic_path.map(str::to_owned),
        "exact quantity conversion overflows its canonical integer representation",
    ));
}

#[cfg(test)]
mod tests {
    use super::{lower_electrical, lower_length};
    use crate::frontend::syntax::{QuantitySyntax, SourceFile, Span, Spanned};
    use crate::quantity::{Quantity, Unit};

    fn syntax(number: &str, unit: &str) -> (SourceFile, QuantitySyntax) {
        let source = SourceFile::new("quantity.circuitc", format!("{number} {unit}"));
        let number_span = Span::new(0, number.len());
        let unit_span = Span::new(number.len() + 1, number.len() + 1 + unit.len());
        (
            source,
            QuantitySyntax {
                number: Spanned::new(number.to_owned(), number_span),
                unit: Spanned::new(unit.to_owned(), unit_span),
                span: number_span.through(unit_span),
            },
        )
    }

    #[test]
    fn lowers_reference_quantities_exactly() {
        let (source, length) = syntax("0.9", "mm");
        assert_eq!(
            lower_length(&source, &length, None, &mut Vec::new()),
            Some(900_000)
        );

        let (source, resistance) = syntax("10", "kohm");
        assert_eq!(
            lower_electrical(&source, &resistance, Unit::Ohm, None, &mut Vec::new()),
            Some(Quantity::new(10, 3, Unit::Ohm))
        );
    }

    #[test]
    fn lowers_simulation_intent_units_exactly() {
        for (number, suffix, unit, expected) in [
            ("10", "Hz", Unit::Hertz, Quantity::new(10, 0, Unit::Hertz)),
            ("2.5", "kHz", Unit::Hertz, Quantity::new(25, 2, Unit::Hertz)),
            ("3", "s", Unit::Second, Quantity::new(3, 0, Unit::Second)),
            (
                "1.5",
                "ms",
                Unit::Second,
                Quantity::new(15, -4, Unit::Second),
            ),
            (
                "25",
                "us",
                Unit::Second,
                Quantity::new(25, -6, Unit::Second),
            ),
            (
                "-90",
                "deg",
                Unit::Degree,
                Quantity::new(-90, 0, Unit::Degree),
            ),
            (
                "0.01",
                "ratio",
                Unit::Dimensionless,
                Quantity::new(1, -2, Unit::Dimensionless),
            ),
        ] {
            let (source, syntax) = syntax(number, suffix);
            assert_eq!(
                lower_electrical(&source, &syntax, unit, None, &mut Vec::new()),
                Some(expected),
                "failed to lower {number} {suffix}"
            );
        }
    }

    #[test]
    fn simulation_quantity_dimension_mismatch_has_a_stable_span_and_path() {
        let (source, wrong) = syntax("1", "ms");
        let mut diagnostics = Vec::new();
        assert_eq!(
            lower_electrical(
                &source,
                &wrong,
                Unit::Hertz,
                Some("design.analyses.ac.start_frequency"),
                &mut diagnostics,
            ),
            None
        );
        assert_eq!(diagnostics.len(), 1);
        assert_eq!(diagnostics[0].code, "CC-LANG-QUANTITY-005");
        assert_eq!(
            diagnostics[0].semantic_path.as_deref(),
            Some("design.analyses.ac.start_frequency")
        );
        assert_eq!((diagnostics[0].start, diagnostics[0].end), (2, 4));
    }

    #[test]
    fn rejects_sub_nanometre_precision() {
        let (source, quantity) = syntax("0.0000001", "mm");
        let mut diagnostics = Vec::new();
        assert_eq!(
            lower_length(&source, &quantity, None, &mut diagnostics),
            None
        );
        assert_eq!(diagnostics[0].code, "CC-LANG-QUANTITY-003");
    }

    #[test]
    fn rejects_dimension_mismatch_and_overflow() {
        let (source, wrong) = syntax("10", "V");
        let mut diagnostics = Vec::new();
        assert_eq!(
            lower_electrical(&source, &wrong, Unit::Ohm, None, &mut diagnostics),
            None
        );
        assert_eq!(diagnostics[0].code, "CC-LANG-QUANTITY-005");

        let (source, huge) = syntax("999999999999999999999999999999999999999999", "mm");
        let mut diagnostics = Vec::new();
        assert_eq!(lower_length(&source, &huge, None, &mut diagnostics), None);
        assert_eq!(diagnostics[0].code, "CC-LANG-QUANTITY-004");
    }

    #[test]
    fn rejects_electrical_exponents_outside_the_ir_contract() {
        let (source, tiny) = syntax("0.00000000000000000001", "V");
        let mut diagnostics = Vec::new();
        assert_eq!(
            lower_electrical(&source, &tiny, Unit::Volt, None, &mut diagnostics),
            None
        );
        assert_eq!(diagnostics[0].code, "CC-LANG-QUANTITY-006");
    }

    #[test]
    fn normalizes_representable_boundaries_and_equivalent_units() {
        let (source, tiny) = syntax("0.0000000000000000010", "V");
        assert_eq!(
            lower_electrical(&source, &tiny, Unit::Volt, None, &mut Vec::new()),
            Some(Quantity::new(1, -18, Unit::Volt))
        );

        let (source, huge) = syntax("100000000000000000000", "ohm");
        assert_eq!(
            lower_electrical(&source, &huge, Unit::Ohm, None, &mut Vec::new()),
            Some(Quantity::new(100, 18, Unit::Ohm))
        );

        let (source, base) = syntax("10000", "ohm");
        let (prefixed_source, prefixed) = syntax("10", "kohm");
        assert_eq!(
            lower_electrical(&source, &base, Unit::Ohm, None, &mut Vec::new()),
            lower_electrical(
                &prefixed_source,
                &prefixed,
                Unit::Ohm,
                None,
                &mut Vec::new()
            )
        );
    }

    #[test]
    fn strips_insignificant_zeros_before_bounded_integer_parsing() {
        let (source, voltage) = syntax("1.0000000000000000000000000000000000000000", "V");
        assert_eq!(
            lower_electrical(&source, &voltage, Unit::Volt, None, &mut Vec::new()),
            Some(Quantity::new(1, 0, Unit::Volt))
        );

        let (source, length) = syntax("1.0000000000000000000000000000000000000000", "mm");
        assert_eq!(
            lower_length(&source, &length, None, &mut Vec::new()),
            Some(1_000_000)
        );
    }
}
