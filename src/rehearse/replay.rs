//! Driving the traffic and recording what came back.
//!
//! `docs/interface.md` §8. A replay takes the *shape* of some traffic -- from
//! imported patterns or from a reduced scenario -- and issues it at a stand-in
//! under one declared disturbance, then writes down what happened and whether
//! the declared criteria were met.
//!
//! # A disturbance never exceeds what was declared
//!
//! Each disturbance carries a value and a bound, and
//! [`super::config::Rehearsal::within`] holds the value inside the bound
//! before anything is issued. The result records both the declared block and
//! what was actually applied, including the concurrency the client really
//! reached, so a reader can check the claim rather than take it: a rehearsal
//! that quietly pushed harder than it said would be measuring a different
//! service from the one that was agreed.
//!
//! # What counts as an error
//!
//! A status of 500 or more, a transport failure, or a timeout. A 4xx is not an
//! error: a stand-in answering `400` to a deliberately malformed body is the
//! service behaving correctly, and counting it as a failure would make the
//! `malformed_input` disturbance fail by definition and measure nothing.

use crate::cli::{Args, Refused};
use crate::json::{obj, s, Json};
use crate::paths;
use crate::rehearse::config::{self, Rehearsal};
use crate::rehearse::target::Target;
use crate::rehearse::trace::{self, write_file};
use crate::rehearse::wire;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Barrier;
use std::time::Duration;

/// The schema of what a replay writes.
pub const RESULT_SCHEMA: &str = "catalyst.rehearsal-result.v1";
/// The schema of a reduced scenario.
pub const SCENARIO_SCHEMA: &str = "catalyst.scenario.v1";

/// The disturbances a replay understands.
pub const KINDS: &[&str] = &[
    "none",
    "burst",
    "slow_dependency",
    "timeout",
    "malformed_input",
];

/// One request to issue.
#[derive(Clone, Debug)]
pub struct Step {
    pub method: String,
    pub path: String,
    pub body: Option<String>,
    /// The place this request has in the whole replay, so a failure can be
    /// pointed at.
    pub seq: usize,
}

/// What one request produced.
struct Record {
    seq: usize,
    method: String,
    path: String,
    status: u16,
    elapsed_ms: f64,
    error: bool,
}

/// Everything a replay needs once the flags have been read.
pub struct Drive {
    pub kind: String,
    /// One inner list per session or per parallel copy; each is issued in
    /// order by one worker, so a session's own sequence is never interleaved.
    pub runs: Vec<Vec<Step>>,
    pub concurrency: usize,
    pub delay_ms: f64,
    pub timeout_ms: f64,
    pub malformed_fraction: f64,
}

/// `catalyst rehearse replay …`.
pub fn replay(args: &Args) -> Result<String, Refused> {
    let dir_named = args.required("dir", "rehearse replay")?;
    let out_named = args.required("out", "rehearse replay")?;
    let out = paths::resolve(out_named)?;
    // The configuration is read -- and its target re-validated -- before any
    // socket is opened and before the output file is touched.
    let rehearsal = config::read(dir_named)?;

    let scenario = match args.value("scenario") {
        Some(named) => Some(read_document(named)?),
        None => None,
    };
    // A reduced scenario carries the disturbance that produced the failure,
    // and replaying it without that disturbance would replay different
    // traffic: the point of a regression scenario is that it reproduces, and
    // a 700 ms service only fails under the timeout that timed it out. An
    // explicit `--disturb` still wins, because somebody asking whether the
    // same traffic survives a *different* disturbance is asking a fair
    // question.
    let kind = match args.value("disturb") {
        Some(given) => given.to_owned(),
        None => scenario
            .as_ref()
            .and_then(|doc| doc.path_string("disturbance.kind"))
            .unwrap_or_else(|| "none".to_owned()),
    };
    if !KINDS.contains(&kind.as_str()) {
        return Err(Refused::usage(format!(
            "--disturb takes one of {}, and `{}` is not one of them",
            KINDS.join(", "),
            kind.chars()
                .filter(|c| !c.is_control())
                .take(32)
                .collect::<String>()
        )));
    }

    let drive = match (scenario, args.value("patterns")) {
        (Some(document), _) => from_scenario(&rehearsal, &kind, &document)?,
        (None, Some(named)) => from_patterns(
            &rehearsal,
            &kind,
            &read_document(named)?,
            trace::number(args, "requests", 0)?,
        )?,
        (None, None) => {
            return Err(Refused::usage(
                "`catalyst rehearse replay` needs --patterns FILE or --scenario FILE",
            ))
        }
    };

    let result = execute(&rehearsal.target, &rehearsal, &drive);
    write_file(&out.full, out_named, &result.render_pretty())?;
    Ok(obj(vec![
        ("ok", Json::Bool(true)),
        ("out", s(out_named)),
        (
            "verdict",
            result.get("verdict").cloned().unwrap_or(Json::Null),
        ),
        (
            "requests",
            result.get("requests").cloned().unwrap_or(Json::Null),
        ),
        (
            "errors",
            result.get("errors").cloned().unwrap_or(Json::Null),
        ),
        (
            "signature",
            result.get("signature").cloned().unwrap_or(Json::Null),
        ),
    ])
    .render())
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
            "name a document this command wrote: patterns from `rehearse import`, or a \
             scenario from `rehearse reduce` or `rehearse held-out`",
        )
    })
}

