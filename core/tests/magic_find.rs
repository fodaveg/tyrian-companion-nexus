//! Magic Find fixtures over owned bytes; never open a real process.
//!
//! The fixtures carry no bytes of the game. They own synthetic contents for every guarded
//! range, the hash table included, and pass their digests to
//! `MagicFindProfile::verified_against`, which accepts other digests only for exactly the
//! audited ranges and which the addon never calls.
//!
//! What ties production to the audit is `fixtures/magic_find_profile.json`: a verbatim copy
//! (sha256 `8992928d…6cebbf`) of `tyrian-companion/docs/audit/loot-mf-probe/profile.json` at
//! the commit "accept game content pointers at their observed alignment in the Magic Find
//! probe". It holds digests only, and that repository's `check_profile_offline.py` compares
//! each of its 19 digests and 12 slots with the installed executable.
//! `production_constants_are_the_audited_profile` requires `GUARDS`, `SLOTS`, the vtables and
//! the modifier types to equal it, so a mistyped digest or RVA here turns the suite red.
//!
//! Heap objects are placed on 8 bytes and game content 4 past, as the live runs of 2026-10-08
//! found them. `live_shapes_of_8_october` are the two states the external probe matched with
//! the hero panel that day: 333.0 and, after a buff change, 363.0.
//!
//! `support/magic_find_field_by_field.rs` is the reader of 0.8.1 (`37e48df`), which copied
//! every field on its own. It is the oracle here: what the reader gives, a value or a reason,
//! is compared with what that one gives over the same bytes.
use std::collections::{BTreeMap, BTreeSet};
use tyrian_companion_nexus_core::inventory::{
    BuildProfile, Memory, ReadError, Reader, BUILD_SHA256,
};
use tyrian_companion_nexus_core::magic_find::*;
use tyrian_companion_nexus_core::passive::{Guard, Uncovered};
use tyrian_companion_nexus_core::sha256::{hex, sha256};
use tyrian_companion_nexus_core::wallet::currency_hash;

#[path = "support/magic_find_field_by_field.rs"]
mod field_by_field;

const BASE: u64 = 0x140000000;
const IMAGE: u64 = 0x2c48000;
const CTX: u64 = 0x200000;
const CHAR_CONTEXT: u64 = 0x210000;
const CHARACTER: u64 = 0x220000;
const PLAYERS: u64 = 0x230000;
const MANAGER: u64 = 0x240000;
const BUCKETS: u64 = 0x250000;
const PUSHED: u64 = 0x260000;
const HEAP: u64 = 0x270000;
const PLAYER: u64 = 0x300000;
const STATS: u64 = PLAYER + 0x9700;
const PLAYER_ID: u64 = 7;
const GUARD_BYTES: usize = 6054;

fn synthetic_bytes(guard: &Guard) -> Vec<u8> {
    let mut state = guard.rva ^ 0x9e37_79b9_7f4a_7c15;
    (0..guard.size)
        .map(|_| {
            state = state.wrapping_mul(6364136223846793005).wrapping_add(1442695040888963407);
            (state >> 33) as u8
        })
        .collect()
}
fn synthetic_guards() -> Vec<Guard> {
    GUARDS
        .iter()
        .map(|guard| Guard {
            sha256: Box::leak(hex(&sha256(&synthetic_bytes(guard))).into_boxed_str()),
            ..*guard
        })
        .collect()
}
fn hash(key: u32) -> u32 {
    let bytes = synthetic_bytes(&GUARDS[18]);
    let mut table = [0u32; 256];
    for (word, raw) in table.iter_mut().zip(bytes.chunks_exact(4)) {
        *word = u32::from_le_bytes(raw.try_into().unwrap());
    }
    currency_hash(key, &table)
}

/// One 72-byte modifier record.
#[derive(Clone, Copy)]
struct Record {
    kind: u32,
    value: f32,
    formula: u32,
    mode: u32,
    target: u64,
    /// The four requirement and condition pointers, at 0x20, 0x30, 0x38 and 0x40.
    need: u64,
    need_b: u64,
    when_a: u64,
    when_b: u64,
    flags: u32,
}
fn record(kind: u32, value: f32) -> Record {
    Record {
        kind,
        value,
        formula: 6,
        mode: 0,
        target: 0,
        need: 0,
        need_b: 0,
        when_a: 0,
        when_b: 0,
        flags: 0,
    }
}

