//! Sparse wallet fixtures over owned bytes; never open a real process.
//!
//! `fixtures/wallet_profile.json` is a verbatim copy of the audited profile in
//! `tyrian-companion/docs/audit/loot-wallet-probe/profile.json` (commit f821362): its guard bytes
//! are what the certified executable holds at those RVAs. Its `scope` text predates the live
//! comparison of 2026-10-07, which found the native keys to be the public currency IDs.
use serde_json::Value;
use std::collections::BTreeMap;
use std::ops::Range;
use std::sync::LazyLock;
use tyrian_companion_nexus_core::inventory::{
    BuildProfile, Memory, ReadError, Reader, BUILD_SHA256,
};
use tyrian_companion_nexus_core::sha256::{hex, sha256};
use tyrian_companion_nexus_core::wallet::*;
const BASE: u64 = 0x140000000;
const IMAGE: u64 = 0x2c48000;
const CTX: u64 = 0x200000;
const CHAR_CONTEXT: u64 = 0x210000;
const CHARACTER: u64 = 0x220000;
const MANAGER: u64 = CHARACTER + 0x1878;
const ENTRIES: u64 = 0x240000;
const HEAP_END: u64 = 0x260000;
/// Guards, eight route reads, header; then five owner/vtable rereads and the header again.
const GUARD_BYTES: usize = 8 + 8 + 80 + 296 + 1024;
const ROUTE_BYTES: usize = 8 * 8 + 16;
const REREAD_BYTES: usize = 5 * 8 + 16;
/// The 55 native keys of the live comparison, which were the account's 55 public currency IDs.
const LIVE_IDS: [u32; 55] = [
    1, 2, 3, 4, 7, 15, 16, 18, 19, 20, 22, 23, 24, 25, 26, 27, 28, 29, 32, 34, 35, 37, 38, 41, 42,
    43, 44, 45, 47, 49, 50, 51, 54, 58, 60, 61, 62, 63, 65, 66, 67, 68, 69, 70, 71, 72, 73, 75, 76,
    78, 79, 80, 81, 82, 83,
];
static AUDITED: LazyLock<Value> =
    LazyLock::new(|| serde_json::from_str(include_str!("fixtures/wallet_profile.json")).unwrap());
