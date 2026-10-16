//! The resolver of the relocation table ([`crate::relocation`]): where every anchor and vtable
//! of the table is in the game's executable, or the first rule of the relocation study
//! (`docs/audit/2026-10-16-relocation-study/README.md`, section 4) that does not hold.
//!
//! It is the `--resolve` of `scripts/relocation/gen_relocation.py`, ported: the same rules, in
//! the same order, with the same texts, so that a resolution in the addon and one made offline
//! over a copy of the same file say the same thing. It reads only the table and the file, as
//! the Python one does: no disassembler and no byte of the game in the table.
//!
//! It runs in two phases, and does no I/O of its own:
//!
//! - **The scan** ([`Scanner`]), inside the pass that hashes the file
//!   ([`crate::executable::Hashing::advance`]): every byte of `.text` and `.rdata` goes through a
//!   rolling hash, and the positions where it gives the hash of an anchor's window are kept as
//!   candidates of that anchor. A window that goes by in two reads is put together. The
//!   `.pdata` and `.reloc` sections are copied as they go by, and the header is read from the
//!   first bytes of the file: positions and sizes come from the file, not from the table.
//! - **The resolution** ([`Resolution`]), once the file has ended: reads at points of the file
//!   (`read_at`), a page of 4 KiB at a time and each page once, and the graph of the table. A
//!   step stops before a read when the clock it is given says so, and the next one goes on: it
//!   runs the resolution again from the start over the pages already read, which is cheap, and
//!   stops at the first page it still needs. It is deterministic, so a resolution in many steps
//!   is the one in a single step.
//!
//! The result is [`Resolved`]: the RVA of each anchor and vtable by its name in the table, the
//! TLS index, the image size, the bounds of `.rdata`, and whether the file held the certificate
//! at its fixed offsets (`identity`) or every place was found again (`relocated`). Or a
//! [`Failure`]: the anchor, the rule, and why, in the words of the Python resolver. A build whose
//! file does not hold the certificate at its fixed offsets is read with what it gives
//! ([`crate::executable::relocated`]): the layout of its places, and the SHA-256 of the file at
//! each anchor ([`Place::file_sha256`]) for the guards of the readers.

use std::collections::HashMap;
use std::fmt;

use crate::executable::Step;
use crate::relocation::{
    self, Anchor, AnchorKind, Bound, Callee, Content, FieldKind, Section, Target, Vtable, ROLLING_BASE,
    ROLLING_PRIME,
};
use crate::sha256::{hex, sha256};

/// A relocation table: what [`relocation`] holds for the game, or a test's own.
#[derive(Debug)]
pub struct Table {
    pub image_base: u64,
    /// The names of the sections of the certified image, in order.
    pub section_names: &'static [&'static str],
    pub anchors: &'static [Anchor],
    pub vtables: &'static [Vtable],
    pub callees: &'static [Callee],
    pub contents: &'static [Content],
}

/// The game's table.
pub static GAME: Table = Table {
    image_base: relocation::IMAGE_BASE,
    section_names: &relocation::SECTION_NAMES,
    anchors: &relocation::ANCHORS,
    vtables: &relocation::VTABLES,
    callees: &relocation::CALLEES,
    contents: &relocation::CONTENTS,
};

impl Table {
    /// Every name [`Resolved::resolved_rva`] answers for, in the table's order: the anchors, then
    /// each vtable as `vtable <name>`, as a [`Failure`] names it.
    pub fn names(&self) -> Vec<String> {
        self.anchors
            .iter()
            .map(|a| a.name.to_string())
            .chain(self.vtables.iter().map(|v| format!("vtable {}", v.name)))
            .collect()
    }
}

/// How much of the start of the file holds its header, the section table included.
const HEADER_BYTES: usize = crate::certificate::HEADER_BYTES;

/// One section header, as the file has it.
#[derive(Debug, Clone)]
struct Sec {
    name: String,
    va: u32,
    vs: u32,
    ro: u32,
    rs: u32,
}

/// The fields of the PE header the resolver uses.
#[derive(Debug, Clone)]
struct Header {
    machine: u16,
    magic: u16,
    image_base: u64,
    image_size: u32,
    /// Data directories 5 (base relocations) and 9 (TLS), as (RVA, size).
    reloc_dir: (u32, u32),
    tls_dir: (u32, u32),
    sections: Vec<Sec>,
}

impl Header {
    /// The header in `h`, the first bytes of the file: `None` while more of them are needed,
    /// `Err` for a file that is not a PE image. Read as the Python `PE` reads it.
    fn parse(h: &[u8]) -> Option<Result<Header, &'static str>> {
        let u16_at = |at: usize| h.get(at..at + 2).map(|b| u16::from_le_bytes([b[0], b[1]]));
        let u32_at = |at: usize| h.get(at..at + 4).map(|b| u32::from_le_bytes([b[0], b[1], b[2], b[3]]));
        let u64_at = |at: usize| h.get(at..at + 8).map(|b| u64::from_le_bytes(b.try_into().unwrap()));
        let full = h.len() >= HEADER_BYTES;
        let need = |v: Option<Result<Header, &'static str>>| if full { Some(Err("truncated header")) } else { v };
        if h.len() < 2 {
            return need(None);
        }
        if &h[..2] != b"MZ" {
            return Some(Err("not MZ"));
        }
        let Some(pe) = u32_at(0x3c) else { return need(None) };
        let pe = pe as usize;
        let Some(signature) = h.get(pe..pe + 4) else { return need(None) };
        if signature != b"PE\0\0" {
            return Some(Err("not PE"));
        }
        let (Some(machine), Some(count), Some(optional), Some(magic)) =
            (u16_at(pe + 4), u16_at(pe + 6), u16_at(pe + 20), u16_at(pe + 24))
        else {
            return need(None);
        };
        let table = pe + 24 + optional as usize;
        let end = (table + 40 * count as usize).max(pe + 24 + 112 + 8 * 16);
        if h.len() < end {
            return need(None);
        }
        let dir = |i: usize| (u32_at(pe + 24 + 112 + 8 * i).unwrap(), u32_at(pe + 24 + 116 + 8 * i).unwrap());
        let sections = (0..count as usize)
            .map(|i| {
                let at = table + 40 * i;
                let raw = &h[at..at + 8];
                let len = raw.iter().rposition(|&b| b != 0).map_or(0, |p| p + 1);
                Sec {
                    name: raw[..len].iter().map(|&b| b as char).collect(),
                    vs: u32_at(at + 8).unwrap(),
                    va: u32_at(at + 12).unwrap(),
                    rs: u32_at(at + 16).unwrap(),
                    ro: u32_at(at + 20).unwrap(),
                }
            })
            .collect();
        Some(Ok(Header {
            machine,
            magic,
            image_base: u64_at(pe + 24 + 24).unwrap(),
            image_size: u32_at(pe + 24 + 56).unwrap(),
            reloc_dir: dir(5),
            tls_dir: dir(9),
            sections,
        }))
    }

    /// The last section of that name, as the Python `pe.sec[name]`.
    fn section(&self, name: &str) -> Option<&Sec> {
        self.sections.iter().rev().find(|s| s.name == name)
    }

    fn r2o(&self, r: i64) -> Option<u64> {
        for s in &self.sections {
            let (va, vs) = (i64::from(s.va), i64::from(s.vs));
            if va <= r && r < va + vs {
                return (r - va < i64::from(s.rs)).then(|| (r - va + i64::from(s.ro)) as u64);
            }
        }
        None
    }

    fn secof(&self, r: i64) -> Option<&str> {
        self.sections
            .iter()
            .find(|s| i64::from(s.va) <= r && r < i64::from(s.va) + i64::from(s.vs))
            .map(|s| s.name.as_str())
    }
}

/// Bytes of the file copied as they go by: one range, known once the header is.
#[derive(Debug, Default)]
struct Captured {
    start: u64,
    bytes: Vec<u8>,
    len: usize,
    seen: usize,
}

impl Captured {
    fn of(start: u64, len: usize) -> Self {
        Self { start, bytes: vec![0; len], len, seen: 0 }
    }

    fn feed(&mut self, at: u64, bytes: &[u8]) {
        let end = at + bytes.len() as u64;
        let mine = self.start + self.len as u64;
        if mine <= at || self.start >= end {
            return;
        }
        let from = self.start.max(at);
        let to = mine.min(end);
        let source = &bytes[(from - at) as usize..(to - at) as usize];
        let target = (from - self.start) as usize;
        self.bytes[target..target + source.len()].copy_from_slice(source);
        self.seen += source.len();
    }

    fn whole(&self) -> bool {
        self.seen == self.len
    }
}

/// The searched windows of one section, by their hash.
#[derive(Debug, Default)]
struct Wanted {
    /// One bit per value of the low 16 bits of a wanted hash: most positions are turned away
    /// by it.
    filter: Vec<u64>,
    /// (hash, anchor, where the window starts in the anchor).
    entries: Vec<(u32, usize, u32)>,
}

/// The rolling hash over one section of the file, from one read to the next.
#[derive(Debug)]
struct Run {
    /// 0 for `.text`, 1 for `.rdata`.
    group: usize,
    start: u64,
    end: u64,
    va: u32,
    h: u64,
    ring: Vec<u8>,
    idx: usize,
    filled: usize,
    seen: u64,
}

/// The first phase of the resolution: the scan of `.text` and `.rdata` for the windows of the
/// searched anchors, and the copy of the header, `.pdata` and `.reloc`, made as the file is
/// read in order ([`Scanner::feed`]).
///
/// A searched anchor is proposed at every place where the rolling hash of `k` bytes gives its
/// [`relocation::Search::hash`]: `sum(w[j] * ROLLING_BASE^(k-1-j)) mod ROLLING_PRIME`, the
/// formula of [`relocation::rolling_hash`] and of the generator. Only the normalized digest,
/// in the second phase, says whether it is there. All the searched anchors of the game's
/// table share one `k`; one with another `k` would never be proposed, and so would fail
/// closed.
#[derive(Debug)]
pub struct Scanner {
    table: &'static Table,
    header: Vec<u8>,
    layout: Option<Result<Header, &'static str>>,
    /// Up to where the file has been fed.
    next: u64,
    /// The file was fed out of order: the scan says nothing.
    broken: bool,
    k: usize,
    /// `b * ROLLING_BASE^(k-1) mod ROLLING_PRIME` for every byte `b`: the term that leaves.
    out_term: Vec<u64>,
    wanted: [Wanted; 2],
    runs: Vec<Run>,
    pdata: Option<Captured>,
    reloc: Option<Captured>,
    /// The candidate RVAs of each anchor, in the order of the file.
    candidates: Vec<Vec<i64>>,
    hash_hits: u64,
    scanned: u64,
}

impl Scanner {
    pub fn new(table: &'static Table) -> Self {
        let k = table.anchors.iter().find_map(|a| a.search).map_or(16, |s| usize::from(s.k));
        let (p, b) = (u64::from(ROLLING_PRIME), u64::from(ROLLING_BASE));
        let top = (1..k).fold(1u64, |t, _| t * b % p);
        let out_term = (0..256u64).map(|x| x * top % p).collect();
        let mut wanted = [Wanted { filter: vec![0; 1024], entries: Vec::new() }, Wanted { filter: vec![0; 1024], entries: Vec::new() }];
        for (i, a) in table.anchors.iter().enumerate() {
            let Some(s) = a.search else { continue };
            if usize::from(s.k) != k {
                continue;
            }
            let w = &mut wanted[group_of(a.section)];
            w.filter[(s.hash as usize & 0xffff) >> 6] |= 1 << (s.hash & 63);
            w.entries.push((s.hash, i, s.at));
        }
        Self {
            table,
            header: Vec::with_capacity(HEADER_BYTES),
            layout: None,
            next: 0,
            broken: false,
            k,
            out_term,
            wanted,
            runs: Vec::new(),
            pdata: None,
            reloc: None,
            candidates: vec![Vec::new(); table.anchors.len()],
            hash_hits: 0,
            scanned: 0,
        }
    }