#[derive(Default, Clone)]
struct Fixture {
    bytes: BTreeMap<u64, u8>,
    heap: u64,
    remainder: u64,
    capacity: u32,
    count: u32,
    used: BTreeSet<u32>,
    pushed: Vec<(u32, f32)>,
    /// A copy that covers this address fails, wherever the copy starts.
    fail: Option<u64>,
    /// On the given visit of a read starting at `.0`, first write each `(address, value, size)`.
    race: Option<(u64, usize, Vec<(u64, u64, usize)>)>,
    visits: usize,
}
impl Fixture {
    fn put(&mut self, address: u64, value: u64, size: usize) {
        for (i, byte) in value.to_le_bytes()[..size].iter().enumerate() {
            self.bytes.insert(address + i as u64, *byte);
        }
    }
    fn get(&self, address: u64) -> u64 {
        (0..8).fold(0, |value, i| {
            value | (*self.bytes.get(&(address + i)).unwrap_or(&0) as u64) << (8 * i)
        })
    }
    fn alloc(&mut self, size: u64, content: bool) -> u64 {
        let address = self.heap + if content { self.remainder } else { 0 };
        self.heap += (size + 31) & !15;
        address
    }
    fn character(&mut self, address: u64) {
        self.put(address, BASE + CHARACTER_VTABLE, 8);
        self.put(address + 8, BASE + CHARACTER_AGENT_VTABLE, 8);
        self.put(address + 0x40, BASE + COMBATANT_VTABLE, 8);
        self.put(address + 0x178, 0x10, 4);
        self.put(address + 0xa0, 0x3000_0000 + PLAYER_ID, 4);
        self.put(address + 0x220, 0, 4);
        self.put(address + 0xd0, MANAGER, 8);
    }
    fn luck(&mut self, level: u32) {
        self.put(STATS + 0x18, 123_456, 4);
        self.put(STATS + 0x24, level as u64, 4);
    }
    fn headers(&mut self) {
        self.put(MANAGER + 0x20, self.capacity as u64, 4);
        self.put(MANAGER + 0x24, self.count as u64, 4);
        self.put(MANAGER + 0x28, BUCKETS, 8);
        self.put(MANAGER + 0xd8, PUSHED, 8);
        self.put(MANAGER + 0xe0, 64, 4);
        self.put(MANAGER + 0xe4, self.pushed.len() as u64, 4);
    }
    fn definition(&mut self, records: &[Record], stacking: u32, category: u32, flags: u32) -> u64 {
        let list = self.alloc((records.len() * 72).max(16) as u64, true);
        for (index, record) in records.iter().enumerate() {
            let at = list + 72 * index as u64;
            self.put(at, record.kind as u64, 4);
            self.put(at + 4, record.formula as u64, 4);
            self.put(at + 8, record.value.to_bits() as u64, 4);
            self.put(at + 0x14, record.mode as u64, 4);
            self.put(at + 0x18, record.target, 8);
            self.put(at + 0x20, record.need, 8);
            self.put(at + 0x28, record.flags as u64, 4);
            self.put(at + 0x30, record.need_b, 8);
            self.put(at + 0x38, record.when_a, 8);
            self.put(at + 0x40, record.when_b, 8);
        }
        let group = self.alloc(0x20, true);
        self.put(group + 0x10, if records.is_empty() { 0 } else { list }, 8);
        self.put(group + 0x18, records.len() as u64, 4);
        let definition = self.alloc(0x30, true);
        self.put(definition + 0x4, flags as u64, 4);
        self.put(definition + 0xc, stacking as u64, 4);
        self.put(definition + 0x18, category as u64, 4);
        self.put(definition + 0x20, group, 8);
        self.put(definition + 0x28, 1, 4);
        definition
    }
    fn plain(&mut self, records: &[Record]) -> u64 {
        self.definition(records, 0, 1, 0)
    }
    /// One applied buff: bucket address, node and content reference.
    fn buff(&mut self, key: u32, effect: u32, definition: u64) -> (u64, u64, u64) {
        let node = self.alloc(0x78, false);
        let instance = self.alloc(0x70, true);
        self.put(node, BASE + BUFF_NODE_VTABLE, 8);
        self.put(node + 0x10, instance, 8);
        self.put(node + 0x18, key as u64, 4);
        self.put(instance + 0x28, effect as u64, 4);
        self.put(instance + 0x58, 1, 4);
        self.put(instance + 0x60, definition, 8);
        let mask = self.capacity - 1;
        let mut index = hash(key) & mask;
        while self.used.contains(&index) {
            index = (index + 1) & mask;
        }
        self.used.insert(index);
        self.count += 1;
        let bucket = BUCKETS + 24 * index as u64;
        self.put(bucket, key as u64, 4);
        self.put(bucket + 8, node, 8);
        self.put(bucket + 0x10, hash(key) as u64, 4);
        self.headers();
        (bucket, node, instance)
    }
    fn push(&mut self, kind: u32, value: f32) {
        self.pushed.push((kind, value));
        self.pushed.sort_by_key(|entry| entry.0);
        for (index, (kind, value)) in self.pushed.clone().into_iter().enumerate() {
            self.put(PUSHED + 12 * index as u64, kind as u64, 4);
            self.put(PUSHED + 12 * index as u64 + 4, value.to_bits() as u64, 4);
            self.put(PUSHED + 12 * index as u64 + 8, 1, 4);
        }
        self.headers();
    }
}
impl Memory for Fixture {
    fn read_exact(&mut self, address: u64, out: &mut [u8]) -> Result<(), ReadError> {
        if self.fail.is_some_and(|at| (address..address + out.len() as u64).contains(&at)) {
            return Err(ReadError::ReadFailed);
        }
        if let Some((trigger, visit, writes)) = self.race.clone() {
            if address == trigger {
                self.visits += 1;
                if self.visits == visit {
                    for (target, value, size) in writes {
                        self.put(target, value, size);
                    }
                }
            }
        }
        // Bytes no fixture wrote read as zero.
        out.fill(0);
        for (at, byte) in self.bytes.range(address..address + out.len() as u64) {
            out[(at - address) as usize] = *byte;
        }
        Ok(())
    }
}
/// For the tests that read the same memory more than once.
impl Memory for &mut Fixture {
    fn read_exact(&mut self, address: u64, out: &mut [u8]) -> Result<(), ReadError> {
        (**self).read_exact(address, out)
    }
}

