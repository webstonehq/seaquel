//! Exact `NUMERIC` values in Postgres's binary wire format.
//!
//! sqlx's own `PgNumeric` is private, and its `rust_decimal`/`bigdecimal`
//! conversions lose NaN, ±Infinity, values beyond 28 digits (rust_decimal)
//! or the display scale. This reads and writes the wire format directly and
//! converts to and from the text Postgres itself prints, so `12.50` stays
//! `12.50`.
//!
//! Wire format (`numeric_send` in `src/backend/utils/adt/numeric.c`):
//! `ndigits: u16, weight: i16, sign: u16, dscale: u16`, then `ndigits`
//! base-10000 digits as `i16`, most significant first. The value is
//! `Σ digits[i] · 10000^(weight − i)`, printed with `dscale` decimals.

use sqlx::encode::IsNull;
use sqlx::error::BoxDynError;
use sqlx::postgres::{
    PgArgumentBuffer, PgHasArrayType, PgTypeInfo, PgValueFormat, PgValueRef, Postgres,
};
use sqlx::{Decode, Encode, Type};

const SIGN_POS: u16 = 0x0000;
const SIGN_NEG: u16 = 0x4000;
const SIGN_NAN: u16 = 0xC000;
const SIGN_PINF: u16 = 0xD000;
const SIGN_NINF: u16 = 0xF000;
/// `NUMERIC_DSCALE_MAX`.
const DSCALE_MAX: usize = 0x3FFF;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Numeric {
    NaN,
    Infinity,
    NegInfinity,
    Number {
        negative: bool,
        weight: i16,
        dscale: u16,
        /// Base-10000 digits, no leading or trailing zero digits.
        digits: Vec<i16>,
    },
}

impl Numeric {
    /// Parse the text of a `Decimal` value: `[+-]digits[.digits][e[+-]n]`,
    /// `NaN`, or `[+-]Infinity`/`inf`, case-insensitively as Postgres does.
    pub fn parse(s: &str) -> Result<Numeric, String> {
        let t = s.trim();
        let (negative, body) = match t.as_bytes().first() {
            Some(b'-') => (true, &t[1..]),
            Some(b'+') => (false, &t[1..]),
            _ => (false, t),
        };
        if body.eq_ignore_ascii_case("nan") && body.len() == t.len() {
            return Ok(Numeric::NaN);
        }
        if body.eq_ignore_ascii_case("infinity") || body.eq_ignore_ascii_case("inf") {
            return Ok(if negative {
                Numeric::NegInfinity
            } else {
                Numeric::Infinity
            });
        }

        let invalid = || format!("invalid numeric \"{s}\"");
        let (mantissa, exponent) = match body.find(['e', 'E']) {
            Some(i) => {
                let exp: i64 = body[i + 1..].parse().map_err(|_| invalid())?;
                (&body[..i], exp)
            }
            None => (body, 0),
        };
        let (int_part, frac_part) = mantissa.split_once('.').unwrap_or((mantissa, ""));
        let all_digits = |p: &str| p.bytes().all(|b| b.is_ascii_digit());
        if (int_part.is_empty() && frac_part.is_empty())
            || !all_digits(int_part)
            || !all_digits(frac_part)
        {
            return Err(invalid());
        }
        // Checked before any zeros are appended for a positive exponent:
        // Postgres holds at most 131072 digits before the decimal point.
        if !(-1_000_000..=131_072).contains(&exponent) {
            return Err(format!("numeric \"{s}\" is out of range"));
        }

        // Every digit, and how many of them are after the decimal point once
        // the exponent is applied.
        let mut digits: Vec<u8> = int_part
            .bytes()
            .chain(frac_part.bytes())
            .map(|b| b - b'0')
            .collect();
        let mut scale = frac_part.len() as i64 - exponent;
        if scale < 0 {
            digits.extend(std::iter::repeat_n(0, scale.unsigned_abs() as usize));
            scale = 0;
        }
        let scale = scale as usize;
        if scale > DSCALE_MAX {
            return Err(format!("numeric \"{s}\" has too many decimal places"));
        }
        let int_len = digits.len().saturating_sub(scale);
        if digits.len() < scale {
            let pad = scale - digits.len();
            digits.splice(0..0, std::iter::repeat_n(0, pad));
        }

        // Align to base-10000 groups around the decimal point.
        let lead = (4 - int_len % 4) % 4;
        let trail = (4 - scale % 4) % 4;
        let mut aligned = vec![0u8; lead];
        aligned.extend_from_slice(&digits);
        aligned.extend(std::iter::repeat_n(0, trail));
        let mut groups: Vec<i16> = aligned
            .chunks(4)
            .map(|c| c.iter().fold(0i16, |acc, d| acc * 10 + i16::from(*d)))
            .collect();
        let mut weight = ((lead + int_len) / 4) as i64 - 1;

        let leading_zeros = groups.iter().take_while(|g| **g == 0).count();
        groups.drain(..leading_zeros);
        weight -= leading_zeros as i64;
        while groups.last() == Some(&0) {
            groups.pop();
        }
        if groups.is_empty() {
            weight = 0;
        }
        let weight =
            i16::try_from(weight).map_err(|_| format!("numeric \"{s}\" is out of range"))?;
        if groups.len() > i16::MAX as usize {
            return Err(format!("numeric \"{s}\" is out of range"));
        }
        Ok(Numeric::Number {
            negative: negative && !groups.is_empty(),
            weight,
            dscale: scale as u16,
            digits: groups,
        })
    }

