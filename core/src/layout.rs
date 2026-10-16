//! Where the game's executable keeps what the readers rely on: every RVA of `inventory`, `bags`,
//! `wallet` and `magic_find`, as one value.
//!
//! Until this module those RVAs were constants of each reader, so a reader could only run on the
//! build they were taken from. The readers now take them from the [`Layout`] of their
//! [`crate::inventory::BuildProfile`], and [`Layout::CERTIFIED`] is the one the certificate
//! proves (`code-208704-r1`, [`crate::certificate`]): with it every reader reads exactly what it
//! read with the constants. A layout resolved from a build whose code moved (the relocation plan
//! of `docs/audit/2026-10-16-relocation-study/`, [`Layout::from_resolved`]) is another value of
//! the same type; the readers do not change for it.
//!
//! Only addresses inside the image are here. Field offsets (`item+0x58`), vtable slots
//! (`vtable+0x68`), sizes, masks and the other constants of the route are not: the readers keep
//! them as constants, because the design keeps them fixed and fails closed when the game changes
//! one. An RVA is what is added to the image base; an offset is added to an object or to a
//! vtable.
//!
//! An RVA that two readers share is one field: the character context's vtable is the same
//! address for the four of them, so a layout cannot give two readers two answers for it.

use crate::certificate::{IMAGE_SIZE, SECTIONS};
use crate::resolver::Resolved;
use std::ops::Range;

/// Every RVA of the game's executable the readers use, with the image's size and where its
/// vtables live. Names follow the certificate's ([`crate::certificate::SPANS`] and
/// [`crate::certificate::SLOTS`]) where it names the same thing.
///
/// A layout is only used after [`Layout::validate`] accepts it; `BuildProfile::checked` refuses
/// one that it does not.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Layout {
    // The image.
    /// SizeOfImage: the loaded image must be exactly this big.
    pub image_size: u64,
    /// Where the TLS directory's AddressOfIndex points: the slot of the game's TLS index.
    pub tls_index: u64,
    /// `.rdata`, where the executable's vtables live: a class pointer inside it is a fixed word
    /// of the executable, one outside it something the game writes.
    pub rdata_vtables: Range<u64>,

    // Vtables, in `.rdata`.
    /// The character context (`ChCliContext`), the first object of every route.
    pub char_context_vtable: u64,
    /// The controlled character as an agent, at `character+8`.
    pub character_agent_vtable: u64,
    /// The controlled character itself, as the Magic Find route checks it.
    pub character_vtable: u64,
    /// The character as a combatant, at `character+0x40`.
    pub combatant_vtable: u64,
    /// The local player (the wallet's character, at `char_context+0xa0`).
    pub player_vtable: u64,
    /// The player's stats, at `player+0x9700`.
    pub player_stats_vtable: u64,
    /// The character's inventory, at `character+0x3f0`.
    pub inventory_vtable: u64,
    /// The item context, at `context+0x178`.
    pub item_context_vtable: u64,
    /// `ItCliBag`, the class of an equipped bag.
    pub bag_item_vtable: u64,
    /// The three item classes whose stack quantity depends on a predicate, and the class of the
    /// predicate each one embeds.
    pub conditional_item_a_vtable: u64,
    pub conditional_predicate_a_vtable: u64,
    pub conditional_item_b_vtable: u64,
    pub conditional_predicate_b_vtable: u64,
    pub conditional_item_c_vtable: u64,
    pub conditional_predicate_c_vtable: u64,
    /// The wallet's currency manager, at `player+0x1878`.
    pub currency_manager_vtable: u64,
    /// The character's buff manager.
    pub buff_manager_vtable: u64,
    /// A node of the buff table.
    pub buff_node_vtable: u64,

    // Data, in `.rdata`.
    /// The 256 dwords of the engine's key hash, which the wallet and the buff table both use.
    pub hash_table: u64,

    // Functions, in `.text`: the route's getters, compared by address in a vtable slot.
    /// Character context slot `0x68`.
    pub controlled_character_getter: u64,
    /// Character context slot `0x70`; the wallet calls it its local character.
    pub local_player_getter: u64,
    /// Character context slot `0x118`.
    pub player_by_id_getter: u64,
    /// Character agent slot `0xc8`.
    pub character_inventory_getter: u64,
    /// Character agent slot `0x68`.
    pub is_player: u64,
    /// Character agent slot `0x18`.
    pub player_id_getter: u64,
    /// Character slot `0x100`.
    pub combatant_cast: u64,
    /// Combatant slot `0x38`.
    pub buff_manager_getter: u64,
    /// Player slot `0x320`.
    pub player_stats_getter: u64,
    /// Player slot `0x250`.
    pub character_wallet_getter: u64,
    /// Player stats slot `0x30`.
    pub account_luck_level_getter: u64,

    // Functions of the inventory and its items.
    /// Inventory slot `0x220`.
    pub inventory_owner_getter: u64,
    /// Inventory slot `0x190`.
    pub location3_index_getter: u64,
    /// Item context slot `0x10`.
    pub item_resolver: u64,
    /// Item slot `0x8`, also the bag item's.
    pub item_definition_getter: u64,
    /// Item slot `0x68`.
    pub item_instance_getter: u64,
    /// Item slot `0x70`.
    pub item_location_getter: u64,
    /// Item slot `0xa0`.
    pub final_stack_quantity_getter: u64,
    /// The quantity getters of item slot `0x260` the inventory reader knows, and the first slot
    /// a stack's class must hold.
    pub stackable_at_0x98: u64,
    pub stackable_at_0xd0: u64,
    /// `xor eax, eax; ret`: a quantity of one.
    pub null_stackable: u64,
    pub stack_quantity_getter: u64,
    /// The quantity getters of the conditional classes: A and B share the first.
    pub conditional_stackable_a8: u64,
    pub conditional_stackable_e0: u64,
    /// The predicate each conditional class's predicate vtable must select.
    pub conditional_predicate_a: u64,
    pub conditional_predicate_b: u64,
    pub conditional_predicate_c: u64,

    // Functions of the bag counter (inventory slots `0x138` to `0x1f0`).
    pub bag_getter: u64,
    pub bag_size_getter: u64,
    pub bag_slot_count_getter: u64,
    pub free_slots_getter: u64,
    pub used_slots_getter: u64,
    pub capacity_getter: u64,
    /// What the bag getters call, guarded by digest only.
    pub slot_validity: u64,
    /// The inventory window's counter, which the bag reader repeats.
    pub capacity_counter_text: u64,

    // Functions of the wallet.
    /// Currency manager slot `0`.
    pub balance_getter: u64,
    /// The open-addressing lookup over [`Self::hash_table`].
    pub hash_lookup: u64,

    // Functions of the Magic Find (buff manager slots `0x18` to `0x28` and what they call).
    pub modifier_value: u64,
    pub pushed_modifier_sum: u64,
    pub buff_iterator: u64,
    pub buff_filter: u64,
    pub buff_lookup: u64,
    pub modifier_sum: u64,
    pub modifier_formula: u64,
    pub magic_find_case: u64,
    pub attribute_modifier_map: u64,
}

