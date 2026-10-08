//! Passive, bounded bag capacity and free slots for the same certified GW2 build.
//!
//! The client keeps no capacity field. Its inventory counter (`InvButtonBar.cpp`, RVA
//! `0x6A2930`) asks the inventory for two numbers and a third method subtracts them:
//!
//! * total, slot `0x1F0` -> RVA `0x11EE520`: the sum of each equipped bag's size;
//! * used, slot `0x1E8` -> RVA `0x11EE4E0`: the non-null entries of the array at `+0xC8`;
//! * free, slot `0x1E0` -> RVA `0x11EE4A0`: total minus used.
//!
//! This module reads the same stored inputs and repeats that arithmetic. The route, its RVAs
//! and the bag item class come from `tyrian-companion/docs/audit/loot-bag-capacity-probe`,
//! whose external probe returned the inventory window's total (414) in a running game on
//! 2026-10-08. The used count is not part of that probe: it follows the audited getter but has
//! not been compared in game yet. As in `wallet`, vtables and getters are compared, never
//! invoked, and any doubt rejects the whole sample: free slots are then unknown, never zero.

use crate::inventory::{BuildProfile, Memory, Reader, MAX_POSITIONS, TLS_INDEX_RVA};
use crate::passive::{
    content, context_checked, dword, heap, identity, object, qword, table, verify, Guard,
    Uncovered,
};

pub const CHAR_CONTEXT_VTABLE: u64 = 0x215cf48;
pub const CHARACTER_AGENT_VTABLE: u64 = 0x21601d0;
pub const INVENTORY_VTABLE: u64 = 0x21621a8;
/// `ItCliBag`: the class the item factory (RVA `0x13C6E03`) builds for definition type 3.
pub const BAG_ITEM_VTABLE: u64 = 0x225cd18;
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

