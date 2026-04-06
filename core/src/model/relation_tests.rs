use super::{RelationMeta, ensure_relation_name, register_relation};

#[derive(crate::Relation)]
struct AutoRelName;

#[derive(crate::Relation)]
#[relation(name = "manual_rel")]
struct ManualRelName;

#[test]
fn relation_name_accepts_valid_identifier() {
    assert!(ensure_relation_name("sign_in").is_ok());
    assert!(ensure_relation_name("_private_rel").is_ok());
}

#[test]
fn relation_name_accepts_arbitrary_name() {
    assert!(ensure_relation_name("9invalid").is_ok());
    assert!(ensure_relation_name("bad-name").is_ok());
    assert!(ensure_relation_name("").is_ok());
}

#[test]
fn relation_registration_works() {
    assert_eq!(register_relation("follows"), "follows");
}

#[test]
fn declare_relation_auto_name_works() {
    assert_eq!(AutoRelName::relation_name(), "auto_rel_name");
}

#[test]
fn declare_relation_manual_name_works() {
    assert_eq!(ManualRelName::relation_name(), "manual_rel");
}
