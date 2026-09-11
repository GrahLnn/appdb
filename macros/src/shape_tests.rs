use super::shape;
use syn::{Type, parse_quote};

fn child_record_id(leaf: &Type) -> Option<Type> {
    match leaf {
        Type::Path(path) if path.path.is_ident("Child") => {
            Some(parse_quote!(::surrealdb::types::RecordId))
        }
        _ => None,
    }
}

fn normalized_tokens(value: &impl quote::ToTokens) -> String {
    value.to_token_stream().to_string().replace(' ', "")
}

#[test]
fn map_option_vec_preserves_nested_wrapper_order() {
    let source: Type = parse_quote!(Option<Vec<Child>>);
    let stored =
        shape::map_option_vec(&source, &child_record_id).expect("Child is a supported leaf");

    assert_eq!(
        normalized_tokens(&stored),
        normalized_tokens(&quote::quote!(
            ::std::option::Option<::std::vec::Vec<::surrealdb::types::RecordId>>
        ))
    );
}

#[test]
fn map_option_vec_keeps_leaf_operator_in_control_of_support() {
    let unsupported: Type = parse_quote!(Option<Box<Child>>);
    assert!(shape::map_option_vec(&unsupported, &child_record_id).is_none());
}

#[test]
fn peel_option_vec_returns_the_direct_leaf_without_wrappers() {
    let source: Type = parse_quote!(Option<Vec<Child>>);
    let leaf = shape::peel_option_vec(&source, &|leaf| Some(leaf.clone()))
        .expect("Child is a supported leaf");

    assert_eq!(normalized_tokens(&leaf), "Child");
}

#[test]
fn container_inner_type_matches_existing_last_segment_rule() {
    let option: Type = parse_quote!(std::option::Option<Child>);
    let vector: Type = parse_quote!(std::vec::Vec<Child>);

    assert!(shape::option_inner_type(&option).is_some());
    assert!(shape::vec_inner_type(&vector).is_some());
    assert!(shape::option_inner_type(&vector).is_none());
    assert!(shape::vec_inner_type(&option).is_none());
}
