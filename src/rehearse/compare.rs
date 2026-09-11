//! From a failure to a scenario, from two targets to a comparison, and from a
//! comparison to a report somebody can act on.
//!
//! `docs/interface.md` §8.
//!
//! # Why a failure becomes a scenario
//!
//! A result says ninety-six requests were issued and fourteen failed. That is
//! evidence, but it is not something anybody can re-run cheaply, and it is not
//! something a repair can be tested against. [`reduce`] takes the failure the
//! result is *about* -- its signature -- and writes the smallest traffic that
//! produces it: the one request, at the concurrency that produced it. Because
//! it is small it can be replayed against a proposed repair in a second, and
//! because it is kept under the rehearsal directory it stays around after the
//! run that found it.
//!
//! # Why held-out scenarios exist
//!
//! A repair that fixes the regression and breaks everything else is not a
//! repair. The held-out set is traffic the repair was *not* tuned against, and
//! it is replayed against both targets so that "it is better" is a comparison
//! rather than a hope. Everything in it is generated from a seed here, so it
//! is the same set on every machine.

use crate::cli::{Args, Refused};
use crate::json::{obj, s, Json};
use crate::paths;
use crate::rehearse::config::{self, Rehearsal};
use crate::rehearse::digest;
use crate::rehearse::replay::{self, Drive, Step};
use crate::rehearse::target::{self, Target};
use crate::rehearse::trace::{write_file, Random};

/// The schema of a held-out set.
pub const SCENARIOS_SCHEMA: &str = "catalyst.scenarios.v1";
/// The schema of a comparison.
pub const COMPARE_SCHEMA: &str = "catalyst.rehearsal-compare.v1";

/// `catalyst rehearse reduce --result FILE --dir R --out FILE`.
pub fn reduce(args: &Args) -> Result<String, Refused> {
    let result_named = args.required("result", "rehearse reduce")?;
    let dir_named = args.required("dir", "rehearse reduce")?;
    let out_named = args.required("out", "rehearse reduce")?;
    let out = paths::resolve(out_named)?;
    let rehearsal = config::read(dir_named)?;
    let result = read_document(result_named)?;

    let signature = result
        .get("signature")
        .and_then(Json::as_str)
        .filter(|text| !text.is_empty())
        .ok_or_else(|| {
            Refused::new(
                "catalyst.rehearsal.nothing_to_reduce",
                "that result records no failure, so there is nothing to reduce",
                "replay under a disturbance that produces a failure first, for example \
                 `catalyst rehearse replay --dir DIR --patterns FILE --disturb burst --out FILE`",
            )
        })?
        .to_owned();
    let failure = result
        .get("failures")
        .and_then(Json::as_arr)
        .and_then(<[Json]>::first)
        .ok_or_else(|| {
            Refused::new(
                "catalyst.rehearsal.nothing_to_reduce",
                "that result names a failure signature but lists no failure to reduce",
                "replay again and reduce the result that run writes",
            )
        })?;

    let method = failure
        .get("method")
        .and_then(Json::as_str)
        .unwrap_or("GET")
        .to_owned();
    let path = failure
        .get("path")
        .and_then(Json::as_str)
        .unwrap_or("/health")
        .to_owned();
    let status = failure.get("status").and_then(Json::as_f64).unwrap_or(0.0);
    // The concurrency that produced it, as measured, is what reproduces it.
    let concurrency = result
        .path_number("disturbance.applied.concurrency_peak")
        .filter(|n| *n >= 1.0)
        .unwrap_or(1.0);

    // The disturbance is part of the failure, not part of the run that found
    // it. A scenario that recorded the one request but not the timeout that
    // made it fail would replay green against the unrepaired target, which is
    // the one thing a regression scenario must never do -- and it is not a
    // corner case: two of the four disturbances are failures *of the
    // disturbance* rather than of the traffic, so before this they could
    // never be shown fixed at all.
    //
    // `applied` goes in beside `kind` as the record of what actually reached
    // the target. Replay re-derives the values it applies from the rehearsal
    // declared today, so the bound rule holds by construction even if the
    // configuration has moved since; `applied` says what produced the failure
    // that is being reduced.
    let kind = result
        .path_string("disturbance.kind")
        .unwrap_or_else(|| "none".to_owned());
    let applied = result
        .path("disturbance.applied")
        .cloned()
        .unwrap_or_else(|| Json::Obj(Vec::new()));

    let scenario = obj(vec![
        ("schema", s(replay::SCENARIO_SCHEMA)),
        ("signature", s(&signature)),
        ("concurrency", Json::Num(concurrency)),
        (
            "disturbance",
            obj(vec![("kind", s(&kind)), ("applied", applied)]),
        ),
        (
            "steps",
            Json::Arr(vec![obj(vec![
                ("method", s(&method)),
                ("path", s(&path)),
                (
                    "body",
                    match method.as_str() {
                        "POST" => s("{\"item\":\"stand-in\",\"qty\":1}"),
                        _ => Json::Null,
                    },
                ),
            ])]),
        ),
        ("expected", obj(vec![("status", Json::Num(status))])),
    ]);

    let text = scenario.render_pretty();
    write_file(&out.full, out_named, &text)?;
    // Kept under the rehearsal directory, named after the failure rather than
    // after the moment: a name with a time in it would make two runs of the
    // same rehearsal differ for no reason anybody cares about.
    let name = format!(
        "regression-{}.json",
        &digest::hex(signature.as_bytes())[..12]
    );
    let kept = format!("{dir_named}/regressions/{name}");
    write_file(
        &rehearsal.full.join("regressions").join(&name),
        &kept,
        &text,
    )?;

    Ok(obj(vec![
        ("ok", Json::Bool(true)),
        ("out", s(out_named)),
        ("kept", s(&kept)),
        ("signature", s(&signature)),
        ("concurrency", Json::Num(concurrency)),
    ])
    .render())
}

