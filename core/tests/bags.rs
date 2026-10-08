//! Bag capacity fixtures over owned bytes; never open a real process.
//!
//! The fixtures carry no bytes of the game. They own synthetic contents for every guarded
//! range and the digests of those contents, and pass them to `BagProfile::verified_against`,
//! which accepts other digests only for exactly the audited ranges and which the addon never
//! calls.
//!
//! What ties production to the audit is `fixtures/bag_capacity_profile.json`: a verbatim copy
//! (sha256 `5c2303e3…599d81`) of `tyrian-companion/docs/audit/loot-bag-capacity-probe/
//! profile.json` at the commit "add the free-slots getter to the audited bag capacity profile".
//! It holds digests only, and that repository's `check_profile_offline.py` compares each of its
//! 12 digests and 10 slots with the installed executable.
//! `production_constants_are_the_audited_profile` requires `GUARDS`, `SLOTS` and the vtables to
//! equal it, so a mistyped digest or RVA here turns the suite red.
//!
//! Heap objects are placed on 8 bytes and game content 4 past, as the live runs of 2026-10-08
//! found them. `live_shape_of_8_october` is the state of that morning: 16 bags adding up to
//! 414, 313 of 512 positions in use, 101 free.
use std::collections::BTreeMap;
use tyrian_companion_nexus_core::bags::*;
use tyrian_companion_nexus_core::inventory::{
    BuildProfile, Memory, ReadError, Reader, BUILD_SHA256,
};
use tyrian_companion_nexus_core::passive::{Guard, Uncovered};
use tyrian_companion_nexus_core::sha256::{hex, sha256};

const BASE: u64 = 0x140000000;
const IMAGE: u64 = 0x2c48000;
const CTX: u64 = 0x200000;
const CHAR_CONTEXT: u64 = 0x210000;
const CHARACTER: u64 = 0x220000;
const INVENTORY: u64 = 0x230000;
const ARRAY: u64 = 0x240000;
const HEAP: u64 = 0x250000;
const LIVE_SIZES: [u32; 16] = [
    18, 32, 24, 20, 20, 20, 20, 20, 20, 28, 32, 32, 32, 32, 32, 32,
];
/// The class the first external candidate accepted by mistake: `ItCliConsumable`.
const CONSUMABLE_VTABLE: u64 = 0x225d070;

