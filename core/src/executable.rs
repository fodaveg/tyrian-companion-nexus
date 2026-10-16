//! Whether the game's executable holds the certificate: what decides it for good, and what only
//! means "not now".
//!
//! The readers touch a game field only after the executable was read whole, found to hold what
//! they rely on ([`crate::certificate`]), and its loaded image checked. Its SHA-256 is taken on
//! the same pass and only names the build: since 16 October 2026 an update of the game that
//! leaves that code as it was does not stop the readers. Some answers of that check depend on
//! nothing but the file and the image: its size, its content, its header. Those are final for as long as the process lives, whichever way
//! they go, and a "no" among them is another build: the source says so and stops. Every other
//! way of not getting an answer is the system's doing, a file that cannot be opened or read
//! or a copy that fails, and is tried again later.
//!
//! This module holds that rule, with the file, the hash and the memory handed in, so that it
//! is tested here. The Windows adapter only opens the file, runs the system's SHA-256 over
//! what this asks it to and copies the bytes of the image.

use std::time::{Duration, SystemTime};

use crate::certificate::{Certificate, Collector, CERTIFICATE, GAME};
use crate::inventory::{BuildProfile, Memory, ReadError, Reader};
use crate::live::RESPONSE_TIMEOUT;
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

/// How much hashing the source keeps quiet about. While the hash is under way the source takes
/// no sample and tells the plugin nothing, and the hash has no limit of its own: on a cold, slow
/// disk, or with something holding up the reads of the file, that was a plugin that saw the
/// capability negotiated and then heartbeats for as long as it took, and a panel waiting without
/// a word of why. Past this much the source says what is true of it, that it could not complete
/// a reading (`live_status unavailable`, `read_failed`), and goes on hashing where it was: the
/// hash is never given up or started over for being slow, which on such a disk would be never
/// finishing it.
///
/// Ten seconds of slices, by two numbers there already were. It is what the source itself gives
/// the plugin to answer before it takes its silence for a failure ([`RESPONSE_TIMEOUT`]), and
/// it is what the hash was given when it was done in one go, with the difference that those ten
/// seconds ended it. It is the time spent hashing that counts ([`Verdict::unfinished_for`]), so
/// a loading screen in the middle, during which no slice runs and the plugin has been told
/// there is no character in a map, is not held against it. Ten slices are ten passes of the
/// worker: about twelve and a half seconds on the clock with a character in a map.
///
/// [`Verdict::unfinished_for`]: crate::verdict::Verdict::unfinished_for
pub const OVERDUE: Duration = RESPONSE_TIMEOUT;

/// Whether a hash that has taken `spent` so far, and is not over, has gone on for longer than
/// the source keeps quiet about.
pub fn overdue(spent: Duration) -> bool {
    spent >= OVERDUE
}

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

/// Whether the file holds what the readers rely on ([`crate::certificate`]), and its SHA-256.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Build {
    /// It holds the certificate. Its SHA-256 is whatever the game's update made it: it is the
    /// build `live_open` names, not a condition.
    Certified([u8; 32]),
    /// It does not: its SHA-256 and the first check of the certificate that failed.
    Uncertified([u8; 32], &'static str),
    /// Decided by size alone, before a byte was read.
    BySize,
}

/// The first 12 hex digits of a SHA-256: enough to tell one binary from another in a log line.
pub fn short_digest(digest: &[u8; 32]) -> String {
    hex(digest)[..12].to_string()
}

/// A size in MiB with one decimal, as the log writes it.
fn mib(bytes: u64) -> String {
    let tenths = bytes.saturating_mul(10) / (1024 * 1024);
    format!("{}.{}", tenths / 10, tenths % 10)
}

/// The log line for an executable the hash decided: which build, the first 12 hex digits of its
/// SHA-256, its size in MiB and how long the hash took. No path and no name of the file.
pub fn decided_line(build: Build, bytes: u64, took: Duration) -> String {
    let (which, digest) = match build {
        Build::Certified(digest) => (format!("certified by content ({CERTIFICATE})"), short_digest(&digest)),
        Build::Uncertified(digest, check) => (format!("not certified, {check} differs"), short_digest(&digest)),
        Build::BySize => return size_line(bytes),
    };
    format!("game executable: {which}, sha256 {digest}, {} MiB, hashed in {} ms", mib(bytes), took.as_millis())
}

/// The log line for the addon's own DLL: its version label and what the file really is, by the
/// first 12 hex digits of its SHA-256, so that a development binary carrying a release's label
/// can be told apart. No path.
pub fn addon_line(version: &str, digest: &[u8; 32], bytes: u64, took: Duration) -> String {
    format!(
        "addon build: v{version}, sha256 {}, {} MiB, hashed in {} ms",
        short_digest(digest),
        mib(bytes),
        took.as_millis()
    )
}

/// The log line for an executable decided by its size alone: it was not hashed, so there is no
/// digest to give.
pub fn size_line(bytes: u64) -> String {
    format!("game executable: other build by size, {} MiB, not hashed", mib(bytes))
}

