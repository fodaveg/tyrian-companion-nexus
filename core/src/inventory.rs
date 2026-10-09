//! Passive, bounded inventory interpretation for one certified GW2 build.
//!
//! Addresses here are RVAs/field offsets from the 2026-10-06 proof, never session pointers.
//! The adapter provides exact reads; this module never dereferences or calls game code.
//! A successful sample is a checked observation, not a causal loot event.

use crate::wallet::{WalletCoverage, WalletSnapshot};
use std::collections::{BTreeMap, BTreeSet};
use std::time::{Duration, Instant};

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

/// How long the thread and the context a full search found stand for the cycles that only
/// verify them. After this long, or as soon as the verification fails, the next cycle searches
/// all the threads again. Accepted by the owner on 8 October 2026.
pub const CONTEXT_SEARCH_EVERY: Duration = Duration::from_secs(30);

/// Where a full search found the game's context: the thread whose TLS route leads to it, that
/// thread's TEB, and the context. Only what a cycle needs to check it again; the thread is
/// named by its id and not held open.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Located {
    pub thread: u32,
    pub teb: u64,
    pub context: u64,
    /// How many threads of its own the process had when this was found, for the diagnostics.
    pub threads: u32,
}

/// What a cycle keeps of its search for the game's context: the thread found and when. One per
/// load, behind the caller's lock; it holds no handle and no address that is read without
/// being checked first.
#[derive(Debug, Default)]
pub struct ContextSearch {
    kept: Option<(Located, Instant)>,
}
impl ContextSearch {
    pub const fn new() -> Self {
        Self { kept: None }
    }
    /// What the last full search found, while it is younger than [`CONTEXT_SEARCH_EVERY`] at
    /// `now` (and `now` is not earlier than it). `None` is a cycle that has to search all the
    /// threads: nothing found yet, or too old, or forgotten.
    pub fn stored(&self, now: Instant) -> Option<Located> {
        let (located, at) = self.kept?;
        now.checked_duration_since(at)
            .filter(|age| *age < CONTEXT_SEARCH_EVERY)
            .map(|_| located)
    }
    /// A full search ended in a single context found on this thread.
    pub fn found(&mut self, located: Located, now: Instant) {
        self.kept = Some((located, now));
    }
    /// Forget it: the next cycle searches. For a failed verification, a cycle the client
    /// discarded, and the unload.
    pub fn forget(&mut self) {
        self.kept = None;
    }
}

