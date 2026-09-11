//! Shared helpers for the prepared acceptance oracles. Not a test.
//!
//! Operator side: this file lives outside the writable source slot. It carries
//! a small JSON reader, a bounded process runner, the Go toolchain environment
//! the check lane needs (caches under `$TMPDIR`, no cgo, no network), and the
//! per-test time bound of criterion C3. Nothing here depends on a crate.
#![allow(dead_code)]

use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::{Duration, Instant};

/// The fixed label of criterion 5.3, verbatim.
pub const LABEL: &str =
    "local-adapter extension interface; live local-model compatibility untested";

/// Criterion C3: no acceptance test exceeds this many seconds.
pub const PER_TEST_BOUND_SECS: u64 = 60;

/// The two limitation sentences of criterion 1.5, verbatim.
pub const LIMITATION_GO: &str =
    "Arbitrary Go is not differentiable: only the exported function and its gradient are generated from the engine's IR.";
pub const LIMITATION_FLOAT: &str =
    "Floating-point results are not identical across languages or platforms: the fixtures declare a tolerance instead of exact equality.";

pub fn root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
}

pub fn read(relative: &str) -> Option<String> {
    std::fs::read_to_string(root().join(relative)).ok()
}

/// Directory names that are never part of the published tree: `.git`,
/// `target`, and every simple name listed in `.gitignore`.
pub fn ignored_names() -> Vec<String> {
    let mut names = vec![".git".to_string(), "target".to_string()];
    if let Some(text) = read(".gitignore") {
        for line in text.lines() {
            let line = line.trim();
            if line.is_empty() || line.starts_with('#') || line.starts_with('!') {
                continue;
            }
            let name = line.trim_start_matches('/').trim_end_matches('/');
            if !name.is_empty() && !name.contains('/') && !name.contains('*') {
                names.push(name.to_string());
            }
        }
    }
    names
}

/// Every regular file under the root, skipping ignored directory names.
pub fn walk(skip: &[String]) -> Vec<PathBuf> {
    walk_under(&root(), skip)
}

/// Every regular file under `base`, skipping ignored directory names.
pub fn walk_under(base: &Path, skip: &[String]) -> Vec<PathBuf> {
    let mut found = Vec::new();
    let mut pending = vec![base.to_path_buf()];
    while let Some(dir) = pending.pop() {
        let Ok(entries) = std::fs::read_dir(&dir) else {
            continue;
        };
        for entry in entries.flatten() {
            let path = entry.path();
            let name = entry.file_name().to_string_lossy().to_string();
            let Ok(kind) = entry.file_type() else {
                continue;
            };
            if kind.is_symlink() {
                continue;
            }
            if kind.is_dir() {
                if !skip.iter().any(|s| s == &name) {
                    pending.push(path);
                }
            } else if kind.is_file() {
                found.push(path);
            }
        }
    }
    found.sort();
    found
}

/// Text files only (valid UTF-8, at most 8 MiB), as (relative path, text).
pub fn text_files(skip: &[String]) -> Vec<(String, String)> {
    let base = root();
    walk(skip)
        .into_iter()
        .filter_map(|path| {
            let bytes = std::fs::read(&path).ok()?;
            if bytes.len() > 8 * 1024 * 1024 {
                return None;
            }
            let text = String::from_utf8(bytes).ok()?;
            Some((relative(&base, &path), text))
        })
        .collect()
}

/// Rust sources under `src/`, as (relative path, text).
pub fn src_files() -> Vec<(String, String)> {
    let base = root();
    walk_under(&base.join("src"), &[])
        .into_iter()
        .filter(|p| p.extension().is_some_and(|e| e == "rs"))
        .filter_map(|p| Some((relative(&base, &p), std::fs::read_to_string(&p).ok()?)))
        .collect()
}

pub fn relative(base: &Path, path: &Path) -> String {
    path.strip_prefix(base)
        .unwrap_or(path)
        .to_string_lossy()
        .replace('\\', "/")
}

/// The value of a `key = "value"` line inside `[package]` of `Cargo.toml`.
pub fn package_field(cargo_toml: &str, key: &str) -> Option<String> {
    let mut in_package = false;
    for line in cargo_toml.lines() {
        let line = line.trim();
        if line.starts_with('[') {
            in_package = line == "[package]";
            continue;
        }
        if !in_package || !line.starts_with(key) {
            continue;
        }
        let rest = line[key.len()..].trim_start();
        let Some(rest) = rest.strip_prefix('=') else {
            continue;
        };
        let value = rest.trim().trim_matches('"').trim();
        return Some(value.to_string());
    }
    None
}

