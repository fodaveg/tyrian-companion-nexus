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

Version 0.6.0 adds the wallet to that source: a sample can list currency balances next to the
item totals. It changes no frame of `live1`; a host that already accepts `currencies:listed`
needs nothing else. See "Wallet coverage" below for what a listed balance does and does not
mean.

Version 0.7.0 adds the bag price block (`price1`, replaced by `price2` in 0.7.2) to the
Labyrinth panel, stops painting the reading-age lines while measurement is normal, and adds two
icons to Nexus's quick access bar. See "Bag price (`price2`)", "A panel that does not jump"
and "Quick access icons".

Version 0.7.1 retries `live_open` every 30 s after the plugin answers `source_conflict`, so the
inventory source recovers without a change of map. While it waits, the panel keeps saying that
another source owns the session and the game is not read between attempts. `unsupported_build`
and `not_gameplay` still block until the game context changes.

Version 0.7.2 shows the bag price gross, as the trading post shows it, over `price2`; it needs
a plugin that announces `price2`, and with an older one the price block does not appear. The
rate and price blocks keep a fixed number of lines, so the panel no longer jumps: what used to
be lines under the rate is now its colour and tooltip, and the bags ETA no longer blinks. The
DLL no longer imports `SuspendThread`, `GetThreadContext`, `SetThreadContext`, `ResumeThread`
or `SetThreadPriority`, which no code of the addon ever called; it ships without its symbol
table and is reproducible. See "Bag price (`price2`)", "A panel that does not jump", "Why the
DLL does not import the thread-control functions" and "Reproducible DLL".

Version 0.8.0 redraws the Labyrinth panel after David's sketch: observed bags and their rate on
the left, the gross price of a stack of 250 on the right, and three lines for free slots, Magic
Find and status, the last with a coloured dot for the connection. Every line is always there,
the window keeps its size, and what used to be lines of the panel is in the tooltips. The
panel has its own title bar, with a button that removes the window's background.

0.8.0 also reads, passively and from the game's own stored inputs, the bags' capacity with
their used and free slots and the Magic Find with its three addends. The panel paints those
readings in its Slots and MF lines, and MF warns when it falls from the session's highest.
A cycle that fails does not make them blink: the last verified reading is held for 5 seconds.
Without a reading the lines fall back to what the plugin sends, and the plugin's Magic Find is
written `MF: 333% partial` because it is not a live reading. Neither reading is sent to the
plugin yet, and no frame of the protocol changes. The two readers have only run against
fixtures: the count of used positions and the 250 ms they share have not been seen in the
game. See "Labyrinth farming panel" and "Bag slots and Magic Find (read by the addon, not on
the wire)".

Version 0.8.1 draws the button that removes the panel's background as the usual contrast sign,
a ring with its left half filled, instead of a square that read as "stop". Nothing else
changes.

Version 0.8.2 is the audit's round on the readers and the settings. The inventory reader
makes fewer reads per object, and a pass now fits 482 stacks of the class that is dearest to
read (367 before). The executable's hash is done a second at a time; if it adds up 10 seconds
without ending, the plugin gets `read_failed` and the hash goes on until it ends. What the
executable itself decides is no longer taken for a transient failure, and a reading that did
fail is tried again 30 seconds later. The Magic Find reader copies each object as one block:
349 reads against 557 over the shape measured on 8 October, but more bytes, so fewer active
buffs fit than at 0.8.1 (the table under "Bag slots and Magic Find"; accepted by the owner on
8 October 2026), and above the limit the pass gives no coverage. A cycle whose context
changed (another character, another map) publishes nothing to the panel. The panel is
computed only when it can have changed, the highest Magic Find is kept per character for the
session, and no character in a map is no longer counted as a capture that failed. The
settings are saved in turns and without I/O under the mutex, and Options shows the reader's
and the panel's timing counters. The protocol with the plugin does not change and neither
does the DLL's import table (257).

Nothing of 0.8.2 has been seen inside the game. The known limit stays: a context that names
one character while the memory belongs to another. Two more cases are not covered, and
never were: a change from A to B and back to A entirely inside one pass (up to 750 ms), and
a change that the context probe does not see because the state, the map and the character
are the same after it. Neither is detected, and nothing here claims to detect them.

After 0.8.2 (not released, not seen in the game): the addon reads the Magic Find through
`magic_find_cached` (`CachedMagicFind`), which the owner accepted on 8 October 2026. The
content of the applied buffs is read whole at most every 30 seconds; on the cycles between,
a pass verifies it by the headers (164 reads and 19556 bytes over the live shape, against
349 and 29328 for a whole pass). Every pass still reads the route, the slots, both table
headers, the buckets, every node and the first 48 bytes of each definition in use, and a
buff applied or removed, another build, context, character or definition makes that same
pass read everything. The game's context is found the same way: the walk over the process's
own threads (about 115 in a running game, each with its handle, a query and a few copies) is
made every 30 seconds, and the cycles in between only check the thread it found: the system
still has it with the same TEB, the TEB's self pointer, process and thread ids are the same,
and its TLS route ends at the same context (6 copies, 68 bytes). A check that fails, or a
copy or call in it that fails, is a full walk in that same cycle, so nothing is read through a
thread that was not just checked. That there is only one context is checked by the walk, once
every 30 seconds, after any change of the game context the client sees, or when the check
fails, and no longer on every cycle: the owner accepted the search every 30 seconds or on a
failed check, and this follows from it. The
cache is also emptied, and the thread found forgotten, whenever the client sees the context
change (during a cycle, which is then thrown away, or between two, the first one after a
reconnection included); the cache is emptied as well by any pass that ends without a value.
What a verifying pass
does not see, for at most 30 seconds, is in the header of `core/src/magic_find.rs`.

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

The optional `live1` channel reads the controlled character's owned carried inventory and the
wallet's currency balances. It
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

That verification starts on the first cycle and its answer is kept for the whole load only
when it is final, which is whenever the file or its loaded image decides it: the certified
build, or another one, by a size that cannot be the certified build's (no bytes, or more than
128 MiB), a hash that was computed and is not its, or a header in memory that is not its. A
"no" of those is `unsupported_build` to the host and stops the source, as it always did.

The hash is done a second at a time (`executable::SLICE`), once on each pass of the thread
that keeps the connection to the plugin alive, going on from where the pass before left it,
with the same calls to the system's SHA-256. It used to be done in one go, with 10 seconds
for it, on that thread: on a cold, slow disk it sent no heartbeat for as long as the hash
took, and the plugin takes the connection for lost after 15 seconds without one. Now a pass
of that thread is at most the second of the slice, the one read of the file that was under
way when it ran out, and the quarter of a second it waits on its socket: about a second and
a quarter between two chances to send a heartbeat, and the slice is cut short at once when
the worker is told to stop or the game is closing.

The hash has no time limit of its own, and nothing shows how far it has got. For as long as
it takes, nothing of the game is read and there is no sample. It ends in a verdict, or in a
failure of the system if the file stops being readable; being slow never ends it, and never
starts it over, which on a disk slow enough would be never finishing it.

**For its first ten seconds of hashing the addon says nothing about it to the plugin.** A
verdict that is pending is not a reading that failed: the source is not ready to sample yet
(`Host::prepare_inventory`, `Readiness::Pending`), so the loop takes no sample, opens no epoch
and sends no `live_status`, exactly as when it is not time to sample. A `live_status` would
open a gap in the plugin's session on every load, and `docs/SPEC-live-loot.md` asks for
nothing between `live_cap` and the first `live_open`: the plugin only starts expecting samples
once it has answered a `live_open` with `ready`, and has no timer of its own on that stretch.
On the wire there are heartbeats and then the `live_open` and its baseline, as on any load;
the first sample comes those few seconds later than when the hash ran in one go. Meanwhile
Options and the panel's status tooltip say "Inventory: waiting for confirmation", the state
the source is in before its first capture.

