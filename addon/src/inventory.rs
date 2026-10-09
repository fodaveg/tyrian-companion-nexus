//! The current-process Win64 adapter for the bounded passive reader.
//!
//! OS metadata calls discover TEBs; exact ReadProcessMemory copies read game fields. No hook,
//! game function, input, write or thread suspension is used. Called only by the background
//! bridge worker after live1 negotiation, at most once per second; render does no memory reads.
//!
//! The game's context is found by walking the process's own threads (a handle, a query and a few
//! copies for each, about 115 in a running game), and that walk is made once every 30 seconds,
//! or at once when the check below fails, or after a cycle the client discarded. The cycles in
//! between check the thread the walk found: the system still has it, in this process, with the
//! same TEB, whose self pointer and client id are the same, and whose TLS route still ends at
//! the same context. That the context is the only one is checked by the walk, not on every cycle.
//!
//! One cycle reads the owned inventory, then the wallet, then the bag slots and last the Magic
//! Find, from the same verified build and context and before the same deadline. Each of the
//! last three has its own smaller read budget and can only fail on its own: the content of an
//! inventory sample never depends on them, and they do not depend on each other.
//!
//! Its timing does. The bag slots and the Magic Find are read before this cycle returns the
//! sample, so before the client reads the game context again and seals it: the time they take
//! widens the window in which a context change discards that inventory copy. They therefore
//! share a shorter deadline of their own, [`EXTRAS_TIME`] from the moment they start and never
//! past the cycle's. Magic Find goes last because it is the largest: a cut costs it, and only
//! it, that cycle's coverage, reported as `Uncovered::Deadline` rather than `ReadFailed`.

use std::collections::BTreeMap;
use std::ffi::{c_void, OsString};
use std::fs::File;
use std::io::Read;
use std::os::windows::ffi::OsStringExt;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Mutex, OnceLock};
use std::time::{Duration, Instant};
use tyrian_companion_nexus_core::bags::{self, BagCoverage, BagProfile, BagSlots};
use tyrian_companion_nexus_core::client::Readiness;
use tyrian_companion_nexus_core::executable::{self, Build, HashedFile, Hashing, Stamp, Step};
use tyrian_companion_nexus_core::inventory::{
    self, BuildProfile, ContextSearch, Diagnostics, InventorySnapshot, Located, Memory, ReadError,
    Reader, MAX_OWN_THREADS, MAX_SYSTEM_THREAD_ENTRIES,
};
use tyrian_companion_nexus_core::magic_find::{
    self, CachedMagicFind, MagicFind, MagicFindCoverage, MagicFindProfile,
};
use tyrian_companion_nexus_core::passive::Uncovered;
use tyrian_companion_nexus_core::perf::{CycleTimes, ReaderCounters};
use tyrian_companion_nexus_core::verdict::{Attempt, Verdict};
use tyrian_companion_nexus_core::wallet::{
    self, WalletCoverage, WalletError, WalletProfile, WalletSnapshot,
};
use windows::core::{s, w, PCWSTR};
use windows::Win32::Foundation::{CloseHandle, ERROR_INVALID_PARAMETER, ERROR_NO_MORE_FILES, HANDLE, HMODULE};
use windows::Win32::Security::Cryptography::*;
use windows::Win32::System::Diagnostics::Debug::ReadProcessMemory;
use windows::Win32::System::Diagnostics::ToolHelp::{
    CreateToolhelp32Snapshot, Thread32First, Thread32Next, TH32CS_SNAPTHREAD, THREADENTRY32,
};
use windows::Win32::System::LibraryLoader::{GetModuleFileNameW, GetModuleHandleW, GetProcAddress};
use windows::Win32::System::Threading::{
    GetCurrentProcess, GetCurrentProcessId, OpenThread, THREAD_QUERY_INFORMATION,
};

/// How long the bag slots and the Magic Find may take together, after the wallet. It bounds
/// what they add to the wait before the client seals the inventory sample. Not measured in a
/// running game: if `Uncovered::Deadline` shows up in the diagnostics, this is the number to
/// revisit.
const EXTRAS_TIME: Duration = Duration::from_millis(250);