/// The lines of a `[section]` of a TOML file, up to the next section header.
pub fn section_lines<'a>(toml: &'a str, section: &str) -> Option<Vec<&'a str>> {
    let mut lines = None;
    for line in toml.lines() {
        let trimmed = line.trim();
        if trimmed.starts_with('[') {
            if lines.is_some() {
                break;
            }
            if trimmed == section {
                lines = Some(Vec::new());
            }
            continue;
        }
        if let Some(collected) = lines.as_mut() {
            collected.push(trimmed);
        }
    }
    lines
}

// ---------------------------------------------------------------------------
// JSON, read only
// ---------------------------------------------------------------------------

#[derive(Clone, Debug, PartialEq)]
pub enum Json {
    Null,
    Bool(bool),
    Num(f64),
    Str(String),
    Arr(Vec<Json>),
    Obj(Vec<(String, Json)>),
}

impl Json {
    pub fn parse(text: &str) -> Result<Json, String> {
        let mut p = JsonParser {
            s: text.as_bytes(),
            i: 0,
        };
        p.ws();
        let v = p.value()?;
        p.ws();
        if p.i != p.s.len() {
            return Err(format!("trailing characters at byte {}", p.i));
        }
        Ok(v)
    }

    pub fn get(&self, key: &str) -> Option<&Json> {
        match self {
            Json::Obj(pairs) => pairs.iter().find(|(k, _)| k == key).map(|(_, v)| v),
            _ => None,
        }
    }

    /// `a.b.c` and `items.0.name` style access.
    pub fn path(&self, path: &str) -> Option<&Json> {
        let mut cur = self;
        for part in path.split('.') {
            cur = match cur {
                Json::Obj(_) => cur.get(part)?,
                Json::Arr(items) => items.get(part.parse::<usize>().ok()?)?,
                _ => return None,
            };
        }
        Some(cur)
    }

    pub fn as_f64(&self) -> Option<f64> {
        match self {
            Json::Num(n) => Some(*n),
            _ => None,
        }
    }
    pub fn as_str(&self) -> Option<&str> {
        match self {
            Json::Str(s) => Some(s),
            _ => None,
        }
    }
    pub fn as_bool(&self) -> Option<bool> {
        match self {
            Json::Bool(b) => Some(*b),
            _ => None,
        }
    }
    pub fn as_arr(&self) -> Option<&Vec<Json>> {
        match self {
            Json::Arr(a) => Some(a),
            _ => None,
        }
    }
    pub fn as_obj(&self) -> Option<&Vec<(String, Json)>> {
        match self {
            Json::Obj(o) => Some(o),
            _ => None,
        }
    }
    pub fn is_null(&self) -> bool {
        matches!(self, Json::Null)
    }
    /// A string field, or a panic naming the field.
    pub fn str_field(&self, key: &str) -> &str {
        self.get(key)
            .and_then(Json::as_str)
            .unwrap_or_else(|| panic!("expected a string field `{key}` in {self:?}"))
    }
    pub fn num_field(&self, key: &str) -> f64 {
        self.get(key)
            .and_then(Json::as_f64)
            .unwrap_or_else(|| panic!("expected a number field `{key}` in {self:?}"))
    }
    /// Serialise back (for building requests from parsed values).
    pub fn render(&self) -> String {
        let mut out = String::new();
        self.render_into(&mut out);
        out
    }
    fn render_into(&self, out: &mut String) {
        match self {
            Json::Null => out.push_str("null"),
            Json::Bool(b) => out.push_str(if *b { "true" } else { "false" }),
            Json::Num(n) => {
                if n.is_finite() {
                    if n.fract() == 0.0 && n.abs() < 1e15 {
                        out.push_str(&format!("{}", *n as i64));
                    } else {
                        out.push_str(&format!("{n:?}"));
                    }
                } else {
                    out.push_str("null");
                }
            }
            Json::Str(s) => json_string(s, out),
            Json::Arr(items) => {
                out.push('[');
                for (i, item) in items.iter().enumerate() {
                    if i > 0 {
                        out.push(',');
                    }
                    item.render_into(out);
                }
                out.push(']');
            }
            Json::Obj(pairs) => {
                out.push('{');
                for (i, (k, v)) in pairs.iter().enumerate() {
                    if i > 0 {
                        out.push(',');
                    }
                    json_string(k, out);
                    out.push(':');
                    v.render_into(out);
                }
                out.push('}');
            }
        }
    }
}

pub fn json_string(s: &str, out: &mut String) {
    out.push('"');
    for c in s.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            c if (c as u32) < 0x20 => out.push_str(&format!("\\u{:04x}", c as u32)),
            c => out.push(c),
        }
    }
    out.push('"');
}

