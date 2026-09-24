//! Platform-independent half of the Tyrian Companion Nexus addon.
//!
//! Everything in this crate is plain, host-testable Rust: no `nexus`, no `windows`, no
//! `nexus::alert::send_alert` call. That includes the TCP client loop itself (`client`), which
//! reaches the game only through its `Host` trait. The `addon` crate (a `cdylib`, Nexus's own
//! `nexus` bindings, and the `Host` that reads `NexusLink`/Mumble Link and paints alerts) is the
//! other half, and only builds when targeting Windows — the `windows` crate it pulls in
//! gates most of its own types behind `cfg(windows)`, so it cannot compile for this
//! repository's own host (Linux). Splitting the crate this way is what lets `cargo test`, run
//! bare from this workspace's root, exercise the wire protocol, the framer, the backoff table
//! and the real client loop against a fake plugin on this machine, while
//! `cargo build --release --target x86_64-pc-windows-gnu` builds the whole thing, `addon`
//! included, into the addon DLL. See `../README.md`.

pub mod backoff;
pub mod client;
pub mod framer;
pub mod game_context;
pub mod instance;
pub mod protocol;
pub mod settings;
pub mod state;
