//! Custom derivative rules: `catalyst.derivative-rules.v1`.
//!
//! `docs/interface.md` §12.5. A rule names a function and the function that
//! computes its derivative, both `define`d in the module and both of the one
//! shape a rule covers: one `double` in, one `double` out. This file reads the
//! document and checks the shape against the module; what the rule *claims*
//! is not checked here, or anywhere else in the front end. The verifier runs
//! the real function, and a wrong rule is a wrong derivative like any other.

use super::llvm::{self, Type};
use super::lower::RuleBinding;
use crate::cli::Refused;
use crate::json::Json;

pub const SCHEMA: &str = "catalyst.derivative-rules.v1";

fn invalid(detail: impl Into<String>) -> Refused {
    Refused::new(
        "catalyst.rule_invalid",
        detail,
        "write {\"schema\":\"catalyst.derivative-rules.v1\",\"rules\":[{\"function\":NAME,\
         \"derivative\":NAME}]}, naming two functions defined in the module that each take \
         one double and return double",
    )
}

/// Read the rules document.
pub fn parse(text: &str) -> Result<Vec<RuleBinding>, Refused> {
    let document = Json::parse(text).map_err(|error| {
        Refused::new(
            "catalyst.syntax",
            format!(
                "the rules document is not JSON: at byte {}, expected {}",
                error.at, error.what
            ),
            "write the rules as one JSON object of schema catalyst.derivative-rules.v1",
        )
    })?;
    if !document.is_obj() {
        return Err(invalid("the rules document is not a JSON object"));
    }
    match document.get("schema").and_then(Json::as_str) {
        Some(SCHEMA) => {}
        Some(_) => return Err(invalid(format!("the `schema` member is not \"{SCHEMA}\""))),
        None => return Err(invalid("the rules document has no string `schema` member")),
    }
    let Some(rules) = document.get("rules").and_then(Json::as_arr) else {
        return Err(invalid("the rules document has no `rules` array"));
    };
    let mut out = Vec::new();
    for (i, rule) in rules.iter().enumerate() {
        let name = |key: &str| -> Result<String, Refused> {
            rule.get(key)
                .and_then(Json::as_str)
                .filter(|s| !s.is_empty())
                .map(|s| s.trim_start_matches('@').to_owned())
                .ok_or_else(|| invalid(format!("rule {i} has no string `{key}` member")))
        };
        let function = name("function")?;
        let derivative = name("derivative")?;
        if out.iter().any(|r: &RuleBinding| r.function == function) {
            return Err(invalid(format!("`{function}` is given more than one rule")));
        }
        out.push(RuleBinding {
            function,
            derivative,
        });
    }
    Ok(out)
}

/// Check every rule against the module: both names defined, both of the
/// shape a rule covers.
pub fn check(rules: &[RuleBinding], module: &llvm::Module) -> Result<(), Refused> {
    for rule in rules {
        for (role, name) in [
            ("function", &rule.function),
            ("derivative", &rule.derivative),
        ] {
            let Some(def) = module.define(name) else {
                return Err(invalid(format!(
                    "the {role} `{name}` of the rule for `{}` is not defined in the module",
                    rule.function
                )));
            };
            let one_double = def.params.len() == 1 && def.params[0].ty == Type::Double;
            if !one_double || def.ret != Type::Double {
                return Err(invalid(format!(
                    "the {role} `{name}` of the rule for `{}` is not a function of one double \
                     returning double",
                    rule.function
                )));
            }
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    const MODULE: &str = "define double @f(double %x) {\nstart:\n  ret double %x\n}\n\
                          define double @g(double %x) {\nstart:\n  ret double 1.000000e+00\n}\n\
                          define double @two(double %x, double %y) {\nstart:\n  ret double %x\n}\n";

    #[test]
    fn a_rule_reads_and_checks_against_the_module() {
        let rules = parse(
            "{\"schema\":\"catalyst.derivative-rules.v1\",\"rules\":[{\"function\":\"f\",\"derivative\":\"g\"}]}",
        )
        .expect("reads");
        assert_eq!(rules.len(), 1);
        let module = llvm::parse(MODULE).expect("reads");
        check(&rules, &module).expect("both defined, both the right shape");
    }

    #[test]
    fn a_missing_or_misshapen_name_is_refused_by_name() {
        let module = llvm::parse(MODULE).expect("reads");
        let missing = parse(
            "{\"schema\":\"catalyst.derivative-rules.v1\",\"rules\":[{\"function\":\"f\",\"derivative\":\"nowhere\"}]}",
        )
        .expect("reads");
        let refused = check(&missing, &module).expect_err("refused");
        assert_eq!(refused.code, "catalyst.rule_invalid");
        assert!(refused.detail.contains("nowhere"));
        let wide = parse(
            "{\"schema\":\"catalyst.derivative-rules.v1\",\"rules\":[{\"function\":\"two\",\"derivative\":\"g\"}]}",
        )
        .expect("reads");
        assert!(check(&wide, &module)
            .expect_err("refused")
            .detail
            .contains("two"));
    }

    #[test]
    fn the_document_shape_is_held() {
        assert_eq!(
            parse("not json").expect_err("refused").code,
            "catalyst.syntax"
        );
        assert_eq!(
            parse("{\"schema\":\"other\",\"rules\":[]}")
                .expect_err("refused")
                .code,
            "catalyst.rule_invalid"
        );
        assert_eq!(
            parse("{\"schema\":\"catalyst.derivative-rules.v1\",\"rules\":[{\"function\":\"f\"}]}")
                .expect_err("refused")
                .code,
            "catalyst.rule_invalid"
        );
    }
}
