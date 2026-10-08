//! The Magic Find reader as it was at `37e48df` (0.8.1): every field its own copy.
//!
//! Kept only as the oracle of `tests/magic_find.rs`: the reader that copies in blocks, and the
//! one that keeps content between passes, must give what this one gives. It is the old
//! function and its helpers from `passive`, unchanged but for two things it cannot reach from
//! outside the crate: the image base and the key hash, which come as parameters.
use std::collections::{BTreeMap, BTreeSet};
use tyrian_companion_nexus_core::inventory::{Memory, Reader};
use tyrian_companion_nexus_core::magic_find::*;
use tyrian_companion_nexus_core::passive::{Uncovered, CONTENT_REMAINDER};

const MAX_POINTER: u64 = 0x0000_7fff_ffff_ffff;
const CONSTANT_FORMULA: u32 = 6;
const MAX_PLAYER_ID: u32 = 0xffff;
const MAX_ABS_PERCENT: f32 = 10_000.0;
const BUCKET: usize = 24;
const PUSHED: usize = 12;
const MODIFIER: usize = 72;

fn in_range(value: u64) -> bool {
    (0x10000..=MAX_POINTER).contains(&value)
}
fn heap(value: u64) -> Result<u64, Uncovered> {
    if !in_range(value) {
        return Err(Uncovered::Bounds);
    }
    if value & 7 != 0 {
        return Err(Uncovered::Alignment);
    }
    Ok(value)
}
fn content(value: u64) -> Result<u64, Uncovered> {
    if !in_range(value) {
        return Err(Uncovered::Bounds);
    }
    if value & 7 != CONTENT_REMAINDER {
        return Err(Uncovered::Alignment);
    }
    Ok(value)
}
fn object<M: Memory>(r: &mut Reader<M>, address: u64) -> Result<u64, Uncovered> {
    let value = r.scalar(address, 8)?;
    if value == 0 {
        return Err(Uncovered::Root);
    }
    heap(value)
}
fn identity<M: Memory>(r: &mut Reader<M>, address: u64, expected: u64) -> Result<(), Uncovered> {
    if r.scalar(address, 8)? != expected {
        return Err(Uncovered::Profile);
    }
    Ok(())
}
fn dword(bytes: &[u8], offset: usize) -> u32 {
    u32::from_le_bytes(bytes[offset..offset + 4].try_into().unwrap())
}
fn qword(bytes: &[u8], offset: usize) -> u64 {
    u64::from(dword(bytes, offset)) | u64::from(dword(bytes, offset + 4)) << 32
}
fn table<M: Memory>(
    r: &mut Reader<M>,
    address: u64,
    size: usize,
    record: usize,
) -> Result<Vec<u8>, Uncovered> {
    let chunk = 4096 / record * record;
    let mut bytes = vec![0u8; size];
    for (index, part) in bytes.chunks_mut(chunk).enumerate() {
        r.read_into(address + (index * chunk) as u64, part)?;
    }
    Ok(bytes)
}

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

