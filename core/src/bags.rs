//! Passive, bounded bag capacity and free slots for the same certified GW2 build.
//!
//! The client keeps no capacity field. Its inventory counter (`InvButtonBar.cpp`, RVA
//! `0x6A29D0`) asks the inventory for two numbers and a third method subtracts them:
//!
//! * total, slot `0x1F0` -> RVA `0x11EEFB0`: the sum of each equipped bag's size;
//! * used, slot `0x1E8` -> RVA `0x11EEF70`: the non-null entries of the array at `+0xC8`;
//! * free, slot `0x1E0` -> RVA `0x11EEF30`: total minus used.
//!
//! This module reads the same stored inputs and repeats that arithmetic. The route, its RVAs
//! and the bag item class come from `tyrian-companion/docs/audit/loot-bag-capacity-probe`,
//! whose external probe returned the inventory window's total (414) in a running game on
//! 2026-10-08. The used count is not part of that probe: it follows the audited getter but has
//! not been compared in game yet. As in `wallet`, vtables and getters are compared, never
//! invoked, and any doubt rejects the whole sample: free slots are then unknown, never zero.

use crate::inventory::{BuildProfile, Memory, Reader, MAX_POSITIONS};
use crate::layout::Layout;
use crate::passive::{
    content, context_checked, dword, heap, identity, object, qword, table, verify, Guard,
    Uncovered,
};

// The vtables of the certified build, for the tests that fix them against the audit. The reader
// takes them from its profile's layout, never from here.
pub const CHAR_CONTEXT_VTABLE: u64 = Layout::CERTIFIED.char_context_vtable;
pub const CHARACTER_AGENT_VTABLE: u64 = Layout::CERTIFIED.character_agent_vtable;
pub const INVENTORY_VTABLE: u64 = Layout::CERTIFIED.inventory_vtable;
/// `ItCliBag`: the class the item factory (RVA `0x13C7923`) builds for definition type 3.
pub const BAG_ITEM_VTABLE: u64 = Layout::CERTIFIED.bag_item_vtable;
/// `CHAR_INVENTORY_BAGS_MAX` and `CHAR_INVENTORY_BAG_MAX_SIZE` of this build.
pub const BAGS: usize = 16;
pub const BAG_SIZE_MAX: u32 = 32;
const BAG_ITEM_TYPE: u32 = 3;
const BAG_SIZE_MIN: u32 = 1;
/// One cycle at its largest, 16 bags and the 640 positions the inventory reader certifies:
///
/// * 883 for the guards, once per verified build;
/// * 272 for the route: 6 pointers and vtables, 10 slots, the flag, the owner, the slot count
///   and the 128-byte bag list;
/// * 576 for 16 bags of 36 bytes;
/// * 16 for the position header and array pointer, and 5120 for the array;
/// * 208 for the rereads of all of the above except the slots and the bags, and 5120 for the
///   array again.
///
/// 12195 in all, which a test pins. The limit leaves a third of headroom and is never raised
/// at run time.
pub const MAX_BYTES: usize = 16_384;

