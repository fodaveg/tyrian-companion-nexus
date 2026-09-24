# Tyrian Companion — Nexus addon

A [Nexus](https://raidcore.gg/) addon for Guild Wars 2 that paints, inside the game, the
loot and price alerts the [Tyrian Companion](https://github.com/fodaveg/tyrian-companion)
Obsidian plugin emits, and tells the plugin what the game is doing so it can mark play
sessions on its own. It implements protocol **v2** of `docs/SPEC-puente-ingame.md` in that
repo, which is the contract and the source of truth if the two disagree.

Version 0.2.0 speaks v2 only. The plugin answers a v1 addon (0.1.x) with
`version_unsupported`, and this addon answers a v1 plugin the same way in reverse, so both
sides have to be updated together.

Version 0.2.1 refuses to save a Guild Wars 2 API key pasted into the token field, and anything
else outside the token's format, with a message that says where to copy the right value; an API
key saved by 0.2.0 is removed from `settings.json` on load and never sent. After a rejected
token the status line says what to do.

## What it does, and does not do

It connects to a loopback TCP server the plugin opens (`127.0.0.1`, port 47823 by default,
configurable in both places and they have to match) and authenticates with a token copied
from the plugin (see "Settings"). Once the plugin accepts it:

- every alert line becomes one native Nexus alert (`GUI_SendAlert`), painting the plugin's
  own `content` string verbatim, once per `(server, seq)`, so a reconnection never repeats an
  alert and an Obsidian restart never hides one;
- it reports the game context whenever it changes: the state (`gameplay`, `loading` or
  `character_select`), the map id (866 is Mad King's Labyrinth) and the character's name;
- it sends a heartbeat whenever it has been silent for the interval the plugin asks for
  (5 s), and a `bye` when it leaves: `game_exit` only when the game window receives
  `WM_CLOSE` or `WM_DESTROY`, `addon_unload` otherwise.

The Options window gets a small panel with the connection status, the port and token
settings, and the last few alerts (for context; the native alert is what actually notifies
you and works without this panel).

Where the context comes from: `NexusLink::is_gameplay`, and the map id and character name in
the Mumble Link Nexus shares with every addon (`DL_MUMBLE_LINK`). Neither tells a loading
screen from character select, so after gameplay ends the addon reports `loading` for up to a
minute and `character_select` after that, with no map and no character; before the first
character loads it reports `character_select`. That minute is an assumption the in-game test
has to confirm (`LOADING_WINDOW` in `core/src/game_context.rs`).

It reads nothing else. No positions, camera, combat, account or loot: the audited sources do
not carry a loot feed or a reliable AFK signal, and the protocol has no field for them. It
does not read process memory, does not call the Guild Wars 2 API, and does not send any input
to the game. That is a structural property, not a style choice: it is what keeps this addon
inside the "utility that helps players without affecting others" branch of ArenaNet's
third-party program policy. What the plugin does with the context happens outside the game.

## Layout

This is a two-crate Cargo workspace, and that split is deliberate:

- **`core/`** (`tyrian_companion_nexus_core`): the v2 wire protocol (building `hello`,
  `context`, `heartbeat` and `bye` exactly as the plugin validates them, reading `welcome`,
  `alert` and `error`), the client loop itself (`client.rs`: connect, authenticate, report,
  reconnect), reading the game context out of the Mumble Link bytes, the `\n` line framer,
  the `[250, 500, 1000, 2000, 5000]` ms reconnect backoff table, settings persistence, and
  shared in-memory state. No dependency on `nexus` or `windows`: the loop reaches the game
  only through a `Host` trait. This is what `cargo test` exercises, and it builds and tests on
  any host, this repository's Linux dev machine included.
- **`addon/`** (`tyrian_companion_nexus`, a `cdylib`): the actual Nexus addon — the
  `nexus::export!` entry point, the `Host` that paints alerts and reads `NexusLink` and the
  Mumble Link, the `WndProc` callback that notices the game closing, and the ImGui options
  panel. Its `nexus` dependency (and everything under it, transitively including the
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

#### Why the build statically links the mingw runtime

`.cargo/config.toml` and `addon/build.rs` exist for one reason: a DLL linked the default way
imports `libstdc++-6.dll`, `libgcc_s_seh-1.dll` and `libwinpthread-1.dll`, and none of those
ship with Windows or with Wine. Nexus resolves an addon's dependencies from the directory of
`Gw2-64.exe`, not from `addons/`, so it finds none of them, drops the addon with

```
[Loader] [WARNING] Failed LoadLibrary on "TyrianCompanion.dll". Incompatible.
                   Error Code 126 : Module not found.
```

and the addon never appears in game. The C++ runtime is not this addon's doing: it arrives
through `arcdps-imgui-sys`, which `nexus` depends on unconditionally and which compiles ImGui
as C++, even though nothing here calls ImGui.

`.cargo/config.toml` covers libgcc and winpthread with `-static-libgcc` and an explicit
`-Wl,-Bstatic -lwinpthread`. It cannot cover libstdc++ the same way: `-static-libstdc++` is a
driver option that only governs the `-lstdc++` the driver adds by itself, and this one is
emitted by `arcdps-imgui-sys` in the middle of the link line, ahead of every `-C link-arg`.
So `addon/build.rs` copies the toolchain's `libstdc++.a` into a directory of its own and puts
it on the library search path, where `ld` reaches it before the toolchain's directory (the
only one holding both `libstdc++.a` and `libstdc++.dll.a`). The comments in both files carry
the detail.

To check that a build is still clean, read the DLL's import table and make sure none of the
three appear:

```sh
python3 -c "
import pefile
pe = pefile.PE('target/x86_64-pc-windows-gnu/release/tyrian_companion_nexus.dll', fast_load=True)
pe.parse_data_directories()
print(sorted({d.dll.decode().lower() for d in pe.DIRECTORY_ENTRY_IMPORT}))
"
```

`nexus` itself is not published on crates.io under that name — a different, unrelated,
abandoned 2016 crate holds it — so `addon/Cargo.toml` pins it as a git dependency at tag
`0.12.0` from [`Zerthox/nexus-rs`](https://github.com/Zerthox/nexus-rs), matching the version
`docs/SPEC-puente-ingame.md` names.

## Settings

Two settings, both in Nexus's Options window under this addon's own section, and both saved
to `<GW2>/addons/tyrian_companion_nexus/settings.json`:

- **Token.** The plugin only talks to addons that know its secret. To paste it:
  1. In Obsidian, open Tyrian Companion's settings and, in the "Addon token" row, press
     **Copy token**. The first press generates the token and keeps it in Obsidian's secret
     storage; later presses copy the same one.
  2. In the game, open Nexus's Options, find Tyrian Companion, and press **Paste** next to
     the Token field (or click the field and press Ctrl+V).
  3. Press **Save**. The status line turns to "connected" within a few seconds if Obsidian
     is open with the plugin enabled.

  The field never shows the token, and the addon never writes it to its log. It does sit in
  clear in `settings.json`, like any addon setting, so anything that can read your files can
  read it. If the plugin rejects it (you rotated it in Obsidian, or pasted something else),
  the addon says so once, with the steps above, and stops trying until you paste a new one and
  save.

  Save trims spaces and newlines around the value and refuses two things, with a message under
  the field: your Guild Wars 2 API key (`XXXXXXXX-XXXX-…`, 72 characters), which is not the
  token and is never saved or sent, and anything that is not 32 to 128 characters without
  spaces. An API key already in `settings.json` from 0.2.0 is removed from the file on load.
- **Port.** **It has to match the port configured in the plugin's own settings inside
  Obsidian** — the default on both sides is `47823`.

Changes take effect on the addon's next connection attempt, right away if the plugin had
rejected the previous token; they do not tear down a connection that is already up.

## Reconnecting

The addon does not need the plugin, or the game, to start first. If there is no server
listening yet — the common case right when the game launches, since the plugin lives inside
Obsidian and the player is free to start either one first — the addon just keeps retrying,
forever, on the backoff table above, without surfacing that as an error. A connection that
drops is retried the same way, well inside the ten minutes the plugin waits before it closes
the session, so a short hiccup continues the same session instead of starting a new one.
Only two answers stop the retries until the settings change: a rejected token
(`auth_rejected`) and a protocol version the plugin no longer speaks (`version_unsupported`,
which also shows "update the Nexus addon").

## Tests

`cargo test` (from the repository root, or `cargo test -p tyrian_companion_nexus_core`)
covers what does not need a running game:

- every line the addon sends, byte for byte against the SPEC's own example lines, and every
  rule the plugin enforces on them (exact keys, the 512-byte cap, canonical `instance`, the
  character-name and map-id bounds);
- every line the plugin sends: `welcome`, `alert`, each `error` code, and the tolerance rules
  (unknown `type` ignored, known `type` with keys missing or extra discarded, a higher `v`
  asking for an update);
- reading map and character out of a Mumble Link laid out as the game writes it, and the
  gameplay / loading / character-select rule;
- `core/tests/client_v2.rs`: the real client loop against a fake plugin on a real loopback
  socket that validates every line the way the plugin does, through a full session (context,
  heartbeat, deduplicated alerts, `bye`), an Obsidian restart, a rejected token, an
  unsupported version, a retryable error, the game closing, a missing token, and an API key in
  the token setting that never goes out in a `hello`;
- what Save accepts as the token (`core/src/token.rs`): an API key refused in either case, a
  43-character token accepted, surrounding whitespace trimmed, too short or too long refused;
- the `\n` framer, the backoff table, settings persistence (an API key saved by 0.2.0 dropped
  and removed from disk on load), and the token never showing up in `Debug` output.

It does not, and cannot, cover the actual Nexus load/unload cycle, what `NexusLink` and the
Mumble Link really contain in each game state, the `WndProc` callback, or the ImGui panel —
those need a running game and are exercised by hand: build the DLL, drop it into
`<GW2>/addons/`, launch the game, and check Nexus's own log window for `Loaded addon`.