    /// Takes `bytes`, those of the file from `at` on. The file must be fed in order, as the
    /// hash reads it; a window, a section or the header that goes by in several reads is put
    /// together.
    pub fn feed(&mut self, at: u64, bytes: &[u8]) {
        if self.broken || bytes.is_empty() {
            return;
        }
        if at != self.next {
            self.broken = true;
            return;
        }
        self.next = at + bytes.len() as u64;
        if self.header.len() < HEADER_BYTES {
            let take = (HEADER_BYTES - self.header.len()).min(bytes.len());
            self.header.extend_from_slice(&bytes[..take]);
        }
        if self.layout.is_none() {
            self.layout = Header::parse(&self.header);
            match &self.layout {
                None => return,
                Some(Err(_)) => return,
                Some(Ok(header)) => {
                    let header = header.clone();
                    self.lay_out(&header);
                    // What came before the header was known is in the header buffer.
                    let seen = self.header.len() as u64;
                    let before = at.min(seen);
                    let replay = self.header[..before as usize].to_vec();
                    self.process(0, &replay);
                    self.process(at, bytes);
                    return;
                }
            }
        }
        if matches!(self.layout, Some(Ok(_))) {
            self.process(at, bytes);
        }
    }

    /// The ranges of the file the header names: the sections that are scanned, `.pdata` and the
    /// base relocations.
    fn lay_out(&mut self, h: &Header) {
        for (group, name) in [(0, ".text"), (1, ".rdata")] {
            if self.wanted[group].entries.is_empty() {
                continue;
            }
            if let Some(s) = h.section(name) {
                let start = u64::from(s.ro);
                self.runs.push(Run {
                    group,
                    start,
                    end: start + u64::from(s.vs.min(s.rs)),
                    va: s.va,
                    h: 0,
                    ring: vec![0; self.k],
                    idx: 0,
                    filled: 0,
                    seen: 0,
                });
            }
        }
        if let Some(s) = h.section(".pdata") {
            self.pdata = Some(Captured::of(u64::from(s.ro), (s.vs / 12 * 12) as usize));
        }
        if let Some(o) = h.r2o(i64::from(h.reloc_dir.0)) {
            self.reloc = Some(Captured::of(o, h.reloc_dir.1 as usize));
        }
    }

    fn process(&mut self, at: u64, bytes: &[u8]) {
        let end = at + bytes.len() as u64;
        if let Some(c) = &mut self.pdata {
            c.feed(at, bytes);
        }
        if let Some(c) = &mut self.reloc {
            c.feed(at, bytes);
        }
        for run in &mut self.runs {
            if run.end <= at || run.start >= end {
                continue;
            }
            let from = run.start.max(at);
            let to = run.end.min(end);
            let slice = &bytes[(from - at) as usize..(to - at) as usize];
            self.scanned += slice.len() as u64;
            roll(run, &self.wanted[run.group], slice, self.k, &self.out_term, &mut self.candidates, &mut self.hash_hits);
        }
    }

    /// How many bytes of `.text` and `.rdata` went through the rolling hash.
    pub fn scanned(&self) -> u64 {
        self.scanned
    }

    /// How many positions gave a wanted hash, before any digest.
    pub fn hash_hits(&self) -> u64 {
        self.hash_hits
    }
}

fn group_of(section: Section) -> usize {
    match section {
        Section::Text => 0,
        Section::Rdata => 1,
    }
}

/// The rolling hash of one run over `bytes`, the next ones of its section.
fn roll(
    run: &mut Run,
    wanted: &Wanted,
    bytes: &[u8],
    k: usize,
    out_term: &[u64],
    candidates: &mut [Vec<i64>],
    hits: &mut u64,
) {
    let (p, b) = (u64::from(ROLLING_PRIME), u64::from(ROLLING_BASE));
    let mut h = run.h;
    for &x in bytes {
        let pos = run.seen;
        run.seen += 1;
        if run.filled < k {
            h = (h * b + u64::from(x)) % p;
            run.ring[run.filled] = x;
            run.filled += 1;
            if run.filled < k {
                continue;
            }
        } else {
            let out = run.ring[run.idx];
            run.ring[run.idx] = x;
            run.idx += 1;
            if run.idx == k {
                run.idx = 0;
            }
            h = ((h + p - out_term[usize::from(out)]) * b + u64::from(x)) % p;
        }
        if wanted.filter[(h as usize & 0xffff) >> 6] >> (h & 63) & 1 == 0 {
            continue;
        }
        let start = pos + 1 - k as u64;
        let mut any = false;
        for &(hash, anchor, at) in &wanted.entries {
            if u64::from(hash) == h {
                any = true;
                candidates[anchor].push(i64::from(run.va) + start as i64 - i64::from(at));
            }
        }
        if any {
            *hits += 1;
        }
    }
    run.h = h;
}

/// Whether the file held the certificate at its fixed offsets ([`Mode::Identity`], rule 1), or
/// every place was found again ([`Mode::Relocated`]).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Mode {
    Identity,
    Relocated,
}

impl Mode {
    pub fn as_str(self) -> &'static str {
        match self {
            Mode::Identity => "identity",
            Mode::Relocated => "relocated",
        }
    }
}

/// Where one anchor or vtable is, and how it was found (the Python resolver's `how`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Place {
    pub name: &'static str,
    /// `None` only for a constructor that was not found and whose vtables were.
    pub rva: Option<u64>,
    pub how: Option<String>,
    /// For an anchor, the SHA-256 of the file's bytes over its length at `rva`, its fields as
    /// they are and not zeroed: what the readers' guards must find in memory where the layout
    /// places them (`passive::GuardDigests`). For a vtable, or an anchor not located, `None`.
    pub file_sha256: Option<[u8; 32]>,
}

/// What the search cost, in a relocated resolution.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SearchStats {
    pub bytes: u64,
    pub hash_hits: u64,
    /// Candidates the normalized digest turned away.
    pub false_candidates: u64,
}

/// A file resolved: every place the readers rely on.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Resolved {
    pub mode: Mode,
    pub image_size: u32,
    /// The RVA the TLS directory's AddressOfIndex names.
    pub tls_index: u64,
    /// `.rdata`, as [start, end) RVAs.
    pub rdata: (u64, u64),
    /// Each anchor of the table, in its order.
    pub anchors: Vec<Place>,
    /// Each vtable of the table, in its order.
    pub vtables: Vec<Place>,
    pub search: Option<SearchStats>,
}

impl Resolved {
    /// The RVA of an anchor by its name in the table, or of a vtable by `vtable <name>`
    /// ([`Table::names`]).
    pub fn resolved_rva(&self, name: &str) -> Option<u64> {
        if let Some(a) = self.anchors.iter().find(|a| a.name == name) {
            return a.rva;
        }
        let v = name.strip_prefix("vtable ")?;
        self.vtables.iter().find(|p| p.name == v).and_then(|p| p.rva)
    }
}

/// The first rule that did not hold: the anchor (or `vtable <name>`, or `header`), the rule of
/// the study, section 4, and why.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Failure {
    pub anchor: String,
    pub rule: u8,
    pub detail: String,
}

/// The rules, as the Python resolver names them.
pub fn rule_name(rule: u8) -> &'static str {
    match rule {
        1 => "identity first",
        2 => "search only locates what was unique when certified",
        3 => "a derived anchor matches its normalized signature where it is derived",
        4 => "every masked field points where it pointed",
        5 => "redundant paths agree",
        6 => "vtable = the constructor lea; every slot holds its located function",
        7 => "field offsets and constants are never masked",
        8 => "header",
        9 => "all or nothing",
        _ => "",
    }
}

impl fmt::Display for Failure {
    /// The line of `gen_relocation.py --resolve`.
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "not resolved: {}: rule {} ({}): {}", self.anchor, self.rule, rule_name(self.rule), self.detail)
    }
}

/// What the scan left for the resolution, read once.
#[derive(Debug)]
struct Prepared {
    table: &'static Table,
    header: Header,
    pdata_begins: Vec<u32>,
    pdata_ends: Vec<u32>,
    relocs: Vec<u32>,
    candidates: Vec<Vec<i64>>,
    scanned: u64,
    hash_hits: u64,
}

impl Prepared {
    /// `Err(None)` when the scan says nothing (the file was not fed in order).
    fn of(s: &Scanner) -> Result<Self, Option<Failure>> {
        if s.broken {
            return Err(None);
        }
        let header_fail = |d: &str| Some(Failure { anchor: "header".into(), rule: 8, detail: d.into() });
        let header = match &s.layout {
            Some(Ok(h)) => h.clone(),
            Some(Err(why)) => return Err(header_fail(why)),
            None => return Err(header_fail("truncated header")),
        };
        let pdata = s.pdata.as_ref().filter(|c| c.whole()).ok_or_else(|| header_fail("no .pdata in the file"))?;
        let reloc = s.reloc.as_ref().filter(|c| c.whole()).ok_or_else(|| header_fail("no base relocations in the file"))?;
        let mut entries: Vec<(u32, u32)> = pdata
            .bytes
            .chunks_exact(12)
            .map(|e| (u32::from_le_bytes(e[0..4].try_into().unwrap()), u32::from_le_bytes(e[4..8].try_into().unwrap())))
            .filter(|&(b, _)| b != 0)
            .collect();
        entries.sort_unstable();
        let mut relocs = Vec::new();
        let r = &reloc.bytes;
        let mut o = 0usize;
        while o + 8 <= r.len() {
            let page = u32::from_le_bytes(r[o..o + 4].try_into().unwrap());
            let size = u32::from_le_bytes(r[o + 4..o + 8].try_into().unwrap()) as usize;
            if size < 8 {
                break;
            }
            for i in 0..(size - 8) / 2 {
                let Some(e) = r.get(o + 8 + 2 * i..o + 10 + 2 * i) else { break };
                let e = u16::from_le_bytes([e[0], e[1]]);
                if e >> 12 == 10 {
                    relocs.push(page.wrapping_add(u32::from(e & 0xfff)));
                }
            }
            o += size;
        }
        relocs.sort_unstable();
        Ok(Self {
            table: s.table,
            header,
            pdata_begins: entries.iter().map(|e| e.0).collect(),
            pdata_ends: entries.iter().map(|e| e.1).collect(),
            relocs,
            candidates: s.candidates.clone(),
            scanned: s.scanned,
            hash_hits: s.hash_hits,
        })
    }
}

/// How a run of the resolution stopped short of a result.
enum Stop {
    /// The clock ran out before a read: the next step goes on.
    Pause,
    /// A read failed: the system's doing, worth another try.
    Failed,
    /// A rule did not hold.
    Fail(Failure),
}

type R<T> = Result<T, Stop>;

fn fail<T>(anchor: impl Into<String>, rule: u8, detail: String) -> R<T> {
    Err(Stop::Fail(Failure { anchor: anchor.into(), rule, detail }))
}

/// `%#x` of Python, negative numbers included.
fn hx(v: i64) -> String {
    if v < 0 {
        format!("-{:#x}", v.unsigned_abs())
    } else {
        format!("{v:#x}")
    }
}

/// `%s` of an optional number in Python: the number, or `None`.
fn or_none(v: Option<i64>, as_hex: bool) -> String {
    match v {
        None => "None".into(),
        Some(v) if as_hex => hx(v),
        Some(v) => v.to_string(),
    }
}

const PAGE: u64 = 4096;

/// The second phase of the resolution, from one step to the next: what the scan left, and the
/// pages of the file read so far.
pub struct Resolution {
    prepared: Result<Prepared, Option<Failure>>,
    pages: HashMap<u64, Vec<u8>>,
    reads: u64,
}

