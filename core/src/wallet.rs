//! Passive, bounded wallet interpretation for the same certified GW2 build.
//!
//! The route and its RVAs come from the static audit of 2026-10-06
//! (`tyrian-companion/docs/audit/loot-wallet-probe`), and the whole map was compared in a running
//! game on 2026-10-07: its native keys are the public currency IDs and the DWORD after each key
//! is the balance. As in `inventory`, the adapter provides exact reads and this module never
//! dereferences or calls game code; vtables and getters are compared, never invoked.
//!
//! The native map is sparse: a currency the account never held has no key. A missing key is
//! missing coverage, never a zero balance, so nothing here invents one. Any doubt rejects the
//! whole wallet sample, and that rejection never touches the inventory sample of the same cycle.

use crate::inventory::{BuildProfile, Memory, ReadError, Reader, MAX_POINTER, TLS_INDEX_RVA};
use crate::sha256::{hex, sha256};
use std::collections::BTreeMap;

pub const CHAR_CONTEXT_VTABLE: u64 = 0x215cf48;
/// Slot `0x70` of the character context's vtable: `mov rax,[rcx+0xA0]; ret`.
pub const CHAR_CONTEXT_GETTER: u64 = 0x498860;
pub const CHARACTER_VTABLE: u64 = 0x215d958;
/// Slot `0x250` of the wallet character's vtable: `lea rax,[rcx+0x1878]; ret`.
pub const CHARACTER_WALLET_GETTER: u64 = 0x11ba5e0;
pub const CURRENCY_MANAGER_VTABLE: u64 = 0x21720f8;
/// Slot 0 of the currency manager's vtable, the balance getter the wallet list calls by ID.
pub const BALANCE_GETTER: u64 = 0x1271810;

/// The map's capacity must be a power of two in `1..=MAX_CAPACITY`.
pub const MAX_CAPACITY: u32 = 4096;
/// live1 carries at most 4096 rows per sample and the inventory may use 640 of them.
pub const MAX_CURRENCIES: usize = crate::live::MAX_ROWS - crate::inventory::MAX_POSITIONS as usize;
/// The wallet's own one-cycle byte budget, separate from the inventory's: the five guards
/// (1416 bytes, once per verified build), the route, both headers and one whole table
/// (`MAX_CAPACITY` buckets of 12 bytes).
pub const MAX_BYTES: usize = 65_536;
const BUCKET: usize = 12;
/// 341 whole buckets per exact read, so no bucket is ever split across two copies.
const CHUNK: usize = 4092;

/// One fixed code or table range of the executable, identified by the digest of its bytes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Guard {
    pub name: &'static str,
    pub rva: u64,
    pub size: usize,
    pub sha256: &'static str,
}
const HASH_TABLE: &str = "hash_table";
/// The two route getters, the balance getter, the open-addressing lookup and its 256-DWORD table.
pub const GUARDS: [Guard; 5] = [
    Guard {
        name: "local_character",
        rva: CHAR_CONTEXT_GETTER,
        size: 8,
        sha256: "8fd42236905c9ccb06782e7a64df231d2be5f1c373f520fb9ff3a1a087796392",
    },
    Guard {
        name: "wallet_manager",
        rva: CHARACTER_WALLET_GETTER,
        size: 8,
        sha256: "c203d880264ebf1b4f76b9fea85fe724b261855c6a7c80a705f2176e816eea1d",
    },
    Guard {
        name: "balance_getter",
        rva: BALANCE_GETTER,
        size: 80,
        sha256: "a2d9cdc2da040577c5682f45c191f1926eca5a08eb7be0e91e495b351eeca6cb",
    },
    Guard {
        name: "hash_lookup",
        rva: 0x2397f0,
        size: 296,
        sha256: "e329f745caf728032181eb834e5c86915ab1f528b8788c40e5982664d06ffb63",
    },
    Guard {
        name: HASH_TABLE,
        rva: 0x1b8fdb0,
        size: 1024,
        sha256: "7a5e36b78f411ef1d20f94aec5c1e324e5f43b47e1b084ea392d596daa25bed6",
    },
];
/// The audited key (volatile magic) and its hash, checked against the table the guard yields.
pub const PROFILE_KEY: u32 = 45;
pub const PROFILE_KEY_HASH: u32 = 0xc0da_54d6;
// `BuildProfile::checked` only accepts an image that reaches the TLS index, which lies beyond
// every address this module reads inside the executable.
const _: () = assert!(CURRENCY_MANAGER_VTABLE + 8 <= TLS_INDEX_RVA);
const _: () = assert!(CHARACTER_VTABLE + 0x258 <= TLS_INDEX_RVA);
const _: () = assert!(0x1b8fdb0 + 1024 <= TLS_INDEX_RVA);