**Past those ten seconds it says so, and goes on hashing** (`executable::OVERDUE`,
`Readiness::Overdue`). The plugin gets `live_status unavailable` with `read_failed`, the line
and the reason of any reading the source could not complete, once; Options and the panel's
status tooltip change to "Inventory: reading unavailable". Before this the silence had no end:
on a cold, slow disk, or with something holding up the reads of the file, the plugin saw the
capability and then heartbeats for as long as the hash took, and the panel waited without a
word of why. No sample is asked for meanwhile, there being no verdict to read under, and the
hash is where it was: the next pass does its next slice, at the same pace, and the pass on
which it ends takes the first sample, which opens its epoch with a baseline as on any load.

Ten seconds because that is what the source itself gives the plugin to answer before it takes
its silence for a failure (`live::RESPONSE_TIMEOUT`), and what the hash was given when it ran
in one go, where they ended it. They are seconds spent hashing, added up over the slices
(`Verdict::unfinished_for`), not seconds on the clock: with a character in a map that is ten
passes, about twelve and a half seconds, and a loading screen in the middle, during which no
slice runs and the plugin has been told `not_gameplay`, is not held against the hash. A hash
that the system fails is started again 30 seconds later, and that one has its own ten.

What the system fails to do is not a verdict: the file cannot be opened or read, it is
written while it is hashed, or a copy of the header fails. That used to be kept as
"unsupported game build" until the addon was loaded again. That one is a reading that
failed, said as `read_failed` when it happens, and tried again 30 seconds later, from the
start if it happened during the hash and without hashing again if the file was already known
to be the certified build's. Which builds are accepted, and the hash they are told by, are
unchanged.

Each cycle is capped at 640 positions, 131072 requested bytes and 32768 exact reads,
including coherence rechecks and TEB discovery. The Windows adapter also checks a
750ms deadline between exact reads; this cannot preempt an OS call already in progress. Discovery is capped at 128 own threads
and 4096 system thread entries. Quantity is limited to 0..250 **per stack** by this
profile; aggregation by ID may exceed 250. A cycle that exceeds a budget or changes during
reread is rejected completely. Excluded entries also recheck their identity and location;
a location change during copying invalidates the capture. Unsupported quantity profiles preserve unknown coverage:
an unknown instance suppresses that entire ID, never becoming quantity one or zero. One
is allowed only for the explicitly certified NULL Stackable fallback branches.

How a pass spends that budget. The position matrix is copied whole, in reads of up to 512
positions, before the first item is followed and again when the final pass starts; a
position that differs between the two copies rejects the capture, whichever it is. The words
of the executable a pass relies on (the vtable slots of each item class, of the stack class
and of a conditional profile's predicate class, and the bytes of the NULL fallback) are
copied once per pass, not once per item, and all of them a second time before the pass is
accepted. Every field the game can change is read as often as it always was: a position
twice; an item's class, instance reference, resolver entry, definition, ID, location and
owner three times (its turn, right after its quantity, the final pass); a stack's class and
count four times, two in each quantity; a conditional definition's subtype, payload pointer
and condition four times. Fields that lie together come in one read of the same bytes.

A position costs 16 requested bytes, a stack 174 more on the common quantity branch and 254
on a conditional one, the route 224 and each item class about a hundred. Over 512 positions,
all of them with a stack,
a pass asks for 97600 bytes and 11823 reads on the common branch. On a conditional branch
482 stacks fit and the 483rd makes the pass ask for more than its budget, so it gives no
sample; beside the largest discovery (a check that fails at its last step, 68 bytes, and then 128 threads, 5632 bytes: 5700 in all) the figure is 460, by 4 bytes.
`core/tests/inventory.rs` pins these figures to the byte.

Free slots are `null`: sparse empty entries do not prove usable bag capacity. Magic Find has
no verified source. These are missing coverage, not zero values or completed MF support.

### Wallet coverage

In the same cycle, after a whole inventory sample, the worker reads the wallet the game's own
wallet window uses: context `+0x98` (character context), `+0xA0` (the wallet's local
character, a different object and vtable from the controlled-inventory wrapper at `+0x98`),
`+0x1878` (the embedded currency manager), and its open-addressing map (capacity and count
DWORDs, a pointer to 12-byte buckets of key, balance and occupied hash). The native key is
the public currency ID of `/v2/currencies` and the balance is the DWORD after it.

Before any balance is copied, the three vtables and the three getters on that route must be
the certified ones. Once per verified build, five fixed ranges of the executable (the two
route getters, the balance getter, the map lookup and its 256-entry hash table, 1416 bytes)
must match their audited SHA-256 digests; a different byte leaves the wallet without coverage
until the addon is loaded again, while a failed copy is tried again on the next cycle. The capacity must be a power
of two up to 4096 and the count at most the capacity. The table is copied once, in reads of
whole buckets, and the owner pointers, the vtables and the header are read again afterwards.
Every occupied bucket must carry the hash the game computes for its key and be reachable by
the game's linear probing from that hash's bucket, no key may repeat, and the number of
occupied buckets must equal the count. A key of zero, a key or balance above 2147483647, or
any other failure rejects **all** currencies of that cycle: nothing is clamped and no part of
a wallet is sent. The wallet has a byte budget of its own, 65536 requested bytes per cycle,
next to the inventory's; it shares the cycle's 750ms deadline and never calls a getter.

A sample with a wallet says `currencies:listed` and adds one `[1,id,balance]` row per key,
after the item rows. `listed` covers **only the IDs in those rows**. A key present with a
balance of zero is a covered zero. A wallet that could not be read, an empty map or an absent
one says `currencies:none` with no currency row: that is missing coverage, not zero balances,
and it never invalidates the item rows of the same sample, opens a new epoch or sends a
`live_status`. If the wallet's owner differs from the previous sample's, that one sample goes
out with `currencies:none`, so the host takes the next balances as a baseline.

**Known limit: the first gain of a currency the account never held is not counted.** The
native map is sparse; a currency the account has never had has no key, so it is not listed
and not covered. When it first appears, the host takes that balance as the local baseline of
that ID, without a delta, as `docs/SPEC-live-loot.md` fixes for any first appearance. The
addon does not invent a zero row to work around this: it cannot tell "never held" from "not
read". Later changes of that currency are observed normally.

The background bridge worker targets one capture per second; render performs no inventory or
wallet reads and never waits for a socket or disk. Unload cancels the worker and joins it before
unmapping; reads and hash chunks check cancellation, but cannot preempt an OS call in progress. Real resolution depends on the reader and host
ACK. The addon emits aggregate begin/rows/end batches, each line at most 512 bytes, up to
eight rows per part, on the existing consecutive TCP sequence. It has one sample in flight,
waits for a durable `live_ack` and bounds ready/ACK and batch transmission to ten seconds.
The actual native maximum is 640 item rows plus 3456 currency rows, which is the contract's
4096 rows and 512 row parts, and under 256 KiB per sample; a wallet with more keys than that
is not listed. The wallet observed in the live comparison had 55. No
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
Both show the wallet coverage of the last capture while a measurement is in progress: the
number of currencies covered, or "no coverage" with one closed reason (no reading, wallet
profile not verified, unknown structure, character unavailable, map out of bounds, wallet
empty or absent, inconsistent map, value out of range, changed while reading, read failed).
Verified Magic Find stays "no coverage".
The optional reader diagnostics show profile, owner-check outcome and bounded counters,
without addresses or a complete inventory dump. Blish can supply presence/alerts/HUD but
needs this local Nexus producer for inventory observations; Mumble alone has no item feed.

The external Fedora/GE-Proton11-7 proof observed item12147 from 0 to2 to4. Its two live
acquisitions used the Python v2 proof; v3 separately checked additional conditional profiles.
That evidence guided this port but does not certify the addon in a running game. Native
Windows load and reader runtime, Fedora/Proton addon bootstrap, reorder/character change,
reconnect and real inventory QA remain pending until tested on this DLL candidate.

The wallet route was first identified statically, then read whole by an external read-only
probe on the same Fedora/GE-Proton setup on 7 October 2026: capacity 128, 55 occupied keys,
the same 55 IDs `/v2/account/wallet` returned, 54 equal balances and one (45, volatile magic)
11 higher than the cached API value while playing, with no hash mismatch and no unreachable
key. That is one account, one session and one executable. It does not cover a spend, a
currency reaching zero, a first-ever currency, a character change or a map rehash, and it is
not a run of this DLL: the wallet reader in the addon has only been exercised against
fixtures.

### Bag slots and Magic Find (read by the addon, not on the wire)

In the same cycle, after the wallet, the worker reads two more things from the same verified
build and context. Both end in the local reader diagnostics (`inventory::Diagnostics::bags`
and `::magic_find`), each as a value or as "no coverage" with one closed reason. From there
the Labyrinth panel paints them in its "Slots" and "MF" lines, before the plugin's figures,
and Options shows each reader's last pass (see "Labyrinth farming panel").

**Neither is on the wire in this version.** The inventory sample that goes out is unchanged,
its `free_slots` stays `None`, and no frame carries Magic Find: the plugin does not receive
what the addon reads. Sending them is pending, and needs the protocol to say how.

The game stores neither number. It computes both when it paints, from stored inputs, and the
reader repeats that arithmetic without calling anything.

**Bag slots** (`core/src/bags.rs`). The inventory window's `used/total` counter asks the
inventory for three numbers: the total is the sum of the sizes of the equipped bags, the used
count is the number of non-null positions of the inventory array, and free is their
difference. The reader takes the bag slot count (`inventory+0x440`), the 16 bag pointers
(`+0x380`) and, for each bag, item `+0x40` -> definition, definition `+0x30` -> bag payload,
payload `+0x28` -> size; then the position array the inventory reader already walks
(`+0xC8`, count `+0xD4`). It requires the `ItCliBag` class on every bag, definition type 3
and a size of at most 32.

The 512 positions the diagnostics line shows (16 bag slots of 32) are the reserved array, not
the capacity: a bag of 20 leaves 12 of its 32 positions unusable. Free slots are therefore
`capacity - occupied`, never `512 - occupied`. With the state of the morning of 8 October
2026 (bags adding up to 414, 313 positions in use) that is 101.

**Magic Find** (`core/src/magic_find.rs`). The hero panel shows
`account luck level + modifiers pushed by the server + modifiers of the applied buffs`, capped.
The reader returns that total, uncapped, with its three addends, so a drop can be attributed:
luck (the account base the API also gives), pushed, and buffs (food, boosters, banners). It
walks the character's buff table, checks every bucket's hash and every node's class, and
evaluates only constant records. A counted record that needs a game mode, a trait or a state
condition makes the whole value "no coverage", never a partial total.

Both readers keep the rules of the other two: twelve and nineteen static ranges must match
their audited SHA-256 digests, every vtable and dispatch slot on the route is compared (never
called), and a second look after the copy rejects the value on any difference. That second
look reads again every owner, vtable and header and every table the game mutates: the bag
list, the whole position array, the pushed-modifier table and the buff table. It does not
read again what hangs from them as game content — bag and buff definitions and their records
— which is read once. Two pointer rules are part of the profile: heap objects sit on 8 bytes and
pointers into game content sit 4 past a multiple of 8. The second one is an observation of
one game session, not something derived from code; if it stops holding, the reason shown is
alignment and there is no value.

Budgets are hard and separate: 16384 requested bytes per cycle for the bags and 65536 for
Magic Find, guards included on the first cycle of a build. A pass that does not fit is no
coverage. The largest bag pass the profile allows, 16 bags and 640 positions copied twice,
asks for 12195. The external probe's whole passes on 8 October asked for about 1.9 KiB (bags,
without the position array, which adds 4 KiB per copy for 512 positions) and 29 to 33 KiB
(Magic Find, with 81 and 92 buffs).