/// Quote a string as a JSON literal.
pub fn quote(s: &str) -> String {
    let mut out = String::new();
    json_string(s, &mut out);
    out
}

struct JsonParser<'a> {
    s: &'a [u8],
    i: usize,
}

impl JsonParser<'_> {
    fn ws(&mut self) {
        while self.i < self.s.len() && matches!(self.s[self.i], b' ' | b'\n' | b'\r' | b'\t') {
            self.i += 1;
        }
    }
    fn peek(&self) -> Option<u8> {
        self.s.get(self.i).copied()
    }
    fn expect(&mut self, lit: &str) -> Result<(), String> {
        if self.s[self.i..].starts_with(lit.as_bytes()) {
            self.i += lit.len();
            Ok(())
        } else {
            Err(format!("expected `{lit}` at byte {}", self.i))
        }
    }
    fn value(&mut self) -> Result<Json, String> {
        match self.peek() {
            None => Err("unexpected end of JSON".into()),
            Some(b'{') => {
                self.i += 1;
                let mut pairs = Vec::new();
                self.ws();
                if self.peek() == Some(b'}') {
                    self.i += 1;
                    return Ok(Json::Obj(pairs));
                }
                loop {
                    self.ws();
                    let key = self.string()?;
                    self.ws();
                    self.expect(":")?;
                    self.ws();
                    let v = self.value()?;
                    pairs.push((key, v));
                    self.ws();
                    match self.peek() {
                        Some(b',') => self.i += 1,
                        Some(b'}') => {
                            self.i += 1;
                            return Ok(Json::Obj(pairs));
                        }
                        _ => return Err(format!("expected `,` or `}}` at byte {}", self.i)),
                    }
                }
            }
            Some(b'[') => {
                self.i += 1;
                let mut items = Vec::new();
                self.ws();
                if self.peek() == Some(b']') {
                    self.i += 1;
                    return Ok(Json::Arr(items));
                }
                loop {
                    self.ws();
                    items.push(self.value()?);
                    self.ws();
                    match self.peek() {
                        Some(b',') => self.i += 1,
                        Some(b']') => {
                            self.i += 1;
                            return Ok(Json::Arr(items));
                        }
                        _ => return Err(format!("expected `,` or `]` at byte {}", self.i)),
                    }
                }
            }
            Some(b'"') => Ok(Json::Str(self.string()?)),
            Some(b't') => {
                self.expect("true")?;
                Ok(Json::Bool(true))
            }
            Some(b'f') => {
                self.expect("false")?;
                Ok(Json::Bool(false))
            }
            Some(b'n') => {
                self.expect("null")?;
                Ok(Json::Null)
            }
            Some(_) => self.number(),
        }
    }
    fn number(&mut self) -> Result<Json, String> {
        let start = self.i;
        while self.i < self.s.len()
            && matches!(
                self.s[self.i],
                b'-' | b'+' | b'.' | b'e' | b'E' | b'0'..=b'9'
            )
        {
            self.i += 1;
        }
        let text = std::str::from_utf8(&self.s[start..self.i]).map_err(|e| e.to_string())?;
        text.parse::<f64>()
            .map(Json::Num)
            .map_err(|_| format!("bad number `{text}` at byte {start}"))
    }
    fn string(&mut self) -> Result<String, String> {
        self.expect("\"")?;
        let mut out = String::new();
        loop {
            let Some(c) = self.peek() else {
                return Err("unterminated string".into());
            };
            self.i += 1;
            match c {
                b'"' => return Ok(out),
                b'\\' => {
                    let Some(e) = self.peek() else {
                        return Err("bad escape".into());
                    };
                    self.i += 1;
                    match e {
                        b'"' => out.push('"'),
                        b'\\' => out.push('\\'),
                        b'/' => out.push('/'),
                        b'b' => out.push('\u{8}'),
                        b'f' => out.push('\u{c}'),
                        b'n' => out.push('\n'),
                        b'r' => out.push('\r'),
                        b't' => out.push('\t'),
                        b'u' => {
                            let mut code = self.hex4()?;
                            if (0xD800..0xDC00).contains(&code) {
                                self.expect("\\u")?;
                                let low = self.hex4()?;
                                code =
                                    0x10000 + ((code - 0xD800) << 10) + (low.wrapping_sub(0xDC00));
                            }
                            out.push(char::from_u32(code).unwrap_or('\u{FFFD}'));
                        }
                        _ => return Err("bad escape".into()),
                    }
                }
                _ => {
                    // Copy one UTF-8 sequence.
                    let len = utf8_len(c);
                    let end = (self.i - 1 + len).min(self.s.len());
                    let piece =
                        std::str::from_utf8(&self.s[self.i - 1..end]).map_err(|e| e.to_string())?;
                    out.push_str(piece);
                    self.i = end;
                }
            }
        }
    }
    fn hex4(&mut self) -> Result<u32, String> {
        if self.i + 4 > self.s.len() {
            return Err("short \\u escape".into());
        }
        let text = std::str::from_utf8(&self.s[self.i..self.i + 4]).map_err(|e| e.to_string())?;
        self.i += 4;
        u32::from_str_radix(text, 16).map_err(|e| e.to_string())
    }
}

