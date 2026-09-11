//! The rehearsal configuration: what is being driven, what counts as passing,
//! and how hard it may be pushed.
//!
//! `docs/interface.md` §8. `catalyst rehearse init` writes `rehearsal.json`
//! once; every later command reads it and none of them change it.
//!
//! # Why the digest covers three things and not four
//!
//! The digest is over the target, the criteria and the disturbances -- and
//! deliberately not over anything imported afterwards. A result carries the
//! digest of the configuration it was measured under, so two results can be
//! compared only when they were judged by the same rule. Importing a trace,
//! replaying, reducing a failure and comparing two targets all leave it
//! untouched, which is what makes "the criteria are unchanged" a checkable
//! statement rather than an assurance.
//!
//! That also settles what a trace can do: nothing. A trace could contain a
//! sentence asking for `max_error_rate` to be raised, and there is no code
//! path from a trace to this file.
//!
//! # Every read re-validates the target
//!
//! `rehearsal.json` is a file on disk, and a file on disk can be edited. So
//! the target is parsed again by [`super::target::parse`] every time it is
//! read, before anything is opened. A configuration edited by hand to point at
//! a payment service is refused at the next command, naming that host, having
//! sent nothing.

use crate::cli::{Args, Refused};
use crate::json::{obj, s, Json};
use crate::paths;
use crate::rehearse::digest;
use crate::rehearse::target::{self, Target};
use crate::rehearse::trace::write_file;

/// The schema of `rehearsal.json`.
pub const SCHEMA: &str = "catalyst.rehearsal.v1";
/// The name it always has inside the rehearsal directory.
pub const FILE: &str = "rehearsal.json";

/// A read rehearsal: the validated target and the rules it is judged by.
pub struct Rehearsal {
    pub target: Target,
    pub criteria: Json,
    pub disturbances: Json,
    pub configuration_digest: String,
    /// The directory as the user named it, for messages.
    pub named: String,
    pub full: std::path::PathBuf,
}

impl Rehearsal {
    pub fn max_error_rate(&self) -> f64 {
        self.criteria
            .get("max_error_rate")
            .and_then(Json::as_f64)
            .unwrap_or(0.01)
    }

    pub fn max_p99_ms(&self) -> f64 {
        self.criteria
            .get("max_p99_ms")
            .and_then(Json::as_f64)
            .unwrap_or(2000.0)
    }

    /// The declared block of one disturbance, or an empty object.
    pub fn declared(&self, kind: &str) -> Json {
        self.disturbances
            .get(kind)
            .cloned()
            .unwrap_or_else(|| Json::Obj(Vec::new()))
    }

    /// A declared number of a disturbance, held inside the bound declared next
    /// to it. A disturbance that could exceed its own declared bound would be
    /// a rehearsal doing something nobody agreed to.
    pub fn within(&self, kind: &str, field: &str, bound: &str, fallback: f64) -> f64 {
        let block = self.declared(kind);
        let value = block.get(field).and_then(Json::as_f64).unwrap_or(fallback);
        match block.get(bound).and_then(Json::as_f64) {
            Some(limit) if bound.starts_with("min_") => value.max(limit),
            Some(limit) => value.min(limit),
            None => value,
        }
    }
}

/// The criteria of §8, as written.
fn criteria() -> Json {
    obj(vec![
        ("max_error_rate", Json::Num(0.01)),
        ("max_p99_ms", Json::Num(2000.0)),
    ])
}

/// The disturbances of §8: each one a declared value and the bound it may
/// never pass.
fn disturbances() -> Json {
    obj(vec![
        (
            "burst",
            obj(vec![
                ("concurrency", Json::Num(16.0)),
                ("max_concurrency", Json::Num(64.0)),
            ]),
        ),
        (
            "slow_dependency",
            obj(vec![
                ("delay_ms", Json::Num(200.0)),
                ("max_delay_ms", Json::Num(2000.0)),
            ]),
        ),
        (
            "timeout",
            obj(vec![
                ("timeout_ms", Json::Num(500.0)),
                ("min_timeout_ms", Json::Num(50.0)),
            ]),
        ),
        (
            "malformed_input",
            obj(vec![
                ("fraction", Json::Num(0.1)),
                ("max_fraction", Json::Num(0.5)),
            ]),
        ),
    ])
}

/// The digest over exactly the target, the criteria and the disturbances.
fn digest_of(written: &str, criteria: &Json, disturbances: &Json) -> String {
    let canonical = obj(vec![
        ("target", s(written)),
        ("criteria", criteria.clone()),
        ("disturbances", disturbances.clone()),
    ]);
    digest::hex(canonical.render().as_bytes())
}

