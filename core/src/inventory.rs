//! Passive, bounded inventory interpretation for one certified GW2 build.
//!
//! Addresses here are RVAs/field offsets from the 2026-10-06 proof, never session pointers.
//! The adapter provides exact reads; this module never dereferences or calls game code.
//! A successful sample is a checked observation, not a causal loot event.

use crate::wallet::{WalletCoverage, WalletSnapshot};
use std::collections::{BTreeMap, BTreeSet};

pub const BUILD_SHA256: &str = "27d179bfe6a92fae633b412b8be0c90f697cd08646fa66a2e04b9e794410802c";
pub const PROFILE: &str = "owned-bags-v3";
pub const TLS_INDEX_RVA: u64 = 0x28145c0;
pub const MAX_POSITIONS: u32 = 640;
pub const MAX_BYTES: usize = 131_072;
pub const MAX_READS: usize = 32_768;
pub(crate) const MAX_POINTER: u64 = 0x0000_7fff_ffff_ffff;

/// Closed diagnostics; no address, character identity or OS error reaches the wire.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ReadError {
    UnsupportedBuild,
    RootUnavailable,
    ReadFailed,
    Bounds,
    ProfileMismatch,
    Changed,
}

/// Only an exact, safe copy into owned bytes is permitted. Windows implements this with RPM.
pub trait Memory {
    fn read_exact(&mut self, address: u64, bytes: &mut [u8]) -> Result<(), ReadError>;
}

/// One-cycle budget, including failed requests and stability checks. Allocation is bounded too.
pub struct Reader<M> {
    memory: M,
    limit: usize,
    pub bytes: usize,
    pub reads: usize,
}
impl<M: Memory> Reader<M> {
    pub fn new(memory: M) -> Self {
        Self::bounded(memory, MAX_BYTES)
    }
    /// A reader with a smaller byte budget of its own; `limit` can never raise [`MAX_BYTES`].
    pub fn bounded(memory: M, limit: usize) -> Self {
        Self {
            memory,
            limit: limit.min(MAX_BYTES),
            bytes: 0,
            reads: 0,
        }
    }
    pub fn read<const N: usize>(&mut self, address: u64) -> Result<[u8; N], ReadError> {
        let mut bytes = [0; N];
        self.read_into(address, &mut bytes)?;
        Ok(bytes)
    }
    /// The one exact copy every read goes through: 1..=4096 bytes, charged before the request.
    pub fn read_into(&mut self, address: u64, bytes: &mut [u8]) -> Result<(), ReadError> {
        let size = bytes.len();
        if size == 0
            || size > 4096
            || address < 0x10000
            || address
                .checked_add(size as u64)
                .is_none_or(|end| end > MAX_POINTER)
            || self.bytes + size > self.limit
            || self.reads >= MAX_READS
        {
            return Err(ReadError::Bounds);
        }
        self.bytes += size;
        self.reads += 1;
        self.memory.read_exact(address, bytes)
    }
    pub fn scalar(&mut self, address: u64, size: usize) -> Result<u64, ReadError> {
        Ok(match size {
            2 => u16::from_le_bytes(self.read(address)?) as u64,
            4 => u32::from_le_bytes(self.read(address)?) as u64,
            8 => u64::from_le_bytes(self.read(address)?),
            _ => return Err(ReadError::Bounds),
        })
    }
    pub fn pointer(&mut self, address: u64) -> Result<u64, ReadError> {
        let value = self.scalar(address, 8)?;
        if value != 0 && !(0x10000..=MAX_POINTER).contains(&value) {
            return Err(ReadError::Bounds);
        }
        Ok(value)
    }
    fn equal(&mut self, address: u64, size: usize, expected: u64) -> Result<(), ReadError> {
        if self.scalar(address, size)? != expected {
            return Err(ReadError::ProfileMismatch);
        }
        Ok(())
    }
}

/// The hash must come from the executable itself. An unsupported hash cannot read game fields.
#[derive(Debug, Clone, Copy)]
pub struct BuildProfile {
    pub(crate) base: u64,
}
impl BuildProfile {
    pub fn checked(hash: &str, base: u64, image_size: u64) -> Result<Self, ReadError> {
        if hash != BUILD_SHA256 {
            return Err(ReadError::UnsupportedBuild);
        }
        if base < 0x10000
            || base & 7 != 0
            || image_size < TLS_INDEX_RVA + 4
            || base
                .checked_add(image_size)
                .is_none_or(|end| end > MAX_POINTER)
        {
            return Err(ReadError::Bounds);
        }
        Ok(Self { base })
    }
}

