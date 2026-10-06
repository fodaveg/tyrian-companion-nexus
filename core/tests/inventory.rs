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
const ITEM: u64 = 0x107000;
const DEF: u64 = 0x108000;
const VT: u64 = BASE + 0x225c148;
#[derive(Default, Clone)]
struct Fixture {
    bytes: BTreeMap<u64, u8>,
    calls: Vec<u64>,
    forbidden: Vec<u64>,
    race: Option<(u64, usize, u64)>,
    visits: usize,
}
impl Fixture {
    fn put(&mut self, address: u64, value: u64, size: usize) {
        for (i, byte) in value.to_le_bytes()[..size].iter().enumerate() {
            self.bytes.insert(address + i as u64, *byte);
        }
    }
}
impl Memory for Fixture {
    fn read_exact(&mut self, address: u64, out: &mut [u8]) -> Result<(), ReadError> {
        assert!(
            !self.forbidden.contains(&address),
            "forbidden read {address:x}"
        );
        if !(0x100000..0x800000).contains(&address) && !(BASE..BASE + 0x2c48000).contains(&address)
        {
            return Err(ReadError::ReadFailed);
        }
        if let Some((target, visit, value)) = self.race {
            if address == target {
                self.visits += 1;
                if self.visits == visit {
                    self.put(address, value, out.len());
                }
            }
        }
        self.calls.push(address);
        for (i, byte) in out.iter_mut().enumerate() {
            *byte = *self.bytes.get(&(address + i as u64)).unwrap_or(&0);
        }
        Ok(())
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
        (ITEM + 0x98, BASE + 0x225c460, 8),
        (BASE + 0x225c460, BASE + 0x168d10, 8),
        (ITEM + 0xa0, 3, 4),
        (ITEMCTX, BASE + 0x225bd00, 8),
        (BASE + 0x225bd10, BASE + 0x13c6400, 8),
        (ITEMCTX + 0x30, 0x106000, 8),
        (ITEMCTX + 0x38, 64, 4),
        (ITEMCTX + 0x3c, 32, 4),
        (0x106000 + 17 * 8, ITEM, 8),
    ] {
        m.put(address, value, size);
    }
    m
}
fn snapshot(m: Fixture) -> Result<InventorySnapshot, ReadError> {
    inventory_snapshot(&mut Reader::new(m), profile(), CTX)
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
        (0x106000 + 17 * 8, ITEM + 8, 8),
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
    for (address, visit, value) in [
        (ITEM + 0xa0, 2, 4),
        (INV + 0x70, 2, CHARACTER + 8),
        (SLOTS, 2, 0),
        (DEF + 0x28, 2, 36038),
        (ITEMCTX + 0x30, 2, 0),
    ] {
        let mut m = fixture(570);
        m.race = Some((address, visit, value));
        assert!(snapshot(m).is_err());
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
                (DEF + 0x30, 0x120000, 8),
                (0x120000 + field, if active { value } else { 0 }, 4),
                (ITEM + stack, BASE + 0x225c460, 8),
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
    m.put(0x106000 + 18 * 8, item2, 8);
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
    fn dense(count: u64) -> Fixture {
        let mut m = fixture(640);
        let vt = BASE + 0x225d580;
        let evt = BASE + 0x225d898;
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
        m.put(ITEMCTX + 0x30, 0x600000, 8);
        m.put(ITEMCTX + 0x38, 1000, 4);
        m.put(ITEMCTX + 0x3c, 1000, 4);
        for n in 0..count {
            let item = 0x200000 + 0x200 * n;
            let def = 0x400000 + 0x40 * n;
            for (a, v, z) in [
                (SLOTS + n * 8, item, 8),
                (0x600000 + (n + 1) * 8, item, 8),
                (item, vt, 8),
                (item + 0x38, n + 1, 4),
                (item + 0x40, def, 8),
                (def + 0x28, 12147 + n, 4),
                (def + 0x2c, 5, 4),
                (def + 0x30, 0x120000, 8),
                (0x120000, 1, 4),
                (item + 0x48, 3, 2),
                (item + 0x58, INV, 8),
                (item + 0xa8, evt, 8),
                (item + 0x98, BASE + 0x225c460, 8),
                (item + 0xa0, 7, 4),
            ] {
                m.put(a, v, z);
            }
        }
        m
    }
    let mut r = Reader::new(dense(308));
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
    let mut r = Reader::new(dense(640));
    assert_eq!(
        inventory_snapshot(&mut r, profile(), CTX),
        Err(ReadError::Bounds)
    );
    assert!(r.bytes <= MAX_BYTES);
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