static TABLE: LazyLock<[u32; 256]> = LazyLock::new(|| {
    let bytes = unhex(AUDITED["guards"][4]["hex"].as_str().unwrap());
    let mut table = [0; 256];
    for (word, raw) in table.iter_mut().zip(bytes.chunks_exact(4)) {
        *word = u32::from_le_bytes(raw.try_into().unwrap());
    }
    table
});
fn unhex(text: &str) -> Vec<u8> {
    (0..text.len())
        .step_by(2)
        .map(|at| u8::from_str_radix(&text[at..at + 2], 16).unwrap())
        .collect()
}
fn hash(key: u32) -> u32 {
    currency_hash(key, &TABLE)
}
#[derive(Default, Clone)]
struct Fixture {
    bytes: BTreeMap<u64, u8>,
    capacity: u32,
    forbidden: Vec<Range<u64>>,
    fail: Option<u64>,
    /// On the given visit of a read starting at `.0`, write `.3` (`.4` bytes) at `.2` first.
    race: Option<(u64, usize, u64, u64, usize)>,
    visits: usize,
}
impl Fixture {
    fn put(&mut self, address: u64, value: u64, size: usize) {
        for (i, byte) in value.to_le_bytes()[..size].iter().enumerate() {
            self.bytes.insert(address + i as u64, *byte);
        }
    }
    fn header(&mut self, capacity: u32, count: u32, entries: u64) {
        self.put(MANAGER + 8, capacity as u64, 4);
        self.put(MANAGER + 12, count as u64, 4);
        self.put(MANAGER + 16, entries, 8);
    }
    fn bucket(&mut self, index: u32, key: u32, balance: u32, occupied_hash: u32) {
        let address = ENTRIES + index as u64 * 12;
        self.put(address, key as u64, 4);
        self.put(address + 4, balance as u64, 4);
        self.put(address + 8, occupied_hash as u64, 4);
    }
    fn occupied(&self, index: u32) -> bool {
        let address = ENTRIES + index as u64 * 12 + 8;
        (0..4).any(|i| {
            self.bytes
                .get(&(address + i))
                .is_some_and(|byte| *byte != 0)
        })
    }
    /// Store a key the way the game's map does: at the first free bucket from its home.
    fn insert(&mut self, key: u32, balance: u32) -> u32 {
        let mask = self.capacity - 1;
        let mut index = hash(key) & mask;
        while self.occupied(index) {
            index = (index + 1) & mask;
        }
        self.bucket(index, key, balance, hash(key));
        index
    }
}
impl Memory for Fixture {
    fn read_exact(&mut self, address: u64, out: &mut [u8]) -> Result<(), ReadError> {
        let end = address + out.len() as u64;
        assert!(
            !self
                .forbidden
                .iter()
                .any(|range| address < range.end && range.start < end),
            "forbidden read {address:x}"
        );
        let mapped = |range: Range<u64>| range.start <= address && end <= range.end;
        if self.fail == Some(address) || !(mapped(CTX..HEAP_END) || mapped(BASE..BASE + IMAGE)) {
            return Err(ReadError::ReadFailed);
        }
        if let Some((trigger, visit, target, value, size)) = self.race {
            if address == trigger {
                self.visits += 1;
                if self.visits == visit {
                    self.put(target, value, size);
                }
            }
        }
        for (i, byte) in out.iter_mut().enumerate() {
            *byte = *self.bytes.get(&(address + i as u64)).unwrap_or(&0);
        }
        Ok(())
    }
}
fn profile() -> BuildProfile {
    BuildProfile::checked(BUILD_SHA256, BASE, IMAGE).unwrap()
}
/// A certified image, a whole route and a map holding exactly `balances`.
fn fixture(capacity: u32, balances: &[(u32, u32)]) -> Fixture {
    let mut m = Fixture {
        capacity,
        ..Default::default()
    };
    for guard in AUDITED["guards"].as_array().unwrap() {
        let address = BASE + guard["rva"].as_u64().unwrap();
        for (i, byte) in unhex(guard["hex"].as_str().unwrap()).iter().enumerate() {
            m.bytes.insert(address + i as u64, *byte);
        }
    }
    for (address, value) in [
        (CTX + 0x98, CHAR_CONTEXT),
        (CHAR_CONTEXT, BASE + CHAR_CONTEXT_VTABLE),
        (
            BASE + CHAR_CONTEXT_VTABLE + 0x70,
            BASE + CHAR_CONTEXT_GETTER,
        ),
        (CHAR_CONTEXT + 0xa0, CHARACTER),
        (CHARACTER, BASE + CHARACTER_VTABLE),
        (
            BASE + CHARACTER_VTABLE + 0x250,
            BASE + CHARACTER_WALLET_GETTER,
        ),
        (MANAGER, BASE + CURRENCY_MANAGER_VTABLE),
        (BASE + CURRENCY_MANAGER_VTABLE, BASE + BALANCE_GETTER),
    ] {
        m.put(address, value, 8);
    }
    for (key, balance) in balances {
        m.insert(*key, *balance);
    }
    m.header(capacity, balances.len() as u32, ENTRIES);
    m
}
/// The addon's order: verify the static guards, then take one sample with the same reader.
fn read(m: Fixture) -> Result<WalletSnapshot, WalletError> {
    let mut r = Reader::bounded(m, MAX_BYTES);
    let verified = WalletProfile::verified(&mut r, profile())?;
    wallet_snapshot(&mut r, &verified, CTX)
}