fn definition_value(definition: &Definition, wanted: u32) -> Result<f32, Uncovered> {
    let mut total = 0.0f32;
    for modifier in &definition.modifiers {
        if modifier.kind != wanted {
            continue;
        }
        if modifier.target != 0 {
            continue;
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
        .fold(0.0, |total, record| total + f32::from_bits(dword(record, 4)))
}

fn owner_route<M: Memory>(
    r: &mut Reader<M>,
    b: u64,
    context: u64,
) -> Result<(u64, u64, u64, u64), Uncovered> {
    let char_context = object(r, context + 0x98)?;
    identity(r, char_context, b + CHAR_CONTEXT_VTABLE)?;
    let character = object(r, char_context + 0x98)?;
    identity(r, character, b + CHARACTER_VTABLE)?;
    let player = object(r, char_context + 0xa0)?;
    identity(r, player, b + PLAYER_VTABLE)?;
    identity(r, player + 0x9700, b + PLAYER_STATS_VTABLE)?;
    let manager = object(r, character + 0xd0)?;
    identity(r, manager, b + BUFF_MANAGER_VTABLE)?;
    Ok((char_context, character, player, manager))
}

pub fn magic_find<M: Memory>(
    r: &mut Reader<M>,
    b: u64,
    key_hash: impl Fn(u32) -> u32,
    context: u64,
) -> Result<(MagicFind, (u64, u64)), Uncovered> {
    if !(0x10000..=MAX_POINTER - 0x198).contains(&context) || context & 7 != 0 {
        return Err(Uncovered::Bounds);
    }
    let owner = owner_route(r, b, context)?;
    let (char_context, character, player, manager) = owner;
    for (vtable, slot, target) in SLOTS {
        identity(r, b + vtable + slot, b + target)?;
    }
    if r.scalar(character + 0x178, 4)? & 0x10 == 0 {
        return Err(Uncovered::Root);
    }
    identity(r, character + 8, b + CHARACTER_AGENT_VTABLE)?;
    identity(r, character + 0x40, b + COMBATANT_VTABLE)?;
    let agent = r.scalar(character + 0xa0, 4)? as u32;
    if agent & 0xf000_0000 != 0x3000_0000 {
        return Err(Uncovered::Root);
    }
    let cached = r.scalar(character + 0x220, 4)? as u32;
    let player_id = if cached != 0 {
        cached
    } else {
        agent - 0x3000_0000
    };
    if player_id == 0
        || player_id > MAX_PLAYER_ID
        || u64::from(player_id) >= r.scalar(char_context + 0x8c, 4)?
    {
        return Err(Uncovered::Bounds);
    }
    let players = object(r, char_context + 0x80)?;
    if r.scalar(players + 8 * u64::from(player_id), 8)? != player {
        return Err(Uncovered::Profile);
    }

    let luck: [u8; 16] = r.read(player + 0x9700 + 0x18)?;
    let level = dword(&luck, 12);
    if level > MAX_LUCK_LEVEL {
        return Err(Uncovered::Bounds);
    }
    let table_header: [u8; 16] = r.read(manager + 0x20)?;
    let pushed_header: [u8; 16] = r.read(manager + 0xd8)?;
    let hidden_mode = r.scalar(manager + 0xf0, 4)?;

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
        if hash != key_hash(key) || node == 0 {
            return Err(Uncovered::Integrity);
        }
        occupied += 1;
        let node = heap(node)?;
        identity(r, node, b + BUFF_NODE_VTABLE)?;
        let link: [u8; 12] = r.read(node + 0x10)?;
        if dword(&link, 8) != key {
            return Err(Uncovered::Integrity);
        }
        let instance = content(qword(&link, 0))?;
        let effect = r.scalar(instance + 0x28, 4)? as u32;
        let reference: [u8; 16] = r.read(instance + 0x58)?;
        if dword(&reference, 0) != 1 {
            return Err(Uncovered::Integrity);
        }
        let definition = qword(&reference, 8);
        if !definitions.contains_key(&definition) {
            definitions.insert(definition, read_definition(r, definition)?);
        }
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
    let boon = buffs
        .iter()
        .any(|(_, definition)| definitions[definition].category == 0);
    if boon {
        from_pushed += pushed_total(&pushed, MAGIC_FIND_BOON);
        from_buffs += buff_total(&buffs, &definitions, MAGIC_FIND_BOON)?;
    }

    if owner_route(r, b, context)? != owner
        || r.read::<16>(player + 0x9700 + 0x18)? != luck
        || r.read::<16>(manager + 0x20)? != table_header
        || r.read::<16>(manager + 0xd8)? != pushed_header
        || r.scalar(manager + 0xf0, 4)? != hidden_mode
        || (!pushed.is_empty() && table(r, pushed_at, pushed.len(), PUSHED)? != pushed)
        || (!buckets.is_empty() && table(r, entries, buckets.len(), BUCKET)? != buckets)
    {
        return Err(Uncovered::Changed);
    }
    let total = from_pushed + from_buffs + level as f32;
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
