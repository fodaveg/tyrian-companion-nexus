//! Passive, bounded Magic Find for the same certified GW2 build.
//!
//! The client stores no total. Its attribute widget (`AtAttribute.cpp`, RVA `0x3E47F8`) sums
//! three stored inputs every time it paints the hero panel:
//!
//! ```text
//! min(cap, account luck level + modifiers(0x71) + [modifiers(0x72) while a boon is applied])
//! ```
//!
//! where `modifiers(type)` (RVA `0x12C0540`) is the sum of the records the server pushed to the
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

use crate::inventory::{BuildProfile, Memory, Reader, TLS_INDEX_RVA};
use crate::passive::{
    content, context_checked, dword, heap, identity, object, qword, table, verify, Guard,
    Uncovered,
};
use crate::wallet::currency_hash;
use std::collections::{BTreeMap, BTreeSet};

pub const CHAR_CONTEXT_VTABLE: u64 = 0x215cf48;
pub const CHARACTER_VTABLE: u64 = 0x215fb60;
pub const CHARACTER_AGENT_VTABLE: u64 = 0x21601d0;
pub const COMBATANT_VTABLE: u64 = 0x2160498;
pub const PLAYER_VTABLE: u64 = 0x215d958;
pub const PLAYER_STATS_VTABLE: u64 = 0x2168138;
pub const BUFF_MANAGER_VTABLE: u64 = 0x2183118;
/// Installed by the table node constructor (RVA `0x12C1890`); 91 of 91 nodes in the live run.
pub const BUFF_NODE_VTABLE: u64 = 0x21830d0;
/// `RewardCreatureAll` and `RewardCreatureAllBoon` in the client's modifier name table.
pub const MAGIC_FIND: u32 = 0x71;
pub const MAGIC_FIND_BOON: u32 = 0x72;
const CONSTANT_FORMULA: u32 = 6;

pub const MAX_CAPACITY: u32 = 512;
pub const MAX_MODIFIERS: u32 = 32;
pub const MAX_PUSHED: u32 = 256;
pub const MAX_LUCK_LEVEL: u32 = 1000;
const MAX_PLAYER_ID: u32 = 0xffff;
const MAX_ABS_PERCENT: f32 = 10_000.0;
/// One whole pass, guards included on the first cycle of a build (6054 bytes). The external
/// probe's live passes of 2026-10-08 asked for 28706 and 32814 bytes, guards included, with 81
/// and 92 buffs. A pass that does not fit is no coverage; the budget is never raised.
pub const MAX_BYTES: usize = 65_536;
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