/// The section the certificate names `.rdata`, second of the seven it checks in order.
const RDATA: usize = 1;
const _: () = assert!(matches!(
    SECTIONS[RDATA].name,
    [b'.', b'r', b'd', b'a', b't', b'a', 0, 0]
));

/// How far past its start each vtable is read: its highest slot read or compared, plus the slot.
/// The relation [`Layout::validate`] checks for each is that the whole extent lies in
/// [`Layout::rdata_vtables`].
const VTABLE_EXTENTS: usize = 18;

impl Layout {
    /// The layout of the certified build (`code-208704-r1`): the RVAs the recertification of
    /// 15 October 2026 proved (`docs/audit/2026-10-15-recertification-build-208704.json`) and
    /// the certificate checks in the file of every build. The image size and `.rdata` are the
    /// certificate's own, so the two cannot disagree.
    pub const CERTIFIED: Layout = Layout {
        image_size: IMAGE_SIZE as u64,
        tls_index: 0x28155c0,
        rdata_vtables: SECTIONS[RDATA].rva as u64
            ..SECTIONS[RDATA].rva as u64 + SECTIONS[RDATA].virtual_size as u64,

        char_context_vtable: 0x215dfc8,
        character_agent_vtable: 0x2161250,
        character_vtable: 0x2160be0,
        combatant_vtable: 0x2161518,
        player_vtable: 0x215e9d8,
        player_stats_vtable: 0x21691b8,
        inventory_vtable: 0x2163228,
        item_context_vtable: 0x225ce50,
        bag_item_vtable: 0x225de68,
        conditional_item_a_vtable: 0x225e6d0,
        conditional_predicate_a_vtable: 0x225e9e8,
        conditional_item_b_vtable: 0x225eec8,
        conditional_predicate_b_vtable: 0x225f1d0,
        conditional_item_c_vtable: 0x225ea60,
        conditional_predicate_c_vtable: 0x225ee08,
        currency_manager_vtable: 0x2173178,
        buff_manager_vtable: 0x21841a8,
        buff_node_vtable: 0x2184160,

        hash_table: 0x1b90df0,

        controlled_character_getter: 0x11b4f10,
        local_player_getter: 0x498900,
        player_by_id_getter: 0x11b77f0,
        character_inventory_getter: 0x11d7f60,
        is_player: 0x11d9be0,
        player_id_getter: 0x11d84e0,
        combatant_cast: 0x11d74b0,
        buff_manager_getter: 0x400100,
        player_stats_getter: 0x11bb1c0,
        character_wallet_getter: 0x11bb070,
        account_luck_level_getter: 0x402b80,

        inventory_owner_getter: 0x45de30,
        location3_index_getter: 0x11eee60,
        item_resolver: 0x13c6f20,
        item_definition_getter: 0x13c4930,
        item_instance_getter: 0x31ba20,
        item_location_getter: 0x13c50f0,
        final_stack_quantity_getter: 0x13c51f0,
        stackable_at_0x98: 0x84c4c0,
        stackable_at_0xd0: 0x13c8ce0,
        null_stackable: 0x168aa0,
        stack_quantity_getter: 0x168d10,
        conditional_stackable_a8: 0x13ca840,
        conditional_stackable_e0: 0x13caa80,
        conditional_predicate_a: 0x13ca880,
        conditional_predicate_b: 0x13cb0b0,
        conditional_predicate_c: 0x13cac50,

        bag_getter: 0x12913f0,
        bag_size_getter: 0x1291440,
        bag_slot_count_getter: 0x11eed90,
        free_slots_getter: 0x11eef30,
        used_slots_getter: 0x11eef70,
        capacity_getter: 0x11eefb0,
        slot_validity: 0x12915d0,
        capacity_counter_text: 0x6a29d0,

        balance_getter: 0x12722a0,
        hash_lookup: 0x2397f0,

        modifier_value: 0x12c2f60,
        pushed_modifier_sum: 0x12c3030,
        buff_iterator: 0x12c33e0,
        buff_filter: 0x12c4e30,
        buff_lookup: 0x251e20,
        modifier_sum: 0x12c1050,
        modifier_formula: 0x12c1a40,
        magic_find_case: 0x3e4898,
        attribute_modifier_map: 0x3e3ea0,
    };

