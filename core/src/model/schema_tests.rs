use super::{HnswIndexDef, VectorDistance, VectorIndexType};

#[test]
fn hnsw_index_def_renders_surrealql_vector_index() {
    let ddl = HnswIndexDef::new(
        "audio_style_embedding_hnsw",
        "audio_style_embedding",
        "embedding",
        256,
    )
    .vector_type(VectorIndexType::F32)
    .distance(VectorDistance::Cosine)
    .ef_construction(150)
    .m(12)
    .concurrently()
    .ddl();

    assert_eq!(
        ddl,
        "DEFINE INDEX IF NOT EXISTS audio_style_embedding_hnsw ON audio_style_embedding FIELDS embedding HNSW DIMENSION 256 TYPE F32 DIST COSINE EFC 150 M 12 CONCURRENTLY;"
    );
}

#[test]
fn hnsw_index_def_supports_surrealdb_defaults() {
    let ddl = HnswIndexDef::new("idx_nested", "documents", "items.embedding", 4).ddl();

    assert_eq!(
        ddl,
        "DEFINE INDEX IF NOT EXISTS idx_nested ON documents FIELDS items.embedding HNSW DIMENSION 4;"
    );
}

#[test]
#[should_panic(expected = "dimension must be greater than zero")]
fn hnsw_index_def_rejects_zero_dimension() {
    let _ = HnswIndexDef::new("idx", "documents", "embedding", 0).ddl();
}

#[test]
#[should_panic(expected = "field must be a plain SurrealQL field path")]
fn hnsw_index_def_rejects_non_plain_field_path() {
    let _ = HnswIndexDef::new("idx", "documents", "embedding;DELETE documents", 4).ddl();
}
