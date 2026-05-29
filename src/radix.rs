//! Positional-notation ("BaseX") numeric encoding.
//!
//! Numbers are written in a custom radix whose digits map onto a curated set
//! of code points (all in `0..=127`, so encoded numbers are pure ASCII). This
//! mapping is the heart of the Senbay wire format.

use crate::error::{Error, Result};

/// Maps a digit value (`0..TABLE.len()`) to the code point representing it.
const TABLE: [u8; 122] = [
    1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11, 12, 13, 14, 15, 16, 17, 18, 19, 20, 21, 22, 23, 24, 25, 26,
    27, 28, 29, 30, 31, 32, 33, 34, 35, 36, 37, 38, 40, 41, 42, 43, 47, 48, 49, 50, 51, 52, 53, 54,
    55, 56, 57, 59, 60, 61, 62, 63, 64, 65, 66, 67, 68, 69, 70, 71, 72, 73, 74, 75, 76, 77, 78, 79,
    80, 81, 82, 83, 84, 85, 86, 87, 88, 89, 90, 91, 92, 93, 94, 95, 96, 97, 98, 99, 100, 101, 102,
    103, 104, 105, 106, 107, 108, 109, 110, 111, 112, 113, 114, 115, 116, 117, 118, 119, 120, 121,
    122, 123, 124, 125, 126, 127,
];

/// Maps a code point back to its digit value (inverse of [`TABLE`]).
const REVERSE: [u8; 128] = [
    0, 0, 1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11, 12, 13, 14, 15, 16, 17, 18, 19, 20, 21, 22, 23, 24,
    25, 26, 27, 28, 29, 30, 31, 32, 33, 34, 35, 36, 37, 0, 38, 39, 40, 41, 0, 0, 0, 42, 43, 44, 45,
    46, 47, 48, 49, 50, 51, 52, 0, 53, 54, 55, 56, 57, 58, 59, 60, 61, 62, 63, 64, 65, 66, 67, 68,
    69, 70, 71, 72, 73, 74, 75, 76, 77, 78, 79, 80, 81, 82, 83, 84, 85, 86, 87, 88, 89, 90, 91, 92,
    93, 94, 95, 96, 97, 98, 99, 100, 101, 102, 103, 104, 105, 106, 107, 108, 109, 110, 111, 112,
    113, 114, 115, 116, 117, 118, 119, 120, 121,
];

/// A validated positional notation (radix) used to encode numbers.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Radix(u32);

impl Radix {
    /// The canonical radix used by Senbay.
    pub const DEFAULT: Radix = Radix(121);

    /// The largest radix the digit table can represent.
    pub const MAX: u32 = TABLE.len() as u32;

    /// Creates a radix, validating that it is within `2..=Radix::MAX`.
    pub fn new(value: u32) -> Result<Radix> {
        if (2..=Self::MAX).contains(&value) {
            Ok(Radix(value))
        } else {
            Err(Error::InvalidRadix {
                value,
                max: Self::MAX,
            })
        }
    }

    /// Returns the underlying radix value.
    pub fn get(self) -> u32 {
        self.0
    }

    /// Encodes a signed integer into its BaseX string.
    pub(crate) fn encode_int(self, value: i64) -> String {
        let radix = self.0 as u64;
        let negative = value < 0;
        let mut magnitude = value.unsigned_abs();

        // Least-significant digit first; zero is encoded as the NUL code point.
        let mut digits = Vec::new();
        if magnitude == 0 {
            digits.push(0u8);
        } else {
            while magnitude > 0 {
                digits.push(TABLE[(magnitude % radix) as usize]);
                magnitude /= radix;
            }
        }

        let mut out = String::with_capacity(digits.len() + usize::from(negative));
        if negative {
            out.push('-');
        }
        out.extend(digits.iter().rev().map(|&b| b as char));
        out
    }

