//! The [`Senbay`] codec: turns [`Record`]s into Senbay text and back.

use crate::error::Result;
use crate::radix::Radix;
use crate::record::{Encoding, Record};
use crate::value::Value;

/// Reserved keys mapped to their single-character short forms, used by the
/// compressed encoding. This is the single source of truth for both directions.
pub(crate) const RESERVED_KEYS: [(&str, &str); 15] = [
    ("TIME", "0"),
    ("LONG", "1"),
    ("LATI", "2"),
    ("ALTI", "3"),
    ("ACCX", "4"),
    ("ACCY", "5"),
    ("ACCZ", "6"),
    ("YAW", "7"),
    ("ROLL", "8"),
    ("PITC", "9"),
    ("HEAD", "A"),
    ("SPEE", "B"),
    ("BRIG", "C"),
    ("AIRP", "D"),
    ("HTBT", "E"),
];

fn short_key(original: &str) -> Option<&'static str> {
    RESERVED_KEYS
        .iter()
        .find(|(k, _)| *k == original)
        .map(|(_, v)| *v)
}

fn original_key(short: &str) -> Option<&'static str> {
    RESERVED_KEYS
        .iter()
        .find(|(_, v)| *v == short)
        .map(|(k, _)| *k)
}

/// A Senbay codec configured with a particular numeric [`Radix`].
#[derive(Debug, Clone, Copy, Default)]
pub struct Senbay {
    radix: Radix,
}

impl Senbay {
    /// Creates a codec using the default radix.
    pub fn new() -> Self {
        Senbay::default()
    }

    /// Creates a codec for a custom radix.
    pub fn with_radix(radix: u32) -> Result<Self> {
        Ok(Senbay {
            radix: Radix::new(radix)?,
        })
    }

    /// Returns the radix in use.
    pub fn radix(&self) -> Radix {
        self.radix
    }

    /// Encodes a record to Senbay text.
    pub fn encode(&self, record: &Record, encoding: Encoding) -> String {
        match encoding {
            Encoding::Plain => format!("V:3,{}", self.format_plain(record)),
            Encoding::Compressed => format!("V:4,{}", self.format_compressed(record)),
        }
    }

    /// Decodes Senbay text back into a record.
    ///
    /// Unknown or malformed fields are skipped. Numbers are returned as
    /// [`Value::Float`]; quoted fields as [`Value::Text`].
    pub fn decode(&self, text: &str) -> Record {
        let is_compressed = text
            .split(',')
            .any(|element| matches!(element.split_once(':'), Some(("V", "4"))));

        let expanded;
        let working = if is_compressed {
            expanded = self.expand(text);
            expanded.as_str()
        } else {
            text
        };

        let mut record = Record::new();
        for element in working.split(',') {
            let Some((key, value)) = element.split_once(':') else {
                continue;
            };
            if key.is_empty() || key == "V" || value.is_empty() || value == "None" {
                continue;
            }
            record.set(key, parse_value(value));
        }
        record
    }

    /// Plain form: `key:value` joined by commas, values written verbatim.
    fn format_plain(&self, record: &Record) -> String {
        join_commas(record.iter().map(|(key, value)| format!("{key}:{value}")))
    }

    /// Compressed form: reserved keys shortened, numbers BaseX-encoded.
    fn format_compressed(&self, record: &Record) -> String {
        join_commas(record.iter().map(|(key, value)| {
            let short = short_key(key);
            let out_key = short.unwrap_or(key);
            let encoded = match value {
                Value::Int(v) => self.radix.encode_int(*v),
                Value::Float(v) => self.radix.encode_float(*v),
                // Text keeps its quotes (Value::Display adds them).
                Value::Text(_) => value.to_string(),
            };
            // Reserved keys are glued directly to the value; others keep the colon.
            if short.is_some() {
                format!("{out_key}{encoded}")
            } else {
                format!("{out_key}:{encoded}")
            }
        }))
    }

    /// Expands compressed text into the plain `key:value` form, decoding
    /// BaseX numbers and restoring reserved keys to their long names.
    fn expand(&self, text: &str) -> String {
        let elements = text.split(',').filter(|e| !e.is_empty()).filter_map(|element| {
            let (key, value) = match element.split_once(':') {
                Some((k, v)) => (k.to_owned(), v.to_owned()),
                None => {
                    // A glued reserved field: first char is the key, rest the value.
                    let mut chars = element.chars();
                    let key = chars.next()?.to_string();
                    (key, chars.as_str().to_owned())
                }
            };

            if key.is_empty() || value.is_empty() {
                return None;
            }
            if key == "V" {
                return Some(format!("V:{value}"));
            }

            let key = original_key(&key).map(str::to_owned).unwrap_or(key);
            if value.starts_with('\'') {
                Some(format!("{key}:{value}"))
            } else {
                Some(format!("{key}:{}", self.radix.decode_float(&value)))
            }
        });

        join_commas(elements)
    }
}