/// `catalyst rehearse held-out --out FILE [--seed S]`.
pub fn held_out(args: &Args) -> Result<String, Refused> {
    let out_named = args.required("out", "rehearse held-out")?;
    let out = paths::resolve(out_named)?;
    let seed = crate::rehearse::trace::number(args, "seed", 1)? as u64;
    let mut random = Random::from(seed);

    // Traffic no repair was tuned against: a bare health check, a whole
    // order-and-pay sequence, and a short repeat of one. All at concurrencies
    // a correctly sized queue serves without complaint, so a failure here is
    // the repair's and not the load's.
    let scenarios = vec![
        scenario(
            "health",
            1.0,
            &[("GET", "/health")],
            200.0,
            "one request at a time against the health endpoint",
        ),
        scenario(
            "order-and-pay",
            2.0,
            &[
                ("POST", "/orders"),
                ("GET", "/orders/{id}"),
                ("POST", "/orders/{id}/pay"),
            ],
            200.0,
            "two sessions placing, reading and paying for an order",
        ),
        scenario(
            "repeat-orders",
            1.0 + (random.below(2) as f64),
            &[("POST", "/orders"), ("POST", "/orders")],
            201.0,
            "orders placed back to back within one session",
        ),
    ];

    let document = obj(vec![
        ("schema", s(SCENARIOS_SCHEMA)),
        ("seed", Json::Num(seed as f64)),
        ("scenarios", Json::Arr(scenarios)),
    ]);
    write_file(&out.full, out_named, &document.render_pretty())?;
    Ok(obj(vec![
        ("ok", Json::Bool(true)),
        ("out", s(out_named)),
        ("scenarios", Json::Num(3.0)),
    ])
    .render())
}

fn scenario(
    name: &str,
    concurrency: f64,
    steps: &[(&str, &str)],
    expected: f64,
    note: &str,
) -> Json {
    obj(vec![
        ("schema", s(replay::SCENARIO_SCHEMA)),
        ("name", s(name)),
        ("note", s(note)),
        ("signature", s(&format!("{name} -> {expected}"))),
        ("concurrency", Json::Num(concurrency)),
        (
            "steps",
            Json::Arr(
                steps
                    .iter()
                    .map(|(method, path)| {
                        obj(vec![
                            ("method", s(method)),
                            ("path", s(path)),
                            (
                                "body",
                                match *method {
                                    "POST" => s("{\"item\":\"stand-in\",\"qty\":1}"),
                                    _ => Json::Null,
                                },
                            ),
                        ])
                    })
                    .collect(),
            ),
        ),
        ("expected", obj(vec![("status", Json::Num(expected))])),
    ])
}