    fn decode_binary(buf: &[u8]) -> Result<Numeric, BoxDynError> {
        let word = |i: usize| -> Result<[u8; 2], BoxDynError> {
            buf.get(i * 2..i * 2 + 2)
                .map(|b| [b[0], b[1]])
                .ok_or_else(|| "numeric value is truncated".into())
        };
        let ndigits = u16::from_be_bytes(word(0)?) as usize;
        let weight = i16::from_be_bytes(word(1)?);
        let sign = u16::from_be_bytes(word(2)?);
        let dscale = u16::from_be_bytes(word(3)?);
        match sign {
            SIGN_NAN => return Ok(Numeric::NaN),
            SIGN_PINF => return Ok(Numeric::Infinity),
            SIGN_NINF => return Ok(Numeric::NegInfinity),
            SIGN_POS | SIGN_NEG => {}
            _ => return Err(format!("invalid numeric sign {sign:#06X}").into()),
        }
        let digits = (0..ndigits)
            .map(|i| word(4 + i).map(i16::from_be_bytes))
            .collect::<Result<Vec<_>, _>>()?;
        if digits.iter().any(|d| !(0..10_000).contains(d)) {
            return Err("invalid numeric digit".into());
        }
        Ok(Numeric::Number {
            negative: sign == SIGN_NEG,
            weight,
            dscale,
            digits,
        })
    }

    fn encode_binary(&self, buf: &mut Vec<u8>) {
        let (ndigits, weight, sign, dscale, digits): (u16, i16, u16, u16, &[i16]) = match self {
            Numeric::NaN => (0, 0, SIGN_NAN, 0, &[]),
            Numeric::Infinity => (0, 0, SIGN_PINF, 0, &[]),
            Numeric::NegInfinity => (0, 0, SIGN_NINF, 0, &[]),
            Numeric::Number {
                negative,
                weight,
                dscale,
                digits,
            } => (
                digits.len() as u16,
                *weight,
                if *negative { SIGN_NEG } else { SIGN_POS },
                *dscale,
                digits,
            ),
        };
        buf.extend_from_slice(&ndigits.to_be_bytes());
        buf.extend_from_slice(&weight.to_be_bytes());
        buf.extend_from_slice(&sign.to_be_bytes());
        buf.extend_from_slice(&dscale.to_be_bytes());
        for d in digits {
            buf.extend_from_slice(&d.to_be_bytes());
        }
    }

