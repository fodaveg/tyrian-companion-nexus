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

use std::time::{Duration, SystemTime};

use crate::inventory::{BuildProfile, Memory, ReadError, Reader, BUILD_SHA256};
use crate::sha256::hex;

/// The largest executable that is hashed at all. A larger file is not the certified build,
/// whatever it is.
pub const MAX_BYTES: u64 = 128 * 1024 * 1024;

/// How long the hash may go on in one call. It runs on the thread that keeps the connection
/// to the plugin alive, which owes it a heartbeat every 5 s and is taken for lost after 15 s
/// without one: hashing the whole file in one go held that thread for as long as a cold, slow
/// disk took, up to the 10 s the hash was given.
///
/// One second, done once on each pass of that thread while the verdict is pending. A pass is
/// then this slice, the read of the file that was under way when it ran out (64 KiB, which
/// cannot be cut short) and the 250 ms the thread waits on its socket: about a second and a
/// quarter between two chances to send a heartbeat, against the 4 s it must never go without
/// one. A heartbeat that is due goes out that much late at most, so the plugin sees one every
/// six and a half seconds in the worst case. A longer slice would get the verdict no sooner:
/// the thread already hashes four fifths of the time.
pub const SLICE: Duration = Duration::from_secs(1);

/// How one step of checking the executable ended.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Step<T> {
    /// More of the file is left to hash: the next call goes on from where this one stopped.
    Unfinished,
    /// Decided by the file or the image itself, so for good: asking again would give the same.
    Decided(T),
    /// Not decided: the system failed to open, read or copy something. Worth another try.
    Failed,
}

/// Which build the file is, by its SHA-256.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Build {
    Certified,
    Other,
}

/// What tells the file the hash began on from another one in its place: its size and when it
/// was last written.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Stamp {
    pub len: u64,
    pub modified: Option<SystemTime>,
}

/// The executable's file and the system's SHA-256 over it, as the adapter provides them. A
/// `None` or a `false` is the system failing, never an answer about the file.
pub trait HashedFile {
    /// Its size and its last write, as they are now.
    fn stamp(&mut self) -> Option<Stamp>;
    /// The next bytes of it, at most `buffer.len()`, and `Some(0)` at its end.
    fn read(&mut self, buffer: &mut [u8]) -> Option<usize>;
    /// Adds `bytes` to what is hashed.
    fn update(&mut self, bytes: &[u8]) -> bool;
    /// The SHA-256 of everything added.
    fn finish(&mut self) -> Option<[u8; 32]>;
}

/// The hash of the executable, under way: the file where the last slice left it, and how much
/// of it has gone into the hash.
pub struct Hashing<F> {
    file: F,
    before: Stamp,
    hashed: u64,
}

impl<F: HashedFile> Hashing<F> {
    /// Starts on `file`. One of no bytes, or larger than [`MAX_BYTES`], is decided here as
    /// another build, and not a byte of it is read.
    pub fn start(mut file: F) -> Result<Self, Step<Build>> {
        let Some(before) = file.stamp() else { return Err(Step::Failed) };
        if !size_in_range(before.len) {
            return Err(Step::Decided(Build::Other));
        }
        Ok(Self { file, before, hashed: 0 })
    }

    /// Hashes more of the file, until it ends or `expired` says this slice is over, which it
    /// is asked before every read. The caller makes a slice [`SLICE`] long and ends it at once
    /// when it is told to stop.
    ///
    /// At the end of the file it must still be the file the hash began on, by its size, its
    /// last write and the number of bytes that came out of it. One that changed meanwhile is
    /// not decided: the hash is of neither file.
    pub fn advance(&mut self, mut expired: impl FnMut() -> bool) -> Step<Build> {
        let mut buffer = [0u8; 65536];
        loop {
            if expired() {
                return Step::Unfinished;
            }
            let Some(read) = self.file.read(&mut buffer) else { return Step::Failed };
            if read == 0 {
                break;
            }
            self.hashed = self.hashed.saturating_add(read as u64);
            if self.hashed > self.before.len || !self.file.update(&buffer[..read.min(buffer.len())]) {
                return Step::Failed;
            }
        }
        if self.hashed != self.before.len || self.file.stamp() != Some(self.before) {
            return Step::Failed;
        }
        match self.file.finish() {
            Some(digest) if hex(&digest) == BUILD_SHA256 => Step::Decided(Build::Certified),
            Some(_) => Step::Decided(Build::Other),
            None => Step::Failed,
        }
    }