fn utf8_len(first: u8) -> usize {
    if first < 0x80 {
        1
    } else if first >> 5 == 0b110 {
        2
    } else if first >> 4 == 0b1110 {
        3
    } else {
        4
    }
}

// ---------------------------------------------------------------------------
// Processes, bounded
// ---------------------------------------------------------------------------

/// What one bounded child run produced.
#[derive(Debug)]
pub struct Run {
    pub status: Option<i32>,
    pub stdout: String,
    pub stderr: String,
    pub timed_out: bool,
    pub elapsed: Duration,
}

impl Run {
    pub fn summary(&self) -> String {
        format!(
            "status={:?} timed_out={} elapsed={:?}\n--- stdout ---\n{}\n--- stderr ---\n{}",
            self.status,
            self.timed_out,
            self.elapsed,
            truncate(&self.stdout, 4000),
            truncate(&self.stderr, 4000)
        )
    }
    /// Parse stdout as JSON, or panic with the whole run.
    pub fn json(&self) -> Json {
        Json::parse(self.stdout.trim())
            .unwrap_or_else(|e| panic!("stdout is not one JSON object ({e}):\n{}", self.summary()))
    }
    pub fn panicked(&self) -> bool {
        self.status == Some(101) || self.stderr.contains("panicked at")
    }
}

pub fn truncate(s: &str, max: usize) -> String {
    if s.len() <= max {
        s.to_string()
    } else {
        let mut end = max;
        while !s.is_char_boundary(end) {
            end -= 1;
        }
        format!("{}… [{} bytes total]", &s[..end], s.len())
    }
}

thread_local! {
    /// When the current test first ran a subprocess. Rust gives each test its
    /// own thread, so this is that test's own clock.
    static TEST_STARTED: std::cell::Cell<Option<Instant>> =
        const { std::cell::Cell::new(None) };
}

/// C3, charged automatically on every subprocess this module runs.
///
/// `Bound` asks explicitly and only four tests remember to. This is the same
/// rule applied to every test without being asked, which is what the criterion
/// says. It is checked *before* each run, so the bound it enforces is the
/// declared budget plus at most one subprocess timeout: a test cannot creep
/// past the budget by making many individually fast calls, which was the way
/// past it.
fn charge_time_budget() {
    TEST_STARTED.with(|cell| match cell.get() {
        None => cell.set(Some(Instant::now())),
        Some(started) => {
            let elapsed = started.elapsed();
            assert!(
                elapsed <= Duration::from_secs(PER_TEST_BOUND_SECS),
                "C3: this test has been running {elapsed:?}, over the declared \
                 {PER_TEST_BOUND_SECS} s per-test bound"
            );
        }
    });
}

/// Run a program with a time bound. The environment is inherited unless
/// `clear_env` is set; `env` entries are added on top. Stdin receives `input`
/// and is then closed.
pub fn run_with(
    program: &Path,
    args: &[&str],
    cwd: &Path,
    clear_env: bool,
    env: &[(&str, &str)],
    input: Option<&str>,
    timeout: Duration,
) -> Run {
    charge_time_budget();
    let mut cmd = Command::new(program);
    cmd.args(args)
        .current_dir(cwd)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    if clear_env {
        cmd.env_clear();
        if let Some(tmp) = std::env::var_os("TMPDIR") {
            cmd.env("TMPDIR", tmp);
        }
    }
    for (k, v) in env {
        cmd.env(k, v);
    }
    let started = Instant::now();
    let mut child = match cmd.spawn() {
        Ok(child) => child,
        Err(error) => {
            return Run {
                status: None,
                stdout: String::new(),
                stderr: format!("could not start {}: {error}", program.display()),
                timed_out: false,
                elapsed: started.elapsed(),
            }
        }
    };
    let mut stdin = child.stdin.take();
    let input_owned = input.map(str::to_owned);
    let writer = std::thread::spawn(move || {
        if let (Some(mut stdin), Some(text)) = (stdin.take(), input_owned) {
            let _ = stdin.write_all(text.as_bytes());
        }
        // Dropping closes stdin either way.
    });
    let mut out = child.stdout.take().expect("piped stdout");
    let mut err = child.stderr.take().expect("piped stderr");
    let out_reader = std::thread::spawn(move || {
        let mut buf = Vec::new();
        let _ = out.read_to_end(&mut buf);
        buf
    });
    let err_reader = std::thread::spawn(move || {
        let mut buf = Vec::new();
        let _ = err.read_to_end(&mut buf);
        buf
    });
    let mut timed_out = false;
    let status = loop {
        match child.try_wait() {
            Ok(Some(status)) => break status.code(),
            Ok(None) => {
                if started.elapsed() > timeout {
                    timed_out = true;
                    let _ = child.kill();
                    let _ = child.wait();
                    break None;
                }
                std::thread::sleep(Duration::from_millis(10));
            }
            Err(_) => break None,
        }
    };
    let _ = writer.join();
    let stdout = String::from_utf8_lossy(&out_reader.join().unwrap_or_default()).into_owned();
    let stderr = String::from_utf8_lossy(&err_reader.join().unwrap_or_default()).into_owned();
    Run {
        status,
        stdout,
        stderr,
        timed_out,
        elapsed: started.elapsed(),
    }
}

