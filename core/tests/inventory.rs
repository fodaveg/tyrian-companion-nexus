//! Sparse fixtures ported from the certified Python reader; never open a real process.
use std::collections::BTreeMap;
use std::time::{Duration, Instant};
use tyrian_companion_nexus_core::inventory::*;
const BASE: u64 = 0x140000000;
const CTX: u64 = 0x100000;
const ITEMCTX: u64 = 0x101000;
const CHARCTX: u64 = 0x109000;
const CHARACTER: u64 = 0x10a000;
const INV: u64 = 0x10b000;
const SLOTS: u64 = 0x10c000;
const RESOLVER: u64 = 0x106000;
const ITEM: u64 = 0x107000;
const DEF: u64 = 0x108000;
const PAYLOAD: u64 = 0x120000;
const VT: u64 = BASE + 0x225c148;
const STACK_VT: u64 = BASE + 0x225c460;
/// A write by the game between two copies: `value` lands at `target` just before the
/// `visit`th copy that covers `on` is served, whatever the size of that copy.
#[derive(Clone, Copy)]
struct Race {
    on: u64,
    visit: usize,
    target: u64,
    value: u64,
    size: usize,
}
#[derive(Default, Clone)]
struct Fixture {
    bytes: BTreeMap<u64, u8>,
    calls: Vec<(u64, usize)>,
    forbidden: Vec<u64>,
    race: Option<Race>,
    visits: usize,
}
impl Fixture {
    fn put(&mut self, address: u64, value: u64, size: usize) {
        for (i, byte) in value.to_le_bytes()[..size].iter().enumerate() {
            self.bytes.insert(address + i as u64, *byte);
        }
    }
    /// The field at `target` changes just before it is copied for the `visit`th time.
    fn racing(mut self, target: u64, visit: usize, value: u64, size: usize) -> Self {
        self.race = Some(Race {
            on: target,
            visit,
            target,
            value,
            size,
        });
        self
    }
    /// How many copies covered `address`.
    fn looks(&self, address: u64) -> usize {
        self.calls
            .iter()
            .filter(|(start, size)| (*start..*start + *size as u64).contains(&address))
            .count()
    }
}
impl Memory for Fixture {
    fn read_exact(&mut self, address: u64, out: &mut [u8]) -> Result<(), ReadError> {
        // A copy is judged by every byte it covers, not by where it starts.
        let covered = address..address + out.len() as u64;
        assert!(
            !self.forbidden.iter().any(|byte| covered.contains(byte)),
            "forbidden read {address:x}+{}",
            out.len()
        );
        if !(0x100000..0x800000).contains(&address) && !(BASE..BASE + 0x2c48000).contains(&address)
        {
            return Err(ReadError::ReadFailed);
        }
        if let Some(race) = self.race {
            if covered.contains(&race.on) {
                self.visits += 1;
                if self.visits == race.visit {
                    self.put(race.target, race.value, race.size);
                }
            }
        }
        self.calls.push((address, out.len()));
        for (i, byte) in out.iter_mut().enumerate() {
            *byte = *self.bytes.get(&(address + i as u64)).unwrap_or(&0);
        }
        Ok(())
    }
}
impl Memory for &mut Fixture {
    fn read_exact(&mut self, address: u64, out: &mut [u8]) -> Result<(), ReadError> {
        (**self).read_exact(address, out)
    }
}
fn profile() -> BuildProfile {
    BuildProfile::checked(BUILD_SHA256, BASE, 0x2c48000).unwrap()
}
fn fixture(count: u64) -> Fixture {
    let mut m = Fixture::default();
    for (address, value, size) in [
        (CTX + 0x98, CHARCTX, 8),
        (CTX + 0x178, ITEMCTX, 8),
        (CHARCTX, BASE + 0x215cf48, 8),
        (BASE + 0x215cf48 + 0x68, BASE + 0x11b4480, 8),
        (CHARCTX + 0x98, CHARACTER, 8),
        (CHARACTER + 0x178, 0x18, 4),
        (CHARACTER + 8, BASE + 0x21601d0, 8),
        (BASE + 0x21601d0 + 0xc8, BASE + 0x11d74d0, 8),
        (CHARACTER + 0x3f0, INV, 8),
        (INV, BASE + 0x21621a8, 8),
        (BASE + 0x21621a8 + 0x220, BASE + 0x45dd90, 8),
        (BASE + 0x21621a8 + 0x190, BASE + 0x11ee3d0, 8),
        (INV + 0x70, CHARACTER, 8),
        (INV + 0xc8, SLOTS, 8),
        (INV + 0xd0, count, 4),
        (INV + 0xd4, count, 4),
        (SLOTS, ITEM, 8),
        (ITEM, VT, 8),
        (VT + 8, BASE + 0x13c3e10, 8),
        (VT + 0x68, BASE + 0x31b980, 8),
        (VT + 0x70, BASE + 0x13c45d0, 8),
        (VT + 0xa0, BASE + 0x13c46d0, 8),
        (VT + 0x260, BASE + 0x84c310, 8),
        (ITEM + 0x38, 17, 4),
        (ITEM + 0x40, DEF, 8),
        (DEF + 0x28, 12147, 4),
        (ITEM + 0x48, 3, 2),
        (ITEM + 0x58, INV, 8),
        (ITEM + 0x98, STACK_VT, 8),
        (STACK_VT, BASE + 0x168d10, 8),
        (ITEM + 0xa0, 3, 4),
        (ITEMCTX, BASE + 0x225bd00, 8),
        (BASE + 0x225bd10, BASE + 0x13c6400, 8),
        (ITEMCTX + 0x30, RESOLVER, 8),
        (ITEMCTX + 0x38, 64, 4),
        (ITEMCTX + 0x3c, 32, 4),
        (RESOLVER + 17 * 8, ITEM, 8),
    ] {
        m.put(address, value, size);
    }
    m
}
fn snapshot(m: Fixture) -> Result<InventorySnapshot, ReadError> {
    inventory_snapshot(&mut Reader::new(m), profile(), CTX)
}
/// The first conditional profile on the one item of [`fixture`]: 7 while its definition's
/// payload says so, 1 otherwise.
fn conditional(count: u64) -> Fixture {
    let mut m = fixture(count);
    let vt = BASE + 0x225d580;
    for (address, value, size) in [
        (vt + 8, BASE + 0x13c3e10, 8),
        (vt + 0x68, BASE + 0x31b980, 8),
        (vt + 0x70, BASE + 0x13c45d0, 8),
        (vt + 0xa0, BASE + 0x13c46d0, 8),
        (vt + 0x260, BASE + 0x13c9d20, 8),
        (ITEM, vt, 8),
        (ITEM + 0xa8, BASE + 0x225d898, 8),
        (BASE + 0x225d898, BASE + 0x13c9d60, 8),
        (DEF + 0x2c, 5, 4),
        (DEF + 0x30, PAYLOAD, 8),
        (PAYLOAD, 1, 4),
        (ITEM + 0xa0, 7, 4),
    ] {
        m.put(address, value, size);
    }
    m
}
#[derive(Clone, Copy, Debug)]
enum Branch {
    /// The quantity getter that reads the stack at a fixed offset.
    Common,
    /// The first conditional profile, with its condition met: the stack is read too.
    Conditional,
}
/// A matrix of `positions` whose first `occupied` hold one stack of 7 each, every stack with
/// an ID, an instance and a definition of its own and all of them of one class.
fn dense(positions: u64, occupied: u64, branch: Branch) -> Fixture {
    let mut m = fixture(positions);
    let vt = match branch {
        Branch::Common => VT,
        Branch::Conditional => BASE + 0x225d580,
    };
    let evt = BASE + 0x225d898;
    if let Branch::Conditional = branch {
        for (off, fun) in [
            (8, 0x13c3e10),
            (0x68, 0x31b980),
            (0x70, 0x13c45d0),
            (0xa0, 0x13c46d0),
            (0x260, 0x13c9d20),
        ] {
            m.put(vt + off, BASE + fun, 8);
        }
        m.put(evt, BASE + 0x13c9d60, 8);
        m.put(PAYLOAD, 1, 4);
    }
    m.put(SLOTS, 0, 8);
    m.put(ITEMCTX + 0x30, 0x600000, 8);
    m.put(ITEMCTX + 0x38, 1000, 4);
    m.put(ITEMCTX + 0x3c, 1000, 4);
    for n in 0..occupied {
        let item = 0x200000 + 0x200 * n;
        let def = 0x400000 + 0x40 * n;
        for (a, v, z) in [
            (SLOTS + n * 8, item, 8),
            (0x600000 + (n + 1) * 8, item, 8),
            (item, vt, 8),
            (item + 0x38, n + 1, 4),
            (item + 0x40, def, 8),
            (def + 0x28, 12147 + n, 4),
            (item + 0x48, 3, 2),
            (item + 0x58, INV, 8),
            (item + 0x98, STACK_VT, 8),
            (item + 0xa0, 7, 4),
        ] {
            m.put(a, v, z);
        }
        if let Branch::Conditional = branch {
            m.put(def + 0x2c, 5, 4);
            m.put(def + 0x30, PAYLOAD, 8);
            m.put(item + 0xa8, evt, 8);
        }
    }
    m
}
/// One pass over `m`: its outcome and what it asked for.
fn pass(m: Fixture) -> (Result<InventorySnapshot, ReadError>, usize, usize) {
    let mut r = Reader::new(m);
    let result = inventory_snapshot(&mut r, profile(), CTX);
    (result, r.reads, r.bytes)
}
#[test]
fn positive_sparse_570_and_unknown_free_slots() {
    let mut r = Reader::new(fixture(570));
    let s = inventory_snapshot(&mut r, profile(), CTX).unwrap();
    assert_eq!(s.quantities, BTreeMap::from([(12147, 3)]));
    assert_eq!(s.free_slots, None);
    assert_eq!(s.unknown, 0);
    assert!(r.bytes < MAX_BYTES);
}
#[test]
fn zero_two_four_are_observations_not_causal_events() {
    for n in [0, 2, 4] {
        let mut m = fixture(570);
        m.put(ITEM + 0xa0, n, 4);
        assert_eq!(snapshot(m).unwrap().quantities[&12147], n as u32);
    }
}
#[test]
fn boundaries_owner_duplicates_and_invalid_pointer_fail_closed() {
    assert!(snapshot(fixture(640)).is_ok());
    assert_eq!(snapshot(fixture(641)), Err(ReadError::Bounds));
    for (field, value, size) in [
        (INV + 0x70, CHARACTER + 0x1000, 8),
        (INV + 0xc8, 0xdeadbeef, 8),
        (ITEM + 0xa0, 251, 4),
        (VT + 0xa0, BASE + 0x13c46d8, 8),
        (ITEMCTX + 0x3c, 65, 4),
        (SLOTS + 8, ITEM, 8),
        (RESOLVER + 17 * 8, ITEM + 8, 8),
    ] {
        let mut m = fixture(570);
        m.put(field, value, size);
        assert!(snapshot(m).is_err());
    }
}
#[test]
fn unknown_quantity_never_becomes_one_or_zero() {
    let mut m = fixture(570);
    m.put(VT + 0x260, BASE + 0x13c9d20, 8);
    let s = snapshot(m).unwrap();
    assert_eq!(s.unknown, 1);
    assert!(s.quantities.is_empty());
}
#[test]
fn certified_null_fallback_requires_machine_bytes() {
    let mut m = fixture(570);
    m.put(VT + 0x260, BASE + 0x168aa0, 8);
    m.put(BASE + 0x168aa0, 0xc3c033, 3);
    assert_eq!(snapshot(m).unwrap().quantities[&12147], 1);
    let mut m = fixture(570);
    m.put(VT + 0x260, BASE + 0x168aa0, 8);
    assert!(snapshot(m).is_err());
}
#[test]
fn location4_matrix_is_not_read() {
    let mut m = fixture(570);
    m.put(INV + 0xa8, 0xdeadbeef, 8);
    m.forbidden = vec![INV + 0xa8, INV + 0xb0, INV + 0xb4, 0xdeadbeef];
    let mut r = Reader::new(m);
    inventory_snapshot(&mut r, profile(), CTX).unwrap();
    assert!(r.bytes < MAX_BYTES);
}
#[test]
fn excluded_item_classification_and_identity_races_reject_the_capture() {
    struct Moving {
        fixture: Fixture,
        change: (u64, u64, usize),
    }
    impl Memory for Moving {
        fn read_exact(&mut self, address: u64, bytes: &mut [u8]) -> Result<(), ReadError> {
            self.fixture.read_exact(address, bytes)?;
            // Mutate after the initial excluded classification has already been copied.
            if address == ITEM + 0x48 {
                let (target, value, size) = self.change;
                self.fixture.put(target, value, size);
            }
            Ok(())
        }
    }
    for change in [(ITEM + 0x48, 3, 2), (ITEM, VT + 8, 8)] {
        let mut fixture = fixture(570);
        fixture.put(ITEM + 0x48, 4, 2);
        let mut reader = Reader::new(Moving { fixture, change });
        assert_eq!(
            inventory_snapshot(&mut reader, profile(), CTX),
            Err(ReadError::Changed)
        );
    }
}
#[test]
fn stable_excluded_item_does_not_read_its_owner_definition_or_quantity() {
    let mut fixture = fixture(570);
    fixture.put(ITEM + 0x48, 4, 2);
    // Every byte of each field: a wider copy that only overlaps one of them is refused too.
    fixture.forbidden = [
        (ITEM + 0x38, 4),
        (ITEM + 0x40, 8),
        (ITEM + 0x58, 8),
        (ITEM + 0x98, 12),
        (VT + 0x260, 8),
        // Nor is its class asked for what only an owned item must answer.
        (VT + 8, 8),
        (VT + 0x68, 8),
        (VT + 0xa0, 8),
    ]
    .into_iter()
    .flat_map(|(field, size)| field..field + size)
    .collect();
    let snapshot = snapshot(fixture).unwrap();
    assert!(snapshot.quantities.is_empty());
    assert_eq!(snapshot.unknown, 0);
}
#[test]
fn wrong_build_and_invalid_ranges_cannot_read() {
    assert!(matches!(
        BuildProfile::checked("bad", BASE, 0x2c48000),
        Err(ReadError::UnsupportedBuild)
    ));
    let mut r = Reader::new(fixture(570));
    assert!(r.read::<8>(1).is_err());
    assert!(r.read::<8>(u64::MAX - 1).is_err());
    assert_eq!(r.bytes, 0);
    r.bytes = MAX_BYTES;
    assert!(r.read::<8>(CTX).is_err());
    assert_eq!(r.reads, 0);
}
#[test]
fn identity_quantity_root_and_slot_races_rejected() {
    for (address, visit, value, size) in [
        (ITEM + 0xa0, 2, 4, 4),
        (INV + 0x70, 2, CHARACTER + 8, 8),
        (SLOTS, 2, 0, 8),
        (DEF + 0x28, 2, 36038, 4),
        (ITEMCTX + 0x30, 2, 0, 8),
    ] {
        let m = fixture(570).racing(address, visit, value, size);
        assert_eq!(snapshot(m), Err(ReadError::Changed), "{address:x}");
    }
}
/// The outcome of a write to `target` before each of its looks after the first, then the
/// capture a write after the last look leaves standing. Nothing watches a field once its
/// last look is over, so a look that went missing shows here as a capture that stands.
fn later_looks(
    stable: &Fixture,
    target: u64,
    size: usize,
    value: u64,
    looks: usize,
) -> Vec<Result<BTreeMap<u32, u32>, ReadError>> {
    let mut seen = stable.clone();
    let mut reader = Reader::new(&mut seen);
    inventory_snapshot(&mut reader, profile(), CTX).unwrap();
    assert_eq!(seen.looks(target), looks, "looks at {target:x}");
    (2..=looks + 1)
        .map(|visit| {
            snapshot(stable.clone().racing(target, visit, value, size)).map(|s| s.quantities)
        })
        .collect()
}
#[test]
fn every_later_look_at_a_volatile_field_of_a_stack_rejects_its_change() {
    let stable = fixture(570);
    let stands = Ok(BTreeMap::from([(12147, 3)]));
    let changed = Err(ReadError::Changed);
    for (target, size, value, expected) in [
        // The position empties, or another item takes it.
        (SLOTS, 8, 0, vec![changed.clone()]),
        (SLOTS, 8, 0x117000, vec![changed.clone()]),
        // The item's class, instance reference, resolver entry, definition, ID, location
        // and owner: read, read again after the quantity and once more in the final pass.
        (ITEM, 8, VT + 0x800, vec![changed.clone(); 2]),
        (ITEM + 0x38, 4, 18, vec![changed.clone(); 2]),
        (RESOLVER + 17 * 8, 8, 0x117000, vec![changed.clone(); 2]),
        (ITEM + 0x40, 8, 0x118000, vec![changed.clone(); 2]),
        (DEF + 0x28, 4, 36038, vec![changed.clone(); 2]),
        (ITEM + 0x48, 2, 4, vec![changed.clone(); 2]),
        (ITEM + 0x58, 8, INV + 0x1000, vec![changed.clone(); 2]),
        // The quantity: twice in the item's turn and twice in the final pass.
        (ITEM + 0xa0, 4, 4, vec![changed.clone(); 3]),
        // The stack's own class: the final pass takes the pointer it finds and judges it, so
        // one that is no stack class at all is a profile mismatch there.
        (
            ITEM + 0x98,
            8,
            STACK_VT + 8,
            vec![
                changed.clone(),
                Err(ReadError::ProfileMismatch),
                changed.clone(),
            ],
        ),
    ] {
        let mut expected = expected;
        expected.push(stands.clone());
        assert_eq!(
            later_looks(&stable, target, size, value, expected.len()),
            expected,
            "{target:x}"
        );
    }
}
#[test]
fn every_later_look_at_a_conditional_quantity_rejects_its_change() {
    let stable = conditional(570);
    let stands = Ok(BTreeMap::from([(12147, 7)]));
    let changed = Err(ReadError::Changed);
    let mismatch = Err(ReadError::ProfileMismatch);
    for (target, size, value, expected) in [
        // The embedded predicate class is judged once per quantity.
        (ITEM + 0xa8, 8, BASE + 0x225d8a0, vec![mismatch.clone()]),
        // Subtype, payload and condition: read and read again in each of the two quantities.
        // The final pass judges the subtype it finds, and takes the payload it finds: one
        // whose condition is not met gives 1, which is not the 7 of the item's turn.
        (
            DEF + 0x2c,
            4,
            6,
            vec![changed.clone(), mismatch.clone(), changed.clone()],
        ),
        (DEF + 0x30, 8, PAYLOAD + 0x100, vec![changed.clone(); 3]),
        (PAYLOAD, 4, 0, vec![changed.clone(); 3]),
        (ITEM + 0xa0, 4, 4, vec![changed.clone(); 3]),
        (DEF + 0x28, 4, 36038, vec![changed.clone(); 2]),
    ] {
        let mut expected = expected;
        expected.push(stands.clone());
        assert_eq!(
            later_looks(&stable, target, size, value, expected.len()),
            expected,
            "{target:x}"
        );
    }
}
#[test]
fn a_later_look_that_finds_no_pointer_at_all_is_out_of_bounds() {
    // A field that held a pointer and now holds something else is judged as a pointer before
    // it is compared, whether it is copied on its own or inside a wider copy.
    for (stable, target, looks) in [
        (fixture(570), ITEM, 3),
        (fixture(570), ITEM + 0x40, 3),
        (fixture(570), RESOLVER + 17 * 8, 3),
        (fixture(570), ITEM + 0x58, 3),
        (fixture(570), ITEM + 0x98, 4),
        (conditional(570), DEF + 0x30, 4),
    ] {
        for visit in 2..=looks {
            assert_eq!(
                snapshot(stable.clone().racing(target, visit, 0x1234, 8)),
                Err(ReadError::Bounds),
                "{target:x} {visit}"
            );
        }
    }
}
#[test]
fn a_position_that_changes_after_its_copy_rejects_the_capture() {
    // Any position of the matrix, occupied or empty, in the first 512 or after them.
    for position in [0, 1, 300, 511, 512, 569] {
        for value in [0, 0x117000] {
            if position == 0 || value != 0 {
                let m = fixture(570).racing(SLOTS + position * 8, 2, value, 8);
                assert_eq!(snapshot(m), Err(ReadError::Changed), "{position}");
            }
        }
    }
    // What is found there the second time is still judged as a pointer.
    let m = fixture(570).racing(SLOTS + 8, 2, 0x1234, 8);
    assert_eq!(snapshot(m), Err(ReadError::Bounds));
    // The change may come at any moment of the pass: here while the item's quantity is read.
    // The last one is a position the pass has not reached yet. The matrix is copied whole
    // before the first item, so it is a change like any other; a reader that took each
    // position in its turn would have followed the new pointer instead.
    for (target, value) in [(SLOTS, 0), (SLOTS, 0x117000), (SLOTS + 8 * 520, 0x117000)] {
        let mut m = fixture(570);
        m.race = Some(Race {
            on: ITEM + 0xa0,
            visit: 1,
            target,
            value,
            size: 8,
        });
        assert_eq!(snapshot(m), Err(ReadError::Changed), "{target:x}");
    }
}
#[test]
fn the_matrix_is_copied_whole_in_two_reads_and_a_part_that_fails_gives_no_sample() {
    // 570 positions do not fit one read: the stack in the last one is still found.
    let mut m = fixture(570);
    m.put(SLOTS, 0, 8);
    m.put(SLOTS + 8 * 569, ITEM, 8);
    let mut r = Reader::new(&mut m);
    let s = inventory_snapshot(&mut r, profile(), CTX).unwrap();
    assert_eq!(s.quantities, BTreeMap::from([(12147, 3)]));
    assert_eq!(s.positions, 570);
    let copies: Vec<_> = (m.calls.iter())
        .filter(|(address, _)| (SLOTS..SLOTS + 570 * 8).contains(address))
        .collect();
    assert_eq!(
        copies,
        [
            &(SLOTS, 4096),
            &(SLOTS + 4096, 464),
            &(SLOTS, 4096),
            &(SLOTS + 4096, 464)
        ]
    );
    // Nothing is readable where the second read of this matrix starts.
    let mut m = fixture(570);
    m.put(INV + 0xc8, 0x7ff800, 8);
    assert_eq!(snapshot(m), Err(ReadError::ReadFailed));
}
#[test]
fn conditional_profiles_true_and_certified_false() {
    for (primary, embedded, evt, slot, predicate, subtype, field, value, stack, getter) in [
        (
            0x225d580, 0xa8, 0x225d898, 0, 0x13c9d60, 5, 0, 1, 0x98, 0x13c9d20,
        ),
        (
            0x225dd78, 0xa8, 0x225e080, 0, 0x13ca590, 10, 0, 1, 0x98, 0x13c9d20,
        ),
        (
            0x225d910, 0xe0, 0x225dcb8, 0x20, 0x13ca130, 9, 0x18, 4, 0xe8, 0x13c9f60,
        ),
    ] {
        for active in [true, false] {
            let mut m = fixture(570);
            let vt = BASE + primary;
            for (off, fun) in [
                (8, 0x13c3e10),
                (0x68, 0x31b980),
                (0x70, 0x13c45d0),
                (0xa0, 0x13c46d0),
                (0x260, getter),
            ] {
                m.put(vt + off, BASE + fun, 8);
            }
            for (addr, val, size) in [
                (ITEM, vt, 8),
                (ITEM + embedded, BASE + evt, 8),
                (BASE + evt + slot, BASE + predicate, 8),
                (DEF + 0x2c, subtype, 4),
                (DEF + 0x30, PAYLOAD, 8),
                (PAYLOAD + field, if active { value } else { 0 }, 4),
                (ITEM + stack, STACK_VT, 8),
                (ITEM + stack + 8, 7, 4),
            ] {
                m.put(addr, val, size);
            }
            assert_eq!(
                snapshot(m).unwrap().quantities[&12147],
                if active { 7 } else { 1 }
            );
        }
    }
}
#[test]
fn teb_tls_route_null_and_bounds() {
    let mut m = fixture(0);
    for (a, v, z) in [
        (BASE + TLS_INDEX_RVA, 3, 4),
        (0x110000 + 0x58, 0x111000, 8),
        (0x111000 + 24, 0x112000, 8),
        (0x112000 + 0x10, CTX, 8),
        (CTX + 0x198, 0x113000, 8),
    ] {
        m.put(a, v, z);
    }
    let mut r = Reader::new(m);
    assert_eq!(
        context_from_teb(&mut r, profile(), 0x110000).unwrap(),
        Some(CTX)
    );
}

