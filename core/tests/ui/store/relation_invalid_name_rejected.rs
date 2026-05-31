use appdb::Relation;

#[derive(Relation)]
#[relation(name = "bad-name")]
struct InvalidRelationName;

fn main() {}