/// A body for a request the replay issues. It is Catalyst's own: an imported
/// trace carries only the *shape* of a body, so there is nothing of anybody's
/// to send back.
fn body_for(method: &str) -> Option<String> {
    (method == "POST").then(|| "{\"item\":\"stand-in\",\"qty\":1}".to_owned())
}

/// Turn imported patterns into runs, one per session pass.
fn from_patterns(
    rehearsal: &Rehearsal,
    kind: &str,
    patterns: &Json,
    requested: i64,
) -> Result<Drive, Refused> {
    let sequences = patterns
        .get("sequences")
        .and_then(Json::as_arr)
        .filter(|items| !items.is_empty())
        .ok_or_else(|| {
            Refused::new(
                "catalyst.syntax",
                "the patterns document carries no `sequences` to replay",
                "derive patterns with `catalyst rehearse import --trace FILE --out FILE`",
            )
        })?;
    let shapes: Vec<Vec<(String, String)>> = sequences
        .iter()
        .filter_map(|sequence| {
            let steps = sequence.get("steps").and_then(Json::as_arr)?;
            Some(
                steps
                    .iter()
                    .filter_map(Json::as_str)
                    .filter_map(|step| {
                        let (method, path) = step.split_once(' ')?;
                        path.starts_with('/')
                            .then(|| (method.to_owned(), path.to_owned()))
                    })
                    .collect::<Vec<_>>(),
            )
            .filter(|steps: &Vec<(String, String)>| !steps.is_empty())
        })
        .collect();
    if shapes.is_empty() {
        return Err(Refused::new(
            "catalyst.syntax",
            "no sequence in the patterns document carries a replayable step",
            "each step reads `METHOD /path`, as `catalyst rehearse import` writes them",
        ));
    }

    let in_one_pass: usize = shapes.iter().map(Vec::len).sum();
    let total = if requested > 0 {
        requested as usize
    } else {
        in_one_pass
    };
    let mut runs: Vec<Vec<Step>> = Vec::new();
    let mut seq = 0usize;
    'filling: loop {
        for shape in &shapes {
            let mut run = Vec::new();
            for (method, path) in shape {
                if seq >= total {
                    if !run.is_empty() {
                        runs.push(run);
                    }
                    break 'filling;
                }
                seq += 1;
                run.push(Step {
                    method: method.clone(),
                    path: path.clone(),
                    body: body_for(method),
                    seq,
                });
            }
            if !run.is_empty() {
                runs.push(run);
            }
            if seq >= total {
                break 'filling;
            }
        }
    }
    Ok(drive_for(rehearsal, kind, runs, None))
}

