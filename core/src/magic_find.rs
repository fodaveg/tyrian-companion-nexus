//! Passive, bounded Magic Find for the same certified GW2 build.
//!
//! The client stores no total. Its attribute widget (`AtAttribute.cpp`, RVA `0x3E4898`) sums
//! three stored inputs every time it paints the hero panel:
//!
//! ```text
//! min(cap, account luck level + modifiers(0x71) + [modifiers(0x72) while a boon is applied])
//! ```
//!
//! where `modifiers(type)` (RVA `0x12C1050`) is the sum of the records the server pushed to the
//! character's buff manager plus the records of that type on every applied buff. This module
//! reads those inputs and repeats the sum; it calls nothing. The route and its RVAs come from
//! `tyrian-companion/docs/audit/loot-mf-probe`, whose external probe matched the hero panel in a
//! running game on 2026-10-08 at 333.0 and, after a buff change, at 363.0.
//!
//! Only the constant formula is evaluated. A counted record that needs a game mode, a trait or
//! a state condition, or another formula, makes the whole value [`Uncovered::Unsupported`]:
//! never a partial total. The display cap is a content number and is not read, so the total is
//! the uncapped sum.
//!
//! Each object on the route is copied once per look, as one block from its vtable to the last
//! field used here, and the vtable slots are copied one run per vtable. A block is only a way
//! to ask for less: the fields interpreted and compared are the ones named below, and no other
//! byte of a block is. A pass over the live shape of 2026-10-08 (91 buffs) asked 557 times for
//! 22620 bytes when every field was its own copy; in blocks it asks 349 times for 29328.
//!
//! [`magic_find_cached`] also keeps, between passes, what the applied buffs hold as game
//! content: each instance's effect and definition, and each definition's records. That is the
//! one relaxation of this reader, accepted by the owner on 2026-10-08: content is read whole at
//! most every [`MAX_CONTENT_AGE`] instead of every pass. Every pass still copies the route with
//! its identities, the slots, both table headers, the pushed table, the buckets, every node
//! and the first 48 bytes of every definition in use, and still takes the second look. What is
//! kept stands for a pass only if the owners, the table header and every byte of the buckets
//! are the ones of the pass that read it, every node leads to an instance it knows, and every
//! definition's first 48 bytes are the ones it read. Otherwise that same pass reads everything
//! again, so a buff applied or removed changes the value in the pass that sees it. Over the
//! live shape a pass that verifies asks 164 times for 19556 bytes.
//!
//! What a verifying pass does not see, for at most [`MAX_CONTENT_AGE`]: an instance whose
//! effect, state or definition pointer changes in place under the same key, node and address;
//! and a definition whose modifier group or records change in place while its first 48 bytes
//! do not. [`magic_find`] keeps nothing and has no such window. The addon reads through
//! [`CachedMagicFind`], which also forgets everything when the client discards a cycle.

use crate::inventory::{BuildProfile, Memory, Reader};
use crate::layout::Layout;
use crate::passive::{
    content, context_checked, dword, heap, identity, object, qword, table, verify, Guard,
    Uncovered,
};
use crate::wallet::currency_hash;
use std::collections::{BTreeMap, BTreeSet};
use std::time::{Duration, Instant};

// The vtables of the certified build, for the tests that fix them against the audit. The reader
// takes them from its profile's layout, never from here.
pub const CHAR_CONTEXT_VTABLE: u64 = Layout::CERTIFIED.char_context_vtable;
pub const CHARACTER_VTABLE: u64 = Layout::CERTIFIED.character_vtable;
pub const CHARACTER_AGENT_VTABLE: u64 = Layout::CERTIFIED.character_agent_vtable;
pub const COMBATANT_VTABLE: u64 = Layout::CERTIFIED.combatant_vtable;
pub const PLAYER_VTABLE: u64 = Layout::CERTIFIED.player_vtable;
pub const PLAYER_STATS_VTABLE: u64 = Layout::CERTIFIED.player_stats_vtable;
pub const BUFF_MANAGER_VTABLE: u64 = Layout::CERTIFIED.buff_manager_vtable;
/// Installed by the table node constructor (RVA `0x12C23A0`); 91 of 91 nodes in the live run.
pub const BUFF_NODE_VTABLE: u64 = Layout::CERTIFIED.buff_node_vtable;
/// `RewardCreatureAll` and `RewardCreatureAllBoon` in the client's modifier name table.
pub const MAGIC_FIND: u32 = 0x71;
pub const MAGIC_FIND_BOON: u32 = 0x72;
const CONSTANT_FORMULA: u32 = 6;

