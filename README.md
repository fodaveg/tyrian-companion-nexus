# Tyrian Companion — Nexus addon

A [Nexus](https://raidcore.gg/) addon for Guild Wars 2 that paints, inside the game, the
loot and price alerts the [Tyrian Companion](https://github.com/fodaveg/tyrian-companion)
Obsidian plugin emits. It is the M3 milestone of `docs/SPEC-puente-ingame.md` in that repo,
which is the contract this addon implements and the source of truth if the two disagree.

## What it does, and does not do

It connects to a loopback TCP server the plugin opens (`127.0.0.1`, port 47823 by default,
configurable in both places and they have to match), sends one line of its own on connect,
and after that only ever reads. Every alert line it receives becomes one native Nexus alert
(`GUI_SendAlert`), painting the plugin's own `content` string verbatim, plus an optional
small panel in Nexus's Options window with connection status, the port setting, and the last
few alerts (for context; the native alert is what actually notifies you and works without
this panel).

It does not read Mumble Link or NexusLink, does not call the Guild Wars 2 API, and does not
send anything to the game or to the plugin beyond that one line at connect time. That is a
structural property, not a style choice: it is what keeps this addon inside the "utility
that helps players without affecting others" branch of ArenaNet's third-party program policy.

## Layout

This is a two-crate Cargo workspace, and that split is deliberate:

- **`core/`** (`tyrian_companion_nexus_core`): the wire protocol (parsing an alert line, the
  512-byte cap, the "unsupported version" and "malformed line" rules), the `\n` line framer,
  the `[250, 500, 1000, 2000, 5000]` ms reconnect backoff table, settings persistence, and
  shared in-memory state. No dependency on `nexus` or `windows`. This is what `cargo test`
  exercises, and it builds and tests on any host, this repository's Linux dev machine
  included.
- **`addon/`** (`tyrian_companion_nexus`, a `cdylib`): the actual Nexus addon — the
  `nexus::export!` entry point, the background TCP client thread, and the optional ImGui
  options panel. Its `nexus` dependency (and everything under it, transitively including the
  `windows` crate) is fenced behind `[target.'cfg(windows)'.dependencies]` in its
  `Cargo.toml`, with matching `#[cfg(windows)]` on the Rust side in `addon/src/lib.rs`. That
  fence exists because `windows` gates most of its own types behind `cfg(windows)` and
  genuinely cannot compile for a non-Windows host — without it, a bare `cargo build`/
  `cargo test` on Linux would fail on the whole workspace, not just skip the addon crate.

## Building

From the repository root:

```sh
cargo test                                          # core crate's tests, on the host
cargo build --release --target x86_64-pc-windows-gnu  # the addon DLL
```

The DLL lands at `target/x86_64-pc-windows-gnu/release/tyrian_companion_nexus.dll`. Copy it
to `<Guild Wars 2 install>/addons/` (create the `addons` folder if Nexus hasn't yet) and
restart the game, or use Nexus's own addon loader if it is already tracking that folder.

### Cross-compiling from Linux

You need:

- The Rust target: `rustup target add x86_64-pc-windows-gnu`.
- A mingw-w64 **C++** cross toolchain, not just C: on Fedora, `sudo dnf install
  mingw64-gcc-c++` (in addition to `mingw64-gcc`, which most mingw setups already have for
  plain C). The C++ compiler is required because `nexus`'s own `Cargo.toml` depends on
  `arcdps-imgui` unconditionally (it is not behind an optional feature), and that crate
  compiles a bundled C++ ImGui implementation via the `cc` crate — this is true even if this
  addon's own code never touches ImGui. Without `mingw64-gcc-c++` (specifically, its
  `cc1plus`, the C++ compiler backend), the build fails at `arcdps-imgui-sys`'s build script
  with `ToolNotFound: failed to find tool "x86_64-w64-mingw32-g++"`, before this crate's own
  code is even reached.
- Nothing else unusual: `cargo build --release --target x86_64-pc-windows-gnu` in this
  workspace's root picks up `mingw64-gcc-c++`'s `x86_64-w64-mingw32-g++` the normal way once
  it is installed.

`nexus` itself is not published on crates.io under that name — a different, unrelated,
abandoned 2016 crate holds it — so `addon/Cargo.toml` pins it as a git dependency at tag
`0.12.0` from [`Zerthox/nexus-rs`](https://github.com/Zerthox/nexus-rs), matching the version
`docs/SPEC-puente-ingame.md` names.

## Settings

The port lives in `<GW2>/addons/tyrian_companion_nexus/settings.json` and in Nexus's Options
window, under this addon's own section. **It has to match the port configured in the
plugin's own settings inside Obsidian** — the default on both sides is `47823`. Changing it
in the options panel takes effect on the addon's next (re)connection attempt; it does not
tear down a connection that is already up.

## Reconnecting

The addon does not need the plugin, or the game, to start first. If there is no server
listening yet — the common case right when the game launches, since the plugin lives inside
Obsidian and the player is free to start either one first — the addon just keeps retrying,
forever, on the backoff table above, without surfacing that as an error.

## Tests

`cargo test` (from the repository root, or `cargo test -p tyrian_companion_nexus_core`)
covers what does not need a running game or a running plugin: parsing a well-formed alert
line, every malformed/oversized/wrong-version case the spec calls out, the `\n` framer
against chunks split at arbitrary byte boundaries, the backoff table's saturation and reset,
and settings persistence. It does not, and cannot, cover the actual Nexus load/unload cycle,
the real TCP client thread, or the ImGui panel — those need a running game (see "What this
addon does" above for why that is out of scope here anyway) and are exercised by hand: build
the DLL, drop it into `<GW2>/addons/`, launch the game, and check Nexus's own log window.