#[test]
fn aggregates_multiple_stacks_over_250_and_suppresses_same_id_if_unknown() {
    let mut m = fixture(570);
    let item2 = 0x117000;
    let vt2 = BASE + 0x225c900;
    // Copy one complete fixture instance, then bind a different sparse reference/slot.
    for off in 0..0x110 {
        let b = *m.bytes.get(&(ITEM + off)).unwrap_or(&0);
        m.bytes.insert(item2 + off, b);
    }
    for off in 0..0x268 {
        let b = *m.bytes.get(&(VT + off)).unwrap_or(&0);
        m.bytes.insert(vt2 + off, b);
    }
    m.put(item2, vt2, 8);
    m.put(item2 + 0x38, 18, 4);
    m.put(RESOLVER + 18 * 8, item2, 8);
    m.put(SLOTS + 8, item2, 8);
    m.put(ITEM + 0xa0, 200, 4);
    m.put(item2 + 0xa0, 200, 4);
    let value = snapshot(m.clone()).unwrap();
    assert_eq!(value.quantities[&12147], 400);
    m.put(vt2 + 0x260, BASE + 0x13c9d20, 8);
    let value = snapshot(m).unwrap();
    assert_eq!(value.unknown, 1);
    assert!(value.quantities.is_empty());
}

