//! An in-memory Senbay record: an ordered set of key/value fields.

use std::collections::BTreeMap;
use std::collections::btree_map;

use crate::value::Value;

/// How a record should be serialized.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Encoding {
    /// Human-readable, `V:3`-tagged form with values written verbatim.
    #[default]
    Plain,
    /// Compact, `V:4`-tagged form with reserved keys and BaseX numbers.
    Compressed,
}

/// A collection of Senbay fields.
///
/// Fields are kept sorted by key, so encoding is deterministic and easy to test.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Record {
    fields: BTreeMap<String, Value>,
}

impl Record {
    /// Creates an empty record.
    pub fn new() -> Self {
        Record::default()
    }

    /// Sets a field, returning `&mut self` for chaining.
    ///
    /// ```
    /// # use senbay_rs::Record;
    /// let mut record = Record::new();
    /// record.set("TIME", 1_700_000_000_000_i64).set("LATI", 35.6895);
    /// ```
    pub fn set(&mut self, key: impl Into<String>, value: impl Into<Value>) -> &mut Self {
        self.fields.insert(key.into(), value.into());
        self
    }

    /// Returns the value for `key`, if present.
    pub fn get(&self, key: &str) -> Option<&Value> {
        self.fields.get(key)
    }

    /// Removes a field, returning its value if it was present.
    pub fn remove(&mut self, key: &str) -> Option<Value> {
        self.fields.remove(key)
    }

    /// Removes all fields.
    pub fn clear(&mut self) {
        self.fields.clear();
    }

    /// Returns the number of fields.
    pub fn len(&self) -> usize {
        self.fields.len()
    }

    /// Returns `true` if the record has no fields.
    pub fn is_empty(&self) -> bool {
        self.fields.is_empty()
    }

    /// Iterates over the fields in key order.
    pub fn iter(&self) -> Iter<'_> {
        Iter {
            inner: self.fields.iter(),
        }
    }

    /// Serializes the record to a JSON object string.
    pub fn to_json(&self) -> String {
        let map: serde_json::Map<String, serde_json::Value> = self
            .fields
            .iter()
            .map(|(k, v)| (k.clone(), v.to_json()))
            .collect();
        serde_json::Value::Object(map).to_string()
    }
}

impl<'a> IntoIterator for &'a Record {
    type Item = (&'a str, &'a Value);
    type IntoIter = Iter<'a>;

    fn into_iter(self) -> Self::IntoIter {
        self.iter()
    }
}

/// Builds a record from `(key, value)` pairs.
impl<K, V> FromIterator<(K, V)> for Record
where
    K: Into<String>,
    V: Into<Value>,
{
    fn from_iter<I: IntoIterator<Item = (K, V)>>(iter: I) -> Self {
        let fields = iter
            .into_iter()
            .map(|(k, v)| (k.into(), v.into()))
            .collect();
        Record { fields }
    }
}

/// Iterator over a record's fields, yielding `(&str, &Value)` in key order.
pub struct Iter<'a> {
    inner: btree_map::Iter<'a, String, Value>,
}

impl<'a> Iterator for Iter<'a> {
    type Item = (&'a str, &'a Value);

    fn next(&mut self) -> Option<Self::Item> {
        self.inner.next().map(|(k, v)| (k.as_str(), v))
    }
}