/// `catalyst rehearse init --dir R --target T`.
pub fn init(args: &Args) -> Result<String, Refused> {
    let named = args.required("dir", "rehearse init")?;
    let written = args.required("target", "rehearse init")?;
    // The directory is checked but not created, and the target is validated,
    // before anything is written: a refused target leaves no directory behind.
    let dir = paths::resolve(named)?;
    let target = target::parse(written)?;

    let criteria = criteria();
    let disturbances = disturbances();
    let document = obj(vec![
        ("schema", s(SCHEMA)),
        ("target", s(&target.written())),
        ("criteria", criteria.clone()),
        ("disturbances", disturbances.clone()),
        (
            "configuration_digest",
            s(&digest_of(&target.written(), &criteria, &disturbances)),
        ),
    ]);
    let file = dir.full.join(FILE);
    write_file(&file, &format!("{named}/{FILE}"), &document.render_pretty())?;
    Ok(answer(named, &document))
}

/// `catalyst rehearse status --dir R`.
pub fn status(args: &Args) -> Result<String, Refused> {
    let named = args.required("dir", "rehearse status")?;
    let rehearsal = read(named)?;
    Ok(answer(
        named,
        &obj(vec![
            ("schema", s(SCHEMA)),
            ("target", s(&rehearsal.target.written())),
            ("criteria", rehearsal.criteria.clone()),
            ("disturbances", rehearsal.disturbances.clone()),
            ("configuration_digest", s(&rehearsal.configuration_digest)),
        ]),
    ))
}

fn answer(named: &str, document: &Json) -> String {
    let mut members = vec![
        ("ok".to_owned(), Json::Bool(true)),
        ("dir".to_owned(), s(named)),
    ];
    if let Json::Obj(pairs) = document {
        members.extend(pairs.clone());
    }
    Json::Obj(members).render()
}

/// Read a rehearsal directory, re-validating the target.
pub fn read(named: &str) -> Result<Rehearsal, Refused> {
    let dir = paths::resolve(named)?;
    let path = format!("{named}/{FILE}");
    let text = paths::read_input(&path)?;
    let document = Json::parse(&text).map_err(|error| {
        Refused::new(
            "catalyst.syntax",
            format!(
                "`{path}` is not one JSON object (at byte {}, expected {})",
                error.at, error.what
            ),
            "run `catalyst rehearse init --dir DIR --target TARGET` to write it again",
        )
    })?;
    if document.get("schema").and_then(Json::as_str) != Some(SCHEMA) {
        return Err(Refused::new(
            "catalyst.syntax",
            format!("`{path}` does not declare `schema`: \"{SCHEMA}\""),
            "run `catalyst rehearse init --dir DIR --target TARGET` to write it again",
        ));
    }
    let Some(written) = document.get("target").and_then(Json::as_str) else {
        return Err(Refused::new(
            "catalyst.syntax",
            format!("`{path}` has no string `target`"),
            "run `catalyst rehearse init --dir DIR --target TARGET` to write it again",
        ));
    };
    // Before anything else that could reach outside this process.
    let target = target::parse(written)?;
    let criteria = document.get("criteria").cloned().unwrap_or_else(criteria);
    let disturbances = document
        .get("disturbances")
        .cloned()
        .unwrap_or_else(disturbances);
    let recomputed = digest_of(&target.written(), &criteria, &disturbances);
    if let Some(recorded) = document.get("configuration_digest").and_then(Json::as_str) {
        if recorded != recomputed {
            return Err(Refused::new(
                "catalyst.rehearsal.configuration_changed",
                format!(
                    "`{path}` no longer matches the digest recorded in it, so a result \
                     measured under it could not be compared with an earlier one"
                ),
                "run `catalyst rehearse init --dir DIR --target TARGET` again to record the \
                 configuration you mean, and re-run the rehearsal under it",
            ));
        }
    }
    Ok(Rehearsal {
        target,
        criteria,
        disturbances,
        configuration_digest: recomputed,
        named: named.to_owned(),
        full: dir.full,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_digest_covers_the_target_the_criteria_and_the_disturbances() {
        let one = digest_of("unix:a.sock", &criteria(), &disturbances());
        let same = digest_of("unix:a.sock", &criteria(), &disturbances());
        let other_target = digest_of("unix:b.sock", &criteria(), &disturbances());
        let other_criteria = digest_of(
            "unix:a.sock",
            &obj(vec![
                ("max_error_rate", Json::Num(1.0)),
                ("max_p99_ms", Json::Num(2000.0)),
            ]),
            &disturbances(),
        );
        assert_eq!(one, same);
        assert_ne!(one, other_target);
        assert_ne!(one, other_criteria);
        assert_eq!(one.len(), 64);
    }

    #[test]
    fn every_disturbance_declares_the_bound_it_may_not_pass() {
        let declared = disturbances();
        for (kind, bound) in [
            ("burst", "max_concurrency"),
            ("slow_dependency", "max_delay_ms"),
            ("timeout", "min_timeout_ms"),
            ("malformed_input", "max_fraction"),
        ] {
            assert!(
                declared
                    .get(kind)
                    .and_then(|block| block.get(bound))
                    .and_then(Json::as_f64)
                    .is_some(),
                "`{kind}` declares no `{bound}`"
            );
        }
    }
}
