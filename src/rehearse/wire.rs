//! One HTTP/1.1 request over one stream, written by hand.
//!
//! `docs/interface.md` §8. The crate has no dependencies, so this is the whole
//! client: open the socket, write a request, read until the far end closes,
//! read the status off the first line. `Connection: close` on every request is
//! what makes "read to the end" a complete answer rather than a guess about
//! where the body stopped, and it is honest about what a rehearsal measures --
//! each request pays for its own connection, which is the shape of the traffic
//! being replayed.
//!
//! # Where a target is turned into an address
//!
//! Nowhere but here, and only from a [`Target`] that
//! [`super::target::parse`] has already accepted. `localhost` and `::1` are
//! mapped to the loopback addresses directly rather than looked up: asking a
//! resolver what `localhost` means would make the answer depend on something
//! outside this process, and the point of the target rule is that it does not.

use crate::rehearse::target::Target;
use std::io::{Read, Write};
use std::net::{Ipv4Addr, Ipv6Addr, SocketAddr, TcpStream};
use std::os::unix::net::UnixStream;
use std::time::{Duration, Instant};

/// What one request produced.
pub struct Reply {
    pub status: u16,
    pub body: String,
    pub elapsed_ms: f64,
}

/// Either kind of stream, behind the two operations this client needs.
enum Wire {
    Unix(UnixStream),
    Tcp(TcpStream),
}

impl Wire {
    fn deadlines(&self, timeout: Duration) -> std::io::Result<()> {
        match self {
            Wire::Unix(stream) => {
                stream.set_read_timeout(Some(timeout))?;
                stream.set_write_timeout(Some(timeout))
            }
            Wire::Tcp(stream) => {
                stream.set_read_timeout(Some(timeout))?;
                stream.set_write_timeout(Some(timeout))
            }
        }
    }
}

impl Read for Wire {
    fn read(&mut self, buffer: &mut [u8]) -> std::io::Result<usize> {
        match self {
            Wire::Unix(stream) => stream.read(buffer),
            Wire::Tcp(stream) => stream.read(buffer),
        }
    }
}

impl Write for Wire {
    fn write(&mut self, buffer: &[u8]) -> std::io::Result<usize> {
        match self {
            Wire::Unix(stream) => stream.write(buffer),
            Wire::Tcp(stream) => stream.write(buffer),
        }
    }
    fn flush(&mut self) -> std::io::Result<()> {
        match self {
            Wire::Unix(stream) => stream.flush(),
            Wire::Tcp(stream) => stream.flush(),
        }
    }
}

/// The loopback address a validated host names. `None` is unreachable for a
/// target the rule accepted, and is treated as a transport failure rather than
/// as a reason to stop.
fn address(host: &str, port: u16) -> Option<SocketAddr> {
    let trimmed = host.trim_matches(['[', ']']);
    if trimmed == "localhost" {
        return Some(SocketAddr::from((Ipv4Addr::LOCALHOST, port)));
    }
    if trimmed == "::1" {
        return Some(SocketAddr::from((Ipv6Addr::LOCALHOST, port)));
    }
    let octets: Vec<u8> = trimmed
        .split('.')
        .filter_map(|part| part.parse::<u8>().ok())
        .collect();
    match octets.as_slice() {
        [a, b, c, d] => Some(SocketAddr::from((Ipv4Addr::new(*a, *b, *c, *d), port))),
        _ => None,
    }
}

fn open(target: &Target, timeout: Duration) -> Result<Wire, String> {
    let wire = match target {
        Target::Unix(path) => {
            Wire::Unix(UnixStream::connect(path).map_err(|error| kind("connect", &error))?)
        }
        Target::Loopback { host, port } => {
            let Some(address) = address(host, *port) else {
                return Err("the target names no address this client can open".to_owned());
            };
            Wire::Tcp(
                TcpStream::connect_timeout(&address, timeout)
                    .map_err(|error| kind("connect", &error))?,
            )
        }
    };
    wire.deadlines(timeout)
        .map_err(|error| kind("set the deadline of", &error))?;
    Ok(wire)
}

/// Send one request and read the whole answer. The error is a short phrase, not
/// an operating-system message: those carry paths, and no absolute path may
/// reach anything Catalyst writes.
pub fn send(
    target: &Target,
    method: &str,
    path: &str,
    body: Option<&str>,
    timeout: Duration,
) -> Result<Reply, String> {
    let started = Instant::now();
    let mut wire = open(target, timeout)?;
    let body = body.unwrap_or("");
    let mut request = format!(
        "{method} {path} HTTP/1.1\r\nHost: {}\r\nConnection: close\r\nAccept: application/json\r\n",
        target.host_header()
    );
    if !body.is_empty() {
        request.push_str("Content-Type: application/json\r\n");
        request.push_str(&format!("Content-Length: {}\r\n", body.len()));
    }
    request.push_str("\r\n");
    request.push_str(body);
    wire.write_all(request.as_bytes())
        .map_err(|error| kind("write to", &error))?;
    wire.flush().map_err(|error| kind("flush", &error))?;

    let mut answer = Vec::new();
    let mut chunk = [0u8; 4096];
    loop {
        match wire.read(&mut chunk) {
            Ok(0) => break,
            Ok(read) => {
                answer.extend_from_slice(&chunk[..read]);
                // A stand-in's answer is small; a far end that will not stop
                // talking is a transport failure, not an answer.
                if answer.len() > 1024 * 1024 {
                    return Err("the answer was longer than a megabyte".to_owned());
                }
            }
            Err(error) => return Err(kind("read from", &error)),
        }
    }
    let text = String::from_utf8_lossy(&answer).into_owned();
    let Some(status) = status_of(&text) else {
        return Err("the answer had no HTTP status line".to_owned());
    };
    Ok(Reply {
        status,
        body: text.split("\r\n\r\n").nth(1).unwrap_or("").to_owned(),
        elapsed_ms: started.elapsed().as_secs_f64() * 1000.0,
    })
}

/// The status code off the first line: `HTTP/1.1 503 Service Unavailable`.
fn status_of(answer: &str) -> Option<u16> {
    let line = answer.lines().next()?;
    let mut words = line.split_whitespace();
    let version = words.next()?;
    if !version.starts_with("HTTP/") {
        return None;
    }
    words.next()?.parse::<u16>().ok()
}

/// What went wrong, said in words, with nothing the operating system attached.
fn kind(what: &str, error: &std::io::Error) -> String {
    format!(
        "could not {what} the target: {}",
        format!("{:?}", error.kind()).to_lowercase()
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_status_is_read_off_the_first_line() {
        assert_eq!(
            status_of("HTTP/1.1 503 Service Unavailable\r\nX: 1\r\n\r\n{}"),
            Some(503)
        );
        assert_eq!(status_of("HTTP/1.0 200 OK\r\n\r\n"), Some(200));
        assert_eq!(status_of(""), None);
        assert_eq!(status_of("not an answer at all"), None);
    }

    #[test]
    fn the_loopback_spellings_all_name_a_loopback_address() {
        for host in ["localhost", "127.0.0.1", "::1", "[::1]", "127.9.9.9"] {
            let found = address(host, 8080).unwrap_or_else(|| panic!("no address for {host}"));
            assert!(found.ip().is_loopback() || host == "127.9.9.9", "{host}");
            assert_eq!(found.port(), 8080);
        }
    }

    #[test]
    fn a_transport_error_never_carries_a_path() {
        let error = std::io::Error::from(std::io::ErrorKind::NotFound);
        let said = kind("connect", &error);
        assert!(said.contains("notfound"), "{said}");
        assert!(!said.contains('/'), "{said}");
    }
}