/// The largest buff table the reader accepts, in buckets. It was 512. The game doubles the
/// table and does not shrink it: 256 buckets held 91 buffs on 8 October 2026, and 512 held from
/// 64 to 199 on 13 October, in a crowd. A table of 1024 would have been `Bounds` on every pass
/// from the next doubling on, for as long as the game stayed open. The buckets are copied twice,
/// 48 bytes each: 49152 of [`MAX_BYTES`] for a table of 1024.
pub const MAX_CAPACITY: u32 = 1024;
/// The most records one definition may declare. It was 32, a guess: the one session the reader
/// was audited against (8 October 2026) had definitions of one record. On 13 October 2026 an
/// applied effect whose definition has 52 left Magic Find without a figure for as long as it
/// was applied (`Bounds`, by the log of that session), though the pass fitted its budget.
///
/// The game has no maximum of its own: its sum (RVA `0x12C1050`) walks the records up to the
/// count the definition gives, and the only check on the way is the content array's
/// `index < m_count`. So this is a bound on a count read from the game, not a rule of it: a
/// definition is copied whole or not at all, and a count that is not one makes no copy. At 256
/// a definition is 18492 bytes in seven copies, a fourteenth of [`MAX_BYTES`], which is what
/// really bounds a pass: all the definitions of one pass together hold fewer than 3641
/// records, whatever each of them declares, and a pass that would need more is `Bounds`.
pub const MAX_MODIFIERS: u32 = 256;
pub const MAX_PUSHED: u32 = 256;
pub const MAX_LUCK_LEVEL: u32 = 1000;
const MAX_PLAYER_ID: u32 = 0xffff;
const MAX_ABS_PERCENT: f32 = 10_000.0;
/// One whole pass, guards included on the first cycle of a build (6054 bytes). A pass that does
/// not fit is no coverage; the budget is never raised at run time.
///
/// It was 65536, sized on the external probe's live passes of 2026-10-08: 28706 and 32814
/// bytes, guards included, with 81 and 92 buffs in a table of 256. On 13 October 2026, with
/// the character in a crowd, the log of the reader showed passes over 168 to 199 buffs of 87
/// to 93 definitions, in a table of 512, asking for just under or just over that: 65284 and
/// 65472 bytes read, and `Bounds` at 185 and at 199 buffs, back and forth 26 times in two
/// and a half minutes.
///
/// What a pass asks for: 2368 bytes for the route twice and the slots, 48 for each bucket of
/// the table (it is copied twice), 92 for each buff and, for each definition in use, 60 and 72
/// more for each of its records. With the mix of that session, about a definition for every
/// two buffs and 244 bytes a definition, that is some 214 bytes a buff: this budget holds a
/// table of [`MAX_CAPACITY`] buckets with over 900 buffs in it.
///
/// Time is not what it costs. Those passes took 170 to 730 microseconds for some 65000 bytes,
/// so one that uses this whole budget takes about 3 milliseconds, of the 250 the bag and
/// Magic Find passes share (the adapter's `EXTRAS_TIME`); a pass that is out of time is no
/// coverage either, and that clock is the adapter's, not this module's.
pub const MAX_BYTES: usize = 262_144;
const BUCKET: usize = 24;
const PUSHED: usize = 12;
const MODIFIER: usize = 72;
const HASH_TABLE: &str = "hash_table";
/// One copy per object, from its vtable to the end of the last field read: the character
/// context to the player pointer at `+0xa0`, the character to the cached player id at `+0x220`,
/// the player stats to the luck record at `+0x18`, the buff manager to its mode at `+0xf0`, and
/// a table node to its key at `+0x18`.
const CHAR_CONTEXT_BLOCK: usize = 0xa8;
const CHARACTER_BLOCK: usize = 0x224;
const STATS_BLOCK: usize = 0x28;
const MANAGER_BLOCK: usize = 0xf4;
const NODE_BLOCK: usize = 0x1c;
/// A buff instance is copied from its effect id at `+0x28` to the end of its definition
/// reference at `+0x68`.
const INSTANCE_FROM: u64 = 0x28;
const INSTANCE_BLOCK: usize = 0x40;
/// The longest run of [`SLOTS`] on one vtable: `CHAR_CONTEXT_VTABLE`, slots `0x68` to `0x118`.
const SLOT_SPAN: usize = 0xb8;
/// The start of a buff definition: flags, stacking rule, category, the pointer to its
/// modifier group and the dword that must not be zero.
const DEFINITION_HEAD: usize = 0x30;
/// How long the content one whole pass read may stand for the passes that only verify it.
/// A [`MagicFindCache`] this old, or older, is not used: that pass reads everything again.
pub const MAX_CONTENT_AGE: Duration = Duration::from_secs(30);

