use catalyst::text::parse;

#[test]
fn excessive_recursive_expression_nesting_is_refused() {
    for expression in [
        format!("{}x{}", "(".repeat(65), ")".repeat(65)),
        format!("{}x", "-".repeat(65)),
        format!("{}x", "x^".repeat(65)),
        format!("{}x{}", "sin(".repeat(65), ")".repeat(65)),
        format!("{}x{}", "sin(-(".repeat(22), "))".repeat(22)),
        format!("{}x{}", "(".repeat(10_000), ")".repeat(10_000)),
    ] {
        assert!(
            parse(&format!("func bounded(x) = {expression}")).is_err(),
            "excessive recursive nesting must return a refusal"
        );
    }
}

#[test]
fn permitted_nesting_and_long_flat_expressions_still_parse() {
    for expression in [
        format!("{}x{}", "(".repeat(64), ")".repeat(64)),
        format!("{}x", "-".repeat(64)),
        format!("{}x", "x^".repeat(64)),
        format!("{}x{}", "sin(".repeat(64), ")".repeat(64)),
        format!("x{}", "+x".repeat(4096)),
    ] {
        assert!(parse(&format!("func bounded(x) = {expression}")).is_ok());
    }
}
