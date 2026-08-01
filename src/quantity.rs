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

    pub const fn new(coefficient: i64, exponent: i8, unit: Unit) -> Self {
        Self {
            coefficient,
            exponent,
            unit,
        }
    }

    pub const fn exponent_is_valid(self) -> bool {
        self.exponent >= Self::MIN_EXPONENT && self.exponent <= Self::MAX_EXPONENT
    }

    /// Return an exact SPICE-compatible decimal literal without using floats.
    pub fn spice_literal(self) -> String {
        if self.exponent == 0 {
            self.coefficient.to_string()
        } else {
            format!("{}e{}", self.coefficient, self.exponent)
        }
    }

    /// Return a compact engineering label suitable for KiCad's value field.
    pub fn engineering_label(self) -> String {
        let prefix = match self.exponent {
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
            Some(prefix) => format!("{}{}{}", self.coefficient, prefix, self.unit.symbol()),
            None => format!(
                "{}e{}{}",
                self.coefficient,
                self.exponent,
                self.unit.symbol()
            ),
        }
    }
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
        assert!(!Quantity::new(1, 19, Unit::Volt).exponent_is_valid());
    }
}