/// The route getters, the widget's case, the modifier sum and its helpers, and the hash table.
pub const fn guards_of(l: &Layout) -> [Guard; 19] {
    [
        Guard {
            name: "local_player_getter",
            rva: l.local_player_getter,
            size: 8,
            sha256: "8fd42236905c9ccb06782e7a64df231d2be5f1c373f520fb9ff3a1a087796392",
        },
        Guard {
            name: "controlled_character_getter",
            rva: l.controlled_character_getter,
            size: 29,
            sha256: "1f8522d1275fdf3efb6c9b9c7d7ae54d914e2723422263f0073ce6d09a8a830b",
        },
        Guard {
            name: "player_by_id_getter",
            rva: l.player_by_id_getter,
            size: 25,
            sha256: "7282c8c69835b41f12c3ddad5eff3e4955cc2fbcbb6514295e510c8e16244086",
        },
        Guard {
            name: "player_stats_getter",
            rva: l.player_stats_getter,
            size: 8,
            sha256: "3e958f563d9a1a4621232a43557373e42652bb1280fbfa2214f0d767abf7f16e",
        },
        Guard {
            name: "account_luck_level_getter",
            rva: l.account_luck_level_getter,
            size: 4,
            sha256: "e27121b8816e0aef2ed29676899d26e79f66af7e5ee5aed902be7208122bb635",
        },
        Guard {
            name: "combatant_cast",
            rva: l.combatant_cast,
            size: 14,
            sha256: "39f54bd1c6a5da3b35f2d3060c65179833ad00f2db5cb9f73dd75a67d3e09faf",
        },
        Guard {
            name: "buff_manager_getter",
            rva: l.buff_manager_getter,
            size: 8,
            sha256: "6b185d33a0706874cb3e05bca6abc44f8ea444d6a7e1e6548f7f0b263040af5b",
        },
        Guard {
            name: "is_player",
            rva: l.is_player,
            size: 24,
            sha256: "1a16f6adc080470acfd8f3d49b216607ece022ee3006e2430d7397ebf6fc164e",
        },
        Guard {
            name: "player_id_getter",
            rva: l.player_id_getter,
            size: 54,
            sha256: "a701c55fe12ab50346fda01b72eb72156039b838ab3da4ad0059014fdd46b891",
        },
        Guard {
            name: "modifier_value",
            rva: l.modifier_value,
            size: 129,
            sha256: "a581f4a80d24c21f359d6b3ee8a93183db4cc533230c6de1b68188aa553cb7e1",
        },
        Guard {
            name: "pushed_modifier_sum",
            rva: l.pushed_modifier_sum,
            size: 212,
            sha256: "84b64e88e0cfcd72545ceb0724c6c3b1214eac9f26e45a0cf04c037ba273ed88",
        },
        Guard {
            name: "buff_iterator",
            rva: l.buff_iterator,
            size: 188,
            sha256: "698db876d7edf85250891673fe1d791224a161b1489d90147497ba7c91abbf18",
        },
        Guard {
            name: "buff_filter",
            rva: l.buff_filter,
            size: 142,
            sha256: "7b44974ea06ed7f392fde2572356338fde9dc4eba4922634d480716d6a20abce",
        },
        Guard {
            name: "buff_lookup",
            rva: l.buff_lookup,
            size: 296,
            sha256: "da6f2f3399943312c769046a68e5612e416d4af1c663cbf0108895acdf32aeea",
        },
        Guard {
            name: "modifier_sum",
            rva: l.modifier_sum,
            size: 2536,
            sha256: "d42282064b43fe101e6c0296e8d39d56b12241736a56b78b20dfb91c96c2a3f2",
        },
        Guard {
            name: "modifier_formula",
            rva: l.modifier_formula,
            size: 816,
            sha256: "03cf6e4c30459e903354441baf6fddf5a18a642b01d8de2cee51296b9a19e500",
        },
        Guard {
            name: "magic_find_case",
            rva: l.magic_find_case,
            size: 241,
            sha256: "189467c9767e02761b8847a53bc2e5e81fd5cc7b9eddb3d35d44341a9180a48a",
        },
        Guard {
            name: "attribute_modifier_map",
            rva: l.attribute_modifier_map,
            size: 296,
            sha256: "f3365e3fff838ba902f21415c68560a92bc466f2a02c9af21eb0c6f41ef0cc3d",
        },
        Guard {
            name: HASH_TABLE,
            rva: l.hash_table,
            size: 1024,
            sha256: "7a5e36b78f411ef1d20f94aec5c1e324e5f43b47e1b084ea392d596daa25bed6",
        },
    ]
}
/// Vtable, slot and the guarded code that slot must select, where `layout` places the vtables
/// and the code; the slots are fixed.
pub const fn slots_of(l: &Layout) -> [(u64, u64, u64); 12] {
    [
        (l.char_context_vtable, 0x68, l.controlled_character_getter),
        (l.char_context_vtable, 0x70, l.local_player_getter),
        (l.char_context_vtable, 0x118, l.player_by_id_getter),
        (l.character_vtable, 0x100, l.combatant_cast),
        (l.combatant_vtable, 0x38, l.buff_manager_getter),
        (l.character_agent_vtable, 0x68, l.is_player),
        (l.character_agent_vtable, 0x18, l.player_id_getter),
        (l.player_vtable, 0x320, l.player_stats_getter),
        (l.player_stats_vtable, 0x30, l.account_luck_level_getter),
        (l.buff_manager_vtable, 0x18, l.modifier_value),
        (l.buff_manager_vtable, 0x20, l.pushed_modifier_sum),
        (l.buff_manager_vtable, 0x28, l.buff_iterator),
    ]
}
/// The guards and slots of the certified build, for the tests that fix them against the audit.
pub const GUARDS: [Guard; 19] = guards_of(&Layout::CERTIFIED);
/// The guards `profile`'s build must hold: the ranges where its layout places them, each with
/// the digest its executable file has there ([`crate::passive::GuardDigests`]).
pub fn guards_in(profile: BuildProfile) -> [Guard; 19] {
    crate::passive::with_digests(guards_of(profile.layout), &profile.digests.magic_find)
}
pub const SLOTS: [(u64, u64, u64); 12] = slots_of(&Layout::CERTIFIED);
// `BuildProfile::checked` only accepts a layout whose image reaches the TLS index, which lies
// beyond every address this module reads inside the executable ([`Layout::validate`] checks it
// for any layout; these are the two that bound this module's slots in the certified one).
const _: () = assert!(Layout::CERTIFIED.buff_manager_vtable + 0x30 <= Layout::CERTIFIED.tls_index);
const _: () = assert!(Layout::CERTIFIED.player_vtable + 0x328 <= Layout::CERTIFIED.tls_index);
// One definition at its bound, with its head and its group, is under a third of a pass's
// budget: the bound on records can never be what lets a pass ask for more than `MAX_BYTES`.
const _: () = assert!((DEFINITION_HEAD + 12 + MAX_MODIFIERS as usize * MODIFIER) * 3 < MAX_BYTES);
// The reader gives no pass more than `MAX_READER_BYTES`, whatever budget it is asked for: a
// budget above it would be one the pass does not have. And the largest table, copied twice,
// has to leave most of the budget to what hangs from it.
const _: () = assert!(MAX_BYTES <= crate::inventory::MAX_READER_BYTES);
const _: () = assert!(MAX_CAPACITY as usize * BUCKET * 2 * 4 < MAX_BYTES);

/// The hero panel's Magic Find, uncapped, with the three addends the client sums.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct MagicFind {
    /// `luck + pushed + buffs` in percentage points, summed in single precision like the client.
    pub total: f32,
    /// Account luck level: the base the API also gives. On its own it is not the Magic Find.
    pub luck: u32,
    /// Records the server pushed to the buff manager.
    pub pushed: f32,
    /// Records of the applied buffs: food, boosters, banners.
    pub buffs: f32,
    /// Whether the boon-only modifier type counted in `pushed` and `buffs`.
    pub boon: bool,
}

/// What the last cycle can say about Magic Find, for local use only.
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub enum MagicFindCoverage {
    /// No read was attempted: no negotiated sample yet, or the inventory route failed.
    #[default]
    NotRead,
    Read(MagicFind),
    Unavailable(Uncovered),
}

/// Proof that the static ranges of a hash-verified build hold the audited bytes, plus the hash
/// table those bytes carry. Verify once per build, not per cycle.
#[derive(Debug, Clone)]
pub struct MagicFindProfile {
    base: u64,
    layout: &'static Layout,
    table: [u32; 256],
}
impl MagicFindProfile {
    pub fn verified<M: Memory>(
        r: &mut Reader<M>,
        profile: BuildProfile,
    ) -> Result<Self, Uncovered> {
        Self::build(r, profile, &guards_in(profile))
    }
    /// For this crate's fixtures only; the addon calls [`Self::verified`] and nothing else.
    /// Fixtures carry no bytes of the game, so they stand for the executable with guard
    /// contents and digests of their own. The ranges must still be exactly the audited ones,
    /// where the profile's layout places them: an empty, shorter or shifted list is refused
    /// before anything is read.
    #[doc(hidden)]
    pub fn verified_against<M: Memory>(
        r: &mut Reader<M>,
        profile: BuildProfile,
        guards: &[Guard],
    ) -> Result<Self, Uncovered> {
        if !crate::passive::same_ranges(guards, &guards_of(profile.layout)) {
            return Err(Uncovered::Guard);
        }
        Self::build(r, profile, guards)
    }
    fn build<M: Memory>(
        r: &mut Reader<M>,
        profile: BuildProfile,
        guards: &[Guard],
    ) -> Result<Self, Uncovered> {
        let mut words = None;
        verify(r, profile.base, guards, |guard, bytes| {
            if guard.name == HASH_TABLE && bytes.len() == 1024 {
                let mut table = [0u32; 256];
                for (index, word) in table.iter_mut().enumerate() {
                    *word = dword(bytes, index * 4);
                }
                words = Some(table);
            }
        })?;
        Ok(Self {
            base: profile.base,
            layout: profile.layout,
            table: words.ok_or(Uncovered::Guard)?,
        })
    }
    /// The engine's key hash over this build's table, for fixtures that fill a buff table.
    pub fn key_hash(&self, key: u32) -> u32 {
        currency_hash(key, &self.table)
    }
}

