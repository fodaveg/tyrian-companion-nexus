//! The relocation table: what lets the readers find, in an update of the game that moved its
//! code, the anchors the certificate ([`crate::certificate`]) checks at fixed offsets.
//!
//! It is the output of `scripts/relocation/gen_relocation.py`, which also resolves an executable
//! with it offline (the semiautomatic recertification), and the tests below hold it to what the
//! certificate says. [`crate::resolver`] is the same resolver in the addon; the readers go by it
//! when a build does not hold the certificate at its fixed offsets ([`crate::executable`]).
//!
//! The table keeps no byte of the game, as the certificate keeps none. For each anchor (the 57
//! spans of the certificate, the constructors that install the vtables, and the context getter
//! that reads the TLS index) it has:
//!
//! - where it was certified (RVA, length) and how many places of the certified build matched it
//!   ([`Anchor::certified_matches`]): only an anchor that matched once is taken from a search
//!   (or one the field checks picked among its matches); one of which several matches pass
//!   every field check is not searched, and is derived from a field or a slot that names it;
//! - the positions of the fields that move with the code (rel32 branches, rip-relative and
//!   image-relative displacements, jump-table entries), never their bytes, and where each one
//!   pointed, as an identity ([`Target`]): another anchor, a vtable, a callee fingerprinted by
//!   its length and normalized digest, a digest of `.rdata` content, an offset inside the anchor
//!   or its function, the image base or the TLS index;
//! - the SHA-256 of its bytes with those fields set to zero ([`Anchor::normalized_sha256`]);
//! - for the anchors that are searched, a 32-bit rolling hash of one window of `k` >= 16 bytes
//!   with no field in it ([`Search`]). 32 bits of a 128-bit window: about 2^96 windows share
//!   each value, so it locates candidates without saying what the bytes are; the normalized
//!   digest confirms each one.
//!
//! The vtables ([`VTABLES`]) are located by the lea of a constructor and checked by their slots.
//! The table is generated (`relocation_data.rs`): a change of it is a recertification, not an
//! edit. The rules the resolver applies are in `scripts/relocation/README.md`.

/// Which section an anchor lies in.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Section {
    Text,
    Rdata,
}

/// What an anchor is to the readers.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AnchorKind {
    /// A span of the certificate: code or a table the readers rely on.
    Span,
    /// A function that installs one or more vtables with a rip-relative lea. Only a way to
    /// those vtables: one that is not found does not fail the resolution if they are located.
    Constructor,
    /// The context getter that reads the TLS index the context route uses.
    TlsGetter,
}

/// The window of an anchor that is searched for by its rolling hash.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Search {
    /// Length of the window, in bytes.
    pub k: u8,
    /// Where the window starts inside the anchor.
    pub at: u32,
    /// [`rolling_hash`] of the window.
    pub hash: u32,
}

/// Where an anchor lies inside its `.pdata` function, and that function's length.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Within {
    pub offset: u32,
    pub len: u32,
}

/// How a field encodes where it points.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FieldKind {
    /// rel32 of a call, jmp or jcc: the target is the end of the field plus the value.
    Rel32,
    /// rip-relative disp32: the target is the end of the field, plus `tail` bytes of immediate,
    /// plus the value.
    Rip,
    /// disp32 off a register that holds the image base: the value is the target's RVA.
    ImageRel,
    /// A jump-table entry: the value is the target's RVA.
    JumpTable,
}

/// What a field pointed at in the certified build, which is what it must point at now.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Target {
    /// Anchor `anchor`, which starts `add` bytes from where the field points.
    Anchor { anchor: usize, add: i32 },
    /// The vtable of that index in [`VTABLES`].
    Vtable(usize),
    /// The image base (RVA 0).
    ImageBase,
    /// The TLS index the TLS directory names.
    TlsIndex,
    /// That many bytes into the anchor itself.
    Internal(u32),
    /// That many bytes into the `.pdata` function that holds the anchor.
    SameFunction(u32),
    /// The callee of that index in [`CALLEES`].
    Callee(usize),
    /// The `.rdata` content of that index in [`CONTENTS`].
    Content(usize),
    /// Somewhere in `.text`: a constructor's call that is not what the readers rely on.
    Code,
    /// Somewhere in `.rdata`: a constructor's data that is not what the readers rely on.
    Data,
}

