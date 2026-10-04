//! Opens Obsidian, or Hebra (the player's choice, [`LaunchApp`]), when this addon's first
//! connection attempt to the plugin's bridge finds nobody listening. `core::client::run`
//! (`tyrian_companion_nexus_core::obsidian_launch`) decides *whether* to call [`open_app`]; this
//! module is only the *how*, and only for Windows — Nexus never loads this crate anywhere else.
//!
//! Two paths, matching `docs/audit/sonda-h18-27-abrir-obsidian-desde-proton.md` in the
//! `tyrian-companion` repo (David, 24 sep 2026: "si empiezo a jugar y está cerrado, que se
//! abra"):
//!
//! - **Under Wine/Proton** (detected by [`running_under_wine`]): launch
//!   `%SystemRoot%\system32\winebrowser.exe` with the app's URI (`obsidian://open`,
//!   `hebra://open`) as its one argument. winebrowser hands that off to the host's `xdg-open`,
//!   which is what actually opens or focuses the app (the Fedora Obsidian flatpak; Hebra's
//!   `x-scheme-handler/hebra`). Verified for Obsidian in H18.27, path B there — no registry key
//!   is written, unlike path A, which the sonda also verified but which needs a key nothing on
//!   this addon's own install path would ever write.
//! - **Native Windows** (no Wine): only if `HKEY_CLASSES_ROOT\<scheme>` exists (`obsidian`,
//!   `hebra`) — the app's own installer registers it — `ShellExecuteW` with the URI the normal
//!   way. If the key is missing, nothing is launched: otherwise Windows would pop its own "how
//!   do you want to open this?" dialog on top of the game. **Not verified**: this repository has
//!   no real Windows machine to test it on (see the README's "Automatic app launch" section).
//!
//! Neither path waits for the child process: `CreateProcessW`/`ShellExecuteW` both return as
//! soon as the OS has accepted the launch request, so [`open_app`] never blocks the game's
//! thread.

use tyrian_companion_nexus_core::obsidian_launch::{LaunchApp, ObsidianLaunchOutcome};
use windows::core::{s, w, PCWSTR, PWSTR};
use windows::Win32::Foundation::{CloseHandle, ERROR_SUCCESS, GetLastError};
use windows::Win32::System::LibraryLoader::{GetModuleHandleW, GetProcAddress};
use windows::Win32::System::Registry::{RegCloseKey, RegOpenKeyExW, HKEY, HKEY_CLASSES_ROOT, KEY_READ};
use windows::Win32::System::SystemInformation::GetSystemDirectoryW;
use windows::Win32::System::Threading::{CreateProcessW, PROCESS_CREATION_FLAGS, PROCESS_INFORMATION, STARTUPINFOW};
use windows::Win32::UI::Shell::ShellExecuteW;
use windows::Win32::UI::WindowsAndMessaging::SW_SHOWNORMAL;

/// Tries to open or focus `app` (Obsidian or Hebra, the player's choice), once, the way
/// `core::client::run` decides to when this addon's very first connection attempt to the plugin
/// finds nobody listening. Both apps go through the same two paths below, differing only in the
/// URI ([`LaunchApp::uri`]) and the registry key ([`LaunchApp::registry_key`]).
pub fn open_app(app: LaunchApp) -> ObsidianLaunchOutcome {
    if running_under_wine() {
        launch_via_winebrowser(app)
    } else {
        launch_native(app)
    }
}

/// `true` if this process is running under Wine/Proton. A real Windows `ntdll.dll` never
/// exports `wine_get_version`; Wine's always does, in every version this addon has to support
/// (it is how Wine identifies itself to software that asks, and Wine has carried it for as long
/// as the project has existed). No prefix access, no registry read: just the loader's own,
/// already-mapped export table.
fn running_under_wine() -> bool {
    // Safety: `ntdll.dll` is always already mapped into a Windows process, so `GetModuleHandleW`
    // only looks it up in the loader's module list — it loads nothing new. `GetProcAddress` only
    // walks that already-mapped module's export table; neither call can fail in a way that
    // leaves anything half-initialized.
    unsafe {
        let Ok(ntdll) = GetModuleHandleW(w!("ntdll.dll")) else { return false };
        GetProcAddress(ntdll, s!("wine_get_version")).is_some()
    }
}

/// Path B from H18.27 §4.4: `winebrowser.exe`, which every Wine/Proton prefix already carries,
/// forwards its one argument to the host's `xdg-open` without this addon ever touching the
/// prefix's registry.
fn launch_via_winebrowser(app: LaunchApp) -> ObsidianLaunchOutcome {
    match winebrowser_path() {
        Some(path) => spawn_process(&path, app.uri()),
        // Safety: `GetLastError` only reads thread-local state `GetSystemDirectoryW` just set;
        // no precondition beyond having just called a Win32 function on this thread.
        None => ObsidianLaunchOutcome::Error(unsafe { GetLastError() }.0),
    }
}