#[test]
fn constants_guard_digests_and_key_hashes_match_the_audited_profile() {
    assert_eq!(AUDITED["binary_sha256"].as_str(), Some(BUILD_SHA256));
    assert_eq!(AUDITED["image_size"].as_u64(), Some(IMAGE));
    for (name, value) in [
        ("char_context_vtable", CHAR_CONTEXT_VTABLE),
        ("char_context_getter", CHAR_CONTEXT_GETTER),
        ("character_vtable", CHARACTER_VTABLE),
        ("character_wallet_getter", CHARACTER_WALLET_GETTER),
        ("currency_manager_vtable", CURRENCY_MANAGER_VTABLE),
        ("balance_getter", BALANCE_GETTER),
        ("currency_id", PROFILE_KEY as u64),
        ("currency_hash", PROFILE_KEY_HASH as u64),
    ] {
        assert_eq!(AUDITED[name].as_u64(), Some(value), "{name}");
    }
    let audited = AUDITED["guards"].as_array().unwrap();
    assert_eq!(audited.len(), GUARDS.len());
    let mut total = 0;
    for (expected, guard) in audited.iter().zip(GUARDS) {
        assert_eq!(expected["name"].as_str(), Some(guard.name));
        assert_eq!(expected["rva"].as_u64(), Some(guard.rva), "{}", guard.name);
        assert_eq!(expected["size"].as_u64(), Some(guard.size as u64));
        assert_eq!(expected["sha256"].as_str(), Some(guard.sha256));
        let bytes = unhex(expected["hex"].as_str().unwrap());
        assert_eq!(bytes.len(), guard.size);
        assert_eq!(hex(&sha256(&bytes)), guard.sha256, "{}", guard.name);
        total += guard.size;
    }
    assert_eq!(total, GUARD_BYTES);
    // `probe.currency_hash` of the audited Python reference, over the same table.
    for (key, expected) in [
        (1, 4220830176u32),
        (2, 2864495157),
        (4, 3238298567),
        (23, 2100370792),
        (45, 3235534038),
        (63, 60586174),
        (83, 1332010356),
        (255, 1944935022),
        (256, 2432222870),
        (65536, 3359162613),
        (0x01020304, 917780940),
        (i32::MAX as u32, 3066445365),
    ] {
        assert_eq!(hash(key), expected, "{key}");
    }
}

#[test]
fn live_shaped_wallet_lists_every_key_with_its_balance_inside_its_own_budget() {
    let expected: BTreeMap<u32, u32> = LIVE_IDS
        .iter()
        .map(|id| (*id, if *id == 45 { 10225 } else { id * 1000 + 7 }))
        .collect();
    let mut m = fixture(128, &[]);
    let displaced = expected
        .iter()
        .filter(|(key, balance)| m.insert(**key, **balance) != hash(**key) & 127)
        .count();
    // The live key set collides in 128 buckets, so this walks real probe chains.
    assert!(displaced >= 5, "{displaced}");
    m.header(128, 55, ENTRIES);
    // The controlled-inventory wrapper at +0x98 is another object: never part of this route.
    m.forbidden = vec![CHAR_CONTEXT + 0x98..CHAR_CONTEXT + 0xa0];
    let mut r = Reader::bounded(m, MAX_BYTES);
    let verified = WalletProfile::verified(&mut r, profile()).unwrap();
    assert_eq!((r.bytes, r.reads), (GUARD_BYTES, 5));
    let s = wallet_snapshot(&mut r, &verified, CTX).unwrap();
    assert_eq!(s.balances(), &expected);
    assert_eq!(s.owner(), (CHAR_CONTEXT, CHARACTER));
    assert_eq!(r.bytes, GUARD_BYTES + ROUTE_BYTES + 128 * 12 + REREAD_BYTES);
    assert_eq!(r.reads, 5 + 9 + 1 + 6);
}

#[test]
fn present_key_with_zero_balance_is_a_covered_zero_and_an_absent_key_is_not_listed() {
    let s = read(fixture(64, &[(45, 0), (1, 12)])).unwrap();
    assert_eq!(s.balances(), &BTreeMap::from([(1, 12), (45, 0)]));
    assert!(!s.balances().contains_key(&2));
}

