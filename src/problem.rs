//! `catalyst.problem.v1` and `catalyst.problem.v2`: the document a person or
//! a model writes. A v1 document carries an expression; a v2 document may
//! instead carry a `computation` of kind `catalyst-expression` or `llvm`.
//!
//! `docs/interface.md` §1 and §12 are the definition; this is the reader for
//! both versions, and every refusal it can produce is one of these stable
//! codes:
//!
//! | code | when |
//! | --- | --- |
//! | `catalyst.syntax` | not JSON, not an object, the wrong schema, a wrong type, a number that is not finite, an empty file |
//! | `catalyst.function_too_long` | the expression is over 65 536 bytes |
//! | `catalyst.problem_incomplete` | a parameter of the function has no input or no domain |
//! | `catalyst.unknown_name` | an input or a domain that is not a parameter, or a name in the body that is not one |
//! | `catalyst.input_out_of_domain` | an input outside the domain declared for it |
//! | `catalyst.computation_kind_unsupported` | a v2 computation of a kind other than `catalyst-expression` or `llvm` |
//! | `catalyst.portable_export_unavailable` | a Go or R export asked of an `llvm` computation |
//!
//! # What a refusal does not do
//!
//! It does not repeat the input back. A document that says `1e999` gets a
//! refusal saying a number was not finite and where, not one quoting the
//! literal: a refusal is read by a log, a terminal and sometimes a person's
//! screen, and none of them agreed to display whatever the document contained.
//!
//! # Unknown top-level keys are ignored
//!
//! Deliberately, and they are also never carried anywhere. A `private` or
//! `scratch` key in somebody's problem file is not part of the problem, so it
//! is not exported, not summarised, and above all not put in a prompt.

use crate::cli::Refused;
use crate::json::{obj, s, Json};
use crate::text;

/// The largest expression the language accepts, in bytes.
pub const MAX_FUNCTION_BYTES: usize = 65_536;

/// The declared range of one parameter.
#[derive(Clone, Debug, PartialEq)]
pub struct Domain {
    pub min: f64,
    pub max: f64,
    pub unit: String,
}

/// What a problem computes: `docs/interface.md` §12.8.
///
/// A v1 document has a `function` and says nothing else; a v2 document names
/// a `computation` of a kind. The expression kind behaves exactly as v1 and
/// differs only in that its answers say which lane produced them.
#[derive(Clone, Debug, PartialEq)]
pub enum Computation {
    /// A `catalyst.problem.v1` document: the expression, and no lane stated.
    Unstated,
    /// `catalyst.problem.v2`, kind `catalyst-expression`.
    Expression,
    /// `catalyst.problem.v2`, kind `llvm`: a module under the path rule, a
    /// `define` in it, and the integer parameters held fixed.
    Llvm {
        module: String,
        function: String,
        fixed: Vec<(String, f64)>,
    },
}

/// A validated problem: the function parses, every parameter has an input and
/// a domain, and every input lies inside its own domain.
///
/// For a computation of kind `llvm` the function is the name of a `define`
/// in the module rather than an expression, and the parameters are the
/// document's `inputs`: whether they are the module's `double` parameters is
/// settled when the module is read, by the Catalyst Gradient Compiler.
#[derive(Clone, Debug)]
pub struct Problem {
    pub name: String,
    pub goal: String,
    pub function: String,
    /// Parameter names in the order the function declares them. Everything
    /// else in this struct is in that same order, so no caller has to match
    /// names up again.
    pub params: Vec<String>,
    pub inputs: Vec<f64>,
    pub domains: Vec<Domain>,
    pub computation: Computation,
}

impl Problem {
    /// Whether the problem names a compiled program rather than an expression.
    pub fn is_llvm(&self) -> bool {
        matches!(self.computation, Computation::Llvm { .. })
    }

