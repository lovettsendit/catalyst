//! The front door, exercised the way the two callers it exists for use it:
//! a crate calling `gradient`, and a process passing JSON to `solve`.
//!
//! Correctness is still checked against central differences, because a
//! convenient wrapper around a wrong answer is worse than no wrapper.

use catalyst::api::{gradient, solve};
use catalyst::text;

#[test]
fn a_power_with_a_constant_exponent_is_differentiable_at_zero() {
    // x^2 at x = 0 has derivative 0, not NaN: the exponent is constant, so
    // no logarithm of zero belongs in the answer. Found by a first-time
    // user whose weight-decay term was wd^2 with wd = 0.
    let answer = gradient("func f(x, y) = x^2 + y^3 + 1", &[0.0, 0.0]).expect("finite");
    assert_eq!(answer.value, 1.0);
    assert_eq!(answer.gradient, vec![0.0, 0.0]);
    // And a genuine power of a variable exponent still has its log term.
    let answer = gradient("func g(x, y) = x^y", &[2.0, 3.0]).expect("finite");
    assert!((answer.gradient[0] - 12.0).abs() < 1e-12);
    assert!((answer.gradient[1] - 8.0 * 2f64.ln()).abs() < 1e-12);
}

#[test]
fn duplicate_parameter_names_are_refused_before_a_gradient_is_reported() {
    let source = "func f(x, x) = x*x";
    assert!(
        text::parse(source).is_err(),
        "duplicate bindings are ambiguous"
    );
    assert!(gradient(source, &[2.0, 9.0]).is_err());
    let answer = solve(r#"{"source":"func f(x, x) = x*x","at":[2,9]}"#);
    assert!(answer.contains("\"ok\":false"), "{answer}");
    assert!(answer.contains("catalyst.syntax"), "{answer}");
}

#[test]
fn malformed_json_never_silently_changes_the_coordinates() {
    for request in [
        r#"{"source":"func f(x)=x*x","at":[2,null]}"#,
        r#"{"source":"func f(x)=x*x","at":[false,2]}"#,
        r#"{"source":"func f(x)=x*x","at":["ignored",2]}"#,
        r#"{"source":"func f(x)=x*x","at":[2,]}"#,
        r#"{"source":"func f(x)=x*x","at":[+2]}"#,
        r#"{"source":"func f(x)=x*x","at":[02]}"#,
        r#"{"source":"func f(x)=x*x","at":[2.]}"#,
        r#"{"source":"func f(x)=x*x","at":[NaN]}"#,
        r#"{"source":"func f(x)=x*x","at":[1e999]}"#,
        r#"{"source" "func f(x)=x*x","at":[2]}"#,
        r#"{"source":"func f(x)=x*x","at":[2]"#,
        r#"{"source":"func f(x)=x*x","at":[2]} trailing"#,
        r#"[{"source":"func f(x)=x*x","at":[2]}]"#,
        r#"{"source":"func f(x)=x*x","source":"func f(x)=x","at":[2]}"#,
        r#"{"source":"func f(x)=x*x","at":[2],"at":[9]}"#,
        r#"{"source":"func f(x)=x*x #\q","at":[2]}"#,
        "{\"source\":\"func f(x)=x*x #\n\",\"at\":[2]}",
    ] {
        let answer = solve(request);
        assert!(answer.contains("\"ok\":false"), "{request} -> {answer}");
        assert!(answer.contains("catalyst.syntax"), "{request} -> {answer}");
    }
}

#[test]
fn valid_json_escapes_and_field_order_preserve_the_request() {
    let answer = solve(r#"{ "at": [2e0], "\u0073ource": "func f(x)=x*x # \uD83D\uDE80" }"#);
    assert!(answer.contains("\"ok\":true"), "{answer}");
    assert!(answer.contains("\"value\":4.0"), "{answer}");
    assert!(answer.contains("\"x\":4.0"), "{answer}");
}

fn central(source: &str, at: &[f64], wrt: usize) -> f64 {
    let h = 1e-5 * at[wrt].abs().max(1.0);
    let mut hi = at.to_vec();
    let mut lo = at.to_vec();
    hi[wrt] += h;
    lo[wrt] -= h;
    let a = gradient(source, &hi).expect("primal").value;
    let b = gradient(source, &lo).expect("primal").value;
    (a - b) / (2.0 * h)
}

fn close(got: f64, want: f64, what: &str) {
    let scale = got.abs().max(want.abs()).max(1.0);
    assert!(
        (got - want).abs() / scale < 1e-6,
        "{what}: got {got}, want {want}"
    );
}

/// Source in, value and gradient out, in one call and with no IR in sight.
#[test]
fn one_call_takes_source_and_returns_a_gradient() {
    let source = "func f(x, y) = x*y + sin(x)";
    let at = [0.7, 1.3];
    let answer = gradient(source, &at).expect("a straight-line function");

    close(answer.value, 0.7 * 1.3 + 0.7_f64.sin(), "value");
    close(answer.gradient[0], central(source, &at, 0), "df/dx");
    close(answer.gradient[1], central(source, &at, 1), "df/dy");
    assert_eq!(answer.names, vec!["x", "y"], "the names come back attached");
    assert!(
        answer.adjoint_insts > 0 && answer.primal_insts > 0,
        "the cost of the gradient is reported, not hidden: {answer:?}"
    );
}

/// A spread of expressions, each checked against a difference quotient. These
/// are the shapes a caller actually writes, so precedence and associativity are
/// under test here as much as the derivative rules are.
#[test]
fn the_grammar_means_what_a_reader_expects() {
    let cases: &[(&str, &[f64])] = &[
        ("func f(x) = x^2 + 3*x - 1", &[1.7]),
        ("func f(x) = -x^2", &[0.9]),
        ("func f(x, y) = (x + y) * (x - y)", &[1.3, 0.4]),
        ("func f(x) = exp(sin(x)) / (1 + x*x)", &[0.6]),
        ("func f(x, y) = max(x, y) + min(x, y)", &[1.5, 0.2]),
        ("func f(x, y) = atan2(x, y) * tanh(x)", &[1.1, 0.8]),
        ("func f(x) = sqrt(abs(x) + 2) * erf(x)", &[0.75]),
        ("func f(a, b, c) = a*b*c + a/b - log(c)", &[1.2, 2.3, 3.4]),
        ("func f(x) = 2.5e-1 * x + 1e2", &[3.0]),
        ("func f(x) = pow(x, 3) - recip(x)", &[1.4]),
    ];
    for (source, at) in cases {
        let answer = gradient(source, at).unwrap_or_else(|e| panic!("{source}: {e}"));
        for i in 0..at.len() {
            close(
                answer.gradient[i],
                central(source, at, i),
                &format!("{source} d/d{}", answer.names[i]),
            );
        }
    }
}

/// `^` associates to the right, so `2^3^2` is 512. Checked as a value rather
/// than trusted, because getting it wrong produces a number and not an error.
#[test]
fn exponentiation_associates_to_the_right() {
    let answer = gradient("func f(x) = x^3^2", &[2.0]).expect("parses");
    close(answer.value, 512.0, "2^(3^2)");
}

/// A parameter the body never mentions still gets a gradient, and it is zero.
/// An absent entry would make the caller guess whether it was zero or missing.
#[test]
fn an_unused_parameter_has_a_gradient_of_zero_rather_than_no_entry() {
    let answer = gradient("func f(x, unused) = x*x", &[3.0, 99.0]).expect("parses");
    close(answer.gradient[0], 6.0, "df/dx");
    assert_eq!(
        answer.gradient[1], 0.0,
        "an unused parameter is zero, not absent"
    );
    assert_eq!(answer.gradient.len(), 2, "every parameter is reported");
}

/// The JSON door: a process on the other end of a pipe gets a parseable object
/// whatever happens, including when what it sent was nonsense.
#[test]
fn the_json_door_answers_every_request_with_an_object() {
    let ok = solve(r#"{"source": "func f(x, y) = x*y + sin(x)", "at": [0.7, 1.3]}"#);
    assert!(ok.contains("\"ok\":true"), "{ok}");
    assert!(ok.contains("\"x\":"), "gradients are keyed by name: {ok}");
    assert!(ok.contains("\"y\":"), "{ok}");
    assert!(ok.contains("\"cost\""), "the cost is reported: {ok}");

    // Every one of these is a refusal, and every one is still an object with
    // the same outer shape, a stable code and a remedy.
    let bad = [
        (r#"{"at": [1.0]}"#, "catalyst.syntax"),
        (
            r#"{"source": "func f(x) = x +", "at": [1.0]}"#,
            "catalyst.syntax",
        ),
        (
            r#"{"source": "func f(x) = wobble(x)", "at": [1.0]}"#,
            "catalyst.unknown_name",
        ),
        (
            r#"{"source": "func f(x) = max(x)", "at": [1.0]}"#,
            "catalyst.wrong_arity",
        ),
        (
            r#"{"source": "func f(x, y) = x*y", "at": [1.0]}"#,
            "catalyst.wrong_arity",
        ),
        (
            r#"{"source": "func f(x) = x @ 2", "at": [1.0]}"#,
            "catalyst.syntax",
        ),
    ];
    for (request, code) in bad {
        let answer = solve(request);
        assert!(answer.contains("\"ok\":false"), "{request} -> {answer}");
        assert!(
            answer.contains(code),
            "{request} -> expected {code}, got {answer}"
        );
        assert!(
            answer.contains("\"remedy\""),
            "a refusal must say what to do: {answer}"
        );
        assert!(
            answer.starts_with('{') && answer.ends_with('}'),
            "not an object: {answer}"
        );
        assert_eq!(
            answer.chars().filter(|c| *c == '{').count(),
            answer.chars().filter(|c| *c == '}').count(),
            "unbalanced braces in {answer}"
        );
    }
}

/// A value JSON cannot carry must not produce a document JSON cannot parse.
/// `log(-1)` is NaN, and a bare `NaN` in the output would break the caller far
/// away from the thing that caused it.
#[test]
fn a_non_finite_result_stays_parseable() {
    let answer = solve(r#"{"source": "func f(x) = log(x)", "at": [-1.0]}"#);
    assert!(
        !answer.contains("NaN") && !answer.contains("inf"),
        "{answer}"
    );
    assert!(
        answer.contains("null"),
        "a non-finite number is null: {answer}"
    );
    assert_eq!(
        answer.chars().filter(|c| *c == '{').count(),
        answer.chars().filter(|c| *c == '}').count(),
        "{answer}"
    );
}

/// The list of names a caller may use is published, and it is the same list the
/// parser resolves against -- so a caller generating source from it cannot
/// generate a name the parser then rejects.
#[test]
fn the_published_names_are_the_names_that_parse() {
    let published = text::names();
    assert!(!published.is_empty());
    for (name, arity) in published {
        let args = match arity {
            1 => "x".to_string(),
            2 => "x, x".to_string(),
            n => panic!("{name} has arity {n}, which the test does not know how to call"),
        };
        let source = format!("func f(x) = {name}({args})");
        let result = gradient(&source, &[1.3]);
        assert!(
            result.is_ok(),
            "{name} is published but does not parse: {:?}",
            result.err().map(|e| e.to_string())
        );
    }
}

/// A refusal from the grammar and a refusal from the transform are the same
/// kind of thing, so one repair loop can handle both. This is the property that
/// makes the surface usable by a program rather than merely reachable by one.
#[test]
fn a_grammar_refusal_and_a_transform_refusal_have_the_same_shape() {
    let grammar = gradient("func f(x) = wobble(x)", &[1.0]).expect_err("unknown name");
    let arity = gradient("func f(x) = max(x)", &[1.0]).expect_err("wrong arity");

    for refusal in [&grammar, &arity] {
        assert!(refusal.code().starts_with("catalyst."));
        assert!(!refusal.cause.remedy().is_empty());
        let json = refusal.to_json();
        assert!(json.contains("\"code\""), "{json}");
        assert!(json.contains("\"detail\""), "{json}");
        assert!(json.contains("\"remedy\""), "{json}");
    }
    assert_ne!(
        grammar.code(),
        arity.code(),
        "two different mistakes must not share one code"
    );
}