#[test]
fn observed_size_dense_conditional_inventory_fits_cycle_budget_and_excess_fails_closed() {
    let mut r = Reader::new(dense(640, 308, Branch::Conditional));
    let result = inventory_snapshot(&mut r, profile(), CTX);
    assert!(
        result.is_ok(),
        "{:?} bytes={} reads={}",
        result,
        r.bytes,
        r.reads
    );
    let s = result.unwrap();
    assert_eq!(s.quantities.len(), 308);
    assert!(r.bytes <= MAX_BYTES - 4096, "{}", r.bytes);
    let mut r = Reader::new(dense(640, 640, Branch::Conditional));
    assert_eq!(
        inventory_snapshot(&mut r, profile(), CTX),
        Err(ReadError::Bounds)
    );
    assert!(r.bytes <= MAX_BYTES);
}
/// What one pass asks for over a matrix of 512 positions, to the read and to the byte. A pass
/// that asks for more than [`MAX_BYTES`] reads nothing at all, so these are the figures that
/// decide how full the bags may be before the reader goes without coverage.
#[test]
fn a_pass_over_512_positions_asks_for_exactly_these_reads_and_bytes() {
    let asked: Vec<_> = [
        (0, Branch::Common),
        (313, Branch::Common),
        (512, Branch::Common),
        (313, Branch::Conditional),
        (414, Branch::Conditional),
    ]
    .into_iter()
    .map(|(occupied, branch)| {
        let (result, reads, bytes) = pass(dense(512, occupied, branch));
        let snapshot = result.unwrap();
        assert_eq!(snapshot.quantities.len() as u64, occupied);
        assert_eq!(
            snapshot.quantities.values().map(|n| *n as u64).sum::<u64>(),
            occupied * 7
        );
        (reads, bytes)
    })
    .collect();
    assert_eq!(
        asked,
        [
            (35, 8_416),
            (7_246, 62_974),
            (11_823, 97_600),
            (9_752, 88_030),
            (12_883, 113_684)
        ]
    );
}
/// The adapter finds the game context with the same reader before the pass, so that search
/// comes out of the same budget: `teb+0x30` and the route of [`context_from_teb`] for each of
/// at most 128 threads of its own. The worst cycle is one whose check of the thread it kept
/// failed at its last step and then walks them all: the check ([`verify_located`], 68 bytes)
/// and the walk are both read by the one reader.
fn discovery() -> usize {
    let (m, located) = located_fixture();
    let (_, _, check) = verify(m, &located);
    walk() + check
}
/// The walk over 128 threads alone.
fn walk() -> usize {
    let mut m = fixture(0);
    for (a, v, z) in [
        (BASE + TLS_INDEX_RVA, 3, 4),
        (0x110000 + 0x58, 0x111000, 8),
        (0x111000 + 24, 0x112000, 8),
        (0x112000 + 0x10, CTX, 8),
        (CTX + 0x198, 0x113000, 8),
    ] {
        m.put(a, v, z);
    }
    let mut r = Reader::new(m);
    context_from_teb(&mut r, profile(), 0x110000).unwrap();
    128 * (8 + r.bytes)
}
#[test]
fn full_bags_are_read_within_the_cycle_budget_beside_the_discovery() {
    assert_eq!(walk(), 5_632);
    assert_eq!(discovery(), 5_632 + 68);
    let full = |occupied, branch| {
        let (result, _, bytes) = pass(dense(512, occupied, branch));
        result.map(|snapshot| {
            assert_eq!(snapshot.quantities.len() as u64, occupied);
            assert_eq!(
                snapshot.quantities.values().map(|n| *n as u64).sum::<u64>(),
                occupied * 7
            );
            bytes
        })
    };
    // The 512 positions 16 bags of 32 can hold, every one with a stack, common branch.
    assert_eq!(full(512, Branch::Common), Ok(97_600));
    assert!(97_600 + discovery() <= MAX_BYTES);
    // A stack of a conditional class costs 80 bytes more. 460 of them fit beside the largest
    // discovery and 482 beside none; from 483 on the pass asks for more than the budget and
    // gives no sample rather than a part of one.
    assert_eq!(full(460, Branch::Conditional), Ok(125_368));
    assert!(125_368 + discovery() <= MAX_BYTES, "the worst cycle no longer fits: 460 stacks are not 460");
    // And it is the most: one more stack, 80 bytes, would not fit beside it.
    assert!(125_368 + 80 + discovery() > MAX_BYTES);
    assert_eq!(full(482, Branch::Conditional), Ok(130_956));
    assert_eq!(full(483, Branch::Conditional), Err(ReadError::Bounds));
    assert_eq!(full(512, Branch::Conditional), Err(ReadError::Bounds));
}
#[test]
fn a_class_is_judged_once_per_pass_and_read_again_before_the_pass_is_accepted() {
    let words = [
        VT + 8,
        VT + 0x68,
        VT + 0x70,
        VT + 0xa0,
        VT + 0x260,
        STACK_VT,
    ];
    // 313 stacks of one class: each word of it and of the stack's class is copied twice in
    // the pass, not once or more per stack.
    let mut m = dense(512, 313, Branch::Common);
    let mut r = Reader::new(&mut m);
    inventory_snapshot(&mut r, profile(), CTX).unwrap();
    for word in words {
        assert_eq!(m.looks(word), 2, "{word:x}");
    }
    // The second copy is what finds a word that changed while the pass ran.
    let changed = Err(ReadError::Changed);
    let stable = fixture(570);
    for word in words {
        assert_eq!(
            later_looks(&stable, word, 8, BASE + 0x13c46d8, 2),
            [changed.clone(), Ok(BTreeMap::from([(12147, 3)]))],
            "{word:x}"
        );
    }
    // The slot of the embedded predicate class of a conditional profile.
    assert_eq!(
        later_looks(&conditional(570), BASE + 0x225d898, 8, BASE + 0x13c9d68, 2),
        [changed.clone(), Ok(BTreeMap::from([(12147, 7)]))]
    );
    // The machine bytes of the certified NULL fallback.
    let mut fallback = fixture(570);
    fallback.put(VT + 0x260, BASE + 0x168aa0, 8);
    fallback.put(BASE + 0x168aa0, 0xc3c033, 3);
    assert_eq!(
        later_looks(&fallback, BASE + 0x168aa0, 3, 0xc3c031, 2),
        [changed, Ok(BTreeMap::from([(12147, 1)]))]
    );
}
#[test]
fn a_stack_class_outside_the_executable_vtables_is_judged_at_every_use() {
    // Memory the game writes is not a fixed word of the executable: nothing is remembered.
    let heap = 0x130000;
    let mut stable = dense(8, 3, Branch::Common);
    stable.put(heap, BASE + 0x168d10, 8);
    for n in 0..3 {
        stable.put(0x200000 + 0x200 * n + 0x98, heap, 8);
    }
    let mut m = stable.clone();
    let mut r = Reader::new(&mut m);
    let s = inventory_snapshot(&mut r, profile(), CTX).unwrap();
    assert_eq!(s.quantities.len(), 3);
    // Each stack's turn and each stack's final pass.
    assert_eq!(m.looks(heap), 6);
    for visit in 2..=6 {
        assert_eq!(
            snapshot(stable.clone().racing(heap, visit, 0, 8)),
            Err(ReadError::ProfileMismatch),
            "{visit}"
        );
    }
}
#[test]
fn invalid_teb_tls_index_and_read_count_reject_before_memory_request() {
    let mut r = Reader::new(fixture(0));
    assert_eq!(
        context_from_teb(&mut r, profile(), u64::MAX),
        Err(ReadError::Bounds)
    );
    assert_eq!(r.reads, 0);
    r.reads = MAX_READS;
    assert!(r.read::<2>(CTX).is_err());
    assert_eq!(r.bytes, 0);
    let mut m = fixture(0);
    m.put(BASE + TLS_INDEX_RVA, 4096, 4);
    let mut r = Reader::new(m);
    assert_eq!(
        context_from_teb(&mut r, profile(), 0x110000),
        Err(ReadError::Bounds)
    );
    assert_eq!(r.reads, 1);
}