    /// The layout of a resolved executable ([`crate::resolver`]): each RVA by the name of its
    /// anchor or vtable in the relocation table, the names of the certificate (`bag:`, `wallet:`,
    /// `mf:`, `item:` and `vtable <class>`); the image size, the TLS index and `.rdata` as the
    /// resolver read them from the file's own header and TLS directory.
    ///
    /// `Err` names the first field's anchor that the table does not have or the resolution did
    /// not place, so a table that lost a name never gives a layout with a hole in it; then the
    /// relation of [`Layout::validate`] that the result breaks, if one does. Every field is named
    /// below, so a field added to `Layout` does not build until this says which anchor gives it.
    pub fn from_resolved(resolved: &Resolved) -> Result<Layout, &'static str> {
        let layout = Self::from_names(resolved)?;
        layout.validate()?;
        Ok(layout)
    }

    /// [`Layout::from_resolved`] before [`Layout::validate`]: `Err` is always a name.
    pub(crate) fn from_names(resolved: &Resolved) -> Result<Layout, &'static str> {
        let at = |name: &'static str| resolved.resolved_rva(name).ok_or(name);
        Ok(Layout {
            image_size: u64::from(resolved.image_size),
            tls_index: resolved.tls_index,
            rdata_vtables: resolved.rdata.0..resolved.rdata.1,

            char_context_vtable: at("vtable char_context")?,
            character_agent_vtable: at("vtable character_agent")?,
            character_vtable: at("vtable character")?,
            combatant_vtable: at("vtable combatant")?,
            player_vtable: at("vtable player (wallet character)")?,
            player_stats_vtable: at("vtable player_stats")?,
            inventory_vtable: at("vtable inventory")?,
            item_context_vtable: at("vtable item_context")?,
            bag_item_vtable: at("vtable bag_item (ItCliBag)")?,
            conditional_item_a_vtable: at("vtable conditional item A")?,
            conditional_predicate_a_vtable: at("vtable conditional predicate A")?,
            conditional_item_b_vtable: at("vtable conditional item B")?,
            conditional_predicate_b_vtable: at("vtable conditional predicate B")?,
            conditional_item_c_vtable: at("vtable conditional item C")?,
            conditional_predicate_c_vtable: at("vtable conditional predicate C")?,
            currency_manager_vtable: at("vtable currency_manager")?,
            buff_manager_vtable: at("vtable buff_manager")?,
            buff_node_vtable: at("vtable buff_node")?,

            hash_table: at("mf:hash_table + wallet:hash_table")?,

            controlled_character_getter: at("bag:controlled_character_getter + mf:controlled_character_getter")?,
            local_player_getter: at("mf:local_player_getter + wallet:local_character")?,
            player_by_id_getter: at("mf:player_by_id_getter")?,
            character_inventory_getter: at("bag:character_inventory_getter")?,
            is_player: at("mf:is_player")?,
            player_id_getter: at("mf:player_id_getter")?,
            combatant_cast: at("mf:combatant_cast")?,
            buff_manager_getter: at("mf:buff_manager_getter")?,
            player_stats_getter: at("mf:player_stats_getter")?,
            character_wallet_getter: at("wallet:wallet_manager")?,
            account_luck_level_getter: at("mf:account_luck_level_getter")?,

            inventory_owner_getter: at("bag:inventory_owner_getter")?,
            location3_index_getter: at("item:location3_index_getter")?,
            item_resolver: at("item:item_resolver")?,
            item_definition_getter: at("bag:item_definition_getter")?,
            item_instance_getter: at("item:item_instance_getter")?,
            item_location_getter: at("item:item_location_getter")?,
            final_stack_quantity_getter: at("item:final_stack_quantity_getter")?,
            stackable_at_0x98: at("item:stackable_at_0x98")?,
            stackable_at_0xd0: at("item:stackable_at_0xd0")?,
            null_stackable: at("item:null_stackable")?,
            stack_quantity_getter: at("item:stack_quantity_getter")?,
            conditional_stackable_a8: at("item:conditional_stackable_a8")?,
            conditional_stackable_e0: at("item:conditional_stackable_e0")?,
            conditional_predicate_a: at("item:conditional_predicate_A")?,
            conditional_predicate_b: at("item:conditional_predicate_B")?,
            conditional_predicate_c: at("item:conditional_predicate_C")?,

            bag_getter: at("bag:bag_getter")?,
            bag_size_getter: at("bag:bag_size_getter")?,
            bag_slot_count_getter: at("bag:bag_slot_count_getter")?,
            free_slots_getter: at("bag:free_slots_getter")?,
            used_slots_getter: at("bag:used_slots_getter")?,
            capacity_getter: at("bag:capacity_getter")?,
            slot_validity: at("bag:slot_validity")?,
            capacity_counter_text: at("bag:capacity_counter_text")?,

            balance_getter: at("wallet:balance_getter")?,
            hash_lookup: at("wallet:hash_lookup")?,

            modifier_value: at("mf:modifier_value")?,
            pushed_modifier_sum: at("mf:pushed_modifier_sum")?,
            buff_iterator: at("mf:buff_iterator")?,
            buff_filter: at("mf:buff_filter")?,
            buff_lookup: at("mf:buff_lookup")?,
            modifier_sum: at("mf:modifier_sum")?,
            modifier_formula: at("mf:modifier_formula")?,
            magic_find_case: at("mf:magic_find_case")?,
            attribute_modifier_map: at("mf:attribute_modifier_map")?,
        })
    }

    /// Each vtable with how far past it the readers read (its highest slot plus 8), and the
    /// relation that fails when it does not lie in `.rdata`.
    const fn vtable_extents(&self) -> [(u64, u64, &'static str); VTABLE_EXTENTS] {
        [
            (
                self.char_context_vtable,
                0x120,
                "char_context_vtable + 0x120 lies in rdata_vtables",
            ),
            (
                self.character_agent_vtable,
                0xd0,
                "character_agent_vtable + 0xd0 lies in rdata_vtables",
            ),
            (
                self.character_vtable,
                0x108,
                "character_vtable + 0x108 lies in rdata_vtables",
            ),
            (
                self.combatant_vtable,
                0x40,
                "combatant_vtable + 0x40 lies in rdata_vtables",
            ),
            (
                self.player_vtable,
                0x328,
                "player_vtable + 0x328 lies in rdata_vtables",
            ),
            (
                self.player_stats_vtable,
                0x38,
                "player_stats_vtable + 0x38 lies in rdata_vtables",
            ),
            (
                self.inventory_vtable,
                0x228,
                "inventory_vtable + 0x228 lies in rdata_vtables",
            ),
            (
                self.item_context_vtable,
                0x18,
                "item_context_vtable + 0x18 lies in rdata_vtables",
            ),
            (
                self.bag_item_vtable,
                0x10,
                "bag_item_vtable + 0x10 lies in rdata_vtables",
            ),
            (
                self.conditional_item_a_vtable,
                0x268,
                "conditional_item_a_vtable + 0x268 lies in rdata_vtables",
            ),
            (
                self.conditional_predicate_a_vtable,
                8,
                "conditional_predicate_a_vtable + 8 lies in rdata_vtables",
            ),
            (
                self.conditional_item_b_vtable,
                0x268,
                "conditional_item_b_vtable + 0x268 lies in rdata_vtables",
            ),
            (
                self.conditional_predicate_b_vtable,
                8,
                "conditional_predicate_b_vtable + 8 lies in rdata_vtables",
            ),
            (
                self.conditional_item_c_vtable,
                0x268,
                "conditional_item_c_vtable + 0x268 lies in rdata_vtables",
            ),
            (
                self.conditional_predicate_c_vtable,
                0x28,
                "conditional_predicate_c_vtable + 0x28 lies in rdata_vtables",
            ),
            (
                self.currency_manager_vtable,
                8,
                "currency_manager_vtable + 8 lies in rdata_vtables",
            ),
            (
                self.buff_manager_vtable,
                0x30,
                "buff_manager_vtable + 0x30 lies in rdata_vtables",
            ),
            (
                self.buff_node_vtable,
                8,
                "buff_node_vtable + 8 lies in rdata_vtables",
            ),
        ]
    }

    /// The functions the inventory reader compares by address, without a guard of a reader that
    /// would bound them, with how many of their bytes it reads (one when it reads none): each
    /// must lie before `.rdata`, which is where `.text` ends in this image.
    const fn compared_functions(&self) -> [(u64, u64); 18] {
        [
            (self.controlled_character_getter, 1),
            (self.character_inventory_getter, 1),
            (self.inventory_owner_getter, 1),
            (self.location3_index_getter, 1),
            (self.item_resolver, 1),
            (self.item_definition_getter, 1),
            (self.item_instance_getter, 1),
            (self.item_location_getter, 1),
            (self.final_stack_quantity_getter, 1),
            (self.stackable_at_0x98, 1),
            (self.stackable_at_0xd0, 1),
            // `xor eax, eax; ret`: the reader copies these 3 bytes.
            (self.null_stackable, 3),
            (self.stack_quantity_getter, 1),
            (self.conditional_stackable_a8, 1),
            (self.conditional_stackable_e0, 1),
            (self.conditional_predicate_a, 1),
            (self.conditional_predicate_b, 1),
            (self.conditional_predicate_c, 1),
        ]
    }

    /// Whether the relations the readers rely on hold. `Err` names the first that does not.
    ///
    /// They are the ones the readers relied on as constants, asserted at compile time then:
    ///
    /// - the image reaches the TLS index (`tls_index + 4 <= image_size`), which
    ///   `BuildProfile::checked` used to check against the image size alone;
    /// - `.rdata` is not empty, ends before the TLS index and so inside the image;
    /// - every vtable, up to its highest slot read, lies in `.rdata` and so before the TLS index
    ///   (`BAG_ITEM_VTABLE + 0x10 <= TLS_INDEX_RVA` and the like);
    /// - every guard range of the bag, wallet and Magic Find readers ends before the TLS index;
    /// - every function the inventory compares by address only lies before `.rdata`;
    /// - the three conditional item classes are three, and the five quantity getters of item slot
    ///   `0x260` are five: the inventory reader tells them apart by address, and two that were
    ///   one would read one class's stack with another's rule.
    ///
    /// None of them proves a layout right; that is the certificate's job and, for a moved build,
    /// the resolver's. They only refuse a layout under which a reader would read outside what it
    /// was written for.
    pub const fn validate(&self) -> Result<(), &'static str> {
        let rdata = &self.rdata_vtables;
        if self.tls_index.checked_add(4).is_none() || self.tls_index + 4 > self.image_size {
            return Err("tls_index + 4 lies in the image");
        }
        if rdata.start == 0 || rdata.start >= rdata.end {
            return Err("rdata_vtables is a non-empty range of the image");
        }
        if rdata.end > self.tls_index {
            return Err("rdata_vtables ends before tls_index");
        }
        let vtables = self.vtable_extents();
        let mut index = 0;
        while index < vtables.len() {
            let (vtable, extent, relation) = vtables[index];
            if vtable < rdata.start || vtable > rdata.end || extent > rdata.end - vtable {
                return Err(relation);
            }
            index += 1;
        }
        let guards = [
            (
                &crate::bags::guards_of(self) as &[crate::passive::Guard],
                "a bags guard ends before tls_index",
            ),
            (
                &crate::wallet::guards_of(self),
                "a wallet guard ends before tls_index",
            ),
            (
                &crate::magic_find::guards_of(self),
                "a magic_find guard ends before tls_index",
            ),
        ];
        let mut reader = 0;
        while reader < guards.len() {
            let (list, relation) = guards[reader];
            let mut index = 0;
            while index < list.len() {
                let guard = &list[index];
                if guard.rva == 0
                    || guard.rva > self.tls_index
                    || guard.size as u64 > self.tls_index - guard.rva
                {
                    return Err(relation);
                }
                index += 1;
            }
            reader += 1;
        }
        let functions = self.compared_functions();
        let mut index = 0;
        while index < functions.len() {
            let (function, extent) = functions[index];
            if function == 0 || function >= rdata.start || extent > rdata.start - function {
                return Err("an inventory function lies in .text, before rdata_vtables");
            }
            index += 1;
        }
        let classes = [
            self.conditional_item_a_vtable,
            self.conditional_item_b_vtable,
            self.conditional_item_c_vtable,
        ];
        if !distinct(&classes) {
            return Err("the three conditional item vtables are distinct");
        }
        let getters = [
            self.stackable_at_0x98,
            self.stackable_at_0xd0,
            self.null_stackable,
            self.conditional_stackable_a8,
            self.conditional_stackable_e0,
        ];
        if !distinct(&getters) {
            return Err("the five stack quantity getters are distinct");
        }
        Ok(())
    }
}