/// The log line for a verification the system failed: the kind of the first failure of this load
/// and how many there have been.
pub fn failure_line(first: &str, count: u32) -> String {
    format!("game executable: verification failed, first failure {first}, {count} so far")
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
    /// The bytes the certificate names, copied as the file goes by.
    collector: Collector,
}

impl<F: HashedFile> Hashing<F> {
    /// Starts on `file`. One of no bytes, or larger than [`MAX_BYTES`], is decided here as
    /// another build, and not a byte of it is read.
    pub fn start(file: F) -> Result<Self, Step<Build>> {
        Self::start_with(file, &GAME)
    }

    /// [`Hashing::start`] against another certificate than the game's: the tests'.
    pub fn start_with(mut file: F, certificate: &'static Certificate) -> Result<Self, Step<Build>> {
        let Some(before) = file.stamp() else { return Err(Step::Failed) };
        if !size_in_range(before.len) {
            return Err(Step::Decided(Build::BySize));
        }
        Ok(Self { file, before, hashed: 0, collector: Collector::new(certificate) })
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
            let at = self.hashed;
            self.hashed = self.hashed.saturating_add(read as u64);
            let bytes = &buffer[..read.min(buffer.len())];
            if self.hashed > self.before.len || !self.file.update(bytes) {
                return Step::Failed;
            }
            self.collector.feed(at, bytes);
        }
        if self.hashed != self.before.len || self.file.stamp() != Some(self.before) {
            return Step::Failed;
        }
        match self.file.finish() {
            Some(digest) => Step::Decided(match self.collector.verdict() {
                Ok(()) => Build::Certified(digest),
                Err(check) => Build::Uncertified(digest, check),
            }),
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

/// The profile of the image loaded at `base`, for a process whose executable file holds the
/// certificate: `Some` when its header is the one the file says (an AMD64 PE32+ image of the
/// certified size), `None` when it is not.
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
    if image_size != u64::from(crate::certificate::IMAGE_SIZE) {
        return Err(ReadError::UnsupportedBuild);
    }
    BuildProfile::checked(base, image_size)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::certificate::synthetic::{certificate_and_file, one_byte_of_each_check};
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
        bytes[PE + 80..PE + 84].copy_from_slice(&crate::certificate::IMAGE_SIZE.to_le_bytes());
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
            Some(crate::sha256::sha256(&self.fed))
        }
    }

    fn contents(len: usize) -> Vec<u8> {
        (0..len).map(|index| (index * 31 % 251) as u8).collect()
    }

    /// Hashes `file` in one go against the synthetic certificate and says how it ended.
    fn whole(file: File) -> Step<Build> {
        match Hashing::start_with(file, certificate_and_file().0) {
            Ok(mut hashing) => hashing.advance(|| false),
            Err(step) => step,
        }
    }

    /// A file that holds the synthetic certificate.
    fn holding() -> Vec<u8> {
        certificate_and_file().1
    }

    #[test]
    fn a_size_out_of_range_is_another_build_before_a_byte_is_read() {
        for len in [0, MAX_BYTES + 1] {
            let mut file = File::of(Vec::new());
            file.len = len;
            // Even a file that would hold the certificate: it is never read.
            file.contents = holding();
            assert!(matches!(Hashing::start(file), Err(Step::Decided(Build::BySize))), "{len}");
        }
        // The largest that is hashed is hashed.
        let mut largest = File::of(contents(10));
        largest.len = MAX_BYTES;
        assert!(Hashing::start(largest).is_ok());
    }

    /// What decides is what the file holds, not its digest: two files with different digests
    /// that both hold the certificate are both certified, each with its own digest, and one byte
    /// that differs in any check is not, with the name of that check for the log.
    #[test]
    fn the_content_decides_and_the_digest_only_names_the_build() {
        let data = holding();
        assert_eq!(whole(File::of(data.clone())), Step::Decided(Build::Certified(crate::sha256::sha256(&data))));
        let mut patched = data.clone();
        patched[0x2c00] ^= 1;
        assert_eq!(whole(File::of(patched.clone())), Step::Decided(Build::Certified(crate::sha256::sha256(&patched))));
        assert_ne!(crate::sha256::sha256(&patched), crate::sha256::sha256(&data));
        for (name, at) in one_byte_of_each_check() {
            let mut changed = data.clone();
            changed[at] ^= 0x40;
            assert_eq!(
                whole(File::of(changed.clone())),
                Step::Decided(Build::Uncertified(crate::sha256::sha256(&changed), name)),
                "byte {at:#x}"
            );
        }
        // A file that is not an executable at all.
        let other = contents(12_345);
        assert_eq!(whole(File::of(other.clone())), Step::Decided(Build::Uncertified(crate::sha256::sha256(&other), "pe_header")));
    }

    /// The fault of the review: the hash had ten seconds in one go, on the thread that owes
    /// the plugin a heartbeat. In slices it goes on from where it stopped, every byte of the
    /// file goes into the hash once and in order, and a slow disk only means more slices.
    #[test]
    fn the_hash_goes_on_across_slices_from_where_it_stopped() {
        let mut data = holding();
        data.extend(contents(40_500 - data.len()));
        let mut hashing = Hashing::start_with(File::of(data.clone()), certificate_and_file().0).ok().unwrap();
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
        assert_eq!(outcome, Step::Decided(Build::Certified(crate::sha256::sha256(&data))));
        assert_eq!(hashing.file.fed, data, "every byte once, in order");
        assert_eq!(hashing.file.reads, 42);
        // The same digest as in one go.
        assert_eq!(hashing.file.finish(), Some(crate::sha256::sha256(&data)));
    }

    /// Ten slices, and not one before: the tenth is the one after which the source speaks.
    #[test]
    fn the_hash_is_overdue_after_ten_seconds_of_slices_and_not_before() {
        assert_eq!(OVERDUE, Duration::from_secs(10));
        assert!(!overdue(Duration::ZERO));
        assert!(!overdue(SLICE * 9));
        assert!(!overdue(OVERDUE - Duration::from_millis(1)));
        assert!(overdue(SLICE * 10));
        assert!(overdue(Duration::from_secs(3600)));
        // A whole number of slices, so that the count of passes is what the doc says it is.
        assert_eq!(OVERDUE.as_millis() % SLICE.as_millis(), 0);
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
        assert!(matches!(hashing.advance(|| false), Step::Decided(Build::Uncertified(_, "pe_header"))));
    }

    /// The lines say which build, 12 hex digits, MiB and time; by size alone there is no digest.
    /// None carries a path or a name: only what the arguments are.
    #[test]
    fn the_executable_lines_give_the_build_a_short_digest_the_size_and_the_time() {
        let digest = crate::sha256::sha256(b"abc");
        assert_eq!(short_digest(&digest), "ba7816bf8f01");
        let ninety_mib = 90 * 1024 * 1024 + 512 * 1024;
        assert_eq!(
            decided_line(Build::Uncertified(digest, "mf:modifier_sum"), ninety_mib, Duration::from_millis(4120)),
            "game executable: not certified, mf:modifier_sum differs, sha256 ba7816bf8f01, 90.5 MiB, hashed in 4120 ms"
        );
        assert_eq!(
            decided_line(Build::Certified(digest), 43_065_736, Duration::from_millis(370)),
            "game executable: certified by content (code-208704-r1), sha256 ba7816bf8f01, 41.0 MiB, hashed in 370 ms"
        );
        // Decided by size: no digest and no time, whatever the caller passes.
        let by_size = "game executable: other build by size, 0.0 MiB, not hashed";
        assert_eq!(decided_line(Build::BySize, 0, Duration::from_secs(9)), by_size);
        assert_eq!(size_line(MAX_BYTES + 1), "game executable: other build by size, 128.0 MiB, not hashed");
        assert_eq!(
            addon_line("0.8.8", &digest, 2 * 1024 * 1024 + 300_000, Duration::from_millis(9)),
            "addon build: v0.8.8, sha256 ba7816bf8f01, 2.2 MiB, hashed in 9 ms"
        );
        assert_eq!(failure_line("open", 3), "game executable: verification failed, first failure open, 3 so far");
    }

    /// What the system fails to do is not an answer about the file: not even with a file that
    /// holds the certificate.
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
            let mut file = File::of(holding());
            fault(&mut file);
            assert_eq!(whole(file), Step::Failed, "{what}");
        }
        // The same file with nothing wrong is decided.
        let sound = holding();
        assert_eq!(whole(File::of(sound.clone())), Step::Decided(Build::Certified(crate::sha256::sha256(&sound))));
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
        let broken: [(&str, fn(&mut Vec<u8>)); 8] = [
            ("no MZ", |bytes| bytes[..2].copy_from_slice(b"ZM")),
            ("a PE offset before the DOS header ends", |bytes| bytes[0x3c..0x40].copy_from_slice(&0x3fu32.to_le_bytes())),
            ("a PE offset past the first page", |bytes| bytes[0x3c..0x40].copy_from_slice(&4097u32.to_le_bytes())),
            ("no PE signature", |bytes| bytes[PE..PE + 4].copy_from_slice(b"PE\0\x01")),
            ("another machine", |bytes| bytes[PE + 4..PE + 6].copy_from_slice(&0x014cu16.to_le_bytes())),
            ("a PE32 optional header", |bytes| bytes[PE + 24..PE + 26].copy_from_slice(&0x10bu16.to_le_bytes())),
            ("an image too small for the profile", |bytes| bytes[PE + 80..PE + 84].copy_from_slice(&(TLS_INDEX_RVA as u32).to_le_bytes())),
            ("an image of another size than the certified file", |bytes| bytes[PE + 80..PE + 84].copy_from_slice(&(crate::certificate::IMAGE_SIZE + 0x1000).to_le_bytes())),
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