impl Resolution {
    /// Starts on what `scanner` collected; it must have been fed the whole file.
    pub fn new(scanner: &Scanner) -> Self {
        Self { prepared: Prepared::of(scanner), pages: HashMap::new(), reads: 0 }
    }

    /// One step: as much of the resolution as the clock allows, reading the file through
    /// `read_at` (bytes at a file offset, `Some(0)` past its end, `None` when the system fails).
    /// `expired` is asked before every read of the file, as [`crate::executable::Hashing::advance`]
    /// asks before each of its reads. With `identity`, the file held the certificate at its
    /// fixed offsets (rule 1).
    pub fn step(
        &mut self,
        identity: bool,
        read_at: &mut dyn FnMut(u64, &mut [u8]) -> Option<usize>,
        expired: &mut dyn FnMut() -> bool,
    ) -> Step<Result<Resolved, Failure>> {
        let prepared = match &self.prepared {
            Ok(p) => p,
            Err(Some(f)) => return Step::Decided(Err(f.clone())),
            Err(None) => return Step::Failed,
        };
        let mut pe = Pe { p: prepared, pages: &mut self.pages, read_at, expired, reads: &mut self.reads, tls: None };
        match run(&mut pe, identity) {
            Ok(r) => Step::Decided(Ok(r)),
            Err(Stop::Fail(f)) => Step::Decided(Err(f)),
            Err(Stop::Pause) => Step::Unfinished,
            Err(Stop::Failed) => Step::Failed,
        }
    }

    /// How many reads of the file the resolution has made, and how many bytes they brought.
    pub fn reads(&self) -> (u64, u64) {
        (self.reads, self.pages.values().map(|p| p.len() as u64).sum())
    }
}

/// The file as the resolution sees it: the header, `.pdata` and the relocations from the scan,
/// and pages read at points.
struct Pe<'a> {
    p: &'a Prepared,
    pages: &'a mut HashMap<u64, Vec<u8>>,
    read_at: &'a mut dyn FnMut(u64, &mut [u8]) -> Option<usize>,
    expired: &'a mut dyn FnMut() -> bool,
    reads: &'a mut u64,
    tls: Option<Option<i64>>,
}

impl Pe<'_> {
    /// `n` bytes at a file offset, or `None` where the file ends before them.
    fn file(&mut self, offset: u64, n: usize) -> R<Option<Vec<u8>>> {
        let mut out = Vec::with_capacity(n);
        let end = offset + n as u64;
        let mut at = offset;
        while at < end {
            let page = at / PAGE;
            if !self.pages.contains_key(&page) {
                if (self.expired)() {
                    return Err(Stop::Pause);
                }
                let mut buf = vec![0u8; PAGE as usize];
                let mut got = 0usize;
                while got < buf.len() {
                    match (self.read_at)(page * PAGE + got as u64, &mut buf[got..]) {
                        None => return Err(Stop::Failed),
                        Some(0) => break,
                        Some(r) => got += r.min(buf.len() - got),
                    }
                }
                *self.reads += 1;
                buf.truncate(got);
                self.pages.insert(page, buf);
            }
            let bytes = &self.pages[&page];
            let from = (at - page * PAGE) as usize;
            if from >= bytes.len() {
                return Ok(None);
            }
            let take = (bytes.len() - from).min((end - at) as usize);
            out.extend_from_slice(&bytes[from..from + take]);
            at += take as u64;
        }
        Ok(Some(out))
    }

    fn h(&self) -> &Header {
        &self.p.header
    }

    fn secof(&self, r: i64) -> Option<&str> {
        self.p.header.secof(r)
    }

    /// `n` bytes at RVA `r`, or `None` if any of them is outside the file's sections.
    fn read(&mut self, r: i64, n: usize) -> R<Option<Vec<u8>>> {
        let p = self.p;
        let h = &p.header;
        let last = r + n as i64 - 1;
        let (Some(o), Some(_)) = (h.r2o(r), h.r2o(last)) else { return Ok(None) };
        if h.secof(r) != h.secof(last) {
            return Ok(None);
        }
        self.file(o, n)
    }

    fn u64(&mut self, r: i64) -> R<Option<u64>> {
        Ok(self.read(r, 8)?.map(|b| u64::from_le_bytes(b[..8].try_into().unwrap())))
    }

    fn tls_index_rva(&mut self) -> R<Option<i64>> {
        if let Some(t) = self.tls {
            return Ok(t);
        }
        let va = i64::from(self.h().tls_dir.0);
        let base = self.h().image_base;
        let t = self.u64(va + 16)?.map(|q| sub(q, base));
        self.tls = Some(t);
        Ok(t)
    }

    /// The `.pdata` function that starts at `r`.
    fn func_at(&self, r: i64) -> Option<(i64, i64)> {
        let bs = &self.p.pdata_begins;
        let i = bs.partition_point(|&b| i64::from(b) < r);
        (i < bs.len() && i64::from(bs[i]) == r).then(|| (i64::from(bs[i]), i64::from(self.p.pdata_ends[i])))
    }

    /// The `.pdata` function that holds `r`.
    fn func_of(&self, r: i64) -> Option<(i64, i64)> {
        let bs = &self.p.pdata_begins;
        let i = bs.partition_point(|&b| i64::from(b) <= r);
        if i == 0 {
            return None;
        }
        let (b, e) = (i64::from(bs[i - 1]), i64::from(self.p.pdata_ends[i - 1]));
        (b <= r && r < e).then_some((b, e))
    }

    /// Starts of the relocations whose 8 bytes overlap [r0, r1).
    fn relocs_in(&self, r0: i64, r1: i64) -> Vec<i64> {
        let rl = &self.p.relocs;
        let i = rl.partition_point(|&r| i64::from(r) < r0 - 7);
        rl[i..].iter().map(|&r| i64::from(r)).take_while(|&r| r < r1).collect()
    }
}

/// `q - base` as Python would compute it, saturated where an RVA cannot be.
fn sub(q: u64, base: u64) -> i64 {
    i64::try_from(i128::from(q) - i128::from(base)).unwrap_or(i64::MAX)
}

fn section_name(s: Section) -> &'static str {
    match s {
        Section::Text => ".text",
        Section::Rdata => ".rdata",
    }
}

fn kind_name(k: FieldKind) -> &'static str {
    match k {
        FieldKind::Rel32 => "rel32",
        FieldKind::Rip => "rip",
        FieldKind::ImageRel => "imagerel",
        FieldKind::JumpTable => "jumptable",
    }
}

fn bound_name(b: Bound) -> &'static str {
    match b {
        Bound::Function => "function",
        Bound::Inside => "inside",
        Bound::Leaf => "leaf",
    }
}

fn normalized(raw: &[u8], mask: impl Iterator<Item = u32>) -> Vec<u8> {
    let mut b = raw.to_vec();
    for p in mask {
        let p = p as usize;
        let end = (p + 4).min(b.len());
        if p < end {
            b[p..end].fill(0);
        }
    }
    b
}

fn sha_hex(b: &[u8]) -> String {
    hex(&sha256(b))
}

/// Where a field of an anchor placed at `base` points, as an RVA.
fn field_target(pe: &mut Pe, base: i64, at: u32, kind: FieldKind, tail: u8) -> R<Option<i64>> {
    let p = base + i64::from(at);
    let Some(v) = pe.read(p, 4)? else { return Ok(None) };
    let v = [v[0], v[1], v[2], v[3]];
    Ok(Some(match kind {
        FieldKind::Rel32 | FieldKind::Rip => {
            (p + 4 + i64::from(tail) + i64::from(i32::from_le_bytes(v))) & 0xffff_ffff
        }
        FieldKind::ImageRel | FieldKind::JumpTable => i64::from(u32::from_le_bytes(v)),
    }))
}

/// Whether anchor `a` matches its normalized signature at `at`, in its section and alignment.
fn digest_at(pe: &mut Pe, a: &Anchor, at: i64) -> R<bool> {
    let Some(raw) = pe.read(at, a.len as usize)? else { return Ok(false) };
    if pe.secof(at) != Some(section_name(a.section)) || at.rem_euclid(i64::from(a.align.max(1))) != 0 {
        return Ok(false);
    }
    Ok(sha_hex(&normalized(&raw, a.fields.iter().map(|f| f.at))) == a.normalized_sha256)
}

/// Whether the function at `t` has the callee's fingerprint: bound, length, normalized digest.
fn callee_ok(pe: &mut Pe, c: &Callee, t: i64) -> R<bool> {
    let r0 = match c.bound {
        Bound::Function => match pe.func_at(t) {
            Some((b, e)) if e - b == i64::from(c.len) => t,
            _ => return Ok(false),
        },
        Bound::Inside => match pe.func_of(t) {
            Some((b, e)) if t - b == i64::from(c.offset) && e - b == i64::from(c.len) => b,
            _ => return Ok(false),
        },
        Bound::Leaf => {
            if pe.func_of(t).is_some() {
                return Ok(false);
            }
            t
        }
    };
    let Some(raw) = pe.read(r0, c.len as usize)? else { return Ok(false) };
    Ok(pe.secof(r0) == Some(".text") && sha_hex(&normalized(&raw, c.mask.iter().copied())) == c.normalized_sha256)
}

/// Whether the `.rdata` bytes at `t` are the content's: its relocations, and its digest with
/// them zeroed.
fn content_ok(pe: &mut Pe, c: &Content, t: i64) -> R<bool> {
    let Some(mut raw) = pe.read(t, c.len as usize)? else { return Ok(false) };
    if pe.secof(t) != Some(".rdata") {
        return Ok(false);
    }
    let here: Vec<i64> = pe.relocs_in(t, t + i64::from(c.len)).into_iter().map(|r| r - t).collect();
    if here.len() != c.pointers.len() || here.iter().zip(c.pointers).any(|(&a, &b)| a != i64::from(b)) {
        return Ok(false);
    }
    for &p in c.pointers {
        let from = p.max(0) as usize;
        let to = (i64::from(p) + 8).clamp(0, raw.len() as i64) as usize;
        if from < to {
            raw[from..to].fill(0);
        }
    }
    Ok(sha_hex(&raw) == c.sha256)
}

/// The places found so far, by index of the table.
struct Located {
    anchors: Vec<Option<i64>>,
    vtables: Vec<Option<i64>>,
}

/// Whether vtable `j` placed at `at` holds each checked slot: the located function, or a place
/// where the function's signature matches. With `only_located`, the slots of functions not
/// located yet are left to their own derivation.
fn vtable_slots_ok(pe: &mut Pe, j: usize, at: i64, loc: &Located, only_located: bool) -> R<Result<(), String>> {
    let t = pe.p.table;
    for s in t.vtables[j].slots {
        let Some(q) = pe.u64(at + i64::from(s.offset))? else {
            return Ok(Err(format!("slot +{} unreadable", hx(s.offset.into()))));
        };
        let f = sub(q, t.image_base);
        let fa = &t.anchors[s.anchor];
        if let Some(l) = loc.anchors[s.anchor] {
            if f != l {
                return Ok(Err(format!("slot +{} holds {}, {} is at {}", hx(s.offset.into()), hx(f), fa.name, hx(l))));
            }
        } else if only_located {
            continue;
        } else if !digest_at(pe, fa, f)? {
            return Ok(Err(format!("slot +{} holds {}, where {} does not match", hx(s.offset.into()), hx(f), fa.name)));
        }
    }
    Ok(Ok(()))
}

