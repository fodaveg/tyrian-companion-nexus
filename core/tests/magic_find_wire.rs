//! `mf1` lines against the fixture shared byte for byte with the plugin
//! (`src/alerts/__fixtures__/mf1.json` there). The loop that sends them is in
//! `tests/client_live.rs`.

use serde_json::{json, Value};
use tyrian_companion_nexus_core::protocol::{build_magic_find_line, parse_server_line, ServerLine, MAX_LINE_BYTES};

const NONCE: &str = "AQEBAQEBAQEBAQEBAQEBAQ";
const OTHER_NONCE: &str = "Yk3m1Qw9Lr0aT7yUc2Vb5g";

fn frames() -> Vec<Value> {
    let text = include_str!("fixtures/mf1.json");
    serde_json::from_str::<Value>(text).unwrap()["frames"].as_array().unwrap().clone()
}

#[test]
fn the_fixture_capability_is_read_for_its_nonce() {
    let cap = &frames()[0];
    assert_eq!(parse_server_line(&cap.to_string()), ServerLine::MagicFindCapability(NONCE.into()));
}

#[test]
fn a_capability_of_another_tag_or_shape_is_dropped_without_closing() {
    let mut other_tag = frames()[0].clone();
    other_tag["tag"] = json!("mf2");
    let mut extra = frames()[0].clone();
    extra["command"] = json!("x");
    let mut v2 = frames()[0].clone();
    v2["v"] = json!(2);
    let mut short_nonce = frames()[0].clone();
    short_nonce["nonce"] = json!("AQEB");
    for line in [other_tag, extra, v2, short_nonce] {
        assert_eq!(parse_server_line(&line.to_string()), ServerLine::Discard, "{line}");
    }
    let other_nonce = json!({"v":3,"type":"mf_cap","nonce":OTHER_NONCE,"tag":"mf1"});
    // Read for what it says; the client compares it with its own connection's nonce.
    assert_eq!(parse_server_line(&other_nonce.to_string()), ServerLine::MagicFindCapability(OTHER_NONCE.into()));
}

#[test]
fn the_built_lines_are_the_fixture_bytes() {
    let frames = frames();
    let figure = build_magic_find_line(NONCE, 4, Some(333), 12).unwrap();
    assert_eq!(figure, format!("{}\n", frames[1]));
    let none = build_magic_find_line(NONCE, 9, None, 0).unwrap();
    assert_eq!(none, format!("{}\n", frames[2]));
    let keys: Vec<&str> = frames[1].as_object().unwrap().keys().map(String::as_str).collect();
    assert_eq!(keys, ["ageMs", "mf", "nonce", "seq", "tag", "type", "v"]);
}

#[test]
fn nothing_identifying_travels_and_the_line_fits_the_cap() {
    let line = build_magic_find_line(NONCE, 9_007_199_254_740_991, Some(100_000), 600_000).unwrap();
    assert!(line.len() - 1 <= MAX_LINE_BYTES);
    let value: Value = serde_json::from_str(&line).unwrap();
    let keys: Vec<&str> = value.as_object().unwrap().keys().map(String::as_str).collect();
    assert_eq!(keys, ["ageMs", "mf", "nonce", "seq", "tag", "type", "v"]);
}

#[test]
fn a_figure_or_an_age_out_of_bounds_gives_no_line() {
    assert_eq!(build_magic_find_line(NONCE, 1, Some(100_001), 0), None);
    assert_eq!(build_magic_find_line(NONCE, 1, Some(333), 600_001), None);
}
