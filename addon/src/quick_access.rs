//! The two icons this addon puts in Nexus's quick access bar: the Labyrinth panel and the
//! addon's options. Which icon does what, the identifiers and the tooltips are decided in
//! `tyrian_companion_nexus_core::quick_access`; this file only talks to Nexus.
//!
//! A click triggers a keybind this addon registers WITHOUT a key (the player may assign one in
//! Nexus), and the keybind opens or closes one of the addon's own windows. Nothing here sends
//! input to the game.
//!
//! Textures are loaded from the PNGs embedded from `addon/assets/` (replace them and rebuild to
//! change the icons). Nexus may not hand a texture back on the very first call, so registration
//! is retried from the render callback for a few seconds; an icon whose textures never load is
//! logged and simply not added, and the addon carries on.

use std::sync::Mutex;

use nexus::keybind::{keybind_handler, register_keybind_with_struct, unregister_keybind, Keybind};
use nexus::quick_access::{add_quick_access, remove_quick_access};
use nexus::texture::get_texture_or_create_from_memory;

use tyrian_companion_nexus_core::quick_access::Shortcut;

/// Frames to keep retrying a texture that has not come back yet (about ten seconds).
const MAX_ATTEMPTS: u32 = 600;

struct Registry {
    /// Quick access icon added, per [`Shortcut::ALL`].
    added: [bool; 2],
    /// Keybind registered, per [`Shortcut::ALL`].
    keybound: [bool; 2],
    attempts: u32,
    gave_up: bool,
}

static REGISTRY: Mutex<Registry> = Mutex::new(Registry {
    added: [false; 2],
    keybound: [false; 2],
    attempts: 0,
    gave_up: false,
});

fn registry() -> std::sync::MutexGuard<'static, Registry> {
    REGISTRY
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
}

fn icons(shortcut: Shortcut) -> (&'static [u8], &'static [u8]) {
    match shortcut {
        Shortcut::Panel => (
            include_bytes!("../assets/qa-panel.png"),
            include_bytes!("../assets/qa-panel-hover.png"),
        ),
        Shortcut::Options => (
            include_bytes!("../assets/qa-options.png"),
            include_bytes!("../assets/qa-options-hover.png"),
        ),
    }
}

fn on_keybind(identifier: &str, is_release: bool) {
    if is_release {
        return;
    }
    if let Some(shortcut) = Shortcut::ALL
        .into_iter()
        .find(|shortcut| shortcut.keybind_id() == identifier)
    {
        crate::render::activate_shortcut(shortcut);
    }
}

/// Registers whatever is not registered yet. Called on load and then once per frame by the
/// render callback; cheap once everything is in or it has given up.
pub fn register_pending(english: bool) {
    let mut registry = registry();
    if registry.gave_up || registry.added.iter().all(|added| *added) {
        return;
    }
    registry.attempts += 1;
    for (index, shortcut) in Shortcut::ALL.into_iter().enumerate() {
        if registry.added[index] {
            continue;
        }
        let (normal, hover) = icons(shortcut);
        let loaded = get_texture_or_create_from_memory(shortcut.texture_id(), normal).is_some()
            && get_texture_or_create_from_memory(shortcut.texture_hover_id(), hover).is_some();
        if !loaded {
            continue;
        }
        if !registry.keybound[index] {
            // No key assigned: `key: 0`. The click works without one.
            register_keybind_with_struct(
                shortcut.keybind_id(),
                keybind_handler!(on_keybind),
                Keybind::without_modifiers(0),
            )
            .leak();
            registry.keybound[index] = true;
        }
        add_quick_access(
            shortcut.id(),
            shortcut.texture_id(),
            shortcut.texture_hover_id(),
            shortcut.keybind_id(),
            shortcut.tooltip(english),
        )
        .leak();
        registry.added[index] = true;
        log::info!("quick access icon {} added", shortcut.id());
    }
    if registry.attempts >= MAX_ATTEMPTS && !registry.added.iter().all(|added| *added) {
        registry.gave_up = true;
        for (index, shortcut) in Shortcut::ALL.into_iter().enumerate() {
            if !registry.added[index] {
                log::error!(
                    "quick access icon {} not added: its texture did not load",
                    shortcut.id()
                );
            }
        }
    }
}

/// Puts the tooltips in the language the panel is set to now: the bar takes the text when an
/// icon is added, so each icon already there is removed and added again.
///
/// The caller must not hold `render`'s `PENDING`: this calls Nexus, and a keybind Nexus
/// dispatches meanwhile takes that lock (`render::activate_shortcut`). The registry stays
/// locked through the calls on purpose, so `unregister` cannot run between a removal and the
/// addition that follows it. The keybind handler does not take the registry: its other takers
/// are `register_pending`, from the render callback, and `unregister`, from `unload`.
pub fn refresh_tooltips(english: bool) {
    let registry = registry();
    for (index, shortcut) in Shortcut::ALL.into_iter().enumerate() {
        if registry.added[index] {
            remove_quick_access(shortcut.id());
            add_quick_access(
                shortcut.id(),
                shortcut.texture_id(),
                shortcut.texture_hover_id(),
                shortcut.keybind_id(),
                shortcut.tooltip(english),
            )
            .leak();
        }
    }
}

/// Removes every icon and keybind this addon registered, so a reload starts clean. The textures
/// stay in Nexus (it offers no way to drop one); the same identifiers are reused on reload.
pub fn unregister() {
    let mut registry = registry();
    for (index, shortcut) in Shortcut::ALL.into_iter().enumerate() {
        if registry.added[index] {
            remove_quick_access(shortcut.id());
        }
        if registry.keybound[index] {
            unregister_keybind(shortcut.keybind_id());
        }
    }
    // Nothing may register again after this, even if a frame still runs before Nexus drops the callbacks.
    *registry = Registry {
        added: [false; 2],
        keybound: [false; 2],
        attempts: 0,
        gave_up: true,
    };
}