#[test]
fn empty_or_absent_map_is_missing_coverage_never_zero_balances() {
    let mut empty = fixture(128, &[]);
    empty.forbidden = vec![ENTRIES..HEAP_END];
    assert_eq!(read(empty), Err(WalletError::Empty));
    let mut absent = fixture(128, &[]);
    absent.header(0, 0, 0);
    assert_eq!(read(absent), Err(WalletError::Empty));
    for balances in [
        BTreeMap::new(),
        BTreeMap::from([(0, 1)]),
        BTreeMap::from([(i32::MAX as u32 + 1, 1)]),
        BTreeMap::from([(1, i32::MAX as u32 + 1)]),
        (1..=MAX_CURRENCIES as u32 + 1).map(|id| (id, 0)).collect(),
    ] {
        assert_eq!(WalletSnapshot::checked((1, 2), balances), None);
    }
}

#[test]
fn wrong_currency_manager_vtable_has_no_coverage() {
    let mut m = fixture(64, &[(45, 100)]);
    m.put(MANAGER, BASE + 0x123400, 8);
    assert_eq!(read(m), Err(WalletError::Profile));
}

#[test]
fn every_wrong_vtable_or_getter_rejects_before_the_header_or_any_bucket_is_read() {
    for address in [
        CHAR_CONTEXT,
        BASE + CHAR_CONTEXT_VTABLE + 0x70,
        CHARACTER,
        BASE + CHARACTER_VTABLE + 0x250,
        MANAGER,
        BASE + CURRENCY_MANAGER_VTABLE,
    ] {
        let mut m = fixture(64, &[(45, 100)]);
        m.put(address, BASE + 0x123400, 8);
        m.forbidden = vec![MANAGER + 8..MANAGER + 24, ENTRIES..HEAP_END];
        assert_eq!(read(m), Err(WalletError::Profile), "{address:x}");
    }
}

#[test]
fn any_changed_static_guard_byte_leaves_the_wallet_unverified_and_unread() {
    for guard in GUARDS {
        for offset in [0, guard.size as u64 - 1] {
            let mut m = fixture(64, &[(45, 100)]);
            let address = BASE + guard.rva + offset;
            let byte = m.bytes[&address];
            m.put(address, (byte ^ 1) as u64, 1);
            m.forbidden = vec![CTX..HEAP_END];
            assert_eq!(read(m), Err(WalletError::Guard), "{}", guard.name);
        }
    }
    // A guard that could not be copied is a failed read, not proof of another build.
    let mut m = fixture(64, &[(45, 100)]);
    m.fail = Some(BASE + GUARDS[4].rva);
    m.forbidden = vec![CTX..HEAP_END];
    assert_eq!(read(m), Err(WalletError::ReadFailed));
}

#[test]
fn malformed_header_is_out_of_bounds_and_reads_no_bucket() {
    for (capacity, count, entries) in [
        (3, 1, ENTRIES),
        (0, 1, ENTRIES),
        (0, 0, ENTRIES),
        (8192, 1, ENTRIES),
        (64, 65, ENTRIES),
        (4096, MAX_CURRENCIES as u32 + 1, ENTRIES),
        (64, 1, 0),
        (64, 1, ENTRIES + 4),
        (64, 1, 0x8000),
        (64, 1, 0x0000_8000_0000_0000),
    ] {
        let mut m = fixture(64, &[(45, 100)]);
        m.header(capacity, count, entries);
        m.forbidden = vec![ENTRIES..HEAP_END];
        assert_eq!(
            read(m),
            Err(WalletError::Bounds),
            "{capacity} {count} {entries:x}"
        );
    }
}

#[test]
fn null_or_invalid_owner_pointers_and_context_fail_closed() {
    for (address, value, expected) in [
        (CTX + 0x98, 0, WalletError::Root),
        (CHAR_CONTEXT + 0xa0, 0, WalletError::Root),
        (CTX + 0x98, CHAR_CONTEXT + 4, WalletError::Bounds),
        (CHAR_CONTEXT + 0xa0, CHARACTER + 4, WalletError::Bounds),
        (
            CHAR_CONTEXT + 0xa0,
            0xdead_0000_0000_0000,
            WalletError::Bounds,
        ),
    ] {
        let mut m = fixture(64, &[(45, 100)]);
        m.put(address, value, 8);
        m.forbidden = vec![ENTRIES..HEAP_END];
        assert_eq!(read(m), Err(expected), "{address:x} {value:x}");
    }
    let mut r = Reader::bounded(fixture(64, &[(45, 100)]), MAX_BYTES);
    let verified = WalletProfile::verified(&mut r, profile()).unwrap();
    for context in [0, CTX + 4, u64::MAX] {
        assert_eq!(
            wallet_snapshot(&mut r, &verified, context),
            Err(WalletError::Bounds)
        );
    }
    assert_eq!(r.reads, 5);
}