    /// The refusal every portable export gives an LLVM problem.
    pub fn portable_export_unavailable(&self) -> Option<Refused> {
        if !self.is_llvm() {
            return None;
        }
        Some(Refused::new(
            "catalyst.portable_export_unavailable",
            format!(
                "`{}` is a computation of kind llvm, and there is no Go or R export of an \
                 LLVM derivative in this phase",
                safe(&self.name)
            ),
            "evaluate it with `catalyst eval`, or write its derivative artifact with \
             `catalyst differentiate llvm`; a portable export needs a computation of kind \
             catalyst-expression",
        ))
    }
    pub fn domain_of(&self, name: &str) -> Option<&Domain> {
        self.params
            .iter()
            .position(|p| p == name)
            .map(|i| &self.domains[i])
    }

    /// The declared point: every parameter at the value the document gave it.
    pub fn at(&self) -> &[f64] {
        &self.inputs
    }

    /// The problem as a `catalyst.problem.v1` document again.
    ///
    /// # Why a validated problem is rebuilt rather than copied
    ///
    /// Because this is what leaves the process. A document that arrived with a
    /// `private` member, or with anything else this reader ignored, must not
    /// carry that member into a prompt, an answer or a file: §1 says unknown
    /// top-level keys are never forwarded anywhere, and the way to keep that
    /// promise is to build the outgoing document out of the six fields that
    /// were validated, rather than to edit the incoming one and hope the list
    /// of things to remove stays complete.
    pub fn to_json(&self) -> Json {
        let inputs = Json::Obj(
            self.params
                .iter()
                .cloned()
                .zip(self.inputs.iter().map(|v| Json::Num(*v)))
                .collect(),
        );
        let domains = Json::Obj(
            self.params
                .iter()
                .zip(self.domains.iter())
                .map(|(name, domain)| {
                    (
                        name.clone(),
                        obj(vec![
                            ("min", Json::Num(domain.min)),
                            ("max", Json::Num(domain.max)),
                            ("unit", s(&domain.unit)),
                        ]),
                    )
                })
                .collect(),
        );
        match &self.computation {
            Computation::Unstated => obj(vec![
                ("schema", s(SCHEMA)),
                ("name", s(&self.name)),
                ("goal", s(&self.goal)),
                ("function", s(&self.function)),
                ("inputs", inputs),
                ("domains", domains),
            ]),
            Computation::Expression => obj(vec![
                ("schema", s(SCHEMA_V2)),
                ("name", s(&self.name)),
                ("goal", s(&self.goal)),
                (
                    "computation",
                    obj(vec![
                        ("kind", s("catalyst-expression")),
                        ("function", s(&self.function)),
                    ]),
                ),
                ("inputs", inputs),
                ("domains", domains),
            ]),
            Computation::Llvm {
                module,
                function,
                fixed,
            } => {
                let mut computation = vec![
                    ("kind", s("llvm")),
                    ("module", s(module)),
                    ("function", s(function)),
                ];
                if !fixed.is_empty() {
                    computation.push((
                        "fixed",
                        Json::Obj(
                            fixed
                                .iter()
                                .map(|(name, value)| (name.clone(), Json::Num(*value)))
                                .collect(),
                        ),
                    ));
                }
                obj(vec![
                    ("schema", s(SCHEMA_V2)),
                    ("name", s(&self.name)),
                    ("goal", s(&self.goal)),
                    ("computation", obj(computation)),
                    ("inputs", inputs),
                    ("domains", domains),
                ])
            }
        }
    }
}

/// The schema every problem document declares.
pub const SCHEMA: &str = "catalyst.problem.v1";
/// The schema of a problem that names a computation, §12.8.
pub const SCHEMA_V2: &str = "catalyst.problem.v2";

fn syntax(detail: impl Into<String>) -> Refused {
    Refused::new(
        "catalyst.syntax",
        detail,
        "write a catalyst.problem.v1 object with string `name`, `goal` and `function`, \
         an `inputs` object of finite numbers, and a `domains` object of \
         {min, max, unit} with min below max; or a catalyst.problem.v2 object with a \
         `computation` of kind catalyst-expression or llvm in place of `function`",
    )
}