/// What a refused record asks for through one of its four conditions, for the local log. No
/// address: what the record names is told by a dword of it, or not at all.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Condition {
    /// The record names something here.
    pub present: bool,
    /// The dword at `+0x28` of what it names, where an effect keeps its id. `None` when it was
    /// not copied: nothing named, a pass that does not probe, or a copy that failed.
    pub word: Option<u32>,
    /// For an effect condition, whether that effect is among the applied ones.
    pub applied: Option<bool>,
}

/// The counted record that made a pass end in [`Uncovered::Unsupported`], as the game holds it:
/// which applied effect carries it and every field the client's sum (RVA `0x12C1050`) looks
/// at. Local diagnostics only; it says what the reader would have to evaluate.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct RefusedRecord {
    /// The id of the applied effect whose definition holds the record (`effect+0x28`).
    pub effect: u32,
    /// Its position among the definition's records.
    pub index: u32,
    pub kind: u32,
    pub formula: u32,
    pub value: f32,
    /// `+0x14`: the game mode the record counts in. 0 is any.
    pub mode: u32,
    pub flags: u32,
    /// The dwords at `+0xc`, `+0x10` and `+0x2c`, which no rule here reads.
    pub words: [u32; 3],
    /// `+0x20` and `+0x30`: the client asks the local player for each of them.
    pub traits: [Condition; 2],
    /// `+0x38` and `+0x40`: the client asks the buff table for each of them.
    pub effects: [Condition; 2],
}

/// What a pass saw on its way, for the local log of a pass that ends without a value: the
/// figures its bounds are judged by and the record it refused. Filled as far as the pass got;
/// what it did not reach stays 0.
#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub struct Trace {
    /// Capacity and count of the buff table, as its header gave them.
    pub capacity: u32,
    pub count: u32,
    /// Records in the table the server pushed.
    pub pushed: u32,
    /// Applied buffs walked, and definitions read or compared for them.
    pub buffs: u32,
    pub definitions: u32,
    /// The most records one definition declared, refused or not ([`MAX_MODIFIERS`] is the bound).
    pub most_records: u32,
    pub refused: Option<RefusedRecord>,
}

/// One 72-byte modifier record of a buff definition.
struct Modifier {
    kind: u32,
    formula: u32,
    value: f32,
    mode: u32,
    target: u64,
    requirements: u64,
    flags: u32,
    /// The four conditions `requirements` is the union of: `+0x20`, `+0x30`, `+0x38`, `+0x40`.
    conditions: [u64; 4],
    /// `+0xc`, `+0x10` and `+0x2c`.
    words: [u32; 3],
}
struct Definition {
    /// The bytes the three fields below and the modifier group's address were read from.
    head: [u8; DEFINITION_HEAD],
    flags: u32,
    stacking: u32,
    category: u32,
    modifiers: Vec<Modifier>,
}

/// The game content one whole pass read for the applied buffs.
#[derive(Default)]
struct Content {
    /// Instance address -> effect id and definition address.
    instances: BTreeMap<u64, (u32, u64)>,
    /// Definition address -> what was read there.
    definitions: BTreeMap<u64, Definition>,
}
/// A whole pass that ended in a value: what it read as content and what it read it under.
struct Kept {
    at: Instant,
    base: u64,
    context: u64,
    owner: (u64, u64, u64, u64),
    table_header: [u8; 16],
    buckets: Vec<u8>,
    content: Content,
}
impl Kept {
    /// Whether this pass may verify the kept content instead of reading it: it is younger than
    /// [`MAX_CONTENT_AGE`] and the build, the context, the four owners, the table header and
    /// every byte of the buckets are the ones it was read under.
    fn stands(&self, now: Instant, base: u64, context: u64, route: &Route, buckets: &[u8]) -> bool {
        now.checked_duration_since(self.at)
            .is_some_and(|age| age < MAX_CONTENT_AGE)
            && self.base == base
            && self.context == context
            && self.owner == route.owner
            && self.table_header == route.table_header
            && self.buckets == buckets
    }
}

/// What [`magic_find_cached`] keeps between passes. The caller owns one per reader, keeps it
/// for as long as it reads the same process, and passes it to every pass; it holds no address
/// the reader follows without copying and checking it again.
///
/// It is filled by a whole pass that ends in a value and emptied by any pass that does not.
/// Its size is bounded by the reader's own bounds: at most [`MAX_CAPACITY`] instances and as
/// many definitions of at most [`MAX_MODIFIERS`] records, and one copy of the buckets. All of
/// it was copied by one pass, so [`MAX_BYTES`] bounds it as well: fewer than 3641 records in all.
#[derive(Default)]
pub struct MagicFindCache {
    kept: Option<Kept>,
}
impl MagicFindCache {
    pub fn new() -> Self {
        Self::default()
    }
    /// Forget everything: the next pass reads all the content. Never needed for correctness,
    /// since a pass decides on its own whether what is kept still stands.
    pub fn clear(&mut self) {
        self.kept = None;
    }
    /// Whether the next pass will read all the content whatever it finds.
    pub fn is_empty(&self) -> bool {
        self.kept.is_none()
    }
}

fn read_definition<M: Memory>(
    r: &mut Reader<M>,
    address: u64,
    trace: &mut Trace,
) -> Result<Definition, Uncovered> {
    let head: [u8; DEFINITION_HEAD] = r.read(content(address)?)?;
    if dword(&head, 0x28) == 0 {
        return Err(Uncovered::Bounds);
    }
    let group: [u8; 12] = r.read(content(qword(&head, 0x20))? + 0x10)?;
    let count = dword(&group, 8);
    trace.most_records = trace.most_records.max(count);
    if count > MAX_MODIFIERS {
        return Err(Uncovered::Bounds);
    }
    let mut modifiers = Vec::with_capacity(count as usize);
    if count != 0 {
        let records = table(
            r,
            content(qword(&group, 0))?,
            count as usize * MODIFIER,
            MODIFIER,
        )?;
        for record in records.chunks_exact(MODIFIER) {
            let conditions = [0x20, 0x30, 0x38, 0x40].map(|at| qword(record, at));
            modifiers.push(Modifier {
                kind: dword(record, 0),
                formula: dword(record, 4),
                value: f32::from_bits(dword(record, 8)),
                mode: dword(record, 0x14),
                target: qword(record, 0x18),
                requirements: conditions.iter().fold(0, |all, one| all | one),
                flags: dword(record, 0x28),
                conditions,
                words: [0xc, 0x10, 0x2c].map(|at| dword(record, at)),
            });
        }
    }
    Ok(Definition {
        head,
        flags: dword(&head, 0x4),
        stacking: dword(&head, 0xc),
        category: dword(&head, 0x18),
        modifiers,
    })
}