/// A field that moves with the code: 4 bytes at `at`, zeroed in the normalized digest.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Field {
    pub at: u32,
    pub kind: FieldKind,
    pub tail: u8,
    pub target: Target,
}

/// One anchor of the table.
#[derive(Debug, Clone, Copy)]
pub struct Anchor {
    pub name: &'static str,
    pub kind: AnchorKind,
    pub section: Section,
    /// Where it was certified.
    pub rva: u32,
    pub len: u32,
    /// The alignment a place must have to be it (8 for the `.rdata` table).
    pub align: u32,
    pub normalized_sha256: &'static str,
    /// How many places of its section matched its normalized signature in the certified build;
    /// `None` when every byte is a field (it is only derived).
    pub certified_matches: Option<u32>,
    /// For an anchor that matched more than once: how many of those places passed every field
    /// check. One means the field checks pick it among the matches; more than one, that no
    /// search can place it (it has no [`Search`] and is only derived).
    pub certified_survivors: Option<u32>,
    pub search: Option<Search>,
    pub function: Option<Within>,
    pub fields: &'static [Field],
}

/// A vtable slot that must hold `image base + RVA` of the located `anchor`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct VtableSlot {
    pub offset: u32,
    pub anchor: usize,
    /// Not a check of the certificate: one of the slots the relocation study added to locate a
    /// function that has no signature of its own (`null_stackable`, `stack_quantity_getter`).
    pub added: bool,
}

#[derive(Debug, Clone, Copy)]
pub struct Vtable {
    pub name: &'static str,
    pub rva: u32,
    pub slots: &'static [VtableSlot],
}

/// How the extent of a callee is known.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Bound {
    /// Its own `.pdata` function.
    Function,
    /// `offset` bytes into a `.pdata` function, which is the one fingerprinted.
    Inside,
    /// A leaf with no `.pdata` entry, up to its first `ret` or `jmp`.
    Leaf,
}

/// A function a span calls or jumps to that is not an anchor: its fingerprint.
#[derive(Debug, Clone, Copy)]
pub struct Callee {
    pub rva: u32,
    pub bound: Bound,
    pub offset: u32,
    pub len: u32,
    pub normalized_sha256: &'static str,
    /// The positions of its own relocatable fields, zeroed in the digest.
    pub mask: &'static [u32],
}

/// `.rdata` bytes a span reads by address, by digest. The 8-byte absolute pointers among them
/// (base relocations, at `pointers` from the start) are zeroed in the digest.
#[derive(Debug, Clone, Copy)]
pub struct Content {
    pub rva: u32,
    pub len: u32,
    pub sha256: &'static str,
    pub pointers: &'static [i32],
}

include!("relocation_data.rs");