/// Resolve one validated Win64 TEB's TLS route. NULL is absence, never an empty inventory.
pub fn context_from_teb<M: Memory>(
    reader: &mut Reader<M>,
    profile: BuildProfile,
    teb: u64,
) -> Result<Option<u64>, ReadError> {
    if !(0x10000..=MAX_POINTER - 0x58).contains(&teb) || teb & 7 != 0 {
        return Err(ReadError::Bounds);
    }
    let index = reader.scalar(profile.base + TLS_INDEX_RVA, 4)?;
    if index > 4095 {
        return Err(ReadError::Bounds);
    }
    let tls = reader.pointer(teb + 0x58)?;
    if tls == 0 {
        return Ok(None);
    }
    let block = reader.pointer(tls + index * 8)?;
    if block == 0 {
        return Ok(None);
    }
    let context = reader.pointer(block + 0x10)?;
    if context == 0 || reader.pointer(context + 0x198)? == 0 {
        return Ok(None);
    }
    Ok(Some(context))
}

/// Only aggregated supported IDs are exposed. An unknown instance suppresses its entire ID.
/// `owner` stays local and forces a new epoch when the controlled inventory changes.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct InventorySnapshot {
    pub owner: (u64, u64),
    pub quantities: BTreeMap<u32, u32>,
    pub unknown: u32,
    /// Sparse matrix positions examined, for local reader diagnostics only.
    pub positions: u32,
    /// The observed sparse array does not certify usable bag capacity or free slots.
    pub free_slots: Option<u32>,
    /// Wallet balances read in the same cycle. `None` is missing coverage, never zero balances;
    /// [`inventory_snapshot`] leaves it empty and a wallet failure never rejects this sample.
    pub wallet: Option<WalletSnapshot>,
}

/// GetStackQuantity's certified branches. Never invoke the getter or guess an unknown quantity.
fn quantity<M: Memory>(
    r: &mut Reader<M>,
    base: u64,
    item: u64,
    vt: u64,
    definition: u64,
) -> Result<Option<u32>, ReadError> {
    let getter = r.pointer(vt + 0x260)?;
    let offset = match getter.checked_sub(base) {
        Some(0x84c310) => Some(0x98),
        Some(0x13c81c0) => Some(0xd0),
        Some(0x168aa0) => {
            if r.read::<3>(getter)? != [0x33, 0xc0, 0xc3] {
                return Err(ReadError::ProfileMismatch);
            }
            return Ok(Some(1)); // Explicitly certified NULL Stackable fallback in final getter.
        }
        _ => None,
    };
    if let Some(offset) = offset {
        return stack_quantity(r, base, item + offset).map(Some);
    }
    let p = match vt.checked_sub(base) {
        Some(0x225d580) => (0xa8, 0x225d898, 0, 0x13c9d60, 5, 0, 1, 1, 0x98, 0x13c9d20),
        Some(0x225dd78) => (0xa8, 0x225e080, 0, 0x13ca590, 10, 0, 1, 1, 0x98, 0x13c9d20),
        Some(0x225d910) => (
            0xe0, 0x225dcb8, 0x20, 0x13ca130, 9, 0x18, 0xffffffff, 4, 0xe8, 0x13c9f60,
        ),
        _ => return Ok(None),
    };
    if getter != base + p.9 {
        return Err(ReadError::ProfileMismatch);
    }
    r.equal(item + p.0, 8, base + p.1)?;
    r.equal(base + p.1 + p.2, 8, base + p.3)?;
    r.equal(definition + 0x2c, 4, p.4)?;
    let payload = r.pointer(definition + 0x30)?;
    if payload == 0 {
        return Err(ReadError::ProfileMismatch);
    }
    let condition = r.scalar(payload + p.5, 4)?;
    let result = if condition & p.6 == p.7 {
        stack_quantity(r, base, item + p.8)?
    } else {
        1
    };
    if r.pointer(definition + 0x30)? != payload
        || r.scalar(definition + 0x2c, 4)? != p.4
        || r.scalar(payload + p.5, 4)? != condition
    {
        return Err(ReadError::Changed);
    }
    Ok(Some(result))
}
fn stack_quantity<M: Memory>(r: &mut Reader<M>, base: u64, stack: u64) -> Result<u32, ReadError> {
    let vt = r.pointer(stack)?;
    r.equal(vt, 8, base + 0x168d10)?;
    let value = r.scalar(stack + 8, 4)?;
    if value > 250 {
        return Err(ReadError::Bounds);
    }
    if r.pointer(stack)? != vt || r.scalar(stack + 8, 4)? != value {
        return Err(ReadError::Changed);
    }
    Ok(value as u32)
}

