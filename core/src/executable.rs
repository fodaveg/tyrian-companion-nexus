//! Whether the game's executable holds what the readers rely on: what decides it for good, and
//! what only means "not now".
//!
//! The readers touch a game field only after the executable was read whole, found to hold what
//! they rely on, and its loaded image checked. Its SHA-256 is taken on the same pass and only
//! names the build: since 16 October 2026 an update of the game that leaves that code as it was
//! does not stop the readers. What they rely on is found in one of two ways, in this order
//! (`docs/audit/2026-10-16-relocation-study/README.md`, section 4, rule 1):
//!
//! - **by identity**: the file holds the certificate at its fixed offsets
//!   ([`crate::certificate`]), and is read with [`Layout::CERTIFIED`] ([`Build::Certified`]);
//! - **by relocation**, only when it does not: the relocation table's resolver
//!   ([`crate::resolver`]) finds every anchor again in the file, and it is read with the layout
//!   that gives ([`Build::Relocated`]). One anchor it does not find is another build.
//!
//! Some answers depend on nothing but the file and the image: its size, its content, its
//! header. Those are final for as long as the process lives, whichever way they go, and a "no"
//! among them is another build: the source says so and stops. Every other way of not getting an
//! answer is the system's doing, a file that cannot be opened or read or a copy that fails, and
//! is tried again later.
//!
//! This module holds that rule, with the file, the hash and the memory handed in, so that it
//! is tested here. The Windows adapter only opens the file, runs the system's SHA-256 over
//! what this asks it to, reads it at the points the resolver asks for and copies the bytes of
//! the image.

use std::time::{Duration, SystemTime};

use crate::certificate::{Certificate, Collector, CERTIFICATE, GAME};
use crate::inventory::{BuildProfile, Memory, ReadError, Reader};
use crate::layout::Layout;
use crate::live::RESPONSE_TIMEOUT;
use crate::passive::{Guard, GuardDigests};
use crate::resolver::{self, rule_name, Failure, Resolution, Resolved, Scanner, Table};
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
///
/// The resolution of a build whose code moved goes in the same slices, on the passes after the
/// hash: a read at a point is 4 KiB, less than the hash's 64 KiB, and it too is asked for only
/// while the slice lasts.
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

