//! C11 (Rust half) — the crate forbids unsafe code and contains none.
mod acceptance_common;
use acceptance_common as common;

#[test]
fn c11_crate_root_forbids_unsafe_code() {
    let root = common::read("src/lib.rs")
        .or_else(|| common::read("src/main.rs"))
        .expect("C11: src/lib.rs (or src/main.rs) must exist");
    let has = root
        .lines()
        .map(str::trim)
        .any(|l| l == "#![forbid(unsafe_code)]");
    assert!(
        has,
        "C11: put `#![forbid(unsafe_code)]` at the top of the crate root"
    );
}

#[test]
fn c11_no_unsafe_anywhere_in_src() {
    let base = common::root().join("src");
    let mut pending = vec![base.clone()];
    let mut hits = Vec::new();
    while let Some(dir) = pending.pop() {
        let Ok(entries) = std::fs::read_dir(&dir) else {
            continue;
        };
        for entry in entries.flatten() {
            let path = entry.path();
            if path.is_dir() {
                pending.push(path);
            } else if path.extension().is_some_and(|e| e == "rs") {
                let text = std::fs::read_to_string(&path).unwrap_or_default();
                for (number, line) in text.lines().enumerate() {
                    let code = line.split("//").next().unwrap_or("");
                    if code.contains("unsafe") && !code.contains("forbid(unsafe_code)") {
                        hits.push(format!(
                            "{}:{}",
                            common::relative(&common::root(), &path),
                            number + 1
                        ));
                    }
                }
            }
        }
    }
    assert!(
        hits.is_empty(),
        "C11: remove `unsafe` from these places: {hits:?}"
    );
}
