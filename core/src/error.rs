use thiserror::Error;

#[derive(Debug, Error)]
/// Library-level database error variants.
pub enum DBError {
    #[error("Database engine error: {0}")]
    Transport(String),
    #[error("SurrealDB error: {0}")]
    Surreal(String),
    #[error("Query response error: {0}")]
    QueryResponse(String),
    #[error("Database not initialized")]
    NotInitialized,
    #[error("Database has already been initialized")]
    AlreadyInitialized,
    #[error("Record not found")]
    NotFound,
    #[error("Missing table: {0}")]
    MissingTable(String),
    #[error("Conflict: {0}")]
    Conflict(String),
    #[error("Decode error: {0}")]
    Decode(String),
    #[error("Empty result from database operation: {0}")]
    EmptyResult(&'static str),
    #[error("Invalid identifier: {0}")]
    InvalidIdentifier(String),
    #[error("Invalid model shape: {0}")]
    InvalidModel(String),
}

impl From<surrealdb::Error> for DBError {
    fn from(err: surrealdb::Error) -> Self {
        classify_db_error_message(err.to_string())
    }
}

impl From<anyhow::Error> for DBError {
    fn from(err: anyhow::Error) -> Self {
        if let Some(db_err) = err.downcast_ref::<DBError>() {
            return clone_db_error(db_err);
        }
        classify_db_error_message(err.to_string())
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DBErrorKind {
    MissingTable,
    NotFound,
    Conflict,
    Decode,
    Transport,
    Engine,
    Other,
}

impl DBError {
    pub fn kind(&self) -> DBErrorKind {
        match self {
            DBError::NotFound => DBErrorKind::NotFound,
            DBError::MissingTable(_) => DBErrorKind::MissingTable,
            DBError::Conflict(_) => DBErrorKind::Conflict,
            DBError::Decode(_) => DBErrorKind::Decode,
            DBError::Transport(_) => DBErrorKind::Transport,
            DBError::Surreal(_) | DBError::QueryResponse(_) => DBErrorKind::Engine,
            DBError::NotInitialized
            | DBError::AlreadyInitialized
            | DBError::EmptyResult(_)
            | DBError::InvalidIdentifier(_)
            | DBError::InvalidModel(_) => DBErrorKind::Other,
        }
    }
}

pub fn classify_db_error_message(message: String) -> DBError {
    let lower = message.to_ascii_lowercase();
    if lower.contains("record not found") {
        DBError::NotFound
    } else if lower.contains("does not exist") && lower.contains("table") {
        DBError::MissingTable(message)
    } else if lower.contains("already exists")
        || lower.contains("duplicate key")
        || lower.contains("constraint violation")
        || lower.contains("conflict")
    {
        DBError::Conflict(message)
    } else if lower.contains("failed to deserialize")
        || lower.contains("invalid type")
        || lower.contains("missing field")
        || lower.contains("unknown variant")
        || lower.contains("expected ")
        || lower.contains("decode")
    {
        DBError::Decode(message)
    } else if lower.contains("transport")
        || lower.contains("connection")
        || lower.contains("socket")
        || lower.contains("timed out")
    {
        DBError::Transport(message)
    } else {
        DBError::Surreal(message)
    }
}

fn clone_db_error(err: &DBError) -> DBError {
    match err {
        DBError::Transport(message) => DBError::Transport(message.clone()),
        DBError::Surreal(message) => DBError::Surreal(message.clone()),
        DBError::QueryResponse(message) => DBError::QueryResponse(message.clone()),
        DBError::NotInitialized => DBError::NotInitialized,
        DBError::AlreadyInitialized => DBError::AlreadyInitialized,
        DBError::NotFound => DBError::NotFound,
        DBError::MissingTable(message) => DBError::MissingTable(message.clone()),
        DBError::Conflict(message) => DBError::Conflict(message.clone()),
        DBError::Decode(message) => DBError::Decode(message.clone()),
        DBError::EmptyResult(op) => DBError::EmptyResult(op),
        DBError::InvalidIdentifier(message) => DBError::InvalidIdentifier(message.clone()),
        DBError::InvalidModel(message) => DBError::InvalidModel(message.clone()),
    }
}
