//! Catalyst as a process: JSON on stdin, JSON on stdout.
//!
//! This is the whole interface for a caller that is not a Rust crate -- a
//! script, a notebook, a training loop in another language, or an agent that
//! can run a command but cannot link one.
//!
//!     echo '{"source": "func f(x, y) = x*y + sin(x)", "at": [0.7, 1.3]}' \
//!       | cargo run --quiet --example grad
//!
//!     {"ok":true,"value":1.554217687237691,
//!      "gradient":{"x":2.0648421872844884,"y":0.7},
//!      "cost":{"primal_insts":5,"adjoint_insts":13}}
//!
//! That output is copied from a run, not composed: 0.7*1.3 + sin(0.7) is
//! 1.554217687237691, and d/dx is y + cos x = 2.0648421872844886.
//!
//! A refusal comes back on stdout too, as an object with the same `ok` field
//! and a stable `code`, so the caller parses one shape and branches on one
//! field. Nothing is written to stderr and the exit status is always 0 for a
//! well-formed run: an agent reading stdout should never have to also
//! interpret a signal to find out what happened.
//!
//! One request per invocation, read to end-of-input. That is the shape a shell
//! pipeline and a subprocess call both already have.

use std::io::Read;

fn main() {
    let request = match read_request(std::io::stdin().lock()) {
        Ok(request) => request,
        Err(_) => {
            // Even this is reported in the shape the caller is parsing for, rather
            // than as a panic it would have to detect some other way.
            println!(
                "{{\"ok\":false,\"code\":\"catalyst.unreadable_request\",\
             \"detail\":\"stdin was not valid UTF-8\",\
             \"remedy\":\"send the request as UTF-8 JSON\"}}"
            );
            return;
        }
    };
    println!("{}", catalyst::api::solve(&request));
}

fn read_request(input: impl Read) -> std::io::Result<String> {
    let mut request = String::new();
    // One extra byte lets the shared parser distinguish an oversized request
    // from an exactly-at-limit request, without consuming an unbounded stream.
    input
        .take(catalyst::api::MAX_REQUEST_BYTES as u64 + 1)
        .read_to_string(&mut request)?;
    Ok(request)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn oversized_input_is_bounded_before_it_is_allocated_in_full() {
        let mut input = std::io::Cursor::new(vec![b' '; 1024 * 1024 + 4096]);
        let request = read_request(&mut input).unwrap();
        assert_eq!(input.position(), 1024 * 1024 + 1);
        assert!(catalyst::api::solve(&request).contains("\"ok\":false"));
    }

    #[test]
    fn a_normal_request_is_read_without_changes() {
        let request = b"{\"source\":\"func f(x)=x*x\",\"at\":[2]}";
        assert_eq!(read_request(&request[..]).unwrap().as_bytes(), request);
        assert!(read_request(&[0xff_u8][..]).is_err());
    }
}