/// `%SystemRoot%\system32\winebrowser.exe`, read from `GetSystemDirectoryW` rather than
/// hardcoded, since that is the API both Windows and Wine implement for exactly this purpose.
/// `None` only if the call itself fails (an oversized result does not happen in practice: Win32
/// paths are bounded well under this buffer).
fn winebrowser_path() -> Option<String> {
    let mut buffer = [0u16; 261];
    // Safety: `buffer` outlives the call and its length is passed to the API through the slice
    // itself, so `GetSystemDirectoryW` cannot write past it.
    let written = unsafe { GetSystemDirectoryW(Some(&mut buffer)) };
    if written == 0 || written as usize >= buffer.len() {
        return None;
    }
    let dir = String::from_utf16_lossy(&buffer[..written as usize]);
    Some(format!("{dir}\\winebrowser.exe"))
}

/// Native Windows path from H18.27's "Límite explícito": only if the app's installer has
/// registered its URI scheme in `HKEY_CLASSES_ROOT` (`obsidian`, `hebra`). **Not verified** on a
/// real Windows machine.
fn launch_native(app: LaunchApp) -> ObsidianLaunchOutcome {
    if !has_handler(app) {
        return ObsidianLaunchOutcome::NoHandler;
    }
    let uri = wide(app.uri());
    // Safety: every pointer argument is a valid, null-terminated wide string alive for the whole
    // call (`uri` is owned by this function); `hwnd: None` means the launch is not tied to any
    // particular window, and `ShellExecuteW` itself never blocks on the process it starts.
    let result = unsafe {
        ShellExecuteW(None, w!("open"), PCWSTR::from_raw(uri.as_ptr()), PCWSTR::null(), PCWSTR::null(), SW_SHOWNORMAL)
    };
    // A pseudo-`HINSTANCE`: > 32 is success, <= 32 is one of the Windows SDK's own `SE_ERR_*`
    // codes (see the sonda's §3: `SE_ERR_NOASSOC` = 31 for "nothing registered").
    if result.0 as isize > 32 {
        ObsidianLaunchOutcome::Launched
    } else {
        ObsidianLaunchOutcome::Error(result.0 as u32)
    }
}

/// `true` if `HKEY_CLASSES_ROOT\<scheme>` ([`LaunchApp::registry_key`]) can be opened for
/// reading at all — its mere presence is what is being checked, not any particular value under it.
fn has_handler(app: LaunchApp) -> bool {
    let subkey = wide(app.registry_key());
    let mut key = HKEY::default();
    // Safety: `subkey` is a valid, null-terminated wide string owned by this function for the
    // whole call; `RegOpenKeyExW` only reads from the registry, and on success it writes a fresh,
    // valid handle into `key`, which is closed right after this function is done with it.
    let opened =
        unsafe { RegOpenKeyExW(HKEY_CLASSES_ROOT, PCWSTR::from_raw(subkey.as_ptr()), None, KEY_READ, &mut key) };
    if opened != ERROR_SUCCESS {
        return false;
    }
    // Safety: `key` is the handle `RegOpenKeyExW` just returned as open; nothing else in this
    // function holds or uses it afterwards.
    unsafe {
        let _ = RegCloseKey(key);
    }
    true
}

/// Launches `application` with the single argument `argument`, without waiting for it.
fn spawn_process(application: &str, argument: &str) -> ObsidianLaunchOutcome {
    let application_wide = wide(application);
    // Quoted so a path with spaces (not expected from `GetSystemDirectoryW`, but this function
    // is not only used with it) is never split into two arguments by `CreateProcessW`'s own
    // command-line parsing.
    let mut command_line = wide(&format!("\"{application}\" \"{argument}\""));
    let startup_info = STARTUPINFOW { cb: std::mem::size_of::<STARTUPINFOW>() as u32, ..Default::default() };
    let mut process_info = PROCESS_INFORMATION::default();

    // Safety: `application_wide` and `command_line` are valid, null-terminated wide strings
    // owned by this function for the whole call; `startup_info` is a fully zeroed, correctly
    // sized struct; `process_info` is a valid, writable out-parameter. `CreateProcessW` may
    // rewrite `command_line` in place but never reallocates it, and this function never reads it
    // again afterwards, so that is not observed. This never waits for the child (no
    // `WaitForSingleObject`), which is what keeps it from blocking the game's thread.
    let result = unsafe {
        CreateProcessW(
            PCWSTR::from_raw(application_wide.as_ptr()),
            Some(PWSTR(command_line.as_mut_ptr())),
            None,
            None,
            false,
            PROCESS_CREATION_FLAGS(0),
            None,
            PCWSTR::null(),
            &startup_info,
            &mut process_info,
        )
    };
    match result {
        Ok(()) => {
            // Safety: both are valid, freshly returned handles this function exclusively owns;
            // closing them right away rather than waiting on the child is exactly what "does not
            // block the game's thread" asks for — the child keeps running independently of these
            // handles.
            unsafe {
                let _ = CloseHandle(process_info.hProcess);
                let _ = CloseHandle(process_info.hThread);
            }
            ObsidianLaunchOutcome::Launched
        }
        Err(error) => ObsidianLaunchOutcome::Error(error.code().0 as u32),
    }
}

/// A null-terminated UTF-16 buffer, the shape every wide Win32 string parameter here needs.
fn wide(text: &str) -> Vec<u16> {
    text.encode_utf16().chain(std::iter::once(0)).collect()
}