/// Turn a reduced scenario into runs: one copy of its steps per parallel
/// worker, at the concurrency the scenario itself records.
fn from_scenario(rehearsal: &Rehearsal, kind: &str, scenario: &Json) -> Result<Drive, Refused> {
    let steps = scenario
        .get("steps")
        .and_then(Json::as_arr)
        .filter(|items| !items.is_empty())
        .ok_or_else(|| {
            Refused::new(
                "catalyst.syntax",
                "the scenario carries no `steps` to replay",
                "reduce a result with `catalyst rehearse reduce --result FILE --dir DIR \
                 --out FILE`, or use a scenario from `catalyst rehearse held-out`",
            )
        })?;
    let shape: Vec<(String, Option<String>)> = steps
        .iter()
        .filter_map(|step| {
            let method = step.get("method").and_then(Json::as_str)?;
            let path = step.get("path").and_then(Json::as_str)?;
            if !path.starts_with('/') {
                return None;
            }
            let body = step
                .get("body")
                .and_then(Json::as_str)
                .map(str::to_owned)
                .or_else(|| body_for(method));
            Some((format!("{method} {path}"), body))
        })
        .collect();
    if shape.is_empty() {
        return Err(Refused::new(
            "catalyst.syntax",
            "no step of the scenario carries a method and a relative path",
            "each step is {\"method\":\"POST\",\"path\":\"/orders\",\"body\":\"…\"}",
        ));
    }
    let concurrency = scenario
        .get("concurrency")
        .and_then(Json::as_f64)
        .filter(|n| n.is_finite() && *n >= 1.0)
        .unwrap_or(1.0) as usize;
    let mut runs = Vec::new();
    let mut seq = 0usize;
    for _ in 0..concurrency {
        let mut run = Vec::new();
        for (step, body) in &shape {
            let (method, path) = step.split_once(' ').unwrap_or(("GET", "/health"));
            seq += 1;
            run.push(Step {
                method: method.to_owned(),
                path: path.to_owned(),
                body: body.clone(),
                seq,
            });
        }
        runs.push(run);
    }
    Ok(drive_for(rehearsal, kind, runs, Some(concurrency)))
}

/// Apply the declared disturbance, held inside its declared bound.
fn drive_for(
    rehearsal: &Rehearsal,
    kind: &str,
    runs: Vec<Vec<Step>>,
    forced_concurrency: Option<usize>,
) -> Drive {
    let mut drive = Drive {
        kind: kind.to_owned(),
        runs,
        concurrency: 1,
        delay_ms: 0.0,
        timeout_ms: 5000.0,
        malformed_fraction: 0.0,
    };
    match kind {
        "burst" => {
            drive.concurrency = rehearsal
                .within("burst", "concurrency", "max_concurrency", 16.0)
                .max(1.0) as usize
        }
        "slow_dependency" => {
            drive.delay_ms = rehearsal
                .within("slow_dependency", "delay_ms", "max_delay_ms", 200.0)
                .max(0.0)
        }
        "timeout" => {
            drive.timeout_ms = rehearsal
                .within("timeout", "timeout_ms", "min_timeout_ms", 500.0)
                .max(1.0)
        }
        "malformed_input" => {
            drive.malformed_fraction = rehearsal
                .within("malformed_input", "fraction", "max_fraction", 0.1)
                .clamp(0.0, 1.0)
        }
        _ => {}
    }
    if let Some(concurrency) = forced_concurrency {
        drive.concurrency = concurrency.max(1);
    }
    drive.concurrency = drive.concurrency.min(drive.runs.len().max(1));
    drive
}