const PID: u32 = 4242;
const THREAD: u32 = 77;
const TEB: u64 = 0x110000;
/// A process whose thread `THREAD` has its TEB at `TEB`, TLS route to `CTX` included.
fn located_fixture() -> (Fixture, Located) {
    let mut m = fixture(0);
    for (a, v, z) in [
        (TEB + 0x30, TEB, 8),
        (TEB + 0x40, PID as u64, 8),
        (TEB + 0x48, THREAD as u64, 8),
        (BASE + TLS_INDEX_RVA, 3, 4),
        (TEB + 0x58, 0x111000, 8),
        (0x111000 + 24, 0x112000, 8),
        (0x112000 + 0x10, CTX, 8),
        (CTX + 0x198, 0x113000, 8),
    ] {
        m.put(a, v, z);
    }
    (m, Located { thread: THREAD, teb: TEB, context: CTX, threads: 115 })
}
fn verify(m: Fixture, located: &Located) -> (Result<bool, ReadError>, usize, usize) {
    let mut r = Reader::new(m);
    let outcome = verify_located(&mut r, profile(), PID, located);
    (outcome, r.reads, r.bytes)
}

/// A cycle that verifies costs six copies and 68 bytes, where finding the thread again costs
/// one OpenThread, one query and the same six copies for each of the process's own threads
/// (about 115 in a running game: some 800 copies).
#[test]
fn a_stored_thread_is_verified_with_six_copies() {
    let (m, located) = located_fixture();
    assert_eq!(verify(m, &located), (Ok(true), 6, 32 + 4 + 8 + 8 + 8 + 8));
}

