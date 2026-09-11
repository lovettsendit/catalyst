//! Which targets a rehearsal may point at, decided by reading the text.
//!
//! `docs/interface.md` §8. A rehearsal drives real traffic at whatever it is
//! aimed at. Aimed at the wrong thing it sends orders to a payment service,
//! mail to real addresses, writes to a production database -- so the set of
//! things it may be aimed at is small, closed, and checked before anything is
//! opened:
//!
//! * `unix:<relative path>` -- a socket inside the current directory, under
//!   the path rule of §0, so it cannot be `/var/run/docker.sock` and cannot
//!   climb out with `..`;
//! * `http://127.0.0.1:PORT`, `http://localhost:PORT`, `http://[::1]:PORT`.
//!
//! # Why the decision is made by parsing and never by resolving
//!
//! Because a name lookup is itself a request to something outside this
//! process, and because what a name resolves to now is not what it resolved to
//! a moment ago. `http://127.0.0.1.evil.example:80` *contains* the loopback
//! address as text and is not the loopback address; a check that asked a
//! resolver would be trusting the resolver, and a check that used `contains`
//! would accept it. So the host is compared as a whole, against a closed list,
//! and 127.0.0.0/8 is recognised by parsing four decimal octets.
//!
//! The refusal always names the host, because the only useful thing a person
//! can be told here is which target they aimed at by mistake.

use crate::cli::Refused;
use crate::paths;

/// A target a rehearsal is allowed to drive.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Target {
    /// A Unix-domain socket at a path inside the current directory.
    Unix(String),
    /// Loopback TCP: the host as written, and the port.
    Loopback { host: String, port: u16 },
}

/// The code every non-local target is refused with.
pub const NOT_LOCAL: &str = "catalyst.rehearsal.target_not_local";

impl Target {
    /// The text this target was written as, which is what the recorded
    /// configuration and every result carry.
    pub fn written(&self) -> String {
        match self {
            Target::Unix(path) => format!("unix:{path}"),
            Target::Loopback { host, port } => format!("http://{host}:{port}"),
        }
    }

    /// What a `Host:` header should say for this target.
    pub fn host_header(&self) -> String {
        match self {
            Target::Unix(_) => "standin".to_owned(),
            Target::Loopback { host, port } => format!("{host}:{port}"),
        }
    }
}

fn not_local(host: &str, why: &str) -> Refused {
    Refused::new(
        NOT_LOCAL,
        format!("the target host `{}` is not local: {why}", safe(host)),
        "a rehearsal may only drive a local stand-in: `unix:<relative path>`, \
         `http://127.0.0.1:PORT`, `http://localhost:PORT` or `http://[::1]:PORT`. \
         start `catalyst rehearse standin` and aim at that",
    )
}

/// Read a target, or refuse. Nothing is opened, nothing is resolved, and no
/// request of any kind is made.
pub fn parse(written: &str) -> Result<Target, Refused> {
    if let Some(path) = written.strip_prefix("unix:") {
        // The path rule of §0 decides this one: a socket outside the current
        // directory is refused as a path, which is what it is.
        paths::resolve(path)?;
        return Ok(Target::Unix(path.to_owned()));
    }
    let Some((scheme, rest)) = written.split_once("://") else {
        return Err(not_local(
            written,
            "it names no scheme this rehearsal understands",
        ));
    };
    let authority = rest.split(['/', '?', '#']).next().unwrap_or("");
    let (host, port) = split_authority(authority);
    if scheme != "http" {
        return Err(not_local(
            host,
            &format!("`{}` is not a scheme a rehearsal may drive", safe(scheme)),
        ));
    }
    if !is_loopback(host) {
        return Err(not_local(
            host,
            "only the loopback interface and a Unix socket may be driven",
        ));
    }
    let port = match port {
        None => 80,
        Some(text) => match text.parse::<u16>() {
            Ok(port) if port > 0 => port,
            _ => {
                return Err(Refused::new(
                    NOT_LOCAL,
                    format!("the target names no usable port after `{}`", safe(host)),
                    "write the port as a number, for example `http://127.0.0.1:8080`",
                ))
            }
        },
    };
    Ok(Target::Loopback {
        host: host.to_owned(),
        port,
    })
}