How many buffs a Magic Find pass fits, which is fewer than at 0.8.1. A pass copies the route
twice and the twelve slots (2368 bytes in all), the buckets twice whatever they hold (48 bytes a
bucket: 12288 for a table of 256, 24576 for one of 512, the largest accepted), and then, for
each buff, its node and its instance, and once for each definition in use its head, its group
and its records (132 bytes with one record, 2364 with the 32 that are the most accepted).
Copying an object as one block took the node and the instance from 40 bytes to 92: fewer
copies, 349 against 557 over the live shape, and more bytes. The most buffs that fit, on any
cycle but the first of a build, whose 6054 bytes of guards come out of the same budget:

| Buckets | Definitions | Now | First cycle | At 0.8.1 | First cycle |
| --- | --- | --- | --- | --- | --- |
| 512 | one for all, one record | 418 | 352 | 512 | 512 |
| 512 | one each, one record | 172 | 145 | 235 | 200 |
| 512 | one each, 32 records | 15 | 13 | 16 | 14 |
| 256 | one for all, one record | 256 | 256 | 256 | 256 |
| 256 | one each, one record | 227 | 200 | 256 | 256 |
| 256 | one each, 32 records | 20 | 18 | 21 | 19 |

One more than that is `Bounds`: no coverage for that cycle, never part of a total.
`core/tests/magic_find.rs` pins every figure, with the reader of 0.8.1 kept there as it was.
What is not known is which row a game session is on. The fixture of the live shape of 8
October 2026 (91 buffs in 256 buckets, 47 definitions of one record each) asks for 29328 of
the 65536 bytes. When the game doubles its table, how many of its buffs share a definition and
how many records a definition carries have not been observed.

Cadence is the cycle's: once per second, in this order — inventory, wallet, bags, Magic Find.
The last two run **before** the cycle hands the inventory sample to the client, which then
reads the game context again and seals the sample or, if the context changed meanwhile,
discards it, and with it what the two readers said in that cycle: the diagnostics the panel
and Options read stay those of the cycle before. They used to be put there before that second
look, so the bags and the Magic Find of a cycle during which the character changed were
painted, and its Magic Find taken for the highest of the session, as a reading of the
character the panel still had. So they cannot change what an inventory sample contains, but the time they take
does widen the window in which a context change discards that copy. To bound it they share a
deadline of their own: 250 ms from the moment they start, and never past the cycle's 750 ms.
A pass cut by that clock is no coverage with the reason "deadline", distinct from a failed
copy. Neither the 250 ms nor the real duration of a pass has been measured in a running game.
The reader diagnostics of Options show how long each pass takes, last and longest, so that a
session in the game can say.
Reading them after the sample is sealed would remove that effect altogether; it needs a second
call from the client loop and is not done here.

Known limits of the Magic Find reader, all of which fail closed: the bound of 10000 on a
pushed value is applied to every pushed record, not only to the two Magic Find types, so one
out-of-range record of another type leaves Magic Find without coverage; the display cap is
not read, so the total is uncapped; a negative total is refused as out of bounds, although a
negative addend is accepted.

A known limit of both readers that does not fail closed: **whose** a reading is. Nothing in
what they read names the character. The panel attributes a reading to the character the game
context names (`NexusLink` and the Mumble Link), and the client keeps a cycle only if that
context was the same before it and after it. That catches a change that falls inside a cycle.
It does not catch a context that names one character while the memory is still, or already,
another's for a whole cycle, before and after agreeing: such a reading would be painted as
the named character's, and its Magic Find could become that character's highest of the
session. Whether the game ever shows that, and for how long, has not been observed: it needs
a character change in a running game with both in sight. No margin of time was put in for it,
there being nothing to size one from.

