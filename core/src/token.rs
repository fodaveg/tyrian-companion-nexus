//! What the user may save as the token, decided before it reaches the settings file or a `hello`.
//!
//! The token field sits a few centimetres from where players keep their Guild Wars 2 API key, and
//! on 2026-09-24 that is exactly what got pasted into it: the addon wrote the API key to
//! `settings.json` in clear and sent it to the plugin as the token, and the plugin answered with a
//! bare `auth_rejected`. An API key is a credential for the player's account, not for this bridge;
//! it has no business on disk here or on the wire. So a value with the API key's shape is refused
//! when saved, dropped when found in an old `settings.json`, and never counts as a usable token.
//!
//! A value outside the format the plugin accepts (32 to 128 printable ASCII characters, no spaces)
//! is refused at save time as well, with its own message, instead of being saved only to fail the
//! next connection with the same bare rejection.
//!
//! The raw value never appears in anything this module returns other than the accepted token:
//! a [`TokenRejection`] carries no copy of what was pasted, so a caller that logs it logs nothing
//! secret.

use crate::protocol::{TOKEN_MAX_CHARS, TOKEN_MIN_CHARS};

/// Group lengths of a Guild Wars 2 API key, in hex digits: `8-4-4-4-20-4-4-4-12`, 72 characters
/// with the dashes. That is two GUIDs back to back, the second one glued to the first's last group.
const GW2_API_KEY_GROUPS: [usize; 9] = [8, 4, 4, 4, 20, 4, 4, 4, 12];

/// Why a pasted value was not saved. Deliberately carries nothing of the value itself.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TokenRejection {
    /// The value has the shape of a Guild Wars 2 API key.
    Gw2ApiKey,
    /// The value is not something the plugin would ever accept as its token.
    Malformed,
}

/// Shown when the pasted value is a Guild Wars 2 API key.
pub const GW2_API_KEY_MESSAGE: &str = "That is your Guild Wars 2 API key, not the token. In Obsidian: Tyrian Companion \
     settings > \"Copy token\" (\"Copiar token\"), then paste it here.";

/// Shown when the pasted value is outside the plugin's format.
pub const MALFORMED_TOKEN_MESSAGE: &str = "That is not the token: the token has 32 to 128 characters and no spaces. In \
     Obsidian: Tyrian Companion settings > \"Copy token\" (\"Copiar token\"), then paste it here.";

/// Shown once at load when an API key saved by 0.2.0 was found and removed from `settings.json`.
pub const DISCARDED_API_KEY_MESSAGE: &str = "Tyrian Companion: the saved token was your Guild Wars 2 API key, so it has \
     been removed. In Obsidian: Tyrian Companion settings > \"Copy token\" (\"Copiar token\"), then paste it in Nexus \
     options";

impl TokenRejection {
    /// What to tell the user, in the same words wherever the rejection is shown.
    pub fn message(self) -> &'static str {
        match self {
            Self::Gw2ApiKey => GW2_API_KEY_MESSAGE,
            Self::Malformed => MALFORMED_TOKEN_MESSAGE,
        }
    }
}

/// `true` if `value` has the exact shape of a Guild Wars 2 API key: hex groups of
/// `8-4-4-4-20-4-4-4-12`, in either case. Surrounding whitespace is not stripped here; callers
/// that deal with pasted text go through [`validate_for_save`], which trims first.
pub fn is_gw2_api_key(value: &str) -> bool {
    let mut groups = value.split('-');
    for expected in GW2_API_KEY_GROUPS {
        match groups.next() {
            Some(group) if group.len() == expected && group.bytes().all(|byte| byte.is_ascii_hexdigit()) => {}
            _ => return false,
        }
    }
    groups.next().is_none()
}

/// Decides what the Save button may store. Trims surrounding whitespace first (a copy from
/// Obsidian or a browser often drags a newline or a space along), then:
///
/// - empty: `Ok("")`, which clears the token on purpose;
/// - an API key's shape: `Err(Gw2ApiKey)`, even though it would pass the plugin's own format;
/// - outside 32 to 128 printable ASCII characters without spaces: `Err(Malformed)`;
/// - anything else: `Ok(trimmed)`, the value to save and send.
pub fn validate_for_save(raw: &str) -> Result<String, TokenRejection> {
    let trimmed = raw.trim();
    if trimmed.is_empty() {
        return Ok(String::new());
    }
    if is_gw2_api_key(trimmed) {
        return Err(TokenRejection::Gw2ApiKey);
    }
    if !is_plugin_format(trimmed) {
        return Err(TokenRejection::Malformed);
    }
    Ok(trimmed.to_string())
}