/// `catalyst rehearse compare …`.
pub fn compare(args: &Args) -> Result<String, Refused> {
    let dir_named = args.required("dir", "rehearse compare")?;
    let out_named = args.required("out", "rehearse compare")?;
    let scenario_named = args.required("scenario", "rehearse compare")?;
    let held_named = args.required("held-out", "rehearse compare")?;
    let out = paths::resolve(out_named)?;

    let rehearsal = config::read(dir_named)?;
    // Both targets are validated before either is opened.
    let baseline = target::parse(args.required("baseline", "rehearse compare")?)?;
    let candidate = target::parse(args.required("candidate", "rehearse compare")?)?;

    let regression = read_document(scenario_named)?;
    let held = read_document(held_named)?;
    let held_scenarios: Vec<Json> = held
        .get("scenarios")
        .and_then(Json::as_arr)
        .map(<[Json]>::to_vec)
        .unwrap_or_default();
    if held_scenarios.is_empty() {
        return Err(Refused::new(
            "catalyst.syntax",
            "the held-out document carries no `scenarios`",
            "write one with `catalyst rehearse held-out --out FILE`",
        ));
    }

    let before = side(&baseline, &rehearsal, &regression, &held_scenarios)?;
    let after = side(&candidate, &rehearsal, &regression, &held_scenarios)?;

    let passed = |side: &Json| -> f64 {
        side.get("held_out")
            .and_then(Json::as_arr)
            .map(|runs| {
                runs.iter()
                    .filter(|run| run.get("verdict").and_then(Json::as_str) == Some("pass"))
                    .count() as f64
            })
            .unwrap_or(0.0)
    };
    let regression_fixed = before.path_string("regression.verdict") == Some("fail".to_owned())
        && after.path_string("regression.verdict") == Some("pass".to_owned());

    let document = obj(vec![
        ("schema", s(COMPARE_SCHEMA)),
        ("configuration_digest", s(&rehearsal.configuration_digest)),
        ("criteria", rehearsal.criteria.clone()),
        ("baseline", before.clone()),
        ("candidate", after.clone()),
        (
            "improvement",
            obj(vec![
                ("regression_fixed", Json::Bool(regression_fixed)),
                ("held_out_passed_before", Json::Num(passed(&before))),
                ("held_out_passed_after", Json::Num(passed(&after))),
                ("held_out_total", Json::Num(held_scenarios.len() as f64)),
            ]),
        ),
    ]);
    write_file(&out.full, out_named, &document.render_pretty())?;
    Ok(obj(vec![
        ("ok", Json::Bool(true)),
        ("out", s(out_named)),
        ("regression_fixed", Json::Bool(regression_fixed)),
        ("held_out_passed_after", Json::Num(passed(&after))),
    ])
    .render())
}

/// One target's side of a comparison: the regression, then every held-out
/// scenario, all under the criteria the rehearsal already recorded.
fn side(
    target: &Target,
    rehearsal: &Rehearsal,
    regression: &Json,
    held: &[Json],
) -> Result<Json, Refused> {
    let regression_result = run(target, rehearsal, regression)?;
    let mut runs = Vec::new();
    for scenario in held {
        let mut result = run(target, rehearsal, scenario)?;
        if let (Json::Obj(pairs), Some(name)) =
            (&mut result, scenario.get("name").and_then(Json::as_str))
        {
            pairs.insert(0, ("name".to_owned(), s(name)));
        }
        runs.push(result);
    }
    Ok(obj(vec![
        ("target", s(&target.written())),
        ("regression", regression_result),
        ("held_out", Json::Arr(runs)),
    ]))
}

fn run(target: &Target, rehearsal: &Rehearsal, scenario: &Json) -> Result<Json, Refused> {
    let steps = scenario
        .get("steps")
        .and_then(Json::as_arr)
        .filter(|items| !items.is_empty())
        .ok_or_else(|| {
            Refused::new(
                "catalyst.syntax",
                "a scenario in this comparison carries no `steps`",
                "every scenario is {\"steps\":[{\"method\",\"path\",\"body\"}],\"concurrency\":N}",
            )
        })?;
    let concurrency = scenario
        .get("concurrency")
        .and_then(Json::as_f64)
        .filter(|n| n.is_finite() && *n >= 1.0)
        .unwrap_or(1.0)
        .min(64.0) as usize;
    let mut runs = Vec::new();
    let mut seq = 0usize;
    for _ in 0..concurrency {
        let mut one = Vec::new();
        for step in steps {
            let method = step
                .get("method")
                .and_then(Json::as_str)
                .unwrap_or("GET")
                .to_owned();
            let path = step
                .get("path")
                .and_then(Json::as_str)
                .filter(|path| path.starts_with('/'))
                .unwrap_or("/health")
                .to_owned();
            seq += 1;
            one.push(Step {
                body: step
                    .get("body")
                    .and_then(Json::as_str)
                    .map(str::to_owned)
                    .or_else(|| {
                        (method == "POST").then(|| "{\"item\":\"stand-in\",\"qty\":1}".to_owned())
                    }),
                method,
                path,
                seq,
            });
        }
        runs.push(one);
    }
    Ok(replay::execute(
        target,
        rehearsal,
        &Drive {
            kind: "none".to_owned(),
            runs,
            concurrency,
            delay_ms: 0.0,
            timeout_ms: 5000.0,
            malformed_fraction: 0.0,
        },
    ))
}