fn synthetic_bytes(guard: &Guard) -> Vec<u8> {
    (0..guard.size)
        .map(|i| (i as u64 * 31 + guard.rva * 7 + guard.name.len() as u64) as u8)
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

#[derive(Default, Clone)]
struct Fixture {
    bytes: BTreeMap<u64, u8>,
    heap: u64,
    remainder: u64,
    fail: Option<u64>,
    /// On the given visit of a read starting at `.0`, first write each `(address, value, size)`.
    race: Option<(u64, usize, Vec<(u64, u64, usize)>)>,
    visits: usize,
    /// item, definition and payload of each bag, by slot.
    bags: BTreeMap<usize, (u64, u64, u64)>,
}
impl Fixture {
    fn put(&mut self, address: u64, value: u64, size: usize) {
        for (i, byte) in value.to_le_bytes()[..size].iter().enumerate() {
            self.bytes.insert(address + i as u64, *byte);
        }
    }
    fn alloc(&mut self, size: u64, content: bool) -> u64 {
        let address = self.heap + if content { self.remainder } else { 0 };
        self.heap += (size + 31) & !15;
        address
    }
    fn bag(&mut self, slot: usize, size: u32) {
        let item = self.alloc(0x98, false);
        let definition = self.alloc(0x48, true);
        let payload = self.alloc(0x38, true);
        self.put(item, BASE + BAG_ITEM_VTABLE, 8);
        self.put(item + 0x40, definition, 8);
        self.put(definition + 0x28, 9000 + slot as u64, 4);
        self.put(definition + 0x2c, 3, 4);
        self.put(definition + 0x30, payload, 8);
        self.put(payload + 0x28, size as u64, 4);
        self.put(INVENTORY + 0x380 + 8 * slot as u64, item, 8);
        self.bags.insert(slot, (item, definition, payload));
    }
    /// `count` positions of which the first `used` hold an item pointer.
    fn positions(&mut self, reserved: u32, count: u32, used: u32) {
        self.put(INVENTORY + 0xc8, ARRAY, 8);
        self.put(INVENTORY + 0xd0, reserved as u64, 4);
        self.put(INVENTORY + 0xd4, count as u64, 4);
        for index in 0..used as u64 {
            // Spread them, as a real inventory is: every position whose index is not a hole.
            self.put(ARRAY + 8 * (index * count as u64 / used.max(1) as u64), HEAP + 8, 8);
        }
    }
}
impl Memory for Fixture {
    fn read_exact(&mut self, address: u64, out: &mut [u8]) -> Result<(), ReadError> {
        if self.fail == Some(address) {
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
        for (i, byte) in out.iter_mut().enumerate() {
            *byte = *self.bytes.get(&(address + i as u64)).unwrap_or(&0);
        }
        Ok(())
    }
}

/// A certified image, a whole route, the given bags and `used` of `count` positions.
fn fixture_at(remainder: u64, sizes: &[Option<u32>], count: u32, used: u32) -> Fixture {
    let mut m = Fixture {
        heap: HEAP + 0x100,
        remainder,
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
        (CHARACTER + 8, BASE + CHARACTER_AGENT_VTABLE),
        (CHARACTER + 0x3f0, INVENTORY),
        (INVENTORY, BASE + INVENTORY_VTABLE),
        (INVENTORY + 0x70, CHARACTER),
    ] {
        m.put(address, value, 8);
    }
    m.put(CHARACTER + 0x178, 0x10, 4);
    m.put(INVENTORY + 0x440, sizes.len() as u64, 4);
    for (slot, size) in sizes.iter().enumerate() {
        if let Some(size) = size {
            m.bag(slot, *size);
        }
    }
    m.positions(640, count, used);
    m
}
fn fixture(sizes: &[u32], count: u32, used: u32) -> Fixture {
    let sizes: Vec<_> = sizes.iter().copied().map(Some).collect();
    fixture_at(4, &sizes, count, used)
}
fn small() -> Fixture {
    fixture(&[20, 32, 32, 28], 128, 40)
}
fn profile() -> BuildProfile {
    BuildProfile::checked(BUILD_SHA256, BASE, IMAGE).unwrap()
}
/// Guards and one whole pass through the same budgeted reader, as the addon's first cycle does.
fn run(m: Fixture) -> (Result<BagSlots, Uncovered>, usize) {
    let mut reader = Reader::bounded(m, MAX_BYTES);
    let result = BagProfile::verified_against(&mut reader, profile(), &synthetic_guards())
        .and_then(|verified| bag_slots(&mut reader, &verified, CTX))
        .map(|(slots, _owner)| slots);
    (result, reader.bytes)
}
fn read(m: Fixture) -> Result<BagSlots, Uncovered> {
    run(m).0
}
fn changed(mutate: impl Fn(&mut Fixture)) -> Result<BagSlots, Uncovered> {
    let mut m = small();
    mutate(&mut m);
    read(m)
}

#[test]
fn live_shape_of_8_october_gives_414_capacity_and_101_free() {
    let (result, bytes) = run(fixture(&LIVE_SIZES, 512, 313));
    assert_eq!(
        result,
        Ok(BagSlots {
            capacity: 414,
            occupied: 313,
            free: 101,
            bag_slots: 16,
            bags: 16
        })
    );
    // 883 of guards, the route, 16 bags, 4096 of positions and the rereads.
    assert!(bytes <= MAX_BYTES, "{bytes}");
}

#[test]
fn the_largest_certified_pass_costs_what_the_budget_comment_says() {
    let (result, bytes) = run(fixture(&[32; 16], 640, 0));
    assert_eq!(result.map(|slots| (slots.capacity, slots.free)), Ok((512, 512)));
    // Guards, route, 16 bags, the 640-position array and every reread, the array included.
    assert_eq!(bytes, 883 + 272 + 576 + 16 + 5120 + 208 + 5120);
    assert_eq!(bytes, 12195);
    assert!(bytes <= MAX_BYTES);
}

#[test]
fn production_constants_are_the_audited_profile() {
    let audited: serde_json::Value =
        serde_json::from_str(include_str!("fixtures/bag_capacity_profile.json")).unwrap();
    for (name, value) in [
        ("char_context_vtable", CHAR_CONTEXT_VTABLE),
        ("character_agent_vtable", CHARACTER_AGENT_VTABLE),
        ("inventory_vtable", INVENTORY_VTABLE),
        ("bag_item_vtable", BAG_ITEM_VTABLE),
        ("bags_max", BAGS as u64),
        ("bag_size_max", BAG_SIZE_MAX as u64),
        ("content_pointer_remainder", tyrian_companion_nexus_core::passive::CONTENT_REMAINDER),
    ] {
        assert_eq!(audited[name].as_u64(), Some(value), "{name}");
    }
    assert_eq!(audited["binary_sha256"].as_str(), Some(BUILD_SHA256));
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
        let mut reader = Reader::bounded(small(), MAX_BYTES);
        BagProfile::verified_against(&mut reader, profile(), guards).map(|_| reader.bytes)
    };
    let synthetic = synthetic_guards();
    assert_eq!(verify(&synthetic), Ok(883));
    // No guard, one guard short, or a range that is not the audited one: nothing is read.
    assert_eq!(verify(&[]), Err(Uncovered::Guard));
    assert_eq!(verify(&synthetic[..11]), Err(Uncovered::Guard));
    let mut moved = synthetic.clone();
    moved[7].rva += 16;
    assert_eq!(verify(&moved), Err(Uncovered::Guard));
    let mut resized = synthetic;
    resized[7].size -= 1;
    assert_eq!(verify(&resized), Err(Uncovered::Guard));
}

#[test]
fn empty_and_locked_bag_slots_add_nothing() {
    let m = fixture_at(4, &[Some(20), None, Some(32), None, None], 64, 10);
    let slots = read(m).unwrap();
    assert_eq!((slots.capacity, slots.bag_slots, slots.bags, slots.free), (52, 5, 2, 42));
    // A bag beyond the unlocked slot count is not the character's capacity.
    let mut m = small();
    m.put(INVENTORY + 0x440, 2, 4);
    assert_eq!(read(m).map(|slots| slots.capacity), Ok(52));
}

#[test]
fn no_bags_is_a_supported_zero_and_not_missing_coverage() {
    let slots = read(fixture_at(4, &[None, None], 0, 0)).unwrap();
    assert_eq!((slots.capacity, slots.occupied, slots.free), (0, 0, 0));
}

#[test]
fn identities_on_the_route_reject() {
    for (address, expected) in [
        (CHAR_CONTEXT, Uncovered::Profile),
        (CHARACTER + 8, Uncovered::Profile),
        (INVENTORY, Uncovered::Profile),
        (BASE + INVENTORY_VTABLE + 0x1f0, Uncovered::Profile),
        (BASE + INVENTORY_VTABLE + 0x1e8, Uncovered::Profile),
        (BASE + INVENTORY_VTABLE + 0x1e0, Uncovered::Profile),
        (BASE + BAG_ITEM_VTABLE + 8, Uncovered::Profile),
    ] {
        let result = changed(|m| m.put(address, BASE + 0x123400, 8));
        assert_eq!(result, Err(expected), "{address:x}");
    }
    assert_eq!(changed(|m| m.put(CHARACTER + 0x178, 0, 4)), Err(Uncovered::Root));
    assert_eq!(changed(|m| m.put(INVENTORY + 0x70, CHARACTER + 0x1000, 8)), Err(Uncovered::Profile));
    assert_eq!(changed(|m| m.put(CHARACTER + 0x3f0, 0, 8)), Err(Uncovered::Root));
}

#[test]
fn bag_class_definition_type_and_size_reject() {
    // The class of a consumable, although its definition getter dispatches alike.
    let consumable = changed(|m| {
        let item = m.bags[&1].0;
        m.put(BASE + CONSUMABLE_VTABLE + 8, BASE + 0x13c3e10, 8);
        m.put(item, BASE + CONSUMABLE_VTABLE, 8);
    });
    assert_eq!(consumable, Err(Uncovered::Profile));
    assert_eq!(changed(|m| m.put(m.bags[&1].1 + 0x2c, 5, 4)), Err(Uncovered::Profile));
    assert_eq!(changed(|m| m.put(m.bags[&1].2 + 0x28, 33, 4)), Err(Uncovered::Bounds));
    // A bag of size 0 is not something the counter can have added; 1 and 32 are the edges.
    assert_eq!(changed(|m| m.put(m.bags[&1].2 + 0x28, 0, 4)), Err(Uncovered::Bounds));
    assert_eq!(changed(|m| m.put(m.bags[&1].2 + 0x28, 1, 4)).map(|s| s.capacity), Ok(81));
    assert_eq!(changed(|m| m.put(m.bags[&0].2 + 0x28, 32, 4)).map(|s| s.capacity), Ok(124));
    assert_eq!(changed(|m| m.put(INVENTORY + 0x440, 17, 4)), Err(Uncovered::Bounds));
}

#[test]
fn content_pointers_off_their_observed_alignment_reject() {
    for remainder in [0, 1, 2, 6] {
        let sizes = [Some(20), Some(32)];
        assert_eq!(
            read(fixture_at(remainder, &sizes, 64, 3)),
            Err(Uncovered::Alignment),
            "{remainder}"
        );
    }
    // The payload is checked on its own, with valid bytes waiting at the moved address.
    let payload = changed(|m| {
        let (_, definition, payload) = m.bags[&2];
        m.put(payload + 4 + 0x28, 20, 4);
        m.put(definition + 0x30, payload + 4, 8);
    });
    assert_eq!(payload, Err(Uncovered::Alignment));
}

#[test]
fn heap_pointers_keep_eight_byte_alignment() {
    let item = changed(|m| {
        let (item, definition, _) = m.bags[&1];
        m.put(item + 4, BASE + BAG_ITEM_VTABLE, 8);
        m.put(item + 4 + 0x40, definition, 8);
        m.put(INVENTORY + 0x388, item + 4, 8);
    });
    assert_eq!(item, Err(Uncovered::Alignment));
    assert_eq!(changed(|m| m.put(INVENTORY + 0xc8, ARRAY + 4, 8)), Err(Uncovered::Alignment));
    assert_eq!(changed(|m| m.put(CTX + 0x98, CHAR_CONTEXT + 4, 8)), Err(Uncovered::Alignment));
}

#[test]
fn null_and_out_of_range_pointers_reject() {
    assert_eq!(changed(|m| m.put(m.bags[&1].0 + 0x40, 0, 8)), Err(Uncovered::Bounds));
    assert_eq!(changed(|m| m.put(m.bags[&1].1 + 0x30, 0, 8)), Err(Uncovered::Bounds));
    assert_eq!(changed(|m| m.put(INVENTORY + 0x388, 1 << 60, 8)), Err(Uncovered::Bounds));
    // A count without an array is a header that disagrees with itself, as in `inventory`.
    // It is not `Root`, which says there is no character to read.
    assert_eq!(changed(|m| m.put(INVENTORY + 0xc8, 0, 8)), Err(Uncovered::Bounds));
    assert_eq!(changed(|m| m.put(CTX + 0x98, 0, 8)), Err(Uncovered::Root));
}

#[test]
fn position_array_bounds_and_overfull_inventory_reject() {
    assert_eq!(changed(|m| m.put(INVENTORY + 0xd4, 641, 4)), Err(Uncovered::Bounds));
    assert_eq!(changed(|m| m.put(INVENTORY + 0xd0, 641, 4)), Err(Uncovered::Bounds));
    // More positions in use than the bags hold: the client's unsigned subtraction would wrap.
    assert_eq!(read(fixture(&[20], 128, 21)), Err(Uncovered::Integrity));
    assert_eq!(read(fixture(&[20], 128, 20)).map(|slots| slots.free), Ok(0));
}

#[test]
fn changes_between_the_two_passes_reject() {
    // One case per reread of the consistency pass, in its order.
    for (target, value, size) in [
        (CHAR_CONTEXT, BASE + 0x123400, 8usize),
        (CHAR_CONTEXT + 0x98, CHARACTER + 0x1000, 8),
        (CHARACTER + 8, BASE + 0x123400, 8),
        (CHARACTER + 0x178, 0, 4),
        (CHARACTER + 0x3f0, INVENTORY + 0x1000, 8),
        (INVENTORY, BASE + 0x123400, 8),
        (INVENTORY + 0x70, CHARACTER + 0x1000, 8),
        (INVENTORY + 0x440, 3, 4),
        (INVENTORY + 0x388, 0, 8),
        (INVENTORY + 0xd0, 512, 4),
        (INVENTORY + 0xd4, 127, 4),
        (INVENTORY + 0xc8, ARRAY + 0x1000, 8),
    ] {
        let mut m = small();
        // The second read of the context pointer is the first reread of the consistency pass.
        m.race = Some((CTX + 0x98, 2, vec![(target, value, size)]));
        assert_eq!(read(m), Err(Uncovered::Changed), "{target:x}");
    }
    let mut m = small();
    m.race = Some((CTX + 0x98, 2, vec![(CTX + 0x98, CHAR_CONTEXT + 0x1000, 8)]));
    assert_eq!(read(m), Err(Uncovered::Changed));
}

#[test]
fn an_item_that_moves_while_the_array_is_copied_is_a_change_not_a_count() {
    // 640 positions take two copies, 512 and 128 entries. One item sits at index 512. After
    // the first copy and before the second it moves to 511: each copy on its own shows no
    // item at all, and without a second look that would read as 0 used and 512 free.
    let mut m = fixture(&[32; 16], 640, 0);
    m.put(ARRAY + 8 * 512, HEAP + 8, 8);
    assert_eq!(read(m.clone()).map(|slots| (slots.occupied, slots.free)), Ok((1, 511)));
    m.race = Some((
        ARRAY + 4096,
        1,
        vec![(ARRAY + 8 * 511, HEAP + 8, 8), (ARRAY + 8 * 512, 0, 8)],
    ));
    assert_eq!(read(m), Err(Uncovered::Changed));

    // An entry that changes after the copy and before the reread: the old count is not kept.
    let mut m = small();
    m.race = Some((CTX + 0x98, 2, vec![(ARRAY + 8 * 127, HEAP + 8, 8)]));
    assert_eq!(read(m), Err(Uncovered::Changed));
    let mut m = small();
    m.race = Some((CTX + 0x98, 2, vec![(ARRAY, 0, 8)]));
    assert_eq!(read(m), Err(Uncovered::Changed));
}

#[test]
fn failed_read_and_exhausted_budget_never_become_a_value() {
    let mut m = small();
    m.fail = Some(m.bags[&2].2 + 0x28);
    assert_eq!(read(m), Err(Uncovered::ReadFailed));
    let mut reader = Reader::bounded(small(), 883 + 200);
    let verified =
        BagProfile::verified_against(&mut reader, profile(), &synthetic_guards()).unwrap();
    assert_eq!(bag_slots(&mut reader, &verified, CTX).map(|_| ()), Err(Uncovered::Bounds));
    // The budget is charged before the request: the refused read is not counted.
    assert!(reader.bytes <= 883 + 200);
}

#[test]
fn a_changed_guard_byte_stops_the_cycle_at_that_guard() {
    let mut m = small();
    m.bytes.entry(BASE + GUARDS[9].rva).and_modify(|byte| *byte ^= 1);
    let (result, bytes) = run(m);
    assert_eq!(result, Err(Uncovered::Guard));
    // Ten guards were copied, the tenth differed, and nothing after it was requested: not the
    // last two guards, and no field of the game.
    assert_eq!(bytes, GUARDS[..10].iter().map(|guard| guard.size).sum::<usize>());
    // The real digests do not accept the fixtures' synthetic contents, and stop at the first.
    let mut reader = Reader::bounded(small(), MAX_BYTES);
    assert_eq!(
        BagProfile::verified(&mut reader, profile()).map(|_| ()),
        Err(Uncovered::Guard)
    );
    assert_eq!(reader.bytes, GUARDS[0].size);
    assert_eq!(GUARDS.iter().map(|guard| guard.size).sum::<usize>(), 883);
}

#[test]
fn misaligned_or_low_context_rejects_before_reading() {
    let mut reader = Reader::bounded(small(), MAX_BYTES);
    let verified =
        BagProfile::verified_against(&mut reader, profile(), &synthetic_guards()).unwrap();
    let before = reader.bytes;
    assert_eq!(bag_slots(&mut reader, &verified, CTX + 4).map(|_| ()), Err(Uncovered::Bounds));
    assert_eq!(bag_slots(&mut reader, &verified, 0x100).map(|_| ()), Err(Uncovered::Bounds));
    assert_eq!(reader.bytes, before);
}
