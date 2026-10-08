//! The current-process Win64 adapter for the bounded passive reader.
//!
//! OS metadata calls discover TEBs; exact ReadProcessMemory copies read game fields. No hook,
//! game function, input, write or thread suspension is used. Called only by the background
//! bridge worker after live1 negotiation, at most once per second; render does no memory reads.
//!
//! One cycle reads the owned inventory, then the wallet, then the bag slots and last the Magic
//! Find, from the same verified build and context and before the same deadline. Each of the
//! last three has its own smaller read budget and can only fail on its own: an inventory
//! sample never depends on them, and they do not depend on each other. Magic Find goes last
//! because it is the largest; if the deadline cuts it, only it loses coverage for that cycle.

use std::collections::BTreeSet;
use std::ffi::{c_void, OsString};
use std::fs::File;
use std::io::Read;
use std::os::windows::ffi::OsStringExt;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Mutex, OnceLock};
use std::time::{Duration, Instant};
use tyrian_companion_nexus_core::bags::{self, BagCoverage, BagProfile, BagSlots};
use tyrian_companion_nexus_core::inventory::{
    self, BuildProfile, Diagnostics, InventorySnapshot, Memory, ReadError, Reader,
};
use tyrian_companion_nexus_core::magic_find::{
    self, MagicFind, MagicFindCoverage, MagicFindProfile,
};
use tyrian_companion_nexus_core::passive::Uncovered;
use tyrian_companion_nexus_core::wallet::{
    self, WalletCoverage, WalletError, WalletProfile, WalletSnapshot,
};
use windows::core::{s, w, PCWSTR};
use windows::Win32::Foundation::{CloseHandle, ERROR_NO_MORE_FILES, HANDLE};
use windows::Win32::Security::Cryptography::*;
use windows::Win32::System::Diagnostics::Debug::ReadProcessMemory;
use windows::Win32::System::Diagnostics::ToolHelp::{
    CreateToolhelp32Snapshot, Thread32First, Thread32Next, TH32CS_SNAPTHREAD, THREADENTRY32,
};
use windows::Win32::System::LibraryLoader::{GetModuleFileNameW, GetModuleHandleW, GetProcAddress};
use windows::Win32::System::Threading::{
    GetCurrentProcess, GetCurrentProcessId, OpenThread, THREAD_QUERY_INFORMATION,
};

