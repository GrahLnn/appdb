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
        classify_surreal_error(err)
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

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ClassifiedDBError {
    pub kind: DBErrorKind,
    pub message: String,
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

impl ClassifiedDBError {
    pub fn into_db_error(self) -> DBError {
        match self.kind {
            DBErrorKind::NotFound => DBError::NotFound,
            DBErrorKind::MissingTable => DBError::MissingTable(self.message),
            DBErrorKind::Conflict => DBError::Conflict(self.message),
            DBErrorKind::Decode => DBError::Decode(self.message),
            DBErrorKind::Transport => DBError::Transport(self.message),
            DBErrorKind::Engine => DBError::Surreal(self.message),
            DBErrorKind::Other => DBError::Surreal(self.message),
        }
    }
}

pub fn classify_db_error_message(message: String) -> DBError {
    classify_db_error_text(message).into_db_error()
}

pub fn classify_surreal_error(err: surrealdb::Error) -> DBError {
    classify_db_error_message(err.to_string())
}

pub fn classify_db_error_text(message: String) -> ClassifiedDBError {
    let lower = message.to_ascii_lowercase();
    let kind = if lower.contains("record not found") {
        DBErrorKind::NotFound
    } else if lower.contains("does not exist") && lower.contains("table") {
        DBErrorKind::MissingTable
    } else if lower.contains("already exists")
        || lower.contains("duplicate key")
        || lower.contains("constraint violation")
        || lower.contains("conflict")
    {
        DBErrorKind::Conflict
    } else if lower.contains("failed to deserialize")
        || lower.contains("invalid type")
        || lower.contains("missing field")
        || lower.contains("unknown variant")
        || lower.contains("expected ")
        || lower.contains("decode")
    {
        DBErrorKind::Decode
    } else if lower.contains("transport")
        || lower.contains("connection")
        || lower.contains("socket")
        || lower.contains("timed out")
    {
        DBErrorKind::Transport
    } else {
        DBErrorKind::Engine
    };

    ClassifiedDBError { kind, message }
}

pub fn classify_db_error(err: &anyhow::Error) -> DBError {
    if let Some(db_err) = err.downcast_ref::<DBError>() {
        return clone_db_error(db_err);
    }
    classify_db_error_message(err.to_string())
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
