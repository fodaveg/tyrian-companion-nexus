//! Whether a game executable holds what the readers rely on: the certificate, checked by content.
//!
//! Until 16 October 2026 the readers only ran on an executable whose SHA-256 was one constant.
//! Every update of the game changed that hash, also the ones of 15 and 16 October 2026 that did
//! not change a byte of what the readers use, and every one stopped the addon until a release
//! carried the new hash. The certificate is what that release used to check by hand
//! (`docs/audit/2026-10-15-recertification-build-208704.json`), checked here on every load
//! against the file itself:
//!
//! - the PE header: AMD64 PE32+, the image base, the image size, every section's name, RVA,
//!   virtual size and file offset, and the TLS directory at its RVA;
//! - the TLS directory's AddressOfIndex is the TLS index the context route reads;
//! - [`SPANS`]: the 37 code and table ranges the three profiles keep by digest, the 14
//!   functions the inventory reader compares by address only, and, since revision 2, the 6
//!   ranges of the game's own code that read the four field offsets no other range named (item
//!   `+0x58`, inventory `+0xd0`, item context `+0x38`, definition `+0x28`), each by the SHA-256
//!   of its bytes at its file offset;
//! - [`SLOTS`]: the vtable slots the readers follow, and those that select two of those six
//!   functions, hold the virtual address of the proven function (image base plus its RVA, as
//!   the file stores it before relocation).
//!
//! A file that holds all of it is certified, whatever its hash. One that differs in any of it
//! is not, and the readers do not run on it: an update that moves or changes that code needs a
//! recertification, as before. Nothing here searches for a range somewhere else or relocates a
//! profile.
//!
//! The file is checked as it is hashed, in the order it is read: the bytes the certificate
//! names are copied as they go by ([`Collector::feed`]), and judged once the file has ended
//! ([`Collector::verdict`]). The image loaded in memory is not used for this: ASLR relocates
//! its vtables, the file is what the certificate was taken from.
//!
//! The table is generated (`certificate_data.rs`, from
//! `docs/audit/2026-10-16-content-certificate/`): a change of it is a recertification, not an
//! edit.

use crate::sha256::{hex, sha256};

/// What the certificate is called in the log and the Options window: the build it was taken
/// from and its revision.
pub const CERTIFICATE: &str = "code-208704-r2";

/// The SHA-256 of the executable the certificate was taken from (the game's build of
/// 15 October 2026). It identifies the certificate's origin and the fixtures; it is not a
/// condition to read.
pub const REFERENCE_BUILD_SHA256: &str = "960c3e454847525993eb3266845ee37377511b8039635a7fbad6fd5592cb61ba";

/// The SHA-256 of the executable the addon certified in this load, in hex: what `live_open`
/// names as `build`. Set once, by the adapter, when the file is certified.
static GAME_BUILD: std::sync::OnceLock<String> = std::sync::OnceLock::new();

/// Records the digest of the executable that was certified. A second call is ignored: the
/// process has one executable.
pub fn set_game_build(digest: &[u8; 32]) {
    let _ = GAME_BUILD.set(hex(digest));
}

/// The build `live_open` names: the certified executable's digest, or, before one was
/// certified (the host tests, which never certify one), [`REFERENCE_BUILD_SHA256`]. The source
/// never opens an epoch before a sample, and never samples before the certificate.
pub fn game_build() -> &'static str {
    GAME_BUILD.get().map_or(REFERENCE_BUILD_SHA256, String::as_str)
}

/// One section header of the certified image.
#[derive(Debug, Clone, Copy)]
pub struct Section {
    pub name: [u8; 8],
    pub rva: u32,
    pub virtual_size: u32,
    pub file_offset: u32,
}

/// A range of the file that must hash to `sha256`.
#[derive(Debug, Clone, Copy)]
pub struct Span {
    pub name: &'static str,
    pub offset: u64,
    pub len: u32,
    pub sha256: &'static str,
}

/// A vtable slot of the file that must hold `IMAGE_BASE + rva`.
#[derive(Debug, Clone, Copy)]
pub struct Slot {
    pub name: &'static str,
    pub offset: u64,
    pub rva: u64,
}

include!("certificate_data.rs");

/// What a file must hold to be read: the image layout and the ranges and slots of [`Span`] and
/// [`Slot`]. The game's is [`GAME`]; tests make their own.
#[derive(Debug)]
pub struct Certificate {
    pub image_base: u64,
    pub image_size: u32,
    pub tls_directory_rva: u32,
    /// Where the TLS directory is in the file.
    pub tls_directory_offset: u64,
    /// What its AddressOfIndex must point at, as an RVA.
    pub tls_index_rva: u64,
    pub sections: &'static [Section],
    pub spans: &'static [Span],
    pub slots: &'static [Slot],
}