/// The rule of [`Reader::pointer`] for a pointer that arrived inside a wider copy.
fn pointer_value(value: u64) -> Result<u64, ReadError> {
    if value != 0 && !(0x10000..=MAX_POINTER).contains(&value) {
        return Err(ReadError::Bounds);
    }
    Ok(value)
}
/// A little-endian field of an owned copy. A copy too short for its field fails closed.
fn qword(bytes: &[u8], offset: usize) -> Result<u64, ReadError> {
    bytes
        .get(offset..)
        .and_then(|rest| rest.first_chunk::<8>())
        .map(|field| u64::from_le_bytes(*field))
        .ok_or(ReadError::Bounds)
}
/// Copy the whole position matrix in exact reads of whole pointers, at most 512 of them a
/// read. The caller has already held `count` to [`MAX_POSITIONS`].
fn positions<M: Memory>(r: &mut Reader<M>, array: u64, count: u64) -> Result<Vec<u8>, ReadError> {
    let mut bytes = vec![0u8; count as usize * 8];
    for (index, part) in bytes.chunks_mut(4096).enumerate() {
        r.read_into(array + (index * 4096) as u64, part)?;
    }
    Ok(bytes)
}

/// Read only the owner's location3 matrix (C8/D0/D4), never the location4 A8 matrix.
/// Sparse itemcontext is resolved by instance index, never scanned to find an item.
pub fn inventory_snapshot<M: Memory>(
    r: &mut Reader<M>,
    profile: BuildProfile,
    context: u64,
) -> Result<InventorySnapshot, ReadError> {
    if !(0x10000..=MAX_POINTER - 0x198).contains(&context) || context & 7 != 0 {
        return Err(ReadError::Bounds);
    }
    let b = profile.base;
    let charctx = r.pointer(context + 0x98)?;
    r.equal(charctx, 8, b + 0x215cf48)?;
    r.equal(b + 0x215cf48 + 0x68, 8, b + 0x11b4480)?;
    let character = r.pointer(charctx + 0x98)?;
    if character == 0 || r.scalar(character + 0x178, 4)? & 0x10 == 0 {
        return Err(ReadError::RootUnavailable);
    }
    r.equal(character + 8, 8, b + 0x21601d0)?;
    r.equal(b + 0x21601d0 + 0xc8, 8, b + 0x11d74d0)?;
    let inventory = r.pointer(character + 0x3f0)?;
    r.equal(inventory, 8, b + 0x21621a8)?;
    r.equal(b + 0x21621a8 + 0x220, 8, b + 0x45dd90)?;
    r.equal(inventory + 0x70, 8, character)?;
    r.equal(b + 0x21621a8 + 0x190, 8, b + 0x11ee3d0)?;
    let count = r.scalar(inventory + 0xd4, 4)?;
    let capacity = r.scalar(inventory + 0xd0, 4)?;
    if count > capacity || capacity > MAX_POSITIONS as u64 {
        return Err(ReadError::Bounds);
    }
    let array = r.pointer(inventory + 0xc8)?;
    if count != 0 && array == 0 {
        return Err(ReadError::Bounds);
    }
    let itemctx = r.pointer(context + 0x178)?;
    r.equal(itemctx, 8, b + 0x225bd00)?;
    r.equal(b + 0x225bd00 + 0x10, 8, b + 0x13c6400)?;
    let length = r.scalar(itemctx + 0x3c, 4)?;
    let item_capacity = r.scalar(itemctx + 0x38, 4)?;
    if length > item_capacity || item_capacity > 1_048_576 {
        return Err(ReadError::Bounds);
    }
    let item_array = r.pointer(itemctx + 0x30)?;
    // The matrix is copied whole before any item is followed and whole again once they have
    // all been read: every position, occupied or not, must hold at the end what it held
    // before the first item was read.
    let matrix = positions(r, array, count)?;
    let mut checked = Vec::with_capacity(count as usize);
    let mut excluded = Vec::with_capacity(count as usize);
    let mut seen = BTreeSet::new();
    let mut quantities: BTreeMap<u32, u32> = BTreeMap::new();
    let mut unsupported = BTreeSet::new();
    let mut unknown = 0;
    for position in matrix.chunks_exact(8) {
        let item = pointer_value(qword(position, 0)?)?;
        if item == 0 {
            continue;
        }
        let vt = r.pointer(item)?;
        if !(b + 0x1913000..b + 0x2552390).contains(&vt) {
            return Err(ReadError::ProfileMismatch);
        }
        r.equal(vt + 0x70, 8, b + 0x13c45d0)?;
        let location = r.scalar(item + 0x48, 2)?;
        if location & 15 != 3 {
            // Classification is part of coverage: an excluded entry must not enter our
            // inventory during this copy. Do not inspect its owner, ID or quantity.
            excluded.push((item, vt, location));
            continue;
        }
        r.equal(item + 0x58, 8, inventory)?;
        r.equal(vt + 0xa0, 8, b + 0x13c46d0)?;
        r.equal(vt + 8, 8, b + 0x13c3e10)?;
        r.equal(vt + 0x68, 8, b + 0x31b980)?;
        let reference = r.scalar(item + 0x38, 4)?;
        if reference == 0 || reference >= length || !seen.insert(reference) {
            return Err(ReadError::Bounds);
        }
        if r.pointer(item_array + reference * 8)? != item {
            return Err(ReadError::Changed);
        }
        let definition = r.pointer(item + 0x40)?;
        let id = r.scalar(definition + 0x28, 4)?;
        if !(1..1_000_000).contains(&id) {
            return Err(ReadError::Bounds);
        }
        let value = quantity(r, b, item, vt, definition)?;
        if let Some(value) = value {
            let total = quantities.entry(id as u32).or_default();
            *total = total
                .checked_add(value)
                .filter(|n| *n <= i32::MAX as u32)
                .ok_or(ReadError::Bounds)?;
        } else {
            unknown += 1;
            unsupported.insert(id as u32);
        }
        checked.push((item, vt, definition, reference, id, location, value));
        // Identity, owner and quantity must agree on the second pass; no volatile pointer escapes.
        if r.pointer(item)? != vt
            || r.pointer(item + 0x40)? != definition
            || r.scalar(definition + 0x28, 4)? != id
            || r.scalar(item + 0x38, 4)? != reference
            || r.pointer(item_array + reference * 8)? != item
            || r.scalar(item + 0x48, 2)? != location
            || r.pointer(item + 0x58)? != inventory
        {
            return Err(ReadError::Changed);
        }
    }
    let again = positions(r, array, count)?;
    for (now, before) in again.chunks_exact(8).zip(matrix.chunks_exact(8)) {
        if pointer_value(qword(now, 0)?)? != qword(before, 0)? {
            return Err(ReadError::Changed);
        }
    }
    for (item, vt, location) in excluded {
        if r.pointer(item)? != vt || r.scalar(item + 0x48, 2)? != location {
            return Err(ReadError::Changed);
        }
    }
    for (item, vt, definition, reference, id, location, value) in checked {
        if r.pointer(item)? != vt
            || r.pointer(item + 0x40)? != definition
            || r.scalar(definition + 0x28, 4)? != id
            || r.scalar(item + 0x38, 4)? != reference
            || r.pointer(item_array + reference * 8)? != item
            || r.scalar(item + 0x48, 2)? != location
            || r.pointer(item + 0x58)? != inventory
            || quantity(r, b, item, vt, definition)? != value
        {
            return Err(ReadError::Changed);
        }
    }
    if r.pointer(context + 0x98)? != charctx
        || r.pointer(charctx + 0x98)? != character
        || r.pointer(character + 0x3f0)? != inventory
        || r.pointer(inventory + 0x70)? != character
        || r.scalar(character + 0x178, 4)? & 0x10 == 0
        || r.scalar(inventory + 0xd4, 4)? != count
        || r.scalar(inventory + 0xd0, 4)? != capacity
        || r.pointer(inventory + 0xc8)? != array
        || r.pointer(context + 0x178)? != itemctx
        || r.pointer(itemctx + 0x30)? != item_array
        || r.scalar(itemctx + 0x3c, 4)? != length
        || r.scalar(itemctx + 0x38, 4)? != item_capacity
    {
        return Err(ReadError::Changed);
    }
    for id in unsupported {
        quantities.remove(&id);
    }
    Ok(InventorySnapshot {
        owner: (character, inventory),
        quantities,
        unknown,
        positions: count as u32,
        free_slots: None,
        wallet: None,
    })
}

/// Small local diagnostics for QA. No memory address or full inventory is displayed/logged.
#[derive(Debug, Clone, Copy, Default)]
pub struct Diagnostics {
    pub threads: u32,
    pub bytes: u32,
    pub reads: u32,
    pub positions: u32,
    pub owner_verified: bool,
    /// Wallet outcome of the same cycle; its reads have their own budget and counters.
    pub wallet: WalletCoverage,
    pub wallet_bytes: u32,
    pub wallet_reads: u32,
    /// Bag capacity and free slots of the same cycle, with their own budget and counters.
    /// The Labyrinth panel paints it (`crate::panel`); nothing puts it on the wire.
    pub bags: crate::bags::BagCoverage,
    pub bag_bytes: u32,
    pub bag_reads: u32,
    /// Magic Find of the same cycle, with its own budget and counters. On the panel too, and
    /// not on the wire either.
    pub magic_find: crate::magic_find::MagicFindCoverage,
    pub magic_find_bytes: u32,
    pub magic_find_reads: u32,
}
