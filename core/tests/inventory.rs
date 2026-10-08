//! Sparse fixtures ported from the certified Python reader; never open a real process.
use std::collections::BTreeMap;
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
    for (target, value) in [(SLOTS, 0), (SLOTS, 0x117000)] {
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
    for (occupied, branch, reads, bytes) in [
        (0, Branch::Common, 1_057, 8_416),
        (313, Branch::Common, 12_638, 82_910),
        (512, Branch::Common, 20_001, 130_272),
        (313, Branch::Conditional, 17_646, 112_958),
    ] {
        let (result, asked_reads, asked_bytes) = pass(dense(512, occupied, branch));
        let snapshot = result.unwrap();
        assert_eq!(snapshot.quantities.len() as u64, occupied);
        assert_eq!(
            snapshot.quantities.values().map(|n| *n as u64).sum::<u64>(),
            occupied * 7
        );
        assert_eq!(
            (asked_reads, asked_bytes),
            (reads, bytes),
            "{occupied} {branch:?}"
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