#[test]
fn failed_copy_of_the_route_the_header_or_any_table_part_gives_no_wallet() {
    // 512 buckets are 6144 bytes: two whole-bucket reads, at ENTRIES and ENTRIES + 4092.
    for address in [
        CTX + 0x98,
        CHAR_CONTEXT,
        MANAGER,
        MANAGER + 8,
        ENTRIES,
        ENTRIES + 4092,
    ] {
        let mut m = fixture(512, &[(45, 100), (1, 2)]);
        m.fail = Some(address);
        assert_eq!(read(m), Err(WalletError::ReadFailed), "{address:x}");
    }
    let mut unmapped = fixture(64, &[(45, 100)]);
    unmapped.header(64, 1, 0x7000_0000);
    assert_eq!(read(unmapped), Err(WalletError::ReadFailed));
}

#[test]
fn bucket_whose_hash_is_not_its_keys_hash_rejects_the_whole_wallet() {
    for (key, occupied_hash) in [(1, hash(2)), (3, hash(1)), (1, hash(1) ^ 0x100)] {
        let mut m = fixture(64, &[(45, 100), (2, 3)]);
        let index = m.insert(1, 2);
        m.bucket(index, key, 2, occupied_hash);
        m.header(64, 3, ENTRIES);
        assert_eq!(read(m), Err(WalletError::Integrity), "{key}");
    }
}

#[test]
fn key_the_games_linear_probing_cannot_reach_rejects_the_whole_wallet() {
    let home = hash(1) & 63;
    // Two buckets after its free home, and one bucket before it: the lookup stops earlier.
    for index in [(home + 2) & 63, home.wrapping_sub(1) & 63] {
        let mut m = fixture(64, &[(45, 100)]);
        assert!(!m.occupied(index) && !m.occupied(home));
        m.bucket(index, 1, 2, hash(1));
        m.header(64, 2, ENTRIES);
        assert_eq!(read(m), Err(WalletError::Integrity), "{index}");
    }
    let mut m = fixture(64, &[(45, 100)]);
    m.bucket(home, 1, 2, hash(1));
    m.header(64, 2, ENTRIES);
    assert_eq!(read(m).unwrap().balances().len(), 2);
}

#[test]
fn probe_chain_wrapping_past_the_last_bucket_and_a_full_table_are_reachable() {
    // In eight buckets, keys 4, 27, 76 and 79 all start at the last one.
    let mut m = fixture(8, &[(4, 1), (27, 2), (76, 3), (79, 4)]);
    assert!([7, 0, 1, 2].into_iter().all(|index| m.occupied(index)));
    assert_eq!(read(m.clone()).unwrap().balances().len(), 4);
    for key in [1, 2, 3, 5] {
        m.insert(key, key);
    }
    m.header(8, 8, ENTRIES);
    let s = read(m).unwrap();
    assert_eq!(s.balances().len(), 8);
    assert_eq!(s.balances()[&79], 4);
}

#[test]
fn occupied_buckets_must_equal_the_header_count_and_keys_cannot_repeat() {
    for count in [1, 3] {
        let mut m = fixture(64, &[(45, 100), (1, 2)]);
        m.header(64, count, ENTRIES);
        assert_eq!(read(m), Err(WalletError::Integrity), "{count}");
    }
    let mut m = fixture(64, &[(45, 100)]);
    m.insert(45, 7);
    m.header(64, 2, ENTRIES);
    assert_eq!(read(m), Err(WalletError::Integrity));
}