const MAX_THREADS: usize = 128;
const MAX_SYSTEM_ENTRIES: usize = 4096;
const MAX_EXECUTABLE_BYTES: u64 = 128 * 1024 * 1024;

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
}
impl<'a> ProcessMemory<'a> {
    fn new(stop: &'a AtomicBool) -> Self {
        Self::until(stop, Instant::now() + Duration::from_millis(750))
    }
    /// A second reader of the same cycle shares the cycle's deadline instead of extending it.
    fn until(stop: &'a AtomicBool, deadline: Instant) -> Self {
        Self { deadline, stop }
    }
}
impl Memory for ProcessMemory<'_> {
    fn read_exact(&mut self, address: u64, bytes: &mut [u8]) -> Result<(), ReadError> {
        if self.stop.load(Ordering::Relaxed) || Instant::now() >= self.deadline {
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
static READER: OnceLock<Result<NativeReader, ReadError>> = OnceLock::new();
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
fn publish(d: Diagnostics) {
    *DIAGNOSTICS.lock().unwrap_or_else(|p| p.into_inner()) = d;
}

/// Lazily verify the executable on the worker. Unknown build never reaches inventory offsets.
/// One sample owns its entire read budget and returns no pointers over the bridge.
pub fn sample(stop: &AtomicBool) -> Result<InventorySnapshot, ReadError> {
    if stop.load(Ordering::Relaxed) {
        return Err(ReadError::ReadFailed);
    }
    publish(Diagnostics::default());
    let native = READER
        .get_or_init(|| NativeReader::new(stop))
        .as_ref()
        .map_err(|error| *error)?;
    native.sample(stop)
}
impl NativeReader {
    fn new(stop: &AtomicBool) -> Result<Self, ReadError> {
        // Safety: these calls return OS module metadata; no game function is resolved or called.
        let module =
            unsafe { GetModuleHandleW(PCWSTR::null()) }.map_err(|_| ReadError::UnsupportedBuild)?;
        let mut path = [0u16; 32768];
        let length = unsafe { GetModuleFileNameW(Some(module), &mut path) } as usize;
        if length == 0 || length >= path.len() {
            return Err(ReadError::UnsupportedBuild);
        }
        let digest = executable_hash(
            File::open(OsString::from_wide(&path[..length]))
                .map_err(|_| ReadError::UnsupportedBuild)?,
            stop,
        )?;
        if digest != inventory::BUILD_SHA256 {
            return Err(ReadError::UnsupportedBuild);
        }
        let base = module.0 as u64;
        let mut reader = Reader::new(ProcessMemory::new(stop));
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
        let profile = BuildProfile::checked(&digest, base, image_size)?;
        let ntdll =
            unsafe { GetModuleHandleW(w!("ntdll.dll")) }.map_err(|_| ReadError::ReadFailed)?;
        let address = unsafe { GetProcAddress(ntdll, s!("NtQueryInformationThread")) }
            .ok_or(ReadError::ReadFailed)?;
        // Safety: Windows exports this function with the NTAPI ThreadBasicInformation ABI.
        // We check returned size, process, thread, TEB self pointer on every result.
        let query = unsafe {
            std::mem::transmute::<unsafe extern "system" fn() -> isize, QueryThread>(address)
        };
        Ok(Self {
            profile,
            query,
            wallet: OnceLock::new(),
            bags: OnceLock::new(),
            magic_find: OnceLock::new(),
        })
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
        magic_find::magic_find(reader, profile, context).map(|(value, _owner)| value)
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
    fn sample(&self, stop: &AtomicBool) -> Result<InventorySnapshot, ReadError> {
        let deadline = Instant::now() + Duration::from_millis(750);
        let mut reader = Reader::new(ProcessMemory::until(stop, deadline));
        let mut wallet_reader =
            Reader::bounded(ProcessMemory::until(stop, deadline), wallet::MAX_BYTES);
        let mut bag_reader = Reader::bounded(ProcessMemory::until(stop, deadline), bags::MAX_BYTES);
        let mut magic_find_reader =
            Reader::bounded(ProcessMemory::until(stop, deadline), magic_find::MAX_BYTES);
        let mut coverage = WalletCoverage::NotRead;
        let mut bag_coverage = BagCoverage::NotRead;
        let mut magic_find_coverage = MagicFindCoverage::NotRead;
        let mut own = 0;
        let result = (|| {
            let pid = unsafe { GetCurrentProcessId() };
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
            let mut contexts = BTreeSet::new();
            loop {
                if stop.load(Ordering::Relaxed) || Instant::now() >= deadline {
                    return Err(ReadError::ReadFailed);
                }
                total += 1;
                if total > MAX_SYSTEM_ENTRIES {
                    return Err(ReadError::Bounds);
                }
                if entry.th32OwnerProcessID == pid {
                    own += 1;
                    if own > MAX_THREADS {
                        return Err(ReadError::Bounds);
                    }
                    let thread = OwnedHandle(
                        unsafe { OpenThread(THREAD_QUERY_INFORMATION, false, entry.th32ThreadID) }
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
                        || basic.thread as usize != entry.th32ThreadID as usize
                    {
                        return Err(ReadError::ReadFailed);
                    }
                    let teb = basic.teb as u64;
                    if teb < 0x10000
                        || teb > 0x0000_7fff_ffff_ff00
                        || reader.pointer(teb + 0x30)? != teb
                    {
                        return Err(ReadError::ReadFailed);
                    }
                    if let Some(context) =
                        inventory::context_from_teb(&mut reader, self.profile, teb)?
                    {
                        contexts.insert(context);
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
            let context = contexts
                .into_iter()
                .next()
                .ok_or(ReadError::RootUnavailable)?;
            let mut snapshot = inventory::inventory_snapshot(&mut reader, self.profile, context)?;
            // Only after a whole inventory sample. Whatever happens here, that sample stands:
            // without a wallet it goes out with `currencies:none`.
            match self.wallet(&mut wallet_reader, context) {
                Ok(wallet) => {
                    coverage = WalletCoverage::Listed(wallet.balances().len() as u32);
                    snapshot.wallet = Some(wallet);
                }
                Err(error) => coverage = WalletCoverage::Unavailable(error),
            }
            // Reader output only: these two stay in the local diagnostics. Nothing here changes
            // the sample that goes out, whose `free_slots` remains `None`.
            bag_coverage = match self.bags(&mut bag_reader, context) {
                Ok(slots) => BagCoverage::Read(slots),
                Err(error) => BagCoverage::Unavailable(error),
            };
            magic_find_coverage = match self.magic_find(&mut magic_find_reader, context) {
                Ok(value) => MagicFindCoverage::Read(value),
                Err(error) => MagicFindCoverage::Unavailable(error),
            };
            Ok(snapshot)
        })();
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
            bag_bytes: bag_reader.bytes as u32,
            bag_reads: bag_reader.reads as u32,
            magic_find: magic_find_coverage,
            magic_find_bytes: magic_find_reader.bytes as u32,
            magic_find_reads: magic_find_reader.reads as u32,
        });
        result
    }
}

/// SHA-256 through the OS already linked by `windows`; fixed memory and file-size limits.
/// File size/mtime must remain stable during hashing. Handles close on every error path.
fn executable_hash(mut file: File, stop: &AtomicBool) -> Result<String, ReadError> {
    let before = file.metadata().map_err(|_| ReadError::UnsupportedBuild)?;
    if before.len() == 0 || before.len() > MAX_EXECUTABLE_BYTES {
        return Err(ReadError::UnsupportedBuild);
    }
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
        || unsafe { BCryptCreateHash(handles.algorithm, &mut handles.hash, None, None, 0) }.0 < 0
    {
        return Err(ReadError::UnsupportedBuild);
    }
    let deadline = Instant::now() + Duration::from_secs(10);
    let mut buffer = [0u8; 65536];
    let mut total = 0u64;
    loop {
        if stop.load(Ordering::Relaxed) || Instant::now() >= deadline {
            return Err(ReadError::UnsupportedBuild);
        }
        let n = file
            .read(&mut buffer)
            .map_err(|_| ReadError::UnsupportedBuild)?;
        if n == 0 {
            break;
        }
        total += n as u64;
        if total > MAX_EXECUTABLE_BYTES
            || unsafe { BCryptHashData(handles.hash, &buffer[..n], 0) }.0 < 0
        {
            return Err(ReadError::UnsupportedBuild);
        }
    }
    let after = file.metadata().map_err(|_| ReadError::UnsupportedBuild)?;
    if total != before.len()
        || before.len() != after.len()
        || before.modified().ok() != after.modified().ok()
    {
        return Err(ReadError::UnsupportedBuild);
    }
    let mut digest = [0u8; 32];
    if unsafe { BCryptFinishHash(handles.hash, &mut digest, 0) }.0 < 0 {
        return Err(ReadError::UnsupportedBuild);
    }
    Ok(digest.iter().map(|byte| format!("{byte:02x}")).collect())
}