/// A certified image and a whole route with no buff and nothing pushed.
fn empty_at(remainder: u64, capacity: u32, luck: u32) -> Fixture {
    let mut m = Fixture {
        heap: HEAP,
        remainder,
        capacity,
        ..Default::default()
    };
    for guard in &GUARDS {
        for (i, byte) in synthetic_bytes(guard).iter().enumerate() {
            m.bytes.insert(BASE + guard.rva + i as u64, *byte);
        }
    }
    for (vtable, slot, target) in SLOTS {
        m.put(BASE + vtable + slot, BASE + target, 8);
    }
    for (address, value) in [
        (CTX + 0x98, CHAR_CONTEXT),
        (CHAR_CONTEXT, BASE + CHAR_CONTEXT_VTABLE),
        (CHAR_CONTEXT + 0x98, CHARACTER),
        (CHAR_CONTEXT + 0xa0, PLAYER),
        (CHAR_CONTEXT + 0x80, PLAYERS),
        (PLAYERS + 8 * PLAYER_ID, PLAYER),
        (PLAYER, BASE + PLAYER_VTABLE),
        (STATS, BASE + PLAYER_STATS_VTABLE),
        (MANAGER, BASE + BUFF_MANAGER_VTABLE),
    ] {
        m.put(address, value, 8);
    }
    m.put(CHAR_CONTEXT + 0x8c, 16, 4);
    m.character(CHARACTER);
    m.luck(luck);
    m.put(MANAGER + 0xf0, 4, 4);
    m.headers();
    m
}
fn empty() -> Fixture {
    empty_at(4, 64, 300)
}
/// 300 from luck, 7 pushed by the server and one 30 food buff: 337 on the hero panel.
fn standard_at(remainder: u64) -> (Fixture, (u64, u64, u64)) {
    let mut m = empty_at(remainder, 64, 300);
    m.push(MAGIC_FIND, 7.0);
    let food = m.plain(&[record(MAGIC_FIND, 30.0)]);
    let buff = m.buff(1001, 501, food);
    (m, buff)
}
fn standard() -> (Fixture, (u64, u64, u64)) {
    standard_at(4)
}
fn profile() -> BuildProfile {
    BuildProfile::checked(BUILD_SHA256, BASE, IMAGE).unwrap()
}
/// Guards and one whole pass through the same budgeted reader, as the addon's first cycle does.
fn run(m: Fixture) -> (Result<MagicFind, Uncovered>, usize) {
    let mut reader = Reader::bounded(m, MAX_BYTES);
    let result = MagicFindProfile::verified_against(&mut reader, profile(), &synthetic_guards())
        .and_then(|verified| magic_find(&mut reader, &verified, CTX))
        .map(|(value, _owner)| value);
    (result, reader.bytes)
}
fn read(m: Fixture) -> Result<MagicFind, Uncovered> {
    run(m).0
}
fn total(m: Fixture) -> f32 {
    read(m).unwrap().total
}
/// The standard fixture with one mutation, which may use the food buff's addresses.
fn changed(mutate: impl Fn(&mut Fixture, (u64, u64, u64))) -> Result<MagicFind, Uncovered> {
    let (mut m, buff) = standard();
    mutate(&mut m, buff);
    read(m)
}
/// The standard fixture plus one more buff whose only record is `extra`.
fn with_record(extra: Record) -> Result<MagicFind, Uncovered> {
    let (mut m, _) = standard();
    let definition = m.plain(&[extra]);
    m.buff(1002, 502, definition);
    read(m)
}

/// 91 other buffs in 256 buckets, 13 pushed in 4 records and one stacking-4 record of `buffs`.
fn live_shape(buffs: f32) -> Fixture {
    let mut m = empty_at(4, 256, 300);
    for (kind, value) in [(14, 5.0), (MAGIC_FIND, 6.0), (MAGIC_FIND, 7.0), (126, 1.0)] {
        m.push(kind, value);
    }
    let shared = m.plain(&[record(14, 3.0)]);
    for key in 0..90 {
        let definition = if key % 2 == 1 {
            shared
        } else {
            m.plain(&[record(5, 1.0)])
        };
        m.buff(2000 + key, 600 + key, definition);
    }
    let boost = m.definition(&[record(MAGIC_FIND, buffs)], 4, 7, 0);
    m.buff(1001, 501, boost);
    m
}

#[test]
fn live_shapes_of_8_october_give_333_and_363() {
    for (buffs, expected) in [(20.0, 333.0), (50.0, 363.0)] {
        let (result, bytes) = run(live_shape(buffs));
        assert_eq!(
            result,
            Ok(MagicFind {
                total: expected,
                luck: 300,
                pushed: 13.0,
                buffs,
                boon: false
            })
        );
        assert!(bytes <= MAX_BYTES, "{bytes}");
    }
}

/// The guards of the fixtures, verified once; a pass needs nothing else from the first cycle.
fn verified() -> MagicFindProfile {
    let mut reader = Reader::bounded(empty(), MAX_BYTES);
    MagicFindProfile::verified_against(&mut reader, profile(), &synthetic_guards()).unwrap()
}
type Outcome = Result<(MagicFind, (u64, u64)), Uncovered>;
/// One pass of the reader over `m`: what it gives, how many copies it asked for and of how
/// many bytes.
fn pass(m: &mut Fixture, verified: &MagicFindProfile) -> (Outcome, usize, usize) {
    let mut reader = Reader::bounded(m, MAX_BYTES);
    let outcome = magic_find(&mut reader, verified, CTX);
    (outcome, reader.reads, reader.bytes)
}
/// The same pass by the reader of 0.8.1, which copied every field on its own.
fn pass_field_by_field(m: &mut Fixture, verified: &MagicFindProfile) -> (Outcome, usize, usize) {
    let mut reader = Reader::bounded(m, MAX_BYTES);
    let outcome = field_by_field::magic_find(&mut reader, BASE, |key| verified.key_hash(key), CTX);
    (outcome, reader.reads, reader.bytes)
}

/// The fixtures' own generator: the same sequence on every run.
struct Dice(u64);
impl Dice {
    fn roll(&mut self, sides: u64) -> u64 {
        self.0 = self.0.wrapping_mul(6364136223846793005).wrapping_add(1442695040888963407);
        (self.0 >> 33) % sides
    }
}
/// Every address a fixture fills outside the guarded ranges: the route, the vtable slots, the
/// tables, the nodes and the content.
fn fields(m: &Fixture) -> Vec<u64> {
    let guarded = |address: u64| {
        GUARDS
            .iter()
            .any(|guard| (BASE + guard.rva..BASE + guard.rva + guard.size as u64).contains(&address))
    };
    m.bytes.keys().copied().filter(|address| !guarded(*address)).collect()
}
/// Fixtures that between them use every branch of the sum.
fn shapes() -> Vec<Fixture> {
    let mut boon = standard().0;
    let boon_only = boon.plain(&[record(MAGIC_FIND_BOON, 40.0)]);
    boon.buff(1002, 502, boon_only);
    boon.push(MAGIC_FIND_BOON, 2.0);
    let category_zero = boon.definition(&[record(14, 5.0)], 0, 0, 0);
    boon.buff(1003, 503, category_zero);
    let mut stacks = empty();
    let duration = stacks.definition(&[record(MAGIC_FIND, 10.0)], 1, 1, 0);
    let intensity = stacks.definition(&[record(MAGIC_FIND, 1.0)], 4, 1, 0);
    for key in [1, 2, 3] {
        stacks.buff(key, 700, duration);
    }
    for key in [11, 12, 13, 14] {
        stacks.buff(key, 800, intensity);
    }
    let mut hidden = standard().0;
    let flagged = hidden.definition(&[record(MAGIC_FIND, 50.0)], 0, 1, 0x40);
    hidden.buff(1002, 502, flagged);
    hidden.put(MANAGER + 0xf0, 1, 4);
    let mut nothing_pushed = empty();
    nothing_pushed.put(MANAGER + 0xd8, 0, 8);
    vec![
        standard().0,
        live_shape(20.0),
        live_shape(50.0),
        empty(),
        empty_at(4, 64, 0),
        nothing_pushed,
        boon,
        stacks,
        hidden,
    ]
}

