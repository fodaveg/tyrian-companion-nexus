//! The two icons this addon adds to Nexus's quick access bar, as plain data and state: their
//! identifiers, their tooltips, and which of the addon's own windows are open. The Nexus calls
//! that register them live in the Windows-only `addon` crate; keeping the decisions here is what
//! lets `cargo test` cover them.
//!
//! Nothing here sends input to the game or automates anything: a click only opens or closes a
//! window of this addon.

use crate::settings::Settings;

/// One icon of the bar.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Shortcut {
    /// Shows or hides the Labyrinth panel.
    Panel,
    /// Opens the addon's settings window.
    Options,
}

impl Shortcut {
    pub const ALL: [Shortcut; 2] = [Shortcut::Panel, Shortcut::Options];

    /// Quick access identifier, stable across loads.
    pub fn id(self) -> &'static str {
        match self {
            Shortcut::Panel => "TYRIAN_COMPANION_QA_PANEL",
            Shortcut::Options => "TYRIAN_COMPANION_QA_OPTIONS",
        }
    }

    /// Keybind the click triggers. Registered without a key: the player may assign one in Nexus.
    pub fn keybind_id(self) -> &'static str {
        match self {
            Shortcut::Panel => "KB_TYRIAN_COMPANION_TOGGLE_PANEL",
            Shortcut::Options => "KB_TYRIAN_COMPANION_OPEN_OPTIONS",
        }
    }

    pub fn texture_id(self) -> &'static str {
        match self {
            Shortcut::Panel => "TEX_TYRIAN_COMPANION_QA_PANEL",
            Shortcut::Options => "TEX_TYRIAN_COMPANION_QA_OPTIONS",
        }
    }

    pub fn texture_hover_id(self) -> &'static str {
        match self {
            Shortcut::Panel => "TEX_TYRIAN_COMPANION_QA_PANEL_HOVER",
            Shortcut::Options => "TEX_TYRIAN_COMPANION_QA_OPTIONS_HOVER",
        }
    }

    /// Tooltip in the language the farming panel is set to.
    pub fn tooltip(self, english: bool) -> &'static str {
        match (self, english) {
            (Shortcut::Panel, false) => "Panel de Laberinto",
            (Shortcut::Panel, true) => "Labyrinth panel",
            (Shortcut::Options, false) => "Opciones de Tyrian Companion",
            (Shortcut::Options, true) => "Tyrian Companion options",
        }
    }

    /// The action a click (or the assigned key) performs.
    pub fn activate(self, windows: &mut PanelWindows) {
        match self {
            Shortcut::Panel => windows.toggle_panel(),
            Shortcut::Options => windows.toggle_options(),
        }
    }
}

/// Which of the addon's own windows are open, and how the panel is shown. The panel flag is the
/// persisted `show_farming_panel` setting, the same one the Options checkbox edits; the two
/// buttons of the panel's title bar (no background, folded) are persisted too; the options
/// window never is.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct PanelWindows {
    pub show_panel: bool,
    pub show_options: bool,
    /// The panel is painted without its window background.
    pub transparent: bool,
    /// The panel is folded down to its title bar.
    pub collapsed: bool,
}

impl PanelWindows {
    pub fn from_settings(settings: &Settings) -> Self {
        Self {
            show_panel: settings.show_farming_panel,
            show_options: false,
            transparent: settings.farming_panel_transparent,
            collapsed: settings.farming_panel_collapsed,
        }
    }

    pub fn toggle_panel(&mut self) {
        self.show_panel = !self.show_panel;
    }

    pub fn toggle_options(&mut self) {
        self.show_options = !self.show_options;
    }

    /// `base` with the persisted part of this state applied.
    pub fn apply_to(&self, mut base: Settings) -> Settings {
        base.show_farming_panel = self.show_panel;
        base.farming_panel_transparent = self.transparent;
        base.farming_panel_collapsed = self.collapsed;
        base
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn identifiers_are_prefixed_distinct_and_tooltips_bilingual() {
        let mut seen = std::collections::HashSet::new();
        for shortcut in Shortcut::ALL {
            for id in [
                shortcut.id(),
                shortcut.keybind_id(),
                shortcut.texture_id(),
                shortcut.texture_hover_id(),
            ] {
                assert!(
                    id.contains("TYRIAN_COMPANION") && id.is_ascii() && !id.contains(' '),
                    "{id}"
                );
                assert!(seen.insert(id), "duplicate {id}");
            }
            assert_ne!(shortcut.tooltip(false), shortcut.tooltip(true));
        }
        assert_eq!(Shortcut::Panel.tooltip(false), "Panel de Laberinto");
    }

    #[test]
    fn panel_icon_toggles_the_same_flag_the_setting_persists() {
        let dir = std::env::temp_dir().join(format!("tyrian-qa-{}", std::process::id()));
        let mut windows = PanelWindows::from_settings(&Settings::default());
        assert!(!windows.show_panel);
        Shortcut::Panel.activate(&mut windows);
        assert!(windows.show_panel && !windows.show_options);
        crate::settings::save(&dir, &windows.apply_to(Settings::default())).unwrap();
        let reloaded = crate::settings::load(&dir).settings;
        assert!(
            reloaded.show_farming_panel,
            "persisted through the shared setting"
        );
        assert!(PanelWindows::from_settings(&reloaded).show_panel);
        Shortcut::Panel.activate(&mut windows);
        assert!(!windows.show_panel);
        crate::settings::save(&dir, &windows.apply_to(reloaded)).unwrap();
        assert!(!crate::settings::load(&dir).settings.show_farming_panel);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn the_title_bar_flags_persist_through_the_settings() {
        let dir = std::env::temp_dir().join(format!("tyrian-qa-flags-{}", std::process::id()));
        let mut windows = PanelWindows::from_settings(&Settings::default());
        assert!(!windows.transparent && !windows.collapsed);
        windows.transparent = true;
        windows.collapsed = true;
        crate::settings::save(&dir, &windows.apply_to(Settings::default())).unwrap();
        let reloaded = PanelWindows::from_settings(&crate::settings::load(&dir).settings);
        assert!(reloaded.transparent && reloaded.collapsed && !reloaded.show_panel);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn options_icon_toggles_only_a_transient_window() {
        let mut windows = PanelWindows::default();
        Shortcut::Options.activate(&mut windows);
        assert!(windows.show_options && !windows.show_panel);
        assert!(!windows.apply_to(Settings::default()).show_farming_panel);
        Shortcut::Options.activate(&mut windows);
        assert!(!windows.show_options);
    }
}