/// The route getters, the widget's case, the modifier sum and its helpers, and the hash table.
pub const GUARDS: [Guard; 19] = [
    Guard {
        name: "local_player_getter",
        rva: 0x498860,
        size: 8,
        sha256: "8fd42236905c9ccb06782e7a64df231d2be5f1c373f520fb9ff3a1a087796392",
    },
    Guard {
        name: "controlled_character_getter",
        rva: 0x11b4480,
        size: 29,
        sha256: "1f8522d1275fdf3efb6c9b9c7d7ae54d914e2723422263f0073ce6d09a8a830b",
    },
    Guard {
        name: "player_by_id_getter",
        rva: 0x11b6d60,
        size: 25,
        sha256: "7282c8c69835b41f12c3ddad5eff3e4955cc2fbcbb6514295e510c8e16244086",
    },
    Guard {
        name: "player_stats_getter",
        rva: 0x11ba730,
        size: 8,
        sha256: "3e958f563d9a1a4621232a43557373e42652bb1280fbfa2214f0d767abf7f16e",
    },
    Guard {
        name: "account_luck_level_getter",
        rva: 0x402ae0,
        size: 4,
        sha256: "e27121b8816e0aef2ed29676899d26e79f66af7e5ee5aed902be7208122bb635",
    },
    Guard {
        name: "combatant_cast",
        rva: 0x11d6a20,
        size: 14,
        sha256: "39f54bd1c6a5da3b35f2d3060c65179833ad00f2db5cb9f73dd75a67d3e09faf",
    },
    Guard {
        name: "buff_manager_getter",
        rva: 0x400060,
        size: 8,
        sha256: "6b185d33a0706874cb3e05bca6abc44f8ea444d6a7e1e6548f7f0b263040af5b",
    },
    Guard {
        name: "is_player",
        rva: 0x11d9150,
        size: 24,
        sha256: "1a16f6adc080470acfd8f3d49b216607ece022ee3006e2430d7397ebf6fc164e",
    },
    Guard {
        name: "player_id_getter",
        rva: 0x11d7a50,
        size: 54,
        sha256: "a701c55fe12ab50346fda01b72eb72156039b838ab3da4ad0059014fdd46b891",
    },
    Guard {
        name: "modifier_value",
        rva: 0x12c2450,
        size: 129,
        sha256: "02c17502d33b741829ee827be8a95cbaf91e69d27f07171cff624f77662a268a",
    },
    Guard {
        name: "pushed_modifier_sum",
        rva: 0x12c2520,
        size: 212,
        sha256: "84b64e88e0cfcd72545ceb0724c6c3b1214eac9f26e45a0cf04c037ba273ed88",
    },
    Guard {
        name: "buff_iterator",
        rva: 0x12c28d0,
        size: 188,
        sha256: "698db876d7edf85250891673fe1d791224a161b1489d90147497ba7c91abbf18",
    },
    Guard {
        name: "buff_filter",
        rva: 0x12c4320,
        size: 142,
        sha256: "490a1348d0a788393e5af59d0a10a6ffb9ac46b65a6ba3dbea93e607c7a58ffe",
    },
    Guard {
        name: "buff_lookup",
        rva: 0x251e20,
        size: 296,
        sha256: "4da24189ebd19ebf99989e17bcdde5d7804b974ca6761d697fbad6357dc70784",
    },
    Guard {
        name: "modifier_sum",
        rva: 0x12c0540,
        size: 2536,
        sha256: "887f54e8f9981ecacb51c55324f2174d2adb2c91b7238b07f6361a051c906593",
    },
    Guard {
        name: "modifier_formula",
        rva: 0x12c0f30,
        size: 816,
        sha256: "03c40bfe57089a26ee53e0790bdfee39a26a3645e94b93cbc5c6d2df45ffd132",
    },
    Guard {
        name: "magic_find_case",
        rva: 0x3e47f8,
        size: 241,
        sha256: "eb21265687ff6a00dc5f53148d98f6a44b268bcde2c93b6c5d77ac419346e93e",
    },
    Guard {
        name: "attribute_modifier_map",
        rva: 0x3e3e00,
        size: 296,
        sha256: "6bad628623282ed7de1d3f5ca56adbd3ee756b9f790460b43e6adb3b3ac25912",
    },
    Guard {
        name: HASH_TABLE,
        rva: 0x1b8fdb0,
        size: 1024,
        sha256: "7a5e36b78f411ef1d20f94aec5c1e324e5f43b47e1b084ea392d596daa25bed6",
    },
];
/// Vtable, slot and the guarded code that slot must select.
pub const SLOTS: [(u64, u64, u64); 12] = [
    (CHAR_CONTEXT_VTABLE, 0x68, 0x11b4480),
    (CHAR_CONTEXT_VTABLE, 0x70, 0x498860),
    (CHAR_CONTEXT_VTABLE, 0x118, 0x11b6d60),
    (CHARACTER_VTABLE, 0x100, 0x11d6a20),
    (COMBATANT_VTABLE, 0x38, 0x400060),
    (CHARACTER_AGENT_VTABLE, 0x68, 0x11d9150),
    (CHARACTER_AGENT_VTABLE, 0x18, 0x11d7a50),
    (PLAYER_VTABLE, 0x320, 0x11ba730),
    (PLAYER_STATS_VTABLE, 0x30, 0x402ae0),
    (BUFF_MANAGER_VTABLE, 0x18, 0x12c2450),
    (BUFF_MANAGER_VTABLE, 0x20, 0x12c2520),
    (BUFF_MANAGER_VTABLE, 0x28, 0x12c28d0),
];
// `BuildProfile::checked` only accepts an image that reaches the TLS index, which lies beyond
// every address this module reads inside the executable.
const _: () = assert!(BUFF_MANAGER_VTABLE + 0x30 <= TLS_INDEX_RVA);
const _: () = assert!(PLAYER_VTABLE + 0x328 <= TLS_INDEX_RVA);

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
    table: [u32; 256],
}
impl MagicFindProfile {
    pub fn verified<M: Memory>(
        r: &mut Reader<M>,
        profile: BuildProfile,
    ) -> Result<Self, Uncovered> {
        Self::build(r, profile, &GUARDS)
    }
    /// For this crate's fixtures only; the addon calls [`Self::verified`] and nothing else.
    /// Fixtures carry no bytes of the game, so they stand for the executable with guard
    /// contents and digests of their own. The ranges must still be exactly the audited ones:
    /// an empty, shorter or shifted list is refused before anything is read.
    #[doc(hidden)]
    pub fn verified_against<M: Memory>(
        r: &mut Reader<M>,
        profile: BuildProfile,
        guards: &[Guard],
    ) -> Result<Self, Uncovered> {
        if !crate::passive::same_ranges(guards, &GUARDS) {
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
            table: words.ok_or(Uncovered::Guard)?,
        })
    }
    /// The engine's key hash over this build's table, for fixtures that fill a buff table.
    pub fn key_hash(&self, key: u32) -> u32 {
        currency_hash(key, &self.table)
    }
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
}
struct Definition {
    flags: u32,
    stacking: u32,
    category: u32,
    modifiers: Vec<Modifier>,
}