#[test]
fn a_pass_over_the_live_shape_asks_for_349_copies_where_it_asked_for_557() {
    let verified = verified();
    let (outcome, reads, bytes) = pass_field_by_field(&mut live_shape(20.0), &verified);
    assert_eq!(outcome.map(|(value, _)| value.total), Ok(333.0));
    assert_eq!((reads, bytes), (557, 22_620));
    let (outcome, reads, bytes) = pass(&mut live_shape(20.0), &verified);
    assert_eq!(outcome.map(|(value, _)| value.total), Ok(333.0));
    assert_eq!((reads, bytes), (349, 29_328));
    // The two looks at the route take 6 copies each and the twelve slots take 7.
    let (_, reads, bytes) = pass(&mut empty(), &verified);
    assert_eq!((reads, bytes), (6 + 7 + 1 + 6, 2 * 1016 + 328 + 8));
}

#[test]
fn blocks_give_what_field_by_field_copies_gave_on_every_fixture() {
    let verified = verified();
    for (index, mut shape) in shapes().into_iter().enumerate() {
        let expected = pass_field_by_field(&mut shape.clone(), &verified).0;
        assert!(expected.is_ok(), "{index}");
        assert_eq!(pass(&mut shape, &verified).0, expected, "{index}");
    }
    for remainder in [0, 1, 2, 6] {
        let mut m = standard_at(remainder).0;
        let expected = pass_field_by_field(&mut m.clone(), &verified).0;
        assert_eq!(expected.clone().map(|_| ()), Err(Uncovered::Alignment));
        assert_eq!(pass(&mut m, &verified).0, expected);
    }
}

#[test]
fn blocks_reject_what_field_by_field_copies_rejected_and_for_the_same_reason() {
    let verified = verified();
    let shapes: Vec<(Fixture, Vec<u64>)> = shapes()
        .into_iter()
        .map(|shape| {
            let fields = fields(&shape);
            (shape, fields)
        })
        .collect();
    let mut dice = Dice(0x7e48_df00_2026_1008);
    let mut reasons = BTreeMap::new();
    for round in 0..3000 {
        let (shape, fields) = &shapes[dice.roll(shapes.len() as u64) as usize];
        let mut m = shape.clone();
        // One or two fields damaged: zeroed, replaced, off by one bit, moved by 4 or 8 bytes,
        // or holding what another field holds.
        for _ in 0..1 + dice.roll(2) {
            let at = fields[dice.roll(fields.len() as u64) as usize] & !3;
            match dice.roll(6) {
                0 => m.put(at, 0, 4),
                1 => m.put(at, dice.roll(1 << 31), 4),
                2 => m.put(at, m.get(at) ^ 1 << dice.roll(32), 4),
                3 => m.put(at & !7, m.get(at & !7).wrapping_add(4), 8),
                4 => m.put(at & !7, m.get(at & !7).wrapping_add(8), 8),
                _ => {
                    let other = fields[dice.roll(fields.len() as u64) as usize] & !7;
                    m.put(at & !7, m.get(other), 8);
                }
            }
        }
        let expected = pass_field_by_field(&mut m, &verified).0;
        assert_eq!(pass(&mut m, &verified).0, expected, "round {round}");
        let reason = match expected {
            Ok(_) => "value".to_string(),
            Err(reason) => format!("{reason:?}"),
        };
        *reasons.entry(reason).or_insert(0) += 1;
    }
    // The damage reached every closed reason a static memory can give, and left values too.
    for reason in ["value", "Profile", "Root", "Bounds", "Alignment", "Integrity", "Unsupported"] {
        assert!(reasons.get(reason).is_some_and(|count| *count >= 20), "{reason}: {reasons:?}");
    }
    // A copy that fails anywhere a fixture has a field is no value in both. The live shapes
    // add no kind of field to the others, only more of each.
    let mut failed = 0;
    for (mut m, fields) in shapes {
        if m.count > 8 {
            continue;
        }
        for at in fields.into_iter().filter(|at| at & 3 == 0) {
            m.fail = Some(at);
            let expected = pass_field_by_field(&mut m, &verified).0;
            assert_eq!(pass(&mut m, &verified).0, expected, "{at:x}");
            failed += usize::from(expected == Err(Uncovered::ReadFailed));
        }
    }
    assert!(failed > 400, "{failed}");
}

#[test]
fn the_rest_of_a_block_is_neither_read_as_a_field_nor_compared() {
    // Bytes between the fields of the character, its context, the stats, the manager, a node
    // and an instance: other values there are the same Magic Find.
    let gaps = |buff: (u64, u64, u64)| {
        [
            CHAR_CONTEXT + 0x10,
            CHAR_CONTEXT + 0x88,
            CHARACTER + 0x10,
            CHARACTER + 0x100,
            CHARACTER + 0x21c,
            STATS + 0x10,
            MANAGER + 0x30,
            MANAGER + 0xe8,
            buff.1 + 8,
            buff.2 + 0x30,
            buff.2 + 0x50,
        ]
    };
    let (mut m, buff) = standard();
    for at in gaps(buff) {
        m.put(at, 0x1234_5678, 4);
    }
    assert_eq!(total(m), 337.0);
    // The same bytes changing between the two looks are not a changed sample either.
    let (mut m, buff) = standard();
    m.race = Some((CTX + 0x98, 2, gaps(buff).map(|at| (at, 0x1234_5678, 4)).to_vec()));
    assert_eq!(total(m), 337.0);
}

#[test]
fn total_is_account_plus_pushed_plus_buffs() {
    let (result, bytes) = run(standard().0);
    assert_eq!(
        result,
        Ok(MagicFind {
            total: 337.0,
            luck: 300,
            pushed: 7.0,
            buffs: 30.0,
            boon: false
        })
    );
    assert!(bytes > GUARD_BYTES && bytes <= MAX_BYTES);
    assert_eq!(GUARDS.iter().map(|guard| guard.size).sum::<usize>(), GUARD_BYTES);
}

