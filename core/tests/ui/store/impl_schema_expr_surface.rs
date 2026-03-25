use appdb::impl_schema;
use appdb::model::schema::SchemaDef;

struct EventLog;

impl_schema!(
    EventLog,
    concat!(
        "DEFINE TABLE IF NOT EXISTS ",
        "event_log",
        " SCHEMAFULL;"
    )
);

fn main() {
    assert_eq!(
        <EventLog as SchemaDef>::SCHEMA,
        "DEFINE TABLE IF NOT EXISTS event_log SCHEMAFULL;"
    );
}