The evidence is the external read-only probes of
`tyrian-companion/docs/audit/loot-bag-capacity-probe` and `loot-mf-probe`, run on Fedora with
GE-Proton11-7 on 8 October 2026: capacity 414 twice, equal to the inventory window, and Magic
Find 333.0 and then 363.0, equal to the hero panel, the rise carried only by the buff addend.
That is one account, one character, one game session and one executable. It does not cover a
drop, a bag change, another launch of the game or native Windows. **It is not a run of this
DLL**: these two readers have only been exercised against fixtures, and the used-positions
count was not part of the probes at all.

## Layout

This is a two-crate Cargo workspace, and that split is deliberate:

- **`core/`** (`tyrian_companion_nexus_core`): the v3 wire protocol (building `hello`,
  `context`, `heartbeat` and `bye` exactly as the plugin validates them, reading `welcome`,
  `alert`, `error` and the optional `farm1` capability/state), the client loop itself (`client.rs`: connect, authenticate, report,
  reconnect), reading the game context out of the Mumble Link bytes, the `\n` line framer,
  the `[250, 500, 1000, 2000, 5000]` ms reconnect backoff table (see "Reconnecting" for which
  wait comes when), settings persistence, and
  shared in-memory state, the safe inventory and wallet interpreters and negotiated live1 producer. No dependency on `nexus` or `windows`: the loop reaches the game
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

#### Why the DLL does not import the thread-control functions

Linking winpthread statically used to leave five `kernel32.dll` imports in the DLL with no
code in the addon calling any of them: `SuspendThread`, `GetThreadContext`,
`SetThreadContext`, `ResumeThread` and `SetThreadPriority`. libstdc++ and libgcc need a
handful of pthread calls (`pthread_once`, the thread-specific keys, mutexes, condition
variables), which pulls `libpthread.a(libwinpthread_la-thread.o)` and
`(libwinpthread_la-sched.o)` into the link. Fedora builds winpthreads without
`-ffunction-sections`: each member is a single `.text` section, so `--gc-sections` cannot drop
the unused part. Three functions kept that way are the only code in the link that references
the five, and nothing reaches them: `pthread_cancel` (its only caller is `pthread_kill`, which
nothing references), `pthread_create` and `pthread_setschedparam`.

`addon/src/absent_imports.rs` defines the five `__imp_` symbols inside the DLL, so the linker
never pulls them from `libkernel32.a` and they stay out of the import table. The unreachable
function bodies are still in the DLL, bound to local functions that only return failure. The
same file notes what that means for any future code that would call one of the five.

The release profile also strips the DLL's symbol table (`strip = "symbols"` in `Cargo.toml`),
which is where those names would otherwise still be readable. To check, neither of these may
print anything:

```sh
x86_64-w64-mingw32-objdump -p target/x86_64-pc-windows-gnu/release/tyrian_companion_nexus.dll \
  | grep -E 'SuspendThread|GetThreadContext|SetThreadContext|ResumeThread|SetThreadPriority'
strings target/x86_64-pc-windows-gnu/release/tyrian_companion_nexus.dll \
  | grep -i -E 'SuspendThread|GetThreadContext|SetThreadContext|ResumeThread|SetThreadPriority'
```

Those two only say the names are gone. They cannot say whether the functions are still
unreachable, and the stand-ins would hide it if they stopped being so: after an upgrade of
winpthreads or libstdc++, or with a C++ dependency that starts a thread with `pthread_create`,
the link would still succeed and both commands would still print nothing, but
`pthread_create` would return success for a thread left suspended for ever. So a third check
goes with them, on **every** release build:

```sh
scripts/check-dead-thread-code.sh
```

It links the addon once more in `target/check-dead-thread-code/` (symbol table kept, linker
map with `--cref`; the release DLL is not touched) and exits with 1 if any object outside
`libpthread.a` references `pthread_create`, `pthread_cancel`, `pthread_kill` or
`pthread_setschedparam`, if any instruction of the DLL outside those four reaches one of them
or goes through one of the five stand-ins, if a pointer to one of the four is stored anywhere
in the DLL, or if one of the five is imported again. It reads every address from the DLL it has
just linked, and exits with 2, not 0, when it cannot check (no symbol table, no map). The
first run compiles everything again for that directory (about 30 s and 350 MB); later runs
only link. Expected output, with cargo's own lines on stderr:

```
  __imp_GetThreadContext     defined by the addon, referenced only by libpthread.a
  __imp_ResumeThread         defined by the addon, referenced only by libpthread.a
  __imp_SetThreadContext     defined by the addon, referenced only by libpthread.a
  __imp_SetThreadPriority    defined by the addon, referenced only by libpthread.a
  __imp_SuspendThread        defined by the addon, referenced only by libpthread.a
  pthread_cancel             no object outside libpthread.a references it
  pthread_create             no object outside libpthread.a references it
  pthread_kill               no object outside libpthread.a references it
  pthread_setschedparam      no object outside libpthread.a references it
  __imp_GetThreadContext     used only by: pthread_cancel
  __imp_ResumeThread         used only by: pthread_cancel pthread_create
  __imp_SetThreadContext     used only by: pthread_cancel
  __imp_SetThreadPriority    used only by: pthread_setschedparam pthread_create
  __imp_SuspendThread        used only by: pthread_cancel
  4 of the four are in the DLL; pointers to them stored in it: none
  import table: none of the five
check-dead-thread-code: OK
```

If it fails, do not ship that DLL. Before anything else, take out of
`addon/src/absent_imports.rs` the stand-ins the function it names uses (the "used only by"
lines say which): they will be imported again, and work.

To see that it measures something, give the check link an object that calls `pthread_create`.
Outside the repository:

```sh
cat > /tmp/probe.c <<'EOF'
#include <pthread.h>
static void *run(void *argument) { return argument; }
int tyrian_negative_probe(void) { pthread_t thread; return pthread_create(&thread, 0, run, 0); }
EOF
x86_64-w64-mingw32-gcc -c -O2 /tmp/probe.c -o /tmp/probe.o
CHECK_DEAD_THREAD_CODE_LINK_ARGS="/tmp/probe.o -Wl,--undefined=tyrian_negative_probe" \
  scripts/check-dead-thread-code.sh; echo "exit=$?"
```

It must end in `check-dead-thread-code: FAIL` and `exit=1`, with these two lines among the
rest (the addresses change from one build to another):

```
  FAIL pthread_create is referenced by /tmp/probe.o
  FAIL tyrian_negative_probe reaches pthread_create:   180266925:	call   180254110 <pthread_create>
```

What the DLL still imports of that kind, and why:

- `OpenThread`, `Thread32First`, `Thread32Next`, `CreateToolhelp32Snapshot`,
  `ReadProcessMemory`, `GetCurrentProcess`, `GetCurrentProcessId`: the passive inventory reader
  (`addon/src/inventory.rs`). It opens threads of its own process with
  `THREAD_QUERY_INFORMATION` to find their TEBs and copies memory of its own process.
- `CreateThread`, `GetCurrentThread`, `GetCurrentThreadId`, `SwitchToThread`,
  `SetThreadStackGuarantee`: Rust's standard library; the addon's connection thread is a Rust
  thread.
- `GetThreadPriority`: winpthreads' bookkeeping of the calling thread, which is live.
- `OpenProcess`, `GetProcessAffinityMask`, `SetProcessAffinityMask`, `IsDebuggerPresent`,
  `GetHandleInformation` and msvcrt's `_beginthreadex`: only referenced by other unreachable
  winpthreads functions (`sched_getscheduler` and `sched_setscheduler`;
  `pthread_num_processors_np` and `pthread_set_num_processors_np`; `pthread_setname_np`;
  `pthread_join`, `pthread_detach`, `pthread_cancel` and their helpers; `pthread_create`).
  Not removed.

