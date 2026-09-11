//! C8 — a licence the operator chose, and manifest metadata that matches it.
mod acceptance_common;
use acceptance_common as common;

#[test]
fn c8_license_file_and_manifest_metadata_are_present_and_consistent() {
    let manifest = common::read("Cargo.toml").expect("C8: Cargo.toml must exist");
    let license = common::package_field(&manifest, "license")
        .filter(|v| !v.is_empty())
        .expect("C8: set `license = \"<the operator's choice>\"` in [package]");
    for key in ["description", "repository"] {
        let value = common::package_field(&manifest, key).unwrap_or_default();
        assert!(!value.is_empty(), "C8: set `{key}` in [package]");
    }
    let text = common::read("LICENSE")
        .expect("C8: add a LICENSE file at the workspace root with the operator's chosen licence");
    assert!(
        text.trim().len() >= 200,
        "C8: LICENSE is too short to be a licence text; paste the full text of {license}"
    );
    let family = license
        .split(['-', ' '])
        .next()
        .unwrap_or("")
        .to_ascii_lowercase();
    let lower = text.to_ascii_lowercase();
    // An SPDX family such as `sspl` or `gpl` never appears literally in its own
    // licence text, which names itself in full; accept either spelling.
    let full_name = match family.as_str() {
        "sspl" => "server side public license",
        "agpl" => "affero general public license",
        "gpl" => "general public license",
        "lgpl" => "lesser general public license",
        "mpl" => "mozilla public license",
        "bsd" => "redistribution and use in source and binary forms",
        "apache" => "apache license",
        "mit" => "permission is hereby granted, free of charge",
        _ => "",
    };
    let named = lower.contains(&family) || (!full_name.is_empty() && lower.contains(full_name));
    assert!(
        family.is_empty() || named,
        "C8: LICENSE does not mention {license}, which Cargo.toml declares; make the two agree"
    );
}