    /// Decodes a BaseX string into a signed integer.
    pub(crate) fn decode_int(self, text: &str) -> i64 {
        let (negative, body) = split_sign(text);
        if body.is_empty() {
            return 0;
        }

        let radix = self.0 as f64;
        let codes: Vec<u32> = body.chars().map(|c| c as u32).collect();
        let len = codes.len();

        // Float accumulation matches the Senbay wire format exactly.
        let total: f64 = codes
            .iter()
            .enumerate()
            .filter_map(|(i, &code)| {
                REVERSE
                    .get(code as usize)
                    .map(|&digit| radix.powi((len - i - 1) as i32) * digit as f64)
            })
            .sum();

        let value = total as i64;
        if negative { -value } else { value }
    }

    /// Encodes a float into its BaseX string (integer and fractional parts
    /// encoded separately, joined by `'.'`).
    pub(crate) fn encode_float(self, value: f64) -> String {
        if !value.is_finite() {
            return self.encode_int(0);
        }

        let negative = value < 0.0;
        let formatted = format!("{}", value.abs());

        let body = match formatted.split_once('.') {
            None => self.encode_int(formatted.parse().unwrap_or(0)),
            Some((int_str, frac_str)) => {
                let int_part = self.encode_int(int_str.parse().unwrap_or(0));
                let frac_part = self.encode_int(frac_str.parse().unwrap_or(0));

                // Each leading zero of the fraction is preserved as a zero digit.
                let zero = self.encode_int(0);
                let leading: String = frac_str
                    .chars()
                    .filter(|&c| c == '0')
                    .flat_map(|_| zero.chars())
                    .collect();

                format!("{int_part}.{leading}{frac_part}")
            }
        };

        if negative {
            format!("-{body}")
        } else {
            body
        }
    }

    /// Decodes a BaseX string into a float.
    pub(crate) fn decode_float(self, text: &str) -> f64 {
        let (negative, body) = split_sign(text);
        if body.is_empty() {
            return 0.0;
        }

        let value = match body.split_once('.') {
            None => self.decode_int(body) as f64,
            Some((int_str, frac_str)) => {
                let int_val = self.decode_int(int_str);

                // Strip the leading zero digits the encoder inserted.
                let zero = self.encode_int(0);
                let leading = match zero.chars().next() {
                    Some(z) => frac_str.chars().take_while(|&c| c == z).count(),
                    None => 0,
                };
                let frac_rest: String = frac_str.chars().skip(leading).collect();
                let frac_val = self.decode_int(&frac_rest);

                let text = format!("{int_val}.{}{frac_val}", "0".repeat(leading));
                text.parse::<f64>().unwrap_or(0.0)
            }
        };

        if negative && value >= 0.0 {
            -value
        } else {
            value
        }
    }
}

impl Default for Radix {
    fn default() -> Self {
        Radix::DEFAULT
    }
}

/// Splits an optional leading `'-'` sign from a number string.
fn split_sign(text: &str) -> (bool, &str) {
    match text.strip_prefix('-') {
        Some(rest) => (true, rest),
        None => (false, text),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rejects_out_of_range() {
        assert!(Radix::new(1).is_err());
        assert!(Radix::new(Radix::MAX + 1).is_err());
        assert!(Radix::new(2).is_ok());
        assert!(Radix::new(Radix::MAX).is_ok());
    }

    #[test]
    fn integers_round_trip() {
        let radix = Radix::DEFAULT;
        for v in [0_i64, 1, -1, 42, -42, 12_345, -12_345, 1_700_000_000_000] {
            assert_eq!(radix.decode_int(&radix.encode_int(v)), v, "value {v}");
        }
    }

    #[test]
    fn floats_round_trip() {
        let radix = Radix::DEFAULT;
        for v in [0.0_f64, 1.5, -1.5, 35.6895, -139.6917, 0.05, 123.456] {
            let decoded = radix.decode_float(&radix.encode_float(v));
            assert!((decoded - v).abs() < 1e-9, "value {v} -> {decoded}");
        }
    }

    #[test]
    fn non_finite_floats_do_not_panic() {
        let radix = Radix::DEFAULT;
        assert_eq!(radix.encode_float(f64::NAN), radix.encode_int(0));
        assert_eq!(radix.encode_float(f64::INFINITY), radix.encode_int(0));
    }
}