/// Split `host:port`, keeping a bracketed IPv6 literal in one piece.
fn split_authority(authority: &str) -> (&str, Option<&str>) {
    if let Some(rest) = authority.strip_prefix('[') {
        return match rest.split_once(']') {
            Some((inside, after)) => (inside, after.strip_prefix(':')),
            None => (authority, None),
        };
    }
    match authority.rsplit_once(':') {
        Some((host, port)) => (host, Some(port)),
        None => (authority, None),
    }
}

/// Whether a host *is* the loopback interface -- not whether it mentions it.
fn is_loopback(host: &str) -> bool {
    if host == "localhost" || host == "::1" || host == "[::1]" {
        return true;
    }
    let octets: Vec<&str> = host.split('.').collect();
    if octets.len() != 4 {
        return false;
    }
    let mut parsed = [0u8; 4];
    for (slot, text) in parsed.iter_mut().zip(&octets) {
        // No leading zeros, no signs, no whitespace: `127.0.0.01` is not an
        // address this accepts, because two readers disagree about what it is.
        if text.is_empty() || (text.len() > 1 && text.starts_with('0')) {
            return false;
        }
        match text.parse::<u8>() {
            Ok(value) => *slot = value,
            Err(_) => return false,
        }
    }
    parsed[0] == 127
}

/// A host from a target, in a form a message can carry.
fn safe(text: &str) -> String {
    let mut out: String = text.chars().filter(|c| !c.is_control()).take(64).collect();
    if text.chars().count() > 64 {
        out.push('…');
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_three_loopback_spellings_and_a_relative_socket_are_accepted() {
        assert_eq!(
            parse("http://127.0.0.1:8080").expect("loopback"),
            Target::Loopback {
                host: "127.0.0.1".to_owned(),
                port: 8080
            }
        );
        assert!(parse("http://localhost:8080").is_ok());
        assert!(parse("http://[::1]:8080").is_ok());
        assert!(parse("http://127.9.9.9:1").is_ok());
        assert_eq!(
            parse("unix:standin.sock").expect("socket"),
            Target::Unix("standin.sock".to_owned())
        );
    }

    #[test]
    fn a_host_that_merely_contains_the_loopback_address_is_not_the_loopback_address() {
        let refused = parse("http://127.0.0.1.evil.example:80").expect_err("refused");
        assert_eq!(refused.code, NOT_LOCAL);
        assert!(
            refused.detail.contains("127.0.0.1.evil.example"),
            "{}",
            refused.detail
        );
    }

    #[test]
    fn another_host_another_scheme_or_no_scheme_is_refused_and_the_host_is_named() {
        for (written, named) in [
            ("http://10.0.0.5:8080", "10.0.0.5"),
            ("http://example.com", "example.com"),
            ("http://192.168.1.10:9", "192.168.1.10"),
            (
                "https://payments.example.com/charge",
                "payments.example.com",
            ),
            ("smtp://mail.example.com:25", "mail.example.com"),
            ("postgres://prod-db:5432/orders", "prod-db"),
        ] {
            let refused = parse(written).expect_err("refused");
            assert_eq!(refused.code, NOT_LOCAL, "{written}");
            assert!(
                refused.detail.contains(named),
                "`{written}` should name `{named}`: {}",
                refused.detail
            );
        }
    }

    #[test]
    fn a_socket_outside_the_current_directory_is_refused_as_a_path() {
        for written in ["unix:/var/run/docker.sock", "unix:../outside.sock"] {
            assert_eq!(
                parse(written).expect_err("refused").code,
                "catalyst.path_refused",
                "{written}"
            );
        }
    }
}
