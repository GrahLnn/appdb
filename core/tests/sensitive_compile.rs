#[test]
fn sensitive_macro_rules_are_enforced() {
    let tests = trybuild::TestCases::new();
    tests.pass("tests/ui/store/nested_ref_supported_shapes.rs");
    tests.pass("tests/ui/store/nested_ref_public_bindref.rs");
    tests.pass("tests/ui/store/derive_bridge_enum_dispatcher.rs");
    tests.compile_fail("tests/ui/store/nested_ref_legacy_attr_rejected.rs");
    tests.compile_fail("tests/ui/store/nested_ref_bindref_unique_rejected.rs");
    tests.compile_fail("tests/ui/store/derive_bridge_unit_variant.rs");
    tests.compile_fail("tests/ui/store/derive_bridge_struct_variant.rs");
    tests.compile_fail("tests/ui/store/derive_bridge_multi_field_variant.rs");
    tests.compile_fail("tests/ui/store/derive_bridge_payload_without_bridge.rs");
    tests.compile_fail("tests/ui/sensitive/no_secure_fields.rs");
    tests.compile_fail("tests/ui/sensitive/unsupported_secure_type.rs");
    tests.compile_fail("tests/ui/sensitive/secure_then_unique.rs");
    tests.compile_fail("tests/ui/sensitive/unique_then_secure.rs");
    tests.compile_fail("tests/ui/sensitive/no_legal_non_secure_lookup.rs");
    tests.compile_fail("tests/ui/store/multiple_id_fields.rs");
    tests.compile_fail("tests/ui/store/nested_ref_non_store_child.rs");
    tests.compile_fail("tests/ui/store/nested_ref_unsupported_box.rs");
    tests.compile_fail("tests/ui/store/nested_ref_unsupported_nested_wrapper.rs");
}
