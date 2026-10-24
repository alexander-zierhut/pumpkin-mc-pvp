//! Maps the WIT `sound` enum to vanilla sounds.

use std::collections::HashMap;
use std::sync::LazyLock;

use pumpkin_data::sound::Sound;

/// Letters and digits only, lowercased: `UiButtonClick` and `ui.button_click` both become
/// `uibuttonclick`, so the generated enum's variant name finds the vanilla sound id.
fn normalize(name: &str) -> String {
    name.chars()
        .filter(char::is_ascii_alphanumeric)
        .map(|c| c.to_ascii_lowercase())
        .collect()
}

static BY_NORMALIZED_NAME: LazyLock<HashMap<String, Sound>> = LazyLock::new(|| {
    Sound::NAMES
        .iter()
        .filter_map(|name| Sound::from_name(name).map(|sound| (normalize(name), sound)))
        .collect()
});

/// The vanilla sound for a WIT sound, given its `Debug` text (e.g. `UiButtonClick`).
#[must_use]
pub fn from_wit_debug_name(debug_name: &str) -> Option<Sound> {
    let variant = debug_name.rsplit("::").next().unwrap_or(debug_name);
    BY_NORMALIZED_NAME.get(&normalize(variant)).copied()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn variant_names_find_vanilla_sounds() {
        assert_eq!(
            from_wit_debug_name("UiButtonClick"),
            Some(Sound::UiButtonClick)
        );
        assert_eq!(
            from_wit_debug_name("Sound::EntityPlayerLevelup"),
            Some(Sound::EntityPlayerLevelup)
        );
        assert_eq!(
            from_wit_debug_name("EntityAllayAmbientWithItem"),
            Some(Sound::EntityAllayAmbientWithItem)
        );
        assert_eq!(from_wit_debug_name("NotASound"), None);
    }
}