/// Whether the file holds what the readers rely on, how that was found, and its SHA-256. The
/// SHA-256 is whatever the game's update made it: it is the build `live_open` names, not a
/// condition, on either path.
///
/// The variants say which path the readers go by, because that is what they read with:
/// `Certified` keeps the name it had, the identity path the readers have always gone by;
/// `Relocated` is the resolver's own word for a build every anchor of which was found again
/// (`resolver::Mode::Relocated`, and the log's "relocated by content"); `Uncertified` keeps its
/// name too, but what it carries is now the resolver's first failure, since the certificate
/// failing at its fixed offsets no longer refuses a build on its own. A [`Failure`] and a
/// [`Relocated`] are kept by a `'static` reference, so that `Build` stays `Copy`: a process
/// decides its executable once, and keeps that answer for as long as it lives.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Build {
    /// It holds the certificate at its fixed offsets: read with [`Layout::CERTIFIED`] and the
    /// audited digests ([`GuardDigests::CERTIFIED`]).
    Certified([u8; 32]),
    /// It does not, and the resolver placed every anchor and vtable of the relocation table
    /// again: read with the layout and the digests of the file it found ([`Relocated`]).
    Relocated([u8; 32], &'static Relocated),
    /// Neither: its SHA-256 and the first rule that did not hold, as the resolver says it. The
    /// readers never get a layout from it, not even part of one.
    Uncertified([u8; 32], &'static Failure),
    /// Decided by size alone, before a byte was read.
    BySize,
}

/// What a build whose code moved is read with, as its resolution gave it.
#[derive(Debug, PartialEq, Eq)]
pub struct Relocated {
    /// Every RVA the readers use ([`Layout::from_resolved`]).
    pub layout: Layout,
    /// What each guard must find in memory: the file's bytes at the range the layout places.
    pub digests: GuardDigests,
    /// How many anchors and vtables of the table are not where the certified build has them.
    pub moved: usize,
}

impl Build {
    /// What the readers read this build with: its layout and its guards' digests. `None` for a
    /// build that is not read.
    pub fn readable(&self) -> Option<(&'static Layout, &'static GuardDigests)> {
        match self {
            Build::Certified(_) => Some((&Layout::CERTIFIED, &GuardDigests::CERTIFIED)),
            Build::Relocated(_, relocated) => Some((&relocated.layout, &relocated.digests)),
            Build::Uncertified(..) | Build::BySize => None,
        }
    }

    /// The SHA-256 of the file, when it was hashed.
    pub fn digest(&self) -> Option<&[u8; 32]> {
        match self {
            Build::Certified(digest) | Build::Relocated(digest, _) | Build::Uncertified(digest, _) => Some(digest),
            Build::BySize => None,
        }
    }
}

/// What the readers read a resolved executable with, or why it is not read: the layout of its
/// places ([`Layout::from_resolved`]), and the digest of the file at each guard's range.
///
/// Every failure is rule 9, all or nothing, named by what it lacks: the anchor of a field the
/// resolution did not place (or that `table` does not have), the relation of the layout that
/// breaks (`layout`), or the guard no anchor of the table is (`bag:`, `wallet:` or `mf:` and its
/// name). A guard's range must be exactly one anchor's, at its place and of its length: the
/// digest the guard is given is that anchor's, taken over the file by the resolver.
///
/// Each digest is kept as a `'static` string, as the audited ones are: called once per process,
/// when the build is decided.
pub fn relocated(resolved: &Resolved, table: &Table) -> Result<Relocated, Failure> {
    let refused = |anchor: &str, detail: String| Failure { anchor: anchor.to_string(), rule: 9, detail };
    let layout = Layout::from_names(resolved)
        .map_err(|name| refused(name, "the layout reads it and the resolution did not place it".into()))?;
    layout.validate().map_err(|relation| refused("layout", format!("the resolved layout breaks: {relation}")))?;
    let digests = GuardDigests {
        bags: guard_digests("bag", crate::bags::guards_of(&layout), resolved, table)?,
        wallet: guard_digests("wallet", crate::wallet::guards_of(&layout), resolved, table)?,
        magic_find: guard_digests("mf", crate::magic_find::guards_of(&layout), resolved, table)?,
    };
    let anchors = resolved.anchors.iter().zip(table.anchors).filter(|(p, a)| p.rva.is_some_and(|rva| rva != u64::from(a.rva)));
    let vtables = resolved.vtables.iter().zip(table.vtables).filter(|(p, v)| p.rva != Some(u64::from(v.rva)));
    Ok(Relocated { layout, digests, moved: anchors.count() + vtables.count() })
}

/// The digest of the file at each of one reader's guards, by the anchor of the table whose name
/// has `prefix:name` as one of its parts.
fn guard_digests<const N: usize>(
    prefix: &str,
    guards: [Guard; N],
    resolved: &Resolved,
    table: &Table,
) -> Result<[&'static str; N], Failure> {
    let mut out = [""; N];
    for (digest, guard) in out.iter_mut().zip(&guards) {
        let name = format!("{prefix}:{}", guard.name);
        let found = resolved.anchors.iter().zip(table.anchors).find(|(p, _)| p.name.split(" + ").any(|part| part == name));
        let file_sha256 = match found {
            Some((p, a)) if p.rva == Some(guard.rva) && a.len as usize == guard.size => p.file_sha256,
            _ => None,
        };
        let Some(file_sha256) = file_sha256 else {
            return Err(Failure {
                anchor: name,
                rule: 9,
                detail: format!("no anchor of the table is this guard's range, {} bytes at {:#x}", guard.size, guard.rva),
            });
        };
        *digest = Box::leak(hex(&file_sha256).into_boxed_str());
    }
    Ok(out)
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

/// A failure of the resolver as the log and the panel say it: the anchor, the rule by its number
/// and its name, and why.
fn refusal(failure: &Failure) -> String {
    format!("{} rule {} ({}): {}", failure.anchor, failure.rule, rule_name(failure.rule), failure.detail)
}

/// The log line for an executable the hash decided: which build and how it was found, the first
/// 12 hex digits of its SHA-256, its size in MiB and how long the hash took; for one that did
/// not hold the certificate at its fixed offsets, how long its resolution took, and how many
/// anchors moved or the first rule that failed. No path and no name of the file, and no byte of
/// it: a failure names RVAs and counts.
///
/// The failure goes first, as the other lines say what the build is first, and its detail is
/// closed by a semicolon: it is free text with commas of its own.
pub fn decided_line(build: Build, bytes: u64, hashed: Duration, resolved: Duration) -> String {
    let size = format!("{} MiB, hashed in {} ms", mib(bytes), hashed.as_millis());
    match build {
        Build::Certified(digest) => {
            format!("game executable: certified by content ({CERTIFICATE}), sha256 {}, {size}", short_digest(&digest))
        }
        Build::Relocated(digest, relocated) => format!(
            "game executable: relocated by content ({CERTIFICATE}), sha256 {}, {size}, {} anchors moved, resolved in {} ms",
            short_digest(&digest),
            relocated.moved,
            resolved.as_millis()
        ),
        Build::Uncertified(digest, failure) => format!(
            "game executable: not certified, {}; sha256 {}, {size}, resolved in {} ms",
            refusal(failure),
            short_digest(&digest),
            resolved.as_millis()
        ),
        Build::BySize => size_line(bytes),
    }
}

/// What the Options window says of the executable: how it was found, or why it is not read.
/// `None` is a verification still under way, or one the system failed and will try again.
pub fn panel_line(build: Option<Build>) -> String {
    match build {
        None => "Game executable: not verified yet".to_string(),
        Some(Build::Certified(_)) => format!("Game executable: certified by content ({CERTIFICATE})"),
        Some(Build::Relocated(_, relocated)) => {
            format!("Game executable: relocated by content ({} anchors moved)", relocated.moved)
        }
        Some(Build::Uncertified(_, failure)) => format!("Game executable: not certified, {}", refusal(failure)),
        Some(Build::BySize) => "Game executable: other build by size".to_string(),
    }
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
    /// At most `buffer.len()` bytes of it from `offset`, `Some(0)` past its end, without
    /// changing where [`HashedFile::read`] goes on. The resolution of the relocation table
    /// ([`Hashing::resolve`]) reads with it at points, once the hash has ended.
    fn read_at(&mut self, offset: u64, buffer: &mut [u8]) -> Option<usize>;
}

/// The hash of the executable, under way: the file where the last slice left it, and how much
/// of it has gone into the hash.
pub struct Hashing<F> {
    file: F,
    before: Stamp,
    hashed: u64,
    /// The bytes the certificate names, copied as the file goes by.
    collector: Collector,
    /// The relocation table the resolution goes by.
    table: &'static Table,
    /// The first phase of the relocation table's resolution, on the same pass.
    scanner: Scanner,
    /// Its second phase, once the hash has ended without the certificate at its fixed offsets
    /// ([`Hashing::advance`]), or [`Hashing::resolve`] is called.
    resolution: Option<Resolution>,
    /// The hash ended with a digest of the whole file.
    decided: bool,
    /// The digest, while the resolution that decides the build is under way.
    resolving: Option<[u8; 32]>,
}

impl<F: HashedFile> Hashing<F> {
    /// Starts on `file`. One of no bytes, or larger than [`MAX_BYTES`], is decided here as
    /// another build, and not a byte of it is read.
    pub fn start(file: F) -> Result<Self, Step<Build>> {
        Self::start_with(file, &GAME)
    }

    /// [`Hashing::start`] against another certificate than the game's: the tests'.
    pub fn start_with(file: F, certificate: &'static Certificate) -> Result<Self, Step<Build>> {
        Self::start_with_table(file, certificate, &resolver::GAME)
    }

    /// [`Hashing::start_with`] with another relocation table than the game's: the tests'.
    pub fn start_with_table(
        mut file: F,
        certificate: &'static Certificate,
        table: &'static Table,
    ) -> Result<Self, Step<Build>> {
        let Some(before) = file.stamp() else { return Err(Step::Failed) };
        if !size_in_range(before.len) {
            return Err(Step::Decided(Build::BySize));
        }
        Ok(Self {
            file,
            before,
            hashed: 0,
            collector: Collector::new(certificate),
            table,
            scanner: Scanner::new(table),
            resolution: None,
            decided: false,
            resolving: None,
        })
    }

    /// Hashes more of the file, until it ends or `expired` says this slice is over, which it
    /// is asked before every read. The caller makes a slice [`SLICE`] long and ends it at once
    /// when it is told to stop.
    ///
    /// At the end of the file it must still be the file the hash began on, by its size, its
    /// last write and the number of bytes that came out of it. One that changed meanwhile is
    /// not decided: the hash is of neither file.
    ///
    /// A file that holds the certificate at its fixed offsets is decided there:
    /// [`Build::Certified`]. One that does not is decided by the resolution of the relocation
    /// table: the call that ends the hash is `Unfinished`, and the next ones each do a step of
    /// it, asking `expired` before every read as the hash does, until it gives
    /// [`Build::Relocated`] or [`Build::Uncertified`]. The resolution starts on a call of its
    /// own so that the time of each phase is the time of its own calls
    /// ([`Hashing::resolving`]).
    pub fn advance(&mut self, mut expired: impl FnMut() -> bool) -> Step<Build> {
        if let Some(digest) = self.resolving {
            return self.relocate(digest, expired);
        }
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
            self.scanner.feed(at, bytes);
        }
        if self.hashed != self.before.len || self.file.stamp() != Some(self.before) {
            return Step::Failed;
        }
        match self.file.finish() {
            Some(digest) => {
                self.decided = true;
                if self.collector.verdict().is_ok() {
                    return Step::Decided(Build::Certified(digest));
                }
                self.resolving = Some(digest);
                Step::Unfinished
            }
            None => Step::Failed,
        }
    }

    /// One step of the resolution that decides a file whose hash ended without the certificate
    /// at its fixed offsets: a layout of every place, or the first rule that does not hold.
    fn relocate(&mut self, digest: [u8; 32], expired: impl FnMut() -> bool) -> Step<Build> {
        match self.resolve_by(false, expired) {
            Step::Unfinished => Step::Unfinished,
            Step::Failed => Step::Failed,
            Step::Decided(resolution) => {
                let table = self.table;
                Step::Decided(match resolution.and_then(|resolved| relocated(&resolved, table)) {
                    Ok(relocated) => Build::Relocated(digest, Box::leak(Box::new(relocated))),
                    Err(failure) => Build::Uncertified(digest, Box::leak(Box::new(failure))),
                })
            }
        }
    }

    /// How many bytes of the file have gone into the hash so far.
    pub fn hashed(&self) -> u64 {
        self.hashed
    }

    /// Whether the hash is over and what [`Hashing::advance`] does now is the resolution: the
    /// adapter counts the time of these calls as the resolution's.
    pub fn resolving(&self) -> bool {
        self.resolving.is_some()
    }

    /// One step of the resolution of the relocation table ([`crate::resolver`]) over this file,
    /// once [`Hashing::advance`] has decided: where every anchor and vtable is, or the first rule
    /// that does not hold. `expired` is asked before every read, as in `advance`, and a step
    /// that it ends is `Unfinished`; the next call goes on. The file held the certificate at its
    /// fixed offsets or not by the same verdict as the [`Build`] (rule 1, identity first).
    ///
    /// It is the same resolution [`Hashing::advance`] decides a build by, and shares its pages:
    /// called after `advance`, it gives what `advance` went by without reading again. A file
    /// that changed since it was hashed, or a read the system fails, is `Failed`, as is a call
    /// before the hash has ended.
    pub fn resolve(&mut self, expired: impl FnMut() -> bool) -> Step<Result<Resolved, Failure>> {
        self.resolve_by(false, expired)
    }

    /// [`Hashing::resolve`] without the identity path: every place found by the search and the
    /// graph, as `gen_relocation.py --resolve --force-search` does.
    pub fn resolve_by_search(&mut self, expired: impl FnMut() -> bool) -> Step<Result<Resolved, Failure>> {
        self.resolve_by(true, expired)
    }

    fn resolve_by(&mut self, force_search: bool, mut expired: impl FnMut() -> bool) -> Step<Result<Resolved, Failure>> {
        if !self.decided {
            return Step::Failed;
        }
        let identity = !force_search && self.collector.verdict().is_ok();
        let scanner = &self.scanner;
        let resolution = self.resolution.get_or_insert_with(|| Resolution::new(scanner));
        let file = &mut self.file;
        let step = resolution.step(identity, &mut |offset, buffer| file.read_at(offset, buffer), &mut expired);
        if matches!(step, Step::Decided(_)) && self.file.stamp() != Some(self.before) {
            return Step::Failed;
        }
        step
    }

    /// How many reads at points the resolution has made, and how many bytes they brought.
    pub fn resolution_reads(&self) -> (u64, u64) {
        self.resolution.as_ref().map_or((0, 0), Resolution::reads)
    }

    /// The first phase of the resolution, for its measures.
    pub fn scanner(&self) -> &Scanner {
        &self.scanner
    }
}

/// Whether a file of `len` bytes can be the certified build at all. One that cannot is decided
/// by its size alone, without hashing a byte of it.
pub fn size_in_range(len: u64) -> bool {
    len != 0 && len <= MAX_BYTES
}

/// The profile of the image loaded at `base`, for a process whose executable file is read with
/// `layout` and `digests` ([`Build::readable`]): `Some` when its header is the one the file says
/// (an AMD64 PE32+ image of the size of that layout, the file's own and not a constant), `None`
/// when it is not.
///
/// The header is in memory for as long as the process lives and does not change, so a header
/// that is not the expected one is as final as a digest that is not. Only a copy that failed
/// says nothing about it.
pub fn image_profile<M: Memory>(
    reader: &mut Reader<M>,
    base: u64,
    layout: &'static Layout,
    digests: &'static GuardDigests,
) -> Step<Option<BuildProfile>> {
    match header(reader, base, layout, digests) {
        Ok(profile) => Step::Decided(Some(profile)),
        Err(ReadError::ReadFailed) => Step::Failed,
        Err(_) => Step::Decided(None),
    }
}

/// An AMD64 PE32+ image of the size `layout` gives, with room for the fields the profile reads.
fn header<M: Memory>(
    reader: &mut Reader<M>,
    base: u64,
    layout: &'static Layout,
    digests: &'static GuardDigests,
) -> Result<BuildProfile, ReadError> {
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
    if image_size != layout.image_size {
        return Err(ReadError::UnsupportedBuild);
    }
    BuildProfile::checked_with(base, layout, digests)
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
        match image_profile(&mut Reader::new(Image { bytes, readable }), BASE, &Layout::CERTIFIED, &GuardDigests::CERTIFIED) {
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
        fn read_at(&mut self, offset: u64, buffer: &mut [u8]) -> Option<usize> {
            let from = (offset as usize).min(self.contents.len());
            let count = buffer.len().min(self.contents.len() - from);
            buffer[..count].copy_from_slice(&self.contents[from..from + count]);
            Some(count)
        }
    }

    fn contents(len: usize) -> Vec<u8> {
        (0..len).map(|index| (index * 31 % 251) as u8).collect()
    }

    /// Hashes `file` in one go against the synthetic certificate, and resolves it if it does not
    /// hold it, and says how it ended.
    fn whole(file: File) -> Step<Build> {
        whole_with(file).0
    }

    /// [`whole`], and the first check of the certificate the file did not hold.
    fn whole_with(file: File) -> (Step<Build>, Result<(), &'static str>) {
        match Hashing::start_with(file, certificate_and_file().0) {
            Ok(mut hashing) => loop {
                match hashing.advance(|| false) {
                    Step::Unfinished => assert!(hashing.resolving(), "a slice that never expires ends only the hash"),
                    step => break (step, hashing.collector.verdict()),
                }
            },
            Err(step) => (step, Ok(())),
        }
    }

    /// A refusal of the resolver, as the build keeps it.
    fn failure(anchor: &str, rule: u8, detail: &str) -> &'static Failure {
        Box::leak(Box::new(Failure { anchor: anchor.into(), rule, detail: detail.into() }))
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
    /// that differs in any check is not certified by identity. The synthetic certificate's file
    /// has none of the game's sections, so its resolution fails, by the header (rule 8).
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
            let (step, check) = whole_with(File::of(changed.clone()));
            assert_eq!(check, Err(name), "byte {at:#x}");
            let Step::Decided(Build::Uncertified(digest, failure)) = step else { panic!("byte {at:#x}: {step:?}") };
            assert_eq!(digest, crate::sha256::sha256(&changed));
            assert_eq!((failure.anchor.as_str(), failure.rule), ("header", 8), "byte {at:#x}: {failure}");
        }
        // A file that is not an executable at all.
        let other = contents(12_345);
        assert_eq!(
            whole(File::of(other.clone())),
            Step::Decided(Build::Uncertified(crate::sha256::sha256(&other), failure("header", 8, "not MZ")))
        );
    }

    /// A file that does not hold the certificate at its fixed offsets is decided by its
    /// resolution, on the calls after the one that ended the hash; one whose resolution fails,
    /// or gives none of what the readers read, is another build, with nothing to read it with.
    #[test]
    fn a_file_without_the_certificate_at_its_offsets_is_decided_by_its_resolution() {
        use crate::resolver::synthetic::{certificate, file, moved, off, table};
        let start = |data: Vec<u8>| Hashing::start_with_table(File::of(data), certificate(), table()).ok().unwrap();
        // Code moved: the synthetic table resolves, but names nothing the readers read, so there
        // is no layout to read it with, not a part of one.
        let data = file(&moved());
        let mut hashing = start(data.clone());
        assert_eq!(hashing.advance(|| false), Step::Unfinished);
        assert!(hashing.resolving());
        assert_eq!(hashing.resolution_reads(), (0, 0), "the call that ends the hash ends there");
        let step = hashing.advance(|| false);
        let expected = failure("vtable char_context", 9, "the layout reads it and the resolution did not place it");
        assert_eq!(step, Step::Decided(Build::Uncertified(crate::sha256::sha256(&data), expected)));
        assert!(hashing.resolution_reads().0 > 0);
        let Step::Decided(build) = step else { unreachable!() };
        assert_eq!(build.readable(), None);
        // The resolution fails: its first failure is what the build says.
        let mut broken = data.clone();
        broken[off(moved().lonely + 10)] ^= 1;
        let mut hashing = start(broken.clone());
        let step = loop {
            match hashing.advance(|| false) {
                Step::Unfinished => continue,
                step => break step,
            }
        };
        let Step::Decided(Build::Uncertified(digest, failure)) = step else { panic!("{step:?}") };
        assert_eq!(digest, crate::sha256::sha256(&broken));
        assert_eq!(
            failure.to_string(),
            "not resolved: s:lonely: rule 9 (all or nothing): not located (search: 0 matches, certified matches 1; no derivation reached it)"
        );
        // And `resolve` gives, after `advance`, the resolution it went by, with no read more.
        let reads = hashing.resolution_reads();
        assert!(matches!(hashing.resolve(|| true), Step::Decided(Err(f)) if f == *failure));
        assert_eq!(hashing.resolution_reads(), reads);
        // The certified file is decided where its hash ends, without a resolution.
        let mut hashing = start(file(&crate::resolver::synthetic::certified()));
        assert!(matches!(hashing.advance(|| false), Step::Decided(Build::Certified(_))));
        assert_eq!((hashing.resolving(), hashing.resolution_reads()), (false, (0, 0)));
    }

    /// What the readers get from a resolution: the certified layout and digests from the one of
    /// identity, and the first thing missing from any other, never a layout with a hole in it.
    #[test]
    fn a_resolution_gives_the_layout_and_the_guard_digests_or_the_first_thing_missing() {
        use crate::layout::tests::identity;
        let certified = relocated(&identity(), &resolver::GAME).unwrap();
        assert_eq!(certified, Relocated { layout: Layout::CERTIFIED, digests: GuardDigests::CERTIFIED, moved: 0 });
        // A guard whose file digest the resolution does not have.
        let mut resolved = identity();
        resolved.anchors.iter_mut().find(|p| p.name == "wallet:balance_getter").unwrap().file_sha256 = None;
        assert_eq!(
            relocated(&resolved, &resolver::GAME),
            Err(Failure { anchor: "wallet:balance_getter".into(), rule: 9, detail: "no anchor of the table is this guard's range, 80 bytes at 0x12722a0".into() })
        );
        // A field the resolution did not place.
        let mut resolved = identity();
        resolved.anchors.iter_mut().find(|p| p.name == "mf:buff_lookup").unwrap().rva = None;
        assert_eq!(
            relocated(&resolved, &resolver::GAME).map_err(|f| f.to_string()),
            Err("not resolved: mf:buff_lookup: rule 9 (all or nothing): the layout reads it and the resolution did not place it".into())
        );
        // A layout that breaks a relation of the readers.
        let mut resolved = identity();
        resolved.tls_index = 0x100;
        assert_eq!(
            relocated(&resolved, &resolver::GAME).map_err(|f| f.anchor + ": " + &f.detail),
            Err("layout: the resolved layout breaks: rdata_vtables ends before tls_index".into())
        );
        // A moved anchor and its file digest: counted, and given to its guard.
        let mut resolved = identity();
        let place = resolved.anchors.iter_mut().find(|p| p.name == "bag:slot_validity").unwrap();
        place.rva = place.rva.map(|rva| rva + 0x40);
        place.file_sha256 = Some(crate::sha256::sha256(b"moved"));
        let moved = relocated(&resolved, &resolver::GAME).unwrap();
        assert_eq!((moved.moved, moved.layout.slot_validity), (1, Layout::CERTIFIED.slot_validity + 0x40));
        let at = crate::bags::GUARDS.iter().position(|g| g.name == "slot_validity").unwrap();
        assert_eq!(moved.digests.bags[at], hex(&crate::sha256::sha256(b"moved")));
        assert_eq!(moved.digests.wallet, GuardDigests::CERTIFIED.wallet);
    }

    /// The model of the worker's passes, on a disk as slow as the doc of [`SLICE`] allows: every
    /// read of the file, in order or at a point, costs a quarter of a second on the model's
    /// clock, and a call is given [`SLICE`] of it. No call of either phase goes past its slice by
    /// more than the read under way when it ran out, the resolution included, and the worst wait
    /// of a heartbeat (its interval, one call and the worker's wait on its socket) stays within
    /// the six and a half seconds the doc of [`SLICE`] gives.
    ///
    /// The clock moves only with reads. What a step spends between reads, going over the pages it
    /// already has, is measured on the real copy (`a_moved_copy_is_relocated_in_slices`).
    #[test]
    fn no_call_goes_past_its_slice_and_the_heartbeat_holds_in_the_worst_case() {
        use crate::resolver::synthetic::{certificate, file, moved, table};
        use std::cell::Cell;
        use std::rc::Rc;
        const READ: Duration = Duration::from_millis(250);
        struct Slow {
            file: File,
            clock: Rc<Cell<Duration>>,
        }
        impl HashedFile for Slow {
            fn stamp(&mut self) -> Option<Stamp> {
                self.file.stamp()
            }
            fn read(&mut self, buffer: &mut [u8]) -> Option<usize> {
                self.clock.set(self.clock.get() + READ);
                self.file.read(buffer)
            }
            fn update(&mut self, bytes: &[u8]) -> bool {
                self.file.update(bytes)
            }
            fn finish(&mut self) -> Option<[u8; 32]> {
                self.file.finish()
            }
            fn read_at(&mut self, offset: u64, buffer: &mut [u8]) -> Option<usize> {
                self.clock.set(self.clock.get() + READ);
                self.file.read_at(offset, buffer)
            }
        }
        let clock = Rc::new(Cell::new(Duration::ZERO));
        let mut slow = File::of(file(&moved()));
        slow.chunk = 65536;
        let mut hashing = Hashing::start_with_table(Slow { file: slow, clock: clock.clone() }, certificate(), table()).ok().unwrap();
        let (mut longest, mut hash_calls, mut resolution_calls) = (Duration::ZERO, 0, 0);
        let outcome = loop {
            let start = clock.get();
            let until = start + SLICE;
            let resolving = hashing.resolving();
            let step = hashing.advance(|| clock.get() >= until);
            let took = clock.get() - start;
            assert!(took <= SLICE + READ, "a call of {took:?}");
            longest = longest.max(took);
            if resolving {
                resolution_calls += 1;
            } else {
                hash_calls += 1;
            }
            match step {
                Step::Unfinished => continue,
                step => break step,
            }
        };
        assert!(matches!(outcome, Step::Decided(Build::Uncertified(..))), "{outcome:?}");
        let (reads, _) = hashing.resolution_reads();
        assert!(resolution_calls > 1 && reads > 4, "{resolution_calls} calls, {reads} reads: the resolution went over several slices");
        assert_eq!(hash_calls, 1, "two reads of 64 KiB and the end: one slice");
        let heartbeat = Duration::from_millis(crate::protocol::DEFAULT_HEARTBEAT_INTERVAL_MS);
        let worst = heartbeat + longest + crate::client::READ_POLL_INTERVAL;
        assert!(worst <= Duration::from_millis(6_500), "a heartbeat waits up to {worst:?}");
        println!("model: {hash_calls} call of hash, {resolution_calls} of resolution ({reads} reads), longest {longest:?}, worst heartbeat {worst:?}");
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
        // And it is still good for the rest: the hash ends, and the resolution refuses the file.
        assert_eq!(hashing.advance(|| false), Step::Unfinished);
        assert!(matches!(hashing.advance(|| false), Step::Decided(Build::Uncertified(_, f)) if f.detail == "not MZ"));
    }

    /// The lines say which build, 12 hex digits, MiB and time; by size alone there is no digest.
    /// None carries a path or a name: only what the arguments are.
    #[test]
    fn the_executable_lines_give_the_build_a_short_digest_the_size_and_the_time() {
        let digest = crate::sha256::sha256(b"abc");
        assert_eq!(short_digest(&digest), "ba7816bf8f01");
        let ninety_mib = 90 * 1024 * 1024 + 512 * 1024;
        let refused = failure("mf:modifier_sum", 4, "field +12 (rel32) calls 0x12c5000, which is not the certified callee (function, 96 bytes)");
        assert_eq!(
            decided_line(Build::Uncertified(digest, refused), ninety_mib, Duration::from_millis(4120), Duration::from_millis(85)),
            "game executable: not certified, mf:modifier_sum rule 4 (every masked field points where it pointed): \
             field +12 (rel32) calls 0x12c5000, which is not the certified callee (function, 96 bytes); \
             sha256 ba7816bf8f01, 90.5 MiB, hashed in 4120 ms, resolved in 85 ms"
        );
        assert_eq!(
            decided_line(Build::Certified(digest), 43_065_736, Duration::from_millis(370), Duration::from_secs(9)),
            "game executable: certified by content (code-208704-r1), sha256 ba7816bf8f01, 41.0 MiB, hashed in 370 ms"
        );
        let relocated = &*Box::leak(Box::new(Relocated { layout: Layout::CERTIFIED, digests: GuardDigests::CERTIFIED, moved: 37 }));
        assert_eq!(
            decided_line(Build::Relocated(digest, relocated), 43_065_736, Duration::from_millis(370), Duration::from_millis(120)),
            format!(
                "game executable: relocated by content ({CERTIFICATE}), sha256 ba7816bf8f01, 41.0 MiB, hashed in 370 ms, \
                 37 anchors moved, resolved in 120 ms"
            )
        );
        // Decided by size: no digest and no time, whatever the caller passes.
        let by_size = "game executable: other build by size, 0.0 MiB, not hashed";
        assert_eq!(decided_line(Build::BySize, 0, Duration::from_secs(9), Duration::from_secs(9)), by_size);
        // The Options window: how it was found, or why it is not read; nothing before a verdict.
        assert_eq!(panel_line(None), "Game executable: not verified yet");
        assert_eq!(panel_line(Some(Build::Certified(digest))), format!("Game executable: certified by content ({CERTIFICATE})"));
        assert_eq!(panel_line(Some(Build::Relocated(digest, relocated))), "Game executable: relocated by content (37 anchors moved)");
        assert_eq!(
            panel_line(Some(Build::Uncertified(digest, failure("header", 8, "not AMD64 PE32+")))),
            "Game executable: not certified, header rule 8 (header): not AMD64 PE32+"
        );
        assert_eq!(panel_line(Some(Build::BySize)), "Game executable: other build by size");
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

    /// The loaded image must have the size of the layout it is read with, not the certified
    /// one: with a layout of another size, that size is the one accepted and the certified one
    /// is refused.
    #[test]
    fn the_image_size_is_the_layout_s() {
        let layout: &'static Layout = Box::leak(Box::new(Layout {
            image_size: Layout::CERTIFIED.image_size + 0x1000,
            ..Layout::CERTIFIED
        }));
        let read = |bytes: Vec<u8>| {
            header(&mut Reader::new(Image { bytes, readable: true }), BASE, layout, &GuardDigests::CERTIFIED)
                .map(|profile| (profile.base, profile.layout.image_size))
        };
        let mut larger = certified();
        larger[PE + 80..PE + 84].copy_from_slice(&(layout.image_size as u32).to_le_bytes());
        assert_eq!(read(larger), Ok((BASE, layout.image_size)));
        assert_eq!(read(certified()).err(), Some(ReadError::UnsupportedBuild));
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

    // --------------------------------------------------------------------------------------
    // The real file, end to end, when there is a copy to point at: the hash and, for the moved
    // copy, the resolution, in slices of the real clock as the adapter makes them, then the
    // guards of the three readers over the file laid out as its image:
    //
    //   TC_GW2_EXE=<copy> cargo test --offline --release -p tyrian_companion_nexus_core \
    //       --lib executable -- --ignored --nocapture
    //   TC_GW2_MOVED_EXE=<moved copy of test_relocation.py --keep-moved> ...
    // --------------------------------------------------------------------------------------

    /// The image base the copies are laid out at: the file's own, so that no pointer of it moves.
    const IMAGE_BASE: u64 = 0x1_4000_0000;

    /// The file laid out as the loader lays out its sections, at [`IMAGE_BASE`]: what the
    /// readers find in memory, before the base relocations ASLR would apply.
    struct Loaded {
        data: Vec<u8>,
        /// (RVA, virtual size, file offset, raw size) of each section.
        sections: Vec<(u64, u64, u64, u64)>,
    }

    impl Loaded {
        fn of(data: Vec<u8>) -> Self {
            let u16_at = |at: usize| u64::from(u16::from_le_bytes([data[at], data[at + 1]]));
            let u32_at = |at: usize| u64::from(u32::from_le_bytes(data[at..at + 4].try_into().unwrap()));
            let pe = u32_at(0x3c) as usize;
            let table = pe + 24 + u16_at(pe + 20) as usize;
            let sections = (0..u16_at(pe + 6) as usize)
                .map(|i| {
                    let at = table + 40 * i;
                    (u32_at(at + 12), u32_at(at + 8), u32_at(at + 20), u32_at(at + 16))
                })
                .collect();
            Self { data, sections }
        }
    }

    impl crate::inventory::Memory for Loaded {
        fn read_exact(&mut self, address: u64, bytes: &mut [u8]) -> Result<(), ReadError> {
            let rva = address.checked_sub(IMAGE_BASE).ok_or(ReadError::ReadFailed)?;
            let end = rva + bytes.len() as u64;
            let &(va, _, ro, rs) =
                self.sections.iter().find(|s| s.0 <= rva && end <= s.0 + s.1).ok_or(ReadError::ReadFailed)?;
            for (i, byte) in bytes.iter_mut().enumerate() {
                // Past its raw data, a section is zeros in memory.
                let inside = rva + i as u64 - va;
                *byte = if inside < rs { self.data[(ro + inside) as usize] } else { 0 };
            }
            Ok(())
        }
    }

    /// How the adapter's calls went, for each phase: the hash, then the resolution.
    struct Calls {
        count: [u32; 2],
        longest: [Duration; 2],
        spent: [Duration; 2],
    }

    /// Hashes and decides `data` as the adapter does, in calls of at most `slice` of the real
    /// clock: the build, how the calls of each phase went, and the hash.
    fn decide_in_slices(data: Vec<u8>, slice: Duration) -> (Build, Calls, Hashing<File>) {
        let mut file = File::of(data);
        file.chunk = 65536;
        let mut hashing = Hashing::start(file).ok().expect("a size in range");
        let mut calls = Calls { count: [0; 2], longest: [Duration::ZERO; 2], spent: [Duration::ZERO; 2] };
        loop {
            let phase = usize::from(hashing.resolving());
            let began = std::time::Instant::now();
            let until = began + slice;
            let step = hashing.advance(|| std::time::Instant::now() >= until);
            let took = began.elapsed();
            calls.count[phase] += 1;
            calls.longest[phase] = calls.longest[phase].max(took);
            calls.spent[phase] += took;
            match step {
                Step::Unfinished => continue,
                Step::Decided(build) => return (build, calls, hashing),
                Step::Failed => panic!("the system failed"),
            }
        }
    }

    /// The guards of the three readers for `profile`, each compared with the SHA-256 of what
    /// `image` holds at its range: how many hold the profile's digest, of how many.
    fn guards_hold(image: &mut Loaded, profile: BuildProfile) -> (usize, usize) {
        let guards: Vec<Guard> = crate::bags::guards_in(profile)
            .into_iter()
            .chain(crate::wallet::guards_in(profile))
            .chain(crate::magic_find::guards_in(profile))
            .collect();
        let held = guards
            .iter()
            .filter(|guard| {
                let mut bytes = vec![0u8; guard.size];
                crate::inventory::Memory::read_exact(image, IMAGE_BASE + guard.rva, &mut bytes).unwrap();
                hex(&crate::sha256::sha256(&bytes)) == guard.sha256
            })
            .count();
        (held, guards.len())
    }

    /// Whether each reader's own verification accepts `profile` over `image`.
    fn readers_verify(image: Loaded, profile: BuildProfile) -> [bool; 3] {
        let mut reader = Reader::new(image);
        [
            crate::bags::BagProfile::verified(&mut reader, profile).is_ok(),
            crate::wallet::WalletProfile::verified(&mut reader, profile).is_ok(),
            crate::magic_find::MagicFindProfile::verified(&mut reader, profile).is_ok(),
        ]
    }

    #[test]
    #[ignore]
    fn the_copy_of_today_is_certified_by_identity_in_slices() {
        let path = std::env::var("TC_GW2_EXE").expect("TC_GW2_EXE");
        let data = std::fs::read(&path).unwrap();
        let digest = crate::sha256::sha256(&data);
        let (build, calls, mut hashing) = decide_in_slices(data.clone(), SLICE);
        assert_eq!(build, Build::Certified(digest));
        assert_eq!(calls.count[1], 0, "no resolution by identity");
        let (layout, digests) = build.readable().unwrap();
        assert_eq!((layout, digests), (&Layout::CERTIFIED, &GuardDigests::CERTIFIED));
        // The resolution of identity, over the same file, gives the same: the file's digest at
        // every guard is the audited one.
        let Step::Decided(Ok(resolved)) = hashing.resolve(|| false) else { panic!("identity resolves") };
        assert_eq!(resolved.mode, resolver::Mode::Identity);
        let from_file = relocated(&resolved, &resolver::GAME).unwrap();
        assert_eq!((&from_file.layout, &from_file.digests, from_file.moved), (&Layout::CERTIFIED, &GuardDigests::CERTIFIED, 0));
        let profile = BuildProfile::checked_with(IMAGE_BASE, layout, digests).unwrap();
        assert_eq!(guards_hold(&mut Loaded::of(data.clone()), profile), (36, 36));
        assert_eq!(readers_verify(Loaded::of(data.clone()), profile), [true; 3]);
        // In slices of a millisecond: the same build, in many calls.
        let (sliced, small, _) = decide_in_slices(data, Duration::from_millis(1));
        assert_eq!(sliced, build);
        println!("{}", decided_line(build, hashing.hashed(), calls.spent[0], calls.spent[1]));
        println!("{}", panel_line(Some(build)));
        println!(
            "slices of {SLICE:?}: {} call(s), longest {:?}; slices of 1 ms: {} calls, longest {:?}; \
             36 of 36 guards hold the audited digests in the file laid out as its image, and the three readers verify",
            calls.count[0], calls.longest[0], small.count[0], small.longest[0]
        );
    }

    /// The resolution of `gen_relocation.py --resolve` over `exe`, as a [`Resolved`].
    fn python_resolution(exe: &str) -> Resolved {
        let script = concat!(env!("CARGO_MANIFEST_DIR"), "/../scripts/relocation/gen_relocation.py");
        let out = std::env::temp_dir().join(format!("tc-executable-{}.json", std::process::id()));
        let status = std::process::Command::new("python3")
            .args(["-I", script, "--resolve", exe, "--json", out.to_str().unwrap()])
            .stdout(std::process::Stdio::null())
            .status()
            .expect("python3");
        let json = std::fs::read_to_string(&out).unwrap_or_else(|_| panic!("python said {status}"));
        let _ = std::fs::remove_file(&out);
        let py: serde_json::Value = serde_json::from_str(&json).unwrap();
        let number = |v: &serde_json::Value| u64::from_str_radix(v.as_str().unwrap().trim_start_matches("0x"), 16).unwrap();
        let places = |key: &str| {
            py[key]
                .as_array()
                .unwrap()
                .iter()
                .map(|p| resolver::Place {
                    name: Box::leak(p["name"].as_str().unwrap().to_string().into_boxed_str()),
                    rva: p["rva"].as_str().map(|_| number(&p["rva"])),
                    how: None,
                    file_sha256: None,
                })
                .collect()
        };
        Resolved {
            mode: resolver::Mode::Relocated,
            image_size: number(&py["image_size"]) as u32,
            tls_index: number(&py["tls_index_rva"]),
            rdata: (number(&py["rdata"][0]), number(&py["rdata"][1])),
            anchors: places("anchors"),
            vtables: places("vtables"),
            search: None,
        }
    }

    #[test]
    #[ignore]
    fn a_moved_copy_is_relocated_in_slices() {
        let path = std::env::var("TC_GW2_MOVED_EXE").expect("TC_GW2_MOVED_EXE");
        let data = std::fs::read(&path).unwrap();
        let digest = crate::sha256::sha256(&data);
        let (build, calls, hashing) = decide_in_slices(data.clone(), SLICE);
        let Build::Relocated(named, found) = build else { panic!("{}", decided_line(build, 0, Duration::ZERO, Duration::ZERO)) };
        assert_eq!(named, digest, "live_open names the file's own digest");
        assert!(calls.count[1] >= 1);
        // Every call within its slice, and the read under way: well within, on this machine.
        assert!(calls.longest.iter().all(|took| *took <= SLICE + Duration::from_millis(250)), "{:?}", calls.longest);
        // The same build in slices of a millisecond, and how far past such a slice a call goes.
        let (sliced, small, _) = decide_in_slices(data.clone(), Duration::from_millis(1));
        assert_eq!(sliced, build);
        // The layout is the one of the Python resolver's places.
        assert_eq!(Layout::from_resolved(&python_resolution(&path)).as_ref(), Ok(&found.layout));
        assert_ne!(found.layout, Layout::CERTIFIED);
        // The guards: every one holds the file's digest in the file laid out as its image, and the
        // three readers verify; with the audited digests, those whose bytes moved do not.
        let with_file = BuildProfile::checked_with(IMAGE_BASE, &found.layout, &found.digests).unwrap();
        assert_eq!(guards_hold(&mut Loaded::of(data.clone()), with_file), (36, 36));
        assert_eq!(readers_verify(Loaded::of(data.clone()), with_file), [true; 3]);
        let with_certified = BuildProfile::checked(IMAGE_BASE, &found.layout).unwrap();
        let (held, of) = guards_hold(&mut Loaded::of(data.clone()), with_certified);
        let refused = readers_verify(Loaded::of(data), with_certified);
        let differ = |audited: &[&str], file: &[&str]| audited.iter().zip(file).any(|(a, f)| a != f);
        let certified = &GuardDigests::CERTIFIED;
        assert_eq!(
            refused,
            [
                !differ(&certified.bags, &found.digests.bags),
                !differ(&certified.wallet, &found.digests.wallet),
                !differ(&certified.magic_find, &found.digests.magic_find),
            ],
            "a reader verifies with the audited digests exactly when its file digests are those"
        );
        println!("{}", decided_line(build, hashing.hashed(), calls.spent[0], calls.spent[1]));
        println!("{}", panel_line(Some(build)));
        println!(
            "slices of {SLICE:?}: {} call(s) of hash, longest {:?}; {} of resolution, longest {:?}; \
             slices of 1 ms: {} and {} calls, longest {:?} and {:?}",
            calls.count[0], calls.longest[0], calls.count[1], calls.longest[1],
            small.count[0], small.count[1], small.longest[0], small.longest[1]
        );
        println!(
            "the layout is gen_relocation.py --resolve's; 36 of 36 guards hold the file's digest and the three readers \
             verify; with the audited digests {held} of {of} hold and the readers (bags, wallet, mf) verify {refused:?}"
        );
    }
}