/// Anything that is not the thread and the route a full search found fails the check, and a
/// copy that cannot be made is an error: the caller searches the threads either way.
#[test]
fn a_stored_thread_that_is_not_what_was_found_fails_the_verification() {
    let (m, located) = located_fixture();
    for (what, address, value, size) in [
        ("a TEB that does not point to itself", TEB + 0x30, TEB + 8, 8),
        ("another process", TEB + 0x40, PID as u64 + 4, 8),
        ("the TEB reused by another thread", TEB + 0x48, THREAD as u64 + 1, 8),
        ("another TLS block", 0x111000 + 24, 0x114000, 8),
        ("a context that is another", 0x112000 + 0x10, CTX + 0x1000, 8),
        ("a context with no character context", CTX + 0x198, 0, 8),
        ("no TLS block", 0x111000 + 24, 0, 8),
        ("no TLS array", TEB + 0x58, 0, 8),
    ] {
        let mut changed = m.clone();
        changed.put(address, value, size);
        let (outcome, _, _) = verify(changed, &located);
        assert_eq!(outcome, Ok(false), "{what}");
    }
    // The thread's memory is gone: not a mismatch to shrug off, an error.
    let gone = Located { teb: 0x900000, ..located };
    assert_eq!(verify(m.clone(), &gone).0, Err(ReadError::ReadFailed));
    // A TEB that cannot be one is refused before it is read.
    let bad = Located { teb: 0x110004, ..located };
    assert_eq!(verify(m, &bad), (Err(ReadError::Bounds), 0, 0));
}

