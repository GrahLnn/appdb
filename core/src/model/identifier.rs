use anyhow::Result;

use crate::error::DBError;

pub(crate) fn ensure_identifier(name: &str, kind: &'static str) -> Result<()> {
    if name.is_empty() {
        return Err(DBError::InvalidIdentifier(format!("{kind} cannot be empty")).into());
    }

    let mut chars = name.chars();
    let Some(first) = chars.next() else {
        return Err(DBError::InvalidIdentifier(format!("{kind} cannot be empty")).into());
    };

    if !(first.is_ascii_alphabetic() || first == '_') {
        return Err(DBError::InvalidIdentifier(format!(
            "{kind} `{name}` must start with an ASCII letter or `_`"
        ))
        .into());
    }

    if !chars.all(|ch| ch.is_ascii_alphanumeric() || ch == '_') {
        return Err(DBError::InvalidIdentifier(format!(
            "{kind} `{name}` must contain only ASCII letters, digits, or `_`"
        ))
        .into());
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::ensure_identifier;

    #[test]
    fn accepts_ascii_identifier() {
        assert!(ensure_identifier("valid_name_1", "field name").is_ok());
        assert!(ensure_identifier("_private", "field name").is_ok());
    }

    #[test]
    fn rejects_invalid_identifier() {
        assert!(ensure_identifier("", "field name").is_err());
        assert!(ensure_identifier("9invalid", "field name").is_err());
        assert!(ensure_identifier("bad-name", "field name").is_err());
    }
}