#[test]
fn empty_tables_are_supported_zeroes_and_not_missing_coverage() {
    assert_eq!(total(empty_at(4, 64, 0)), 0.0);
    // The client returns 0 for an empty pushed table and no buff for a zero count without
    // dereferencing either pointer.
    let mut m = empty();
    m.put(MANAGER + 0xd8, 0, 8);
    m.put(MANAGER + 0x28, 0, 8);
    m.put(MANAGER + 0x20, 0, 4);
    assert_eq!(total(m), 300.0);
    let mut m = empty();
    m.push(MAGIC_FIND, 7.0);
    assert_eq!(total(m), 307.0);
}

#[test]
fn identities_on_the_route_reject() {
    for address in [
        CHAR_CONTEXT,
        CHARACTER,
        CHARACTER + 8,
        CHARACTER + 0x40,
        PLAYER,
        STATS,
        MANAGER,
        BASE + BUFF_MANAGER_VTABLE + 0x28,
        BASE + PLAYER_STATS_VTABLE + 0x30,
        BASE + COMBATANT_VTABLE + 0x38,
    ] {
        let result = changed(|m, _| m.put(address, BASE + 0x123400, 8));
        assert_eq!(result, Err(Uncovered::Profile), "{address:x}");
    }
    // A table node of another class.
    assert_eq!(changed(|m, buff| m.put(buff.1, BASE + 0x123400, 8)), Err(Uncovered::Profile));
}

#[test]
fn character_must_be_the_controlled_local_player() {
    assert_eq!(changed(|m, _| m.put(CHARACTER + 0x178, 0, 4)), Err(Uncovered::Root));
    assert_eq!(changed(|m, _| m.put(CHARACTER + 0xa0, 0x2000_0007, 4)), Err(Uncovered::Root));
    assert_eq!(changed(|m, _| m.put(CHARACTER + 0xa0, 0x3000_0000, 4)), Err(Uncovered::Bounds));
    assert_eq!(changed(|m, _| m.put(CHAR_CONTEXT + 0x8c, PLAYER_ID, 4)), Err(Uncovered::Bounds));
    let other = changed(|m, _| m.put(PLAYERS + 8 * PLAYER_ID, PLAYER + 0x10000, 8));
    assert_eq!(other, Err(Uncovered::Profile));
    // The id cached on the character wins over the one derived from the agent id.
    let cached = |fix: bool| {
        changed(|m, _| {
            m.put(CHARACTER + 0x220, 9, 4);
            m.put(PLAYERS + 8 * 9, if fix { PLAYER } else { PLAYER + 0x10000 }, 8);
        })
    };
    assert_eq!(cached(false), Err(Uncovered::Profile));
    assert_eq!(cached(true).map(|value| value.total), Ok(337.0));
}

#[test]
fn content_pointers_off_their_observed_alignment_reject() {
    for remainder in [0, 1, 2, 6] {
        assert_eq!(read(standard_at(remainder).0), Err(Uncovered::Alignment), "{remainder}");
    }
}

#[test]
fn each_content_hop_is_checked_on_its_own() {
    // One hop moved to an 8-byte boundary, with valid bytes waiting there, is still rejected.
    let moved = |holder: fn(&Fixture, (u64, u64, u64)) -> u64, size: u64| {
        changed(move |m, buff| {
            let holder = holder(m, buff);
            let source = m.get(holder);
            let copy: Vec<u8> = (0..size)
                .map(|offset| *m.bytes.get(&(source + offset)).unwrap_or(&0))
                .collect();
            for (offset, byte) in copy.into_iter().enumerate() {
                m.bytes.insert(source + 4 + offset as u64, byte);
            }
            m.put(holder, source + 4, 8);
        })
    };
    assert_eq!(moved(|_, buff| buff.1 + 0x10, 0x70), Err(Uncovered::Alignment));
    assert_eq!(moved(|_, buff| buff.2 + 0x60, 0x30), Err(Uncovered::Alignment));
    assert_eq!(moved(|m, buff| m.get(buff.2 + 0x60) + 0x20, 0x20), Err(Uncovered::Alignment));
    assert_eq!(
        moved(|m, buff| m.get(m.get(buff.2 + 0x60) + 0x20) + 0x10, 0x48),
        Err(Uncovered::Alignment)
    );
}

#[test]
fn heap_pointers_keep_eight_byte_alignment() {
    // A node 4 past an 8-byte boundary, with a valid node waiting there.
    let node = changed(|m, buff| {
        m.put(buff.1 + 4, BASE + BUFF_NODE_VTABLE, 8);
        m.put(buff.1 + 4 + 0x10, buff.2, 8);
        m.put(buff.1 + 4 + 0x18, 1001, 4);
        m.put(buff.0 + 8, buff.1 + 4, 8);
    });
    assert_eq!(node, Err(Uncovered::Alignment));
    assert_eq!(changed(|m, _| m.put(MANAGER + 0x28, BUCKETS + 4, 8)), Err(Uncovered::Alignment));
    assert_eq!(changed(|m, _| m.put(MANAGER + 0xd8, PUSHED + 4, 8)), Err(Uncovered::Alignment));
    assert_eq!(changed(|m, _| m.put(CHARACTER + 0xd0, MANAGER + 4, 8)), Err(Uncovered::Alignment));
}

#[test]
fn null_and_out_of_range_pointers_reject() {
    assert_eq!(changed(|m, _| m.put(CHARACTER + 0xd0, 0, 8)), Err(Uncovered::Root));
    assert_eq!(changed(|m, buff| m.put(buff.1 + 0x10, 0, 8)), Err(Uncovered::Bounds));
    assert_eq!(changed(|m, buff| m.put(buff.2 + 0x60, 1 << 60, 8)), Err(Uncovered::Bounds));
    assert_eq!(changed(|m, buff| m.put(buff.0 + 8, 0, 8)), Err(Uncovered::Integrity));
}

