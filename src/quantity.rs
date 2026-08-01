use std::fmt;

/// Physical dimension carried by an exact decimal quantity.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Unit {
    Ohm,
    Volt,
    Ampere,
    Farad,
    Henry,
    Dimensionless,
}

impl Unit {
    pub const fn symbol(self) -> &'static str {
        match self {
            Self::Ohm => "Ω",
            Self::Volt => "V",
            Self::Ampere => "A",
            Self::Farad => "F",
            Self::Henry => "H",
            Self::Dimensionless => "",
        }
    }
}

/// An exact value represented as `coefficient * 10^exponent` SI units.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Quantity {
    pub coefficient: i64,
    pub exponent: i8,
    pub unit: Unit,
}

impl Quantity {
    pub const MIN_EXPONENT: i8 = -18;
    pub const MAX_EXPONENT: i8 = 18;

    pub fn new(coefficient: i64, exponent: i8, unit: Unit) -> Self {
        let raw = Self {
            coefficient,
            exponent,
            unit,
        };
        normalize_decimal(i128::from(coefficient), i32::from(exponent), unit).unwrap_or(raw)
    }

    pub fn canonicalized(self) -> Self {
        Self::new(self.coefficient, self.exponent, self.unit)
    }

    pub fn is_canonical(self) -> bool {
        self == self.canonicalized()
    }

    pub const fn exponent_is_valid(self) -> bool {
        self.exponent >= Self::MIN_EXPONENT && self.exponent <= Self::MAX_EXPONENT
    }

    /// Return an exact SPICE-compatible decimal literal without using floats.
    pub fn spice_literal(self) -> String {
        let (coefficient, exponent) = self.engineering_parts();
        if exponent == 0 {
            coefficient.to_string()
        } else {
            format!("{coefficient}e{exponent}")
        }
    }

    /// Return a compact engineering label suitable for KiCad's value field.
    pub fn engineering_label(self) -> String {
        let (coefficient, exponent) = self.engineering_parts();
        let prefix = match exponent {
            -18 => Some("a"),
            -15 => Some("f"),
            -12 => Some("p"),
            -9 => Some("n"),
            -6 => Some("µ"),
            -3 => Some("m"),
            0 => Some(""),
            3 => Some("k"),
            6 => Some("M"),
            9 => Some("G"),
            12 => Some("T"),
            15 => Some("P"),
            18 => Some("E"),
            _ => None,
        };

        match prefix {
            Some(prefix) => format!("{coefficient}{}{}", prefix, self.unit.symbol()),
            None => format!(
                "{}e{}{}",
                self.coefficient,
                self.exponent,
                self.unit.symbol()
            ),
        }
    }

    fn engineering_parts(self) -> (i128, i8) {
        if !self.exponent_is_valid() {
            return (i128::from(self.coefficient), self.exponent);
        }
        let exponent = self.exponent.div_euclid(3) * 3;
        let shift = u32::from((self.exponent - exponent) as u8);
        (i128::from(self.coefficient) * 10_i128.pow(shift), exponent)
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum NormalizationError {
    Exponent(i32),
    Coefficient,
}

pub(crate) fn normalize_decimal(
    mut coefficient: i128,
    mut exponent: i32,
    unit: Unit,
) -> Result<Quantity, NormalizationError> {
    if coefficient == 0 {
        return Ok(Quantity {
            coefficient: 0,
            exponent: 0,
            unit,
        });
    }
    while coefficient % 10 == 0 && exponent < i32::from(Quantity::MAX_EXPONENT) {
        coefficient /= 10;
        exponent = exponent
            .checked_add(1)
            .ok_or(NormalizationError::Coefficient)?;
    }
    if exponent < i32::from(Quantity::MIN_EXPONENT) {
        return Err(NormalizationError::Exponent(exponent));
    }
    let target_exponent = exponent.min(i32::from(Quantity::MAX_EXPONENT));
    let shift =
        u32::try_from(exponent - target_exponent).map_err(|_| NormalizationError::Coefficient)?;
    coefficient = coefficient
        .checked_mul(
            10_i128
                .checked_pow(shift)
                .ok_or(NormalizationError::Coefficient)?,
        )
        .ok_or(NormalizationError::Coefficient)?;
    let coefficient = i64::try_from(coefficient).map_err(|_| NormalizationError::Coefficient)?;
    let exponent = i8::try_from(target_exponent).map_err(|_| NormalizationError::Coefficient)?;
    Ok(Quantity {
        coefficient,
        exponent,
        unit,
    })
}

impl fmt::Display for Quantity {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.engineering_label())
    }
}

#[cfg(test)]
mod tests {
    use super::{Quantity, Unit};

    #[test]
    fn formats_exact_engineering_and_spice_values() {
        let resistance = Quantity::new(10, 3, Unit::Ohm);
        assert_eq!(resistance.engineering_label(), "10kΩ");
        assert_eq!(resistance.spice_literal(), "10e3");

        let voltage = Quantity::new(5, 0, Unit::Volt);
        assert_eq!(voltage.engineering_label(), "5V");
        assert_eq!(voltage.spice_literal(), "5");
    }

    #[test]
    fn validates_and_formats_quantity_boundaries_without_floating_point() {
        for exponent in [Quantity::MIN_EXPONENT, Quantity::MAX_EXPONENT] {
            let quantity = Quantity::new(i64::MIN, exponent, Unit::Volt);
            assert!(quantity.exponent_is_valid());
            assert_eq!(quantity.spice_literal(), format!("{}e{exponent}", i64::MIN));
        }
        assert!(!Quantity::new(1, -19, Unit::Volt).exponent_is_valid());
        assert_eq!(
            Quantity::new(1, 19, Unit::Volt),
            Quantity::new(10, 18, Unit::Volt)
        );
        assert!(!Quantity::new(i64::MAX, 19, Unit::Volt).exponent_is_valid());
    }

    #[test]
    fn canonicalizes_equivalent_engineering_representations() {
        let expected = Quantity::new(10, 3, Unit::Ohm);
        assert_eq!(Quantity::new(10_000, 0, Unit::Ohm), expected);
        assert_eq!(Quantity::new(1, 4, Unit::Ohm), expected);
        assert_eq!(
            Quantity::new(1, -1, Unit::Volt),
            Quantity::new(100, -3, Unit::Volt)
        );
        assert!(expected.is_canonical());
    }
}