fn read_definition<M: Memory>(r: &mut Reader<M>, address: u64) -> Result<Definition, Uncovered> {
    let head: [u8; 0x30] = r.read(content(address)?)?;
    if dword(&head, 0x28) == 0 {
        return Err(Uncovered::Bounds);
    }
    let group: [u8; 12] = r.read(content(qword(&head, 0x20))? + 0x10)?;
    let count = dword(&group, 8);
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
            modifiers.push(Modifier {
                kind: dword(record, 0),
                formula: dword(record, 4),
                value: f32::from_bits(dword(record, 8)),
                mode: dword(record, 0x14),
                target: qword(record, 0x18),
                requirements: qword(record, 0x20)
                    | qword(record, 0x30)
                    | qword(record, 0x38)
                    | qword(record, 0x40),
                flags: dword(record, 0x28),
            });
        }
    }
    Ok(Definition {
        flags: dword(&head, 0x4),
        stacking: dword(&head, 0xc),
        category: dword(&head, 0x18),
        modifiers,
    })
}

/// One definition's contribution, or a refusal when the client would need live state.
fn definition_value(definition: &Definition, wanted: u32) -> Result<f32, Uncovered> {
    let mut total = 0.0f32;
    for modifier in &definition.modifiers {
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
            return Err(Uncovered::Unsupported);
        }
        if !modifier.value.is_finite() || modifier.value.abs() > MAX_ABS_PERCENT {
            return Err(Uncovered::Bounds);
        }
        total += modifier.value;
        if modifier.flags & 1 != 0 {
            break;
        }
    }
    Ok(total)
}