struct OwnedHandle(HANDLE);
impl Drop for OwnedHandle {
    fn drop(&mut self) {
        unsafe {
            let _ = CloseHandle(self.0);
        }
    }
}
struct ProcessMemory<'a> {
    deadline: Instant,
    stop: &'a AtomicBool,
    /// Set when a copy is refused because the time ran out. The refusal itself is the same
    /// `ReadFailed` as ever; this only lets the caller name it apart from an unreadable page.
    expired: Option<&'a AtomicBool>,
}
impl<'a> ProcessMemory<'a> {
    fn new(stop: &'a AtomicBool) -> Self {
        Self::until(stop, Instant::now() + Duration::from_millis(750))
    }
    /// A second reader of the same cycle shares the cycle's deadline instead of extending it.
    fn until(stop: &'a AtomicBool, deadline: Instant) -> Self {
        Self {
            deadline,
            stop,
            expired: None,
        }
    }
    /// The same, recording in `expired` whether the deadline is what stopped it.
    fn timed(stop: &'a AtomicBool, deadline: Instant, expired: &'a AtomicBool) -> Self {
        Self {
            deadline,
            stop,
            expired: Some(expired),
        }
    }
}
impl Memory for ProcessMemory<'_> {
    fn read_exact(&mut self, address: u64, bytes: &mut [u8]) -> Result<(), ReadError> {
        if self.stop.load(Ordering::Relaxed) {
            return Err(ReadError::ReadFailed);
        }
        if Instant::now() >= self.deadline {
            if let Some(expired) = self.expired {
                expired.store(true, Ordering::Relaxed);
            }
            return Err(ReadError::ReadFailed);
        }
        let mut copied = 0;
        // Safety: RPM handles inaccessible/concurrently unmapped pages without dereferencing
        // the remote address in Rust. Its destination is our owned slice, of exactly this size.
        unsafe {
            ReadProcessMemory(
                GetCurrentProcess(),
                address as *const c_void,
                bytes.as_mut_ptr().cast(),
                bytes.len(),
                Some(&mut copied),
            )
        }
        .map_err(|_| ReadError::ReadFailed)?;
        if copied != bytes.len() {
            return Err(ReadError::ReadFailed);
        }
        Ok(())
    }
}

/// Native AMD64 ThreadBasicInformation, class 0. Only OS-owned pointers are copied here.
#[repr(C)]
#[derive(Default)]
struct ThreadBasic {
    status: i32,
    teb: *mut c_void,
    process: *mut c_void,
    thread: *mut c_void,
    affinity: usize,
    priority: i32,
    base_priority: i32,
}
type QueryThread = unsafe extern "system" fn(HANDLE, u32, *mut ThreadBasic, u32, *mut u32) -> i32;
const _: () = assert!(std::mem::size_of::<ThreadBasic>() == 48);
const _: () = assert!(std::mem::offset_of!(ThreadBasic, teb) == 8);

struct NativeReader {
    profile: BuildProfile,
    query: QueryThread,
    /// The wallet's static guards, checked once for this verified build: `Some` holds the
    /// proof, `None` records bytes that differ. A failed copy is not recorded and is retried.
    wallet: OnceLock<Option<WalletProfile>>,
    /// The same once-per-build verdict for the bag and Magic Find guards.
    bags: OnceLock<Option<BagProfile>>,
    magic_find: OnceLock<Option<MagicFindProfile>>,
}

/// How long after a verification of the executable that the system failed (the file could not
/// be opened or read, a copy of the image failed) the next one is tried. Not on every cycle: a
/// failure of that kind does not go away in a second.
const VERIFY_RETRY: Duration = Duration::from_secs(30);

/// The system's SHA-256: its provider and one hash object over it. Closed on every path.
struct HashHandles {
    algorithm: BCRYPT_ALG_HANDLE,
    hash: BCRYPT_HASH_HANDLE,
}
impl Drop for HashHandles {
    fn drop(&mut self) {
        unsafe {
            if !self.hash.0.is_null() {
                let _ = BCryptDestroyHash(self.hash);
            }
            if !self.algorithm.0.is_null() {
                let _ = BCryptCloseAlgorithmProvider(self.algorithm, 0);
            }
        }
    }
}
// Safety: the two CNG handles are provider objects of the process, not of the thread that
// made them. They are only used under `CHECK`'s lock, by one thread at a time, and that is
// the bridge worker except for the release on unload, which runs after the worker has ended.
unsafe impl Send for HashHandles {}