/// The getters of the route and the three counter methods, by the digest of their bytes.
pub const GUARDS: [Guard; 12] = [
    Guard {
        name: "controlled_character_getter",
        rva: 0x11b4480,
        size: 29,
        sha256: "1f8522d1275fdf3efb6c9b9c7d7ae54d914e2723422263f0073ce6d09a8a830b",
    },
    Guard {
        name: "character_inventory_getter",
        rva: 0x11d74d0,
        size: 70,
        sha256: "e7bd0ae3e1e372ee63d98c89639d52046ad8085b5d899cc076e252c808b0410c",
    },
    Guard {
        name: "inventory_owner_getter",
        rva: 0x45dd90,
        size: 5,
        sha256: "cce27efcd9ca6b4ac178564623ce7ed65d6bd9654f5dd661f5d3dfda6cfe8107",
    },
    Guard {
        name: "bag_slot_count_getter",
        rva: 0x11ee300,
        size: 7,
        sha256: "0a2a4a82e58c44dee59c4f6ae11b0670102f94daba8668d05c1e0876334a5c8b",
    },
    Guard {
        name: "bag_getter",
        rva: 0x12908e0,
        size: 72,
        sha256: "bac37e1cff99524d0b12d2ee360c6f462acfc93a8bd73a1d7cf274935d245b90",
    },
    Guard {
        name: "bag_size_getter",
        rva: 0x1290930,
        size: 133,
        sha256: "e830e97da7ff9a0a24ac044e3ff26824bda8637d2c07c998bea87d55880453e8",
    },
    Guard {
        name: "slot_validity",
        rva: 0x1290ac0,
        size: 148,
        sha256: "587a452b7f33987938235c45fb271fe137fa495019d31b6b492ca5db6f440587",
    },
    Guard {
        name: "free_slots_getter",
        rva: 0x11ee4a0,
        size: 59,
        sha256: "a2be789b4f0a47ba795672184770efe43f40f27df46e7fd29635e09a2ba8392a",
    },
    Guard {
        name: "used_slots_getter",
        rva: 0x11ee4e0,
        size: 60,
        sha256: "00b5744bc9e50c1364b05b0c47eddf31ddd1abf36845dc7cb948775e128b839d",
    },
    Guard {
        name: "capacity_getter",
        rva: 0x11ee520,
        size: 103,
        sha256: "a42aa2dae8d1baae211cce3e124bf1acbb2681681106a1a38918009ba0bc4342",
    },
    Guard {
        name: "item_definition_getter",
        rva: 0x13c3e10,
        size: 61,
        sha256: "d8cf3b68aa76a6ded61325e8533f1ff5735173ed7593a308456a7fa32d84037e",
    },
    Guard {
        name: "capacity_counter_text",
        rva: 0x6a2930,
        size: 136,
        sha256: "208e0765e402ed4d92092a68ba72c3cb56e5d5b2b793f172800405a6ce265203",
    },
];
/// Vtable, slot and the guarded code that slot must select.
pub const SLOTS: [(u64, u64, u64); 10] = [
    (CHAR_CONTEXT_VTABLE, 0x68, 0x11b4480),
    (CHARACTER_AGENT_VTABLE, 0xc8, 0x11d74d0),
    (INVENTORY_VTABLE, 0x220, 0x45dd90),
    (INVENTORY_VTABLE, 0x138, 0x12908e0),
    (INVENTORY_VTABLE, 0x140, 0x1290930),
    (INVENTORY_VTABLE, 0x148, 0x11ee300),
    (INVENTORY_VTABLE, 0x1e0, 0x11ee4a0),
    (INVENTORY_VTABLE, 0x1e8, 0x11ee4e0),
    (INVENTORY_VTABLE, 0x1f0, 0x11ee520),
    (BAG_ITEM_VTABLE, 0x8, 0x13c3e10),
];
// `BuildProfile::checked` only accepts an image that reaches the TLS index, which lies beyond
// every address this module reads inside the executable.
const _: () = assert!(BAG_ITEM_VTABLE + 0x10 <= TLS_INDEX_RVA);
const _: () = assert!(INVENTORY_VTABLE + 0x228 <= TLS_INDEX_RVA);

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
}
impl BagProfile {
    pub fn verified<M: Memory>(
        r: &mut Reader<M>,
        profile: BuildProfile,
    ) -> Result<Self, Uncovered> {
        Self::build(r, profile, &GUARDS)
    }
    /// For this crate's fixtures only; the addon calls [`Self::verified`] and nothing else.
    /// Fixtures carry no bytes of the game, so they stand for the executable with guard
    /// contents and digests of their own. The ranges must still be exactly the audited ones:
    /// an empty, shorter or shifted list is refused before anything is read.
    #[doc(hidden)]
    pub fn verified_against<M: Memory>(
        r: &mut Reader<M>,
        profile: BuildProfile,
        guards: &[Guard],
    ) -> Result<Self, Uncovered> {
        if !crate::passive::same_ranges(guards, &GUARDS) {
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
        Ok(Self { base: profile.base })
    }
}

/// One equipped bag: item -> definition -> bag payload -> size, the hops of RVA `0x1290930`.
fn bag_size<M: Memory>(r: &mut Reader<M>, base: u64, item: u64) -> Result<u32, Uncovered> {
    let item = heap(item)?;
    identity(r, item, base + BAG_ITEM_VTABLE)?;
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
    let b = profile.base;
    let char_context = object(r, context + 0x98)?;
    identity(r, char_context, b + CHAR_CONTEXT_VTABLE)?;
    let character = object(r, char_context + 0x98)?;
    identity(r, character + 8, b + CHARACTER_AGENT_VTABLE)?;
    let inventory = object(r, character + 0x3f0)?;
    identity(r, inventory, b + INVENTORY_VTABLE)?;
    for (vtable, slot, target) in SLOTS {
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
            capacity += bag_size(r, b, item)?;
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
        || r.scalar(char_context, 8)? != b + CHAR_CONTEXT_VTABLE
        || r.scalar(char_context + 0x98, 8)? != character
        || r.scalar(character + 8, 8)? != b + CHARACTER_AGENT_VTABLE
        || r.scalar(character + 0x178, 4)? & 0x10 == 0
        || r.scalar(character + 0x3f0, 8)? != inventory
        || r.scalar(inventory, 8)? != b + INVENTORY_VTABLE
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