#### Reproducible DLL

The link passes `--no-insert-timestamp` (`.cargo/config.toml`), so the PE header and the export
table carry no link date. With the symbol table stripped as well, two clean builds of the same
tree in the same directory, with the same toolchain, give the same file byte for byte, and a
DLL can be compared by its sha256. Paths of the registry crates are part of the file, so a
build in another home directory is not expected to match.

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

`settings.json` is replaced whole or not at all: the new contents are written next to it, in
`settings.json.tmp`, and renamed over it, so a write cut short (the game killed, a full disk)
leaves the file of before instead of half of a new one. It is not flushed to the disk before
the rename, so this does not cover a power cut.

A save is asked for on one of two threads: the frame's, for everything clicked in a window,
and Nexus's input thread, for a quick access icon or its key. The settings are read under
their lock and the file is written after that lock is let go, so neither thread waits for
the other's disk on it. Each save takes a number while it still holds the lock; one that
reaches the file after a newer one writes nothing, and only one writes at a time, so two of
them never share `settings.json.tmp`. Neither thread writes the file itself: a save is
handed to a thread of the addon's own, which writes the saves one after the other in the
order they were asked for, so a click does not wait for the disk on the frame that took it.
When the addon unloads that thread writes what is still waiting before it ends, and a save
asked for while it is not running (before it starts, after it ends, or if it could not be
started) is written by whoever asked, so none is dropped.

A save that fails used to go to the log and nowhere else, and the window looked as it does
after one that worked. Options now says so in red above the port, in English and Spanish,
until the newest settings are on disk: the file is as it was before, what was changed is in
use until the game closes, and **Save** tries again. The settings the disk refused are kept,
and the next save that gets through writes the newest there are, its own or those: an older
save that crossed a newer one that failed neither puts its own settings on disk nor turns
the notice off. And if the game dies between the write of
`settings.json.tmp` and its rename, that file stays behind with the token in it: it is
removed, without being read, the next time the addon loads its settings, before anything can
save.

If the file is there and cannot be loaded, because it cannot be read or does not parse, the
addon runs on the default settings, leaves the file as it is and **saves nothing by itself**:
the checkboxes, the buttons of the panel's bar and the quick access icon still work for that
load, but they are not written. Only **Save**, with the token pasted again, writes, and it
replaces that file; from then on everything saves as usual. Until then Options says so above
the port, in English and Spanish. Before this a file that did not parse was loaded as the
defaults and the first click on a checkbox wrote them, with an empty token, over it. A file
that is not there is a first run and nothing of this applies.

## Labyrinth farming panel

In Nexus Options, enable **Show Labyrinth farming panel / Mostrar panel de Laberinto**.
It is hidden by default, including for older `settings.json` files. The adjacent language
checkbox switches only this panel between Spanish and English; the position reset restores
the window if it was moved outside the screen. These controls save immediately without saving
a pending token edit. The host's font and DPI are retained.

Since 0.8.0 the panel follows David's sketch of 8 Oct 2026 and has a fixed shape:

```
▾ Tyrian · Laberinto        ◐ ×
────────────────────────────────
bolsas          stack
143             7g 7s 62c
37 b/h          8g 90s 37c
────────────────────────────────
Huecos: 63 libres
MF: 333%
Estado: ● Midiendo
```

- **Left column:** the positive **observed bags**, large, and under them the rate per hour.
- **Right column:** the gross trading-post price of a **stack of 250 bags**: the highest buy
  order, then the lowest sell offer (see "Bag price (`price2`)").
- **Slots:** the free bag slots, orange at 10 or fewer and red at 3 or fewer.
- **MF:** the Magic Find.
- **Status:** a dot, a text and a tooltip. The colour is never the only signal.

Slots and MF take their figure from the first of these that has one:

1. **What the addon's own reader read and verified in its last cycle** (see "Bag slots and
   Magic Find (read by the addon, not on the wire)"), while the reader is sampling.
   - Slots: the tooltip says "Read and verified by the addon" and gives the inventory window's
     counter, `Inventory: 97 used of 160`, and the bags.
   - MF is written bare, `MF: 333%`, with its three addends in the tooltip: luck, server (what
     the server pushed) and effects (food, boosters, banners). It is the total the hero panel
     adds up, without that panel's cap. While it is below the highest total of the session it
     turns orange, and the tooltip says how many points it fell and which addends
     (`Effects: from 53% to 3%`). Fractions are written with one decimal.
2. **What the plugin sends in `farm1`.**
   - Slots: the tooltip says they are the plugin's, whether they are a recent character's, and
     how old the reading is; an old one paints the line orange.
   - MF is written **`MF: 333% partial`** (`parcial`) and in grey. It is a value declared when
     the session started, or a partial one, and does not follow the game: the word and the
     colour are there so it is never taken for a live reading, and it never warns about a
     drop. The tooltip says so.
3. **`—`**, in grey.

When the addon's reader tried and got nothing, the line falls back to 2 or 3 and its tooltip
says "Addon reading: no coverage" with the reason: this game build is not the audited one,
structure not recognised, character unavailable, outside the read limits, misaligned pointer,
inconsistent data, an effect needs live state, changed while reading, read failed, or the read
ran out of time. No coverage is not something the player did, so nothing turns red for it.
Outside sampling (no session, a source conflict, an unsupported build) the reader's last
figures are not painted: they are of then, not of now, and the tooltip says the reader is not
sampling. The MF tooltip ends with the host's preparation state and, unless the figure is the
addon's own, with "temporary buffs unverified".

A cycle of the reader that comes back without a figure does not make the line blink. The panel
**holds the last verified reading for 5 seconds** (`panel::READING_HOLD`): while the reader
keeps returning none, the cell stays exactly as it was, the same figure in the same colour,
and only its tooltip changes, adding "Last reading N s ago" and the reason there is no new
one. After 5 seconds without a reading the line falls back as above. Two kinds of failed
cycle are held through:

- one of the two readers has no figure (`Changed`, `Deadline`, `ReadFailed`…), and
- the whole capture fails before they run, because the copy of the inventory changed under
  it, could not be read or ran out of time. The inventory status is then "reading unavailable"
  for the second until the next capture, and the tooltip says "the last capture failed".

A reading is as old as the cycle that read it, not as the frame that paints it. The reader's
output only changes when a cycle runs, so the instant of that cycle travels with it
(`SharedState::inventory_reading`): the 5 seconds count from there, and from 2 seconds on the
tooltip says the age even though the reader's last word was a figure. That is what a plugin
slow to confirm a sample looks like, and it is why the figures of a connection before the
current one, still in the reader's output after a reconnection, are not painted.

When the reader has really stopped the reading is dropped at once and does not come back: no
connection, the game closing, no negotiated source, a source conflict, an unsupported build or
storage down. A session that starts holds nothing of what was read before it.

The session's highest Magic Find, the one a fall is measured against:

- is fed only by a reading the reader has just returned, never by one that is being held, and
  only while the session measures: not while it prepares, and not once it is complete;
- is fed only by a reading whose own cycle ran once the session was seen measuring. The
  reader's last output stays in place until the next cycle, so the one that is there when a
  session starts can be up to 5 seconds older than the session: it is painted, as the last
  thing the reader verified, and it is not taken as the highest;