/// Whether no two of `values` are equal.
const fn distinct(values: &[u64]) -> bool {
    let mut a = 0;
    while a < values.len() {
        let mut b = a + 1;
        while b < values.len() {
            if values[a] == values[b] {
                return false;
            }
            b += 1;
        }
        a += 1;
    }
    true
}

// The certified layout holds every relation, and this is checked when the crate is built.
const _: () = assert!(Layout::CERTIFIED.validate().is_ok());

#[cfg(test)]
pub(crate) mod tests {
    use super::*;

    #[test]
    fn the_certified_layout_holds_every_relation() {
        assert_eq!(Layout::CERTIFIED.validate(), Ok(()));
    }

    #[test]
    fn the_certified_layout_is_the_certificate_s_image() {
        let layout = &Layout::CERTIFIED;
        assert_eq!(layout.image_size, 0x2c49000);
        assert_eq!(layout.rdata_vtables, 0x1914000..0x2553500);
        assert_eq!(crate::certificate::GAME.tls_index_rva, layout.tls_index);
    }

    /// The RVA of a file offset of the certified image, by its section table.
    fn rva_of(offset: u64) -> u64 {
        let section = SECTIONS
            .iter()
            .find(|s| {
                (s.file_offset as u64..s.file_offset as u64 + s.virtual_size as u64)
                    .contains(&offset)
            })
            .unwrap();
        offset - section.file_offset as u64 + section.rva as u64
    }
    /// The field of the layout the certificate names `name`, or `None` for what only the
    /// certificate checks (the item factory's code, which leads to the bag class).
    fn named(l: &Layout, name: &str) -> Option<u64> {
        Some(match name {
            "char_context" => l.char_context_vtable,
            "character_agent" => l.character_agent_vtable,
            "character" => l.character_vtable,
            "combatant" => l.combatant_vtable,
            "player (wallet character)" => l.player_vtable,
            "player_stats" => l.player_stats_vtable,
            "inventory" => l.inventory_vtable,
            "item_context" => l.item_context_vtable,
            "bag_item (ItCliBag)" => l.bag_item_vtable,
            "conditional item A" => l.conditional_item_a_vtable,
            "conditional predicate A" => l.conditional_predicate_a_vtable,
            "conditional item B" => l.conditional_item_b_vtable,
            "conditional predicate B" => l.conditional_predicate_b_vtable,
            "conditional item C" => l.conditional_item_c_vtable,
            "conditional predicate C" => l.conditional_predicate_c_vtable,
            "currency_manager" => l.currency_manager_vtable,
            "buff_manager" => l.buff_manager_vtable,
            "hash_table" => l.hash_table,
            "controlled_character_getter" => l.controlled_character_getter,
            "local_player_getter" | "local_character" => l.local_player_getter,
            "player_by_id_getter" => l.player_by_id_getter,
            "character_inventory_getter" => l.character_inventory_getter,
            "is_player" => l.is_player,
            "player_id_getter" => l.player_id_getter,
            "combatant_cast" => l.combatant_cast,
            "buff_manager_getter" => l.buff_manager_getter,
            "player_stats_getter" => l.player_stats_getter,
            "wallet_manager" => l.character_wallet_getter,
            "account_luck_level_getter" => l.account_luck_level_getter,
            "inventory_owner_getter" => l.inventory_owner_getter,
            "location3_index_getter" => l.location3_index_getter,
            "item_resolver" => l.item_resolver,
            "item_definition_getter" => l.item_definition_getter,
            "item_instance_getter" => l.item_instance_getter,
            "item_location_getter" => l.item_location_getter,
            "final_stack_quantity_getter" => l.final_stack_quantity_getter,
            "stackable_at_0x98" => l.stackable_at_0x98,
            "stackable_at_0xd0" => l.stackable_at_0xd0,
            "null_stackable" => l.null_stackable,
            "stack_quantity_getter" => l.stack_quantity_getter,
            "conditional_stackable_a8" => l.conditional_stackable_a8,
            "conditional_stackable_e0" => l.conditional_stackable_e0,
            "conditional_predicate_A" => l.conditional_predicate_a,
            "conditional_predicate_B" => l.conditional_predicate_b,
            "conditional_predicate_C" => l.conditional_predicate_c,
            "bag_getter" => l.bag_getter,
            "bag_size_getter" => l.bag_size_getter,
            "bag_slot_count_getter" => l.bag_slot_count_getter,
            "free_slots_getter" => l.free_slots_getter,
            "used_slots_getter" => l.used_slots_getter,
            "capacity_getter" => l.capacity_getter,
            "slot_validity" => l.slot_validity,
            "capacity_counter_text" => l.capacity_counter_text,
            "balance_getter" => l.balance_getter,
            "hash_lookup" => l.hash_lookup,
            "modifier_value" => l.modifier_value,
            "pushed_modifier_sum" => l.pushed_modifier_sum,
            "buff_iterator" => l.buff_iterator,
            "buff_filter" => l.buff_filter,
            "buff_lookup" => l.buff_lookup,
            "modifier_sum" => l.modifier_sum,
            "modifier_formula" => l.modifier_formula,
            "magic_find_case" => l.magic_find_case,
            "attribute_modifier_map" => l.attribute_modifier_map,
            "item_factory_type_switch"
            | "item_factory_bag_case"
            | "item_factory_table_bag_entry"
            | "bag_item_constructor" => return None,
            _ => panic!("the certificate names {name}, which the layout does not know"),
        })
    }

