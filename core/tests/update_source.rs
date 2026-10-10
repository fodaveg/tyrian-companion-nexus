//! Pins how the addon tells Nexus where to look for a new version.
//!
//! The `nexus::export!` block lives in `addon/src/lib.rs` behind `#[cfg(windows)]`, so it never
//! compiles on the Linux host and no test can call it. What Nexus reads from it is two plain
//! values, and both are checked here as text: the provider (GitHub) and the repository link. A
//! change to either has to be deliberate, because an addon with `UpdateProvider::None` is never
//! updated by Nexus, and nothing else would notice.
//!
//! Nexus compares the version declared by the DLL (taken from `addon/Cargo.toml`) with the tag
//! of each GitHub release, which it must parse as `v?N.N[.N[.N]]`; the tag check below keeps
//! the repository's convention (no `v`, three numbers) inside that format.

use std::fs;
use std::path::PathBuf;

const REPOSITORY: &str = "https://github.com/fodaveg/tyrian-companion-nexus";

fn workspace_file(relative: &str) -> String {
    let path = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("..")
        .join(relative);
    fs::read_to_string(&path)
        .unwrap_or_else(|error| panic!("cannot read {}: {error}", path.display()))
}

/// The `export!` block, without the lines that are comments.
fn export_block() -> String {
    let source = workspace_file("addon/src/lib.rs");
    let start = source
        .find("nexus::export! {")
        .expect("addon/src/lib.rs has no nexus::export! block");
    let block = &source[start..];
    let end = block
        .find("\n}")
        .expect("the nexus::export! block is not closed");
    block[..end]
        .lines()
        .filter(|line| !line.trim_start().starts_with("//"))
        .collect::<Vec<_>>()
        .join("\n")
}

#[test]
fn the_addon_declares_github_as_its_update_provider() {
    let block = export_block();
    assert!(
        block.contains("provider: nexus::UpdateProvider::GitHub,"),
        "the export block must declare GitHub as the update provider:\n{block}"
    );
}

#[test]
fn the_update_link_is_the_repository_of_the_addon() {
    let block = export_block();
    assert!(
        block.contains(&format!("update_link: \"{REPOSITORY}\",")),
        "the export block must name {REPOSITORY} as its update link:\n{block}"
    );
    let manifest = workspace_file("addon/Cargo.toml");
    assert!(
        manifest.contains(&format!("repository = \"{REPOSITORY}\"")),
        "addon/Cargo.toml and the update link must name the same repository"
    );
}

#[test]
fn the_declared_version_has_the_shape_of_a_release_tag_nexus_can_read() {
    let manifest = workspace_file("addon/Cargo.toml");
    let version = manifest
        .lines()
        .find_map(|line| line.strip_prefix("version = \""))
        .and_then(|rest| rest.strip_suffix('"'))
        .expect("addon/Cargo.toml has no version");
    let parts: Vec<&str> = version.split('.').collect();
    assert_eq!(
        parts.len(),
        3,
        "the release tag is the bare crate version, three numbers: {version}"
    );
    for part in parts {
        assert!(
            part.parse::<u16>().is_ok(),
            "{part:?} in {version} is not a number a Nexus version holds"
        );
    }
}