    /// How many bytes of the file have gone into the hash so far.
    pub fn hashed(&self) -> u64 {
        self.hashed
    }
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
            Step::Unfinished => Step::Unfinished,
            Step::Failed => Step::Failed,
        }
    }

    /// A file as the adapter hands it in, with everything that can go wrong with one.
    struct File {
        contents: Vec<u8>,
        /// What it says its size is; the contents can be longer or shorter than that.
        len: u64,
        modified: Option<SystemTime>,
        position: usize,
        /// The most bytes one read gives.
        chunk: usize,
        reads: usize,
        /// The read after which reading fails, and the one after which the hash does.
        read_fails_after: Option<usize>,
        update_fails_after: Option<usize>,
        stamp_fails: bool,
        finish_fails: bool,
        /// A write to the file while it is being hashed, seen in its stamp at the end.
        written_meanwhile: bool,
        /// The hash object says the certified digest, whatever went into it.
        certified: bool,
        fed: Vec<u8>,
    }

    impl File {
        fn of(contents: Vec<u8>) -> Self {
            Self {
                len: contents.len() as u64,
                contents,
                modified: Some(SystemTime::UNIX_EPOCH + Duration::from_secs(1_791_000_000)),
                position: 0,
                chunk: 1_000,
                reads: 0,
                read_fails_after: None,
                update_fails_after: None,
                stamp_fails: false,
                finish_fails: false,
                written_meanwhile: false,
                certified: false,
                fed: Vec::new(),
            }
        }
    }

    impl HashedFile for File {
        fn stamp(&mut self) -> Option<Stamp> {
            let later = self.written_meanwhile && self.position > 0;
            let modified = self.modified.map(|modified| if later { modified + Duration::from_secs(1) } else { modified });
            (!self.stamp_fails).then_some(Stamp { len: self.len, modified })
        }
        fn read(&mut self, buffer: &mut [u8]) -> Option<usize> {
            if self.read_fails_after.is_some_and(|after| self.reads >= after) {
                return None;
            }
            self.reads += 1;
            let count = self.chunk.min(buffer.len()).min(self.contents.len() - self.position);
            buffer[..count].copy_from_slice(&self.contents[self.position..self.position + count]);
            self.position += count;
            Some(count)
        }
        fn update(&mut self, bytes: &[u8]) -> bool {
            self.fed.extend_from_slice(bytes);
            !self.update_fails_after.is_some_and(|after| self.reads > after)
        }
        fn finish(&mut self) -> Option<[u8; 32]> {
            if self.finish_fails {
                return None;
            }
            if !self.certified {
                return Some(crate::sha256::sha256(&self.fed));
            }
            let mut digest = [0u8; 32];
            for (byte, pair) in digest.iter_mut().zip(BUILD_SHA256.as_bytes().chunks_exact(2)) {
                *byte = u8::from_str_radix(std::str::from_utf8(pair).unwrap(), 16).unwrap();
            }
            Some(digest)
        }
    }

    fn contents(len: usize) -> Vec<u8> {
        (0..len).map(|index| (index * 31 % 251) as u8).collect()
    }

    /// Hashes `file` in one go and says how it ended.
    fn whole(file: File) -> Step<Build> {
        match Hashing::start(file) {
            Ok(mut hashing) => hashing.advance(|| false),
            Err(step) => step,
        }
    }

    #[test]
    fn a_size_out_of_range_is_another_build_before_a_byte_is_read() {
        for len in [0, MAX_BYTES + 1] {
            let mut file = File::of(Vec::new());
            file.len = len;
            // Even a hash that would say "certified": it is never asked.
            file.certified = true;
            assert!(matches!(Hashing::start(file), Err(Step::Decided(Build::Other))), "{len}");
        }
        // The largest that is hashed is hashed.
        let mut largest = File::of(contents(10));
        largest.len = MAX_BYTES;
        assert!(Hashing::start(largest).is_ok());
    }

    #[test]
    fn the_digest_decides_which_build_it_is() {
        assert_eq!(whole(File::of(contents(12_345))), Step::Decided(Build::Other));
        let mut certified = File::of(contents(12_345));
        certified.certified = true;
        assert_eq!(whole(certified), Step::Decided(Build::Certified));
    }

    /// The fault of the review: the hash had ten seconds in one go, on the thread that owes
    /// the plugin a heartbeat. In slices it goes on from where it stopped, every byte of the
    /// file goes into the hash once and in order, and a slow disk only means more slices.
    #[test]
    fn the_hash_goes_on_across_slices_from_where_it_stopped() {
        let data = contents(40_500);
        let mut hashing = Hashing::start(File::of(data.clone())).ok().unwrap();
        let mut slices = 0;
        let outcome = loop {
            // A slice that lasts three reads, as one that ran out of time would.
            let mut reads = 0;
            let step = hashing.advance(|| {
                reads += 1;
                reads > 3
            });
            slices += 1;
            match step {
                Step::Unfinished => assert_eq!(hashing.hashed(), slices * 3_000, "slice {slices}"),
                other => break other,
            }
        };
        // 41 reads of 1000 bytes and the one that finds the end: fourteen slices of three.
        assert_eq!((slices, hashing.hashed()), (14, 40_500));
        assert_eq!(outcome, Step::Decided(Build::Other));
        assert_eq!(hashing.file.fed, data, "every byte once, in order");
        assert_eq!(hashing.file.reads, 42);
        // The same digest as in one go.
        assert_eq!(hashing.file.finish(), Some(crate::sha256::sha256(&data)));
    }

    #[test]
    fn a_slice_that_is_over_before_it_starts_reads_nothing_and_decides_nothing() {
        // What the adapter does when it is told to stop.
        let mut hashing = Hashing::start(File::of(contents(5_000))).ok().unwrap();
        for _ in 0..3 {
            assert_eq!(hashing.advance(|| true), Step::Unfinished);
        }
        assert_eq!((hashing.hashed(), hashing.file.reads), (0, 0));
        // And it is still good for the rest.
        assert_eq!(hashing.advance(|| false), Step::Decided(Build::Other));
    }

    /// What the system fails to do is not an answer about the file: not even with a hash
    /// object that would say "certified".
    #[test]
    fn what_the_system_fails_to_do_decides_nothing() {
        let failing: [(&str, fn(&mut File)); 8] = [
            ("the size cannot be asked", |file| file.stamp_fails = true),
            ("the first read fails", |file| file.read_fails_after = Some(0)),
            ("a read in the middle fails", |file| file.read_fails_after = Some(3)),
            ("the hash refuses some bytes", |file| file.update_fails_after = Some(2)),
            ("the hash cannot be finished", |file| file.finish_fails = true),
            ("the file is written while it is hashed", |file| file.written_meanwhile = true),
            ("more bytes come out than it had", |file| file.len -= 1),
            ("fewer bytes come out than it had", |file| file.len += 1),
        ];
        for (what, fault) in failing {
            let mut file = File::of(contents(6_000));
            file.certified = true;
            fault(&mut file);
            assert_eq!(whole(file), Step::Failed, "{what}");
        }
        // The same file with nothing wrong is decided.
        let mut sound = File::of(contents(6_000));
        sound.certified = true;
        assert_eq!(whole(sound), Step::Decided(Build::Certified));
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
