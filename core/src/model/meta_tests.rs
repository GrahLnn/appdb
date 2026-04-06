use super::{default_table_name, register_table};

#[test]
fn table_name_is_snake_case() {
    assert_eq!(default_table_name("User"), "user");
    assert_eq!(default_table_name("UserProfile"), "user_profile");
    assert_eq!(default_table_name("crate::domain::DbUser"), "db_user");
}

#[test]
fn register_table_is_idempotent_for_model() {
    let first = register_table("ModelA", "alpha");
    let second = register_table("ModelA", "beta");
    assert_eq!(first, "alpha");
    assert_eq!(second, "alpha");
}
