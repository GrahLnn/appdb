use serde::{Deserialize, Serialize};
use specta::Type;
use std::fmt;
use surrealdb::types::{Kind, SurrealValue, Value};
use surrealdb_types::Datetime;

/// Opaque appdb-managed scalar used by `#[fill(...)]` fields.
///
/// The field stays pending until the Store write pipeline resolves it into a
/// concrete token such as the current timestamp.
#[derive(
    Debug, Clone, Default, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize, Type,
)]
#[serde(transparent)]
pub struct AutoFill(Option<String>);

impl AutoFill {
    /// Creates a pending value that will be resolved by the Store write path.
    pub fn pending() -> Self {
        Self(None)
    }

    /// Creates an already-resolved value that bypasses automatic filling.
    pub fn resolved(value: impl Into<String>) -> Self {
        Self(Some(value.into()))
    }

    /// Returns the resolved token when available.
    pub fn as_str(&self) -> Option<&str> {
        self.0.as_deref()
    }

    /// Returns the raw optional token.
    pub fn into_inner(self) -> Option<String> {
        self.0
    }

    /// Returns whether the value still needs to be filled.
    pub fn is_pending(&self) -> bool {
        self.0.is_none()
    }

    /// Resolves the value only when it is still pending.
    pub fn fill_with_if_pending(&mut self, fill: impl FnOnce() -> String) {
        if self.is_pending() {
            self.0 = Some(fill());
        }
    }

    /// Resolves the value to the current UTC timestamp when it is still pending.
    pub fn fill_now_if_pending(&mut self) {
        self.fill_with_if_pending(|| normalized_now_token(Datetime::now().to_string()));
    }
}

impl From<String> for AutoFill {
    fn from(value: String) -> Self {
        Self::resolved(value)
    }
}

impl From<&str> for AutoFill {
    fn from(value: &str) -> Self {
        Self::resolved(value)
    }
}

impl From<Option<String>> for AutoFill {
    fn from(value: Option<String>) -> Self {
        Self(value)
    }
}

impl From<AutoFill> for Option<String> {
    fn from(value: AutoFill) -> Self {
        value.0
    }
}

impl fmt::Display for AutoFill {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self.as_str() {
            Some(value) => f.write_str(value),
            None => f.write_str(""),
        }
    }
}

impl SurrealValue for AutoFill {
    fn kind_of() -> Kind {
        <Option<String> as SurrealValue>::kind_of()
    }

    fn is_value(value: &Value) -> bool {
        <Option<String> as SurrealValue>::is_value(value)
    }

    fn into_value(self) -> Value {
        self.0.into_value()
    }

    fn from_value(value: Value) -> Result<Self, surrealdb::Error> {
        <Option<String> as SurrealValue>::from_value(value).map(Self)
    }
}

fn normalized_now_token(raw: String) -> String {
    let Some(prefix) = raw.strip_suffix('Z') else {
        return raw;
    };

    let (head, fraction) = match prefix.split_once('.') {
        Some((head, fraction)) => (head, fraction),
        None => return format!("{prefix}.000000000Z"),
    };

    let mut digits = fraction.to_owned();
    if digits.len() < 9 {
        digits.push_str(&"0".repeat(9 - digits.len()));
    } else if digits.len() > 9 {
        digits.truncate(9);
    }

    format!("{head}.{digits}Z")
}

#[cfg(test)]
#[path = "autofill_tests.rs"]
mod tests;
