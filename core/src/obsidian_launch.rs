//! Whether this addon's load should try to open Obsidian, and what the panel shows about the
//! attempt. Pure decision only: the actual OS mechanism (winebrowser.exe under Wine/Proton,
//! `ShellExecuteW` on native Windows) lives in the `addon` crate, behind `Host::open_obsidian`,
//! since it needs `windows` and cannot build for this repository's Linux host. Splitting it this
//! way is what lets `cargo test` cover the decision on Linux.
//!
//! David, 24 sep 2026: "si empiezo a jugar y está cerrado, que se abra". Sondado in H18.27
//! (`docs/audit/sonda-h18-27-abrir-obsidian-desde-proton.md` in the `tyrian-companion` repo):
//! a process inside Wine/Proton can make the host's `obsidian://` open or focus the real
//! Obsidian. The addon is the only thing that can see "Obsidian is closed" — the plugin itself
//! never runs while Obsidian is closed — and the one signal it has for that is its own first
//! attempt to connect to the plugin's bridge finding nobody there.

use std::fmt;

/// What [`should_launch`] treats as "was the plugin listening?", for the very first connection
/// attempt this addon's load makes. Only a TCP-level failure of that first `connect()` — refused
/// or timed out, `core::client`'s only two ways for it to fail — counts as [`NoListener`]:
/// anything that requires speaking the wire protocol at all (a `welcome`, an `auth_rejected`, a
/// `version_unsupported`) only happens after the TCP connect already succeeded, so it is
/// [`Connected`] regardless of what the plugin says afterwards. That is what keeps a wrong token
/// or an old addon version from being mistaken for "Obsidian is closed".
///
/// [`NoListener`]: FirstConnectOutcome::NoListener
/// [`Connected`]: FirstConnectOutcome::Connected
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FirstConnectOutcome {
    /// The TCP `connect()` itself failed: nobody was listening on the port.
    NoListener,
    /// The TCP `connect()` succeeded, whatever happened at the protocol level afterwards.
    Connected,
}

/// What happened, if anything, when the addon tried to open Obsidian. Shown on the Options
/// panel; never a native alert (the SPEC's alerts are for the plugin's own content, and a
/// housekeeping message here would be noise the player did not ask for).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ObsidianLaunchOutcome {
    /// The launch call was accepted by the OS. This does not confirm Obsidian actually opened
    /// or is now focused, only that the launch mechanism did not report an immediate error.
    Launched,
    /// Windows native only: no `HKEY_CLASSES_ROOT\obsidian` handler is registered, so nothing
    /// was launched — the alternative is Windows popping its own "how do you want to open
    /// this?" dialog on top of the game.
    NoHandler,
    /// The launch mechanism reported an error. The code is whatever the OS call returned
    /// (`GetLastError`, `ShellExecuteW`'s own `SE_ERR_*` table, or an `HRESULT`); it is shown
    /// as-is rather than translated, so it can be looked up against Microsoft's own tables.
    Error(u32),
}

impl fmt::Display for ObsidianLaunchOutcome {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Launched => write!(formatter, "launched"),
            Self::NoHandler => write!(formatter, "no obsidian:// handler registered"),
            Self::Error(code) => write!(formatter, "error (code {code})"),
        }
    }
}

/// Decides whether *this* attempt — the addon's very first connection attempt since load —
/// should launch Obsidian. Pure: no I/O, no OS call, no clock. `core::client::run` calls this
/// exactly once per load, right after its first `connect()`, with `already_attempted` always
/// `false` that one time; it is a parameter (rather than hidden state in here) so each rule can
/// be tested on its own without spinning up a client loop.
///
/// - `open_on_start`: the `open_obsidian_on_start` setting.
/// - `already_attempted`: `true` once this load has already made this decision, whatever it
///   decided. Guarantees the "as most once per load" rule even if the caller is ever called more
///   than once by mistake.
/// - `first_connect`: the outcome of the connection attempt this decision is reacting to.
pub fn should_launch(open_on_start: bool, already_attempted: bool, first_connect: FirstConnectOutcome) -> bool {
    open_on_start && !already_attempted && first_connect == FirstConnectOutcome::NoListener
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn launches_when_the_first_connect_is_refused_and_the_setting_is_on() {
        assert!(should_launch(true, false, FirstConnectOutcome::NoListener));
    }

    #[test]
    fn does_not_launch_when_the_first_connect_succeeds() {
        assert!(!should_launch(true, false, FirstConnectOutcome::Connected));
    }

    #[test]
    fn does_not_launch_with_the_setting_off() {
        assert!(!should_launch(false, false, FirstConnectOutcome::NoListener));
    }

    #[test]
    fn does_not_launch_twice_even_if_more_retries_fail() {
        // `already_attempted` is what the second and later retries pass, once the first one has
        // already made this decision — whether or not it actually launched.
        assert!(!should_launch(true, true, FirstConnectOutcome::NoListener));
    }

    #[test]
    fn does_not_launch_over_an_authentication_error() {
        // auth_rejected and version_unsupported only ever arrive after a successful TCP
        // connect, so they are `Connected`, never `NoListener` — this is the same rule as
        // `does_not_launch_when_the_first_connect_succeeds`, asserted under the name the
        // scenario is actually about.
        assert!(!should_launch(true, false, FirstConnectOutcome::Connected));
    }
}