    /// Every range and every slot the certificate checks in the file is where the certified
    /// layout says: the layout is what the certificate proves, field by field.
    #[test]
    fn the_certified_layout_is_what_the_certificate_checks() {
        let l = &Layout::CERTIFIED;
        let mut fields = 0;
        for span in crate::certificate::SPANS {
            for part in span.name.split(" + ") {
                let (_, name) = part.split_once(':').unwrap();
                if let Some(rva) = named(l, name) {
                    assert_eq!(rva, rva_of(span.offset), "{}", span.name);
                    fields += 1;
                }
            }
        }
        for slot in crate::certificate::SLOTS {
            let (class, at) = slot
                .name
                .strip_prefix("vtable ")
                .unwrap()
                .rsplit_once(" +")
                .unwrap();
            let at = u64::from_str_radix(at.trim_start_matches("0x"), 16).unwrap();
            let vtable = named(l, class).unwrap();
            assert_eq!(vtable + at, rva_of(slot.offset), "{}", slot.name);
            assert!(
                l.vtable_extents()
                    .iter()
                    .any(|(rva, extent, _)| *rva == vtable && at + 8 <= *extent),
                "{}: a slot past the extent validate checks",
                slot.name
            );
        }
        // The 47 ranges of the certificate that are a field of the layout, three of them named
        // twice because two readers share them.
        assert_eq!(fields, 47 + 3);
    }