/// `catalyst rehearse report --compare FILE --out FILE`.
pub fn report(args: &Args) -> Result<String, Refused> {
    let compare_named = args.required("compare", "rehearse report")?;
    let out_named = args.required("out", "rehearse report")?;
    let out = paths::resolve(out_named)?;
    let comparison = read_document(compare_named)?;

    let digest = comparison
        .get("configuration_digest")
        .and_then(Json::as_str)
        .unwrap_or("");
    let fixed = comparison
        .path_bool("improvement.regression_fixed")
        .unwrap_or(false);
    let before = comparison
        .path_number("improvement.held_out_passed_before")
        .unwrap_or(0.0);
    let after = comparison
        .path_number("improvement.held_out_passed_after")
        .unwrap_or(0.0);
    let total = comparison
        .path_number("improvement.held_out_total")
        .unwrap_or(0.0);
    let signature = comparison
        .path_string("baseline.regression.signature")
        .unwrap_or_default();

    let mut text = String::new();
    text.push_str("# Failure rehearsal: baseline against candidate\n\n");
    text.push_str(&format!(
        "Both targets are local stand-ins driven by `catalyst rehearse`. Every run below was \
         judged by the criteria recorded in configuration `{digest}`, unchanged between the \
         two.\n\n"
    ));

    text.push_str("## Measured improvement\n\n");
    text.push_str(&format!(
        "The reduced failure `{}` was replayed against both targets.\n\n",
        if signature.is_empty() {
            "(no signature recorded)".to_owned()
        } else {
            signature.clone()
        }
    ));
    for which in ["baseline", "candidate"] {
        text.push_str(&format!("- {which}: {}\n", one_line(&comparison, which)));
    }
    text.push_str(&format!(
        "- the reduced failure is reproduced on the baseline and no longer reproduced on the \
         candidate: {}\n",
        if fixed { "yes" } else { "no" }
    ));
    text.push_str(&format!(
        "- held-out scenarios meeting the criteria: {before} of {total} before, {after} of \
         {total} after\n\n"
    ));

    text.push_str("## Remaining failures\n\n");
    let remaining = failing(&comparison, "candidate");
    if remaining.is_empty() {
        text.push_str(
            "No scenario replayed here failed against the candidate. That covers the reduced \
             failure and the held-out set, and nothing else was replayed.\n\n",
        );
    } else {
        for line in &remaining {
            text.push_str(&format!("- {line}\n"));
        }
        text.push('\n');
    }

    text.push_str("## Remaining uncertainty\n\n");
    text.push_str(
        "This is a rehearsal against a stand-in service over a local socket. It measures the \
         sequences that were replayed, at the concurrency that was applied, under one \
         disturbance at a time. What was not replayed was not measured: other traffic shapes, \
         other concurrencies, several disturbances at once, and everything about the real \
         dependencies a deployment would have.\n\n",
    );
    text.push_str(
        "A passing rehearsal is therefore evidence that a particular failure no longer \
         reproduces under a particular load, and it is not a promise about how a deployment \
         behaves. Treat the numbers above as what they are: measurements of these runs.\n",
    );

    write_file(&out.full, out_named, &text)?;
    Ok(obj(vec![
        ("ok", Json::Bool(true)),
        ("out", s(out_named)),
        ("regression_fixed", Json::Bool(fixed)),
        ("remaining_failures", Json::Num(remaining.len() as f64)),
    ])
    .render())
}

