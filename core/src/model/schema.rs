/// Inventory item for one schema DDL statement.
pub struct SchemaItem {
    /// Raw DDL submitted into the schema inventory.
    pub ddl: &'static str,
}
inventory::collect!(SchemaItem);

/// Inventory item for one HNSW vector index definition.
pub struct HnswSchemaItem {
    pub index: HnswIndexDef,
}
inventory::collect!(HnswSchemaItem);

/// Trait implemented by types that register a schema statement.
pub trait SchemaDef {
    /// DDL statement submitted during database initialization.
    const SCHEMA: &'static str;
}

/// Primitive scalar representation used by SurrealDB vector indexes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum VectorIndexType {
    F64,
    F32,
    F16,
    I64,
    I32,
    I16,
    I8,
    U64,
    U32,
    U16,
    U8,
}

impl VectorIndexType {
    pub const fn as_surql(self) -> &'static str {
        match self {
            Self::F64 => "F64",
            Self::F32 => "F32",
            Self::F16 => "F16",
            Self::I64 => "I64",
            Self::I32 => "I32",
            Self::I16 => "I16",
            Self::I8 => "I8",
            Self::U64 => "U64",
            Self::U32 => "U32",
            Self::U16 => "U16",
            Self::U8 => "U8",
        }
    }
}

/// Distance metric used by SurrealDB vector indexes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum VectorDistance {
    Euclidean,
    Cosine,
    InnerProduct,
    CosineNormalized,
}

impl VectorDistance {
    pub const fn as_surql(self) -> &'static str {
        match self {
            Self::Euclidean => "EUCLIDEAN",
            Self::Cosine => "COSINE",
            Self::InnerProduct => "INNER_PRODUCT",
            Self::CosineNormalized => "COSINE_NORMALIZED",
        }
    }
}

/// Definition for one SurrealDB HNSW vector index.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct HnswIndexDef {
    pub name: &'static str,
    pub table: &'static str,
    pub field: &'static str,
    pub dimension: usize,
    pub vector_type: Option<VectorIndexType>,
    pub distance: Option<VectorDistance>,
    pub ef_construction: Option<usize>,
    pub m: Option<usize>,
    pub concurrently: bool,
    pub defer: bool,
}

impl HnswIndexDef {
    pub const fn new(
        name: &'static str,
        table: &'static str,
        field: &'static str,
        dimension: usize,
    ) -> Self {
        Self {
            name,
            table,
            field,
            dimension,
            vector_type: None,
            distance: None,
            ef_construction: None,
            m: None,
            concurrently: false,
            defer: false,
        }
    }

    pub const fn vector_type(mut self, vector_type: VectorIndexType) -> Self {
        self.vector_type = Some(vector_type);
        self
    }

    pub const fn distance(mut self, distance: VectorDistance) -> Self {
        self.distance = Some(distance);
        self
    }

    pub const fn ef_construction(mut self, ef_construction: usize) -> Self {
        self.ef_construction = Some(ef_construction);
        self
    }

    pub const fn m(mut self, m: usize) -> Self {
        self.m = Some(m);
        self
    }

    pub const fn concurrently(mut self) -> Self {
        self.concurrently = true;
        self
    }

    pub const fn deferred(mut self) -> Self {
        self.defer = true;
        self
    }

    pub fn ddl(self) -> String {
        assert!(
            self.dimension > 0,
            "HNSW vector index dimension must be greater than zero"
        );
        assert!(
            self.name.bytes().all(is_schema_identifier_byte),
            "HNSW vector index name must be a plain SurrealQL identifier"
        );
        assert!(
            self.table.bytes().all(is_schema_identifier_byte),
            "HNSW vector index table must be a plain SurrealQL identifier"
        );
        assert!(
            self.field.bytes().all(is_schema_field_byte),
            "HNSW vector index field must be a plain SurrealQL field path"
        );

        let mut ddl = format!(
            "DEFINE INDEX IF NOT EXISTS {} ON {} FIELDS {} HNSW DIMENSION {}",
            self.name, self.table, self.field, self.dimension
        );
        if let Some(vector_type) = self.vector_type {
            ddl.push_str(" TYPE ");
            ddl.push_str(vector_type.as_surql());
        }
        if let Some(distance) = self.distance {
            ddl.push_str(" DIST ");
            ddl.push_str(distance.as_surql());
        }
        if let Some(ef_construction) = self.ef_construction {
            ddl.push_str(" EFC ");
            ddl.push_str(&ef_construction.to_string());
        }
        if let Some(m) = self.m {
            ddl.push_str(" M ");
            ddl.push_str(&m.to_string());
        }
        if self.concurrently {
            ddl.push_str(" CONCURRENTLY");
        }
        if self.defer {
            ddl.push_str(" DEFER");
        }
        ddl.push(';');
        ddl
    }
}

fn is_schema_identifier_byte(byte: u8) -> bool {
    byte.is_ascii_alphanumeric() || byte == b'_'
}

fn is_schema_field_byte(byte: u8) -> bool {
    is_schema_identifier_byte(byte) || byte == b'.'
}

#[cfg(test)]
#[path = "schema_tests.rs"]
mod tests;

#[macro_export]
/// Registers a schema DDL string for a type.
macro_rules! impl_schema {
    ($ty:ty, $ddl:expr) => {
        impl $crate::model::schema::SchemaDef for $ty {
            const SCHEMA: &'static str = $ddl;
        }

        inventory::submit! {
            $crate::model::schema::SchemaItem {
                ddl: < $ty as $crate::model::schema::SchemaDef >::SCHEMA,
            }
        }
    };
}

#[macro_export]
/// Registers a SurrealDB HNSW vector index for a type.
macro_rules! impl_hnsw_index {
    (
        $ty:ty,
        name: $name:expr,
        table: $table:expr,
        field: $field:expr,
        dimension: $dimension:expr
        $(, vector_type: $vector_type:expr)?
        $(, distance: $distance:expr)?
        $(, ef_construction: $ef_construction:expr)?
        $(, m: $m:expr)?
        $(, concurrently: $concurrently:expr)?
        $(, defer: $defer:expr)?
        $(,)?
    ) => {
        impl $crate::model::schema::SchemaDef for $ty {
            const SCHEMA: &'static str = "";
        }

        ::inventory::submit! {
            $crate::model::schema::HnswSchemaItem {
                index: $crate::model::schema::HnswIndexDef {
                    name: $name,
                    table: $table,
                    field: $field,
                    dimension: $dimension,
                    vector_type: $crate::impl_hnsw_index!(@optional; $($vector_type)?),
                    distance: $crate::impl_hnsw_index!(@optional; $($distance)?),
                    ef_construction: $crate::impl_hnsw_index!(@optional; $($ef_construction)?),
                    m: $crate::impl_hnsw_index!(@optional; $($m)?),
                    concurrently: $crate::impl_hnsw_index!(@optional_bool; $($concurrently)?),
                    defer: $crate::impl_hnsw_index!(@optional_bool; $($defer)?),
                },
            }
        }
    };
    (@optional;) => {
        None
    };
    (@optional; $value:expr) => {
        Some($value)
    };
    (@optional_bool;) => {
        false
    };
    (@optional_bool; $value:expr) => {
        $value
    };
}