/// Closed reasons for a wallet without coverage; no address, key or OS error reaches the UI.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WalletError {
    /// A static code/table range differs from the audited bytes of this build.
    Guard,
    /// A vtable or getter on the route is not the certified one.
    Profile,
    /// The route has no character context or wallet character right now.
    Root,
    /// The map header, a pointer or the read budget is outside the certified bounds.
    Bounds,
    /// The map is absent or has no key: nothing is covered, and nothing is zero.
    Empty,
    /// A bucket's hash, its probe position, a repeated key or the occupied count is inconsistent.
    Integrity,
    /// A key or balance does not fit the wire's `1..=i32::MAX` / `0..=i32::MAX`.
    Range,
    /// The owner, a vtable or the header changed while the table was being copied.
    Changed,
    ReadFailed,
}
impl From<ReadError> for WalletError {
    fn from(error: ReadError) -> Self {
        match error {
            ReadError::Bounds => Self::Bounds,
            ReadError::ProfileMismatch | ReadError::UnsupportedBuild => Self::Profile,
            ReadError::RootUnavailable => Self::Root,
            ReadError::Changed => Self::Changed,
            ReadError::ReadFailed => Self::ReadFailed,
        }
    }
}

/// What the last cycle can say about the wallet, for local diagnostics only.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum WalletCoverage {
    /// No wallet read was attempted: no negotiated sample yet, or the inventory route failed.
    #[default]
    NotRead,
    /// This many currencies were read and verified. IDs outside that set are not covered.
    Listed(u32),
    Unavailable(WalletError),
}

/// The balances of every key present in the native map, by public currency ID. Built only from
/// values that fit the wire, and never empty: an empty wallet is missing coverage, not a sample.
/// `owner` stays local; a different owner cannot be compared with the previous balances.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WalletSnapshot {
    owner: (u64, u64),
    balances: BTreeMap<u32, u32>,
}
impl WalletSnapshot {
    /// `None` unless there is at least one balance and every row fits live1.
    pub fn checked(owner: (u64, u64), balances: BTreeMap<u32, u32>) -> Option<Self> {
        let fits = |(id, balance): (&u32, &u32)| {
            (1..=i32::MAX as u32).contains(id) && *balance <= i32::MAX as u32
        };
        (!balances.is_empty() && balances.len() <= MAX_CURRENCIES && balances.iter().all(fits))
            .then_some(Self { owner, balances })
    }
    pub fn owner(&self) -> (u64, u64) {
        self.owner
    }
    pub fn balances(&self) -> &BTreeMap<u32, u32> {
        &self.balances
    }
}

/// The game's own 32-bit key hash, as its byte-verified open-addressing lookup computes it.
/// Zero marks a free bucket, so the lookup replaces a zero result with a fixed constant.
pub fn currency_hash(key: u32, table: &[u32; 256]) -> u32 {
    let mut value =
        (table[(key & 255) as usize] ^ table[50] ^ 0x00c9_747a).wrapping_add(0x325d_1eae);
    for shift in [8, 16, 24] {
        value =
            (table[(value >> 24) as usize] ^ table[((key >> shift) & 255) as usize] ^ (value >> 6))
                .wrapping_add(value);
    }
    if value == 0 {
        0x3ade_68b1
    } else {
        value
    }
}

/// Proof that the five static ranges of a hash-verified build hold the audited bytes, plus the
/// hash table those bytes carry. It can only be built from a [`BuildProfile`], so an
/// unsupported executable never reaches a wallet offset. Verify once per build, not per cycle.
#[derive(Debug, Clone)]
pub struct WalletProfile {
    base: u64,
    table: [u32; 256],
}
impl WalletProfile {
    pub fn verified<M: Memory>(
        r: &mut Reader<M>,
        profile: BuildProfile,
    ) -> Result<Self, WalletError> {
        let mut table = [0u32; 256];
        for guard in &GUARDS {
            let mut bytes = vec![0u8; guard.size];
            r.read_into(profile.base + guard.rva, &mut bytes)?;
            if hex(&sha256(&bytes)) != guard.sha256 {
                return Err(WalletError::Guard);
            }
            if guard.name == HASH_TABLE {
                for (word, raw) in table.iter_mut().zip(bytes.chunks_exact(4)) {
                    *word = u32::from_le_bytes([raw[0], raw[1], raw[2], raw[3]]);
                }
            }
        }
        if currency_hash(PROFILE_KEY, &table) != PROFILE_KEY_HASH {
            return Err(WalletError::Guard);
        }
        Ok(Self {
            base: profile.base,
            table,
        })
    }
}

/// A non-null, aligned object pointer on the route; NULL is absence of the owner, not a wallet.
fn object<M: Memory>(r: &mut Reader<M>, address: u64) -> Result<u64, WalletError> {
    let value = r.pointer(address)?;
    if value == 0 {
        return Err(WalletError::Root);
    }
    if value & 7 != 0 {
        return Err(WalletError::Bounds);
    }
    Ok(value)
}
/// A vtable pointer or vtable slot must be exactly the certified one.
fn identity<M: Memory>(r: &mut Reader<M>, address: u64, expected: u64) -> Result<(), WalletError> {
    if r.scalar(address, 8)? != expected {
        return Err(WalletError::Profile);
    }
    Ok(())
}
fn dword(bytes: &[u8], offset: usize) -> u32 {
    u32::from_le_bytes([
        bytes[offset],
        bytes[offset + 1],
        bytes[offset + 2],
        bytes[offset + 3],
    ])
}