fn one_line(comparison: &Json, which: &str) -> String {
    let verdict = comparison
        .path_string(&format!("{which}.regression.verdict"))
        .unwrap_or_else(|| "not run".to_owned());
    let requests = comparison
        .path_number(&format!("{which}.regression.requests"))
        .unwrap_or(0.0);
    let errors = comparison
        .path_number(&format!("{which}.regression.errors"))
        .unwrap_or(0.0);
    let rate = comparison
        .path_number(&format!("{which}.regression.error_rate"))
        .unwrap_or(0.0);
    let p99 = comparison
        .path_number(&format!("{which}.regression.p99_ms"))
        .unwrap_or(0.0);
    format!(
        "verdict {verdict}, {requests} requests, {errors} of them errors, error rate {rate}, \
         99th percentile {p99} ms"
    )
}

fn failing(comparison: &Json, which: &str) -> Vec<String> {
    let mut lines = Vec::new();
    if comparison.path_string(&format!("{which}.regression.verdict")) == Some("fail".to_owned()) {
        lines.push(format!(
            "the reduced failure still reproduces: {}",
            comparison
                .path_string(&format!("{which}.regression.signature"))
                .unwrap_or_default()
        ));
    }
    if let Some(runs) = comparison
        .get(which)
        .and_then(|side| side.get("held_out"))
        .and_then(Json::as_arr)
    {
        for run in runs {
            if run.get("verdict").and_then(Json::as_str) == Some("fail") {
                lines.push(format!(
                    "held-out scenario `{}` did not meet the criteria: {}",
                    run.get("name").and_then(Json::as_str).unwrap_or("unnamed"),
                    run.get("signature").and_then(Json::as_str).unwrap_or("")
                ));
            }
        }
    }
    lines
}

fn read_document(named: &str) -> Result<Json, Refused> {
    let text = paths::read_input(named)?;
    Json::parse(&text).map_err(|error| {
        Refused::new(
            "catalyst.syntax",
            format!(
                "`{named}` is not one JSON object (at byte {}, expected {})",
                error.at, error.what
            ),
            "name a document `catalyst rehearse` wrote",
        )
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn comparison() -> Json {
        obj(vec![
            ("configuration_digest", s("abc123")),
            (
                "baseline",
                obj(vec![
                    (
                        "regression",
                        obj(vec![
                            ("verdict", s("fail")),
                            ("requests", Json::Num(16.0)),
                            ("errors", Json::Num(14.0)),
                            ("error_rate", Json::Num(0.875)),
                            ("p99_ms", Json::Num(31.0)),
                            ("signature", s("POST /orders -> 503")),
                        ]),
                    ),
                    (
                        "held_out",
                        Json::Arr(vec![obj(vec![("verdict", s("pass"))])]),
                    ),
                ]),
            ),
            (
                "candidate",
                obj(vec![
                    (
                        "regression",
                        obj(vec![
                            ("verdict", s("pass")),
                            ("requests", Json::Num(16.0)),
                            ("errors", Json::Num(0.0)),
                            ("error_rate", Json::Num(0.0)),
                            ("p99_ms", Json::Num(33.0)),
                            ("signature", s("")),
                        ]),
                    ),
                    (
                        "held_out",
                        Json::Arr(vec![obj(vec![("verdict", s("pass"))])]),
                    ),
                ]),
            ),
            (
                "improvement",
                obj(vec![
                    ("regression_fixed", Json::Bool(true)),
                    ("held_out_passed_before", Json::Num(1.0)),
                    ("held_out_passed_after", Json::Num(1.0)),
                    ("held_out_total", Json::Num(1.0)),
                ]),
            ),
        ])
    }

    /// The report has to be usable and honest at once: the sections a reader
    /// looks for, real numbers in them, and none of the phrases that would
    /// turn a measurement of two runs into a promise about a deployment.
    #[test]
    fn the_report_states_numbers_and_promises_nothing() {
        let comparison = comparison();
        let mut text = String::new();
        text.push_str(&one_line(&comparison, "baseline"));
        text.push_str(&one_line(&comparison, "candidate"));
        assert!(text.contains("0.875"));
        assert!(text.contains("16 requests"));
        let lower = text.to_ascii_lowercase();
        for phrase in [
            "failure-free",
            "guarantee",
            "will never fail",
            "will not fail",
            "cannot fail",
            "zero failures",
        ] {
            assert!(!lower.contains(phrase), "`{phrase}` in: {text}");
        }
    }

    #[test]
    fn a_candidate_that_still_fails_is_listed_rather_than_summarised_away() {
        assert!(failing(&comparison(), "candidate").is_empty());
        assert_eq!(failing(&comparison(), "baseline").len(), 1);
    }
}