/// Sum over applied buffs as the client does: one per effect unless it stacks by intensity.
fn buff_total(
    buffs: &[(u32, u64)],
    definitions: &BTreeMap<u64, Definition>,
    wanted: u32,
) -> Result<f32, Uncovered> {
    let mut seen = BTreeSet::new();
    let mut total = 0.0f32;
    for (effect, definition) in buffs {
        let definition = &definitions[definition];
        if definition.stacking != 4 && seen.contains(effect) {
            continue;
        }
        seen.insert(*effect);
        total += definition_value(definition, wanted)?;
    }
    Ok(total)
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
    context: u64,
) -> Result<(Route, [u8; CHAR_CONTEXT_BLOCK], [u8; CHARACTER_BLOCK]), Uncovered> {
    let char_context = object(r, context + 0x98)?;
    let context_block: [u8; CHAR_CONTEXT_BLOCK] = r.read(char_context)?;
    certified(u64_at(&context_block, 0)?, b + CHAR_CONTEXT_VTABLE)?;
    let character = owned(u64_at(&context_block, 0x98)?)?;
    let character_block: [u8; CHARACTER_BLOCK] = r.read(character)?;
    certified(u64_at(&character_block, 0)?, b + CHARACTER_VTABLE)?;
    let player = owned(u64_at(&context_block, 0xa0)?)?;
    identity(r, player, b + PLAYER_VTABLE)?;
    let stats: [u8; STATS_BLOCK] = r.read(player + 0x9700)?;
    certified(u64_at(&stats, 0)?, b + PLAYER_STATS_VTABLE)?;
    let manager = owned(u64_at(&character_block, 0xd0)?)?;
    let manager_block: [u8; MANAGER_BLOCK] = r.read(manager)?;
    certified(u64_at(&manager_block, 0)?, b + BUFF_MANAGER_VTABLE)?;
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

/// Every slot of [`SLOTS`] must select its guarded code. The slots of one vtable are copied
/// together, from the lowest to the highest.
fn slots<M: Memory>(r: &mut Reader<M>, b: u64) -> Result<(), Uncovered> {
    let mut rest: &[(u64, u64, u64)] = &SLOTS;
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

/// Read the three stored addends of the controlled character's Magic Find, then read every
/// owner, vtable, header and table again. `owner` stays local, as in the other readers.
pub fn magic_find<M: Memory>(
    r: &mut Reader<M>,
    profile: &MagicFindProfile,
    context: u64,
) -> Result<(MagicFind, (u64, u64)), Uncovered> {
    context_checked(context)?;
    let b = profile.base;
    let (first, context_block, character_block) = route(r, b, context)?;
    let (_, character, player, _) = first.owner;
    slots(r, b)?;
    // The widget resolves the player from the character's id; it must be the local one.
    if u32_at(&character_block, 0x178)? & 0x10 == 0 {
        return Err(Uncovered::Root);
    }
    certified(u64_at(&character_block, 8)?, b + CHARACTER_AGENT_VTABLE)?;
    certified(u64_at(&character_block, 0x40)?, b + COMBATANT_VTABLE)?;
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

    // RVA 0x12C2520 returns 0 for an empty table without dereferencing it.
    let pushed_count = dword(&pushed_header, 12);
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

    // RVA 0x12C28D0 returns no buff when the count is 0, whatever the table holds.
    let (capacity, count) = (dword(&table_header, 0), dword(&table_header, 4));
    let entries = qword(&table_header, 8);
    let buckets = if count == 0 {
        Vec::new()
    } else {
        if capacity > MAX_CAPACITY || !capacity.is_power_of_two() || count > capacity {
            return Err(Uncovered::Bounds);
        }
        table(r, heap(entries)?, capacity as usize * BUCKET, BUCKET)?
    };
    let mut definitions = BTreeMap::new();
    let mut buffs = Vec::with_capacity(count as usize);
    let mut occupied = 0;
    for bucket in buckets.chunks_exact(BUCKET) {
        let (key, node, hash) = (dword(bucket, 0), qword(bucket, 8), dword(bucket, 0x10));
        if hash == 0 {
            continue;
        }
        if hash != profile.key_hash(key) || node == 0 {
            return Err(Uncovered::Integrity);
        }
        occupied += 1;
        let node = heap(node)?;
        let link: [u8; NODE_BLOCK] = r.read(node)?;
        certified(u64_at(&link, 0)?, b + BUFF_NODE_VTABLE)?;
        if u32_at(&link, 0x18)? != key {
            return Err(Uncovered::Integrity);
        }
        let instance = content(u64_at(&link, 0x10)?)?;
        let applied: [u8; INSTANCE_BLOCK] = r.read(instance + INSTANCE_FROM)?;
        let effect = u32_at(&applied, 0x28 - INSTANCE_FROM as usize)?;
        if u32_at(&applied, 0x58 - INSTANCE_FROM as usize)? != 1 {
            return Err(Uncovered::Integrity);
        }
        let definition = u64_at(&applied, 0x60 - INSTANCE_FROM as usize)?;
        if !definitions.contains_key(&definition) {
            definitions.insert(definition, read_definition(r, definition)?);
        }
        // buff_filter: in this manager mode the client hides definitions flagged 0x40.
        if hidden_mode == 1 && definitions[&definition].flags & 0x40 != 0 {
            continue;
        }
        buffs.push((effect, definition));
    }
    if occupied != count {
        return Err(Uncovered::Integrity);
    }

    let mut from_pushed = pushed_total(&pushed, MAGIC_FIND);
    let mut from_buffs = buff_total(&buffs, &definitions, MAGIC_FIND)?;
    // RewardCreatureAllBoon only counts while some applied buff has category 0.
    let boon = buffs
        .iter()
        .any(|(_, definition)| definitions[definition].category == 0);
    if boon {
        from_pushed += pushed_total(&pushed, MAGIC_FIND_BOON);
        from_buffs += buff_total(&buffs, &definitions, MAGIC_FIND_BOON)?;
    }

    // The second look: the route again, with its identities, then the two tables. Of the
    // copied blocks it compares the owners and the fields of `Route`, nothing else.
    if route(r, b, context)?.0 != first
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
