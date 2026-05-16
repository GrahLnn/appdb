use super::{AutoFill, normalized_now_token};

#[test]
fn normalized_now_token_uses_fixed_nine_digit_fraction() {
    assert_eq!(
        normalized_now_token("2026-04-23T08:00:00Z".to_owned()),
        "2026-04-23T08:00:00.000000000Z"
    );
    assert_eq!(
        normalized_now_token("2026-04-23T08:00:00.123Z".to_owned()),
        "2026-04-23T08:00:00.123000000Z"
    );
    assert_eq!(
        normalized_now_token("2026-04-23T08:00:00.123456789Z".to_owned()),
        "2026-04-23T08:00:00.123456789Z"
    );
}

#[test]
fn fill_now_resolves_pending_values_only_once() {
    let mut value = AutoFill::pending();
    value.fill_now_if_pending();
    let first = value.clone();
    value.fill_now_if_pending();
    assert_eq!(value, first);
    assert!(!value.is_pending());
}
