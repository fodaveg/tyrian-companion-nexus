//! A build whose code moved, made from the certified one: what a game update that only moves
//! code and data does to what the readers rely on.
//!
//! [`layout`] is `Layout::CERTIFIED` with every RVA moved by the delta of its section: `.text`
//! [`TEXT`], `.rdata` [`RDATA`] and what lies past `.rdata` (the TLS index) [`DATA`]. [`bytes`]
//! moves the bytes of a fixture of the certified build the same way: everything inside the image
//! goes where its section went, and every pointer into the image, wherever it is stored (an
//! object's vtable, a vtable's slot, a stack's class), points where its target went. The heap is
//! left where it is, as an update leaves the game's objects.
//!
//! A reader that takes every RVA from its profile's layout reads, over the moved bytes with the
//! moved layout, exactly what it reads over the certified bytes with `Layout::CERTIFIED`. One
//! that still reads one RVA as a constant of the certified build reads the old place, where the
//! moved bytes hold nothing, and fails.
use std::collections::{BTreeMap, BTreeSet};
use tyrian_companion_nexus_core::layout::Layout;
#[allow(unused_imports)]
use tyrian_companion_nexus_core::{
    certificate::SPANS,
    passive::Guard,
    relocation::FieldKind,
    resolver::{Mode, Place, Resolved, GAME},
    sha256::sha256,
};

pub const BASE: u64 = 0x140000000;
/// The deltas of the three sections. They differ, so that a reader that took an RVA of one
/// section for one of another would not land on the right place by chance.
pub const TEXT: u64 = 0x1000;
pub const RDATA: u64 = 0x2000;
pub const DATA: u64 = 0x3000;

/// Where the moved build keeps what the certified one keeps at `rva`.
pub fn moved(rva: u64) -> u64 {
    let rdata = &Layout::CERTIFIED.rdata_vtables;
    if rva < rdata.start {
        rva + TEXT
    } else if rva <= rdata.end {
        rva + RDATA
    } else {
        rva + DATA
    }
}

/// The moved build's layout. Every field is named, so a field added to `Layout` does not build
/// here until this says where the moved build keeps it.
pub fn layout() -> &'static Layout {
    let l = &Layout::CERTIFIED;
    let m = moved;
    Box::leak(Box::new(Layout {
        image_size: l.image_size + DATA,
        tls_index: m(l.tls_index),
        rdata_vtables: m(l.rdata_vtables.start)..m(l.rdata_vtables.end),
        char_context_vtable: m(l.char_context_vtable),
        character_agent_vtable: m(l.character_agent_vtable),
        character_vtable: m(l.character_vtable),
        combatant_vtable: m(l.combatant_vtable),
        player_vtable: m(l.player_vtable),
        player_stats_vtable: m(l.player_stats_vtable),
        inventory_vtable: m(l.inventory_vtable),
        item_context_vtable: m(l.item_context_vtable),
        bag_item_vtable: m(l.bag_item_vtable),
        conditional_item_a_vtable: m(l.conditional_item_a_vtable),
        conditional_predicate_a_vtable: m(l.conditional_predicate_a_vtable),
        conditional_item_b_vtable: m(l.conditional_item_b_vtable),
        conditional_predicate_b_vtable: m(l.conditional_predicate_b_vtable),
        conditional_item_c_vtable: m(l.conditional_item_c_vtable),
        conditional_predicate_c_vtable: m(l.conditional_predicate_c_vtable),
        currency_manager_vtable: m(l.currency_manager_vtable),
        buff_manager_vtable: m(l.buff_manager_vtable),
        buff_node_vtable: m(l.buff_node_vtable),
        hash_table: m(l.hash_table),
        controlled_character_getter: m(l.controlled_character_getter),
        local_player_getter: m(l.local_player_getter),
        player_by_id_getter: m(l.player_by_id_getter),
        character_inventory_getter: m(l.character_inventory_getter),
        is_player: m(l.is_player),
        player_id_getter: m(l.player_id_getter),
        combatant_cast: m(l.combatant_cast),
        buff_manager_getter: m(l.buff_manager_getter),
        player_stats_getter: m(l.player_stats_getter),
        character_wallet_getter: m(l.character_wallet_getter),
        account_luck_level_getter: m(l.account_luck_level_getter),
        inventory_owner_getter: m(l.inventory_owner_getter),
        location3_index_getter: m(l.location3_index_getter),
        item_resolver: m(l.item_resolver),
        item_definition_getter: m(l.item_definition_getter),
        item_instance_getter: m(l.item_instance_getter),
        item_location_getter: m(l.item_location_getter),
        final_stack_quantity_getter: m(l.final_stack_quantity_getter),
        stackable_at_0x98: m(l.stackable_at_0x98),
        stackable_at_0xd0: m(l.stackable_at_0xd0),
        null_stackable: m(l.null_stackable),
        stack_quantity_getter: m(l.stack_quantity_getter),
        conditional_stackable_a8: m(l.conditional_stackable_a8),
        conditional_stackable_e0: m(l.conditional_stackable_e0),
        conditional_predicate_a: m(l.conditional_predicate_a),
        conditional_predicate_b: m(l.conditional_predicate_b),
        conditional_predicate_c: m(l.conditional_predicate_c),
        bag_getter: m(l.bag_getter),
        bag_size_getter: m(l.bag_size_getter),
        bag_slot_count_getter: m(l.bag_slot_count_getter),
        free_slots_getter: m(l.free_slots_getter),
        used_slots_getter: m(l.used_slots_getter),
        capacity_getter: m(l.capacity_getter),
        slot_validity: m(l.slot_validity),
        capacity_counter_text: m(l.capacity_counter_text),
        balance_getter: m(l.balance_getter),
        hash_lookup: m(l.hash_lookup),
        modifier_value: m(l.modifier_value),
        pushed_modifier_sum: m(l.pushed_modifier_sum),
        buff_iterator: m(l.buff_iterator),
        buff_filter: m(l.buff_filter),
        buff_lookup: m(l.buff_lookup),
        modifier_sum: m(l.modifier_sum),
        modifier_formula: m(l.modifier_formula),
        magic_find_case: m(l.magic_find_case),
        attribute_modifier_map: m(l.attribute_modifier_map),
    }))
}