#[test]
fn key_or_balance_outside_the_wire_range_rejects_without_clamping() {
    for (key, balance) in [
        (45, i32::MAX as u32 + 1),
        (45, u32::MAX),
        (0, 5),
        (i32::MAX as u32 + 1, 5),
        (u32::MAX, 5),
    ] {
        let mut m = fixture(64, &[(1, 2)]);
        m.insert(key, balance);
        m.header(64, 2, ENTRIES);
        assert_eq!(read(m), Err(WalletError::Range), "{key} {balance}");
    }
    let s = read(fixture(64, &[(i32::MAX as u32, i32::MAX as u32)])).unwrap();
    assert_eq!(s.balances()[&(i32::MAX as u32)], i32::MAX as u32);
}

#[test]
fn owner_vtable_or_header_change_during_the_copy_is_concurrent_and_yields_nothing() {
    let header = |capacity: u64, count: u64| capacity | count << 32;
    for (trigger, target, value) in [
        (CTX + 0x98, CTX + 0x98, CHAR_CONTEXT + 0x1000),
        (CHAR_CONTEXT + 0xa0, CHAR_CONTEXT + 0xa0, CHARACTER + 0x1000),
        (CHAR_CONTEXT, CHAR_CONTEXT, BASE + 0x123400),
        (CHARACTER, CHARACTER, BASE + 0x123400),
        (MANAGER, MANAGER, BASE + 0x123400),
        (MANAGER + 8, MANAGER + 8, header(64, 3)),
        (MANAGER + 8, MANAGER + 8, header(128, 2)),
        (MANAGER + 8, MANAGER + 16, ENTRIES + 0x1000),
    ] {
        let mut m = fixture(64, &[(45, 100), (1, 2)]);
        m.race = Some((trigger, 2, target, value, 8));
        assert_eq!(read(m), Err(WalletError::Changed), "{trigger:x} {target:x}");
    }
    // The same fixture without a second-visit change is a stable sample.
    assert!(read(fixture(64, &[(45, 100), (1, 2)])).is_ok());
}

#[test]
fn largest_listable_wallet_fits_the_wallet_budget_and_a_smaller_budget_fails_closed() {
    let pairs: Vec<(u32, u32)> = (1..=MAX_CURRENCIES as u32).map(|id| (id, id)).collect();
    let m = fixture(MAX_CAPACITY, &pairs);
    let whole = GUARD_BYTES + ROUTE_BYTES + MAX_CAPACITY as usize * 12 + REREAD_BYTES;
    let mut r = Reader::bounded(m.clone(), MAX_BYTES);
    let verified = WalletProfile::verified(&mut r, profile()).unwrap();
    let s = wallet_snapshot(&mut r, &verified, CTX).unwrap();
    assert_eq!(s.balances().len(), MAX_CURRENCIES);
    assert_eq!(r.bytes, whole);
    assert!(whole <= MAX_BYTES);
    // One byte short of the final header reread: nothing is returned from a partial check.
    let mut r = Reader::bounded(m.clone(), whole - 1);
    let verified = WalletProfile::verified(&mut r, profile()).unwrap();
    assert_eq!(
        wallet_snapshot(&mut r, &verified, CTX),
        Err(WalletError::Bounds)
    );
    assert!(r.bytes < whole);
    // A budget that cannot hold the table stops inside it, at a whole-bucket read.
    let mut r = Reader::bounded(m, GUARD_BYTES + ROUTE_BYTES + 4092);
    let verified = WalletProfile::verified(&mut r, profile()).unwrap();
    assert_eq!(
        wallet_snapshot(&mut r, &verified, CTX),
        Err(WalletError::Bounds)
    );
    assert_eq!(r.bytes, GUARD_BYTES + ROUTE_BYTES + 4092);
    // The wallet budget is a share of its own, never a way around the inventory's ceiling.
    let mut r = Reader::bounded(fixture(64, &[(45, 100)]), usize::MAX);
    r.bytes = tyrian_companion_nexus_core::inventory::MAX_BYTES;
    assert_eq!(
        WalletProfile::verified(&mut r, profile()).err(),
        Some(WalletError::Bounds)
    );
}