    /// The text Postgres prints (`get_str_from_var`).
    pub fn to_text(&self) -> String {
        let (negative, weight, dscale, digits) = match self {
            Numeric::NaN => return "NaN".into(),
            Numeric::Infinity => return "Infinity".into(),
            Numeric::NegInfinity => return "-Infinity".into(),
            Numeric::Number {
                negative,
                weight,
                dscale,
                digits,
            } => (*negative, i64::from(*weight), usize::from(*dscale), digits),
        };
        let digit = |i: i64| -> i16 {
            usize::try_from(i)
                .ok()
                .and_then(|i| digits.get(i))
                .copied()
                .unwrap_or(0)
        };
        let mut out = String::new();
        if negative {
            out.push('-');
        }
        if weight < 0 {
            out.push('0');
        } else {
            out.push_str(&digit(0).to_string());
            for i in 1..=weight {
                out.push_str(&format!("{:04}", digit(i)));
            }
        }
        if dscale > 0 {
            out.push('.');
            let mut frac = String::with_capacity(dscale + 4);
            let mut i = weight + 1;
            while frac.len() < dscale {
                frac.push_str(&format!("{:04}", digit(i)));
                i += 1;
            }
            frac.truncate(dscale);
            out.push_str(&frac);
        }
        out
    }
}

impl Type<Postgres> for Numeric {
    fn type_info() -> PgTypeInfo {
        <rust_decimal::Decimal as Type<Postgres>>::type_info()
    }
}

impl PgHasArrayType for Numeric {
    fn array_type_info() -> PgTypeInfo {
        <rust_decimal::Decimal as PgHasArrayType>::array_type_info()
    }
}

impl Encode<'_, Postgres> for Numeric {
    fn encode_by_ref(&self, buf: &mut PgArgumentBuffer) -> Result<IsNull, BoxDynError> {
        self.encode_binary(buf);
        Ok(IsNull::No)
    }
}

impl Decode<'_, Postgres> for Numeric {
    fn decode(value: PgValueRef<'_>) -> Result<Self, BoxDynError> {
        match value.format() {
            PgValueFormat::Binary => Numeric::decode_binary(value.as_bytes()?),
            PgValueFormat::Text => Ok(Numeric::parse(value.as_str()?)?),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::Numeric;

    fn round_trip(text: &str) -> String {
        let n = Numeric::parse(text).unwrap();
        let mut buf = Vec::new();
        n.encode_binary(&mut buf);
        Numeric::decode_binary(&buf).unwrap().to_text()
    }

    #[test]
    fn prints_like_postgres() {
        for s in [
            "0",
            "0.000",
            "12.50",
            "-0.0012",
            "10000",
            "9999",
            "100000000.00000001",
            "123456789012345678901234567890.123",
            "0.00000000000000000001",
            "-1",
            "NaN",
            "Infinity",
            "-Infinity",
        ] {
            assert_eq!(round_trip(s), s);
        }
    }

    #[test]
    fn normalises_input_like_postgres() {
        for (input, printed) in [
            ("-1e3", "-1000"),
            ("1.2e3", "1200"),
            ("1e-3", "0.001"),
            ("1.50E+1", "15.0"),
            ("+5", "5"),
            ("-0.00", "0.00"),
            (".5", "0.5"),
            ("5.", "5"),
            ("007", "7"),
            ("nan", "NaN"),
            ("-inf", "-Infinity"),
            (" 1 ", "1"),
        ] {
            assert_eq!(round_trip(input), printed, "{input}");
        }
    }

    #[test]
    fn groups_digits_in_base_10000() {
        assert_eq!(
            Numeric::parse("12345.6789").unwrap(),
            Numeric::Number {
                negative: false,
                weight: 1,
                dscale: 4,
                digits: vec![1, 2345, 6789]
            }
        );
        assert_eq!(
            Numeric::parse("0.00001").unwrap(),
            Numeric::Number {
                negative: false,
                weight: -2,
                dscale: 5,
                digits: vec![1000]
            }
        );
    }

    #[test]
    fn rejects_garbage() {
        for s in [
            "",
            "-",
            ".",
            "abc",
            "1.2.3",
            "1e",
            "--1",
            "-nan",
            "1e99999999",
        ] {
            assert!(Numeric::parse(s).is_err(), "{s}");
        }
    }
}
