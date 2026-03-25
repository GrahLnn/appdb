use appdb::Relation;
use appdb::model::relation::RelationMeta;

#[derive(Relation)]
struct Follow;

#[derive(Relation)]
#[relation(name = "custom_edge")]
struct CustomFollow {
    created_at: i64,
}

fn main() {
    assert_eq!(Follow::relation_name(), "follow");
    assert_eq!(CustomFollow::relation_name(), "custom_edge");
}