/// The executable's own file on disk and the hash over it, kept from one cycle to the next
/// while the hash is under way. Nothing of the running game is read here.
struct Executable {
    file: File,
    handles: HashHandles,
}
impl Executable {
    /// `None` when the system fails to name the file, open it or give a hash object.
    fn open(module: HMODULE) -> Option<Self> {
        let mut path = [0u16; 32768];
        // Safety: OS module metadata, copied into a buffer of this size.
        let length = unsafe { GetModuleFileNameW(Some(module), &mut path) } as usize;
        if length == 0 || length >= path.len() {
            return None;
        }
        let file = File::open(OsString::from_wide(&path[..length])).ok()?;
        let mut handles = HashHandles {
            algorithm: BCRYPT_ALG_HANDLE::default(),
            hash: BCRYPT_HASH_HANDLE::default(),
        };
        if unsafe {
            BCryptOpenAlgorithmProvider(
                &mut handles.algorithm,
                BCRYPT_SHA256_ALGORITHM,
                PCWSTR::null(),
                BCRYPT_OPEN_ALGORITHM_PROVIDER_FLAGS(0),
            )
        }
        .0 < 0
            || unsafe { BCryptCreateHash(handles.algorithm, &mut handles.hash, None, None, 0) }.0
                < 0
        {
            return None;
        }
        Some(Self { file, handles })
    }
}
impl HashedFile for Executable {
    fn stamp(&mut self) -> Option<Stamp> {
        let metadata = self.file.metadata().ok()?;
        Some(Stamp {
            len: metadata.len(),
            modified: metadata.modified().ok(),
        })
    }
    fn read(&mut self, buffer: &mut [u8]) -> Option<usize> {
        self.file.read(buffer).ok()
    }
    fn update(&mut self, bytes: &[u8]) -> bool {
        unsafe { BCryptHashData(self.handles.hash, bytes, 0) }.0 >= 0
    }
    fn finish(&mut self) -> Option<[u8; 32]> {
        let mut digest = [0u8; 32];
        (unsafe { BCryptFinishHash(self.handles.hash, &mut digest, 0) }.0 >= 0).then_some(digest)
    }
}

/// How far the verification of the executable has got. The hash is done a slice on each pass
/// of the bridge worker (`executable::SLICE`), so the file and the hash object wait here in
/// between; once the file is known to be the certified build it is not hashed again, whatever
/// fails after that.
enum Check {
    Unstarted,
    Hashing(Hashing<Executable>),
    Certified,
}
static CHECK: Mutex<Check> = Mutex::new(Check::Unstarted);

/// What the Magic Find passes keep between cycles: the content of the applied buffs, read whole
/// at most every `magic_find::MAX_CONTENT_AGE` and verified by its headers on the passes
/// between. It stands only for the build, context and character it was read under (the pass
/// checks that itself), is emptied whenever the client sees the context change ([`discard_cycle`]), and
/// goes with the load.
static MAGIC_FIND: CachedMagicFind = CachedMagicFind::new();

/// The game context changed, under a cycle that is thrown away or between cycles (the client
/// calls this whenever it sees one): the Magic Find content that was kept is emptied and the
/// thread the context was found on is forgotten, so the next cycle reads and finds everything
/// again, the uniqueness of the context included. Also what unload does.
pub fn discard_cycle() {
    MAGIC_FIND.discard();
    SEARCH.lock().unwrap_or_else(|p| p.into_inner()).forget();
}

/// The thread and the context the last full search found, and when. The cycles in between
/// verify them instead of walking every thread of the process; the search is made again after
/// `inventory::CONTEXT_SEARCH_EVERY`, as soon as a check fails (in that same cycle), and after
/// [`discard_cycle`], which forgets it.
static SEARCH: Mutex<ContextSearch> = Mutex::new(ContextSearch::new());

/// Closes the executable's file and the hash object if a hash was left half done, and
/// discards what the readers kept ([`discard_cycle`]). For the unload, once the worker has
/// ended: a static is not dropped when the DLL goes.
pub fn release_executable() {
    discard_cycle();
    let mut check = CHECK.lock().unwrap_or_else(|p| p.into_inner());
    if matches!(*check, Check::Hashing(_)) {
        *check = Check::Unstarted;
    }
}

/// Why there is no verdict on the executable yet. The two are not the same thing to the
/// plugin, and are kept apart all the way.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Unverified {
    /// The hash is still under way: nothing has failed and nothing is known. The source is not
    /// ready to sample (`prepare`), and says nothing until the hash is overdue.
    Pending,
    /// The system failed to open or read the file, or to copy the image: a reading that
    /// failed, reported as one and tried again [`VERIFY_RETRY`] later.
    Failed,
}