    /// What the resolver gives for the certified build by identity: every anchor and vtable of
    /// the game's table at the RVA it was certified at, and the certificate's image. A span of
    /// the certificate carries, as the file's digest there, the certificate's own: a file that
    /// holds the certificate has those bytes there.
    pub(crate) fn identity() -> Resolved {
        use crate::resolver::{Mode, Place, GAME};
        let span = |name: &str| {
            crate::certificate::SPANS.iter().find(|s| s.name == name).map(|s| {
                let mut digest = [0u8; 32];
                for (i, byte) in digest.iter_mut().enumerate() {
                    *byte = u8::from_str_radix(&s.sha256[2 * i..2 * i + 2], 16).unwrap();
                }
                digest
            })
        };
        let place = |name: &'static str, rva: u32| Place { name, rva: Some(u64::from(rva)), how: None, file_sha256: span(name) };
        Resolved {
            mode: Mode::Identity,
            image_size: IMAGE_SIZE,
            tls_index: crate::certificate::GAME.tls_index_rva,
            rdata: (
                u64::from(SECTIONS[RDATA].rva),
                u64::from(SECTIONS[RDATA].rva) + u64::from(SECTIONS[RDATA].virtual_size),
            ),
            anchors: GAME.anchors.iter().map(|a| place(a.name, a.rva)).collect(),
            vtables: GAME.vtables.iter().map(|v| place(v.name, v.rva)).collect(),
            search: None,
        }
    }