- only goes up;
- is each character's own, for the length of the session. When the game context names a
  different character, the highest of the one that was being played is put away under its
  name and the one that comes in gets back its own, or starts one if it has none: A's
  highest is still there after playing B, and B's is never A's. Nothing read of the
  character before is painted as the new one's, and a cycle during which the context changed
  feeds nobody's: the client discards what the readers said in it (see the limit on whose a
  reading is, under "Bag slots and Magic Find"). Going to character select and coming back
  with the same character changes nothing. It is kept for 80 characters
  (`panel::CHARACTER_PEAKS`), the one not played for longest going first, and all of it ends
  with the session;
- is the session's. `farm1` carries no session id, so another session is one that starts
  running after one that was not, or one whose declared duration goes back by more than a
  minute or to under a minute; the host sends a frame every 5 seconds, so a short `starting`
  may never be seen. A lost connection also ends it: what happened meanwhile is not known;
- is followed while the panel is closed too, so reopening it in a later session does not show
  a fall from an earlier one's highest.

In Options, three lines give the two readers' last pass, always in sight, for a screenshot
when a figure is missing:

```
Bags: read, 97 used of 160, 63 free (5 bags in 8 bag slots); bytes: 1480 / 16384; reads: 37
Magic Find: no coverage, Deadline (the read ran out of time); bytes: 41200 / 65536; reads: 603
Reader: last pass 1 s ago
```

They show the outcome with its exact reason, the bytes and reads of that pass against its
budget, taken from the reader's own constants, and how long ago the pass ran. These lines are
the reader's raw last pass: they do not hold anything, so they show a failed cycle the panel
is painting over, and "not read" for a capture that failed as a whole. A pass cut by the clock
has its own reason: `Deadline` means the 250 ms the two readers share, or the cycle's 750 ms,
ran out, and `Bounds` that a pass asked for more than its byte budget. The one pass they do
not show is a cycle the client discarded because the game context changed during it: the
lines stay those of the pass before, and its age goes on counting.

Under **Reader diagnostics**, four more lines say what the cycles and the panel take, counted
since the addon loaded:

```
Pass times in µs, last cycle (max): threads 210 (max 1900); inventory 3400 (max 5200); wallet 300 (max 450); bags 150 (max 300); Magic Find 9000 (max 41000); whole cycle 13200 (max 48900)
Captures: 1234 ok; 3 Changed; 1 ReadFailed; 0 Deadline; 0 Bounds; 12 other; epochs opened: 7
Most own threads in a cycle: 61 / 128
Panel frame in µs: mean 0.4, max 41.3, over 123456 frames (2345 computed)
```

- **Pass times**: microseconds each pass of the last cycle took and the longest it has ever
  taken: finding the game's context among the process's own threads, the inventory, the
  wallet, the bags, the Magic Find, and the whole cycle. "not run" is a pass the last cycle
  did not reach. The cycle has 750 ms, and the bags and the Magic Find 250 ms between them.
- **Captures**, by how they ended: `Changed` is an inventory that changed under the copy,
  `ReadFailed` a copy that failed with time left, `Deadline` one refused because the cycle's
  750 ms had run out, `Bounds` a count, a pointer or a budget out of bounds, and "other" no
  character to read, a profile that does not match or another build. "Epochs opened" is how
  many times the source has started an epoch (a `live_open`), over all its connections.
- **Most own threads**: against the 128 the reader stops at.
- **Panel frame**: what the panel's render callback takes, mean and longest, and how many of
  those frames had to compute the panel (see "What a frame of the panel costs").

They are the clock read around the passes the cycle already ran: no read of the game, no
pass and no guard is added or moved for them. None of them is sent to the plugin, in `live1`
or in `farm1`. They are there to answer in the game what has only been estimated outside it,
and none of these numbers has been looked at in a running game yet.

| Dot | Means | Text |
|---|---|---|
| green | connected, a session under way | the phase: "Measuring", "Preparing measurement"… |
| grey | connected, nothing measuring; or the game closing | "Waiting for session", "Session complete"… |
| orange | connected, a lasting problem to read in the tooltip | the phase, in orange: old data, an inventory source that cannot measure while a session needs it, or captures failing for more than 5 seconds |
| red | no connection, or a session error | "Offline", "Token missing", "Could not update"… |

Every cell has a tooltip, and what used to be lines of the panel is in the tooltip of the cell
it is about: duration, goal, progress and ETA, and the signed **net bags at close**, on the
observed bags; the range of an averaged rate and the notes about the rate, on the rate; which
side each price is, its unit price and why there is none, on the prices; inventory status,
wallet coverage, host connection, stale data and where to look after an error, on the status,
with or without a connection; "Verified Magic Find: no coverage" and the preparation notes on
MF. A cell whose tooltip reports a problem is painted orange, or red for an error; one with no
figure, or with a figure that is not a live reading, is grey.

The window has no native title bar, because ImGui's cannot hold a button of ours. Its own bar
keeps what the native one had and adds one button:

- the triangle folds the panel down to its bar, and unfolds it;
- drag the bar, or any empty spot of the panel, to move it;
- the contrast sign, a ring with its left half filled, removes the window's background and
  leaves the text, which then gets a dark outline so it stays readable over the game; a second
  click puts the background back. The sign is the same in both states: the panel itself shows
  which one it is in, and the tooltip says what a click does. Options has the same switch as a
  checkbox. Until 0.8.0 this button was a square, which between the fold triangle and the
  cross read as "stop";
- the cross hides the panel until it is re-enabled in Options or from the quick access bar.

Folded and background are saved in `settings.json`, like the panel's visibility. The three
buttons and the status dot are drawn, not written, so they do not depend on the host's font
having a glyph for them.

The addon never estimates a rate or advances the duration by itself. A bags ETA requires an
observation younger than 15 seconds. Duration countdowns remain available through observation
errors and unknown/old source age, while still requiring a fresh transport snapshot and active
session. The feed does not identify the source type. Temporary buffs and AFK are never
represented as verified.

No start/stop or goal-edit buttons are in this window: manage the session and preparation
in Hebra or Obsidian. Neither hiding the panel nor losing the bridge stops a session.

The host advertises `{"v":3,"type":"farming_cap","nonce":…,"tag":"farm1"}` only
after authentication. The addon subscribes once using `farming_sub`, sharing the same
outgoing sequence as context, heartbeat and alert acknowledgements. A host without that
capability gets no subscription and the status says "No panel", in orange. Incoming
flat `farming_state` frames stay within 512 bytes, require exact keys, closed enums and
int32-or-null metrics, and are accepted only with the current 22-character base64url nonce
and increasing positive int32 farming sequence. Their sequence is independent of alert
deduplication and they are never ACKed.

Snapshots expire after 15 seconds of monotonic time, or immediately on disconnection.
The window retains the last reading, paints it orange with **Stale data / Datos antiguos** in
its tooltips and in the status, and removes
ETA. Counts, declared duration and rates freeze; a transport refresh never renews an inventory
observation or a character-slot observation. The feed does not send account/character
identity, builds, economic details, inventory contents, or free-form text. No new API
polling happens inside the addon.

QA limits for 0.4.0: portable parser/state and loopback tests cover the feed, and the Windows
cross-build checks the ImGui code and DLL dependencies. These checks cannot certify panel
placement, text contrast, keyboard navigation or Nexus load in a running Guild Wars 2
session; Fedora/Wine/game runtime QA remains pending until measured on that client.

### Bag price (`price2`)

The right column of the panel shows the trading-post price of a stack of 250 Labyrinth bags
(item 36038): first the **highest buy order** (what selling at once is offered), then the
**lowest sell offer**. The figures are **gross, as the trading post shows them**: no fee is
taken off, and a stack is exactly 250 times the unit price. They are written as `8g 62s 50c`.
The tooltip of each says which side it is and its unit price.

The two prices are always there. With no figure they say `—` and the tooltip says why: no
connection, a host without the price, no session ("The price is read during a session"),
"Price not read yet" (no frame yet, `pending`, or a transport of 15 s or more), "No quote",
or, with the column in orange, "Price expired (11 min ago)". One side alone can also be `—`.