/// The verified build's reader, or `None` for an executable that is another build by what the
/// file or its loaded image says: the two final answers, kept for the whole load. A
/// verification the system failed is not kept: it is tried again [`VERIFY_RETRY`] later.
static READER: Verdict<Option<NativeReader>, Unverified> = Verdict::new(VERIFY_RETRY);

/// One step of verifying the executable: at most `executable::SLICE` of hashing, and once the
/// file is the certified build's, the check of its loaded image.
///
/// `interrupted` cuts the slice of hashing short: the worker told to stop, or the game closing.
fn verify(stop: &AtomicBool, interrupted: &dyn Fn() -> bool) -> Attempt<Option<NativeReader>, Unverified> {
    // Safety: OS module metadata; no game function is resolved or called.
    let Ok(module) = (unsafe { GetModuleHandleW(PCWSTR::null()) }) else {
        return Attempt::Failed(Unverified::Failed);
    };
    let mut check = CHECK.lock().unwrap_or_else(|p| p.into_inner());
    if matches!(*check, Check::Unstarted) {
        let Some(file) = Executable::open(module) else {
            return Attempt::Failed(Unverified::Failed);
        };
        match Hashing::start(file) {
            Ok(hashing) => *check = Check::Hashing(hashing),
            // Its size alone: not the certified build, and no byte of it was read.
            Err(Step::Decided(_)) => return Attempt::Settled(None),
            Err(_) => return Attempt::Failed(Unverified::Failed),
        }
    }
    if let Check::Hashing(hashing) = &mut *check {
        let until = Instant::now() + executable::SLICE;
        let step = hashing.advance(|| interrupted() || Instant::now() >= until);
        match step {
            Step::Unfinished => return Attempt::Unfinished(Unverified::Pending),
            Step::Decided(Build::Certified) => *check = Check::Certified,
            Step::Decided(Build::Other) => {
                *check = Check::Unstarted;
                return Attempt::Settled(None);
            }
            Step::Failed => {
                *check = Check::Unstarted;
                return Attempt::Failed(Unverified::Failed);
            }
        }
    }
    match NativeReader::of_image(module.0 as u64, stop) {
        Step::Decided(reader) => Attempt::Settled(reader),
        _ => Attempt::Failed(Unverified::Failed),
    }
}

/// Does one slice of the verification of the executable, if it is not over, and says whether
/// the source can be asked for a sample: not while the hash is still under way. The bridge
/// worker asks this before every sample and takes none while it is not ready, so a verdict that
/// is pending is never read past and never reaches the plugin as a sample that was tried. Once
/// there is a verdict, or the system has failed, [`sample`] says which.
///
/// A hash that has taken `executable::OVERDUE` of slices and is not over is `Overdue` instead
/// of `Pending`: the worker then tells the plugin that the source could not complete a reading,
/// where it used to say nothing for as long as a slow disk took. That is all that changes. The
/// hash is where it was, the next call does the next slice of it, and the call on which it
/// ends is `Ready`. The time is the `Verdict`'s, counted over the slices of one hash: a hash
/// that the system fails is started again [`VERIFY_RETRY`] later, with its own.
///
/// `interrupted` is the worker's: it cuts the slice short when it was told to stop or the game
/// is closing, so that neither waits for the hash.
pub fn prepare(stop: &AtomicBool, interrupted: &dyn Fn() -> bool) -> Readiness {
    match READER.get_or_try(Instant::now, || verify(stop, interrupted)).err() {
        Some(Unverified::Pending) if executable::overdue(READER.unfinished_for()) => Readiness::Overdue,
        Some(Unverified::Pending) => Readiness::Pending,
        _ => Readiness::Ready,
    }
}
static DIAGNOSTICS: Mutex<Diagnostics> = Mutex::new(Diagnostics {
    threads: 0,
    bytes: 0,
    reads: 0,
    positions: 0,
    owner_verified: false,
    wallet: WalletCoverage::NotRead,
    wallet_bytes: 0,
    wallet_reads: 0,
    bags: BagCoverage::NotRead,
    bag_bytes: 0,
    bag_reads: 0,
    magic_find: MagicFindCoverage::NotRead,
    magic_find_bytes: 0,
    magic_find_reads: 0,
});
pub fn diagnostics() -> Diagnostics {
    *DIAGNOSTICS.lock().unwrap_or_else(|p| p.into_inner())
}
/// What the cycles have taken and how they have ended since the addon loaded, for the reader
/// diagnostics of the Options window. Local: none of it goes to the plugin.
static COUNTERS: Mutex<ReaderCounters> = Mutex::new(ReaderCounters::new());
pub fn counters() -> ReaderCounters {
    *COUNTERS.lock().unwrap_or_else(|p| p.into_inner())
}
fn publish(d: Diagnostics) {
    *DIAGNOSTICS.lock().unwrap_or_else(|p| p.into_inner()) = d;
}