/// Rule 4 for anchor `i` placed at `base`: every masked field points at its target's new
/// place; targets not located yet are checked by their own signature there.
fn check_fields(pe: &mut Pe, i: usize, base: i64, loc: &Located) -> R<Result<(), String>> {
    let t = pe.p.table;
    let a = &t.anchors[i];
    let mut func = None;
    if let Some(w) = a.function {
        func = pe.func_of(base);
        match func {
            Some((b, e)) if base - b == i64::from(w.offset) && e - b == i64::from(w.len) => {}
            _ => {
                return Ok(Err(format!(
                    "rule 4: its function is not the certified one (offset {}, length {})",
                    or_none(func.map(|(b, _)| base - b), false),
                    or_none(func.map(|(b, e)| e - b), false)
                )))
            }
        }
    }
    for x in a.fields {
        let where_ = format!("field +{} ({})", x.at, kind_name(x.kind));
        let Some(tg) = field_target(pe, base, x.at, x.kind, x.tail)? else {
            return Ok(Err(format!("{where_} unreadable")));
        };
        match x.target {
            Target::ImageBase => {
                if tg != 0 {
                    return Ok(Err(format!("{where_} points at {}, not the image base", hx(tg))));
                }
            }
            Target::TlsIndex => {
                let tls = pe.tls_index_rva()?;
                if Some(tg) != tls {
                    return Ok(Err(format!("{where_} points at {}, the TLS directory names {}", hx(tg), hx(tls.unwrap_or(0)))));
                }
            }
            Target::Internal(off) => {
                if tg - base != i64::from(off) {
                    return Ok(Err(format!("{where_} points {} bytes into it, certified {}", tg - base, off)));
                }
            }
            Target::SameFunction(off) => {
                if func.map(|(b, _)| tg - b) != Some(i64::from(off)) {
                    return Ok(Err(format!(
                        "{where_} points {} bytes into its function, certified {}",
                        or_none(func.map(|(b, _)| tg - b), false),
                        off
                    )));
                }
            }
            Target::Anchor { anchor: j, add } => {
                let b = &t.anchors[j];
                let at = tg + i64::from(add);
                if let Some(l) = loc.anchors[j] {
                    if at != l {
                        return Ok(Err(format!("{where_} names {} at {}, it is at {}", b.name, hx(at), hx(l))));
                    }
                } else if !digest_at(pe, b, at)? {
                    return Ok(Err(format!("{where_} names {} at {}, where its signature does not match", b.name, hx(at))));
                }
            }
            Target::Vtable(j) => {
                let name = t.vtables[j].name;
                if let Some(l) = loc.vtables[j] {
                    if tg != l {
                        return Ok(Err(format!("{where_} names vtable {name} at {}, it is at {}", hx(tg), hx(l))));
                    }
                } else if let Err(why) = vtable_slots_ok(pe, j, tg, loc, false)? {
                    return Ok(Err(format!("{where_} names vtable {name} at {}: {why}", hx(tg))));
                }
            }
            Target::Callee(j) => {
                let c = &t.callees[j];
                if !callee_ok(pe, c, tg)? {
                    return Ok(Err(format!(
                        "{where_} calls {}, which is not the certified callee ({}, {} bytes)",
                        hx(tg),
                        bound_name(c.bound),
                        c.len
                    )));
                }
            }
            Target::Content(j) => {
                let c = &t.contents[j];
                if !content_ok(pe, c, tg)? {
                    return Ok(Err(format!("{where_} reads {}, whose {} bytes are not the certified ones", hx(tg), c.len)));
                }
            }
            Target::Code => {
                if pe.secof(tg) != Some(".text") {
                    return Ok(Err(format!("{where_} points at {}, outside .text", hx(tg))));
                }
            }
            Target::Data => {
                if pe.secof(tg) != Some(".rdata") {
                    return Ok(Err(format!("{where_} points at {}, outside .rdata", hx(tg))));
                }
            }
        }
    }
    Ok(Ok(()))
}

/// Rule 8: what the header must still be.
fn header_check(pe: &mut Pe) -> R<()> {
    let t = pe.p.table;
    let h = pe.h();
    if h.machine != 0x8664 || h.magic != 0x20b {
        return fail("header", 8, "not AMD64 PE32+".into());
    }
    if h.image_base != t.image_base {
        return fail("header", 8, format!("image base {:#x}", h.image_base));
    }
    let names: Vec<&str> = h.sections.iter().map(|s| s.name.as_str()).collect();
    if names != t.section_names {
        let list = names.iter().map(|n| format!("'{n}'")).collect::<Vec<_>>().join(", ");
        return fail("header", 8, format!("sections [{list}]"));
    }
    if pe.tls_index_rva()?.is_none() {
        return fail("header", 8, "no TLS directory".into());
    }
    Ok(())
}

/// The whole resolution over what has been read: the Python `resolve`, rule by rule.
fn run(pe: &mut Pe, identity: bool) -> R<Resolved> {
    header_check(pe)?;
    let t = pe.p.table;
    let n = t.anchors.len();
    if identity {
        // Rule 1. The certificate holds at fixed offsets: every anchor is where it was
        // certified. The constructor and TLS anchors are not in the certificate: they must
        // still match there.
        let loc = Located {
            anchors: t.anchors.iter().map(|a| Some(i64::from(a.rva))).collect(),
            vtables: t.vtables.iter().map(|v| Some(i64::from(v.rva))).collect(),
        };
        for a in t.anchors {
            if !digest_at(pe, a, i64::from(a.rva))? {
                return fail(a.name, 1, "the certificate holds but this anchor does not match at its RVA".into());
            }
        }
        let how = vec![Some("identity".to_string()); n];
        return finish(pe, loc, how, vec![None; t.vtables.len()], Mode::Identity, None);
    }
    // The candidates of the scan, confirmed by their normalized digest.
    let mut found: Vec<Vec<i64>> = vec![Vec::new(); n];
    let mut false_candidates = 0;
    let p = pe.p;
    for (i, a) in t.anchors.iter().enumerate() {
        if a.search.is_none() {
            continue;
        }
        for &at in &p.candidates[i] {
            if digest_at(pe, a, at)? {
                found[i].push(at);
            } else {
                false_candidates += 1;
            }
        }
    }
    let stats = SearchStats { bytes: p.scanned, hash_hits: p.hash_hits, false_candidates };
    let mut loc = Located { anchors: vec![None; n], vtables: vec![None; t.vtables.len()] };
    let mut how: Vec<Option<String>> = vec![None; n];
    let mut vhow: Vec<Option<String>> = vec![None; t.vtables.len()];
    // Rule 2: by search, only what had one match when certified and has one now.
    for (i, a) in t.anchors.iter().enumerate() {
        if a.search.is_some() && search_accepts(a, &found[i]) {
            loc.anchors[i] = Some(found[i][0]);
            how[i] = Some("search (1 match)".into());
        }
    }
    let mut changed = true;
    while changed {
        changed = false;
        // Rule 3: derivation from the fields of located anchors.
        let snapshot: Vec<usize> = (0..n).filter(|&i| loc.anchors[i].is_some()).collect();
        for i in snapshot {
            let a = &t.anchors[i];
            let base = loc.anchors[i].unwrap();
            for x in a.fields {
                match x.target {
                    Target::Anchor { anchor: j, add } => {
                        if loc.anchors[j].is_some() {
                            continue;
                        }
                        let tg = field_target(pe, base, x.at, x.kind, x.tail)?;
                        let Some(tg) = tg else {
                            return fail(t.anchors[j].name, 3, format!("derived by field +{} of {}, which is unreadable", x.at, a.name));
                        };
                        let at = tg + i64::from(add);
                        if !digest_at(pe, &t.anchors[j], at)? {
                            return fail(
                                t.anchors[j].name,
                                3,
                                format!("derived at {} by field +{} of {}, its signature does not match there", hx(at), x.at, a.name),
                            );
                        }
                        loc.anchors[j] = Some(at);
                        how[j] = Some(format!("field +{} of {}", x.at, a.name));
                        changed = true;
                    }
                    Target::Vtable(j) => {
                        if loc.vtables[j].is_some() {
                            continue;
                        }
                        let vname = format!("vtable {}", t.vtables[j].name);
                        let Some(tg) = field_target(pe, base, x.at, x.kind, x.tail)? else {
                            return fail(vname, 6, format!("lea in {} is unreadable", a.name));
                        };
                        if let Err(why) = vtable_slots_ok(pe, j, tg, &loc, true)? {
                            return fail(vname, 6, format!("lea in {} names {}: {why}", a.name, hx(tg)));
                        }
                        loc.vtables[j] = Some(tg);
                        vhow[j] = Some(format!("lea in {}", a.name));
                        changed = true;
                    }
                    _ => {}
                }
            }
        }
        // Rules 6 and 3: functions named by the slots of located vtables.
        let snapshot: Vec<usize> = (0..t.vtables.len()).filter(|&j| loc.vtables[j].is_some()).collect();
        for j in snapshot {
            let v = &t.vtables[j];
            let vat = loc.vtables[j].unwrap();
            for s in v.slots {
                if loc.anchors[s.anchor].is_some() {
                    continue;
                }
                let f = pe.u64(vat + i64::from(s.offset))?.map(|q| sub(q, t.image_base));
                let b = &t.anchors[s.anchor];
                let ok = match f {
                    Some(f) => digest_at(pe, b, f)?,
                    None => false,
                };
                if !ok {
                    return fail(
                        b.name,
                        3,
                        format!(
                            "slot +{} of vtable {} holds {}, where its signature does not match",
                            hx(s.offset.into()),
                            v.name,
                            or_none(f, true)
                        ),
                    );
                }
                loc.anchors[s.anchor] = f;
                how[s.anchor] = Some(format!("slot +{} of vtable {}", hx(s.offset.into()), v.name));
                changed = true;
            }
        }
        if changed {
            continue; // the graph first: twins only once nothing more derives
        }
        // Twins: an anchor that matched several places when certified, of which the field
        // checks picked one, is taken from the search when they pick one now.
        for (i, a) in t.anchors.iter().enumerate() {
            if loc.anchors[i].is_some() || a.search.is_none() || a.certified_survivors != Some(1) {
                continue;
            }
            let mut ok = Vec::new();
            for &h in &found[i] {
                if check_fields(pe, i, h, &loc)?.is_ok() {
                    ok.push(h);
                }
            }
            if ok.len() == 1 {
                loc.anchors[i] = Some(ok[0]);
                how[i] = Some(format!("search ({} matches), the field checks pick one", found[i].len()));
                changed = true;
            } else if ok.is_empty() {
                return fail(a.name, 4, format!("{} matches, none passes the field checks", found[i].len()));
            }
        }
    }
    // Rule 9: all or nothing, over what the readers rely on. A constructor is only a way to its
    // vtable: one that is not found is no failure, as long as its vtable is located.
    for (i, a) in t.anchors.iter().enumerate() {
        if loc.anchors[i].is_some() {
            continue;
        }
        if a.kind == AnchorKind::Constructor {
            how[i] = Some(format!("not found ({} matches); optional", found[i].len()));
            continue;
        }
        let count = a.search.map(|_| found[i].len());
        if let Some(c) = count {
            if a.certified_matches == Some(1) && c > 1 {
                let mut passing = 0;
                for &h in &found[i] {
                    if check_fields(pe, i, h, &loc)?.is_ok() {
                        passing += 1;
                    }
                }
                return fail(
                    a.name,
                    2,
                    format!("certified with 1 match, {c} now ({passing} pass every field check): ambiguous, and nothing derives it"),
                );
            }
        }
        return fail(
            a.name,
            9,
            format!(
                "not located (search: {}, certified matches {}; no derivation reached it)",
                count.map_or("no window".to_string(), |c| format!("{c} matches")),
                a.certified_matches.map_or("None".to_string(), |m| m.to_string())
            ),
        );
    }
    for (j, v) in t.vtables.iter().enumerate() {
        if loc.vtables[j].is_none() {
            return fail(format!("vtable {}", v.name), 9, "not located".into());
        }
    }
    finish(pe, loc, how, vhow, Mode::Relocated, Some(stats))
}

/// Rule 2: a search places an anchor only if it matched once when certified and once now.
fn search_accepts(a: &Anchor, places: &[i64]) -> bool {
    a.certified_matches == Some(1) && places.len() == 1
}