/// Read and validate one problem document.
pub fn parse(text_of_document: &str) -> Result<Problem, Refused> {
    if text_of_document.trim().is_empty() {
        return Err(syntax("the document is empty"));
    }
    let document = Json::parse(text_of_document)
        .map_err(|error| syntax(format!("at byte {}, expected {}", error.at, error.what)))?;
    if !document.is_obj() {
        return Err(syntax("the document is not a JSON object"));
    }

    let version = match document.get("schema").and_then(Json::as_str) {
        Some(SCHEMA) => 1,
        Some(SCHEMA_V2) => 2,
        Some(_) => {
            return Err(syntax(
                "the `schema` member is neither \"catalyst.problem.v1\" nor \
                 \"catalyst.problem.v2\"",
            ))
        }
        None => return Err(syntax("the document has no string `schema` member")),
    };

    let name = string_member(&document, "name")?;
    if !is_valid_name(&name) {
        return Err(syntax(
            "`name` must be a lower-case letter followed by at most 31 more \
             letters, digits, underscores or hyphens",
        ));
    }
    let goal = string_member(&document, "goal")?;

    // A v2 document says what it computes; the expression kind then reads
    // exactly as a v1 document with the lane made visible, and the llvm kind
    // is its own reader below.
    let (function, computation) = if version == 1 {
        (string_member(&document, "function")?, Computation::Unstated)
    } else {
        let computation = match document.get("computation") {
            Some(value) if value.is_obj() => value,
            Some(_) => return Err(syntax("the `computation` member is not an object")),
            None => {
                return Err(syntax(
                    "a catalyst.problem.v2 document has a `computation` member",
                ))
            }
        };
        match computation.get("kind").and_then(Json::as_str) {
            Some("catalyst-expression") => (
                string_member(computation, "function")?,
                Computation::Expression,
            ),
            Some("llvm") => return parse_llvm(&document, computation, name, goal),
            Some(other) => {
                return Err(Refused::new(
                    "catalyst.computation_kind_unsupported",
                    format!(
                        "a computation of kind `{}` is not built; this phase evaluates \
                         catalyst-expression and llvm",
                        safe(other)
                    ),
                    "compile the program to LLVM IR text yourself -- `rustc --emit=llvm-ir` \
                     or `clang -S -emit-llvm` -- and name the .ll file with kind llvm: \
                     {\"kind\":\"llvm\",\"module\":\"program.ll\",\"function\":\"name\"}",
                ))
            }
            None => return Err(syntax("the `computation` has no string `kind` member")),
        }
    };
    if function.len() > MAX_FUNCTION_BYTES {
        return Err(Refused::new(
            "catalyst.function_too_long",
            format!(
                "the expression is {} bytes, over the {MAX_FUNCTION_BYTES} byte limit",
                function.len()
            ),
            "shorten the expression, or split it into several problems",
        ));
    }

    // The function decides which names exist, so it is read before the two
    // tables are checked against it.
    let parsed = text::parse(&function).map_err(Refused::from)?;
    let (declared_inputs, declared_domains) = tables(&document)?;

    // Every parameter needs both entries...
    for param in &parsed.params {
        if !declared_inputs.iter().any(|(k, _)| k == param) {
            return Err(incomplete(param, "an input value"));
        }
        if !declared_domains.iter().any(|(k, _)| k == param) {
            return Err(incomplete(param, "a domain"));
        }
    }
    // ...and nothing else may have one.
    for (key, _) in declared_inputs.iter() {
        if !parsed.params.contains(key) {
            return Err(unknown_name(key, "an input"));
        }
    }
    for (key, _) in declared_domains.iter() {
        if !parsed.params.contains(key) {
            return Err(unknown_name(key, "a domain"));
        }
    }

    let (values, ranges) = in_order(&parsed.params, &declared_inputs, &declared_domains)?;
    Ok(Problem {
        name,
        goal,
        function,
        params: parsed.params,
        inputs: values,
        domains: ranges,
        computation,
    })
}

