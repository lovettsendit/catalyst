//! The stand-in service: a Go program Catalyst writes out, for a rehearsal to
//! be aimed at instead of anything real.
//!
//! `docs/interface.md` §8. It is a *stand-in*: it says so in its own start
//! banner, and every JSON body it answers with carries `"stand_in":true`, so a
//! result recorded against it cannot later be mistaken for a measurement of a
//! real service. That naming is not decoration; it is the difference between a
//! rehearsal and a claim.
//!
//! It listens on a Unix-domain socket or on loopback and refuses anything
//! else, with the same rule and the same reasoning as
//! [`super::target`]: a stand-in that could be published on a real interface
//! would be a service nobody meant to run.
//!
//! # Why the source is embedded from `go/standin/` rather than restated here
//!
//! `docs/interface.md` §11. It used to be a string constant in this file, and
//! the cost of that was invisible until it was named: the Go toolchain never
//! saw the program until Catalyst had written it to a temporary directory, so
//! `go vet` in the repository had nothing to look at, and somebody opening a
//! project whose whole business is Go found no Go in it.
//!
//! Embedding the file rather than copying its text is the point. Two copies
//! would drift -- somebody fixes the one they can see, and the binary keeps
//! shipping the other. With [`include_str!`] the bytes the toolchain checks
//! and the bytes `catalyst rehearse standin` writes are the same bytes by
//! construction, and the acceptance oracle compares them anyway.
//!
//! The embedding happens at compile time, so the running binary still carries
//! the program and reaches for no path at run time: writing the stand-in works
//! from any directory, with the repository nowhere in sight.

use crate::cli::Refused;
use crate::json::{obj, s, Json};
use crate::paths;

/// The files an export of the stand-in writes.
pub const FILES: &[&str] = &["go.mod", "main.go", "listen.go"];

/// `go.mod`: no `require`, because the stand-in uses the standard library and
/// nothing else, and a rehearsal that had to fetch a module would be a
/// rehearsal that needed a network.
pub const GO_MOD: &str = include_str!("../../go/standin/go.mod");

/// `main.go`, the stand-in itself.
pub const MAIN_GO: &str = include_str!("../../go/standin/main.go");

/// `listen.go`: the one file of the stand-in that opens anything.
///
/// Separated for the same reason [`crate::adapter`] is separated from the
/// rest of Catalyst: everything that can reach outside the process is in one
/// short file, so "what can this program touch" is a question somebody
/// answers by reading rather than by grepping. It is also the only file that
/// names the `net` package -- the service itself is written against
/// `net/http` and never opens a socket of its own.
pub const LISTEN_GO: &str = include_str!("../../go/standin/listen.go");

/// `catalyst rehearse standin --out DIR`.
pub fn write_standin(args: &crate::cli::Args) -> Result<String, Refused> {
    let named = args.required("out", "rehearse standin")?;
    let out = paths::prepare_out_dir(named)?;
    paths::write_file(&out.full, "go.mod", GO_MOD)?;
    paths::write_file(&out.full, "main.go", MAIN_GO)?;
    paths::write_file(&out.full, "listen.go", LISTEN_GO)?;
    Ok(obj(vec![
        ("ok", Json::Bool(true)),
        ("out", s(named)),
        ("files", Json::Arr(FILES.iter().map(|f| s(f)).collect())),
        ("stand_in", Json::Bool(true)),
    ])
    .render())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_stand_in_names_itself_a_stand_in() {
        assert!(MAIN_GO.to_ascii_lowercase().contains("stand-in"));
        assert!(MAIN_GO.contains("stand_in"));
    }

    #[test]
    fn the_module_needs_nothing_and_the_source_imports_only_the_standard_library() {
        assert!(!GO_MOD.contains("require"));
        assert!(GO_MOD.contains("go 1.21"));
        for source in [MAIN_GO, LISTEN_GO] {
            for line in source.lines() {
                let trimmed = line.trim();
                if !trimmed.starts_with('"') || !trimmed.ends_with('"') {
                    continue;
                }
                let import = trimmed.trim_matches('"');
                assert!(
                    !import.split('/').next().unwrap_or("").contains('.'),
                    "the stand-in must import only the standard library: {import}"
                );
            }
        }
    }

    /// Everything that can open anything is in `listen.go`, and nothing else
    /// in the stand-in names a socket.
    #[test]
    fn only_one_file_of_the_stand_in_can_open_anything() {
        assert!(LISTEN_GO.contains("net.Listen("));
        for forbidden in ["net.Listen(", "net.Dial(", "\"net\"", "os/exec"] {
            assert!(
                !MAIN_GO.contains(forbidden),
                "main.go must not name `{forbidden}`"
            );
        }
    }

    /// The embedded bytes are the file's bytes. This is what stops the two
    /// from ever becoming two programs.
    #[test]
    fn what_is_embedded_is_the_file_in_the_tree() {
        let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("go/standin");
        for (name, embedded) in [
            ("go.mod", GO_MOD),
            ("main.go", MAIN_GO),
            ("listen.go", LISTEN_GO),
        ] {
            let on_disk = std::fs::read_to_string(root.join(name))
                .unwrap_or_else(|error| panic!("go/standin/{name}: {error}"));
            assert_eq!(on_disk, embedded, "go/standin/{name}");
        }
    }
}