/// One definition's contribution, or a refusal when the client would need live state. A
/// refusal for a record the reader does not evaluate says which record it was.
fn definition_value(
    definition: &Definition,
    wanted: u32,
) -> Result<f32, (Uncovered, Option<usize>)> {
    let mut total = 0.0f32;
    for (index, modifier) in definition.modifiers.iter().enumerate() {
        if modifier.kind != wanted {
            continue;
        }
        if modifier.target != 0 {
            continue; // the widget passes no target, so the client skips these too
        }
        if modifier.mode != 0
            || modifier.requirements != 0
            || modifier.flags & 0x1e != 0
            || modifier.formula != CONSTANT_FORMULA
        {
            return Err((Uncovered::Unsupported, Some(index)));
        }
        if !modifier.value.is_finite() || modifier.value.abs() > MAX_ABS_PERCENT {
            return Err((Uncovered::Bounds, None));
        }
        total += modifier.value;
        if modifier.flags & 1 != 0 {
            break;
        }
    }
    Ok(total)
}

/// A record `definition_value` refused: the applied effect, its definition and the record.
type Refused = (u32, u64, usize);

/// Sum over applied buffs as the client does: one per effect unless it stacks by intensity.
fn buff_total(
    buffs: &[(u32, u64)],
    definitions: &BTreeMap<u64, Definition>,
    wanted: u32,
) -> Result<f32, (Uncovered, Option<Refused>)> {
    let mut seen = BTreeSet::new();
    let mut total = 0.0f32;
    for (effect, address) in buffs {
        let definition = definitions.get(address).ok_or((Uncovered::Integrity, None))?;
        if definition.stacking != 4 && seen.contains(effect) {
            continue;
        }
        seen.insert(*effect);
        total += definition_value(definition, wanted)
            .map_err(|(reason, index)| (reason, index.map(|index| (*effect, *address, index))))?;
    }
    Ok(total)
}

/// The refused record as the log tells it. With `probe`, what each condition names is asked
/// for the dword where an effect keeps its id: up to four copies of 4 bytes, made only by a
/// pass that has already ended without a value, and a copy that fails only leaves that dword
/// unknown. An effect condition that names an applied effect has its id without any copy.
fn refused_record<M: Memory>(
    r: &mut Reader<M>,
    probe: bool,
    content_read: &Content,
    (effect, definition, index): Refused,
) -> Option<RefusedRecord> {
    let modifier = content_read.definitions.get(&definition)?.modifiers.get(index)?;
    let mut named = |pointer: u64, is_effect: bool| {
        if pointer == 0 {
            return Condition::default();
        }
        let applied = content_read.instances.get(&pointer).map(|(id, _)| *id);
        let word = applied.or_else(|| {
            let at = content(pointer).ok().filter(|_| probe)?;
            r.scalar(at + 0x28, 4).ok().map(|word| word as u32)
        });
        Condition { present: true, word, applied: is_effect.then_some(applied.is_some()) }
    };
    let [trait_a, trait_b, effect_a, effect_b] = modifier.conditions;
    Some(RefusedRecord {
        effect,
        index: index as u32,
        kind: modifier.kind,
        formula: modifier.formula,
        value: modifier.value,
        mode: modifier.mode,
        flags: modifier.flags,
        words: modifier.words,
        traits: [named(trait_a, false), named(trait_b, false)],
        effects: [named(effect_a, true), named(effect_b, true)],
    })
}

fn pushed_total(records: &[u8], wanted: u32) -> f32 {
    records
        .chunks_exact(PUSHED)
        .filter(|record| dword(record, 0) == wanted)
        // Not `sum()`: an empty f32 sum is -0.0, and the client's accumulator starts at +0.0.
        .fold(0.0, |total, record| total + f32::from_bits(dword(record, 4)))
}

/// A little-endian field of a copied block. A block too short for it is a bound, not a panic.
fn u32_at(bytes: &[u8], offset: usize) -> Result<u32, Uncovered> {
    match bytes.get(offset..).and_then(|rest| rest.get(..4)) {
        Some(&[a, b, c, d]) => Ok(u32::from_le_bytes([a, b, c, d])),
        _ => Err(Uncovered::Bounds),
    }
}
fn u64_at(bytes: &[u8], offset: usize) -> Result<u64, Uncovered> {
    match bytes.get(offset..).and_then(|rest| rest.get(..8)) {
        Some(&[a, b, c, d, e, f, g, h]) => Ok(u64::from_le_bytes([a, b, c, d, e, f, g, h])),
        _ => Err(Uncovered::Bounds),
    }
}
fn field<const N: usize>(bytes: &[u8], offset: usize) -> Result<[u8; N], Uncovered> {
    bytes
        .get(offset..)
        .and_then(|rest| rest.get(..N))
        .and_then(|part| part.try_into().ok())
        .ok_or(Uncovered::Bounds)
}
/// [`object`] for a pointer that is already in a copied block: NULL is that owner's absence.
fn owned(value: u64) -> Result<u64, Uncovered> {
    if value == 0 {
        return Err(Uncovered::Root);
    }
    heap(value)
}
/// [`identity`] for a vtable pointer or slot that is already in a copied block.
fn certified(found: u64, expected: u64) -> Result<(), Uncovered> {
    if found != expected {
        return Err(Uncovered::Profile);
    }
    Ok(())
}

/// The owners and every field of theirs that the second look compares.
#[derive(PartialEq)]
struct Route {
    /// Character context, controlled character, local player and the character's buff manager.
    owner: (u64, u64, u64, u64),
    /// `stats+0x18`: the luck record, whose last dword is the account luck level.
    luck: [u8; 16],
    /// `manager+0x20`: capacity, count and buckets of the buff table.
    table_header: [u8; 16],
    /// `manager+0xd8`: the table of records the server pushed.
    pushed_header: [u8; 16],
    /// `manager+0xf0`.
    hidden_mode: u32,
}