    /// Each field of a layout and its value, as `Debug` writes them: every field, without a list
    /// of them that could leave one out.
    fn fields(layout: &Layout) -> Vec<(String, String)> {
        format!("{layout:#?}")
            .lines()
            .filter_map(|line| line.trim().trim_end_matches(',').split_once(": "))
            .map(|(name, value)| (name.to_string(), value.to_string()))
            .collect()
    }

    /// The resolution of identity gives the certified layout, field by field.
    #[test]
    fn the_layout_of_the_identity_resolution_is_the_certified_one_field_by_field() {
        let layout = Layout::from_resolved(&identity()).unwrap();
        let (got, certified) = (fields(&layout), fields(&Layout::CERTIFIED));
        assert_eq!(got.len(), 68);
        for (field, expected) in got.iter().zip(&certified) {
            assert_eq!(field, expected);
        }
        assert_eq!(layout, Layout::CERTIFIED);
    }

    /// Every one of the 68 fields comes from the resolution, each from one name of its own:
    /// moving one anchor or vtable of the resolution, or its image size, TLS index or `.rdata`,
    /// moves exactly one field, and every field is moved by one of them. A field left at a
    /// constant would be moved by none; two fields read from one name, by the same one.
    #[test]
    fn every_field_of_the_layout_comes_from_one_place_of_the_resolution() {
        let base = fields(&Layout::from_names(&identity()).unwrap());
        let mut moved_by: Vec<Vec<String>> = vec![Vec::new(); base.len()];
        let mut check = |what: String, resolved: Resolved| {
            let changed: Vec<usize> = fields(&Layout::from_names(&resolved).unwrap())
                .iter()
                .zip(&base)
                .enumerate()
                .filter(|(_, (a, b))| a != b)
                .map(|(i, _)| i)
                .collect();
            assert!(changed.len() <= 1, "{what} moves {} fields", changed.len());
            for i in changed {
                moved_by[i].push(what.clone());
            }
        };
        let names = crate::resolver::GAME.names();
        for name in &names {
            let mut resolved = identity();
            let place = resolved
                .anchors
                .iter_mut()
                .chain(resolved.vtables.iter_mut())
                .find(|p| *name == p.name || name.strip_prefix("vtable ") == Some(p.name))
                .unwrap();
            place.rva = place.rva.map(|rva| rva + 0x10);
            check(name.clone(), resolved);
        }
        let mut resolved = identity();
        resolved.image_size += 0x1000;
        check("image size".into(), resolved);
        let mut resolved = identity();
        resolved.tls_index += 8;
        check("TLS index".into(), resolved);
        let mut resolved = identity();
        resolved.rdata.1 += 0x1000;
        check(".rdata".into(), resolved);
        for ((field, _), by) in base.iter().zip(&moved_by) {
            assert_eq!(by.len(), 1, "{field} is moved by {by:?}");
        }
        assert_eq!(base.len(), 68);
    }