/// The full search stands for 30 seconds and is forgotten on request.
#[test]
fn a_full_search_stands_for_thirty_seconds_and_is_forgotten_on_request() {
    let (_, located) = located_fixture();
    let start = std::time::Instant::now();
    let mut search = ContextSearch::new();
    assert_eq!(search.stored(start), None, "nothing found yet");
    search.found(located, start);
    assert_eq!(search.stored(start), Some(located));
    assert_eq!(search.stored(start + CONTEXT_SEARCH_EVERY - std::time::Duration::from_millis(1)), Some(located));
    assert_eq!(search.stored(start + CONTEXT_SEARCH_EVERY), None, "30 s old: search again");
    // A clock that went back is no age at all.
    assert_eq!(search.stored(start - std::time::Duration::from_secs(1)), None);
    search.forget();
    assert_eq!(search.stored(start), None);
}

/// What the adapter does each cycle, with the two operations that ask the system faked: how
/// many times each was asked, and what they answer.
struct Cycle {
    verified: usize,
    searched: usize,
    thread_is_there: bool,
    search_answers: Result<Located, ReadError>,
}
impl Cycle {
    fn new(search_answers: Result<Located, ReadError>) -> Self {
        Self { verified: 0, searched: 0, thread_is_there: true, search_answers }
    }
    fn locate(&mut self, search: &mut ContextSearch, now: Instant) -> Result<Located, ReadError> {
        search.locate(
            now,
            self,
            |cycle, _| {
                cycle.verified += 1;
                cycle.thread_is_there
            },
            |cycle| {
                cycle.searched += 1;
                cycle.search_answers
            },
        )
    }
}

