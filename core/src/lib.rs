//! Platform-independent half of the Tyrian Companion Nexus addon.
//!
//! Everything in this crate is plain, host-testable Rust: no `nexus`, no `windows`, no
//! socket, no `nexus::alert::send_alert` call. The `addon` crate (a `cdylib`, Nexus's own
//! `nexus` bindings, and the actual TCP client) is the other half, and only builds when
//! targeting Windows — the `windows` crate it pulls in transitively gates most of its own
//! types behind `cfg(windows)`, so it cannot compile for this repository's own host (Linux).
//! Splitting the crate this way is what lets `cargo test`, run bare from this workspace's
//! root, exercise the wire protocol, the framer, and the backoff table on this machine,
//! while `cargo build --release --target x86_64-pc-windows-gnu` builds the whole thing,
//! `addon` included, into the addon DLL. See `../README.md`.

pub mod backoff;
pub mod framer;
pub mod protocol;
pub mod settings;
pub mod state;
