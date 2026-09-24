//! Turns what the host exposes (Nexus's `DL_MUMBLE_LINK` copy of the game's Mumble Link, and
//! `NexusLink::is_gameplay`) into the protocol's `context`: state, map id and character.
//!
//! Only those three. The Mumble Link also carries positions, the camera, combat and the account's
//! world, and none of it is read here: the SPEC's `context` is closed ("ni coordenadas, ni
//! cámara, ni combate, ni cuenta, ni botín"), and the audited sources carry no loot feed and no
//! reliable AFK signal, so nothing here pretends to derive either.
//!
//! The Mumble Link is parsed from raw bytes rather than through `nexus`'s optional `mumble`
//! feature: that feature pulls a second git dependency (`gw2_mumble`) for a struct whose layout
//! is fixed and public, and reading bytes keeps the parsing here, in the crate `cargo test` runs
//! on Linux, instead of in the Windows-only addon crate.
//!
//! Layout (`LinkedMem` from Mumble's link plugin, `wchar_t` being UTF-16 on Windows; the game's
//! `context` block from the Guild Wars 2 wiki's "API:MumbleLink"):
//!
//! | Offset | Field |
//! |---|---|
//! | 0 | `uiVersion: u32` |
//! | 4 | `uiTick: u32`, advanced by the game while it writes the link |
//! | 44 | `name: [u16; 256]`, `"Guild Wars 2"` once the game has written it |
//! | 592 | `identity: [u16; 256]`, a JSON object whose `name` is the character |
//! | 1108 | `context: [u8; 256]`; its `mapId: u32` sits at byte 28 of the block |
//! | 5460 | end of the structure |

use std::time::{Duration, Instant};

use crate::protocol::{sanitize_character_name, sanitize_map_id, GameContext, GameState};

/// Size of the Mumble Link shared structure, in bytes.
pub const MUMBLE_LINK_BYTES: usize = 5460;

const TICK_OFFSET: usize = 4;
const NAME_OFFSET: usize = 44;
const IDENTITY_OFFSET: usize = 592;
const WIDE_FIELD_CHARS: usize = 256;
const CONTEXT_OFFSET: usize = 1108;
const CONTEXT_MAP_ID_OFFSET: usize = 28;

/// What the game writes into `name` once it owns the link.
const GAME_LINK_NAME: &str = "Guild Wars 2";

/// How long after the last moment of gameplay a non-gameplay reading still counts as a loading
/// screen; past it, it counts as character select. An assumption, not a measurement: neither
/// the Mumble Link nor `NexusLink` distinguishes the two (both stop at `is_gameplay == false`),
/// and a loading screen is normally well under a minute. The in-game test is what can confirm or
/// tune it. Getting it wrong only mislabels which of the two non-gameplay states is reported;
/// neither one starts or ends a session in the plugin.
pub const LOADING_WINDOW: Duration = Duration::from_secs(60);

/// The fields this addon reads from one copy of the Mumble Link.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MumbleSnapshot {
    pub ui_tick: u32,
    /// `name` is `"Guild Wars 2"`: the game, not some other Mumble client, owns the link.
    pub written_by_game: bool,
    pub map_id: u32,
    /// The identity JSON's `name`, raw. `None` if the identity was empty or did not parse, which
    /// a copy taken while the game rewrites it can produce.
    pub character: Option<String>,
}

fn read_u32(bytes: &[u8], offset: usize) -> u32 {
    u32::from_le_bytes([bytes[offset], bytes[offset + 1], bytes[offset + 2], bytes[offset + 3]])
}

/// Decodes a NUL-terminated UTF-16LE field of `WIDE_FIELD_CHARS` code units.
fn read_wide(bytes: &[u8], offset: usize) -> String {
    let units: Vec<u16> = bytes[offset..offset + WIDE_FIELD_CHARS * 2]
        .chunks_exact(2)
        .map(|pair| u16::from_le_bytes([pair[0], pair[1]]))
        .take_while(|&unit| unit != 0)
        .collect();
    String::from_utf16_lossy(&units)
}