#[test]
fn a_cycle_searches_when_nothing_is_kept_and_verifies_while_it_stands() {
    let (_, found) = located_fixture();
    let start = Instant::now();
    let mut search = ContextSearch::new();
    let mut cycle = Cycle::new(Ok(found));
    assert_eq!(cycle.locate(&mut search, start), Ok(found));
    assert_eq!((cycle.verified, cycle.searched), (0, 1), "nothing was kept: no check, a search");
    for second in 1..30 {
        let now = start + Duration::from_secs(second);
        assert_eq!(cycle.locate(&mut search, now), Ok(found));
    }
    assert_eq!((cycle.verified, cycle.searched), (29, 1), "29 cycles checked, none searched");
    // At 30 s the thread is not asked about: the search is made again, and it is kept.
    assert_eq!(cycle.locate(&mut search, start + CONTEXT_SEARCH_EVERY), Ok(found));
    assert_eq!((cycle.verified, cycle.searched), (29, 2));
    assert!(search.stored(start + CONTEXT_SEARCH_EVERY).is_some());
}

#[test]
fn a_check_that_fails_searches_in_that_same_cycle_and_keeps_what_the_search_finds() {
    let (_, old) = located_fixture();
    let new = Located { thread: 78, teb: 0x120000, ..old };
    let start = Instant::now();
    let mut search = ContextSearch::new();
    search.found(old, start);
    // The thread is gone, or its id went to another thread with another TEB: either way the
    // check says no, and the answer of this cycle is the search's.
    let mut cycle = Cycle::new(Ok(new));
    cycle.thread_is_there = false;
    assert_eq!(cycle.locate(&mut search, start + Duration::from_secs(1)), Ok(new));
    assert_eq!((cycle.verified, cycle.searched), (1, 1));
    assert_eq!(search.stored(start + Duration::from_secs(1)), Some(new));
}