/// The getters of the route and the three counter methods, by the digest of their bytes, where
/// `layout` places them. The digests are those of the certified build's code.
pub const fn guards_of(l: &Layout) -> [Guard; 12] {
    [
        Guard {
            name: "controlled_character_getter",
            rva: l.controlled_character_getter,
            size: 29,
            sha256: "1f8522d1275fdf3efb6c9b9c7d7ae54d914e2723422263f0073ce6d09a8a830b",
        },
        Guard {
            name: "character_inventory_getter",
            rva: l.character_inventory_getter,
            size: 70,
            sha256: "12edfab95716e429e1d8f6d5878c4aa57e53e1d87a2c7fbfc080ae7cbd21647f",
        },
        Guard {
            name: "inventory_owner_getter",
            rva: l.inventory_owner_getter,
            size: 5,
            sha256: "cce27efcd9ca6b4ac178564623ce7ed65d6bd9654f5dd661f5d3dfda6cfe8107",
        },
        Guard {
            name: "bag_slot_count_getter",
            rva: l.bag_slot_count_getter,
            size: 7,
            sha256: "0a2a4a82e58c44dee59c4f6ae11b0670102f94daba8668d05c1e0876334a5c8b",
        },
        Guard {
            name: "bag_getter",
            rva: l.bag_getter,
            size: 72,
            sha256: "47c8e28fb4ed3ccd1506da0d63905194e3da2d67e87b806a600bd3d11cf8eced",
        },
        Guard {
            name: "bag_size_getter",
            rva: l.bag_size_getter,
            size: 133,
            sha256: "e054565f4fd7d4e99cf9170cebe37842aaf80619131e799a3defbc9c67c220cc",
        },
        Guard {
            name: "slot_validity",
            rva: l.slot_validity,
            size: 148,
            sha256: "a85a558be74a0dd3cb112e51769194faba3a6788fd59ea4e053726e7c534e396",
        },
        Guard {
            name: "free_slots_getter",
            rva: l.free_slots_getter,
            size: 59,
            sha256: "a2be789b4f0a47ba795672184770efe43f40f27df46e7fd29635e09a2ba8392a",
        },
        Guard {
            name: "used_slots_getter",
            rva: l.used_slots_getter,
            size: 60,
            sha256: "00b5744bc9e50c1364b05b0c47eddf31ddd1abf36845dc7cb948775e128b839d",
        },
        Guard {
            name: "capacity_getter",
            rva: l.capacity_getter,
            size: 103,
            sha256: "a42aa2dae8d1baae211cce3e124bf1acbb2681681106a1a38918009ba0bc4342",
        },
        Guard {
            name: "item_definition_getter",
            rva: l.item_definition_getter,
            size: 61,
            sha256: "f430638166c7ed4e47aed1717abbed7cc30ffe92c7714a7a9e66648c0fbca7ef",
        },
        Guard {
            name: "capacity_counter_text",
            rva: l.capacity_counter_text,
            size: 136,
            sha256: "2e39a5e22e8a6ea2e8f214ae1715088732885d31bed67f0b246b18620f7af850",
        },
    ]
}
/// Vtable, slot and the guarded code that slot must select, where `layout` places the vtables
/// and the code; the slots are fixed.
pub const fn slots_of(l: &Layout) -> [(u64, u64, u64); 10] {
    [
        (l.char_context_vtable, 0x68, l.controlled_character_getter),
        (l.character_agent_vtable, 0xc8, l.character_inventory_getter),
        (l.inventory_vtable, 0x220, l.inventory_owner_getter),
        (l.inventory_vtable, 0x138, l.bag_getter),
        (l.inventory_vtable, 0x140, l.bag_size_getter),
        (l.inventory_vtable, 0x148, l.bag_slot_count_getter),
        (l.inventory_vtable, 0x1e0, l.free_slots_getter),
        (l.inventory_vtable, 0x1e8, l.used_slots_getter),
        (l.inventory_vtable, 0x1f0, l.capacity_getter),
        (l.bag_item_vtable, 0x8, l.item_definition_getter),
    ]
}
/// The guards and slots of the certified build, for the tests that fix them against the audit.
pub const GUARDS: [Guard; 12] = guards_of(&Layout::CERTIFIED);
/// The guards `profile`'s build must hold: the ranges where its layout places them, each with
/// the digest its executable file has there ([`crate::passive::GuardDigests`]).
pub fn guards_in(profile: BuildProfile) -> [Guard; 12] {
    crate::passive::with_digests(guards_of(profile.layout), &profile.digests.bags)
}
pub const SLOTS: [(u64, u64, u64); 10] = slots_of(&Layout::CERTIFIED);
// `BuildProfile::checked` only accepts a layout whose image reaches the TLS index, which lies
// beyond every address this module reads inside the executable ([`Layout::validate`] checks it
// for any layout; these are the two that bound this module's slots in the certified one).
const _: () = assert!(Layout::CERTIFIED.bag_item_vtable + 0x10 <= Layout::CERTIFIED.tls_index);
const _: () = assert!(Layout::CERTIFIED.inventory_vtable + 0x228 <= Layout::CERTIFIED.tls_index);

/// What the inventory window's counter shows, recomputed from stored inputs.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct BagSlots {
    /// Sum of the sizes of the equipped bags: the counter's total.
    pub capacity: u32,
    /// Non-null positions of the inventory array: the counter's used count.
    pub occupied: u32,
    /// `capacity - occupied`, the client's own subtraction.
    pub free: u32,
    /// Unlocked bag slots and how many of them hold a bag.
    pub bag_slots: u32,
    pub bags: u32,
}

/// What the last cycle can say about the bags, for local use only.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum BagCoverage {
    /// No bag read was attempted: no negotiated sample yet, or the inventory route failed.
    #[default]
    NotRead,
    Read(BagSlots),
    Unavailable(Uncovered),
}

/// Proof that the static ranges of a hash-verified build hold the audited bytes. It can only be
/// built from a [`BuildProfile`]. Verify once per build, not per cycle.
#[derive(Debug, Clone, Copy)]
pub struct BagProfile {
    base: u64,
    layout: &'static Layout,
}
impl BagProfile {
    pub fn verified<M: Memory>(
        r: &mut Reader<M>,
        profile: BuildProfile,
    ) -> Result<Self, Uncovered> {
        Self::build(r, profile, &guards_in(profile))
    }
    /// For this crate's fixtures only; the addon calls [`Self::verified`] and nothing else.
    /// Fixtures carry no bytes of the game, so they stand for the executable with guard
    /// contents and digests of their own. The ranges must still be exactly the audited ones,
    /// where the profile's layout places them: an empty, shorter or shifted list is refused
    /// before anything is read.
    #[doc(hidden)]
    pub fn verified_against<M: Memory>(
        r: &mut Reader<M>,
        profile: BuildProfile,
        guards: &[Guard],
    ) -> Result<Self, Uncovered> {
        if !crate::passive::same_ranges(guards, &guards_of(profile.layout)) {
            return Err(Uncovered::Guard);
        }
        Self::build(r, profile, guards)
    }
    fn build<M: Memory>(
        r: &mut Reader<M>,
        profile: BuildProfile,
        guards: &[Guard],
    ) -> Result<Self, Uncovered> {
        verify(r, profile.base, guards, |_, _| {})?;
        Ok(Self {
            base: profile.base,
            layout: profile.layout,
        })
    }
}