/// Context -> controlled character, local player and the character's buff manager, with the
/// identity of each. Returns the route and the character context and character as copied, for
/// the checks only the first look makes.
fn route<M: Memory>(
    r: &mut Reader<M>,
    b: u64,
    l: &Layout,
    context: u64,
) -> Result<(Route, [u8; CHAR_CONTEXT_BLOCK], [u8; CHARACTER_BLOCK]), Uncovered> {
    let char_context = object(r, context + 0x98)?;
    let context_block: [u8; CHAR_CONTEXT_BLOCK] = r.read(char_context)?;
    certified(u64_at(&context_block, 0)?, b + l.char_context_vtable)?;
    let character = owned(u64_at(&context_block, 0x98)?)?;
    let character_block: [u8; CHARACTER_BLOCK] = r.read(character)?;
    certified(u64_at(&character_block, 0)?, b + l.character_vtable)?;
    let player = owned(u64_at(&context_block, 0xa0)?)?;
    identity(r, player, b + l.player_vtable)?;
    let stats: [u8; STATS_BLOCK] = r.read(player + 0x9700)?;
    certified(u64_at(&stats, 0)?, b + l.player_stats_vtable)?;
    let manager = owned(u64_at(&character_block, 0xd0)?)?;
    let manager_block: [u8; MANAGER_BLOCK] = r.read(manager)?;
    certified(u64_at(&manager_block, 0)?, b + l.buff_manager_vtable)?;
    Ok((
        Route {
            owner: (char_context, character, player, manager),
            luck: field(&stats, 0x18)?,
            table_header: field(&manager_block, 0x20)?,
            pushed_header: field(&manager_block, 0xd8)?,
            hidden_mode: u32_at(&manager_block, 0xf0)?,
        },
        context_block,
        character_block,
    ))
}

/// Every slot of [`slots_of`] the layout must select its guarded code. The slots of one vtable
/// are copied together, from the lowest to the highest.
fn slots<M: Memory>(r: &mut Reader<M>, b: u64, l: &Layout) -> Result<(), Uncovered> {
    let table = slots_of(l);
    let mut rest: &[(u64, u64, u64)] = &table;
    while let Some(&(vtable, ..)) = rest.first() {
        let run = rest.iter().take_while(|slot| slot.0 == vtable).count();
        let (group, later) = rest.split_at_checked(run).ok_or(Uncovered::Bounds)?;
        let low = group.iter().map(|slot| slot.1).min().ok_or(Uncovered::Bounds)?;
        let high = group.iter().map(|slot| slot.1).max().ok_or(Uncovered::Bounds)?;
        let mut block = [0u8; SLOT_SPAN];
        let part = block
            .get_mut(..(high - low) as usize + 8)
            .ok_or(Uncovered::Bounds)?;
        r.read_into(b + vtable + low, part)?;
        for &(_, slot, target) in group {
            certified(u64_at(part, (slot - low) as usize)?, b + target)?;
        }
        rest = later;
    }
    Ok(())
}

/// The instance an occupied bucket leads to through its node, or `None` for a free bucket.
fn occupant<M: Memory>(
    r: &mut Reader<M>,
    profile: &MagicFindProfile,
    bucket: &[u8],
) -> Result<Option<u64>, Uncovered> {
    let (key, node, hash) = (u32_at(bucket, 0)?, u64_at(bucket, 8)?, u32_at(bucket, 0x10)?);
    if hash == 0 {
        return Ok(None);
    }
    if hash != profile.key_hash(key) || node == 0 {
        return Err(Uncovered::Integrity);
    }
    let node = heap(node)?;
    let link: [u8; NODE_BLOCK] = r.read(node)?;
    certified(u64_at(&link, 0)?, profile.base + profile.layout.buff_node_vtable)?;
    if u32_at(&link, 0x18)? != key {
        return Err(Uncovered::Integrity);
    }
    Ok(Some(content(u64_at(&link, 0x10)?)?))
}
/// buff_filter: in manager mode 1 the client hides definitions flagged 0x40.
fn shown(hidden_mode: u32, definition: &Definition) -> bool {
    hidden_mode != 1 || definition.flags & 0x40 == 0
}

/// Walk the buckets and read every instance and every definition in use: the applied buffs
/// the widget counts, as effect and definition, and the content they were read from.
fn read_content<M: Memory>(
    r: &mut Reader<M>,
    profile: &MagicFindProfile,
    buckets: &[u8],
    count: u32,
    hidden_mode: u32,
    trace: &mut Trace,
) -> Result<(Vec<(u32, u64)>, Content), Uncovered> {
    let mut read = Content::default();
    let mut buffs = Vec::with_capacity(count.min(MAX_CAPACITY) as usize);
    let mut occupied = 0;
    // What a pass that only compared had counted is not this walk's.
    (trace.buffs, trace.definitions) = (0, 0);
    for bucket in buckets.chunks_exact(BUCKET) {
        let Some(instance) = occupant(r, profile, bucket)? else {
            continue;
        };
        occupied += 1;
        trace.buffs = occupied;
        // The table has at most `MAX_CAPACITY` buckets, so this bound cannot be met; it is
        // here so that what is kept between passes is bounded by its own code.
        if occupied > MAX_CAPACITY {
            return Err(Uncovered::Bounds);
        }
        let applied: [u8; INSTANCE_BLOCK] = r.read(instance + INSTANCE_FROM)?;
        let effect = u32_at(&applied, 0x28 - INSTANCE_FROM as usize)?;
        if u32_at(&applied, 0x58 - INSTANCE_FROM as usize)? != 1 {
            return Err(Uncovered::Integrity);
        }
        let definition = u64_at(&applied, 0x60 - INSTANCE_FROM as usize)?;
        if !read.definitions.contains_key(&definition) {
            let content = read_definition(r, definition, trace)?;
            read.definitions.insert(definition, content);
            trace.definitions = read.definitions.len() as u32;
        }
        read.instances.insert(instance, (effect, definition));
        let known = read.definitions.get(&definition).ok_or(Uncovered::Integrity)?;
        if shown(hidden_mode, known) {
            buffs.push((effect, definition));
        }
    }
    if occupied != count {
        return Err(Uncovered::Integrity);
    }
    Ok((buffs, read))
}