#[test]
fn bucket_and_table_integrity_reject() {
    assert_eq!(changed(|m, buff| m.put(buff.0 + 0x10, (hash(1001) ^ 1) as u64, 4)), Err(Uncovered::Integrity));
    assert_eq!(changed(|m, buff| m.put(buff.1 + 0x18, 1002, 4)), Err(Uncovered::Integrity));
    assert_eq!(changed(|m, buff| m.put(buff.2 + 0x58, 0, 4)), Err(Uncovered::Integrity));
    assert_eq!(changed(|m, _| m.put(MANAGER + 0x24, 2, 4)), Err(Uncovered::Integrity));
    for (capacity, count) in [(0u64, 1u64), (3, 1), (1024, 1), (64, 65)] {
        let result = changed(|m, _| {
            m.put(MANAGER + 0x20, capacity, 4);
            m.put(MANAGER + 0x24, count, 4);
        });
        assert_eq!(result, Err(Uncovered::Bounds), "{capacity} {count}");
    }
    assert_eq!(changed(|m, _| m.luck(MAX_LUCK_LEVEL + 1)), Err(Uncovered::Bounds));
}

#[test]
fn definition_bounds_reject() {
    let mut m = empty();
    let definition = m.plain(&[record(MAGIC_FIND, 30.0)]);
    m.buff(1, 1, definition);
    m.put(definition + 0x28, 0, 4);
    assert_eq!(read(m), Err(Uncovered::Bounds));
    let mut m = empty();
    let definition = m.plain(&[record(5, 1.0); MAX_MODIFIERS as usize + 1]);
    m.buff(1, 1, definition);
    assert_eq!(read(m), Err(Uncovered::Bounds));
}

#[test]
fn pushed_table_sums_only_its_type_and_must_be_sorted_and_bounded() {
    let mut m = empty();
    for (kind, value) in [(14, 1000.0), (MAGIC_FIND, 7.0), (MAGIC_FIND, 3.0), (MAGIC_FIND_BOON, 40.0), (126, 5.0)] {
        m.push(kind, value);
    }
    assert_eq!(total(m), 310.0);
    let mut m = empty();
    m.push(MAGIC_FIND, 7.0);
    m.push(126, 5.0);
    m.put(PUSHED, 127, 4);
    assert_eq!(read(m), Err(Uncovered::Integrity));
    let mut m = empty();
    m.push(MAGIC_FIND, f32::NAN);
    assert_eq!(read(m), Err(Uncovered::Bounds));
    let mut m = empty();
    m.put(MANAGER + 0xe4, MAX_PUSHED as u64 + 1, 4);
    assert_eq!(read(m), Err(Uncovered::Bounds));
}

#[test]
fn unsupported_magic_find_record_is_no_coverage_never_a_partial_total() {
    let base = record(MAGIC_FIND, 50.0);
    for extra in [
        Record { formula: 0, ..base },
        Record { mode: 1, ..base },
        Record { need: 0x280004, ..base },
        Record { flags: 0x8, ..base },
    ] {
        assert_eq!(with_record(extra), Err(Uncovered::Unsupported));
    }
    let infinite = Record { value: f32::INFINITY, ..base };
    assert_eq!(with_record(infinite), Err(Uncovered::Bounds));
}

#[test]
fn records_the_client_does_not_count_are_ignored() {
    // Another modifier type, however unsupported its shape.
    let other = Record { formula: 0, mode: 2, need: 0x280004, flags: 0x1e, ..record(14, 100.0) };
    assert_eq!(with_record(other).map(|value| value.total), Ok(337.0));
    // A target condition: the widget passes no target, so the client skips the record.
    let targeted = Record { formula: 0, target: 0x280004, ..record(MAGIC_FIND, 99.0) };
    assert_eq!(with_record(targeted).map(|value| value.total), Ok(337.0));
    // The stop flag ends a definition after its first match.
    let mut m = empty();
    let first = Record { flags: 1, ..record(MAGIC_FIND, 10.0) };
    let definition = m.plain(&[first, record(MAGIC_FIND, 20.0)]);
    m.buff(1, 1, definition);
    assert_eq!(total(m), 310.0);
}

#[test]
fn stacking_counts_once_per_effect_unless_by_intensity() {
    let mut m = empty();
    let duration = m.definition(&[record(MAGIC_FIND, 10.0)], 1, 1, 0);
    let intensity = m.definition(&[record(MAGIC_FIND, 1.0)], 4, 1, 0);
    for key in [1, 2, 3] {
        m.buff(key, 700, duration);
    }
    for key in [11, 12, 13, 14] {
        m.buff(key, 800, intensity);
    }
    assert_eq!(total(m), 314.0);
}

#[test]
fn boon_modifier_needs_an_applied_category_zero_buff() {
    let (mut m, _) = standard();
    let boon_only = m.plain(&[record(MAGIC_FIND_BOON, 40.0)]);
    m.buff(1002, 502, boon_only);
    m.push(MAGIC_FIND_BOON, 2.0);
    assert_eq!(read(m.clone()).map(|value| (value.total, value.boon)), Ok((337.0, false)));
    let boon = m.definition(&[record(14, 5.0)], 0, 0, 0);
    m.buff(1003, 503, boon);
    assert_eq!(
        read(m),
        Ok(MagicFind { total: 379.0, luck: 300, pushed: 9.0, buffs: 70.0, boon: true })
    );
    // An unsupported boon-only record matters only once it counts.
    let (mut m, _) = standard();
    let conditional = m.plain(&[Record { mode: 1, ..record(MAGIC_FIND_BOON, 40.0) }]);
    m.buff(1002, 502, conditional);
    assert_eq!(read(m.clone()).map(|value| value.total), Ok(337.0));
    let boon = m.definition(&[], 0, 0, 0);
    m.buff(1003, 503, boon);
    assert_eq!(read(m), Err(Uncovered::Unsupported));
}

#[test]
fn manager_mode_hides_flagged_definitions() {
    let (mut m, _) = standard();
    let hidden = m.definition(&[record(MAGIC_FIND, 50.0)], 0, 1, 0x40);
    m.buff(1002, 502, hidden);
    assert_eq!(total(m.clone()), 387.0);
    m.put(MANAGER + 0xf0, 1, 4);
    assert_eq!(total(m), 337.0);
}