/// Rules 4 and 6 over everything, with every place known, and the result.
fn finish(
    pe: &mut Pe,
    loc: Located,
    how: Vec<Option<String>>,
    vhow: Vec<Option<String>>,
    mode: Mode,
    search: Option<SearchStats>,
) -> R<Resolved> {
    let t = pe.p.table;
    for (i, a) in t.anchors.iter().enumerate() {
        let Some(base) = loc.anchors[i] else { continue };
        if let Err(why) = check_fields(pe, i, base, &loc)? {
            return fail(a.name, 4, why);
        }
    }
    for (j, v) in t.vtables.iter().enumerate() {
        let at = loc.vtables[j].expect("every vtable is located before finish");
        if let Err(why) = vtable_slots_ok(pe, j, at, &loc, false)? {
            return fail(format!("vtable {}", v.name), 6, why);
        }
    }
    let tls = pe.tls_index_rva()?.expect("the header check read it");
    let rdata = pe.h().section(".rdata").map_or((0, 0), |s| (u64::from(s.va), u64::from(s.va) + u64::from(s.vs)));
    let to_u64 = |v: Option<i64>| v.map(|v| v as u64);
    // The raw bytes of each anchor where it is: every one was read for its digest above, so this
    // reads no page that is not already in.
    let mut file_sha256 = Vec::with_capacity(t.anchors.len());
    for (i, a) in t.anchors.iter().enumerate() {
        file_sha256.push(match loc.anchors[i] {
            Some(at) => pe.read(at, a.len as usize)?.map(|raw| sha256(&raw)),
            None => None,
        });
    }
    Ok(Resolved {
        mode,
        image_size: pe.h().image_size,
        tls_index: tls as u64,
        rdata,
        anchors: t
            .anchors
            .iter()
            .zip(how)
            .zip(file_sha256)
            .enumerate()
            .map(|(i, ((a, how), file_sha256))| Place { name: a.name, rva: to_u64(loc.anchors[i]), how, file_sha256 })
            .collect(),
        vtables: t
            .vtables
            .iter()
            .zip(vhow)
            .enumerate()
            .map(|(j, (v, how))| Place { name: v.name, rva: to_u64(loc.vtables[j]), how, file_sha256: None })
            .collect(),
        search,
    })
}

/// A small table and the files that hold it, for the tests: a certified build and one where
/// every anchor, callee, content and vtable moved by its own delta, with the fields, slots,
/// `.pdata` and `.reloc` kept coherent, as an update of the game that only moved code would.
/// Nine anchors cover every way the resolver locates one: search, field, slot, twins, a
/// constructor's lea, `.rdata`, the TLS getter. The bytes are a pseudo-random fill: none is
/// the game's.
#[cfg(test)]
pub(crate) mod synthetic {
    use super::*;
    use crate::certificate::{self, Certificate, Span};
    use crate::relocation::{Field, Search, VtableSlot, Within};
    use std::sync::OnceLock;

    pub const IB: u64 = 0x1_4000_0000;
    const PE: usize = 0x80;
    /// (name, RVA, virtual size, file offset, raw size). Every file offset is the RVA less
    /// 0xc00, so that a mistake between the two shows.
    const SECTIONS: [(&str, u32, u32, u32, u32); 4] = [
        (".text", 0x1000, 0x18000, 0x400, 0x18000),
        (".rdata", 0x19000, 0x1000, 0x18400, 0x1000),
        (".pdata", 0x1a000, 0x100, 0x19400, 0x200),
        (".reloc", 0x1b000, 0x100, 0x19600, 0x200),
    ];
    pub const LEN: usize = 0x19800;
    pub const IMAGE_SIZE: u32 = 0x1c000;
    pub const TLS_DIR: u32 = 0x19400;
    pub const TLS_INDEX: u32 = 0x19500;

    /// Where each piece is, and what can be made to differ.
    #[derive(Debug, Clone)]
    pub struct Places {
        pub root: u32,
        pub derived: u32,
        pub ctor: u32,
        pub slotfn: u32,
        pub slotonly: u32,
        pub twin: u32,
        pub twin_copy: u32,
        pub tlsget: u32,
        pub lonely: u32,
        pub callee0: u32,
        pub callee1: u32,
        pub callee2: u32,
        pub inside_fn: u32,
        pub leaf: u32,
        pub code: u32,
        pub table: u32,
        pub content: u32,
        pub vt: u32,
        /// What the vtable's slots +0 and +8 hold, if not the functions they name.
        pub slot0: Option<u32>,
        pub slot8: Option<u32>,
        /// What the twin that is the anchor calls, if not its callee.
        pub twin_calls: Option<u32>,
        /// A second copy of `s:lonely`.
        pub lonely_copy: Option<u32>,
        /// Where the base relocation of the content's pointer is, from the content.
        pub content_reloc: u32,
        /// The section names the header gives.
        pub names: [&'static str; 4],
    }

    pub fn certified() -> Places {
        Places {
            root: 0x1100,
            derived: 0x1200,
            ctor: 0x1300,
            slotfn: 0x1400,
            slotonly: 0x1500,
            twin: 0x1600,
            twin_copy: 0x1700,
            tlsget: 0x1800,
            // Its window, [4, 20) of it, is at file offsets 0xfffa..0x1000a: across the first
            // two reads of 64 KiB.
            lonely: 0x10bf6,
            callee0: 0x2000,
            callee1: 0x2100,
            callee2: 0x2200,
            inside_fn: 0x2300,
            leaf: 0x2400,
            code: 0x2500,
            table: 0x19100,
            content: 0x19200,
            vt: 0x19300,
            slot0: None,
            slot8: None,
            twin_calls: None,
            lonely_copy: None,
            content_reloc: 8,
            names: [".text", ".rdata", ".pdata", ".reloc"],
        }
    }

    /// Every piece moved, each by its own delta (the copy of the twin and its callee stay).
    pub fn moved() -> Places {
        Places {
            root: 0x5100,
            derived: 0x3210,
            ctor: 0x7300,
            slotfn: 0x6400,
            slotonly: 0x8508,
            twin: 0x9600,
            tlsget: 0xa800,
            lonely: 0x12000,
            callee0: 0xb000,
            callee1: 0xb100,
            inside_fn: 0xc300,
            leaf: 0xd400,
            code: 0xe500,
            table: 0x19600,
            content: 0x19700,
            vt: 0x19880,
            ..certified()
        }
    }

    pub fn off(rva: u32) -> usize {
        let s = SECTIONS.iter().find(|s| rva >= s.1 && rva < s.1 + s.2).expect("rva in a section");
        (rva - s.1 + s.3) as usize
    }

    fn fill(seed: u64, len: usize) -> Vec<u8> {
        let mut x = seed.wrapping_mul(0x9e37_79b9_7f4a_7c15) | 1;
        (0..len)
            .map(|_| {
                x ^= x << 13;
                x ^= x >> 7;
                x ^= x << 17;
                (x >> 24) as u8
            })
            .collect()
    }

    /// The fields each piece of code has: (at, kind, tail, where it points).
    fn code_fields(p: &Places) -> Vec<(u32, u64, usize, Vec<(u32, FieldKind, u8, u32)>)> {
        // (place, seed, len, fields)
        vec![
            (
                p.root,
                1,
                64,
                vec![
                    (0, FieldKind::Rip, 0, p.table),
                    (4, FieldKind::Rel32, 0, p.derived),
                    (8, FieldKind::Rip, 0, p.callee0),
                    (12, FieldKind::Rel32, 0, p.inside_fn + 8),
                    (16, FieldKind::Rel32, 0, p.leaf),
                    (20, FieldKind::Rip, 1, p.content),
                    (28, FieldKind::Rip, 0, 0),
                ],
            ),
            (p.derived, 2, 32, vec![(8, FieldKind::Rel32, 0, p.derived + 24)]),
            (p.ctor, 3, 48, vec![(4, FieldKind::Rip, 0, p.vt), (10, FieldKind::Rel32, 0, p.code)]),
            (p.slotfn, 4, 24, vec![]),
            (p.slotonly, 5, 8, vec![]),
            (p.twin, 6, 40, vec![(4, FieldKind::Rel32, 0, p.twin_calls.unwrap_or(p.callee1))]),
            (p.twin_copy, 6, 40, vec![(4, FieldKind::Rel32, 0, p.callee2)]),
            (p.tlsget, 7, 32, vec![(4, FieldKind::Rip, 0, TLS_INDEX)]),
            (p.lonely, 8, 24, vec![]),
            (p.callee0, 10, 32, vec![]),
            (p.callee1, 11, 24, vec![]),
            (p.callee2, 12, 24, vec![]),
            (p.inside_fn, 13, 48, vec![(20, FieldKind::Rel32, 0, p.callee0)]),
            (p.leaf, 14, 12, vec![]),
        ]
    }

    fn put(file: &mut [u8], rva: u32, bytes: &[u8]) {
        let o = off(rva);
        file[o..o + bytes.len()].copy_from_slice(bytes);
    }

    /// The file for `p`.
    pub fn file(p: &Places) -> Vec<u8> {
        let mut f = fill(99, LEN);
        f[..0x400].fill(0);
        for s in &SECTIONS[2..] {
            f[s.3 as usize..(s.3 + s.4) as usize].fill(0);
        }
        // Header.
        f[..2].copy_from_slice(b"MZ");
        f[0x3c..0x40].copy_from_slice(&(PE as u32).to_le_bytes());
        f[PE..PE + 4].copy_from_slice(b"PE\0\0");
        f[PE + 4..PE + 6].copy_from_slice(&0x8664u16.to_le_bytes());
        f[PE + 6..PE + 8].copy_from_slice(&(SECTIONS.len() as u16).to_le_bytes());
        f[PE + 20..PE + 22].copy_from_slice(&240u16.to_le_bytes());
        f[PE + 24..PE + 26].copy_from_slice(&0x20bu16.to_le_bytes());
        f[PE + 48..PE + 56].copy_from_slice(&IB.to_le_bytes());
        f[PE + 80..PE + 84].copy_from_slice(&IMAGE_SIZE.to_le_bytes());
        for (i, s) in SECTIONS.iter().enumerate() {
            let at = PE + 24 + 240 + 40 * i;
            let mut name = [0u8; 8];
            name[..p.names[i].len()].copy_from_slice(p.names[i].as_bytes());
            f[at..at + 8].copy_from_slice(&name);
            f[at + 8..at + 12].copy_from_slice(&s.2.to_le_bytes());
            f[at + 12..at + 16].copy_from_slice(&s.1.to_le_bytes());
            f[at + 16..at + 20].copy_from_slice(&s.4.to_le_bytes());
            f[at + 20..at + 24].copy_from_slice(&s.3.to_le_bytes());
        }
        // Code, then its fields.
        let mut pieces = code_fields(p);
        if let Some(c) = p.lonely_copy {
            pieces.push((c, 8, 24, vec![]));
        }
        for (place, seed, len, fields) in pieces {
            put(&mut f, place, &fill(seed, len));
            for (at, kind, tail, target) in fields {
                let field = place + at;
                let v = match kind {
                    FieldKind::Rel32 | FieldKind::Rip => {
                        (i64::from(target) - (i64::from(field) + 4 + i64::from(tail))) as i32 as u32
                    }
                    _ => target,
                };
                put(&mut f, field, &v.to_le_bytes());
            }
        }
        // .rdata: the table anchor, the content and its pointer, the vtable, the TLS directory.
        put(&mut f, p.table, &fill(20, 32));
        put(&mut f, p.content, &fill(21, 16));
        put(&mut f, p.content + 8, &(IB + u64::from(p.callee0)).to_le_bytes());
        put(&mut f, p.vt, &(IB + u64::from(p.slot0.unwrap_or(p.slotfn))).to_le_bytes());
        put(&mut f, p.vt + 8, &(IB + u64::from(p.slot8.unwrap_or(p.slotonly))).to_le_bytes());
        put(&mut f, TLS_DIR, &[0; 0x28]);
        put(&mut f, TLS_DIR + 16, &(IB + u64::from(TLS_INDEX)).to_le_bytes());
        // .pdata, in no particular order.
        let functions = [(p.root, 64), (p.callee0, 32), (p.callee1, 24), (p.callee2, 24), (p.inside_fn, 48)];
        for (i, (b, len)) in functions.iter().enumerate() {
            let at = SECTIONS[2].3 as usize + 12 * i;
            f[at..at + 4].copy_from_slice(&b.to_le_bytes());
            f[at + 4..at + 8].copy_from_slice(&(b + len).to_le_bytes());
        }
        // .reloc: one block per page.
        let mut relocs = vec![p.content + p.content_reloc, p.vt, p.vt + 8, TLS_DIR + 16];
        relocs.sort_unstable();
        let mut blocks: Vec<u8> = Vec::new();
        let mut i = 0;
        while i < relocs.len() {
            let page = relocs[i] & !0xfff;
            let mut entries: Vec<u16> = Vec::new();
            while i < relocs.len() && relocs[i] & !0xfff == page {
                entries.push(0xa000 | (relocs[i] & 0xfff) as u16);
                i += 1;
            }
            if entries.len() % 2 == 1 {
                entries.push(0);
            }
            blocks.extend_from_slice(&page.to_le_bytes());
            blocks.extend_from_slice(&(8 + 2 * entries.len() as u32).to_le_bytes());
            for e in entries {
                blocks.extend_from_slice(&e.to_le_bytes());
            }
        }
        put(&mut f, SECTIONS[3].1, &blocks);
        let dir = |i: usize| PE + 24 + 112 + 8 * i;
        f[dir(5)..dir(5) + 4].copy_from_slice(&SECTIONS[3].1.to_le_bytes());
        f[dir(5) + 4..dir(5) + 8].copy_from_slice(&(blocks.len() as u32).to_le_bytes());
        f[dir(9)..dir(9) + 4].copy_from_slice(&TLS_DIR.to_le_bytes());
        f[dir(9) + 4..dir(9) + 8].copy_from_slice(&0x28u32.to_le_bytes());
        f
    }