/// The certificate of the game's executable.
pub static GAME: Certificate = Certificate {
    image_base: IMAGE_BASE,
    image_size: IMAGE_SIZE,
    tls_directory_rva: TLS_DIRECTORY_RVA,
    tls_directory_offset: TLS_DIRECTORY_OFFSET,
    tls_index_rva: crate::inventory::TLS_INDEX_RVA,
    sections: &SECTIONS,
    spans: &SPANS,
    slots: &SLOTS,
};

/// How much of the start of the file the header check reads: the DOS header, the PE header and
/// the section table of the certified image are well inside it.
pub const HEADER_BYTES: usize = 4096;

/// The bytes a certificate names, copied out of the file as it is read in order.
pub struct Collector {
    certificate: &'static Certificate,
    header: Vec<u8>,
    /// One buffer per piece, in the order of [`Certificate::pieces`].
    parts: Vec<Vec<u8>>,
    /// How many bytes of each piece have been seen.
    seen: Vec<usize>,
}

impl Certificate {
    /// Every range the verdict needs besides the header: the spans, the slots and the TLS
    /// directory's AddressOfIndex, as (offset, length).
    fn pieces(&self) -> impl Iterator<Item = (u64, usize)> + '_ {
        self.spans
            .iter()
            .map(|span| (span.offset, span.len as usize))
            .chain(self.slots.iter().map(|slot| (slot.offset, 8)))
            .chain(std::iter::once((self.tls_directory_offset + 16, 8)))
    }
}

impl Collector {
    pub fn new(certificate: &'static Certificate) -> Self {
        let parts: Vec<Vec<u8>> = certificate.pieces().map(|(_, len)| vec![0; len]).collect();
        let seen = vec![0; parts.len()];
        Self { certificate, header: Vec::with_capacity(HEADER_BYTES), parts, seen }
    }

    /// Copies whatever the certificate names out of `bytes`, which are those of the file from
    /// `at` on. Called with the file in order; a piece that goes by in two reads is put together.
    pub fn feed(&mut self, at: u64, bytes: &[u8]) {
        let end = at.saturating_add(bytes.len() as u64);
        if (self.header.len() as u64) == at && self.header.len() < HEADER_BYTES {
            let take = (HEADER_BYTES - self.header.len()).min(bytes.len());
            self.header.extend_from_slice(&bytes[..take]);
        }
        for (index, (offset, len)) in self.certificate.pieces().enumerate() {
            let piece_end = offset.saturating_add(len as u64);
            if piece_end <= at || offset >= end {
                continue;
            }
            let from = offset.max(at);
            let to = piece_end.min(end);
            let source = &bytes[(from - at) as usize..(to - at) as usize];
            let target = (from - offset) as usize;
            self.parts[index][target..target + source.len()].copy_from_slice(source);
            self.seen[index] = self.seen[index].saturating_add(source.len());
        }
    }

    /// `Ok` when the file held everything the certificate names; otherwise the name of the first
    /// check that failed, for the log. Only meaningful once the whole file has been fed.
    pub fn verdict(&self) -> Result<(), &'static str> {
        let c = self.certificate;
        self.header_verdict()?;
        let mut parts = self.parts.iter().zip(&self.seen);
        for span in c.spans {
            let (bytes, seen) = parts.next().ok_or("certificate")?;
            if *seen != bytes.len() || hex(&sha256(bytes)) != span.sha256 {
                return Err(span.name);
            }
        }
        for slot in c.slots {
            let (bytes, seen) = parts.next().ok_or("certificate")?;
            if *seen != 8 || u64::from_le_bytes(word(bytes)) != c.image_base + slot.rva {
                return Err(slot.name);
            }
        }
        let (bytes, seen) = parts.next().ok_or("certificate")?;
        if *seen != 8 || u64::from_le_bytes(word(bytes)) != c.image_base + c.tls_index_rva {
            return Err("tls_index");
        }
        Ok(())
    }

    fn header_verdict(&self) -> Result<(), &'static str> {
        let c = self.certificate;
        let h = &self.header;
        let u16_at = |at: usize| h.get(at..at + 2).map(|b| u16::from_le_bytes([b[0], b[1]]));
        let u32_at = |at: usize| h.get(at..at + 4).map(|b| u32::from_le_bytes([b[0], b[1], b[2], b[3]]));
        let u64_at = |at: usize| h.get(at..at + 8).map(|b| u64::from_le_bytes(word(b)));
        if h.get(..2) != Some(b"MZ") {
            return Err("pe_header");
        }
        let pe = u32_at(0x3c).ok_or("pe_header")? as usize;
        if !(0x40..=1024).contains(&pe)
            || h.get(pe..pe + 4) != Some(b"PE\0\0")
            || u16_at(pe + 4) != Some(0x8664)
            || u16_at(pe + 24) != Some(0x20b)
        {
            return Err("pe_header");
        }
        if u64_at(pe + 24 + 24) != Some(c.image_base) {
            return Err("image_base");
        }
        if u32_at(pe + 24 + 56) != Some(c.image_size) {
            return Err("image_size");
        }
        // Data directory 9 is the TLS directory: its RVA.
        if u32_at(pe + 24 + 112 + 9 * 8) != Some(c.tls_directory_rva) {
            return Err("tls_directory");
        }
        let count = u16_at(pe + 6).ok_or("pe_header")? as usize;
        let optional = u16_at(pe + 20).ok_or("pe_header")? as usize;
        if count != c.sections.len() {
            return Err("sections");
        }
        for (i, section) in c.sections.iter().enumerate() {
            let at = pe + 24 + optional + 40 * i;
            if h.get(at..at + 8) != Some(&section.name[..])
                || u32_at(at + 8) != Some(section.virtual_size)
                || u32_at(at + 12) != Some(section.rva)
                || u32_at(at + 20) != Some(section.file_offset)
            {
                return Err("sections");
            }
        }
        Ok(())
    }
}