/// The `catalyst` binary under test.
///
/// Cargo sets `CARGO_BIN_EXE_catalyst` for integration tests once the package
/// has a `src/main.rs`; until then the oracle fails with the next step rather
/// than failing to compile.
pub fn catalyst_bin() -> PathBuf {
    if let Some(path) = option_env!("CARGO_BIN_EXE_catalyst") {
        return PathBuf::from(path);
    }
    if let Ok(exe) = std::env::current_exe() {
        if let Some(candidate) = exe
            .parent()
            .and_then(Path::parent)
            .map(|d| d.join("catalyst"))
        {
            if candidate.is_file() {
                return candidate;
            }
        }
    }
    panic!("the `catalyst` binary does not exist: add src/main.rs so the crate builds a `catalyst` binary (docs/interface.md §0)");
}

/// Run `catalyst ARGS` in `cwd` with the inherited environment plus `env`.
pub fn catalyst(args: &[&str], cwd: &Path, env: &[(&str, &str)]) -> Run {
    run_with(
        &catalyst_bin(),
        args,
        cwd,
        false,
        env,
        None,
        Duration::from_secs(PER_TEST_BOUND_SECS),
    )
}

/// Run `catalyst ARGS` with `input` on stdin.
pub fn catalyst_stdin(args: &[&str], cwd: &Path, input: &str) -> Run {
    run_with(
        &catalyst_bin(),
        args,
        cwd,
        false,
        &[],
        Some(input),
        Duration::from_secs(PER_TEST_BOUND_SECS),
    )
}

/// Assert a structured refusal: exit code 2, `ok:false`, a code with the
/// given prefix, a non-empty `remedy`, and no panic. Returns the object.
pub fn assert_refusal(run: &Run, code_prefix: &str) -> Json {
    assert!(
        !run.panicked(),
        "the command panicked instead of refusing:\n{}",
        run.summary()
    );
    assert_eq!(
        run.status,
        Some(2),
        "a refusal exits with code 2:\n{}",
        run.summary()
    );
    let json = run.json();
    assert_eq!(
        json.get("ok").and_then(Json::as_bool),
        Some(false),
        "a refusal carries ok:false:\n{}",
        run.summary()
    );
    let code = json.str_field("code");
    assert!(
        code.starts_with(code_prefix),
        "refusal code `{code}` should start with `{code_prefix}`:\n{}",
        run.summary()
    );
    let remedy = json.str_field("remedy");
    assert!(
        remedy.trim().len() >= 8,
        "a refusal must carry a remedy that names the next step:\n{}",
        run.summary()
    );
    json
}

/// Assert success: exit 0, `ok:true`, no panic. Returns the object.
pub fn assert_ok(run: &Run) -> Json {
    assert!(!run.panicked(), "the command panicked:\n{}", run.summary());
    assert_eq!(run.status, Some(0), "expected exit 0:\n{}", run.summary());
    let json = run.json();
    assert_eq!(
        json.get("ok").and_then(Json::as_bool),
        Some(true),
        "expected ok:true:\n{}",
        run.summary()
    );
    json
}

// ---------------------------------------------------------------------------
// Scratch space and Go
// ---------------------------------------------------------------------------

static COUNTER: AtomicUsize = AtomicUsize::new(0);

/// A fresh, empty directory under the scratch space the check lane grants.
pub fn scratch(name: &str) -> PathBuf {
    let n = COUNTER.fetch_add(1, Ordering::SeqCst);
    let dir =
        std::env::temp_dir().join(format!("catalyst-oracle-{name}-{}-{n}", std::process::id()));
    std::fs::create_dir_all(&dir).expect("create scratch directory");
    dir
}