/// The `inputs` and `domains` tables of a document, read but not yet
/// matched against a parameter list.
type Tables = (Vec<(String, f64)>, Vec<(String, Domain)>);

fn tables(document: &Json) -> Result<Tables, Refused> {
    let inputs = object_member(document, "inputs")?;
    let domains = object_member(document, "domains")?;

    let mut declared_inputs: Vec<(String, f64)> = Vec::new();
    for (key, value) in inputs {
        let Some(number) = value.as_f64() else {
            return Err(syntax(format!(
                "the input `{}` is not a finite number",
                safe(key)
            )));
        };
        declared_inputs.push((key.clone(), number));
    }

    let mut declared_domains: Vec<(String, Domain)> = Vec::new();
    for (key, value) in domains {
        let min = finite_member(value, "min", key)?;
        let max = finite_member(value, "max", key)?;
        if min >= max {
            return Err(syntax(format!(
                "the domain of `{}` does not have min below max",
                safe(key)
            )));
        }
        let unit = match value.get("unit") {
            None => String::new(),
            Some(Json::Str(text)) => text.clone(),
            Some(_) => {
                return Err(syntax(format!(
                    "the `unit` of `{}` is not a string",
                    safe(key)
                )))
            }
        };
        declared_domains.push((key.clone(), Domain { min, max, unit }));
    }
    Ok((declared_inputs, declared_domains))
}

/// The values and domains in parameter order, each input checked against
/// its own domain.
fn in_order(
    params: &[String],
    declared_inputs: &[(String, f64)],
    declared_domains: &[(String, Domain)],
) -> Result<(Vec<f64>, Vec<Domain>), Refused> {
    let mut values = Vec::new();
    let mut ranges = Vec::new();
    for param in params {
        let value = declared_inputs
            .iter()
            .find(|(k, _)| k == param)
            .map(|(_, v)| *v)
            .unwrap_or_default();
        let domain = declared_domains
            .iter()
            .find(|(k, _)| k == param)
            .map(|(_, d)| d.clone())
            .unwrap_or(Domain {
                min: 0.0,
                max: 1.0,
                unit: String::new(),
            });
        if value < domain.min || value > domain.max {
            return Err(Refused::new(
                "catalyst.input_out_of_domain",
                format!(
                    "the input for `{}` is outside the domain declared for it",
                    safe(param)
                ),
                "move the input inside [min, max], or widen the domain to include it",
            ));
        }
        values.push(value);
        ranges.push(domain);
    }
    Ok((values, ranges))
}