impl MumbleSnapshot {
    /// Reads a snapshot out of a copy of the link. `None` if `bytes` is shorter than the link.
    pub fn from_bytes(bytes: &[u8]) -> Option<Self> {
        if bytes.len() < MUMBLE_LINK_BYTES {
            return None;
        }
        let identity = read_wide(bytes, IDENTITY_OFFSET);
        let character = serde_json::from_str::<serde_json::Value>(&identity)
            .ok()
            .and_then(|value| value.get("name")?.as_str().map(str::to_string));
        Some(Self {
            ui_tick: read_u32(bytes, TICK_OFFSET),
            written_by_game: read_wide(bytes, NAME_OFFSET) == GAME_LINK_NAME,
            map_id: read_u32(bytes, CONTEXT_OFFSET + CONTEXT_MAP_ID_OFFSET),
            character,
        })
    }

    /// `true` once the game has loaded a character into a map at least once: before that, the
    /// link is all zeros and there is no map or character to report.
    pub fn has_game_data(&self) -> bool {
        self.written_by_game && self.ui_tick != 0
    }
}

/// Keeps what one reading alone cannot tell: when gameplay was last seen, and the last character
/// name that read cleanly.
#[derive(Debug, Default)]
pub struct ContextTracker {
    last_gameplay_at: Option<Instant>,
    last_character: Option<String>,
}

impl ContextTracker {
    pub fn new() -> Self {
        Self::default()
    }