pub fn write(path: &Path, contents: &str) {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).expect("create parent");
    }
    std::fs::write(path, contents).unwrap_or_else(|e| panic!("write {}: {e}", path.display()));
}

pub fn read_file(path: &Path) -> String {
    std::fs::read_to_string(path).unwrap_or_else(|e| panic!("read {}: {e}", path.display()))
}

/// The Go toolchain environment the lane needs: caches under `$TMPDIR`
/// (`HOME` is read-only there), no cgo, no toolchain download, no proxy.
pub fn go_env() -> Vec<(String, String)> {
    let base = std::env::temp_dir().join("catalyst-oracle-go");
    let _ = std::fs::create_dir_all(&base);
    vec![
        (
            "GOCACHE".into(),
            base.join("cache").to_string_lossy().into_owned(),
        ),
        (
            "GOMODCACHE".into(),
            base.join("modcache").to_string_lossy().into_owned(),
        ),
        (
            "GOPATH".into(),
            base.join("gopath").to_string_lossy().into_owned(),
        ),
        (
            "GOTMPDIR".into(),
            base.join("tmp").to_string_lossy().into_owned(),
        ),
        ("CGO_ENABLED".into(), "0".into()),
        ("GOTOOLCHAIN".into(), "local".into()),
        ("GOPROXY".into(), "off".into()),
        ("GOFLAGS".into(), "-mod=mod".into()),
    ]
}

/// Run `go ARGS` in `cwd` with that environment.
pub fn go(args: &[&str], cwd: &Path) -> Run {
    let env = go_env();
    let _ = std::fs::create_dir_all(std::env::temp_dir().join("catalyst-oracle-go").join("tmp"));
    let pairs: Vec<(&str, &str)> = env.iter().map(|(k, v)| (k.as_str(), v.as_str())).collect();
    let program = which("go").unwrap_or_else(|| PathBuf::from("go"));
    run_with(
        &program,
        args,
        cwd,
        false,
        &pairs,
        None,
        Duration::from_secs(PER_TEST_BOUND_SECS),
    )
}

/// Resolve a program on `PATH`, or on the usual system directories when the
/// lane passes no `PATH` at all.
pub fn which(name: &str) -> Option<PathBuf> {
    let mut dirs: Vec<PathBuf> = std::env::var_os("PATH")
        .map(|p| std::env::split_paths(&p).collect())
        .unwrap_or_default();
    for extra in ["/usr/local/go/bin", "/usr/local/bin", "/usr/bin", "/bin"] {
        dirs.push(PathBuf::from(extra));
    }
    dirs.into_iter().map(|d| d.join(name)).find(|p| p.is_file())
}

/// Import lines of a Go source file (single and grouped forms), as the quoted
/// path without quotes.
pub fn go_imports(source: &str) -> Vec<String> {
    let mut imports = Vec::new();
    let mut in_group = false;
    for line in source.lines() {
        let t = line.trim();
        if in_group {
            if t == ")" {
                in_group = false;
                continue;
            }
            if let Some(q) = t.split('"').nth(1) {
                imports.push(q.to_string());
            }
            continue;
        }
        if t == "import (" {
            in_group = true;
        } else if let Some(rest) = t.strip_prefix("import ") {
            if let Some(q) = rest.split('"').nth(1) {
                imports.push(q.to_string());
            }
        }
    }
    imports
}

// ---------------------------------------------------------------------------
// Numbers, problems, key scripts, time bounds
// ---------------------------------------------------------------------------

/// Criterion 1.4's comparison: within relative `rel` or absolute `abs`,
/// whichever is larger. Two non-finite values of the same kind agree.
pub fn within(got: f64, want: f64, rel: f64, abs: f64) -> bool {
    if got.is_nan() && want.is_nan() {
        return true;
    }
    if !got.is_finite() || !want.is_finite() {
        return got == want;
    }
    let allowed = abs.max(rel * want.abs().max(got.abs()));
    (got - want).abs() <= allowed
}

/// One input of a problem: name, value, min, max, unit.
pub type Input<'a> = (&'a str, f64, f64, f64, &'a str);