#[test]
fn changes_between_the_two_passes_reject() {
    for (target, value, size) in [
        (STATS + 0x24, 301u64, 4usize),
        (MANAGER + 0x24, 2, 4),
        (MANAGER + 0xe4, 2, 4),
        (MANAGER + 0xf0, 1, 4),
        (PUSHED + 4, 8.0f32.to_bits() as u64, 4),
        (BUCKETS + 4, 1, 4),
    ] {
        let (mut m, _) = standard();
        // The second read of the context pointer is the first reread of the consistency pass.
        m.race = Some((CTX + 0x98, 2, vec![(target, value, size)]));
        assert_eq!(read(m), Err(Uncovered::Changed), "{target:x}");
    }
    // Another character under the same context: a different owner, never the same sample.
    let (mut m, _) = standard();
    m.character(CHARACTER + 0x1000);
    m.race = Some((CTX + 0x98, 2, vec![(CHAR_CONTEXT + 0x98, CHARACTER + 0x1000, 8)]));
    assert_eq!(read(m), Err(Uncovered::Changed));
    // A vtable replaced while reading is the route's identity failing at the recheck.
    let (mut m, _) = standard();
    m.race = Some((CTX + 0x98, 2, vec![(MANAGER, BASE + 0x123400, 8)]));
    assert_eq!(read(m), Err(Uncovered::Profile));
}

#[test]
fn production_constants_are_the_audited_profile() {
    let audited: serde_json::Value =
        serde_json::from_str(include_str!("fixtures/magic_find_profile.json")).unwrap();
    for (name, value) in [
        ("char_context_vtable", CHAR_CONTEXT_VTABLE),
        ("character_vtable", CHARACTER_VTABLE),
        ("character_agent_vtable", CHARACTER_AGENT_VTABLE),
        ("combatant_vtable", COMBATANT_VTABLE),
        ("player_vtable", PLAYER_VTABLE),
        ("player_stats_vtable", PLAYER_STATS_VTABLE),
        ("buff_manager_vtable", BUFF_MANAGER_VTABLE),
        ("buff_node_vtable", BUFF_NODE_VTABLE),
        ("magic_find_modifier", MAGIC_FIND as u64),
        ("magic_find_boon_modifier", MAGIC_FIND_BOON as u64),
        ("content_pointer_remainder", tyrian_companion_nexus_core::passive::CONTENT_REMAINDER),
    ] {
        assert_eq!(audited[name].as_u64(), Some(value), "{name}");
    }
    assert_eq!(audited["binary_sha256"].as_str(), Some(BUILD_SHA256));
    // The only formula the reader evaluates is the audited constant one.
    assert_eq!(audited["constant_formula"].as_u64(), Some(6));
    assert_eq!(with_record(record(MAGIC_FIND, 50.0)).map(|value| value.total), Ok(387.0));
    let guards = audited["guards"].as_array().unwrap();
    assert_eq!(guards.len(), GUARDS.len());
    for (expected, guard) in guards.iter().zip(GUARDS) {
        assert_eq!(expected["name"].as_str(), Some(guard.name));
        assert_eq!(expected["rva"].as_u64(), Some(guard.rva), "{}", guard.name);
        assert_eq!(expected["size"].as_u64(), Some(guard.size as u64), "{}", guard.name);
        assert_eq!(expected["sha256"].as_str(), Some(guard.sha256), "{}", guard.name);
    }
    let slots = audited["slots"].as_array().unwrap();
    assert_eq!(slots.len(), SLOTS.len());
    for (expected, (vtable, slot, target)) in slots.iter().zip(SLOTS) {
        let name = expected["target"].as_str().unwrap();
        let table = audited[expected["vtable"].as_str().unwrap()].as_u64();
        assert_eq!(table, Some(vtable), "{name}");
        assert_eq!(expected["slot"].as_u64(), Some(slot), "{name}");
        assert_eq!(expected["target_rva"].as_u64(), Some(target), "{name}");
        // Every slot selects code that is itself guarded by digest.
        assert!(GUARDS.iter().any(|guard| guard.name == name && guard.rva == target), "{name}");
    }
}

#[test]
fn only_the_audited_ranges_can_stand_for_a_verified_profile() {
    let verify = |guards: &[Guard]| {
        let mut reader = Reader::bounded(standard().0, MAX_BYTES);
        MagicFindProfile::verified_against(&mut reader, profile(), guards).map(|_| reader.bytes)
    };
    let synthetic = synthetic_guards();
    assert_eq!(verify(&synthetic), Ok(GUARD_BYTES));
    // No guard, a list without the hash table, or a range that is not the audited one.
    assert_eq!(verify(&[]), Err(Uncovered::Guard));
    assert_eq!(verify(&synthetic[..18]), Err(Uncovered::Guard));
    let mut moved = synthetic.clone();
    moved[14].rva += 16;
    assert_eq!(verify(&moved), Err(Uncovered::Guard));
    let mut resized = synthetic;
    resized[18].size -= 4;
    assert_eq!(verify(&resized), Err(Uncovered::Guard));
}

#[test]
fn nothing_pushed_is_positive_zero_and_a_negative_total_is_no_value() {
    // An empty f32 `sum()` would be -0.0, which a panel would print as "-0".
    let value = read(empty()).unwrap();
    assert_eq!(value.pushed.to_bits(), 0.0f32.to_bits());
    assert_eq!(value.buffs.to_bits(), 0.0f32.to_bits());
    assert!(!value.total.is_sign_negative());
    let value = read(empty_at(4, 64, 0)).unwrap();
    assert_eq!(value.total.to_bits(), 0.0f32.to_bits());
    // One addend may be negative while the total is not.
    let mut m = empty();
    m.push(MAGIC_FIND, -50.0);
    assert_eq!(read(m).map(|value| (value.total, value.pushed)), Ok((250.0, -50.0)));
    let (mut m, _) = standard();
    let penalty = m.plain(&[record(MAGIC_FIND, -37.0)]);
    m.buff(1002, 502, penalty);
    assert_eq!(read(m).map(|value| (value.total, value.buffs)), Ok((300.0, -7.0)));
    // A negative total is not a Magic Find to show.
    let mut m = empty_at(4, 64, 0);
    m.push(MAGIC_FIND, -50.0);
    assert_eq!(read(m), Err(Uncovered::Bounds));
    let mut m = empty_at(4, 64, 10);
    let penalty = m.plain(&[record(MAGIC_FIND, -10.5)]);
    m.buff(1, 1, penalty);
    assert_eq!(read(m), Err(Uncovered::Bounds));
}

