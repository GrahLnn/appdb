use serde::{Deserialize, Deserializer, Serialize, Serializer};
use specta::Type;
use std::fmt;
use surrealdb::types::{Kind, Number, RecordId, RecordIdKey, SurrealValue, ToSql, Value, kind};

#[derive(Debug, Deserialize)]
#[serde(untagged)]
enum IdOrRecordId {
    String(String),
    Number(i64),
    Record(RecordId),
}

fn record_key_to_id(key: RecordIdKey) -> Result<Id, String> {
    match key {
        RecordIdKey::String(value) => Ok(Id::String(value)),
        RecordIdKey::Number(value) => Ok(Id::Number(value)),
        other => Err(format!(
            "only string/number id is supported right now, got {}",
            other.to_sql()
        )),
    }
}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Type)]
/// Application-facing id type that accepts either string or integer ids.
pub enum Id {
    /// String record key.
    String(String),
    /// Integer record key.
    Number(i64),
}

impl Id {
    /// Returns the inner string when this id is string-backed.
    pub fn as_string(&self) -> Option<&str> {
        match self {
            Self::String(value) => Some(value.as_str()),
            Self::Number(_) => None,
        }
    }

    /// Returns the inner number when this id is number-backed.
    pub fn as_number(&self) -> Option<i64> {
        match self {
            Self::String(_) => None,
            Self::Number(value) => Some(*value),
        }
    }

    /// Converts this value into a SurrealDB record-id key.
    pub fn into_record_id_key(self) -> RecordIdKey {
        match self {
            Self::String(value) => RecordIdKey::String(value),
            Self::Number(value) => RecordIdKey::Number(value),
        }
    }
}

impl From<String> for Id {
    fn from(value: String) -> Self {
        Self::String(value)
    }
}

impl From<&str> for Id {
    fn from(value: &str) -> Self {
        Self::String(value.to_owned())
    }
}

impl From<i64> for Id {
    fn from(value: i64) -> Self {
        Self::Number(value)
    }
}

impl From<Id> for RecordIdKey {
    fn from(value: Id) -> Self {
        value.into_record_id_key()
    }
}

impl fmt::Display for Id {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::String(value) => f.write_str(value),
            Self::Number(value) => write!(f, "{value}"),
        }
    }
}

impl Serialize for Id {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        match self {
            Self::String(value) => serializer.serialize_str(value),
            Self::Number(value) => serializer.serialize_i64(*value),
        }
    }
}

impl<'de> Deserialize<'de> for Id {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        let value = IdOrRecordId::deserialize(deserializer)?;
        match value {
            IdOrRecordId::String(value) => Ok(Self::String(value)),
            IdOrRecordId::Number(value) => Ok(Self::Number(value)),
            IdOrRecordId::Record(record) => record_key_to_id(record.key).map_err(|message| {
                <D::Error as serde::de::Error>::custom(format!(
                    "failed to deserialize id from record id: {message}"
                ))
            }),
        }
    }
}

impl SurrealValue for Id {
    fn kind_of() -> Kind {
        kind!(string | number)
    }

    fn is_value(value: &Value) -> bool {
        match value {
            Value::String(_) | Value::Number(Number::Int(_)) => true,
            Value::RecordId(record) => {
                matches!(record.key, RecordIdKey::String(_) | RecordIdKey::Number(_))
            }
            _ => false,
        }
    }

    fn into_value(self) -> Value {
        match self {
            Self::String(value) => Value::String(value),
            Self::Number(value) => Value::Number(Number::Int(value)),
        }
    }

    fn from_value(value: Value) -> Result<Self, surrealdb::types::Error> {
        match value {
            Value::String(value) => Ok(Self::String(value)),
            Value::Number(Number::Int(value)) => Ok(Self::Number(value)),
            Value::RecordId(record) => {
                record_key_to_id(record.key).map_err(surrealdb::types::Error::internal)
            }
            other => Err(surrealdb::types::Error::internal(format!(
                "expected string/number/record id for Id, got {}",
                other.kind().to_sql()
            ))),
        }
    }
}

/// Deserializes a string, number, or record id into a plain string id.
pub fn deserialize_id_or_record_id_as_string<'de, D>(deserializer: D) -> Result<String, D::Error>
where
    D: Deserializer<'de>,
{
    let value = Id::deserialize(deserializer)?;
    Ok(value.to_string())
}

/// Serializes an id field as a plain string.
pub fn serialize_id_as_string<S>(value: &str, serializer: S) -> Result<S::Ok, S::Error>
where
    S: Serializer,
{
    serializer.serialize_str(value)
}

pub fn deserialize_record_id_or_compat_string<'de, D>(deserializer: D) -> Result<RecordId, D::Error>
where
    D: Deserializer<'de>,
{
    let value = serde_json::Value::deserialize(deserializer)?;
    match value {
        serde_json::Value::String(text) => parse_record_id_or_plain_string(&text, Some("_"))
            .map_err(|invalid| {
                <D::Error as serde::de::Error>::custom(format!(
                    "failed to deserialize record id: invalid record id string `{invalid}`"
                ))
            }),
        other => serde_json::from_value(other).map_err(|err| {
            <D::Error as serde::de::Error>::custom(format!(
                "failed to deserialize record id: {err}"
            ))
        }),
    }
}

pub fn record_id_to_plain_string(record: &RecordId) -> String {
    match &record.key {
        RecordIdKey::String(value) => value.trim_matches('`').to_owned(),
        RecordIdKey::Number(value) => value.to_string(),
        other => other.to_sql(),
    }
}

pub fn parse_record_id_or_plain_string<'a>(
    text: &'a str,
    fallback_table: Option<&str>,
) -> Result<RecordId, &'a str> {
    if let Some(record) = crate::parse_record_id_compat_string(text) {
        Ok(record)
    } else if let Ok(record) = RecordId::parse_simple(text) {
        Ok(record)
    } else if let Some(table) = fallback_table {
        Ok(RecordId::new(table, text.trim_matches('`').to_owned()))
    } else {
        Err(text)
    }
}

pub fn normalize_public_root_id_value(value: &mut serde_json::Value) {
    let Some(id) = value.as_object_mut().and_then(|map| map.get_mut("id")) else {
        return;
    };

    let normalized = match id {
        serde_json::Value::String(text) => parse_record_id_or_plain_string(text, None)
            .ok()
            .map(|record| serde_json::Value::String(record_id_to_plain_string(&record))),
        serde_json::Value::Object(_) => serde_json::from_value::<RecordId>(id.clone())
            .ok()
            .map(|record| serde_json::Value::String(record_id_to_plain_string(&record))),
        _ => None,
    };

    if let Some(normalized) = normalized {
        *id = normalized;
    }
}

#[cfg(test)]
#[path = "id_tests.rs"]
mod tests;