/// A `catalyst.problem.v1` document.
pub fn problem_json(name: &str, goal: &str, function: &str, inputs: &[Input]) -> String {
    let mut s = String::from("{\n  \"schema\": \"catalyst.problem.v1\",\n  \"name\": ");
    s.push_str(&quote(name));
    s.push_str(",\n  \"goal\": ");
    s.push_str(&quote(goal));
    s.push_str(",\n  \"function\": ");
    s.push_str(&quote(function));
    s.push_str(",\n  \"inputs\": {");
    for (i, (n, v, _, _, _)) in inputs.iter().enumerate() {
        if i > 0 {
            s.push_str(", ");
        }
        s.push_str(&format!("{}: {v:?}", quote(n)));
    }
    s.push_str("},\n  \"domains\": {");
    for (i, (n, _, lo, hi, unit)) in inputs.iter().enumerate() {
        if i > 0 {
            s.push_str(", ");
        }
        s.push_str(&format!(
            "{}: {{\"min\": {lo:?}, \"max\": {hi:?}, \"unit\": {}}}",
            quote(n),
            quote(unit)
        ));
    }
    s.push_str("}\n}\n");
    s
}

/// The damped-spring settling goal agreed at clarification.
pub const SPRING_FUNCTION: &str =
    "func spring(k, c) = 8 / c + 100 * exp(0 - 3.141592653589793 * c / sqrt(4 * k - c * c))";
pub const SPRING_INPUTS: &[Input] = &[
    ("k", 12.0, 1.0, 100.0, "N/m"),
    ("c", 1.5, 0.1, 1.9, "N*s/m"),
];

pub fn spring_problem() -> String {
    problem_json(
        "spring",
        "settle within 0.4 s with less than 5 % overshoot",
        SPRING_FUNCTION,
        SPRING_INPUTS,
    )
}

/// The simplest problem of the agreed subset.
pub fn simple_problem() -> String {
    problem_json(
        "simple",
        "the product plus a wave",
        "func simple(x, y) = x * y + sin(x)",
        &[("x", 0.7, -3.0, 3.0, ""), ("y", 1.3, -2.0, 2.0, "")],
    )
}

/// A key script for `catalyst tui --keys`, one event per line.
pub fn keys(events: &[&str]) -> String {
    let mut s = String::new();
    for e in events {
        s.push_str(e);
        s.push('\n');
    }
    s
}

/// The key script that types a whole problem by hand through the guided
/// flow: Goal, then per parameter value/min/max/unit, then Run, then Results,
/// then Export to `out`.
pub fn typed_flow(function: &str, inputs: &[Input], out: &str) -> String {
    let mut events: Vec<String> = vec![format!("text:{function}"), "enter".into()];
    for (i, (_, v, lo, hi, unit)) in inputs.iter().enumerate() {
        if i > 0 {
            events.push("tab".into());
        }
        events.push(format!("text:{v}"));
        events.push("tab".into());
        events.push(format!("text:{lo}"));
        events.push("tab".into());
        events.push(format!("text:{hi}"));
        events.push("tab".into());
        events.push(format!("text:{unit}"));
    }
    events.push("enter".into()); // Inputs -> Run
    events.push("enter".into()); // Run -> Results
    events.push("enter".into()); // Results -> Export
    events.push(format!("text:{out}"));
    events.push("enter".into()); // write the export
    let refs: Vec<&str> = events.iter().map(String::as_str).collect();
    keys(&refs)
}

/// Frames of a headless transcript, as (header line, body).
pub fn frames(transcript: &str) -> Vec<(String, String)> {
    let mut out: Vec<(String, String)> = Vec::new();
    for line in transcript.lines() {
        if line.starts_with("--- frame ") {
            out.push((line.to_string(), String::new()));
        } else if let Some(last) = out.last_mut() {
            last.1.push_str(line);
            last.1.push('\n');
        }
    }
    out
}

/// Criterion C3: a stopwatch that fails the test past the declared bound.
pub struct Bound {
    started: Instant,
    what: &'static str,
}

impl Bound {
    pub fn start(what: &'static str) -> Bound {
        Bound {
            started: Instant::now(),
            what,
        }
    }
    pub fn check(&self) {
        let elapsed = self.started.elapsed();
        assert!(
            elapsed <= Duration::from_secs(PER_TEST_BOUND_SECS),
            "C3: `{}` took {elapsed:?}, over the declared {PER_TEST_BOUND_SECS} s per-test bound",
            self.what
        );
    }
}

/// A directory listing (names only, sorted), to prove a refused request
/// changed nothing.
pub fn listing(dir: &Path) -> Vec<String> {
    let mut names: Vec<String> = std::fs::read_dir(dir)
        .map(|entries| {
            entries
                .flatten()
                .map(|e| e.file_name().to_string_lossy().into_owned())
                .collect()
        })
        .unwrap_or_default();
    names.sort();
    names
}

// ---------------------------------------------------------------------------
// Digests and fixtures, for the gradient-compiler oracles
// ---------------------------------------------------------------------------