/// One equipped bag: item -> definition -> bag payload -> size, the hops of RVA `0x1291440`.
/// `vtable` is the layout's `ItCliBag`.
fn bag_size<M: Memory>(
    r: &mut Reader<M>,
    base: u64,
    item: u64,
    vtable: u64,
) -> Result<u32, Uncovered> {
    let item = heap(item)?;
    identity(r, item, base + vtable)?;
    let definition = content(r.scalar(item + 0x40, 8)?)?;
    let record: [u8; 16] = r.read(definition + 0x28)?;
    if dword(&record, 4) != BAG_ITEM_TYPE {
        return Err(Uncovered::Profile);
    }
    let payload = content(qword(&record, 8))?;
    let size = r.scalar(payload + 0x28, 4)? as u32;
    // A bag that holds nothing is not a bag the counter can have added.
    if !(BAG_SIZE_MIN..=BAG_SIZE_MAX).contains(&size) {
        return Err(Uncovered::Bounds);
    }
    Ok(size)
}

/// Read the controlled character's bag slots and its position array, then read the owner, the
/// vtables, both headers and the bag list again. `owner` stays local, as in the other readers.
pub fn bag_slots<M: Memory>(
    r: &mut Reader<M>,
    profile: &BagProfile,
    context: u64,
) -> Result<(BagSlots, (u64, u64)), Uncovered> {
    context_checked(context)?;
    let (b, l) = (profile.base, profile.layout);
    let char_context = object(r, context + 0x98)?;
    identity(r, char_context, b + l.char_context_vtable)?;
    let character = object(r, char_context + 0x98)?;
    identity(r, character + 8, b + l.character_agent_vtable)?;
    let inventory = object(r, character + 0x3f0)?;
    identity(r, inventory, b + l.inventory_vtable)?;
    for (vtable, slot, target) in slots_of(l) {
        identity(r, b + vtable + slot, b + target)?;
    }
    if r.scalar(character + 0x178, 4)? & 0x10 == 0 {
        return Err(Uncovered::Root);
    }
    if r.scalar(inventory + 0x70, 8)? != character {
        return Err(Uncovered::Profile);
    }
    let bag_slots = r.scalar(inventory + 0x440, 4)? as u32;
    if bag_slots as usize > BAGS {
        return Err(Uncovered::Bounds);
    }
    let list: [u8; 8 * BAGS] = r.read(inventory + 0x380)?;
    let mut capacity = 0;
    let mut bags = 0;
    for index in 0..bag_slots as usize {
        let item = qword(&list, index * 8);
        if item != 0 {
            capacity += bag_size(r, b, item, l.bag_item_vtable)?;
            bags += 1;
        }
    }
    let positions: [u8; 8] = r.read(inventory + 0xd0)?;
    let (reserved, count) = (dword(&positions, 0), dword(&positions, 4));
    if count > reserved || reserved > MAX_POSITIONS {
        return Err(Uncovered::Bounds);
    }
    let array = r.scalar(inventory + 0xc8, 8)?;
    let entries = if count == 0 {
        Vec::new()
    } else {
        table(r, heap(array)?, count as usize * 8, 8)?
    };
    let occupied = entries
        .chunks_exact(8)
        .filter(|entry| entry.iter().any(|byte| *byte != 0))
        .count() as u32;
    if r.scalar(context + 0x98, 8)? != char_context
        || r.scalar(char_context, 8)? != b + l.char_context_vtable
        || r.scalar(char_context + 0x98, 8)? != character
        || r.scalar(character + 8, 8)? != b + l.character_agent_vtable
        || r.scalar(character + 0x178, 4)? & 0x10 == 0
        || r.scalar(character + 0x3f0, 8)? != inventory
        || r.scalar(inventory, 8)? != b + l.inventory_vtable
        || r.scalar(inventory + 0x70, 8)? != character
        || r.scalar(inventory + 0x440, 4)? as u32 != bag_slots
        || r.read::<{ 8 * BAGS }>(inventory + 0x380)? != list
        || r.read::<8>(inventory + 0xd0)? != positions
        || r.scalar(inventory + 0xc8, 8)? != array
        // The array is copied in up to two reads, and an item can move between them or after
        // them: only a second copy equal to the first makes `occupied` a count of one moment.
        || (!entries.is_empty() && table(r, array, entries.len(), 8)? != entries)
    {
        return Err(Uncovered::Changed);
    }
    // The client subtracts unsigned; more used positions than capacity is not a count to show.
    let free = capacity.checked_sub(occupied).ok_or(Uncovered::Integrity)?;
    Ok((
        BagSlots {
            capacity,
            occupied,
            free,
            bag_slots,
            bags,
        },
        (character, inventory),
    ))
}