/// Walk the buckets and answer for every instance with what is kept, after comparing the
/// first bytes of each definition in use with the ones kept. `None` when a node leads to an
/// instance that is not kept, or a definition no longer starts as it did: the kept content
/// does not answer for this pass, and the caller reads everything.
fn kept_content<M: Memory>(
    r: &mut Reader<M>,
    profile: &MagicFindProfile,
    buckets: &[u8],
    hidden_mode: u32,
    kept: &Content,
    trace: &mut Trace,
) -> Result<Option<Vec<(u32, u64)>>, Uncovered> {
    let mut compared = BTreeSet::new();
    let mut buffs = Vec::with_capacity(kept.instances.len());
    for bucket in buckets.chunks_exact(BUCKET) {
        let Some(instance) = occupant(r, profile, bucket)? else {
            continue;
        };
        trace.buffs += 1;
        let Some(&(effect, definition)) = kept.instances.get(&instance) else {
            return Ok(None);
        };
        let Some(known) = kept.definitions.get(&definition) else {
            return Ok(None);
        };
        if compared.insert(definition) && r.read::<DEFINITION_HEAD>(definition)? != known.head {
            return Ok(None);
        }
        trace.definitions = compared.len() as u32;
        if shown(hidden_mode, known) {
            buffs.push((effect, definition));
        }
    }
    Ok(Some(buffs))
}

/// Read the three stored addends of the controlled character's Magic Find, then read every
/// owner, vtable, header and table again. `owner` stays local, as in the other readers.
/// Nothing is kept from one call to the next: every pass reads all the content.
pub fn magic_find<M: Memory>(
    r: &mut Reader<M>,
    profile: &MagicFindProfile,
    context: u64,
) -> Result<(MagicFind, (u64, u64)), Uncovered> {
    pass(r, profile, context, None, &mut Trace::default(), false)
}

/// [`magic_find`], reading the buffs' content whole only when `cache` cannot answer for it.
///
/// `cache` is the caller's, one per reader, passed to every pass and never shared; an empty
/// one makes this pass exactly [`magic_find`]. `now` is the caller's monotonic clock: the
/// instant of this pass, never earlier than the previous one's. This function reads no clock.
///
/// The pass reads all the content, as [`magic_find`] does, when the cache is empty, when the
/// last whole pass is [`MAX_CONTENT_AGE`] old or `now` is earlier than it, when the build, the
/// context or any of the four owners is another, when the table header or any byte of the
/// buckets differs from that pass's (a buff applied or removed), when a node leads to an
/// instance that pass did not read, or when the first 48 bytes of a definition in use differ.
/// It then fills the cache if it ends in a value. Any pass that ends without a value empties
/// it, so the next one reads everything.
///
/// Otherwise it copies what [`magic_find`] copies except the instances, the modifier groups
/// and the records, and takes the same second look.
pub fn magic_find_cached<M: Memory>(
    r: &mut Reader<M>,
    profile: &MagicFindProfile,
    context: u64,
    cache: &mut MagicFindCache,
    now: Instant,
) -> Result<(MagicFind, (u64, u64)), Uncovered> {
    let result = pass(r, profile, context, Some((&mut *cache, now)), &mut Trace::default(), false);
    if result.is_err() {
        cache.clear();
    }
    result
}

/// [`magic_find_cached`], leaving in `trace` what the pass saw on its way: the figures its
/// bounds are judged by and, when it ends in [`Uncovered::Unsupported`], the record it refused.
/// The value and the reason are the ones [`magic_find_cached`] gives, and so is every copy of a
/// pass that ends in a value. A pass that refuses a record makes up to four more copies, of 4
/// bytes each, for the dword that tells what each condition of that record names; they come
/// out of the same budget, and one that fails changes nothing but that dword.
pub fn magic_find_traced<M: Memory>(
    r: &mut Reader<M>,
    profile: &MagicFindProfile,
    context: u64,
    cache: &mut MagicFindCache,
    now: Instant,
    trace: &mut Trace,
) -> Result<(MagicFind, (u64, u64)), Uncovered> {
    *trace = Trace::default();
    let result = pass(r, profile, context, Some((&mut *cache, now)), trace, true);
    if result.is_err() {
        cache.clear();
    }
    result
}

/// The Magic Find reader as the addon holds it: one [`MagicFindCache`] for as long as the
/// reader of that process lives, behind a lock because the reader is shared, and the clock
/// read here, so that no caller can pass a pass the wrong instant.
///
/// The cache is emptied by [`CachedMagicFind::discard`], which the addon calls whenever the
/// client sees the game context change, during a cycle (which it then throws away) or between
/// two: nothing kept before a change is used after it. The rest needs no call: a pass reads
/// everything again by itself when the build (the image base the profile was verified at), the
/// context, the character (its owners), the table or any definition differs from what the
/// whole pass kept. The addon holds one of these for the whole load, in a static; the verdict
/// on the executable is final once it is given, so there is no other reader to give it to.
#[derive(Default)]
pub struct CachedMagicFind {
    cache: std::sync::Mutex<MagicFindCache>,
}
impl CachedMagicFind {
    pub const fn new() -> Self {
        Self { cache: std::sync::Mutex::new(MagicFindCache { kept: None }) }
    }
    /// One pass of [`magic_find_cached`], dated now.
    pub fn read<M: Memory>(
        &self,
        r: &mut Reader<M>,
        profile: &MagicFindProfile,
        context: u64,
    ) -> Result<(MagicFind, (u64, u64)), Uncovered> {
        let mut cache = self.cache.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
        magic_find_cached(r, profile, context, &mut cache, Instant::now())
    }
    /// One pass of [`magic_find_traced`], dated now.
    pub fn read_traced<M: Memory>(
        &self,
        r: &mut Reader<M>,
        profile: &MagicFindProfile,
        context: u64,
        trace: &mut Trace,
    ) -> Result<(MagicFind, (u64, u64)), Uncovered> {
        let mut cache = self.cache.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
        magic_find_traced(r, profile, context, &mut cache, Instant::now(), trace)
    }
    /// Forget everything the passes kept: the next one reads all the content.
    pub fn discard(&self) {
        self.cache.lock().unwrap_or_else(|poisoned| poisoned.into_inner()).clear();
    }
}

