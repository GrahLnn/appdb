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
fn relation_name_rejects_non_identifier() {
    for name in ["9invalid", "bad-name", "", "bad name", "bad;name"] {
        let err = ensure_relation_name(name).expect_err("invalid relation name should fail");
        assert!(
            err.to_string()
                .contains("must be a plain SurrealQL identifier"),
            "{err}"
        );
    }
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