    fn leak<T>(v: Vec<T>) -> &'static [T] {
        Box::leak(v.into_boxed_slice())
    }

    fn leak_str(s: String) -> &'static str {
        Box::leak(s.into_boxed_str())
    }

    /// The table, taken from the certified file as the generator takes the game's.
    pub fn table() -> &'static Table {
        static TABLE: OnceLock<&'static Table> = OnceLock::new();
        TABLE.get_or_init(|| {
            let p = certified();
            let f = file(&p);
            let at = |rva: u32, len: usize| f[off(rva)..off(rva) + len].to_vec();
            let field = |at: u32, kind, tail, target| Field { at, kind, tail, target };
            // (name, kind, section, rva, len, align, matches, survivors, search at, function, fields)
            type Row = (&'static str, AnchorKind, Section, u32, u32, u32, Option<u32>, Option<u32>, Option<u32>, Option<Within>, Vec<Field>);
            let rows: Vec<Row> = vec![
                (
                    "s:root",
                    AnchorKind::Span,
                    Section::Text,
                    p.root,
                    64,
                    1,
                    Some(1),
                    None,
                    Some(40),
                    Some(Within { offset: 0, len: 64 }),
                    vec![
                        field(0, FieldKind::Rip, 0, Target::Anchor { anchor: 6, add: 0 }),
                        field(4, FieldKind::Rel32, 0, Target::Anchor { anchor: 1, add: 0 }),
                        field(8, FieldKind::Rip, 0, Target::Callee(0)),
                        field(12, FieldKind::Rel32, 0, Target::Callee(2)),
                        field(16, FieldKind::Rel32, 0, Target::Callee(3)),
                        field(20, FieldKind::Rip, 1, Target::Content(0)),
                        field(28, FieldKind::Rip, 0, Target::ImageBase),
                    ],
                ),
                ("s:derived", AnchorKind::Span, Section::Text, p.derived, 32, 1, Some(1), None, None, None, vec![field(8, FieldKind::Rel32, 0, Target::Internal(24))]),
                (
                    "ctor:vt",
                    AnchorKind::Constructor,
                    Section::Text,
                    p.ctor,
                    48,
                    1,
                    Some(1),
                    None,
                    Some(16),
                    None,
                    vec![field(4, FieldKind::Rip, 0, Target::Vtable(0)), field(10, FieldKind::Rel32, 0, Target::Code)],
                ),
                ("s:slotfn", AnchorKind::Span, Section::Text, p.slotfn, 24, 1, Some(1), None, Some(4), None, vec![]),
                ("s:slotonly", AnchorKind::Span, Section::Text, p.slotonly, 8, 1, Some(1), None, None, None, vec![]),
                ("s:twin", AnchorKind::Span, Section::Text, p.twin, 40, 1, Some(2), Some(1), Some(20), None, vec![field(4, FieldKind::Rel32, 0, Target::Callee(1))]),
                ("s:table", AnchorKind::Span, Section::Rdata, p.table, 32, 8, Some(1), None, Some(8), None, vec![]),
                ("tls:getter", AnchorKind::TlsGetter, Section::Text, p.tlsget, 32, 1, Some(1), None, Some(12), None, vec![field(4, FieldKind::Rip, 0, Target::TlsIndex)]),
                ("s:lonely", AnchorKind::Span, Section::Text, p.lonely, 24, 1, Some(1), None, Some(4), None, vec![]),
            ];
            let anchors = rows
                .into_iter()
                .map(|(name, kind, section, rva, len, align, matches, survivors, search, function, fields)| {
                    let raw = at(rva, len as usize);
                    let normalized_sha256 = leak_str(sha_hex(&normalized(&raw, fields.iter().map(|f| f.at))));
                    Anchor {
                        name,
                        kind,
                        section,
                        rva,
                        len,
                        align,
                        normalized_sha256,
                        certified_matches: matches,
                        certified_survivors: survivors,
                        search: search.map(|w| Search {
                            k: 16,
                            at: w,
                            hash: relocation::rolling_hash(&raw[w as usize..w as usize + 16]),
                        }),
                        function,
                        fields: leak(fields),
                    }
                })
                .collect();
            let callee = |rva: u32, bound, offset: u32, len: u32, mask: Vec<u32>| {
                let raw = at(rva - offset, len as usize);
                Callee {
                    rva,
                    bound,
                    offset,
                    len,
                    normalized_sha256: leak_str(sha_hex(&normalized(&raw, mask.iter().copied()))),
                    mask: leak(mask),
                }
            };
            let callees = vec![
                callee(p.callee0, Bound::Function, 0, 32, vec![]),
                callee(p.callee1, Bound::Function, 0, 24, vec![]),
                callee(p.inside_fn + 8, Bound::Inside, 8, 48, vec![20]),
                callee(p.leaf, Bound::Leaf, 0, 12, vec![]),
            ];
            let mut content = at(p.content, 16);
            content[8..16].fill(0);
            let contents = vec![Content { rva: p.content, len: 16, sha256: leak_str(sha_hex(&content)), pointers: leak(vec![8]) }];
            let vtables = vec![Vtable {
                name: "vt",
                rva: p.vt,
                slots: leak(vec![
                    VtableSlot { offset: 0, anchor: 3, added: false },
                    VtableSlot { offset: 8, anchor: 4, added: false },
                ]),
            }];
            &*Box::leak(Box::new(Table {
                image_base: IB,
                section_names: leak(SECTIONS.iter().map(|s| s.0).collect()),
                anchors: leak(anchors),
                vtables: leak(vtables),
                callees: leak(callees),
                contents: leak(contents),
            }))
        })
    }

    /// The certificate of the certified file: its spans at their file offsets, the slots, the
    /// header and the TLS directory.
    pub fn certificate() -> &'static Certificate {
        static CERTIFICATE: OnceLock<&'static Certificate> = OnceLock::new();
        CERTIFICATE.get_or_init(|| {
            let t = table();
            let p = certified();
            let f = file(&p);
            let spans = t
                .anchors
                .iter()
                .filter(|a| a.kind == AnchorKind::Span)
                .map(|a| {
                    let o = off(a.rva);
                    Span { name: a.name, offset: o as u64, len: a.len, sha256: leak_str(sha_hex(&f[o..o + a.len as usize])) }
                })
                .collect();
            let slots = vec![
                certificate::Slot { name: "vtable vt +0x0", offset: off(p.vt) as u64, rva: u64::from(p.slotfn) },
                certificate::Slot { name: "vtable vt +0x8", offset: off(p.vt + 8) as u64, rva: u64::from(p.slotonly) },
            ];
            let sections = SECTIONS
                .iter()
                .map(|s| {
                    let mut name = [0u8; 8];
                    name[..s.0.len()].copy_from_slice(s.0.as_bytes());
                    certificate::Section { name, rva: s.1, virtual_size: s.2, file_offset: s.3 }
                })
                .collect();
            &*Box::leak(Box::new(Certificate {
                image_base: IB,
                image_size: IMAGE_SIZE,
                tls_directory_rva: TLS_DIR,
                tls_directory_offset: off(TLS_DIR) as u64,
                tls_index_rva: u64::from(TLS_INDEX),
                sections: leak(sections),
                spans: leak(spans),
                slots: leak(slots),
            }))
        })
    }

    /// Where each anchor of [`table`] is in a file made from `p`, in the table's order.
    pub fn expected(p: &Places) -> Vec<u64> {
        [p.root, p.derived, p.ctor, p.slotfn, p.slotonly, p.twin, p.table, p.tlsget, p.lonely]
            .iter()
            .map(|&r| u64::from(r))
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::synthetic::*;
    use super::*;
    use crate::certificate::Certificate;
    use crate::executable::{Build, HashedFile, Hashing, Stamp};
    use std::time::{Duration, Instant, SystemTime};

    /// A file in memory, read in order in reads of `chunk` bytes, as the adapter hands one in.
    struct Bytes {
        data: Vec<u8>,
        pos: usize,
        chunk: usize,
        /// Reads at points so far, and the one after which they fail.
        reads_at: usize,
        read_at_fails_after: Option<usize>,
        /// The file is written once the hash has ended.
        written_after_hash: bool,
    }

    impl Bytes {
        fn of(data: Vec<u8>, chunk: usize) -> Self {
            Self { data, pos: 0, chunk, reads_at: 0, read_at_fails_after: None, written_after_hash: false }
        }
    }

    impl HashedFile for Bytes {
        fn stamp(&mut self) -> Option<Stamp> {
            let later = self.written_after_hash && self.pos == self.data.len() && self.reads_at > 0;
            Some(Stamp { len: self.data.len() as u64, modified: Some(SystemTime::UNIX_EPOCH + Duration::from_secs(u64::from(later))) })
        }
        fn read(&mut self, buffer: &mut [u8]) -> Option<usize> {
            let n = self.chunk.min(buffer.len()).min(self.data.len() - self.pos);
            buffer[..n].copy_from_slice(&self.data[self.pos..self.pos + n]);
            self.pos += n;
            Some(n)
        }
        fn update(&mut self, _: &[u8]) -> bool {
            true
        }
        fn finish(&mut self) -> Option<[u8; 32]> {
            Some([0; 32])
        }
        fn read_at(&mut self, offset: u64, buffer: &mut [u8]) -> Option<usize> {
            if self.read_at_fails_after.is_some_and(|n| self.reads_at >= n) {
                return None;
            }
            self.reads_at += 1;
            let from = (offset as usize).min(self.data.len());
            let n = buffer.len().min(self.data.len() - from);
            buffer[..n].copy_from_slice(&self.data[from..from + n]);
            Some(n)
        }
    }

    /// Hashes `file` whole against `certificate` and `table`: the build, when the certificate
    /// holds at its fixed offsets and the hash decides it, and `None` when it does not and
    /// `advance` would go on to the resolution, which is left to the test.
    fn hashed(file: Bytes, certificate: &'static Certificate, table: &'static Table) -> (Hashing<Bytes>, Option<Build>) {
        let mut h = Hashing::start_with_table(file, certificate, table).ok().expect("start");
        match h.advance(|| false) {
            Step::Decided(build) => (h, Some(build)),
            Step::Unfinished if h.resolving() => (h, None),
            _ => panic!("the hash did not end"),
        }
    }

    fn resolve_synthetic(data: Vec<u8>, chunk: usize, by_search: bool) -> Result<Resolved, Failure> {
        let (mut h, _) = hashed(Bytes::of(data, chunk), certificate(), table());
        let step = if by_search { h.resolve_by_search(|| false) } else { h.resolve(|| false) };
        match step {
            Step::Decided(r) => r,
            _ => panic!("the resolution did not decide"),
        }
    }

    fn rvas(r: &Resolved) -> Vec<u64> {
        r.anchors.iter().map(|a| a.rva.expect(a.name)).collect()
    }

    fn hows(r: &Resolved) -> Vec<&str> {
        r.anchors.iter().map(|a| a.how.as_deref().unwrap_or("-")).collect()
    }

    #[test]
    fn the_certified_file_resolves_by_identity_whatever_the_size_of_the_reads() {
        let file = file(&certified());
        for chunk in [1, 7, 4096, 65536] {
            let (_, build) = hashed(Bytes::of(file.clone(), chunk), certificate(), table());
            assert_eq!(build, Some(Build::Certified([0; 32])), "the synthetic certificate holds");
            let r = resolve_synthetic(file.clone(), chunk, false).unwrap();
            assert_eq!(r.mode, Mode::Identity, "reads of {chunk}");
            assert_eq!(rvas(&r), expected(&certified()));
            assert!(hows(&r).iter().all(|&h| h == "identity"));
            assert_eq!(r.vtables[0].rva, Some(u64::from(certified().vt)));
            assert_eq!((r.tls_index, r.image_size, r.rdata), (u64::from(TLS_INDEX), IMAGE_SIZE, (0x19000, 0x1a000)));
            assert_eq!(r.search, None);
        }
    }

    #[test]
    fn the_certified_file_resolves_to_itself_by_search_alone() {
        let r = resolve_synthetic(file(&certified()), 65536, true).unwrap();
        assert_eq!(r.mode, Mode::Relocated);
        assert_eq!(rvas(&r), expected(&certified()));
        // Seven searched windows, the twin's at two places; nothing turned away by the digest.
        assert_eq!(r.search, Some(SearchStats { bytes: 0x18000 + 0x1000, hash_hits: 8, false_candidates: 0 }));
    }

    /// The study's case: code moved, by a different delta for each piece, and everything that
    /// points at it rewritten. Every anchor is found again, each the way the table means.
    #[test]
    fn a_file_whose_code_moved_resolves_to_the_moved_places() {
        let p = moved();
        let file = file(&p);
        let (_, build) = hashed(Bytes::of(file.clone(), 65536), certificate(), table());
        assert_eq!(build, None, "not the certified offsets");
        for chunk in [1, 4096, 65536] {
            let r = resolve_synthetic(file.clone(), chunk, false).unwrap();
            assert_eq!(r.mode, Mode::Relocated);
            assert_eq!(rvas(&r), expected(&p), "reads of {chunk}");
            assert_eq!(
                hows(&r),
                [
                    "search (1 match)",
                    "field +4 of s:root",
                    "search (1 match)",
                    "search (1 match)",
                    "slot +0x8 of vtable vt",
                    "search (2 matches), the field checks pick one",
                    "search (1 match)",
                    "search (1 match)",
                    "search (1 match)",
                ]
            );
            assert_eq!(
                r.vtables[0],
                Place { name: "vt", rva: Some(u64::from(p.vt)), how: Some("lea in ctor:vt".into()), file_sha256: None }
            );
            // Each anchor carries the digest of the moved file's bytes where it was found.
            for (place, a) in r.anchors.iter().zip(table().anchors) {
                let o = off(place.rva.unwrap() as u32);
                assert_eq!(place.file_sha256, Some(crate::sha256::sha256(&file[o..o + a.len as usize])), "{}", a.name);
            }
            assert_eq!(r.resolved_rva("vtable vt"), Some(u64::from(p.vt)));
            assert_eq!(r.resolved_rva("s:derived"), Some(u64::from(p.derived)));
            assert_eq!(r.resolved_rva("vt"), None, "a vtable is named as a failure names it");
            assert_eq!(r.resolved_rva("nothing"), None);
        }
        assert_eq!(table().names().last().map(String::as_str), Some("vtable vt"));
    }

    /// The window of `s:lonely` starts 6 bytes before the end of the first read of 64 KiB and
    /// ends in the second: the hash goes on across the two.
    #[test]
    fn a_window_across_two_reads_of_64_kib_is_found() {
        let p = certified();
        let window = off(p.lonely) + 4;
        assert!(window < 65536 && window + 16 > 65536, "{window:#x}");
        let file = file(&p);
        let lonely = table().anchors.iter().position(|a| a.name == "s:lonely").unwrap();
        for chunks in [vec![65536, file.len() - 65536], vec![file.len()], vec![window + 8 - 1, 1, file.len() - window - 8]] {
            let mut s = Scanner::new(table());
            let mut at = 0;
            for n in chunks.iter().copied() {
                s.feed(at as u64, &file[at..at + n]);
                at += n;
            }
            assert_eq!(s.candidates[lonely], [i64::from(p.lonely)], "{chunks:?}");
            assert_eq!(s.scanned(), 0x19000);
        }
        // After the first read alone the window is not whole yet; the second completes it.
        let mut s = Scanner::new(table());
        s.feed(0, &file[..65536]);
        assert!(s.candidates[lonely].is_empty());
        s.feed(65536, &file[65536..65536 + 16]);
        assert_eq!(s.candidates[lonely], [i64::from(p.lonely)]);
    }

    #[test]
    fn a_file_fed_out_of_order_says_nothing() {
        let file = file(&certified());
        let mut s = Scanner::new(table());
        s.feed(0, &file[..100]);
        s.feed(200, &file[200..]);
        let mut r = Resolution::new(&s);
        assert_eq!(r.step(false, &mut |_, _| Some(0), &mut || false), Step::Failed);
    }

    /// The clock decides where a step stops, never what it gives: a resolution in as many steps
    /// as it has reads is the one in a single step, and a step that is over before it starts
    /// reads nothing.
    #[test]
    fn the_resolution_goes_on_across_steps_and_gives_the_same() {
        let file = file(&moved());
        let whole = resolve_synthetic(file.clone(), 65536, false).unwrap();
        let (mut h, _) = hashed(Bytes::of(file, 65536), certificate(), table());
        for _ in 0..3 {
            assert_eq!(h.resolve(|| true), Step::Unfinished);
        }
        assert_eq!(h.resolution_reads(), (0, 0));
        let mut steps = 0;
        let outcome = loop {
            let mut asked = 0;
            steps += 1;
            match h.resolve(|| {
                asked += 1;
                asked > 1
            }) {
                Step::Unfinished => continue,
                other => break other,
            }
        };
        let (reads, bytes) = h.resolution_reads();
        assert_eq!(outcome, Step::Decided(Ok(whole)));
        assert_eq!(steps, reads, "one read per step, and the one with the last read decides");
        assert!(reads > 5 && bytes <= reads * 4096, "{reads} reads, {bytes} bytes");
    }

    #[test]
    fn what_the_system_fails_to_do_decides_nothing() {
        let data = file(&moved());
        // A read at a point that fails.
        let mut file = Bytes::of(data.clone(), 65536);
        file.read_at_fails_after = Some(2);
        let (mut h, _) = hashed(file, certificate(), table());
        assert_eq!(h.resolve(|| false), Step::Failed);
        // The file written after it was hashed.
        let mut file = Bytes::of(data.clone(), 65536);
        file.written_after_hash = true;
        let (mut h, _) = hashed(file, certificate(), table());
        assert_eq!(h.resolve(|| false), Step::Failed);
        // Before the hash has decided.
        let mut h = Hashing::start_with_table(Bytes::of(data, 65536), certificate(), table()).ok().unwrap();
        assert_eq!(h.resolve(|| false), Step::Failed);
    }

    /// One case of each rule, on the moved file unless it is the identity path: what the
    /// resolver says, literally.
    fn negative(p: Places, edit: impl Fn(&mut Vec<u8>)) -> Failure {
        let mut f = file(&p);
        edit(&mut f);
        let failure = resolve_synthetic(f, 65536, false).expect_err("it must not resolve");
        println!("{failure}");
        failure
    }

    fn is(f: &Failure, anchor: &str, rule: u8) {
        assert_eq!((f.anchor.as_str(), f.rule), (anchor, rule), "{f}");
    }

    #[test]
    fn a_field_offset_that_changed_fails_the_anchor_by_rule_3() {
        // A byte of s:derived outside its one field: an offset or a constant, never masked
        // (rule 7), so its signature does not match where s:root's call derives it.
        let p = moved();
        let f = negative(p.clone(), |f| f[off(p.derived + 20)] ^= 0x08);
        is(&f, "s:derived", 3);
        assert_eq!(f.detail, "derived at 0x3210 by field +4 of s:root, its signature does not match there");
    }

    #[test]
    fn a_twin_that_calls_another_callee_is_rejected_by_rule_4() {
        // The anchor's own place now calls what its twin calls: neither passes.
        let mut p = moved();
        p.twin_calls = Some(p.callee2);
        let f = negative(p, |_| {});
        is(&f, "s:twin", 4);
        assert_eq!(f.detail, "2 matches, none passes the field checks");
        // A callee whose body changed.
        let p = moved();
        let f = negative(p.clone(), |f| f[off(p.callee0 + 3)] ^= 1);
        is(&f, "s:root", 4);
        assert_eq!(f.detail, "field +8 (rip) calls 0xb000, which is not the certified callee (function, 32 bytes)");
    }

    #[test]
    fn an_anchor_found_twice_that_nothing_derives_is_ambiguous_by_rule_2() {
        let mut p = moved();
        p.lonely_copy = Some(0x14000);
        let f = negative(p, |_| {});
        is(&f, "s:lonely", 2);
        assert_eq!(f.detail, "certified with 1 match, 2 now (2 pass every field check): ambiguous, and nothing derives it");
    }

    #[test]
    fn a_slot_that_does_not_hold_its_function_fails_by_rule_6_or_3() {
        // s:slotfn is found by search: the slot that names another place contradicts it.
        let mut p = moved();
        p.slot0 = Some(p.lonely);
        let f = negative(p, |_| {});
        is(&f, "vtable vt", 6);
        assert_eq!(f.detail, "lea in ctor:vt names 0x19880: slot +0x0 holds 0x12000, s:slotfn is at 0x6400");
        // s:slotonly is only found through its slot: there, its signature does not match.
        let mut p = moved();
        p.slot8 = Some(p.slotonly + 1);
        let f = negative(p, |_| {});
        is(&f, "s:slotonly", 3);
        assert_eq!(f.detail, "slot +0x8 of vtable vt holds 0x8509, where its signature does not match");
    }

    #[test]
    fn a_content_whose_relocations_moved_is_not_the_certified_content_by_rule_4() {
        let mut p = moved();
        p.content_reloc = 4;
        let f = negative(p, |_| {});
        is(&f, "s:root", 4);
        assert_eq!(f.detail, "field +20 (rip) reads 0x19700, whose 16 bytes are not the certified ones");
    }

    #[test]
    fn the_identity_path_still_checks_what_the_certificate_does_not_name() {
        // A byte of the constructor, which no span of the certificate covers: the certificate
        // holds, rule 1 does not.
        let p = certified();
        let f = negative(p.clone(), |f| f[off(p.ctor + 30)] ^= 1);
        is(&f, "ctor:vt", 1);
        assert_eq!(f.detail, "the certificate holds but this anchor does not match at its RVA");
        // Its callee, which the certificate does not name either: rule 4, by identity too.
        let f = negative(p.clone(), |f| f[off(p.inside_fn + 40)] ^= 1);
        is(&f, "s:root", 4);
    }

    #[test]
    fn a_header_that_is_not_the_certified_layout_fails_by_rule_8() {
        let mut p = moved();
        p.names[1] = ".data";
        let f = negative(p, |_| {});
        is(&f, "header", 8);
        assert_eq!(f.detail, "sections ['.text', '.data', '.pdata', '.reloc']");
        let f = negative(moved(), |f| f[PE_MACHINE] = 0x4c);
        is(&f, "header", 8);
        assert_eq!(f.detail, "not AMD64 PE32+");
    }
    const PE_MACHINE: usize = 0x84;

    #[test]
    fn one_anchor_that_is_not_found_fails_the_whole_by_rule_9() {
        let p = moved();
        let f = negative(p.clone(), |f| f[off(p.lonely + 10)] ^= 1);
        is(&f, "s:lonely", 9);
        assert_eq!(f.detail, "not located (search: 0 matches, certified matches 1; no derivation reached it)");
        assert_eq!(
            f.to_string(),
            "not resolved: s:lonely: rule 9 (all or nothing): not located (search: 0 matches, certified matches 1; no derivation reached it)"
        );
    }

    #[test]
    fn a_constructor_that_is_not_found_is_optional_but_what_only_its_vtable_reaches_is_not() {
        // Without its constructor the vtable has no way, nor the function only its slot names.
        let p = moved();
        let f = negative(p.clone(), |f| f[off(p.ctor + 20)] ^= 1);
        is(&f, "s:slotonly", 9);
        assert_eq!(f.detail, "not located (search: no window, certified matches 1; no derivation reached it)");
    }

    #[test]
    fn every_searched_anchor_of_the_game_shares_one_window_length() {
        let ks: std::collections::BTreeSet<u8> = GAME.anchors.iter().filter_map(|a| a.search.map(|s| s.k)).collect();
        assert_eq!(ks.len(), 1, "{ks:?}");
        assert_eq!(GAME.names().len(), GAME.anchors.len() + GAME.vtables.len());
    }

    // --------------------------------------------------------------------------------------
    // The real file, when there is a copy to point at. Each runs the Python resolver over the
    // same copy and the same table and compares every place, every `how`, the mode, the TLS
    // index, the image size, the bounds of `.rdata` and the cost of the search:
    //
    //   TC_GW2_EXE=<copy> cargo test --offline -p tyrian_companion_nexus_core resolver -- --ignored
    //   TC_GW2_MOVED_EXE=<moved copy of test_relocation.py --keep-moved> ...
    // --------------------------------------------------------------------------------------

    fn python_resolve(exe: &str, force_search: bool) -> serde_json::Value {
        let script = concat!(env!("CARGO_MANIFEST_DIR"), "/../scripts/relocation/gen_relocation.py");
        let out = std::env::temp_dir().join(format!("tc-resolve-{}-{force_search}.json", std::process::id()));
        let mut args = vec!["-I", script, "--resolve", exe, "--json", out.to_str().unwrap()];
        if force_search {
            args.push("--force-search");
        }
        let status = std::process::Command::new("python3").args(&args).stdout(std::process::Stdio::null()).status().expect("python3");
        let json = std::fs::read_to_string(&out).unwrap_or_else(|_| panic!("python said {status}"));
        let _ = std::fs::remove_file(&out);
        serde_json::from_str(&json).unwrap()
    }

    fn real(path: &str, by_search: bool, chunk: usize, one_read_per_step: bool) -> (Result<Resolved, Failure>, u64, u64, u64) {
        let data = std::fs::read(path).expect("read the copy");
        let (mut h, _) = hashed(Bytes::of(data, chunk), &crate::certificate::GAME, &GAME);
        let mut steps = 0;
        loop {
            steps += 1;
            let mut asked = 0;
            let expired = || {
                asked += 1;
                one_read_per_step && asked > 1
            };
            let step = if by_search { h.resolve_by_search(expired) } else { h.resolve(expired) };
            match step {
                Step::Unfinished => continue,
                Step::Decided(r) => {
                    let (reads, bytes) = h.resolution_reads();
                    return (r, steps, reads, bytes);
                }
                Step::Failed => panic!("failed"),
            }
        }
    }

    /// Every place, `how`, mode and measure of `rust` against the Python resolver's JSON.
    fn same_as_python(rust: &Result<Resolved, Failure>, py: &serde_json::Value) {
        let r = match (rust, py.get("failed")) {
            (Err(f), Some(pf)) => {
                assert_eq!((f.anchor.as_str(), f.rule.to_string().as_str(), f.detail.as_str()), (
                    pf["anchor"].as_str().unwrap(),
                    pf["rule"].as_str().unwrap(),
                    pf["detail"].as_str().unwrap()
                ));
                return;
            }
            (Ok(r), None) => r,
            (rust, py) => panic!("rust {rust:?}, python {py:?}"),
        };
        let hx = |v: Option<u64>| v.map(|v| format!("{v:#x}"));
        assert_eq!(py["mode"], r.mode.as_str());
        assert_eq!(py["image_size"], format!("{:#x}", r.image_size));
        assert_eq!(py["tls_index_rva"], format!("{:#x}", r.tls_index));
        assert_eq!(py["rdata"], serde_json::json!([format!("{:#x}", r.rdata.0), format!("{:#x}", r.rdata.1)]));
        let pa = py["anchors"].as_array().unwrap();
        assert_eq!(pa.len(), r.anchors.len());
        for (p, a) in pa.iter().zip(&r.anchors) {
            assert_eq!(p["name"], a.name);
            assert_eq!(p["rva"].as_str().map(String::from), hx(a.rva), "{}", a.name);
            assert_eq!(p["how"].as_str(), a.how.as_deref(), "{}", a.name);
        }
        let pv = py["vtables"].as_array().unwrap();
        assert_eq!(pv.len(), r.vtables.len());
        for (p, v) in pv.iter().zip(&r.vtables) {
            assert_eq!(p["name"], v.name);
            assert_eq!(p["rva"].as_str().map(String::from), hx(v.rva), "vtable {}", v.name);
            assert_eq!(p["how"].as_str(), v.how.as_deref(), "vtable {}", v.name);
        }
        match (&r.search, py.get("search")) {
            (None, None) => {}
            (Some(s), Some(ps)) => {
                assert_eq!(ps["bytes"], s.bytes);
                assert_eq!(ps["hash_hits"], s.hash_hits);
                assert_eq!(ps["false_candidates"], s.false_candidates);
            }
            other => panic!("search: {other:?}"),
        }
    }

    fn summary(r: &Result<Resolved, Failure>) -> String {
        match r {
            Err(f) => f.to_string(),
            Ok(r) => format!(
                "resolved ({}): {} anchors ({} located), {} vtables, {} moved; image size {:#x}, TLS index {:#x}, .rdata {:#x}..{:#x}{}",
                r.mode.as_str(),
                r.anchors.len(),
                r.anchors.iter().filter(|a| a.rva.is_some()).count(),
                r.vtables.len(),
                r.anchors
                    .iter()
                    .zip(GAME.anchors)
                    .filter(|(p, a)| p.rva.is_some_and(|v| v != u64::from(a.rva)))
                    .count()
                    + r.vtables.iter().zip(GAME.vtables).filter(|(p, v)| p.rva != Some(u64::from(v.rva))).count(),
                r.image_size,
                r.tls_index,
                r.rdata.0,
                r.rdata.1,
                r.search.map_or(String::new(), |s| format!(
                    ", search over {} bytes, {} hash hits, {} false candidates",
                    s.bytes, s.hash_hits, s.false_candidates
                ))
            ),
        }
    }

    fn parity(path: &str, by_search: bool) -> Result<Resolved, Failure> {
        let py = python_resolve(path, by_search);
        let (whole, _, reads, bytes) = real(path, by_search, 65536, false);
        same_as_python(&whole, &py);
        // The same in one read per step.
        let (sliced, steps, ..) = real(path, by_search, 65536, true);
        assert_eq!(sliced, whole);
        println!("{}", summary(&whole));
        println!("   the same as gen_relocation.py --resolve{}: every place, how, mode and measure; {reads} reads of 4 KiB ({bytes} bytes), the same again in {steps} steps of one read", if by_search { " --force-search" } else { "" });
        whole
    }

    #[test]
    #[ignore]
    fn the_copy_of_today_resolves_as_the_python_resolver_does() {
        let path = std::env::var("TC_GW2_EXE").expect("TC_GW2_EXE");
        let identity = parity(&path, false).unwrap();
        assert_eq!(identity.mode, Mode::Identity);
        let search = parity(&path, true).unwrap();
        assert_eq!(search.mode, Mode::Relocated);
        let places = |r: &Resolved| r.anchors.iter().chain(&r.vtables).map(|p| p.rva).collect::<Vec<_>>();
        assert_eq!(places(&search), places(&identity), "by search, the certified places");
    }

    #[test]
    #[ignore]
    fn the_moved_copy_resolves_as_the_python_resolver_does() {
        let path = std::env::var("TC_GW2_MOVED_EXE").expect("TC_GW2_MOVED_EXE");
        let r = parity(&path, false).unwrap();
        assert_eq!(r.mode, Mode::Relocated);
    }

    /// The cost of each phase over the copy, from memory (no disk in it): the scan alone, in
    /// reads of 64 KiB as the hash makes them, and the resolution from a cold start.
    #[test]
    #[ignore]
    fn the_cost_of_the_two_phases_over_the_copy() {
        let path = std::env::var("TC_GW2_EXE").expect("TC_GW2_EXE");
        let data = std::fs::read(&path).unwrap();
        let mut times = Vec::new();
        let mut scanned = (0, 0);
        for _ in 0..5 {
            let mut s = Scanner::new(&GAME);
            let t0 = Instant::now();
            for (i, c) in data.chunks(65536).enumerate() {
                s.feed((i * 65536) as u64, c);
            }
            times.push(t0.elapsed().as_millis());
            scanned = (s.scanned(), s.hash_hits());
        }
        let mut c = crate::certificate::Collector::new(&crate::certificate::GAME);
        let t0 = Instant::now();
        for (i, part) in data.chunks(65536).enumerate() {
            c.feed((i * 65536) as u64, part);
        }
        let collector = t0.elapsed();
        println!(
            "phase 1 (scan inside the pass): {times:?} ms in 5 runs, {} bytes of .text and .rdata, {} hash hits (the certificate's collector alone: {} ms)",
            scanned.0,
            scanned.1,
            collector.as_millis()
        );
        for by_search in [false, true] {
            let (mut h, _) = hashed(Bytes::of(data.clone(), 65536), &crate::certificate::GAME, &GAME);
            let t0 = Instant::now();
            let step = if by_search { h.resolve_by_search(|| false) } else { h.resolve(|| false) };
            let cold = t0.elapsed();
            let t1 = Instant::now();
            let again = if by_search { h.resolve_by_search(|| false) } else { h.resolve(|| false) };
            let warm = t1.elapsed();
            assert_eq!(step, again);
            let Step::Decided(Ok(r)) = step else { panic!("{step:?}") };
            let (reads, bytes) = h.resolution_reads();
            println!(
                "phase 2 ({}): {} ms cold, {} ms over the pages already read; {reads} reads of 4 KiB, {bytes} bytes",
                r.mode.as_str(),
                cold.as_millis(),
                warm.as_millis()
            );
        }
    }
}