/// One pass. Without a cache, or with one that cannot answer, it reads all the content.
/// `trace` is filled as the pass goes, whatever it ends in. `probe` lets a pass that refuses a
/// record ask what its conditions name ([`refused_record`]); nothing else depends on it.
fn pass<M: Memory>(
    r: &mut Reader<M>,
    profile: &MagicFindProfile,
    context: u64,
    mut cache: Option<(&mut MagicFindCache, Instant)>,
    trace: &mut Trace,
    probe: bool,
) -> Result<(MagicFind, (u64, u64)), Uncovered> {
    context_checked(context)?;
    let (b, l) = (profile.base, profile.layout);
    let (first, context_block, character_block) = route(r, b, l, context)?;
    let (_, character, player, _) = first.owner;
    slots(r, b, l)?;
    // The widget resolves the player from the character's id; it must be the local one.
    if u32_at(&character_block, 0x178)? & 0x10 == 0 {
        return Err(Uncovered::Root);
    }
    certified(u64_at(&character_block, 8)?, b + l.character_agent_vtable)?;
    certified(u64_at(&character_block, 0x40)?, b + l.combatant_vtable)?;
    let agent = u32_at(&character_block, 0xa0)?;
    if agent & 0xf000_0000 != 0x3000_0000 {
        return Err(Uncovered::Root);
    }
    let cached = u32_at(&character_block, 0x220)?;
    let player_id = if cached != 0 {
        cached
    } else {
        agent - 0x3000_0000
    };
    if player_id == 0
        || player_id > MAX_PLAYER_ID
        || player_id >= u32_at(&context_block, 0x8c)?
    {
        return Err(Uncovered::Bounds);
    }
    let players = owned(u64_at(&context_block, 0x80)?)?;
    if r.scalar(players + 8 * u64::from(player_id), 8)? != player {
        return Err(Uncovered::Profile);
    }

    let level = dword(&first.luck, 12);
    if level > MAX_LUCK_LEVEL {
        return Err(Uncovered::Bounds);
    }
    let (table_header, pushed_header) = (first.table_header, first.pushed_header);
    let hidden_mode = first.hidden_mode;

    // RVA 0x12C3030 returns 0 for an empty table without dereferencing it.
    let pushed_count = dword(&pushed_header, 12);
    trace.pushed = pushed_count;
    if pushed_count > MAX_PUSHED {
        return Err(Uncovered::Bounds);
    }
    let pushed_at = qword(&pushed_header, 0);
    let pushed = if pushed_count == 0 {
        Vec::new()
    } else {
        table(
            r,
            heap(pushed_at)?,
            pushed_count as usize * PUSHED,
            PUSHED,
        )?
    };
    let mut previous = 0;
    for record in pushed.chunks_exact(PUSHED) {
        let value = f32::from_bits(dword(record, 4));
        if dword(record, 0) < previous {
            return Err(Uncovered::Integrity);
        }
        if !value.is_finite() || value.abs() > MAX_ABS_PERCENT {
            return Err(Uncovered::Bounds);
        }
        previous = dword(record, 0);
    }

    // RVA 0x12C33E0 returns no buff when the count is 0, whatever the table holds.
    let (capacity, count) = (dword(&table_header, 0), dword(&table_header, 4));
    let entries = qword(&table_header, 8);
    (trace.capacity, trace.count) = (capacity, count);
    let buckets = if count == 0 {
        Vec::new()
    } else {
        if capacity > MAX_CAPACITY || !capacity.is_power_of_two() || count > capacity {
            return Err(Uncovered::Bounds);
        }
        table(r, heap(entries)?, capacity as usize * BUCKET, BUCKET)?
    };
    // What a whole pass kept answers for this one only while it stands and every node and
    // definition agrees with it. If not, it is dropped before anything else is read.
    let answered = match &cache {
        Some((MagicFindCache { kept: Some(kept) }, now))
            if kept.stands(*now, b, context, &first, &buckets) =>
        {
            kept_content(r, profile, &buckets, hidden_mode, &kept.content, trace)?
        }
        _ => None,
    };
    let (buffs, read) = match answered {
        Some(buffs) => (buffs, None),
        None => {
            if let Some((cache, _)) = cache.as_mut() {
                cache.clear();
            }
            let (buffs, read) = read_content(r, profile, &buckets, count, hidden_mode, trace)?;
            (buffs, Some(read))
        }
    };
    let content_read = match (&read, &cache) {
        (Some(read), _) => read,
        (None, Some((MagicFindCache { kept: Some(kept) }, _))) => &kept.content,
        // Not reachable: the buffs came from one of the two.
        (None, _) => return Err(Uncovered::Integrity),
    };
    let definitions = &content_read.definitions;
    // A refusal names its record, which goes to the trace before the pass ends without a value.
    let mut buffs_of = |wanted: u32| {
        buff_total(&buffs, definitions, wanted).map_err(|(reason, record)| {
            if let Some(record) = record {
                trace.refused = refused_record(r, probe, content_read, record);
            }
            reason
        })
    };

    let mut from_pushed = pushed_total(&pushed, MAGIC_FIND);
    let mut from_buffs = buffs_of(MAGIC_FIND)?;
    // RewardCreatureAllBoon only counts while some applied buff has category 0.
    let boon = buffs.iter().any(|(_, definition)| {
        definitions
            .get(definition)
            .is_some_and(|definition| definition.category == 0)
    });
    if boon {
        from_pushed += pushed_total(&pushed, MAGIC_FIND_BOON);
        from_buffs += buffs_of(MAGIC_FIND_BOON)?;
    }

    // The second look: the route again, with its identities, then the two tables. Of the
    // copied blocks it compares the owners and the fields of `Route`, nothing else.
    if route(r, b, l, context)?.0 != first
        || (!pushed.is_empty() && table(r, pushed_at, pushed.len(), PUSHED)? != pushed)
        || (!buckets.is_empty() && table(r, entries, buckets.len(), BUCKET)? != buckets)
    {
        return Err(Uncovered::Changed);
    }
    let total = from_pushed + from_buffs + level as f32;
    // One addend may be negative. A negative total is not a Magic Find anyone can be shown.
    if total < 0.0 {
        return Err(Uncovered::Bounds);
    }
    // Only a whole pass that ends in a value is kept, dated by the caller's clock.
    if let (Some(content), Some((cache, at))) = (read, cache) {
        cache.kept = Some(Kept {
            at,
            base: b,
            context,
            owner: first.owner,
            table_header,
            buckets,
            content,
        });
    }
    Ok((
        MagicFind {
            total,
            luck: level,
            pushed: from_pushed,
            buffs: from_buffs,
            boon,
        },
        (character, player),
    ))
}