fn word(bytes: &[u8]) -> [u8; 8] {
    let mut out = [0u8; 8];
    out.copy_from_slice(&bytes[..8]);
    out
}

/// A small certificate and a file that holds it, for the tests of this crate and of
/// `executable`: two sections, two spans, one slot and the TLS directory, all in a few KiB.
#[cfg(test)]
pub(crate) mod synthetic {
    use super::*;

    pub const PE: usize = 0x100;
    pub const LEN: usize = 0x3000;
    const SPAN_A: (u64, usize) = (0x1200, 300);
    const SPAN_B: (u64, usize) = (0x1ffc, 10); // crosses a 4 KiB boundary
    const SLOT: u64 = 0x2400;
    const TLS_DIR: u64 = 0x2500;

    fn fill(at: u64, len: usize) -> Vec<u8> {
        (0..len).map(|i| ((at as usize + i) * 7 % 253) as u8).collect()
    }

    /// The certificate, and a file of [`LEN`] bytes that holds it.
    pub fn certificate_and_file() -> (&'static Certificate, Vec<u8>) {
        let sections: &'static [Section] = Box::leak(Box::new([
            Section { name: *b".text\0\0\0", rva: 0x1000, virtual_size: 0x1000, file_offset: 0x1000 },
            Section { name: *b".rdata\0\0", rva: 0x2000, virtual_size: 0x1000, file_offset: 0x2000 },
        ]));
        let mut file: Vec<u8> = (0..LEN).map(|i| (i * 31 % 251) as u8).collect();
        let digest = |(at, len): (u64, usize), file: &mut Vec<u8>| {
            let bytes = fill(at, len);
            file[at as usize..at as usize + len].copy_from_slice(&bytes);
            &*Box::leak(hex(&sha256(&bytes)).into_boxed_str())
        };
        let a = digest(SPAN_A, &mut file);
        let b = digest(SPAN_B, &mut file);
        let spans: &'static [Span] = Box::leak(Box::new([
            Span { name: "span a", offset: SPAN_A.0, len: SPAN_A.1 as u32, sha256: a },
            Span { name: "span b", offset: SPAN_B.0, len: SPAN_B.1 as u32, sha256: b },
        ]));
        let slots: &'static [Slot] = Box::leak(Box::new([Slot { name: "slot", offset: SLOT, rva: 0x1200 }]));
        let c: &'static Certificate = Box::leak(Box::new(Certificate {
            image_base: 0x1_4000_0000,
            image_size: 0x5000,
            tls_directory_rva: 0x2500,
            tls_directory_offset: TLS_DIR,
            tls_index_rva: 0x2800,
            sections,
            spans,
            slots,
        }));
        file[..HEADER_BYTES].fill(0);
        file[..2].copy_from_slice(b"MZ");
        file[0x3c..0x40].copy_from_slice(&(PE as u32).to_le_bytes());
        file[PE..PE + 4].copy_from_slice(b"PE\0\0");
        file[PE + 4..PE + 6].copy_from_slice(&0x8664u16.to_le_bytes());
        file[PE + 6..PE + 8].copy_from_slice(&(sections.len() as u16).to_le_bytes());
        file[PE + 20..PE + 22].copy_from_slice(&240u16.to_le_bytes());
        file[PE + 24..PE + 26].copy_from_slice(&0x20bu16.to_le_bytes());
        file[PE + 48..PE + 56].copy_from_slice(&c.image_base.to_le_bytes());
        file[PE + 80..PE + 84].copy_from_slice(&c.image_size.to_le_bytes());
        file[PE + 136 + 72..PE + 136 + 76].copy_from_slice(&c.tls_directory_rva.to_le_bytes());
        for (i, s) in sections.iter().enumerate() {
            let at = PE + 24 + 240 + 40 * i;
            file[at..at + 8].copy_from_slice(&s.name);
            file[at + 8..at + 12].copy_from_slice(&s.virtual_size.to_le_bytes());
            file[at + 12..at + 16].copy_from_slice(&s.rva.to_le_bytes());
            file[at + 20..at + 24].copy_from_slice(&s.file_offset.to_le_bytes());
        }
        let slot = (c.image_base + 0x1200).to_le_bytes();
        file[SLOT as usize..SLOT as usize + 8].copy_from_slice(&slot);
        let index = (c.image_base + c.tls_index_rva).to_le_bytes();
        file[TLS_DIR as usize + 16..TLS_DIR as usize + 24].copy_from_slice(&index);
        (c, file)
    }

    /// Where one byte of each check is, with the name the verdict gives when it differs.
    pub fn one_byte_of_each_check() -> [(&'static str, usize); 10] {
        [
            ("pe_header", 0),
            ("pe_header", PE + 4),
            ("image_base", PE + 48),
            ("image_size", PE + 80),
            ("tls_directory", PE + 136 + 72),
            ("sections", PE + 24 + 240 + 40 + 12),
            ("span a", SPAN_A.0 as usize + 150),
            ("span b", SPAN_B.0 as usize + 9),
            ("slot", SLOT as usize + 1),
            ("tls_index", TLS_DIR as usize + 17),
        ]
    }
}