/// The anchor of the game's relocation table that one of a reader's guards is: the one whose
/// name has `prefix:name` as one of its parts, as `executable::relocated` finds it.
#[allow(dead_code)]
fn anchor_of(prefix: &str, name: &str) -> &'static tyrian_companion_nexus_core::relocation::Anchor {
    let part = format!("{prefix}:{name}");
    GAME.anchors
        .iter()
        .find(|a| a.name.split(" + ").any(|p| p == part))
        .unwrap_or_else(|| panic!("no anchor is {part}"))
}

/// The four bytes at `address`, as a little-endian dword; a byte the fixture does not have is 0.
#[allow(dead_code)]
fn dword(bytes: &BTreeMap<u64, u8>, address: u64) -> u32 {
    u32::from_le_bytes(std::array::from_fn(|i| *bytes.get(&(address + i as u64)).unwrap_or(&0)))
}

/// Rewrites, in `bytes` (a fixture already moved by [`bytes`]), the fields the relocation table
/// masks in each of `guards` (the certified ones of a reader, named `prefix:` in the table), as
/// moving the code rewrites them: a rel32 or rip-relative field points where its target went
/// from where the field went, an image-relative one names where its target went. A field whose
/// target moved by the same delta as the field keeps its bytes; one into another section does
/// not. How many of the guards' ranges changed is returned.
///
/// This is what [`bytes`] leaves out: it moves code without touching it, so a guard's bytes, and
/// their digest, stay the certified ones. A real update rewrites them, and the readers must then
/// compare memory with the digest of the file and not with the certified one.
#[allow(dead_code)]
pub fn rewrite_fields(bytes: &mut BTreeMap<u64, u8>, guards: &[Guard], prefix: &str) -> usize {
    let mut changed = 0;
    for guard in guards {
        let anchor = anchor_of(prefix, guard.name);
        assert_eq!((u64::from(anchor.rva), anchor.len as usize), (guard.rva, guard.size), "{prefix}:{}", guard.name);
        let before: Vec<Option<u8>> = (0..guard.size as u64).map(|i| bytes.get(&(BASE + moved(guard.rva) + i)).copied()).collect();
        for field in anchor.fields {
            let site = guard.rva + u64::from(field.at);
            let value = dword(bytes, BASE + moved(site));
            let new = match field.kind {
                FieldKind::Rel32 | FieldKind::Rip => {
                    let end = site + 4 + u64::from(field.tail);
                    let target = end.wrapping_add(i64::from(value as i32) as u64) & 0xffff_ffff;
                    (moved(target) as i64 - (moved(site) + 4 + u64::from(field.tail)) as i64) as i32 as u32
                }
                FieldKind::ImageRel | FieldKind::JumpTable => moved(u64::from(value)) as u32,
            };
            for (i, byte) in new.to_le_bytes().iter().enumerate() {
                bytes.insert(BASE + moved(site) + i as u64, *byte);
            }
        }
        let after: Vec<Option<u8>> = (0..guard.size as u64).map(|i| bytes.get(&(BASE + moved(guard.rva) + i)).copied()).collect();
        changed += usize::from(before != after);
    }
    changed
}