The price comes from the public trading-post data and can be up to about **2 minutes** old,
and older if the plugin's network fails (it expires after 10 minutes). It is not "live" and the
panel never says so. Outside an active session there is no figure.

`price2` replaces `price1`, which had the same frames with figures net of the trading-post
fees. The tag is what tells them apart, and this addon reads `price2` only. With a plugin that
announces `price1`, or none, the capability is discarded without closing the connection, the
addon never subscribes, and the two prices say `—`: a net figure is never painted under a
gross label. Addon 0.7.1 and older do the same with a plugin that announces `price2`.

Wire: `price_cap` (`v,type,nonce,tag`) after authentication; the addon sends one `price_sub`
only after receiving it on that connection, on the same outgoing sequence as `farming_sub`; then
`price_state` frames (12 exact keys, `st` one of `ok`, `idle`, `pending`, `stale`, amounts int32
or null, null whenever `st` is not `ok`). Same 512-byte cap, 22-character nonce, increasing
sequence and 15-second monotonic transport expiry as `farm1`; disconnecting removes the figures
at once. `farm1` and `live1` are unchanged. No item id, name or account travels in the feed.

### A panel that does not jump

The panel has the same lines in every state: each cell is always there and says `—` without a
figure, so the window never grows or shrinks. Its width does not follow the content either:
it is reserved once from the longest text each part can hold (`panel::width_samples`: a rate
of `9999–9999 b/h`, a price of `99999g 99s 99c`, the longest status text), measured with the
widest digit of the host's font, so a text that changes moves nothing. Only a figure beyond
those, such as a stack of 100 000 g, widens it.

The rate is a range when the host sends one, `480–560 b/h`. While the range is wide it is
shown as one number, its average, with `~` in front and the range in the tooltip: it turns
into the average when (high − low) / average goes above 0.30 and back into a range when it
goes below 0.20, and between the two it stays as it was, so it does not alternate. A range
one unit wide (`37–38`) is one number rounded down and up, which is what a live session
sends, and is shown as `37 b/h`. With only a lower bound it is `≥480 b/h`.

"Rate not available yet", "Last recorded rate" and "Last reading ago Xs" / "No reading" are
the rate's tooltip, and while any of them applies the rate is painted in the warning colour
(orange):

- no rate yet: the host has sent no band;
- last recorded rate: the transport is 15 s old or more, or the reading is, or has no age;
- reading age: only while a session is starting, active, stopping, provisional or in error,
  and the transport is 15 s old or more, the host reports an error, there is no reading, or
  the reading is 15 s old or more.

"Last recorded rate" used to follow the 5 s freshness of the source. The host sends a frame
every 5 s and the age keeps counting in between, so that line came and went in normal
measurement and made the window jump. The bags ETA followed the same 5 s. Both use one
threshold: the observation counts as current until it is 15 s old, with a fresh transport.

The inventory states that come and go in normal measurement ("waiting for confirmation",
"unresolved quantities") are in the status tooltip and do not change its colour. Nexus's
Options show the inventory status and the wallet coverage always.

The status dot does not blink either. One capture that fails as a whole, or one read of the
wallet that fails, changes neither the dot nor the status text: for 5 seconds
(`panel::READING_HOLD`) from the last capture that worked, or the last one that listed the
wallet, they stay as they were and only the tooltip says "The last capture failed" or the
wallet's reason. After that it is a lasting problem and they turn orange. It counts from the
instant of that capture, the same one the readings are dated by. A reader that has stopped (no
connection, the game closing, no negotiated source, a source conflict, an unsupported build,
storage down) changes the status at once, and so does a failure when no capture of this
connection has worked yet: there is nothing to hold on to.

The source is also unavailable while there is no character in a map, at character select and
on a loading screen, and then no capture has failed: there is nothing to read. The panel
tells the two apart by the game context the addon already reports to the plugin (`gameplay`
or not, from `NexusLink` and the Mumble Link; no reader is involved). Without a character in
a map the status tooltip says "No character in a map (character select or loading screen)"
and the Slots and MF tooltips "Addon reading: no character in a map", where they used to say
that the last capture failed. Nothing else changes: the readings are held for the same 5
seconds, so a short loading screen moves nothing, and a longer stay turns the dot orange as a
source that cannot measure while a session needs it.

Still to be looked at in the game; none of the panel's painting has been seen there:

- That the two readers work in this DLL at all: they have only run against fixtures. The
  tooltip of Slots against the inventory window's counter (used of total, and free), MF
  against the hero panel, and removing an effect to see MF fall, turn orange and name the
  addend. If either line says `—` or `partial`, the "Bags", "Magic Find" and "Reader" lines
  of Options say why.
- What the plugin does with one failed capture. The addon tells it the source is unavailable,
  and if the plugin's next `farm1` frame, at most 5 s later, carries `err: observe`, the status
  turns red with "Could not update" and the rate orange until the frame after. That would be
  the host's own word in a frame, not something this panel holds through.
- Whether the two readers fit the 250 ms they share. That figure is an estimate: if it is
  short, the diagnostics line of Magic Find says `Deadline`, and if it stays short for more
  than the 5 seconds the panel holds a reading, MF falls back to the plugin's figure or `—`.
- The count of used positions: it was not part of the external probes at all, so "used of
  total" against the inventory window is the first time it is checked.
- That the panel paints at all as described: the two columns side by side without touching,
  the large figure, the dot, and the three drawn buttons of the bar.
- That the bar's buttons take the click, that the panel moves when dragged by its bar or an
  empty spot, and that folding leaves only the bar.
- That without the background the outlined text reads well over the game, and that the
  window still takes the mouse over its area even though nothing is painted behind the text.
- That the tooltips appear with the game in the foreground.
- That `—`, `–`, `≥` and `~` have a glyph in the host's font.
- When the source loses coverage, as on a change of map, the host sends `err: observe`: the
  status turns red and says "Could not update" until coverage is back. The line stays where it
  is; only its colour and text change.

### What a frame of the panel costs

The panel used to be worked out from scratch on every frame, also to paint only the bar of a
folded one: the texts of its cells and their tooltips, some two hundred allocations, and the
forty measurements its reserved width takes. At the game's frame rate nearly every frame
repeats the one before, so the panel is computed only when it can have changed
(`panel::PanelCache`):

- the shared state changed: every setter of something the panel is painted from bumps a
  counter (`SharedState::panel_generation`), and setting what was already there does not;
- the language changed, or what the frame does with the panel (closed, folded, painted);
- a whole second went by since one of the instants the panel counts from: a `farm1` or a
  price frame, the reading each of the two lines has, the last capture and the last wallet
  that worked. The ages are written in whole seconds and the 2 s, 5 s and 15 s rules turn on
  whole seconds of those, so with the same state nothing can change in between;
- a quarter of a second went by, whatever else.

With the first three the panel that is kept is the very panel that frame would compute, and
nothing that is painted changes: the 5 seconds a reading is held, the dot, the lines and the
ages are where they were. `core/tests/panel_cache.rs` plays a session of 80 seconds, some
6000 frames a few milliseconds apart plus frames exactly on those whole seconds, against a
panel computed on every frame, and compares the view and the memory after each one. The
quarter of a second alone does not give that: the 5 seconds of a held reading would run out
up to 250 ms late, and that test fails with it. It stays as a bound, for a setter that some
day forgets the counter. In that session about one frame in ten is computed (some 650 of
6400), the frames forced onto the whole seconds included.

Folded, no cell is built at all: the memory is moved on as a painted frame would leave it
(`panel::observe_folded`), which is the rate's range-or-average choice besides what a closed
panel already followed. The width samples are built once per language, and the reserved
width is measured once per language and font, the font being its size, the frame height,
the line height and the width of the ten digits.