/// Read the whole currency map of the wallet UI's local character (`ChCliContext+0xA0`, never
/// the controlled-inventory wrapper at `+0x98`). Identities are checked before any balance is
/// copied, the table is copied once in whole-bucket reads, and the owner, vtables and header are
/// read again afterwards. Every occupied bucket must carry its key's hash and be reachable by
/// the game's linear probing; one failing bucket rejects all of them.
pub fn wallet_snapshot<M: Memory>(
    r: &mut Reader<M>,
    profile: &WalletProfile,
    context: u64,
) -> Result<WalletSnapshot, WalletError> {
    if !(0x10000..=MAX_POINTER - 0x198).contains(&context) || context & 7 != 0 {
        return Err(WalletError::Bounds);
    }
    let b = profile.base;
    let char_context = object(r, context + 0x98)?;
    identity(r, char_context, b + CHAR_CONTEXT_VTABLE)?;
    identity(r, b + CHAR_CONTEXT_VTABLE + 0x70, b + CHAR_CONTEXT_GETTER)?;
    let character = object(r, char_context + 0xa0)?;
    identity(r, character, b + CHARACTER_VTABLE)?;
    identity(r, b + CHARACTER_VTABLE + 0x250, b + CHARACTER_WALLET_GETTER)?;
    let manager = character + 0x1878;
    identity(r, manager, b + CURRENCY_MANAGER_VTABLE)?;
    identity(r, b + CURRENCY_MANAGER_VTABLE, b + BALANCE_GETTER)?;
    let header: [u8; 16] = r.read(manager + 8)?;
    let capacity = dword(&header, 0);
    let count = dword(&header, 4);
    let entries = u64::from(dword(&header, 8)) | u64::from(dword(&header, 12)) << 32;
    if capacity == 0 && count == 0 && entries == 0 {
        return Err(WalletError::Empty);
    }
    if capacity == 0
        || capacity > MAX_CAPACITY
        || !capacity.is_power_of_two()
        || count > capacity
        || count as usize > MAX_CURRENCIES
        || entries & 7 != 0
        || !(0x10000..=MAX_POINTER).contains(&entries)
    {
        return Err(WalletError::Bounds);
    }
    if count == 0 {
        return Err(WalletError::Empty);
    }
    let mut table = vec![0u8; capacity as usize * BUCKET];
    for (index, chunk) in table.chunks_mut(CHUNK).enumerate() {
        r.read_into(entries + (index * CHUNK) as u64, chunk)?;
    }
    if r.scalar(context + 0x98, 8)? != char_context
        || r.scalar(char_context + 0xa0, 8)? != character
        || r.scalar(char_context, 8)? != b + CHAR_CONTEXT_VTABLE
        || r.scalar(character, 8)? != b + CHARACTER_VTABLE
        || r.scalar(manager, 8)? != b + CURRENCY_MANAGER_VTABLE
        || r.read::<16>(manager + 8)? != header
    {
        return Err(WalletError::Changed);
    }
    let mask = capacity - 1;
    let occupied_hash = |bucket: u32| dword(&table, bucket as usize * BUCKET + 8);
    // Walk one full turn starting right after a free bucket, so `run` is always the number of
    // occupied buckets immediately before the current one. With no free bucket at all, every
    // position is reachable from every home.
    let free = (0..capacity).find(|bucket| occupied_hash(*bucket) == 0);
    let start = free.map_or(0, |bucket| bucket + 1);
    let mut balances = BTreeMap::new();
    let mut run = 0u32;
    for step in 0..capacity {
        let bucket = (start + step) & mask;
        let hash = occupied_hash(bucket);
        if hash == 0 {
            run = 0;
            continue;
        }
        let key = dword(&table, bucket as usize * BUCKET);
        let balance = dword(&table, bucket as usize * BUCKET + 4);
        if hash != currency_hash(key, &profile.table) {
            return Err(WalletError::Integrity);
        }
        // The lookup starts at `hash & mask` and stops at the first free bucket.
        if free.is_some() && (bucket.wrapping_sub(hash & mask) & mask) > run {
            return Err(WalletError::Integrity);
        }
        run += 1;
        if key == 0 || key > i32::MAX as u32 || balance > i32::MAX as u32 {
            return Err(WalletError::Range);
        }
        if balances.insert(key, balance).is_some() {
            return Err(WalletError::Integrity);
        }
    }
    if balances.len() != count as usize {
        return Err(WalletError::Integrity);
    }
    WalletSnapshot::checked((char_context, character), balances).ok_or(WalletError::Integrity)
}