/// Checks, with the reader, that `located` is still what a full search found: the TEB at
/// `teb+0x30` points to itself and carries this process and this thread in its client id
/// (`teb+0x40` and `teb+0x48`), and its TLS route still ends at the same context, past the same
/// checks as [`context_from_teb`]. `pid` is the process's own.
///
/// This is half of the check; the other half is the system's, that the thread still exists with
/// that TEB (the adapter asks for it by id). A TEB that was freed and reused by another thread
/// fails here by its client id. Anything but a perfect match is `false`, and a copy that fails
/// is an error: either way the caller searches all the threads, and reads nothing with what it
/// had.
pub fn verify_located<M: Memory>(
    reader: &mut Reader<M>,
    profile: BuildProfile,
    pid: u32,
    located: &Located,
) -> Result<bool, ReadError> {
    let Located { thread, teb, context, .. } = *located;
    if !(0x10000..=MAX_POINTER - 0x58).contains(&teb) || teb & 7 != 0 {
        return Err(ReadError::Bounds);
    }
    let head: [u8; 32] = reader.read(teb + 0x30)?;
    let word = |at: usize| u64::from_le_bytes(head[at..at + 8].try_into().unwrap());
    if word(0) != teb || word(0x10) != u64::from(pid) || word(0x18) != u64::from(thread) {
        return Ok(false);
    }
    Ok(context_from_teb(reader, profile, teb)? == Some(context))
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

/// Where the item vtables of the certified build live, and so where a class is a fixed word of
/// the executable rather than something the game writes.
fn vtables(base: u64) -> std::ops::Range<u64> {
    base + 0x1913000..base + 0x2552390
}
/// Up to 8 bytes at `address`, little-endian.
fn word<M: Memory>(r: &mut Reader<M>, address: u64, size: usize) -> Result<u64, ReadError> {
    let mut bytes = [0u8; 8];
    r.read_into(address, bytes.get_mut(..size).ok_or(ReadError::Bounds)?)?;
    Ok(u64::from_le_bytes(bytes))
}

/// Words of the executable itself that one pass has already copied: vtable slots and the
/// bytes of a getter. Items share a handful of classes, so each word is asked for once per
/// pass and not once per item, and [`Self::unchanged`] copies every one of them again before
/// the pass is accepted. Never kept from one pass to the next.
#[derive(Default)]
struct Statics(BTreeMap<(u64, usize), u64>);
impl Statics {
    fn word<M: Memory>(
        &mut self,
        r: &mut Reader<M>,
        address: u64,
        size: usize,
    ) -> Result<u64, ReadError> {
        if let Some(value) = self.0.get(&(address, size)) {
            return Ok(*value);
        }
        let value = word(r, address, size)?;
        self.0.insert((address, size), value);
        Ok(value)
    }
    /// A vtable slot must select exactly the certified code.
    fn equal<M: Memory>(
        &mut self,
        r: &mut Reader<M>,
        address: u64,
        expected: u64,
    ) -> Result<(), ReadError> {
        if self.word(r, address, 8)? != expected {
            return Err(ReadError::ProfileMismatch);
        }
        Ok(())
    }
    /// Every word this pass relied on, copied a second time.
    fn unchanged<M: Memory>(&self, r: &mut Reader<M>) -> Result<(), ReadError> {
        for ((address, size), value) in &self.0 {
            if word(r, *address, *size)? != *value {
                return Err(ReadError::Changed);
            }
        }
        Ok(())
    }
}

/// A certified conditional profile: where the item embeds its predicate class, that class's
/// vtable, the slot and the predicate it must select, the definition subtype, the payload
/// field with its mask and value, where the stack lies and the quantity getter.
type Conditional = (u64, u64, u64, u64, u64, u64, u64, u64, u64, u64);
/// The conditional profile of an item class, for the three classes that have one.
fn conditional(base: u64, vt: u64) -> Option<Conditional> {
    Some(match vt.checked_sub(base) {
        Some(0x225d580) => (0xa8, 0x225d898, 0, 0x13c9d60, 5, 0, 1, 1, 0x98, 0x13c9d20),
        Some(0x225dd78) => (0xa8, 0x225e080, 0, 0x13ca590, 10, 0, 1, 1, 0x98, 0x13c9d20),
        Some(0x225d910) => (
            0xe0, 0x225dcb8, 0x20, 0x13ca130, 9, 0x18, 0xffffffff, 4, 0xe8, 0x13c9f60,
        ),
        _ => return None,
    })
}
/// A definition's subtype and payload pointer, copied only for a class with a conditional
/// profile.
type Detail = Option<(u64, u64)>;
/// A definition's ID at `+0x28` and, when `detail` is asked for, the subtype and the payload
/// pointer that follow it at `+0x2c` and `+0x30`, in the same copy.
fn definition_record<M: Memory>(
    r: &mut Reader<M>,
    definition: u64,
    detail: bool,
) -> Result<(u64, Detail), ReadError> {
    if !detail {
        return Ok((r.scalar(definition + 0x28, 4)?, None));
    }
    let bytes: [u8; 16] = r.read(definition + 0x28)?;
    Ok((
        dword(&bytes, 0)?,
        Some((dword(&bytes, 4)?, qword(&bytes, 8)?)),
    ))
}

/// GetStackQuantity's certified branches. Never invoke the getter or guess an unknown quantity.
/// `conditional` is the item class's profile with the subtype and payload pointer its
/// definition held when the ID was copied; a class without a profile has neither.
fn quantity<M: Memory>(
    r: &mut Reader<M>,
    statics: &mut Statics,
    base: u64,
    item: u64,
    vt: u64,
    definition: u64,
    conditional: Option<(Conditional, (u64, u64))>,
) -> Result<Option<u32>, ReadError> {
    let getter = pointer_value(statics.word(r, vt + 0x260, 8)?)?;
    let offset = match getter.checked_sub(base) {
        Some(0x84c310) => Some(0x98),
        Some(0x13c81c0) => Some(0xd0),
        Some(0x168aa0) => {
            // `xor eax, eax; ret`, the bytes 33 C0 C3.
            if statics.word(r, getter, 3)? != 0xc3c033 {
                return Err(ReadError::ProfileMismatch);
            }
            return Ok(Some(1)); // Explicitly certified NULL Stackable fallback in final getter.
        }
        _ => None,
    };
    if let Some(offset) = offset {
        return stack_quantity(r, statics, base, item + offset).map(Some);
    }
    let Some((p, (subtype, payload))) = conditional else {
        return Ok(None);
    };
    if getter != base + p.9 {
        return Err(ReadError::ProfileMismatch);
    }
    r.equal(item + p.0, 8, base + p.1)?;
    statics.equal(r, base + p.1 + p.2, base + p.3)?;
    if subtype != p.4 {
        return Err(ReadError::ProfileMismatch);
    }
    let payload = pointer_value(payload)?;
    if payload == 0 {
        return Err(ReadError::ProfileMismatch);
    }
    let condition = r.scalar(payload + p.5, 4)?;
    let result = if condition & p.6 == p.7 {
        stack_quantity(r, statics, base, item + p.8)?
    } else {
        1
    };
    // Subtype and payload pointer lie together: one copy reads both again.
    let again: [u8; 12] = r.read(definition + 0x2c)?;
    if pointer_value(qword(&again, 4)?)? != payload
        || dword(&again, 0)? != p.4
        || r.scalar(payload + p.5, 4)? != condition
    {
        return Err(ReadError::Changed);
    }
    Ok(Some(result))
}
/// A stack's class pointer and its count, which lie together, in one copy.
fn stack<M: Memory>(r: &mut Reader<M>, address: u64) -> Result<(u64, u64), ReadError> {
    let bytes: [u8; 12] = r.read(address)?;
    Ok((pointer_value(qword(&bytes, 0)?)?, dword(&bytes, 8)?))
}
fn stack_quantity<M: Memory>(
    r: &mut Reader<M>,
    statics: &mut Statics,
    base: u64,
    address: u64,
) -> Result<u32, ReadError> {
    let (vt, value) = stack(r, address)?;
    // The stack's class is a pointer the game writes. Among the executable's vtables its
    // first slot is a fixed word like any other; anywhere else it is judged at every use.
    let first = if vtables(base).contains(&vt) {
        statics.word(r, vt, 8)?
    } else {
        r.scalar(vt, 8)?
    };
    if first != base + 0x168d10 {
        return Err(ReadError::ProfileMismatch);
    }
    if value > 250 {
        return Err(ReadError::Bounds);
    }
    if stack(r, address)? != (vt, value) {
        return Err(ReadError::Changed);
    }
    Ok(value as u32)
}

/// One owned stack as its turn left it, to be read again after its quantity and in the final
/// pass.
#[derive(Clone, Copy)]
struct Held {
    item: u64,
    vt: u64,
    definition: u64,
    reference: u64,
    id: u64,
    location: u64,
    value: Option<u32>,
}
/// Read again what identifies and places a held stack: its class, ID, instance reference,
/// resolver entry, definition, location and owner. `None` if any of them moved; otherwise the
/// definition's subtype and payload pointer when `detail` asks for them with the ID.
fn placed<M: Memory>(
    r: &mut Reader<M>,
    held: &Held,
    item_array: u64,
    inventory: u64,
    detail: bool,
) -> Result<Option<Detail>, ReadError> {
    if r.pointer(held.item)? != held.vt {
        return Ok(None);
    }
    let (id, detail) = definition_record(r, held.definition, detail)?;
    if id != held.id
        || r.scalar(held.item + 0x38, 4)? != held.reference
        || r.pointer(item_array + held.reference * 8)? != held.item
    {
        return Ok(None);
    }
    // The definition pointer and the location lie together: one copy for both.
    let place: [u8; 10] = r.read(held.item + 0x40)?;
    if pointer_value(qword(&place, 0)?)? != held.definition
        || word16(&place, 8)? != held.location
        || r.pointer(held.item + 0x58)? != inventory
    {
        return Ok(None);
    }
    Ok(Some(detail))
}

/// The rule of [`Reader::pointer`] for a pointer that arrived inside a wider copy.
fn pointer_value(value: u64) -> Result<u64, ReadError> {
    if value != 0 && !(0x10000..=MAX_POINTER).contains(&value) {
        return Err(ReadError::Bounds);
    }
    Ok(value)
}
/// A little-endian field of an owned copy. A copy too short for its field fails closed.
fn field<const N: usize>(bytes: &[u8], offset: usize) -> Result<[u8; N], ReadError> {
    bytes
        .get(offset..)
        .and_then(|rest| rest.first_chunk::<N>())
        .copied()
        .ok_or(ReadError::Bounds)
}
fn qword(bytes: &[u8], offset: usize) -> Result<u64, ReadError> {
    Ok(u64::from_le_bytes(field(bytes, offset)?))
}
fn dword(bytes: &[u8], offset: usize) -> Result<u64, ReadError> {
    Ok(u32::from_le_bytes(field(bytes, offset)?).into())
}
fn word16(bytes: &[u8], offset: usize) -> Result<u64, ReadError> {
    Ok(u16::from_le_bytes(field(bytes, offset)?).into())
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
    let mut statics = Statics::default();
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
        if !vtables(b).contains(&vt) {
            return Err(ReadError::ProfileMismatch);
        }
        statics.equal(r, vt + 0x70, b + 0x13c45d0)?;
        let location = r.scalar(item + 0x48, 2)?;
        if location & 15 != 3 {
            // Classification is part of coverage: an excluded entry must not enter our
            // inventory during this copy. Do not inspect its owner, ID or quantity.
            excluded.push((item, vt, location));
            continue;
        }
        r.equal(item + 0x58, 8, inventory)?;
        statics.equal(r, vt + 0xa0, b + 0x13c46d0)?;
        statics.equal(r, vt + 8, b + 0x13c3e10)?;
        statics.equal(r, vt + 0x68, b + 0x31b980)?;
        let reference = r.scalar(item + 0x38, 4)?;
        if reference == 0 || reference >= length || !seen.insert(reference) {
            return Err(ReadError::Bounds);
        }
        if r.pointer(item_array + reference * 8)? != item {
            return Err(ReadError::Changed);
        }
        let definition = r.pointer(item + 0x40)?;
        let profile = conditional(b, vt);
        let (id, detail) = definition_record(r, definition, profile.is_some())?;
        if !(1..1_000_000).contains(&id) {
            return Err(ReadError::Bounds);
        }
        let profile = profile.zip(detail);
        let value = quantity(r, &mut statics, b, item, vt, definition, profile)?;
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
        let held = Held {
            item,
            vt,
            definition,
            reference,
            id,
            location,
            value,
        };
        checked.push(held);
        // Identity, owner and quantity must agree on the second pass; no volatile pointer escapes.
        if placed(r, &held, item_array, inventory, false)?.is_none() {
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
    for held in checked {
        let profile = conditional(b, held.vt);
        let Some(detail) = placed(r, &held, item_array, inventory, profile.is_some())? else {
            return Err(ReadError::Changed);
        };
        let Held {
            item,
            vt,
            definition,
            value,
            ..
        } = held;
        let profile = profile.zip(detail);
        if quantity(r, &mut statics, b, item, vt, definition, profile)? != value {
            return Err(ReadError::Changed);
        }
    }
    // The final pass used to ask each item's class for its getter again. The classes are
    // now read once per pass, so all their words are read again here, after the last item.
    statics.unchanged(r)?;
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
