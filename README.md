# Tyrian Companion — Nexus addon

A [Nexus](https://raidcore.gg/) addon for Guild Wars 2 that paints, inside the game, the
loot and price alerts the [Tyrian Companion](https://github.com/fodaveg/tyrian-companion)
Obsidian or Hebra plugin emits, and reports game context and negotiated passive inventory
observations for automatic sessions. It implements protocol **v3** of `docs/SPEC-puente-ingame.md` in that
repo, which is the contract and the source of truth if the two disagree.

Version 0.2.0 speaks v2 only. The plugin answers a v1 addon (0.1.x) with
`version_unsupported`, and this addon answers a v1 plugin the same way in reverse, so both
sides have to be updated together.

Version 0.2.1 refuses to save a Guild Wars 2 API key pasted into the token field, and anything
else outside the token's format, with a message that says where to copy the right value; an API
key saved by 0.2.0 is removed from `settings.json` on load and never sent. After a rejected
token the status line says what to do.

Version 0.3.0 speaks protocol v3: after painting an alert it sends the plugin an `alert_ack`,
and the alert's trail in Tyrian Companion moves to "Recibido en el juego". It needs Tyrian
Companion 0.2.12 or later; with an earlier plugin the addon gets `version_unsupported` and
asks you to update Tyrian Companion in Obsidian. It also opens Obsidian when the game starts
with Obsidian closed (see "Automatic app launch" below).

Version 0.3.1 lets you choose whether that automatic launch opens Obsidian or Hebra, which
also hosts Tyrian Companion; Obsidian stays the default.

Version 0.4.0 adds an optional, movable Labyrinth farming window. Its `farm1` feed is an
authenticated extension of v3 negotiated after `welcome`; older v3 hosts still receive the
same context, heartbeat and alert acknowledgements and no farming subscription.

Version 0.5.0 adds the negotiated passive inventory source `live1`. It retains the v3 base
protocol and `farm1` compatibility. Compilation and portable tests do not certify native
reader runtime; see the coverage and QA limits below.

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

The optional `live1` channel reads the controlled character's owned carried inventory. It
uses bounded, passive `ReadProcessMemory` copies in the normally loaded Nexus addon, not
DRF or an authenticated inventory API. It never calls game getters, writes game memory,
hooks game code, suspends threads, or sends input. This describes the implementation; it
is not a claim of ArenaNet approval. Map/presence still come from Nexus/Mumble Link.

## Passive inventory source (`live1`)

A v3 host advertises `live_cap` after authentication. Older hosts get the existing context,
heartbeat, alert acknowledgements and optional `farm1` messages only; no memory sampling
starts without live negotiation. Presence and measurement availability remain independent.

The reader validates the executable's SHA-256 and AMD64 PE profile, discovers TEB/TLS
contexts autonomously, and checks the controlled character, inventory owner, location3
matrix (`C8/D0/D4`), sparse instance resolver, definitions and quantity getter identities.
No session pointer or PID is hardcoded. The currently certified executable is
`27d179bfe6a92fae633b412b8be0c90f697cd08646fa66a2e04b9e794410802c`, profile
`owned-bags-v3`. All other builds are unavailable until separately certified.

Each cycle is capped at 640 positions, 131072 requested bytes and 32768 exact reads,
including coherence rechecks and TEB discovery. The Windows adapter also checks a
750ms deadline between exact reads; this cannot preempt an OS call already in progress. Discovery is capped at 128 own threads
and 4096 system thread entries. Quantity is limited to 0..250 **per stack** by this
profile; aggregation by ID may exceed 250. A cycle that exceeds a budget or changes during
reread is rejected completely. Unsupported quantity profiles preserve unknown coverage:
an unknown instance suppresses that entire ID, never becoming quantity one or zero. One
is allowed only for the explicitly certified NULL Stackable fallback branches.

Free slots are `null`: sparse empty entries do not prove usable bag capacity. Currencies
are `none`, and Magic Find has no verified source. These are missing coverage, not zero
balances or completed wallet/MF support.

The background bridge worker targets one capture per second; render performs no inventory
reads and never waits for a socket or disk. Unload cancels the worker and joins it before
unmapping; reads and hash chunks check cancellation, but cannot preempt an OS call in progress. Real resolution depends on the reader and host
ACK. The addon emits aggregate begin/rows/end batches, each line at most 512 bytes, up to
eight rows per part, on the existing consecutive TCP sequence. It has one sample in flight,
waits for a durable `live_ack` and bounds ready/ACK and batch transmission to ten seconds.
The actual native maximum is 640 rows, 80 row parts and under 256 KiB per sample. No
unbounded queue or durable replay is implemented here.

Every connection, actual map/character/state change, inventory-owner change and read gap
requires a new random epoch and baseline. An identical periodic context preserves the
epoch. Baseline never represents newly acquired loot. Unknown quantities yield partial
samples for inspection, then require rebaseline. Read/ACK failures keep game presence but
expose unavailable measurement or storage errors. Inventory increases and decreases have
**unknown cause**; rapid changes that cancel between samples and gaps cannot be recovered.
The host owns persistence, prices and notifications; the addon does not label these changes
as certified drops, sales, deposits or openings.

Nexus Options and the farming window distinguish inventory measurement from connection.
The optional reader diagnostics show profile, owner-check outcome and bounded counters,
without addresses or a complete inventory dump. Blish can supply presence/alerts/HUD but
needs this local Nexus producer for inventory observations; Mumble alone has no item feed.

The external Fedora/GE-Proton11-7 proof observed item12147 from 0 to2 to4. Its two live
acquisitions used the Python v2 proof; v3 separately checked additional conditional profiles.
That evidence guided this port but does not certify the addon in a running game. Native
Windows load and reader runtime, Fedora/Proton addon bootstrap, reorder/character change,
reconnect and real inventory QA remain pending until tested on this DLL candidate.

## Layout

This is a two-crate Cargo workspace, and that split is deliberate:

- **`core/`** (`tyrian_companion_nexus_core`): the v3 wire protocol (building `hello`,
  `context`, `heartbeat` and `bye` exactly as the plugin validates them, reading `welcome`,
  `alert`, `error` and the optional `farm1` capability/state), the client loop itself (`client.rs`: connect, authenticate, report,
  reconnect), reading the game context out of the Mumble Link bytes, the `\n` line framer,
  the `[250, 500, 1000, 2000, 5000]` ms reconnect backoff table, settings persistence, and
  shared in-memory state, the safe inventory interpreter and negotiated live1 producer. No dependency on `nexus` or `windows`: the loop reaches the game
  only through a `Host` trait. This is what `cargo test` exercises, and it builds and tests on
  any host, this repository's Linux dev machine included.
- **`addon/`** (`tyrian_companion_nexus`, a `cdylib`): the actual Nexus addon — the
  `nexus::export!` entry point, the `Host` that paints alerts and reads `NexusLink` and the
  Mumble Link, the `WndProc` callback that notices the game closing, and the ImGui options
  and farming panels, and the current-process Win64 inventory adapter. Its `nexus` dependency (and everything under it, transitively including the
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
as C++, including the native ImGui panels this addon now renders.

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

## Automatic app launch

If the app that runs the plugin is closed when the game starts, this addon opens it: David, 24
sep 2026, "si empiezo a jugar y está cerrado, que se abra" (`docs/SPEC-puente-ingame.md` in the
plugin repo). The app is **Obsidian** by default; since 0.3.1 you can pick **Hebra** instead,
which hosts Tyrian Companion as an external plugin.

**When it fires.** At most once per addon load, right after the very first connection attempt
to the plugin's bridge. If that attempt's TCP `connect()` is refused or times out — nobody
listening, the ordinary case when the game starts before Obsidian does — the addon takes that as
"the app is closed" and launches the chosen one. A successful connect, even if the plugin then answers
`auth_rejected` or `version_unsupported`, means somebody was listening, so nothing is launched
either way; later reconnection retries never trigger a second launch. The decision itself
(`core::obsidian_launch::should_launch`) is plain Rust with no OS call, covered by `cargo test`
on Linux.

**Settings.** Two, both in this addon's Options panel:

- `open_obsidian_on_start`, a checkbox, on by default — including for a `settings.json` saved
  before this setting existed. With it off, nothing is ever launched. The key keeps its original
  name on disk so files from 0.3.0 still load; it now governs whichever app is chosen.
- `launch_app`, an "App to open" radio pair, `"obsidian"` (default) or `"hebra"`. A
  `settings.json` without the key means Obsidian, so an older install behaves exactly as before.
  Choosing applies and saves at once, and does not wake a client the plugin told to stop
  retrying.

The Options panel also shows a line, naming the app, with the outcome of this load's one attempt
(launched / no handler / error with its code), never a native alert: the SPEC's alerts are for
the plugin's own content.

**Mechanism.** Two paths, gated behind `#[cfg(windows)]` like the rest of this addon:

- **Under Wine/Proton** (this repository's only tested platform, David's own machine: Fedora +
  GE-Proton): launches `%SystemRoot%\system32\winebrowser.exe` with `obsidian://open` (or
  `hebra://open`) as its one argument. `winebrowser.exe` ships in every Wine/Proton prefix and
  forwards that argument to the host's `xdg-open`, which is what actually opens or focuses the
  Fedora Obsidian flatpak, or starts Hebra through its `x-scheme-handler/hebra` entry (Hebra's
  deep-link parser ignores a URI that is not a note or a Lumbre connect, so `hebra://open` only
  starts the app) — no registry key is written in the prefix. The Obsidian path was measured in
  [H18.27](https://github.com/fodaveg/tyrian-companion/blob/main/docs/audit/sonda-h18-27-abrir-obsidian-desde-proton.md)
  (path B there). Wine/Proton is detected by the `wine_get_version` export Wine's own
  `ntdll.dll` carries and a real Windows one never does — no prefix access needed for the check
  itself.
- **Native Windows** (no Wine): only if `HKEY_CLASSES_ROOT\obsidian` (or `\hebra`) exists — the
  app's own Windows installer registers it — `ShellExecuteW` opens `obsidian://open` (or
  `hebra://open`) the normal way. If
  the key is missing, nothing is launched, on purpose: otherwise Windows would pop its own "how
  do you want to open this?" dialog on top of the game. **Not verified**: this repository has no
  real Windows machine to test this path on; it is implemented against the documented Win32
  behavior only.

Neither path waits for the process it starts: both return as soon as the OS has accepted the
launch request, so this never blocks the game's thread.

## Settings

Four settings, all in Nexus's Options window under this addon's own section, and all saved
to `<GW2>/addons/tyrian_companion_nexus/settings.json`. `open_obsidian_on_start` and
`launch_app` (see "Automatic app launch" above) are the odd ones out: their checkbox and radio
buttons apply and save right away, since there is nothing to validate, unlike the port and the
token below, which only take effect after **Save**:

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

## Labyrinth farming panel

In Nexus Options, enable **Show Labyrinth farming panel / Mostrar panel de Laberinto**.
It is hidden by default, including for older `settings.json` files. The adjacent language
checkbox switches only this panel between Spanish and English; the position reset restores
the window if it was moved outside the screen. Drag its native title bar to move it; closing
it hides it until re-enabled in Options. These controls save immediately without saving a
pending token edit. The host's font/DPI and window style are retained, with an opaque background.

The read-only window separates measurement phase from host connection. It shows positive
**observed bags**, the host's declared session duration, a bags/hour band (or a minimum
if there is no upper bound), the observation's age, and free character bag slots with their
own age. A recent-character slot source is explicitly marked. Optional bag/time goals show
numeric progress and the host's estimate; the addon never estimates a rate or advances the
duration by itself. Closing reconciliation shows signed **net bags at close** separately
from the observed counter. Partial Magic Find and preparation are labelled as such;
temporary buffs and AFK are never represented as verified.

No start/stop or goal-edit buttons are in this window: manage the session and preparation
in Hebra or Obsidian. Neither hiding the panel nor losing the bridge stops a session.

The host advertises `{"v":3,"type":"farming_cap","nonce":…,"tag":"farm1"}` only
after authentication. The addon subscribes once using `farming_sub`, sharing the same
outgoing sequence as context, heartbeat and alert acknowledgements. A host without that
capability gets no subscription and the window says the panel is unavailable. Incoming
flat `farming_state` frames stay within 512 bytes, require exact keys, closed enums and
int32-or-null metrics, and are accepted only with the current 22-character base64url nonce
and increasing positive int32 farming sequence. Their sequence is independent of alert
deduplication and they are never ACKed.

Snapshots expire after 15 seconds of monotonic time, or immediately on disconnection.
The window retains the last reading, marks it **Stale data / Datos antiguos**, and removes
ETA. Counts, declared duration and rates freeze; a transport refresh never renews an inventory
observation or a character-slot observation. The feed does not send account/character
identity, builds, economic details, inventory contents, or free-form text. No new API
polling happens inside the addon.

QA limits for 0.4.0: portable parser/state and loopback tests cover the feed, and the Windows
cross-build checks the ImGui code and DLL dependencies. These checks cannot certify panel
placement, text contrast, keyboard navigation or Nexus load in a running Guild Wars 2
session; Fedora/Wine/game runtime QA remains pending until measured on that client.

## Reconnecting

The addon does not need the plugin, or the game, to start first. If there is no server
listening yet — the common case right when the game launches, since the plugin lives inside
Obsidian and the player is free to start either one first — the addon just keeps retrying,
forever, on the backoff table above, without surfacing that as an error. A connection that
drops is retried the same way, well inside the ten minutes the plugin waits before it closes
the session, so a short hiccup continues the same session instead of starting a new one.
Only two answers stop the retries until the settings change: a rejected token
(`auth_rejected`) and a protocol version the plugin does not speak (`version_unsupported`).
It shows "update the Nexus addon" when the plugin's `v` is 3 or more, and "update Tyrian
Companion in Obsidian" when it is below 3 (a plugin that predates v3).

Protocol v3 is v2 plus one message from the addon: right after painting an alert it
sends `{"v":3,"type":"alert_ack","nonce":…,"seq":…,"alertSeq":…}` on the same `seq` sequence as
`context`, `heartbeat` and `bye`, once per `(server, alertSeq)`. The `hello` goes out with `"v":3`;
the plugin's `welcome`, `alert` and `error` lines are read at v2 or v3.
The optional `farm1` subscription uses that sequence too, without changing those existing frames.

## Tests

`cargo test` (from the repository root, or `cargo test -p tyrian_companion_nexus_core`)
covers what does not need a running game:

- passive inventory fixtures: build/profile guards, owned location3, sparse references,
  conditional/NULL quantity branches, aggregation/unknowns, concurrent changes and budgets;
- live1 canonical wire fixtures, 512/513 cap, old-host negotiation, epochs/baselines, context
  equality, ACK isolation, source/storage failure, partial samples and bounded chunks;

- `farm1` byte cap (512 accepted, 513 rejected), exact keys and duplicate-key rejection,
  numeric bounds, nullable values, every enum, capability negotiation with older-server
  compatibility, nonce and sequence isolation, no farming ACKs, independent observation
  ages, monotonic TTL and immediate disconnect invalidation;

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
