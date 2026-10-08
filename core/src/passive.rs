//! What the bag and Magic Find interpreters share: closed reasons, guard verification and the
//! two pointer rules of the certified build.
//!
//! Both rules were measured in a running game on 2026-10-08 with the external probes of
//! `tyrian-companion/docs/audit/loot-mf-probe` and `loot-bag-capacity-probe`: heap objects sat
//! on 8 bytes (91 of 91 buff nodes, 16 of 16 bag items) and pointers into game content sat 4
//! past a multiple of 8 (278 of 278 in one pass, and every bag definition and payload). The
//! content rule is observed in one session of one build, not derived from code: if it stops
//! holding, a reader reports [`Uncovered::Alignment`] and no value.

use crate::inventory::{Memory, ReadError, Reader, MAX_POINTER};
use crate::sha256::{hex, sha256};
pub use crate::wallet::Guard;

/// Address modulo 8 of every pointer into game content on the certified build.
pub const CONTENT_REMAINDER: u64 = 4;

/// Closed reasons for a value without coverage; no address or OS error reaches the UI.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Uncovered {
    /// A static code/table range differs from the audited bytes of this build.
    Guard,
    /// A vtable, dispatch slot or definition type on the route is not the certified one.
    Profile,
    /// The route has no controlled character, local player or owner right now.
    Root,
    /// A header, count, pointer or the read budget is outside the certified bounds.
    Bounds,
    /// A pointer does not sit where its kind sits: 8 bytes for heap objects, 4 past for content.
    Alignment,
    /// A bucket hash, a key, a count or a sorted table is inconsistent with itself.
    Integrity,
    /// A counted record needs live game state this passive reader does not evaluate.
    Unsupported,
    /// An owner, a vtable, a header or a table changed while it was being copied.
    Changed,
    ReadFailed,
}
impl From<ReadError> for Uncovered {
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

/// Every guard must hold the audited bytes; `each` sees the bytes of the ones that do.
pub(crate) fn verify<M: Memory>(
    r: &mut Reader<M>,
    base: u64,
    guards: &[Guard],
    mut each: impl FnMut(&Guard, &[u8]),
) -> Result<(), Uncovered> {
    for guard in guards {
        let mut bytes = vec![0u8; guard.size];
        r.read_into(base + guard.rva, &mut bytes)?;
        if hex(&sha256(&bytes)) != guard.sha256 {
            return Err(Uncovered::Guard);
        }
        each(guard, &bytes);
    }
    Ok(())
}

fn in_range(value: u64) -> bool {
    (0x10000..=MAX_POINTER).contains(&value)
}
/// A heap object pointer already read as part of a record. NULL is absence of its owner.
pub(crate) fn heap(value: u64) -> Result<u64, Uncovered> {
    if value == 0 {
        return Err(Uncovered::Root);
    }
    if !in_range(value) {
        return Err(Uncovered::Bounds);
    }
    if value & 7 != 0 {
        return Err(Uncovered::Alignment);
    }
    Ok(value)
}
/// A pointer into game content already read as part of a record. The client dereferences
/// these without a null check, so NULL here is a structure this reader misread.
pub(crate) fn content(value: u64) -> Result<u64, Uncovered> {
    if !in_range(value) {
        return Err(Uncovered::Bounds);
    }
    if value & 7 != CONTENT_REMAINDER {
        return Err(Uncovered::Alignment);
    }
    Ok(value)
}
/// Read a heap object pointer on the route.
pub(crate) fn object<M: Memory>(r: &mut Reader<M>, address: u64) -> Result<u64, Uncovered> {
    heap(r.scalar(address, 8)?)
}
/// A vtable pointer or vtable slot must be exactly the certified one.
pub(crate) fn identity<M: Memory>(
    r: &mut Reader<M>,
    address: u64,
    expected: u64,
) -> Result<(), Uncovered> {
    if r.scalar(address, 8)? != expected {
        return Err(Uncovered::Profile);
    }
    Ok(())
}
/// The context handed over by the TEB route: an aligned user pointer with room for its fields.
pub(crate) fn context_checked(context: u64) -> Result<(), Uncovered> {
    if !(0x10000..=MAX_POINTER - 0x198).contains(&context) || context & 7 != 0 {
        return Err(Uncovered::Bounds);
    }
    Ok(())
}
pub(crate) fn dword(bytes: &[u8], offset: usize) -> u32 {
    u32::from_le_bytes([
        bytes[offset],
        bytes[offset + 1],
        bytes[offset + 2],
        bytes[offset + 3],
    ])
}
pub(crate) fn qword(bytes: &[u8], offset: usize) -> u64 {
    u64::from(dword(bytes, offset)) | u64::from(dword(bytes, offset + 4)) << 32
}
/// Copy `size` bytes of one game-owned array in exact reads of whole `record`s.
pub(crate) fn table<M: Memory>(
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