/// What the resolver gives for the moved build: every anchor and vtable of the game's table
/// where [`moved`] puts it, and its image. An anchor carries, as the digest of the file there,
/// that of the bytes `bytes` holds over its whole range, or, for one they do not hold, the
/// certificate's digest of it: the moved file holds that anchor's certified bytes unchanged.
#[allow(dead_code)]
pub fn resolved(bytes: &BTreeMap<u64, u8>) -> Resolved {
    let l = &Layout::CERTIFIED;
    let digest_of = |name: &str, rva: u64, len: u32| {
        let range: Option<Vec<u8>> = (0..u64::from(len)).map(|i| bytes.get(&(BASE + moved(rva) + i)).copied()).collect();
        range.map(|range| sha256(&range)).or_else(|| {
            SPANS.iter().find(|s| s.name == name).map(|s| std::array::from_fn(|i| u8::from_str_radix(&s.sha256[2 * i..2 * i + 2], 16).unwrap()))
        })
    };
    Resolved {
        mode: Mode::Relocated,
        image_size: (l.image_size + DATA) as u32,
        tls_index: moved(l.tls_index),
        rdata: (moved(l.rdata_vtables.start), moved(l.rdata_vtables.end)),
        anchors: GAME
            .anchors
            .iter()
            .map(|a| Place {
                name: a.name,
                rva: Some(moved(u64::from(a.rva))),
                how: None,
                file_sha256: digest_of(a.name, u64::from(a.rva), a.len),
            })
            .collect(),
        vtables: GAME
            .vtables
            .iter()
            .map(|v| Place { name: v.name, rva: Some(moved(u64::from(v.rva))), how: None, file_sha256: None })
            .collect(),
        search: None,
    }
}

/// The bytes of a fixture of the certified build, as the moved build holds them.
///
/// A pointer into the image is a whole 8-byte value the fixture put (`words`, where it put
/// them) and that still lies in the image. Not any 8 bytes that do: a float of 2 to 97 followed
/// by a dword 1, as in a pushed Magic Find record, reads as an address of the image, and moving
/// it would change the record. A mistake here can only turn a test red: the moved bytes would
/// then not be the ones the certified build holds, moved.
pub fn bytes(certified: &BTreeMap<u64, u8>, words: &BTreeSet<u64>) -> BTreeMap<u64, u8> {
    let image = BASE..BASE + Layout::CERTIFIED.image_size;
    let place = |address: u64| {
        if image.contains(&address) {
            BASE + moved(address - BASE)
        } else {
            address
        }
    };
    let mut out: BTreeMap<u64, u8> = certified
        .iter()
        .map(|(address, byte)| (place(*address), *byte))
        .collect();
    for address in words {
        let word = u64::from_le_bytes(std::array::from_fn(|i| {
            *certified.get(&(address + i as u64)).unwrap_or(&0)
        }));
        if image.contains(&word) {
            // A pointer lies in one section, so its 8 bytes move together.
            for (i, byte) in place(word).to_le_bytes().iter().enumerate() {
                out.insert(place(*address) + i as u64, *byte);
            }
        }
    }
    out
}
