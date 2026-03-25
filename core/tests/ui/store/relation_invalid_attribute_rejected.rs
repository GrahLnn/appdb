use appdb::Relation;

#[derive(Relation)]
#[relation(other = "edge_name")]
struct InvalidRelation;

fn main() {}