/// SHA-256 of `data` as 64 lowercase hex characters. Written here because the
/// oracles have no crate to reach for, and checked against the published test
/// vectors in `acceptance_10_gradient_compiler.rs`.
pub fn sha256_hex(data: &[u8]) -> String {
    const K: [u32; 64] = [
        0x428a2f98, 0x71374491, 0xb5c0fbcf, 0xe9b5dba5, 0x3956c25b, 0x59f111f1, 0x923f82a4,
        0xab1c5ed5, 0xd807aa98, 0x12835b01, 0x243185be, 0x550c7dc3, 0x72be5d74, 0x80deb1fe,
        0x9bdc06a7, 0xc19bf174, 0xe49b69c1, 0xefbe4786, 0x0fc19dc6, 0x240ca1cc, 0x2de92c6f,
        0x4a7484aa, 0x5cb0a9dc, 0x76f988da, 0x983e5152, 0xa831c66d, 0xb00327c8, 0xbf597fc7,
        0xc6e00bf3, 0xd5a79147, 0x06ca6351, 0x14292967, 0x27b70a85, 0x2e1b2138, 0x4d2c6dfc,
        0x53380d13, 0x650a7354, 0x766a0abb, 0x81c2c92e, 0x92722c85, 0xa2bfe8a1, 0xa81a664b,
        0xc24b8b70, 0xc76c51a3, 0xd192e819, 0xd6990624, 0xf40e3585, 0x106aa070, 0x19a4c116,
        0x1e376c08, 0x2748774c, 0x34b0bcb5, 0x391c0cb3, 0x4ed8aa4a, 0x5b9cca4f, 0x682e6ff3,
        0x748f82ee, 0x78a5636f, 0x84c87814, 0x8cc70208, 0x90befffa, 0xa4506ceb, 0xbef9a3f7,
        0xc67178f2,
    ];
    let mut h: [u32; 8] = [
        0x6a09e667, 0xbb67ae85, 0x3c6ef372, 0xa54ff53a, 0x510e527f, 0x9b05688c, 0x1f83d9ab,
        0x5be0cd19,
    ];
    let mut msg = data.to_vec();
    let bit_len = (data.len() as u64).wrapping_mul(8);
    msg.push(0x80);
    while msg.len() % 64 != 56 {
        msg.push(0);
    }
    msg.extend_from_slice(&bit_len.to_be_bytes());
    for chunk in msg.chunks(64) {
        let mut w = [0u32; 64];
        for (word, bytes) in w.iter_mut().zip(chunk.chunks(4)) {
            *word = u32::from_be_bytes([bytes[0], bytes[1], bytes[2], bytes[3]]);
        }
        for i in 16..64 {
            let s0 = w[i - 15].rotate_right(7) ^ w[i - 15].rotate_right(18) ^ (w[i - 15] >> 3);
            let s1 = w[i - 2].rotate_right(17) ^ w[i - 2].rotate_right(19) ^ (w[i - 2] >> 10);
            w[i] = w[i - 16]
                .wrapping_add(s0)
                .wrapping_add(w[i - 7])
                .wrapping_add(s1);
        }
        let [mut a, mut b, mut c, mut d, mut e, mut f, mut g, mut hh] = h;
        for (k, word) in K.iter().zip(w.iter()) {
            let s1 = e.rotate_right(6) ^ e.rotate_right(11) ^ e.rotate_right(25);
            let ch = (e & f) ^ (!e & g);
            let t1 = hh
                .wrapping_add(s1)
                .wrapping_add(ch)
                .wrapping_add(*k)
                .wrapping_add(*word);
            let s0 = a.rotate_right(2) ^ a.rotate_right(13) ^ a.rotate_right(22);
            let maj = (a & b) ^ (a & c) ^ (b & c);
            let t2 = s0.wrapping_add(maj);
            hh = g;
            g = f;
            f = e;
            e = d.wrapping_add(t1);
            d = c;
            c = b;
            b = a;
            a = t1.wrapping_add(t2);
        }
        for (slot, add) in h.iter_mut().zip([a, b, c, d, e, f, g, hh]) {
            *slot = slot.wrapping_add(add);
        }
    }
    let mut out = String::with_capacity(64);
    for word in h {
        out.push_str(&format!("{word:08x}"));
    }
    out
}

/// A file under `tests/fixtures/`, read in full.
pub fn fixture(relative: &str) -> String {
    read_file(&root().join("tests").join("fixtures").join(relative))
}

/// The `version` of the package, from `Cargo.toml`.
pub fn crate_version() -> String {
    package_field(&read("Cargo.toml").unwrap_or_default(), "version").unwrap_or_default()
}
