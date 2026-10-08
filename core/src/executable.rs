//! Whether the game's executable is the certified build: what decides it for good, and what
//! only means "not now".
//!
//! The readers touch a game field only after the executable was hashed and its loaded image
//! checked. Some answers of that check depend on nothing but the file and the image: its size,
//! its digest, its header. Those are final for as long as the process lives, whichever way
//! they go, and a "no" among them is another build: the source says so and stops. Every other
//! way of not getting an answer is the system's doing, a file that cannot be opened or read
//! or a copy that fails, and is tried again later.
//!
//! This module holds that rule, with the file, the hash and the memory handed in, so that it
//! is tested here. The Windows adapter only opens the file, runs the system's SHA-256 over
//! what this asks it to and copies the bytes of the image.

use crate::inventory::{BuildProfile, Memory, ReadError, Reader, BUILD_SHA256};

/// The largest executable that is hashed at all. A larger file is not the certified build,
/// whatever it is.
pub const MAX_BYTES: u64 = 128 * 1024 * 1024;

/// How one step of checking the executable ended.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Step<T> {
    /// Decided by the file or the image itself, so for good: asking again would give the same.
    Decided(T),
    /// Not decided: the system failed to open, read or copy something. Worth another try.
    Failed,
}

/// Whether a file of `len` bytes can be the certified build at all. One that cannot is decided
/// by its size alone, without hashing a byte of it.
pub fn size_in_range(len: u64) -> bool {
    len != 0 && len <= MAX_BYTES
}

/// The profile of the image loaded at `base`, for a process whose executable file has the
/// certified digest: `Some` when its header is the certified build's, `None` when it is not.
///
/// The header is in memory for as long as the process lives and does not change, so a header
/// that is not the expected one is as final as a digest that is not. Only a copy that failed
/// says nothing about it.
pub fn image_profile<M: Memory>(reader: &mut Reader<M>, base: u64) -> Step<Option<BuildProfile>> {
    match header(reader, base) {
        Ok(profile) => Step::Decided(Some(profile)),
        Err(ReadError::ReadFailed) => Step::Failed,
        Err(_) => Step::Decided(None),
    }
}

/// An AMD64 PE32+ image with room for the fields the profile reads.
fn header<M: Memory>(reader: &mut Reader<M>, base: u64) -> Result<BuildProfile, ReadError> {
    if reader.read::<2>(base)? != *b"MZ" {
        return Err(ReadError::UnsupportedBuild);
    }
    let pe = reader.scalar(base + 0x3c, 4)?;
    if !(0x40..=4096).contains(&pe)
        || reader.read::<4>(base + pe)? != *b"PE\0\0"
        || reader.scalar(base + pe + 4, 2)? != 0x8664
        || reader.scalar(base + pe + 24, 2)? != 0x20b
    {
        return Err(ReadError::UnsupportedBuild);
    }
    let image_size = reader.scalar(base + pe + 24 + 56, 4)?;
    BuildProfile::checked(BUILD_SHA256, base, image_size)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::inventory::TLS_INDEX_RVA;

    const BASE: u64 = 0x7ff6_1000_0000;
    const PE: usize = 0x100;

    /// The first page of a loaded image, and whether copying from it works.
    struct Image {
        bytes: Vec<u8>,
        readable: bool,
    }

    impl Memory for Image {
        fn read_exact(&mut self, address: u64, bytes: &mut [u8]) -> Result<(), ReadError> {
            let start = (address - BASE) as usize;
            if !self.readable {
                return Err(ReadError::ReadFailed);
            }
            bytes.copy_from_slice(&self.bytes[start..start + bytes.len()]);
            Ok(())
        }
    }

    /// The header of the certified build as far as the check looks at it.
    fn certified() -> Vec<u8> {
        let mut bytes = vec![0u8; 4096];
        bytes[..2].copy_from_slice(b"MZ");
        bytes[0x3c..0x40].copy_from_slice(&(PE as u32).to_le_bytes());
        bytes[PE..PE + 4].copy_from_slice(b"PE\0\0");
        bytes[PE + 4..PE + 6].copy_from_slice(&0x8664u16.to_le_bytes());
        bytes[PE + 24..PE + 26].copy_from_slice(&0x20bu16.to_le_bytes());
        bytes[PE + 80..PE + 84].copy_from_slice(&0x0300_0000u32.to_le_bytes());
        bytes
    }

    fn check(bytes: Vec<u8>, readable: bool) -> Step<Option<u64>> {
        match image_profile(&mut Reader::new(Image { bytes, readable }), BASE) {
            Step::Decided(profile) => Step::Decided(profile.map(|profile| profile.base)),
            Step::Failed => Step::Failed,
        }
    }

    #[test]
    fn a_file_of_no_bytes_or_too_large_is_decided_by_its_size() {
        for len in [1, 4_324_864, MAX_BYTES] {
            assert!(size_in_range(len), "{len}");
        }
        for len in [0, MAX_BYTES + 1, u64::MAX] {
            assert!(!size_in_range(len), "{len}");
        }
    }

    #[test]
    fn the_certified_header_gives_the_profile() {
        assert_eq!(check(certified(), true), Step::Decided(Some(BASE)));
    }

    /// The digest was the certified one and the image is not what that build loads as: it is
    /// in memory, it will be the same on every later look, and it is not this build.
    #[test]
    fn a_header_that_is_not_the_certified_one_is_decided_for_good() {
        let broken: [(&str, fn(&mut Vec<u8>)); 7] = [
            ("no MZ", |bytes| bytes[..2].copy_from_slice(b"ZM")),
            ("a PE offset before the DOS header ends", |bytes| bytes[0x3c..0x40].copy_from_slice(&0x3fu32.to_le_bytes())),
            ("a PE offset past the first page", |bytes| bytes[0x3c..0x40].copy_from_slice(&4097u32.to_le_bytes())),
            ("no PE signature", |bytes| bytes[PE..PE + 4].copy_from_slice(b"PE\0\x01")),
            ("another machine", |bytes| bytes[PE + 4..PE + 6].copy_from_slice(&0x014cu16.to_le_bytes())),
            ("a PE32 optional header", |bytes| bytes[PE + 24..PE + 26].copy_from_slice(&0x10bu16.to_le_bytes())),
            ("an image too small for the profile", |bytes| bytes[PE + 80..PE + 84].copy_from_slice(&(TLS_INDEX_RVA as u32).to_le_bytes())),
        ];
        for (what, damage) in broken {
            let mut bytes = certified();
            damage(&mut bytes);
            assert_eq!(check(bytes, true), Step::Decided(None), "{what}");
        }
    }

    /// A copy that fails, or that the adapter refuses because its time ran out, is not an
    /// answer about the image: the certified one included.
    #[test]
    fn a_copy_that_fails_decides_nothing() {
        assert_eq!(check(certified(), false), Step::Failed);
        let mut other = certified();
        other[..2].copy_from_slice(b"ZM");
        assert_eq!(check(other, false), Step::Failed);
    }
}
