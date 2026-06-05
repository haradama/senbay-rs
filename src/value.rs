//! The value type stored in a [`Record`](crate::Record) field.

use std::fmt;

/// A single Senbay field value.
///
/// Numbers are carried in their natural Rust type; text is stored unquoted
/// (the quotes are part of the wire format, not the value).
#[derive(Debug, Clone, PartialEq)]
pub enum Value {
    /// An integer value.
    Int(i64),
    /// A floating-point value.
    Float(f64),
    /// A text value.
    Text(String),
}

impl Value {
    /// Returns `true` if this is a text value.
    pub fn is_text(&self) -> bool {
        matches!(self, Value::Text(_))
    }

    /// Returns the numeric value as an `f64`, or `None` for text.
    pub fn as_f64(&self) -> Option<f64> {
        match self {
            Value::Int(v) => Some(*v as f64),
            Value::Float(v) => Some(*v),
            Value::Text(_) => None,
        }
    }

    /// Returns the text value, or `None` for numbers.
    pub fn as_str(&self) -> Option<&str> {
        match self {
            Value::Text(s) => Some(s),
            _ => None,
        }
    }

    /// Converts the value to a `serde_json::Value` for serialization.
    pub(crate) fn to_json(&self) -> serde_json::Value {
        match self {
            Value::Int(v) => serde_json::Value::from(*v),
            Value::Float(v) => serde_json::Number::from_f64(*v)
                .map(serde_json::Value::Number)
                .unwrap_or(serde_json::Value::Null),
            Value::Text(s) => serde_json::Value::from(s.clone()),
        }
    }
}

/// Formats a value as it appears in Senbay text: numbers bare, text quoted.
impl fmt::Display for Value {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Value::Int(v) => write!(f, "{v}"),
            Value::Float(v) => write!(f, "{v}"),
            Value::Text(s) => write!(f, "'{s}'"),
        }
    }
}

macro_rules! impl_from_int {
    ($($t:ty),*) => {$(
        impl From<$t> for Value {
            fn from(v: $t) -> Self {
                Value::Int(v as i64)
            }
        }
    )*};
}
impl_from_int!(i8, i16, i32, i64, u8, u16, u32);

impl From<f32> for Value {
    fn from(v: f32) -> Self {
        Value::Float(v as f64)
    }
}

impl From<f64> for Value {
    fn from(v: f64) -> Self {
        Value::Float(v)
    }
}

impl From<&str> for Value {
    fn from(v: &str) -> Self {
        Value::Text(v.to_owned())
    }
}

impl From<String> for Value {
    fn from(v: String) -> Self {
        Value::Text(v)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn is_text_distinguishes_variants() {
        assert!(Value::Text("hi".into()).is_text());
        assert!(!Value::Int(1).is_text());
        assert!(!Value::Float(1.0).is_text());
    }

    #[test]
    fn as_f64_covers_all_variants() {
        assert_eq!(Value::Int(42).as_f64(), Some(42.0));
        assert_eq!(Value::Float(1.5).as_f64(), Some(1.5));
        assert_eq!(Value::Text("x".into()).as_f64(), None);
    }

    #[test]
    fn as_str_covers_all_variants() {
        assert_eq!(Value::Text("hi".into()).as_str(), Some("hi"));
        assert_eq!(Value::Int(1).as_str(), None);
        assert_eq!(Value::Float(1.0).as_str(), None);
    }

    #[test]
    fn to_json_covers_all_variants() {
        assert_eq!(Value::Int(7).to_json(), serde_json::json!(7));
        assert_eq!(Value::Float(1.5).to_json(), serde_json::json!(1.5));
        assert_eq!(Value::Text("hi".into()).to_json(), serde_json::json!("hi"));
        // Non-finite floats are not representable in JSON and become null.
        assert_eq!(Value::Float(f64::NAN).to_json(), serde_json::Value::Null);
    }

    #[test]
    fn display_formats_numbers_bare_and_text_quoted() {
        assert_eq!(Value::Int(42).to_string(), "42");
        assert_eq!(Value::Float(1.5).to_string(), "1.5");
        assert_eq!(Value::Text("hi".into()).to_string(), "'hi'");
    }

    #[test]
    fn from_conversions() {
        assert_eq!(Value::from(1_i8), Value::Int(1));
        assert_eq!(Value::from(1_u32), Value::Int(1));
        assert_eq!(Value::from(1.5_f32), Value::Float(1.5));
        assert_eq!(Value::from(1.5_f64), Value::Float(1.5));
        assert_eq!(Value::from("hi"), Value::Text("hi".into()));
        assert_eq!(Value::from(String::from("hi")), Value::Text("hi".into()));
    }
}