#[cfg(test)]
mod tests {
    use super::synthetic::*;
    use super::*;

    fn verdict_of(c: &'static Certificate, file: &[u8], chunk: usize) -> Result<(), &'static str> {
        let mut collector = Collector::new(c);
        for (i, part) in file.chunks(chunk).enumerate() {
            collector.feed((i * chunk) as u64, part);
        }
        collector.verdict()
    }

    #[test]
    fn a_file_that_holds_the_certificate_passes_whatever_the_size_of_the_reads() {
        let (c, file) = certificate_and_file();
        for chunk in [1, 7, 1000, 4096, 65536] {
            assert_eq!(verdict_of(c, &file, chunk), Ok(()), "reads of {chunk}");
        }
    }

    #[test]
    fn one_byte_that_differs_in_any_check_is_named_and_the_rest_of_the_file_does_not_matter() {
        let (c, file) = certificate_and_file();
        for (name, at) in one_byte_of_each_check() {
            let mut changed = file.clone();
            changed[at] ^= 0x40;
            assert_eq!(verdict_of(c, &changed, 1000), Err(name), "byte {at:#x}");
        }
        // A byte the certificate does not name: another hash, the same verdict.
        let mut other = file.clone();
        other[0x2c00] ^= 0x40;
        assert_eq!(verdict_of(c, &other, 1000), Ok(()));
    }

    #[test]
    fn a_file_that_ends_before_a_piece_is_not_certified() {
        let (c, file) = certificate_and_file();
        assert_eq!(verdict_of(c, &file[..0x1250], 1000), Err("span a"));
        assert_eq!(verdict_of(c, &file[..0x2504], 1000), Err("tls_index"));
    }

    #[test]
    fn the_game_table_has_every_check_once() {
        let mut names = std::collections::BTreeSet::new();
        for span in GAME.spans {
            assert!(span.len > 0 && span.sha256.len() == 64, "{}", span.name);
            assert!(names.insert(span.name), "{}", span.name);
        }
        for slot in GAME.slots {
            assert!(names.insert(slot.name), "{}", slot.name);
        }
        assert_eq!((GAME.spans.len(), GAME.slots.len(), GAME.sections.len()), (57, 48, 7));
        // Every piece lies inside a section of the file.
        for (offset, len) in GAME.pieces() {
            assert!(
                GAME.sections.iter().any(|s| offset >= s.file_offset as u64
                    && offset + len as u64 <= s.file_offset as u64 + s.virtual_size as u64),
                "{offset:#x}"
            );
        }
    }

    /// The real file, when there is a copy to point at: `TC_GW2_EXE=<copy of Gw2-64.exe>
    /// cargo test -p tyrian_companion_nexus_core certificate -- --ignored`.
    #[test]
    #[ignore]
    fn a_copy_of_the_installed_executable_holds_the_certificate() {
        let path = std::env::var("TC_GW2_EXE").expect("TC_GW2_EXE");
        let file = std::fs::read(path).expect("read");
        assert_eq!(verdict_of(&GAME, &file, 65536), Ok(()));
        println!("certified by content ({CERTIFICATE}), sha256 {}", hex(&sha256(&file)));
    }
}
