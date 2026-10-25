use serde::{Deserialize, Serialize};

/// Configuration for player-versus-player mechanics.
///
/// Controls whether PVP is enabled, combat effects, and player protections.
#[derive(Deserialize, Serialize)]
#[serde(default)]
pub struct PVPConfig {
    /// Whether PVP is enabled on the server.
    pub enabled: bool,
    /// Whether to show the red hurt animation and FOV bobbing when hit.
    pub hurt_animation: bool,
    /// Whether players in creative mode are protected from PVP.
    pub protect_creative: bool,
    /// Whether knockback from attacks is applied.
    pub knockback: bool,
    /// Whether players swing their hand when attacking.
    pub swing: bool,
    /// Minecraft 1.8 combat for modern clients (no attack cooldown, 1.8 damage, knockback,
    /// armour, regeneration and bow).
    pub legacy_combat: LegacyCombatConfig,
}

/// Minecraft 1.8 combat mechanics, in the spirit of the `OldCombatMechanics` Bukkit plugin.
#[derive(Deserialize, Serialize, Clone)]
#[serde(default)]
pub struct LegacyCombatConfig {
    /// Whether 1.8 combat is used instead of the vanilla 26.x rules.
    pub enabled: bool,
    /// Base attack speed given to players, high enough that the client never shows a cooldown.
    pub attack_speed: f64,
    /// Divisor for the victim's motion before knockback is added (1.8: 2).
    pub knockback_friction: f64,
    /// Horizontal base knockback (1.8: 0.4).
    pub knockback_horizontal: f64,
    /// Vertical base knockback (1.8: 0.4).
    pub knockback_vertical: f64,
    /// Cap for the vertical motion after the base knockback (1.8: 0.4).
    pub knockback_vertical_limit: f64,
    /// Extra horizontal knockback per sprint/Knockback level (1.8: 0.5).
    pub knockback_extra_horizontal: f64,
    /// Extra vertical knockback when sprinting or with Knockback (1.8: 0.1).
    pub knockback_extra_vertical: f64,
    /// Roll the 1.8 random 50-100 % of the protection enchantment factor.
    pub armour_randomness: bool,
    /// Ticks between natural regeneration heals with 18+ food (1.8: 80, no saturation boost).
    pub regen_interval_ticks: u32,
    /// Exhaustion added per natural heal (1.8: 3).
    pub regen_exhaustion: f32,
}

impl Default for LegacyCombatConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            attack_speed: 40.0,
            knockback_friction: 2.0,
            knockback_horizontal: 0.4,
            knockback_vertical: 0.4,
            knockback_vertical_limit: 0.4,
            knockback_extra_horizontal: 0.5,
            knockback_extra_vertical: 0.1,
            armour_randomness: true,
            regen_interval_ticks: 80,
            regen_exhaustion: 3.0,
        }
    }
}

impl Default for PVPConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            hurt_animation: true,
            protect_creative: true,
            knockback: true,
            swing: true,
            legacy_combat: LegacyCombatConfig::default(),
        }
    }
}