/// Issue the traffic and judge it. This is the only function that opens
/// anything, and it opens only what the validated target names.
pub fn execute(target: &Target, rehearsal: &Rehearsal, drive: &Drive) -> Json {
    let workers = drive.concurrency.max(1);
    let mut chunks: Vec<Vec<Vec<Step>>> = vec![Vec::new(); workers];
    for (index, run) in drive.runs.iter().enumerate() {
        chunks[index % workers].push(run.clone());
    }

    let in_flight = AtomicUsize::new(0);
    let peak = AtomicUsize::new(0);
    let malformed = AtomicUsize::new(0);
    // Every worker waits here, so a burst is a burst rather than a queue of
    // threads that happened to start at different moments.
    let barrier = Barrier::new(workers);
    let timeout = Duration::from_millis(drive.timeout_ms.max(1.0) as u64);
    let delay = Duration::from_millis(drive.delay_ms.max(0.0) as u64);

    let collected: Vec<Vec<Record>> = std::thread::scope(|scope| {
        let mut handles = Vec::new();
        for chunk in &chunks {
            let in_flight = &in_flight;
            let peak = &peak;
            let malformed = &malformed;
            let barrier = &barrier;
            handles.push(scope.spawn(move || {
                let mut records = Vec::new();
                barrier.wait();
                for run in chunk {
                    // Each session knows the identifier it created, so
                    // `/orders/{id}` is a request for something that exists.
                    let mut known = "o1".to_owned();
                    for step in run {
                        let path = step.path.replace("{id}", &known);
                        let body = match step.body.as_deref() {
                            Some(_) if is_malformed(step.seq, drive.malformed_fraction) => {
                                malformed.fetch_add(1, Ordering::SeqCst);
                                Some("{ this is not a JSON body".to_owned())
                            }
                            Some(body) => Some(body.to_owned()),
                            None => None,
                        };
                        let now = in_flight.fetch_add(1, Ordering::SeqCst) + 1;
                        peak.fetch_max(now, Ordering::SeqCst);
                        let answer =
                            wire::send(target, &step.method, &path, body.as_deref(), timeout);
                        in_flight.fetch_sub(1, Ordering::SeqCst);
                        match answer {
                            Ok(reply) => {
                                if let Some(id) = identifier(&reply.body) {
                                    known = id;
                                }
                                records.push(Record {
                                    seq: step.seq,
                                    method: step.method.clone(),
                                    path: path.clone(),
                                    status: reply.status,
                                    elapsed_ms: reply.elapsed_ms,
                                    error: reply.status >= 500,
                                });
                            }
                            Err(_) => records.push(Record {
                                seq: step.seq,
                                method: step.method.clone(),
                                path: path.clone(),
                                // No status at all: the request did not arrive
                                // or no answer came back.
                                status: 0,
                                elapsed_ms: drive.timeout_ms,
                                error: true,
                            }),
                        }
                        if !delay.is_zero() {
                            std::thread::sleep(delay);
                        }
                    }
                }
                records
            }));
        }
        handles
            .into_iter()
            .map(|handle| handle.join().unwrap_or_default())
            .collect()
    });

    let mut records: Vec<Record> = collected.into_iter().flatten().collect();
    records.sort_by_key(|record| record.seq);
    judge(
        target,
        rehearsal,
        drive,
        &records,
        peak.load(Ordering::SeqCst),
        malformed.load(Ordering::SeqCst),
    )
}

/// Whether this request's body is one of the malformed fraction. Spread evenly
/// over the sequence numbers, so the same replay malforms the same requests.
fn is_malformed(seq: usize, fraction: f64) -> bool {
    if fraction <= 0.0 {
        return false;
    }
    let before = ((seq.saturating_sub(1)) as f64 * fraction).floor();
    ((seq as f64) * fraction).floor() > before
}

/// The identifier a stand-in answered with, if it answered with one.
fn identifier(body: &str) -> Option<String> {
    let after = body.split("\"id\":\"").nth(1)?;
    let id: String = after.chars().take_while(|c| *c != '"').take(64).collect();
    (!id.is_empty()).then_some(id)
}

