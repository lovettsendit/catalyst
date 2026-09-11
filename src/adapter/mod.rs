//! The local-adapter boundary: `catalyst adapter translate` and
//! `catalyst adapter conformance`.
//!
//! `docs/interface.md` §6 and §7. This is an *extension interface*, and the
//! label of [`crate::label::LABEL`] says exactly that, on this command's usage
//! text, on `catalyst tools discover` and on the conformance report: the
//! protocol is defined and checked; what has not been checked is a live
//! conversation with anything on the other side of it.
//!
//! # What this code does not do
//!
//! It starts nothing, opens no address, and reads nothing but the request file
//! the user named. Every function under `src/adapter/` is a transformation of
//! one value into another, and the acceptance oracle of criterion 5.4 reads
//! these files and fails the build if that stops being true. That is deliberate
//! and it is the point of the whole outcome: a boundary that could reach out is
//! a boundary whose behaviour depends on what it reached, and then none of the
//! conformance fixtures below would mean anything.
//!
//! # Why the label is honest rather than modest
//!
//! Nothing here has talked to a model. Saying so is not a disclaimer bolted on
//! at the end; it is the difference between "we checked this protocol" -- which
//! the fixtures do -- and "this works with the thing you are about to plug in",
//! which nobody here has any evidence for.

pub mod conformance;
pub mod protocol;

use crate::cli::{Args, Refused};
use crate::json::{obj, s, Json};
use crate::paths;

/// `catalyst adapter …`. Returns the one JSON object the command prints.
///
/// `catalyst adapter --help` is answered before this by the dispatcher, with
/// the usage text that carries the label.
pub fn run(args: &Args) -> Result<String, Refused> {
    if args.has("conformance") {
        return Ok(conformance::report().render());
    }
    if args.has("translate") {
        let named = args.required("request", "adapter translate")?;
        let request = protocol::read_request(&paths::read_input(named)?)?;
        let translated = protocol::translate(&request)?;
        return Ok(obj(vec![
            ("ok", Json::Bool(true)),
            ("capability", s(&request.capability)),
            ("request", translated),
        ])
        .render());
    }
    Err(Refused::usage(
        "`catalyst adapter` needs a subcommand: `conformance`, or `translate --request FILE`",
    ))
}

#[cfg(test)]
mod tests {
    use crate::json::Json;
    use crate::label::LABEL;

    /// 5.3: the label is on every surface of this boundary. The conformance
    /// report and discovery are checked where they are built; this is the
    /// third surface, the usage text a person reads.
    #[test]
    fn the_usage_text_carries_the_label_verbatim() {
        assert!(crate::cli::usage("adapter").contains(LABEL));
    }

    /// 5.2: what translation produces is what the tool surface serves. The two
    /// are joined here rather than only in the acceptance oracle, so a change
    /// to either one that parts them fails at once.
    #[test]
    fn a_translated_request_is_served_by_the_tool_surface_unchanged() {
        let problem = "{\"schema\":\"catalyst.problem.v1\",\"name\":\"p\",\"goal\":\"g\",\
                       \"function\":\"func p(x) = x * x\",\"inputs\":{\"x\":3},\
                       \"domains\":{\"x\":{\"min\":0,\"max\":9,\"unit\":\"\"}}}";
        let text = format!(
            "{{\"schema\":\"{}\",\"capability\":\"evaluate\",\"arguments\":{{\"problem\":{problem}}}}}",
            super::protocol::REQUEST_SCHEMA
        );
        let request = super::protocol::read_request(&text).expect("read");
        let translated = super::protocol::translate(&request).expect("translated");
        let served = crate::tools::call(&translated.render()).expect("served");
        let answer = Json::parse(&served).expect("one object");
        assert_eq!(
            answer
                .get("result")
                .and_then(|result| result.get("value"))
                .and_then(Json::as_f64),
            Some(9.0)
        );
    }
}