#[test]
fn a_search_that_fails_leaves_nothing_kept() {
    let (_, old) = located_fixture();
    let start = Instant::now();
    let now = start + Duration::from_secs(1);
    for (check_passes, kept) in [(false, true), (false, false)] {
        let mut search = ContextSearch::new();
        if kept {
            search.found(old, start);
        }
        let mut cycle = Cycle::new(Err(ReadError::RootUnavailable));
        cycle.thread_is_there = check_passes;
        assert_eq!(cycle.locate(&mut search, now), Err(ReadError::RootUnavailable));
        assert_eq!(search.stored(now), None, "kept: {kept}");
        // And the next cycle searches again, with nothing to check.
        let verified = cycle.verified;
        let _ = cycle.locate(&mut search, now);
        assert_eq!((cycle.verified, cycle.searched), (verified, 2), "kept: {kept}");
    }
}

#[test]
fn what_the_client_discards_or_the_unload_forgets_is_searched_for_again() {
    let (_, found) = located_fixture();
    let start = Instant::now();
    let mut search = ContextSearch::new();
    let mut cycle = Cycle::new(Ok(found));
    cycle.locate(&mut search, start).unwrap();
    // `discard_cycle` and the unload of the adapter are this call.
    search.forget();
    cycle.locate(&mut search, start + Duration::from_secs(1)).unwrap();
    assert_eq!((cycle.verified, cycle.searched), (0, 2), "forgotten: no check, a search");
}

/// The system's half of the check: it is asked first, and when it does not answer with the TEB
/// that was found nothing of the game is read.
#[test]
fn the_system_must_still_have_the_thread_with_the_same_teb_before_anything_is_read() {
    let (m, located) = located_fixture();
    let check = |system: Result<u64, ReadError>, m: Fixture| {
        let mut r = Reader::new(m);
        (still_located(&mut r, profile(), PID, &located, system), r.reads)
    };
    assert_eq!(check(Ok(TEB), m.clone()), (true, 6));
    // OpenThread or the query failed: the thread is gone, not ours, or cannot be asked.
    assert_eq!(check(Err(ReadError::ReadFailed), m.clone()), (false, 0));
    // The id belongs to another thread now, with another TEB.
    assert_eq!(check(Ok(TEB + 0x2000), m.clone()), (false, 0));
    // The system agrees with the TEB but the memory says another thread (reused TEB).
    let mut reused = m.clone();
    reused.put(TEB + 0x48, THREAD as u64 + 1, 8);
    assert_eq!(check(Ok(TEB), reused), (false, 1));
}
