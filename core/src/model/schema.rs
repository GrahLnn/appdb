/// Inventory item for one schema DDL statement.
pub struct SchemaItem {
    /// Raw DDL submitted into the schema inventory.
    pub ddl: &'static str,
}
inventory::collect!(SchemaItem);

/// Trait implemented by types that register a schema statement.
pub trait SchemaDef {
    /// DDL statement submitted during database initialization.
    const SCHEMA: &'static str;
}

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