### Quick access icons

Two icons appear in Nexus's quick access bar (the row of addon icons beside the game menu):

- **Labyrinth panel** shows or hides the "Tyrian · Laberinto" window. It is the same setting as
  the Options checkbox "Show Labyrinth farming panel", saved the same way.
- **Tyrian Companion options** brings up the addon's own Options window (Nexus gives addons no
  call to show its Options window on their section, so this window paints exactly the same
  content and closes with its cross).

Each icon triggers a Nexus keybind, `KB_TYRIAN_COMPANION_TOGGLE_PANEL` and
`KB_TYRIAN_COMPANION_OPEN_OPTIONS`, created **without a key**; assign one in Nexus's Input
binds if you want. The click works without it. Tooltips follow the panel's language setting and
are refreshed when you change it. If an icon's texture does not load, the failure is logged and
that icon does not appear; nothing else is affected. Nothing here sends input to the game: it only
shows and hides this addon's windows. All of it is removed when the addon unloads.

The icons are four PNG files embedded in the DLL: `addon/assets/qa-panel.png`,
`qa-panel-hover.png`, `qa-options.png` and `qa-options-hover.png`, 32x32 RGBA, full colour (the
pumpkin for the panel, the monster for the options; the hover is the same drawing with a 1 px white
halo). They are David's drawings, loaded as they are with no tint or conversion. Replacing them is
changing those four files and rebuilding.

## Reconnecting

The addon does not need the plugin, or the game, to start first. If there is no server
listening yet — the common case right when the game launches, since the plugin lives inside
Obsidian and the player is free to start either one first — the addon just keeps retrying,
forever, without surfacing that as an error.

The waits between retries come from the table `[250, 500, 1000, 2000, 5000]` ms, and the
first one after a failure is **500 ms, not 250**: an attempt that fails moves one step up the
table before its wait is taken. With nobody listening the addon tries, waits 500 ms, tries,
waits 1 s, then 2 s, and then 5 s between tries for as long as it takes. The 250 ms is only
the wait after a connection that had lived for 10 seconds since the plugin's `welcome`:
that one starts the table over and is retried a quarter of a second after it drops, and if
that retry fails the waits are 500 ms, 1 s, 2 s and 5 s again.

A connection that drops is retried that way, well inside the ten minutes the plugin waits
before it closes the session, so a short hiccup continues the same session instead of
starting a new one. One the plugin welcomes and closes before those 10 seconds is one more
step up the table, like one that never got a `welcome`: a plugin that keeps closing at once
is retried after 500 ms, 1 s, 2 s and then every 5 seconds, not four times a second.
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
- passive wallet fixtures over the audited guard bytes: the live-shaped 55-key map, a covered
  zero, empty and absent maps, wrong vtables and getters, a changed guard byte, malformed
  headers and pointers, failed copies, wrong hashes, unreachable and repeated keys, a count
  that disagrees, out-of-range keys and balances, concurrent changes and the wallet budget;
- passive bag and Magic Find fixtures over synthetic guard contents (no byte of the game),
  with the production digests, RVAs and slots required to equal verbatim copies of the two
  audited probe profiles (`core/tests/fixtures/bag_capacity_profile.json` and
  `magic_find_profile.json`, digests only): the
  live shapes of 8 October 2026 (414 capacity and 101 free; 333.0 and 363.0), covered zeroes,
  every identity on both routes, the bag class and definition type, both pointer rules with
  valid bytes waiting at the misplaced address, bucket hashes and node keys, unsupported
  records, stacking, the boon rule, concurrent changes, failed copies and both budgets, and
  the most buffs a Magic Find pass fits in its budget, with the first that is `Bounds`, in
  tables of 512 and 256 buckets and three ways of holding content, now and at 0.8.1;
- live1 canonical wire fixtures, 512/513 cap, old-host negotiation, epochs/baselines, context
  equality, ACK isolation, source/storage failure, partial samples and bounded chunks, and
  `currencies:listed` rows: their order and chunking, a failed wallet read next to a valid
  inventory, and the largest sample against every live1 limit;

- `farm1` byte cap (512 accepted, 513 rejected), exact keys and duplicate-key rejection,
  numeric bounds, nullable values, every enum, capability negotiation with older-server
  compatibility, nonce and sequence isolation, no farming ACKs, independent observation
  ages, monotonic TTL and immediate disconnect invalidation;

- the Labyrinth panel as data (`core/tests/panel.rs`): the sketch itself, every state of
  every cell, the same cells in every state, the rate's two thresholds and that it does not
  alternate between them, slots and Magic Find from the addon's reader, from the plugin and
  from neither, every no-coverage reason of both readers with and without the plugin's
  figure, the fall of a verified Magic Find from the session's highest, the hold of the last
  verified reading (one failed cycle or one capture that fails as a whole changes neither
  cell, six seconds let go, a stopped reader at once, a new session, and that a held reading
  does not move the highest), a reading being as old as its own cycle, no character in a map
  told apart from a capture that failed, a reading from before the session or of another
  character not becoming the highest, another session noticed
  with the panel closed and without a `starting` frame, a complete session not feeding the
  highest, the two diagnostics lines of Options, and that every text the panel produces is
  covered by a width reserved for its own cell. None of the painting itself is tested. On the
  real loop (`core/tests/client_live.rs`), with the panel following every frame: a cycle
  during which the character changes leaves nothing of the readers' output for the panel and
  no highest, and a cycle whose context held leaves it, dated, also when its capture failed;
- the panel that is kept between frames (`core/tests/panel_cache.rs`): the same view and the
  same memory as a panel computed on every frame, frame by frame through a whole session, a
  folded panel leaving the memory a painted one would, and every setter of what the panel
  paints moving the counter the cache looks at;
- the check of the executable (`core/src/executable.rs`, `core/src/verdict.rs`), over a file,
  a hash and a memory handed in: a size out of range decided without reading a byte, the
  digest deciding the build, the hash going on across slices with every byte in it once, a
  header that is not the certified one decided for good, everything the system can fail at
  deciding nothing, and a verdict kept only when final, an unfinished slice neither kept nor
  waited for, the time of the unfinished slices added up and counted from nothing after a
  failure, the tenth second of them being the overdue one, and a failure tried again after
  its wait. On the real loop
  (`core/tests/client_live.rs`), a source whose verdict is pending puts nothing but heartbeats
  on the wire, then opens with its baseline as always, or says `unsupported_build` once, or
  `read_failed` for a failure of the system; one whose verdict is overdue says `read_failed`
  once, is asked for its next slice on every pass and never for a sample, and opens with its
  baseline when the hash is over. The adapter that opens the real file and calls the system's
  SHA-256 is not tested, and neither is the one line in it that asks whether the hash is
  overdue;
- the counters of the reader diagnostics (`core/src/perf.rs`): last and longest time of a
  pass, captures counted by how they ended with a copy refused by the clock kept apart from
  one that failed, the lines Options shows, the frame's mean in tenths of a microsecond, and
  every `live_open` counted as one epoch opened. The clock readings themselves are taken in
  the Windows adapter and are not tested;

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
  and removed from disk on load; a save that a concurrent reader never sees half of; a file
  that cannot be loaded told apart from a missing one, left as it is and not replaced by an
  automatic save until an explicit one), and the token never showing up in `Debug` output.

It does not, and cannot, cover the actual Nexus load/unload cycle, what `NexusLink` and the
Mumble Link really contain in each game state, the `WndProc` callback, or the ImGui panel —
those need a running game and are exercised by hand: build the DLL, drop it into
`<GW2>/addons/`, launch the game, and check Nexus's own log window for `Loaded addon`.