    /// A name the table does not have, or a place the resolution did not give, is no layout:
    /// never one with a hole, or with the certified value standing in for it.
    #[test]
    fn a_layout_is_only_made_of_a_resolution_that_placed_every_field() {
        let mut resolved = identity();
        resolved.anchors.iter_mut().find(|p| p.name == "wallet:hash_lookup").unwrap().rva = None;
        assert_eq!(Layout::from_resolved(&resolved), Err("wallet:hash_lookup"));
        let mut resolved = identity();
        resolved.vtables.retain(|p| p.name != "buff_node");
        assert_eq!(Layout::from_resolved(&resolved), Err("vtable buff_node"));
        // A resolution with none of the game's names, as the resolver's synthetic table gives.
        let mut resolved = identity();
        resolved.anchors.clear();
        resolved.vtables.clear();
        assert_eq!(Layout::from_resolved(&resolved), Err("vtable char_context"));
        // Every place given, and a relation broken: refused with the relation.
        let mut resolved = identity();
        resolved.rdata = (0, 0);
        assert_eq!(Layout::from_resolved(&resolved), Err("rdata_vtables is a non-empty range of the image"));
    }

    /// Each relation, broken on its own in an otherwise certified layout, is the one named.
    #[test]
    fn a_broken_layout_is_refused_with_the_relation_it_breaks() {
        let certified = Layout::CERTIFIED;
        let broken: [(Layout, &str); 9] = [
            (
                Layout {
                    image_size: certified.tls_index + 3,
                    ..certified.clone()
                },
                "tls_index + 4 lies in the image",
            ),
            (
                Layout {
                    rdata_vtables: 0x2553500..0x1914000,
                    ..certified.clone()
                },
                "rdata_vtables is a non-empty range of the image",
            ),
            (
                // The TLS index before the vtables: what the compile-time asserts used to refuse.
                Layout {
                    tls_index: 0x2000000,
                    ..certified.clone()
                },
                "rdata_vtables ends before tls_index",
            ),
            (
                Layout {
                    bag_item_vtable: certified.rdata_vtables.end - 8,
                    ..certified.clone()
                },
                "bag_item_vtable + 0x10 lies in rdata_vtables",
            ),
            (
                Layout {
                    inventory_vtable: 0x13c4930,
                    ..certified.clone()
                },
                "inventory_vtable + 0x228 lies in rdata_vtables",
            ),
            (
                Layout {
                    hash_table: certified.tls_index - 1000,
                    ..certified.clone()
                },
                "a wallet guard ends before tls_index",
            ),
            (
                Layout {
                    null_stackable: certified.rdata_vtables.start - 2,
                    ..certified.clone()
                },
                "an inventory function lies in .text, before rdata_vtables",
            ),
            (
                Layout {
                    conditional_item_c_vtable: certified.conditional_item_a_vtable,
                    ..certified.clone()
                },
                "the three conditional item vtables are distinct",
            ),
            (
                Layout {
                    conditional_stackable_e0: certified.stackable_at_0xd0,
                    ..certified.clone()
                },
                "the five stack quantity getters are distinct",
            ),
        ];
        for (layout, relation) in broken {
            assert_eq!(layout.validate(), Err(relation));
        }
    }
}