/// The plugin's own rule (`isUsableIngameBridgeSecret`): 32 to 128 printable ASCII characters,
/// no spaces. Counted in bytes, which is the same as characters once every byte is ASCII.
pub(crate) fn is_plugin_format(token: &str) -> bool {
    (TOKEN_MIN_CHARS..=TOKEN_MAX_CHARS).contains(&token.len())
        && token.bytes().all(|byte| (0x21..=0x7e).contains(&byte))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Made up, with the real shape: what David pasted on 2026-09-24 looked like this.
    const API_KEY: &str = "0A1B2C3D-4E5F-6071-8293-A4B5C6D7E8F90A1B2C3D-4E5F-6071-8293-A4B5C6D7E8F9";
    /// 43 base64url characters, the shape `createIngameBridgeSecret` produces.
    const TOKEN: &str = "k2VnU0bq9mRjYp8tXwH3cL5sA7dF1gJ4hN6zQ0eT2uB";

    #[test]
    fn the_fixtures_have_the_shapes_they_claim() {
        assert_eq!(API_KEY.len(), 72);
        assert_eq!(TOKEN.len(), 43);
    }

    #[test]
    fn an_api_key_is_rejected_in_either_case() {
        assert_eq!(validate_for_save(API_KEY), Err(TokenRejection::Gw2ApiKey));
        assert_eq!(validate_for_save(&API_KEY.to_lowercase()), Err(TokenRejection::Gw2ApiKey));
    }

    #[test]
    fn an_api_key_with_clipboard_whitespace_around_it_is_still_rejected() {
        assert_eq!(validate_for_save(&format!("  {API_KEY}\r\n")), Err(TokenRejection::Gw2ApiKey));
    }

    #[test]
    fn a_43_character_token_is_accepted() {
        assert_eq!(validate_for_save(TOKEN), Ok(TOKEN.to_string()));
    }

    #[test]
    fn whitespace_around_a_token_is_trimmed() {
        assert_eq!(validate_for_save(&format!(" \t{TOKEN}\n")), Ok(TOKEN.to_string()));
    }

    #[test]
    fn a_short_or_long_value_is_rejected() {
        assert_eq!(validate_for_save(&"a".repeat(31)), Err(TokenRejection::Malformed));
        assert_eq!(validate_for_save(&"a".repeat(129)), Err(TokenRejection::Malformed));
        assert_eq!(validate_for_save(&"a".repeat(32)), Ok("a".repeat(32)));
        assert_eq!(validate_for_save(&"a".repeat(128)), Ok("a".repeat(128)));
    }

    #[test]
    fn inner_spaces_and_non_printable_characters_are_rejected() {
        assert_eq!(validate_for_save(&format!("{} {}", "a".repeat(20), "b".repeat(20))), Err(TokenRejection::Malformed));
        assert_eq!(validate_for_save(&format!("{}\u{7f}", "a".repeat(40))), Err(TokenRejection::Malformed));
        assert_eq!(validate_for_save(&format!("{}é", "a".repeat(40))), Err(TokenRejection::Malformed));
    }

    #[test]
    fn an_empty_value_clears_the_token() {
        assert_eq!(validate_for_save(""), Ok(String::new()));
        assert_eq!(validate_for_save("  \n"), Ok(String::new()));
    }

    #[test]
    fn near_misses_of_the_api_key_shape_are_not_mistaken_for_one() {
        // One group short, one digit short, a non-hex digit: none is an API key.
        let (head, _) = API_KEY.rsplit_once('-').unwrap();
        assert!(!is_gw2_api_key(head));
        assert!(!is_gw2_api_key(&API_KEY[..71]));
        assert!(!is_gw2_api_key(&API_KEY.replacen('A', "G", 1)));
        assert!(!is_gw2_api_key(&format!("{API_KEY}-0")));
        assert!(!is_gw2_api_key(TOKEN));
    }

    #[test]
    fn a_rejection_never_carries_the_pasted_value() {
        let printed = format!("{:?} {}", validate_for_save(API_KEY), TokenRejection::Gw2ApiKey.message());
        assert!(!printed.contains(API_KEY), "{printed}");
    }
}