fn judge(
    target: &Target,
    rehearsal: &Rehearsal,
    drive: &Drive,
    records: &[Record],
    peak: usize,
    malformed: usize,
) -> Json {
    let requests = records.len();
    let errors = records.iter().filter(|record| record.error).count();
    let error_rate = if requests == 0 {
        0.0
    } else {
        errors as f64 / requests as f64
    };
    let p99 = percentile99(records);
    let verdict = if error_rate > rehearsal.max_error_rate() || p99 > rehearsal.max_p99_ms() {
        "fail"
    } else {
        "pass"
    };

    // The signature is the failure that happened most often; ties go to the
    // one that happened first, so the same run always names the same thing.
    let mut tally: Vec<(String, usize, usize)> = Vec::new();
    for record in records.iter().filter(|record| record.error) {
        let named = format!(
            "{} {} -> {}",
            record.method,
            trace::template(&record.path),
            record.status
        );
        match tally.iter_mut().find(|(name, _, _)| *name == named) {
            Some(entry) => entry.1 += 1,
            None => tally.push((named, 1, record.seq)),
        }
    }
    tally.sort_by(|a, b| b.1.cmp(&a.1).then(a.2.cmp(&b.2)));
    let signature = tally
        .first()
        .map(|(name, _, _)| name.clone())
        .unwrap_or_default();

    let mut applied = vec![("concurrency_peak", Json::Num(peak as f64))];
    match drive.kind.as_str() {
        "slow_dependency" => applied.push(("delay_ms", Json::Num(drive.delay_ms))),
        "timeout" => applied.push(("timeout_ms", Json::Num(drive.timeout_ms))),
        "malformed_input" => {
            applied.push(("fraction", Json::Num(drive.malformed_fraction)));
            applied.push(("malformed_requests", Json::Num(malformed as f64)));
        }
        _ => {}
    }

    obj(vec![
        ("schema", s(RESULT_SCHEMA)),
        ("target", s(&target.written())),
        (
            "disturbance",
            obj(vec![
                ("kind", s(&drive.kind)),
                ("declared", rehearsal.declared(&drive.kind)),
                ("applied", obj(applied)),
            ]),
        ),
        ("requests", Json::Num(requests as f64)),
        ("errors", Json::Num(errors as f64)),
        ("error_rate", Json::Num(rounded(error_rate, 6))),
        ("p99_ms", Json::Num(rounded(p99, 3))),
        ("criteria", rehearsal.criteria.clone()),
        ("configuration_digest", s(&rehearsal.configuration_digest)),
        ("verdict", s(verdict)),
        (
            "failures",
            Json::Arr(
                records
                    .iter()
                    .filter(|record| record.error)
                    .take(20)
                    .map(|record| {
                        obj(vec![
                            ("method", s(&record.method)),
                            ("path", s(&trace::template(&record.path))),
                            ("status", Json::Num(f64::from(record.status))),
                            ("seq", Json::Num(record.seq as f64)),
                        ])
                    })
                    .collect(),
            ),
        ),
        ("signature", s(&signature)),
    ])
}

fn percentile99(records: &[Record]) -> f64 {
    if records.is_empty() {
        return 0.0;
    }
    let mut times: Vec<f64> = records.iter().map(|record| record.elapsed_ms).collect();
    times.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
    let index = (((times.len() as f64) * 0.99).ceil() as usize).saturating_sub(1);
    times[index.min(times.len() - 1)]
}

/// A measurement written to as many decimals as it is worth, and no more.
fn rounded(value: f64, decimals: u32) -> f64 {
    if !value.is_finite() {
        return 0.0;
    }
    let scale = 10f64.powi(decimals as i32);
    (value * scale).round() / scale
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_malformed_fraction_is_spread_evenly_and_is_the_same_every_time() {
        let chosen: Vec<usize> = (1..=20).filter(|seq| is_malformed(*seq, 0.1)).collect();
        assert_eq!(chosen, vec![10, 20]);
        assert!(!is_malformed(5, 0.0));
        assert_eq!((1..=10).filter(|seq| is_malformed(*seq, 1.0)).count(), 10);
    }

    #[test]
    fn an_identifier_is_read_out_of_a_stand_in_answer() {
        assert_eq!(
            identifier("{\"id\":\"o17\",\"stand_in\":true}"),
            Some("o17".to_owned())
        );
        assert_eq!(identifier("{\"stand_in\":true}"), None);
    }

    #[test]
    fn the_ninety_ninth_percentile_is_the_slow_end_and_not_the_average() {
        let records: Vec<Record> = (1..=100)
            .map(|n| Record {
                seq: n,
                method: "GET".to_owned(),
                path: "/health".to_owned(),
                status: 200,
                elapsed_ms: n as f64,
                error: false,
            })
            .collect();
        assert_eq!(percentile99(&records), 99.0);
    }

    #[test]
    fn rounding_keeps_a_measurement_from_claiming_more_than_it_measured() {
        assert_eq!(rounded(0.8333333333, 6), 0.833333);
        assert_eq!(rounded(30.1234, 3), 30.123);
        assert_eq!(rounded(f64::NAN, 3), 0.0);
    }
}