    /// The context to report at `now`, given Nexus's `is_gameplay` (`None` if the `NexusLink`
    /// could not be read, treated as "not in gameplay") and a Mumble Link snapshot (`None` if it
    /// could not be read).
    ///
    /// - `is_gameplay` → `gameplay`, with the map and character the link has.
    /// - Not in gameplay, and gameplay was seen within [`LOADING_WINDOW`] → `loading`, still with
    ///   the link's map and character (the link keeps the last map until the next one loads).
    /// - Otherwise → `character_select`, with no map and no character: at character select the
    ///   link still holds the last map, and reporting it would claim a map nobody is on.
    pub fn observe(&mut self, now: Instant, is_gameplay: Option<bool>, mumble: Option<&MumbleSnapshot>) -> GameContext {
        let mumble = mumble.filter(|snapshot| snapshot.has_game_data());
        if let Some(name) = mumble.and_then(|snapshot| snapshot.character.as_deref()).and_then(sanitize_character_name) {
            self.last_character = Some(name);
        }
        let map_id = mumble.and_then(|snapshot| sanitize_map_id(snapshot.map_id));
        let character = mumble.and(self.last_character.clone());

        if is_gameplay == Some(true) {
            self.last_gameplay_at = Some(now);
            return GameContext { state: GameState::Gameplay, map_id, character };
        }
        let recently_in_gameplay = self
            .last_gameplay_at
            .is_some_and(|seen| now.saturating_duration_since(seen) <= LOADING_WINDOW);
        if recently_in_gameplay && mumble.is_some() {
            return GameContext { state: GameState::Loading, map_id, character };
        }
        GameContext::character_select()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Builds a link the way the game lays it out.
    fn link(tick: u32, name: &str, identity: &str, map_id: u32) -> Vec<u8> {
        let mut bytes = vec![0u8; MUMBLE_LINK_BYTES];
        bytes[0..4].copy_from_slice(&2u32.to_le_bytes());
        bytes[TICK_OFFSET..TICK_OFFSET + 4].copy_from_slice(&tick.to_le_bytes());
        for (index, unit) in name.encode_utf16().enumerate() {
            bytes[NAME_OFFSET + index * 2..NAME_OFFSET + index * 2 + 2].copy_from_slice(&unit.to_le_bytes());
        }
        for (index, unit) in identity.encode_utf16().enumerate() {
            bytes[IDENTITY_OFFSET + index * 2..IDENTITY_OFFSET + index * 2 + 2].copy_from_slice(&unit.to_le_bytes());
        }
        let map_at = CONTEXT_OFFSET + CONTEXT_MAP_ID_OFFSET;
        bytes[map_at..map_at + 4].copy_from_slice(&map_id.to_le_bytes());
        bytes
    }

    const IDENTITY: &str = r#"{"name":"Astra Uno","profession":4,"spec":55,"race":4,"map_id":866,"world_id":268435458,"team_color_id":0,"commander":false,"map":866,"fov":0.873,"uisz":1}"#;

    fn in_labyrinth() -> MumbleSnapshot {
        MumbleSnapshot::from_bytes(&link(1200, "Guild Wars 2", IDENTITY, 866)).unwrap()
    }

    #[test]
    fn reads_tick_map_and_character_from_the_game_layout() {
        let snapshot = in_labyrinth();
        assert_eq!(snapshot.ui_tick, 1200);
        assert!(snapshot.written_by_game);
        assert_eq!(snapshot.map_id, 866);
        assert_eq!(snapshot.character.as_deref(), Some("Astra Uno"));
        assert!(snapshot.has_game_data());
    }

    #[test]
    fn a_zeroed_link_has_no_game_data() {
        let snapshot = MumbleSnapshot::from_bytes(&vec![0u8; MUMBLE_LINK_BYTES]).unwrap();
        assert!(!snapshot.has_game_data());
        assert_eq!(snapshot.character, None);
    }

    #[test]
    fn a_link_written_by_another_mumble_client_is_not_the_game() {
        let snapshot = MumbleSnapshot::from_bytes(&link(5, "Other Game", IDENTITY, 866)).unwrap();
        assert!(!snapshot.has_game_data());
    }

    #[test]
    fn a_short_buffer_is_refused() {
        assert_eq!(MumbleSnapshot::from_bytes(&[0u8; 100]), None);
    }

    #[test]
    fn gameplay_reports_map_and_character() {
        let mut tracker = ContextTracker::new();
        let context = tracker.observe(Instant::now(), Some(true), Some(&in_labyrinth()));
        assert_eq!(context, GameContext { state: GameState::Gameplay, map_id: Some(866), character: Some("Astra Uno".into()) });
    }

    #[test]
    fn before_any_character_has_loaded_it_is_character_select_with_nothing_known() {
        let mut tracker = ContextTracker::new();
        let zeroed = MumbleSnapshot::from_bytes(&vec![0u8; MUMBLE_LINK_BYTES]).unwrap();
        assert_eq!(tracker.observe(Instant::now(), Some(false), Some(&zeroed)), GameContext::character_select());
        assert_eq!(tracker.observe(Instant::now(), None, None), GameContext::character_select());
    }

    #[test]
    fn leaving_gameplay_is_loading_within_the_window_then_character_select() {
        let mut tracker = ContextTracker::new();
        let start = Instant::now();
        tracker.observe(start, Some(true), Some(&in_labyrinth()));
        let loading = tracker.observe(start + Duration::from_secs(5), Some(false), Some(&in_labyrinth()));
        assert_eq!(loading, GameContext { state: GameState::Loading, map_id: Some(866), character: Some("Astra Uno".into()) });
        let later = tracker.observe(start + LOADING_WINDOW + Duration::from_secs(1), Some(false), Some(&in_labyrinth()));
        assert_eq!(later, GameContext::character_select());
    }

    #[test]
    fn a_torn_identity_keeps_the_last_character_that_read_cleanly() {
        let mut tracker = ContextTracker::new();
        let now = Instant::now();
        tracker.observe(now, Some(true), Some(&in_labyrinth()));
        let torn = MumbleSnapshot::from_bytes(&link(1201, "Guild Wars 2", r#"{"name":"Ast"#, 866)).unwrap();
        assert_eq!(torn.character, None);
        let context = tracker.observe(now, Some(true), Some(&torn));
        assert_eq!(context.character.as_deref(), Some("Astra Uno"));
    }

    #[test]
    fn a_map_id_of_zero_is_reported_as_no_map() {
        let mut tracker = ContextTracker::new();
        let snapshot = MumbleSnapshot::from_bytes(&link(3, "Guild Wars 2", IDENTITY, 0)).unwrap();
        assert_eq!(tracker.observe(Instant::now(), Some(true), Some(&snapshot)).map_id, None);
    }
}
