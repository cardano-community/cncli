use dashu_int::IBig;
use pallas_math::math::FixedDecimal;
use std::str::FromStr;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) struct ExactRatio {
    pub(super) numerator: u64,
    pub(super) denominator: u64,
}

fn integer(text: &str) -> Result<IBig, String> {
    let digits = text
        .strip_prefix('-')
        .or_else(|| text.strip_prefix('+'))
        .unwrap_or(text);
    if digits.is_empty() || !digits.bytes().all(|c| c.is_ascii_digit()) {
        return Err("Invalid rational integer".to_string());
    }
    IBig::from_str(text.strip_prefix('+').unwrap_or(text)).map_err(|_| "Invalid rational integer".to_string())
}

impl ExactRatio {
    pub(super) fn from_parts(mut numerator: IBig, mut denominator: IBig) -> Result<Self, String> {
        if denominator == IBig::ZERO {
            return Err("Rational denominator must be nonzero".to_string());
        }
        if denominator < IBig::ZERO {
            numerator = -numerator;
            denominator = -denominator;
        }
        if numerator < IBig::ZERO || numerator > denominator {
            return Err("Rational must be between zero and one".to_string());
        }
        let mut a = numerator.clone();
        let mut b = denominator.clone();
        while b != IBig::ZERO {
            let remainder = &a % &b;
            a = b;
            b = remainder;
        }
        numerator /= &a;
        denominator /= &a;
        Ok(Self {
            numerator: u64::try_from(&numerator).map_err(|_| "Reduced numerator exceeds Word64".to_string())?,
            denominator: u64::try_from(&denominator).map_err(|_| "Reduced denominator exceeds Word64".to_string())?,
        })
    }

    pub(super) fn from_text(text: &str) -> Result<Self, String> {
        if let Some((numerator, denominator)) = text.split_once('/') {
            return Self::from_parts(integer(numerator)?, integer(denominator)?);
        }
        let (mantissa, exponent) = match text.find(['e', 'E']) {
            Some(index) => (&text[..index], integer(&text[index + 1..])?),
            None => (text, IBig::ZERO),
        };
        let negative = mantissa.starts_with('-');
        let mantissa = mantissa
            .strip_prefix('-')
            .or_else(|| mantissa.strip_prefix('+'))
            .unwrap_or(mantissa);
        let (whole, fraction) = mantissa.split_once('.').unwrap_or((mantissa, ""));
        if (whole.is_empty() && fraction.is_empty())
            || !whole.bytes().all(|c| c.is_ascii_digit())
            || !fraction.bytes().all(|c| c.is_ascii_digit())
        {
            return Err("Invalid decimal rational".to_string());
        }
        let mut digits = format!("{whole}{fraction}");
        let mut exponent = exponent - IBig::from(fraction.len());
        while digits.ends_with('0') {
            digits.pop();
            exponent += IBig::ONE;
        }
        if digits.is_empty() {
            return Self::from_parts(IBig::ZERO, IBig::ONE);
        }
        if exponent < IBig::from(-19) || exponent > IBig::from(19) {
            return Err("Normalized decimal exponent exceeds ledger bounds".to_string());
        }
        let exponent = i32::try_from(&exponent).map_err(|_| "Invalid decimal exponent".to_string())?;
        let mut numerator = integer(&digits)?;
        if negative {
            numerator = -numerator;
        }
        let power = IBig::from(10).pow(exponent.unsigned_abs() as usize);
        if exponent >= 0 {
            Self::from_parts(numerator * power, IBig::ONE)
        } else {
            Self::from_parts(numerator, power)
        }
    }

    #[expect(
        clippy::wrong_self_convention,
        reason = "The specified exact-ratio boundary API borrows its input"
    )]
    pub(super) fn to_fixed(&self) -> FixedDecimal {
        FixedDecimal::from(self.numerator) / FixedDecimal::from(self.denominator)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use pallas_math::math::{FixedPrecision, DEFAULT_PRECISION};

    #[test]
    fn exact_decimal_and_ratio_inputs() {
        for text in ["0.00001", "1e-5", "0.000010000"] {
            assert_eq!(
                ExactRatio::from_text(text).unwrap(),
                ExactRatio {
                    numerator: 1,
                    denominator: 100000
                }
            );
        }
        assert_eq!(
            ExactRatio::from_text("0.12345678901234567").unwrap(),
            ExactRatio {
                numerator: 12345678901234567,
                denominator: 100000000000000000
            }
        );
        assert_eq!(
            ExactRatio::from_text("1e-19").unwrap().denominator,
            10000000000000000000
        );
        assert_eq!(
            ExactRatio::from_text("10e-20").unwrap(),
            ExactRatio::from_text("1e-19").unwrap()
        );
        assert_eq!(
            ExactRatio::from_text("-2/-6").unwrap(),
            ExactRatio {
                numerator: 1,
                denominator: 3
            }
        );
        assert_eq!(
            ExactRatio::from_text("18446744073709551616/36893488147419103232").unwrap(),
            ExactRatio {
                numerator: 1,
                denominator: 2
            }
        );
        assert_eq!(
            ExactRatio::from_text("1/3").unwrap().to_fixed(),
            FixedDecimal::from_str("3333333333333333333333333333333333", DEFAULT_PRECISION).unwrap()
        );
    }

    #[test]
    fn invalid_ratios_are_rejected() {
        for text in [
            "",
            "NaN",
            "inf",
            "-0.1",
            "1.1",
            "1/0",
            "1e-20",
            "1/18446744073709551616",
            "1e20",
            "1e",
            "1/2/3",
            ".",
            "1..0",
            "1e--1",
        ] {
            assert!(ExactRatio::from_text(text).is_err(), "{text}");
        }
    }
}