/// Classifies a decoded field value: quoted text vs. number (falling back to
/// text if it does not parse).
fn parse_value(value: &str) -> Value {
    if value.starts_with('\'') {
        // Drop the surrounding quotes (char-wise, so UTF-8 stays valid).
        let mut chars = value.chars();
        chars.next();
        let mut text: String = chars.collect();
        text.pop();
        Value::Text(text)
    } else {
        match value.parse::<f64>() {
            Ok(number) => Value::Float(number),
            Err(_) => Value::Text(value.to_owned()),
        }
    }
}

/// Joins string parts with commas without allocating an intermediate `Vec`.
fn join_commas(parts: impl Iterator<Item = String>) -> String {
    let mut out = String::new();
    for (i, part) in parts.enumerate() {
        if i > 0 {
            out.push(',');
        }
        out.push_str(&part);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample() -> Record {
        let mut record = Record::new();
        record
            .set("TIME", 1_700_000_000_000_i64)
            .set("LONG", 139.6917)
            .set("LATI", 35.6895)
            .set("MEMO", "hello world");
        record
    }

    #[test]
    fn plain_round_trips() {
        let codec = Senbay::new();
        let record = sample();
        let text = codec.encode(&record, Encoding::Plain);
        assert!(text.starts_with("V:3,"));

        let decoded = codec.decode(&text);
        assert_eq!(decoded.get("MEMO").unwrap(), &Value::Text("hello world".into()));
        assert_eq!(decoded.get("TIME").unwrap().as_f64(), Some(1_700_000_000_000.0));
        assert!(decoded.get("V").is_none());
    }

    #[test]
    fn compressed_round_trips() {
        let codec = Senbay::new();
        let record = sample();
        let text = codec.encode(&record, Encoding::Compressed);
        assert!(text.starts_with("V:4,"));

        let decoded = codec.decode(&text);
        assert_eq!(decoded.get("LATI").unwrap().as_f64(), Some(35.6895));
        assert_eq!(decoded.get("LONG").unwrap().as_f64(), Some(139.6917));
        assert_eq!(decoded.get("MEMO").unwrap().as_str(), Some("hello world"));
        assert_eq!(decoded.get("TIME").unwrap().as_f64(), Some(1_700_000_000_000.0));
    }

    #[test]
    fn compressed_uses_short_keys() {
        let codec = Senbay::new();
        let mut record = Record::new();
        record.set("TIME", 0_i64);
        // TIME -> "0", glued to the encoded value (zero == NUL code point).
        let text = codec.encode(&record, Encoding::Compressed);
        assert_eq!(text, "V:4,0\u{0}");
    }

    #[test]
    fn empty_record_encodes_header_only() {
        let codec = Senbay::new();
        let record = Record::new();
        assert_eq!(codec.encode(&record, Encoding::Plain), "V:3,");
        assert_eq!(codec.encode(&record, Encoding::Compressed), "V:4,");
    }

    #[test]
    fn json_output() {
        let mut record = Record::new();
        record.set("LATI", 35.6895).set("MEMO", "hi");
        let json = record.to_json();
        // BTreeMap ordering makes this deterministic.
        assert_eq!(json, r#"{"LATI":35.6895,"MEMO":"hi"}"#);
    }

    #[test]
    fn with_radix_validates_and_exposes_radix() {
        let codec = Senbay::with_radix(64).unwrap();
        assert_eq!(codec.radix().get(), 64);
        assert_eq!(Senbay::new().radix(), Radix::default());
        assert!(Senbay::with_radix(1).is_err());
    }

    #[test]
    fn decode_skips_malformed_and_unparsable_fields() {
        let codec = Senbay::new();
        // "MEMO" has no colon (skipped); "FOO:bar" is non-numeric, non-quoted text.
        let decoded = codec.decode("V:3,MEMO,FOO:bar,EMPTY:,NONE:None");
        assert!(decoded.get("MEMO").is_none());
        assert_eq!(decoded.get("FOO").unwrap().as_str(), Some("bar"));
        assert!(decoded.get("EMPTY").is_none());
        assert!(decoded.get("NONE").is_none());
    }

    #[test]
    fn expand_skips_empty_glued_field() {
        let codec = Senbay::new();
        // A bare reserved key "0" with no value expands to nothing.
        let decoded = codec.decode("V:4,0");
        assert!(decoded.get("TIME").is_none());
    }
}