/// A v2 document of kind `llvm`.
///
/// The parameters are the document's `inputs`, in the order written; whether
/// they are `double` parameters of the named function, and whether every
/// integer parameter is in `fixed`, is checked when the module is read.
/// What is checked here is what can be: every input has a domain and lies in
/// it, every domain has an input, and `fixed` holds finite whole numbers.
fn parse_llvm(
    document: &Json,
    computation: &Json,
    name: String,
    goal: String,
) -> Result<Problem, Refused> {
    let module = string_member(computation, "module")?;
    if module.is_empty() {
        return Err(syntax("the `module` of the computation is empty"));
    }
    let function = string_member(computation, "function")?;
    if function.is_empty() || function.len() > 256 {
        return Err(syntax(
            "the `function` of the computation names a define in the module",
        ));
    }
    let mut fixed = Vec::new();
    match computation.get("fixed") {
        None => {}
        Some(value) if value.is_obj() => {
            for (key, value) in value.as_obj().unwrap_or_default() {
                let Some(number) = value.as_f64().filter(|x| x.fract() == 0.0) else {
                    return Err(syntax(format!(
                        "the fixed value of `{}` is not a whole number",
                        safe(key)
                    )));
                };
                fixed.push((key.clone(), number));
            }
        }
        Some(_) => {
            return Err(syntax(
                "the `fixed` member of the computation is not an object",
            ))
        }
    }
    let (declared_inputs, declared_domains) = tables(document)?;
    let params: Vec<String> = declared_inputs.iter().map(|(k, _)| k.clone()).collect();
    for param in &params {
        if !declared_domains.iter().any(|(k, _)| k == param) {
            return Err(incomplete(param, "a domain"));
        }
        if fixed.iter().any(|(k, _)| k == param) {
            return Err(syntax(format!(
                "`{}` is both an input and a fixed value; a parameter is one or the other",
                safe(param)
            )));
        }
    }
    for (key, _) in &declared_domains {
        if !params.contains(key) {
            return Err(unknown_name(key, "a domain"));
        }
    }
    let (values, ranges) = in_order(&params, &declared_inputs, &declared_domains)?;
    Ok(Problem {
        name,
        goal,
        function: function.clone(),
        params,
        inputs: values,
        domains: ranges,
        computation: Computation::Llvm {
            module,
            function,
            fixed,
        },
    })
}

fn incomplete(param: &str, missing: &str) -> Refused {
    Refused::new(
        "catalyst.problem_incomplete",
        format!(
            "the parameter `{}` of the function has no {missing}",
            safe(param)
        ),
        "give every parameter of the function an entry in `inputs` and in `domains`",
    )
}

fn unknown_name(key: &str, what: &str) -> Refused {
    Refused::new(
        "catalyst.unknown_name",
        format!(
            "`{}` is {what} of the problem but not a parameter of the function",
            safe(key)
        ),
        "remove the entry, or declare that name as a parameter of the function",
    )
}

fn string_member(document: &Json, key: &str) -> Result<String, Refused> {
    match document.get(key) {
        Some(Json::Str(text)) => Ok(text.clone()),
        Some(_) => Err(syntax(format!("the `{key}` member is not a string"))),
        None => Err(syntax(format!("the document has no `{key}` member"))),
    }
}

fn object_member<'a>(document: &'a Json, key: &str) -> Result<&'a [(String, Json)], Refused> {
    match document.get(key) {
        Some(value) => value
            .as_obj()
            .ok_or_else(|| syntax(format!("the `{key}` member is not an object"))),
        None => Err(syntax(format!("the document has no `{key}` member"))),
    }
}

fn finite_member(domain: &Json, key: &str, of: &str) -> Result<f64, Refused> {
    match domain.get(key) {
        Some(value) => value.as_f64().ok_or_else(|| {
            syntax(format!(
                "the `{key}` of `{}` is not a finite number",
                safe(of)
            ))
        }),
        None => Err(syntax(format!(
            "the domain of `{}` has no `{key}`",
            safe(of)
        ))),
    }
}

/// A name from the document, cut to a length a message can carry and stripped
/// of anything that would move a terminal cursor. A refusal quotes names,
/// because naming the offending key is what makes it actionable; it quotes
/// them in a form that cannot do anything to whatever displays it.
fn safe(name: &str) -> String {
    let mut out: String = name.chars().filter(|c| !c.is_control()).take(32).collect();
    if name.chars().count() > 32 {
        out.push('…');
    }
    out
}

fn is_valid_name(name: &str) -> bool {
    let mut chars = name.chars();
    let Some(first) = chars.next() else {
        return false;
    };
    if !first.is_ascii_lowercase() {
        return false;
    }
    name.len() <= 32
        && chars.all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '_' || c == '-')
}

#[cfg(test)]
mod tests {
    use super::*;

    fn document(inputs: &str, domains: &str) -> String {
        format!(
            "{{\"schema\":\"catalyst.problem.v1\",\"name\":\"p\",\"goal\":\"g\",\
             \"function\":\"func p(x) = x\",\"inputs\":{inputs},\"domains\":{domains}}}"
        )
    }

