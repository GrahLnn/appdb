use appdb::impl_schema;
use appdb::model::schema::{SchemaDef, VectorDistance, VectorIndexType};

struct EventLog;
struct EventEmbedding;

impl_schema!(
    EventLog,
    concat!(
        "DEFINE TABLE IF NOT EXISTS ",
        "event_log",
        " SCHEMAFULL;"
    )
);

appdb::impl_hnsw_index!(
    EventEmbedding,
    name: "event_embedding_hnsw",
    table: "event_embedding",
    field: "embedding",
    dimension: 64,
    vector_type: VectorIndexType::F32,
    distance: VectorDistance::Cosine,
    ef_construction: 150,
    m: 12,
    concurrently: true,
);

fn main() {
    assert_eq!(
        <EventLog as SchemaDef>::SCHEMA,
        "DEFINE TABLE IF NOT EXISTS event_log SCHEMAFULL;"
    );
    assert_eq!(<EventEmbedding as SchemaDef>::SCHEMA, "");
}