/// Lazily verify the executable on the worker. Unknown build never reaches inventory offsets.
/// One sample owns its entire read budget and returns no pointers over the bridge.
///
/// The verification takes several passes of the worker, a slice of the hash on each, so that
/// this thread, which also keeps the connection alive, is never held by it. The worker asks
/// [`prepare`] first and does not come here while the verdict is pending. A verification the
/// system failed comes back as `ReadFailed`, not as `UnsupportedBuild`: the build is not known
/// to be another, and `UnsupportedBuild` stops the source until the game context changes. Only
/// a final answer is kept for the load (`verify`).
pub fn sample(stop: &AtomicBool) -> Result<InventorySnapshot, ReadError> {
    if stop.load(Ordering::Relaxed) {
        return Err(ReadError::ReadFailed);
    }
    publish(Diagnostics::default());
    // `Pending` is only here for a caller that did not ask `prepare`: with no sample to give,
    // the one answer left in `ReadError` that does not stop the source.
    let native = READER
        .get_or_try(Instant::now, || verify(stop, &|| stop.load(Ordering::Relaxed)))
        .map_err(|_: Unverified| ReadError::ReadFailed)?;
    native
        .as_ref()
        .ok_or(ReadError::UnsupportedBuild)?
        .sample(stop)
}
impl NativeReader {
    /// The reader for the image loaded at `base`, of an executable whose file is the certified
    /// build's: `None` when the image is not what that build loads as, which is final.
    fn of_image(base: u64, stop: &AtomicBool) -> Step<Option<Self>> {
        let profile =
            match executable::image_profile(&mut Reader::new(ProcessMemory::new(stop)), base) {
                Step::Decided(Some(profile)) => profile,
                Step::Decided(None) => return Step::Decided(None),
                _ => return Step::Failed,
            };
        // Safety: OS module metadata; no game function is resolved or called.
        let Ok(ntdll) = (unsafe { GetModuleHandleW(w!("ntdll.dll")) }) else {
            return Step::Failed;
        };
        let Some(address) = (unsafe { GetProcAddress(ntdll, s!("NtQueryInformationThread")) })
        else {
            return Step::Failed;
        };
        // Safety: Windows exports this function with the NTAPI ThreadBasicInformation ABI.
        // We check returned size, process, thread, TEB self pointer on every result.
        let query = unsafe {
            std::mem::transmute::<unsafe extern "system" fn() -> isize, QueryThread>(address)
        };
        Step::Decided(Some(Self {
            profile,
            query,
            wallet: OnceLock::new(),
            bags: OnceLock::new(),
            magic_find: OnceLock::new(),
        }))
    }
    /// The bag slots of the context the inventory was just read from, guards verified once.
    fn bags<M: Memory>(
        &self,
        reader: &mut Reader<M>,
        context: u64,
    ) -> Result<BagSlots, Uncovered> {
        let verified = match self.bags.get() {
            Some(verified) => verified,
            None => match BagProfile::verified(reader, self.profile) {
                Ok(profile) => self.bags.get_or_init(|| Some(profile)),
                Err(Uncovered::Guard) => self.bags.get_or_init(|| None),
                Err(error) => return Err(error),
            },
        };
        let profile = verified.as_ref().ok_or(Uncovered::Guard)?;
        bags::bag_slots(reader, profile, context).map(|(slots, _owner)| slots)
    }
    /// The Magic Find of the same context, guards verified once.
    fn magic_find<M: Memory>(
        &self,
        reader: &mut Reader<M>,
        context: u64,
    ) -> Result<MagicFind, Uncovered> {
        let verified = match self.magic_find.get() {
            Some(verified) => verified,
            None => match MagicFindProfile::verified(reader, self.profile) {
                Ok(profile) => self.magic_find.get_or_init(|| Some(profile)),
                Err(Uncovered::Guard) => self.magic_find.get_or_init(|| None),
                Err(error) => return Err(error),
            },
        };
        let profile = verified.as_ref().ok_or(Uncovered::Guard)?;
        MAGIC_FIND.read(reader, profile, context).map(|(value, _owner)| value)
    }
    /// The wallet of the context the inventory was just read from. Its guards are verified on
    /// the first cycle that reaches this point; every later cycle reuses that verdict.
    fn wallet<M: Memory>(
        &self,
        reader: &mut Reader<M>,
        context: u64,
    ) -> Result<WalletSnapshot, WalletError> {
        let verified = match self.wallet.get() {
            Some(verified) => verified,
            None => match WalletProfile::verified(reader, self.profile) {
                Ok(profile) => self.wallet.get_or_init(|| Some(profile)),
                Err(WalletError::Guard) => self.wallet.get_or_init(|| None),
                Err(error) => return Err(error),
            },
        };
        let profile = verified.as_ref().ok_or(WalletError::Guard)?;
        wallet::wallet_snapshot(reader, profile, context)
    }
    /// Asks the system for the TEB of thread `id` of this process; an error if the thread is
    /// gone, is not ours or its TEB is not one. One handle, closed on return.
    fn teb_of_thread(&self, pid: u32, id: u32) -> Result<u64, ReadError> {
        let thread = OwnedHandle(
            unsafe { OpenThread(THREAD_QUERY_INFORMATION, false, id) }
                .map_err(|_| ReadError::ReadFailed)?,
        );
        let mut basic = ThreadBasic::default();
        let mut size = 0;
        let status = unsafe {
            (self.query)(
                thread.0,
                0,
                &mut basic,
                std::mem::size_of::<ThreadBasic>() as u32,
                &mut size,
            )
        };
        if status < 0
            || size as usize != std::mem::size_of::<ThreadBasic>()
            || basic.process as usize != pid as usize
            || basic.thread as usize != id as usize
        {
            return Err(ReadError::ReadFailed);
        }
        let teb = basic.teb as u64;
        if teb < 0x10000 || teb > 0x0000_7fff_ffff_ff00 {
            return Err(ReadError::ReadFailed);
        }
        Ok(teb)
    }
    /// The context the TLS route of thread `id` leads to, with that thread's TEB.
    fn context_of_thread(
        &self,
        reader: &mut Reader<ProcessMemory<'_>>,
        pid: u32,
        id: u32,
    ) -> Result<Option<(u64, u64)>, ReadError> {
        let teb = self.teb_of_thread(pid, id)?;
        if reader.pointer(teb + 0x30)? != teb {
            return Err(ReadError::ReadFailed);
        }
        Ok(inventory::context_from_teb(reader, self.profile, teb)?.map(|context| (context, teb)))
    }
    /// Whether the system no longer has thread `id`: `OpenThread` says the id is not valid
    /// (`ERROR_INVALID_PARAMETER`). A thread that is there but cannot be opened (access denied)
    /// is not gone. The handle of a successful open is closed on return.
    fn thread_is_gone(id: u32) -> bool {
        match unsafe { OpenThread(THREAD_QUERY_INFORMATION, false, id) } {
            Ok(handle) => {
                drop(OwnedHandle(handle));
                false
            }
            Err(error) => {
                error.code() == windows::core::HRESULT::from_win32(ERROR_INVALID_PARAMETER.0)
            }
        }
    }
    /// Whether the thread and the context a full search found are still there: the system
    /// still has that thread, in this process, with that TEB, and the TEB and its TLS route
    /// are what they were (`inventory::verify_located`). Anything else, including a copy or a
    /// call that fails, is `false`, and the caller searches all the threads in this same
    /// cycle; nothing is read with a thread that was not just checked.
    fn still_there(&self, reader: &mut Reader<ProcessMemory<'_>>, pid: u32, located: &Located) -> bool {
        let system_teb = self.teb_of_thread(pid, located.thread);
        inventory::still_located(reader, self.profile, pid, located, system_teb)
    }
    /// The full search: every thread of the process, its TEB asked of the system and checked,
    /// its TLS route followed. A single context among them, as it was on every cycle before
    /// the thread was kept; that it is single is now checked
    /// here: every [`inventory::CONTEXT_SEARCH_EVERY`], when a check fails and after any change
    /// of game context, and not on every cycle. (The owner accepted the search every 30 s or
    /// on a failed check; the rest follows from it.) Counts the process's own threads in `own`.
    fn search_threads(
        &self,
        reader: &mut Reader<ProcessMemory<'_>>,
        stop: &AtomicBool,
        deadline: Instant,
        pid: u32,
        own: &mut usize,
    ) -> Result<Located, ReadError> {
        let snapshot = OwnedHandle(
            unsafe { CreateToolhelp32Snapshot(TH32CS_SNAPTHREAD, 0) }
                .map_err(|_| ReadError::ReadFailed)?,
        );
        let mut entry = THREADENTRY32 {
            dwSize: std::mem::size_of::<THREADENTRY32>() as u32,
            ..Default::default()
        };
        unsafe { Thread32First(snapshot.0, &mut entry) }.map_err(|_| ReadError::ReadFailed)?;
        let mut total = 0;
        // Context -> the first thread whose route led to it, and that thread's TEB.
        let mut contexts: BTreeMap<u64, (u32, u64)> = BTreeMap::new();
        loop {
            if stop.load(Ordering::Relaxed) || Instant::now() >= deadline {
                return Err(ReadError::ReadFailed);
            }
            total += 1;
            if total > MAX_SYSTEM_THREAD_ENTRIES {
                return Err(ReadError::Bounds);
            }
            if entry.th32OwnerProcessID == pid {
                *own += 1;
                if *own > MAX_OWN_THREADS {
                    return Err(ReadError::Bounds);
                }
                match self.context_of_thread(reader, pid, entry.th32ThreadID) {
                    Ok(Some(context)) => {
                        let teb = context.1;
                        contexts.entry(context.0).or_insert((entry.th32ThreadID, teb));
                    }
                    Ok(None) => {}
                    // A thread that ended after the snapshot was taken is not a failed
                    // capture: it holds no context any more. Only that is skipped, and only
                    // when the system confirms the thread is gone; any other failure still is.
                    Err(ReadError::ReadFailed) if Self::thread_is_gone(entry.th32ThreadID) => {
                        *own -= 1;
                    }
                    Err(error) => return Err(error),
                }
                if contexts.len() > 1 {
                    return Err(ReadError::RootUnavailable);
                }
            }
            if let Err(error) = unsafe { Thread32Next(snapshot.0, &mut entry) } {
                if error.code() != windows::core::HRESULT::from_win32(ERROR_NO_MORE_FILES.0) {
                    return Err(ReadError::ReadFailed);
                }
                break;
            }
        }
        let (context, (thread, teb)) = contexts
            .into_iter()
            .next()
            .ok_or(ReadError::RootUnavailable)?;
        Ok(Located { thread, teb, context, threads: *own as u32 })
    }
    fn sample(&self, stop: &AtomicBool) -> Result<InventorySnapshot, ReadError> {
        // The cycle's start, read once: its deadline counts from it, and so do the times taken
        // below for the reader diagnostics. Those are the clock read around the passes this
        // cycle runs anyway: no read of the game, no pass and no guard is added or moved.
        let started = Instant::now();
        let deadline = started + Duration::from_millis(750);
        let mut took = CycleTimes::default();
        let mut reader = Reader::new(ProcessMemory::until(stop, deadline));
        let mut wallet_reader =
            Reader::bounded(ProcessMemory::until(stop, deadline), wallet::MAX_BYTES);
        // Built when their turn comes, so their deadline counts from then.
        let mut bag_reader = None;
        let mut magic_find_reader = None;
        let (bags_expired, magic_find_expired) = (AtomicBool::new(false), AtomicBool::new(false));
        let mut coverage = WalletCoverage::NotRead;
        let mut bag_coverage = BagCoverage::NotRead;
        let mut magic_find_coverage = MagicFindCoverage::NotRead;
        let mut own = 0;
        let result = (|| {
            let pid = unsafe { GetCurrentProcessId() };
            // The thread and the context the last full search found stand for 30 seconds, and
            // the cycles in between check them: the system still has that thread with that
            // TEB, and the TEB and the TLS route are what they were. A check that fails, or a
            // copy or call that fails in it, is a full search in this same cycle.
            // The decision is `ContextSearch::locate`'s, in core, where it is tested; this only
            // supplies the two operations that ask the system.
            let located = SEARCH.lock().unwrap_or_else(|p| p.into_inner()).locate(
                started,
                &mut reader,
                |reader, located| self.still_there(reader, pid, located),
                |reader| self.search_threads(reader, stop, deadline, pid, &mut own),
            )?;
            if own == 0 {
                // A cycle that only verified did not count the threads: the walk's count stands.
                own = located.threads as usize;
            }
            let found = Instant::now();
            took.threads = Some(found.saturating_duration_since(started));
            let context = located.context;
            let read = inventory::inventory_snapshot(&mut reader, self.profile, context);
            let after_inventory = Instant::now();
            took.inventory = Some(after_inventory.saturating_duration_since(found));
            let mut snapshot = read?;
            // Only after a whole inventory sample. Whatever happens here, that sample stands:
            // without a wallet it goes out with `currencies:none`.
            match self.wallet(&mut wallet_reader, context) {
                Ok(wallet) => {
                    coverage = WalletCoverage::Listed(wallet.balances().len() as u32);
                    snapshot.wallet = Some(wallet);
                }
                Err(error) => coverage = WalletCoverage::Unavailable(error),
            }
            // The one reading of the clock there was here: the wallet's time ends at it and the
            // two extras' deadline counts from it, as it did.
            let after_wallet = Instant::now();
            took.wallet = Some(after_wallet.saturating_duration_since(after_inventory));
            // These two stay in the local diagnostics, where the panel and Options read them.
            // Nothing here changes the sample that goes out, whose `free_slots` remains `None`.
            let extras = deadline.min(after_wallet + EXTRAS_TIME);
            // A copy refused by the clock comes back as `ReadFailed`; name it for what it was.
            let named = |error: Uncovered, expired: &AtomicBool| match error {
                Uncovered::ReadFailed if expired.load(Ordering::Relaxed) => Uncovered::Deadline,
                other => other,
            };
            let reader = bag_reader.insert(Reader::bounded(
                ProcessMemory::timed(stop, extras, &bags_expired),
                bags::MAX_BYTES,
            ));
            bag_coverage = match self.bags(reader, context) {
                Ok(slots) => BagCoverage::Read(slots),
                Err(error) => BagCoverage::Unavailable(named(error, &bags_expired)),
            };
            let after_bags = Instant::now();
            took.bags = Some(after_bags.saturating_duration_since(after_wallet));
            let reader = magic_find_reader.insert(Reader::bounded(
                ProcessMemory::timed(stop, extras, &magic_find_expired),
                magic_find::MAX_BYTES,
            ));
            magic_find_coverage = match self.magic_find(reader, context) {
                Ok(value) => MagicFindCoverage::Read(value),
                Err(error) => MagicFindCoverage::Unavailable(named(error, &magic_find_expired)),
            };
            took.magic_find = Some(after_bags.elapsed());
            Ok(snapshot)
        })();
        // For Options only: what the cycle and each of its passes took, how it ended, and how
        // many threads of its own the process had. A cycle that ended while still looking for
        // the game's context spent all of its time there. `ReadFailed` after the deadline is a
        // copy the clock refused, which the count keeps apart from one that failed.
        let ended = Instant::now();
        took.cycle = ended.saturating_duration_since(started);
        took.threads = took.threads.or(Some(took.cycle));
        COUNTERS
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .record(took, result.as_ref().err().copied(), ended >= deadline, own as u32);
        publish(Diagnostics {
            threads: own as u32,
            bytes: reader.bytes as u32,
            reads: reader.reads as u32,
            positions: result.as_ref().map(|s| s.positions).unwrap_or(0),
            owner_verified: result.is_ok(),
            wallet: coverage,
            wallet_bytes: wallet_reader.bytes as u32,
            wallet_reads: wallet_reader.reads as u32,
            bags: bag_coverage,
            bag_bytes: bag_reader.as_ref().map_or(0, |r| r.bytes as u32),
            bag_reads: bag_reader.as_ref().map_or(0, |r| r.reads as u32),
            magic_find: magic_find_coverage,
            magic_find_bytes: magic_find_reader.as_ref().map_or(0, |r| r.bytes as u32),
            magic_find_reads: magic_find_reader.as_ref().map_or(0, |r| r.reads as u32),
        });
        result
    }
}