#[test]
fn a_finite_value_above_the_bound_rejects() {
    // Finite, so only the bound of 10000 can be what rejects it; 10000 itself is accepted.
    assert_eq!(with_record(record(MAGIC_FIND, 10_000.5)), Err(Uncovered::Bounds));
    assert_eq!(with_record(record(MAGIC_FIND, -10_000.5)), Err(Uncovered::Bounds));
    assert_eq!(with_record(record(MAGIC_FIND, 10_000.0)).map(|v| v.total), Ok(10_337.0));
    for value in [10_000.5, -10_000.5] {
        let mut m = empty();
        m.push(MAGIC_FIND, value);
        assert_eq!(read(m), Err(Uncovered::Bounds), "{value}");
    }
}

#[test]
fn every_requirement_pointer_and_every_state_bit_is_unsupported_on_its_own() {
    let base = record(MAGIC_FIND, 50.0);
    for extra in [
        Record { need: 0x280004, ..base },
        Record { need_b: 0x280004, ..base },
        Record { when_a: 0x280004, ..base },
        Record { when_b: 0x280004, ..base },
    ] {
        assert_eq!(with_record(extra), Err(Uncovered::Unsupported));
    }
    for flags in [0x2, 0x4, 0x8, 0x10, 0x6, 0x18, 0x1e, 0x1f] {
        let extra = Record { flags, ..base };
        assert_eq!(with_record(extra), Err(Uncovered::Unsupported), "{flags:#x}");
    }
    // Bits outside the mask are not conditions: 0x1 only ends the definition, 0x20 is ignored.
    for flags in [0x1, 0x20, 0x21] {
        let extra = Record { flags, ..base };
        assert_eq!(with_record(extra).map(|value| value.total), Ok(387.0), "{flags:#x}");
    }
}

#[test]
fn player_id_above_the_bound_rejects_even_inside_the_player_array() {
    // The array claims to be long enough, so only the bound on the id can reject it.
    let result = changed(|m, _| {
        m.put(CHARACTER + 0x220, 0x1_0000, 4);
        m.put(CHAR_CONTEXT + 0x8c, 0x2_0000, 4);
        m.put(PLAYERS + 8 * 0x1_0000, PLAYER, 8);
    });
    assert_eq!(result, Err(Uncovered::Bounds));
    let edge = changed(|m, _| {
        m.put(CHARACTER + 0x220, 0xffff, 4);
        m.put(CHAR_CONTEXT + 0x8c, 0x2_0000, 4);
        m.put(PLAYERS + 8 * 0xffff, PLAYER, 8);
    });
    assert_eq!(edge.map(|value| value.total), Ok(337.0));
}

#[test]
fn a_count_without_its_table_is_bounds_not_an_absent_character() {
    assert_eq!(changed(|m, _| m.put(MANAGER + 0xd8, 0, 8)), Err(Uncovered::Bounds));
    assert_eq!(changed(|m, _| m.put(MANAGER + 0x28, 0, 8)), Err(Uncovered::Bounds));
    assert_eq!(changed(|m, _| m.put(CTX + 0x98, 0, 8)), Err(Uncovered::Root));
    assert_eq!(changed(|m, _| m.put(CHAR_CONTEXT + 0xa0, 0, 8)), Err(Uncovered::Root));
}

#[test]
fn failed_read_and_exhausted_budget_never_become_a_value() {
    // The node's link, the instance's reference and the definition's group, each on its own.
    let failing = |at: fn(&Fixture, (u64, u64, u64)) -> u64| {
        let (mut m, buff) = standard();
        m.fail = Some(at(&m, buff));
        read(m)
    };
    assert_eq!(failing(|_, buff| buff.1 + 0x10), Err(Uncovered::ReadFailed));
    assert_eq!(failing(|_, buff| buff.2 + 0x60), Err(Uncovered::ReadFailed));
    assert_eq!(failing(|m, buff| m.get(m.get(buff.2 + 0x60) + 0x20) + 0x10), Err(Uncovered::ReadFailed));
    let mut reader = Reader::bounded(standard().0, GUARD_BYTES + 200);
    let verified =
        MagicFindProfile::verified_against(&mut reader, profile(), &synthetic_guards()).unwrap();
    assert_eq!(magic_find(&mut reader, &verified, CTX).map(|_| ()), Err(Uncovered::Bounds));
}

#[test]
fn a_changed_guard_byte_stops_the_cycle_at_that_guard() {
    let (mut m, _) = standard();
    m.bytes.entry(BASE + GUARDS[14].rva + 2535).and_modify(|byte| *byte ^= 1);
    let (result, bytes) = run(m);
    assert_eq!(result, Err(Uncovered::Guard));
    // Fifteen guards were copied, the fifteenth differed in its last byte, and nothing after
    // it was requested: not the last four guards, and no field of the game.
    assert_eq!(bytes, GUARDS[..15].iter().map(|guard| guard.size).sum::<usize>());
    // The real digests do not accept the fixtures' synthetic contents, and stop at the first.
    let mut reader = Reader::bounded(standard().0, MAX_BYTES);
    assert_eq!(
        MagicFindProfile::verified(&mut reader, profile()).map(|_| ()),
        Err(Uncovered::Guard)
    );
    assert_eq!(reader.bytes, GUARDS[0].size);
}

#[test]
fn misaligned_or_low_context_rejects_before_reading() {
    let mut reader = Reader::bounded(standard().0, MAX_BYTES);
    let verified =
        MagicFindProfile::verified_against(&mut reader, profile(), &synthetic_guards()).unwrap();
    let before = reader.bytes;
    assert_eq!(magic_find(&mut reader, &verified, CTX + 4).map(|_| ()), Err(Uncovered::Bounds));
    assert_eq!(magic_find(&mut reader, &verified, 0x100).map(|_| ()), Err(Uncovered::Bounds));
    assert_eq!(reader.bytes, before);
}
