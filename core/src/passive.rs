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
    /// A copy failed: the page was unreadable or the adapter was asked to stop.
    ReadFailed,
    /// The cycle's time ran out before the pass finished. Nothing in this crate returns it: the
    /// adapter, which owns the clock, reports it in place of the `ReadFailed` its refusal causes.
    Deadline,
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
/// What every guard of the bag, wallet and Magic Find readers must find in memory: the SHA-256
/// of the executable file's bytes over its range, where the layout places it, in hex and in the
/// order of each reader's `guards_of`.
///
/// A guard proves that the code at a range is the code the reader was written against. For the
/// certified build ([`GuardDigests::CERTIFIED`]) that is the audited digest, which the
/// certificate checks in the file at the same offsets. A build whose code moved keeps the same
/// code with its rel32 and rip-relative fields rewritten, so its bytes, and their digest, differ
/// from the audited ones even where the layout is right: the resolver proves that code the same
/// by its normalized digest and every field's target (`crate::resolver`), and the guard proves
/// that memory holds what the file holds there. Nothing is loosened: the file's digest of a
/// range is the certified one exactly when its bytes are, and a range of memory that differs from
/// the file fails as it always did.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GuardDigests {
    pub bags: [&'static str; 12],
    pub wallet: [&'static str; 5],
    pub magic_find: [&'static str; 19],
}

impl GuardDigests {
    /// The audited digests of the certified build: those of each reader's `GUARDS`.
    pub const CERTIFIED: GuardDigests = GuardDigests {
        bags: digests_of(crate::bags::GUARDS),
        wallet: digests_of(crate::wallet::GUARDS),
        magic_find: digests_of(crate::magic_find::GUARDS),
    };
}

/// The digest of each guard, in order.
const fn digests_of<const N: usize>(guards: [Guard; N]) -> [&'static str; N] {
    let mut out = [""; N];
    let mut index = 0;
    while index < N {
        out[index] = guards[index].sha256;
        index += 1;
    }
    out
}

/// `guards`, each with the digest `digests` gives it in the same order: the ranges stay where
/// the layout put them.
pub(crate) fn with_digests<const N: usize>(mut guards: [Guard; N], digests: &[&'static str; N]) -> [Guard; N] {
    for (guard, digest) in guards.iter_mut().zip(digests) {
        guard.sha256 = digest;
    }
    guards
}

/// Guards the fixtures stand in for must cover exactly the audited ranges: same names, RVAs
/// and sizes, in the same order. Only the digests may differ, so an empty or shorter list can
/// never yield a verified profile.
pub(crate) fn same_ranges(guards: &[Guard], audited: &[Guard]) -> bool {
    guards.len() == audited.len()
        && guards
            .iter()
            .zip(audited)
            .all(|(a, b)| a.name == b.name && a.rva == b.rva && a.size == b.size)
}

/// A heap pointer already read as part of a record: an array or a node its header says exists.
/// NULL here is a header that disagrees with its pointer, not an absent character.
pub(crate) fn heap(value: u64) -> Result<u64, Uncovered> {
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
/// Read a heap object pointer on the route. NULL is absence of that owner right now.
pub(crate) fn object<M: Memory>(r: &mut Reader<M>, address: u64) -> Result<u64, Uncovered> {
    let value = r.scalar(address, 8)?;
    if value == 0 {
        return Err(Uncovered::Root);
    }
    heap(value)
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