    #[test]
    fn a_valid_problem_reads_in_declaration_order() {
        let text = "{\"schema\":\"catalyst.problem.v1\",\"name\":\"p\",\"goal\":\"g\",\
                    \"function\":\"func p(a, b) = a * b\",\"inputs\":{\"b\":2,\"a\":1},\
                    \"domains\":{\"b\":{\"min\":0,\"max\":3,\"unit\":\"\"},\
                    \"a\":{\"min\":0,\"max\":3,\"unit\":\"m\"}}}";
        let problem = parse(text).expect("valid");
        assert_eq!(problem.params, vec!["a", "b"]);
        assert_eq!(problem.inputs, vec![1.0, 2.0]);
        assert_eq!(problem.domains[0].unit, "m");
    }

    #[test]
    fn an_unknown_top_level_key_is_ignored() {
        let text = "{\"schema\":\"catalyst.problem.v1\",\"name\":\"p\",\"goal\":\"g\",\
                    \"private\":\"secret\",\"function\":\"func p(x) = x\",\
                    \"inputs\":{\"x\":1},\"domains\":{\"x\":{\"min\":0,\"max\":2,\"unit\":\"\"}}}";
        assert!(parse(text).is_ok());
    }

    #[test]
    fn a_hostile_literal_is_not_echoed_back() {
        let text = document(
            "{\"x\":1e999}",
            "{\"x\":{\"min\":0,\"max\":2,\"unit\":\"\"}}",
        );
        let refusal = parse(&text).expect_err("refused");
        assert_eq!(refusal.code, "catalyst.syntax");
        assert!(!refusal.detail.contains("1e999"), "{}", refusal.detail);
    }

    #[test]
    fn each_shape_of_mistake_has_its_own_code() {
        let cases = [
            (
                document("{\"x\":5}", "{\"x\":{\"min\":0,\"max\":2,\"unit\":\"\"}}"),
                "catalyst.input_out_of_domain",
            ),
            (
                document(
                    "{\"x\":1,\"z\":2}",
                    "{\"x\":{\"min\":0,\"max\":2,\"unit\":\"\"},\
                     \"z\":{\"min\":0,\"max\":2,\"unit\":\"\"}}",
                ),
                "catalyst.unknown_name",
            ),
            (
                document("{\"x\":1}", "{\"x\":{\"min\":2,\"max\":2,\"unit\":\"\"}}"),
                "catalyst.syntax",
            ),
        ];
        for (text, code) in cases {
            assert_eq!(parse(&text).expect_err("refused").code, code, "{text}");
        }
    }

    #[test]
    fn a_parameter_with_no_entry_is_incomplete_and_is_named() {
        let text = "{\"schema\":\"catalyst.problem.v1\",\"name\":\"p\",\"goal\":\"g\",\
                    \"function\":\"func p(x, y) = x * y\",\"inputs\":{\"x\":1},\
                    \"domains\":{\"x\":{\"min\":0,\"max\":2,\"unit\":\"\"}}}";
        let refusal = parse(text).expect_err("refused");
        assert_eq!(refusal.code, "catalyst.problem_incomplete");
        assert!(refusal.detail.contains('y'), "{}", refusal.detail);
    }

    #[test]
    fn an_over_long_expression_is_refused_before_it_is_parsed() {
        let body = format!("x{}", " + x".repeat(20_000));
        let text = format!(
            "{{\"schema\":\"catalyst.problem.v1\",\"name\":\"p\",\"goal\":\"g\",\
             \"function\":\"func p(x) = {body}\",\"inputs\":{{\"x\":1}},\
             \"domains\":{{\"x\":{{\"min\":0,\"max\":2,\"unit\":\"\"}}}}}}"
        );
        assert_eq!(
            parse(&text).expect_err("refused").code,
            "catalyst.function_too_long"
        );
    }
}