/// The rolling hash of a window: `sum(w[j] * ROLLING_BASE^(len-1-j)) mod ROLLING_PRIME`, the one
/// the table's [`Search::hash`] holds.
pub fn rolling_hash(window: &[u8]) -> u32 {
    let (p, b) = (u64::from(ROLLING_PRIME), u64::from(ROLLING_BASE));
    window.iter().fold(0u64, |h, &x| (h * b + u64::from(x)) % p) as u32
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::certificate;
    use std::collections::BTreeSet;

    fn rva_of(offset: u64) -> u32 {
        let s = certificate::SECTIONS
            .iter()
            .find(|s| offset >= u64::from(s.file_offset) && offset < u64::from(s.file_offset) + u64::from(s.virtual_size))
            .expect("offset in a section");
        (offset - u64::from(s.file_offset)) as u32 + s.rva
    }

    #[test]
    fn the_table_was_taken_with_the_certificate_in_force() {
        assert_eq!(CERTIFICATE, certificate::CERTIFICATE);
        assert_eq!(IMAGE_BASE, certificate::IMAGE_BASE);
        assert_eq!(u64::from(TLS_INDEX_RVA), crate::inventory::TLS_INDEX_RVA);
        let names: Vec<&str> = certificate::SECTIONS
            .iter()
            .map(|s| std::str::from_utf8(&s.name).unwrap().trim_end_matches('\0'))
            .collect();
        assert_eq!(names, SECTION_NAMES);
    }

    #[test]
    fn every_span_of_the_certificate_is_an_anchor_where_it_was_certified() {
        assert_eq!(certificate::SPANS.len(), 57);
        for span in certificate::SPANS.iter() {
            let a = ANCHORS.iter().find(|a| a.name == span.name).unwrap_or_else(|| panic!("{}", span.name));
            assert_eq!((a.kind, a.rva, a.len), (AnchorKind::Span, rva_of(span.offset), span.len), "{}", span.name);
        }
    }

    #[test]
    fn every_slot_of_the_certificate_is_a_slot_of_its_vtable() {
        for slot in certificate::SLOTS.iter() {
            let (name, offset) = slot.name.strip_prefix("vtable ").unwrap().rsplit_once(" +0x").unwrap();
            let offset = u32::from_str_radix(offset, 16).unwrap();
            let v = VTABLES.iter().find(|v| v.name == name).unwrap_or_else(|| panic!("{name}"));
            assert_eq!(rva_of(slot.offset), v.rva + offset, "{}", slot.name);
            let s = v.slots.iter().find(|s| s.offset == offset).unwrap_or_else(|| panic!("{}", slot.name));
            assert_eq!((u64::from(ANCHORS[s.anchor].rva), s.added), (slot.rva, false), "{}", slot.name);
        }
        let added: Vec<_> = VTABLES
            .iter()
            .flat_map(|v| v.slots.iter().filter(|s| s.added).map(move |s| (v.name, s.offset, ANCHORS[s.anchor].name)))
            .collect();
        assert_eq!(
            added,
            [
                ("player_stats", 0x260, "item:stack_quantity_getter"),
                ("bag_item (ItCliBag)", 0x260, "item:null_stackable"),
                ("conditional predicate C", 0x28, "item:stack_quantity_getter"),
            ]
        );
    }

    #[test]
    fn every_index_holds_and_every_field_lies_inside_its_anchor_apart_from_the_window() {
        let mut names = BTreeSet::new();
        for a in ANCHORS.iter() {
            assert!(names.insert(a.name), "{}", a.name);
            assert_eq!(a.normalized_sha256.len(), 64, "{}", a.name);
            let mut end = 0;
            for f in a.fields {
                assert!(f.at >= end && f.at + 4 <= a.len, "{} +{}", a.name, f.at);
                end = f.at + 4;
                match f.target {
                    Target::Anchor { anchor, .. } => assert!(anchor < ANCHORS.len() && anchor != ANCHORS.iter().position(|b| b.name == a.name).unwrap()),
                    Target::Vtable(i) => assert!(i < VTABLES.len()),
                    Target::Callee(i) => assert!(i < CALLEES.len() && a.kind != AnchorKind::Constructor),
                    Target::Content(i) => assert!(i < CONTENTS.len() && a.kind != AnchorKind::Constructor),
                    Target::Internal(at) => assert!(at < a.len),
                    Target::SameFunction(at) => assert!(at < a.function.expect("a function").len),
                    Target::Code | Target::Data => assert_eq!(a.kind, AnchorKind::Constructor, "{}", a.name),
                    Target::ImageBase | Target::TlsIndex => {}
                }
            }
            if let Some(s) = a.search {
                // At least 16 bytes, all of them outside the fields, and only for what matched
                // once or what the field checks picked among its matches.
                assert!(s.k >= 16 && s.at + u32::from(s.k) <= a.len, "{}", a.name);
                assert!(a.fields.iter().all(|f| f.at + 4 <= s.at || f.at >= s.at + u32::from(s.k)), "{}", a.name);
                assert!(a.certified_matches == Some(1) || a.certified_survivors == Some(1), "{}", a.name);
            }
            if a.kind == AnchorKind::Constructor {
                assert!(a.search.is_some(), "{}", a.name);
            }
        }
        for c in CALLEES.iter() {
            assert!(c.mask.iter().all(|&p| p + 4 <= c.len) && c.normalized_sha256.len() == 64);
            assert!(c.offset == 0 || c.bound == Bound::Inside);
        }
        for c in CONTENTS.iter() {
            assert!(c.len > 0 && c.sha256.len() == 64);
        }
        for v in VTABLES.iter() {
            assert!(v.slots.iter().all(|s| s.anchor < ANCHORS.len()), "{}", v.name);
        }
    }

    #[test]
    fn every_vtable_has_a_constructor_lea_and_the_tls_getter_reads_the_index() {
        for (i, v) in VTABLES.iter().enumerate() {
            assert!(
                ANCHORS.iter().any(|a| a.fields.iter().any(|f| f.target == Target::Vtable(i))),
                "{}",
                v.name
            );
        }
        let getter = &ANCHORS[TLS_GETTER];
        assert_eq!(getter.kind, AnchorKind::TlsGetter);
        assert!(getter.fields.iter().any(|f| f.target == Target::TlsIndex));
    }

    #[test]
    fn what_no_search_places_is_named_by_a_field_or_a_slot() {
        // An anchor with no search (no window free of fields, or several certified places that
        // pass every field check, as the array reserve of Array.h) is only derived: a field of
        // another anchor or a vtable slot must name it.
        for (i, a) in ANCHORS.iter().enumerate() {
            if a.search.is_some() {
                continue;
            }
            let by_field =
                ANCHORS.iter().any(|b| b.fields.iter().any(|f| matches!(f.target, Target::Anchor { anchor, .. } if anchor == i)));
            let by_slot = VTABLES.iter().any(|v| v.slots.iter().any(|s| s.anchor == i));
            assert!(by_field || by_slot, "{}", a.name);
        }
        let ambiguous: Vec<_> = ANCHORS
            .iter()
            .filter(|a| a.certified_survivors.is_some_and(|n| n > 1))
            .map(|a| (a.name, a.certified_matches, a.search.is_none()))
            .collect();
        assert_eq!(ambiguous, [("item:array_reserve + bag:array_reserve", Some(2), true)]);
    }

    #[test]
    fn the_rolling_hash_is_the_one_the_generator_uses() {
        // Vectors computed by scripts/relocation/gen_relocation.py's `rolling` (no game bytes).
        let ramp: Vec<u8> = (0..16).collect();
        assert_eq!(rolling_hash(&ramp), 0xacbaa0c5);
        let ramp1: Vec<u8> = (1..17).collect();
        assert_eq!(rolling_hash(&ramp1), 0xb22ac09f);
        assert_eq!(rolling_hash(&[0xff; 16]), 0x6aafba3f);
        // Rolling it one byte on (drop 1, take 17) gives the hash of the next window.
        let (p, b) = (u64::from(ROLLING_PRIME), u64::from(ROLLING_BASE));
        let top = (0..15).fold(1u64, |t, _| t * b % p);
        let next = ((u64::from(rolling_hash(&ramp1)) + p - top) % p * b + 17) % p;
        let ramp2: Vec<u8> = (2..18).collect();
        assert_eq!(next as u32, rolling_hash(&ramp2));
    }
}
