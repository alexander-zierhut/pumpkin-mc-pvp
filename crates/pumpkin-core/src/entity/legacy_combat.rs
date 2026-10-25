//! Minecraft 1.8 combat for modern clients, enabled with `[pvp.legacy_combat]`.
//!
//! The rules follow the 1.8.9 server (MCP names in the comments) and the `OldCombatMechanics`
//! Bukkit plugin, which does the same on Paper: no attack cooldown, 1.8 weapon damage, sharpness,
//! critical hits, strength, knockback, armour and protection, sword blocking, natural regeneration
//! and the 1.8 bow launch (no shooter momentum, Gaussian spread, launch offset).

use std::sync::Arc;
use std::sync::atomic::Ordering;

use pumpkin_config::LegacyCombatConfig;
use pumpkin_data::damage::DamageType;
use pumpkin_data::effect::StatusEffect;
use pumpkin_data::item::Item;
use pumpkin_data::item_stack::ItemStack;
use pumpkin_data::tag::{self, Taggable};
use pumpkin_data::{Block, Enchantment};
use pumpkin_protocol::java::client::play::{Animation, CEntityAnimation};
use pumpkin_util::math::vector3::Vector3;

use crate::entity::player::Player;
use crate::entity::{Entity, EntityBase};
use crate::world::World;

/// Motion of a resting 1.8 player: gravity (0.08) after drag (0.98), from `EntityLivingBase.moveEntityWithHeading`.
pub const RESTING_MOTION_Y: f64 = -0.0784;
/// `EntityLivingBase.moveEntityWithHeading` horizontal friction in the air.
const AIR_FRICTION: f64 = 0.91;
/// Same, on a normal block (slipperiness 0.6 * 0.91).
const GROUND_FRICTION: f64 = 0.546;
/// `EntityLivingBase.onLivingUpdate`: smaller motion components snap to 0.
const MOTION_EPSILON: f64 = 0.005;
/// `EntityLivingBase.getJumpUpwardsMotion`.
const JUMP_MOTION: f64 = 0.42;
/// `EntityLivingBase.jump`: sprint jumps add this along the yaw.
const SPRINT_JUMP_BOOST: f64 = 0.2;
/// `Entity.setThrowableHeading` inaccuracy step.
const LAUNCH_INACCURACY: f64 = 0.007_499_999_832_361_937;
/// `ItemBow.onPlayerStoppedUsing`: arrow speed is `force * 2 * 1.5`.
const ARROW_SPEED: f64 = 3.0;
/// `NetHandlerPlayServer.processUseEntity` accepts attacks closer than 6 blocks.
const MAX_ATTACK_DISTANCE_SQUARED: f64 = 36.0;

/// The legacy combat config when it is enabled for this world's server.
pub fn config(world: &World) -> Option<LegacyCombatConfig> {
    let server = world.server.upgrade()?;
    let cfg = &server.advanced_config.pvp.legacy_combat;
    cfg.enabled.then(|| cfg.clone())
}

/// Total 1.8 melee damage of the held item, base hand damage (1) included.
/// Values from `ItemSword`/`ItemTool` 1.8 and the `OldCombatMechanics` table for newer items.
#[must_use]
pub fn weapon_damage(item: &Item) -> f32 {
    const TABLE: &[(&Item, f32)] = &[
        (&Item::WOODEN_SWORD, 5.0),
        (&Item::GOLDEN_SWORD, 5.0),
        (&Item::STONE_SWORD, 6.0),
        (&Item::COPPER_SWORD, 6.0),
        (&Item::IRON_SWORD, 7.0),
        (&Item::DIAMOND_SWORD, 8.0),
        (&Item::NETHERITE_SWORD, 9.0),
        (&Item::WOODEN_AXE, 4.0),
        (&Item::GOLDEN_AXE, 4.0),
        (&Item::STONE_AXE, 5.0),
        (&Item::COPPER_AXE, 6.0),
        (&Item::IRON_AXE, 6.0),
        (&Item::DIAMOND_AXE, 7.0),
        (&Item::NETHERITE_AXE, 8.0),
        (&Item::WOODEN_PICKAXE, 3.0),
        (&Item::GOLDEN_PICKAXE, 3.0),
        (&Item::STONE_PICKAXE, 4.0),
        (&Item::COPPER_PICKAXE, 4.0),
        (&Item::IRON_PICKAXE, 5.0),
        (&Item::DIAMOND_PICKAXE, 6.0),
        (&Item::NETHERITE_PICKAXE, 7.0),
        (&Item::WOODEN_SHOVEL, 2.0),
        (&Item::GOLDEN_SHOVEL, 2.0),
        (&Item::STONE_SHOVEL, 3.0),
        (&Item::COPPER_SHOVEL, 3.0),
        (&Item::IRON_SHOVEL, 4.0),
        (&Item::DIAMOND_SHOVEL, 5.0),
        (&Item::NETHERITE_SHOVEL, 6.0),
        (&Item::TRIDENT, 8.0),
        (&Item::MACE, 6.0),
    ];
    TABLE
        .iter()
        .find(|(entry, _)| entry.id == item.id)
        .map_or(1.0, |(_, damage)| *damage)
}

/// 1.8 `EnchantmentDamage.calcDamageByCreature`: Sharpness adds 1.25 per level.
#[must_use]
pub fn sharpness_damage(level: i32) -> f32 {
    level.max(0) as f32 * 1.25
}

/// 1.8 `PotionAttackDamage`: strength multiplies by `1 + 1.3 * level`, weakness by `1 - 0.5 * level`
/// (operation 2 attribute modifiers on attack damage).
#[must_use]
pub fn apply_attack_potions(damage: f32, strength_level: i32, weakness_level: i32) -> f32 {
    let strength = 1.0 + 1.3 * strength_level.max(0) as f32;
    let weakness = (1.0 - 0.5 * weakness_level.max(0) as f32).max(0.0);
    (damage * strength * weakness).max(0.0)
}

/// 1.8 `EntityPlayer.attackTargetEntityWithCurrentItem` critical hit condition (sprinting allowed).
#[must_use]
#[expect(
    clippy::fn_params_excessive_bools,
    reason = "the six conditions of the 1.8 check, one to one"
)]
pub fn is_critical(
    fall_distance: f32,
    on_ground: bool,
    climbing: bool,
    in_water: bool,
    blind: bool,
    riding: bool,
) -> bool {
    fall_distance > 0.0 && !on_ground && !climbing && !in_water && !blind && !riding
}

/// Full 1.8 melee damage: (weapon * potions) [* 1.5 when critical] + sharpness.
#[must_use]
pub fn melee_damage(
    weapon: f32,
    strength_level: i32,
    weakness_level: i32,
    critical: bool,
    sharpness_level: i32,
) -> f32 {
    let mut damage = apply_attack_potions(weapon, strength_level, weakness_level);
    let enchantment = sharpness_damage(sharpness_level);
    if critical && damage > 0.0 {
        damage *= 1.5;
    }
    damage + enchantment
}

/// 1.8 `EntityLivingBase.knockBack` as configured in `OldCombatMechanics` (`old-player-knockback`):
/// `dx`/`dz` point from the victim to the attacker.
#[must_use]
pub fn base_knockback(
    motion: Vector3<f64>,
    mut dx: f64,
    mut dz: f64,
    cfg: &LegacyCombatConfig,
) -> Vector3<f64> {
    while dx.mul_add(dx, dz * dz) < 1.0E-4 {
        dx = (rand::random::<f64>() - rand::random::<f64>()) * 0.01;
        dz = (rand::random::<f64>() - rand::random::<f64>()) * 0.01;
    }
    let length = dx.hypot(dz);
    Vector3::new(
        motion.x / cfg.knockback_friction - dx / length * cfg.knockback_horizontal,
        (motion.y / cfg.knockback_friction + cfg.knockback_vertical)
            .min(cfg.knockback_vertical_limit),
        motion.z / cfg.knockback_friction - dz / length * cfg.knockback_horizontal,
    )
}

/// 1.8 extra knockback for sprint hits and the Knockback enchantment (`addVelocity` in the attack).
#[must_use]
pub fn extra_knockback(attacker_yaw: f32, level: i32, cfg: &LegacyCombatConfig) -> Vector3<f64> {
    let yaw = f64::from(attacker_yaw.to_radians());
    let level = f64::from(level);
    Vector3::new(
        -yaw.sin() * level * cfg.knockback_extra_horizontal,
        cfg.knockback_extra_vertical,
        yaw.cos() * level * cfg.knockback_extra_horizontal,
    )
}

/// One tick of the motion a 1.8 server keeps for a player (`EntityPlayerMP.onUpdateEntity`).
///
/// 1.8 runs it once per movement packet. The server never sees the client's input or the knockback the client flies with, so this motion only decays
/// and falls: friction on x/z, gravity and drag on y, and landing (`Block.onLanded`) zeroes y first. `on_ground`
/// is the state of the previous packet.
#[must_use]
pub fn server_motion_tick(motion: Vector3<f64>, on_ground: bool) -> Vector3<f64> {
    let snap = |v: f64| if v.abs() < MOTION_EPSILON { 0.0 } else { v };
    let friction = if on_ground {
        GROUND_FRICTION
    } else {
        AIR_FRICTION
    };
    let y = if on_ground && motion.y <= 0.0 {
        0.0
    } else {
        snap(motion.y)
    };
    Vector3::new(
        snap(motion.x) * friction,
        (y - 0.08) * 0.98,
        snap(motion.z) * friction,
    )
}

/// 1.8 `EntityLivingBase.jump` on the server's copy of a player's motion.
///
/// The 1.8 server runs it itself when a movement packet leaves the ground going up
/// (`NetHandlerPlayServer.processPlayer`); a sprint jump also adds 0.2 along the yaw.
#[must_use]
pub fn server_jump(
    motion: Vector3<f64>,
    yaw: f32,
    sprinting: bool,
    jump_boost_level: i32,
) -> Vector3<f64> {
    let mut motion = Vector3::new(
        motion.x,
        JUMP_MOTION + f64::from(jump_boost_level.max(0)) * 0.1,
        motion.z,
    );
    if sprinting {
        let yaw = f64::from(yaw.to_radians());
        motion.x -= yaw.sin() * SPRINT_JUMP_BOOST;
        motion.z += yaw.cos() * SPRINT_JUMP_BOOST;
    }
    motion
}

/// 1.8 `EntityLivingBase.applyArmorCalculations`: 4 % less damage per armour point, no toughness.
#[must_use]
pub fn armour_absorb(damage: f32, armour_points: f32) -> f32 {
    let armour = armour_points.clamp(0.0, 25.0).floor();
    damage * (25.0 - armour) / 25.0
}

/// 1.8 `EnchantmentProtection.calcModifierDamage` for one enchanted armour piece.
#[must_use]
pub fn protection_epf(enchantment: &Enchantment, level: i32, damage_type: &DamageType) -> i32 {
    if level <= 0
        || damage_type.has_tag(&tag::DamageType::MINECRAFT_BYPASSES_INVULNERABILITY)
        || damage_type.has_tag(&tag::DamageType::MINECRAFT_BYPASSES_ENCHANTMENTS)
    {
        return 0;
    }
    let type_modifier = if *enchantment == Enchantment::PROTECTION {
        0.75
    } else if *enchantment == Enchantment::FIRE_PROTECTION
        && damage_type.has_tag(&tag::DamageType::MINECRAFT_IS_FIRE)
    {
        1.25
    } else if *enchantment == Enchantment::FEATHER_FALLING
        && damage_type.has_tag(&tag::DamageType::MINECRAFT_IS_FALL)
    {
        2.5
    } else if (*enchantment == Enchantment::BLAST_PROTECTION
        && damage_type.has_tag(&tag::DamageType::MINECRAFT_IS_EXPLOSION))
        || (*enchantment == Enchantment::PROJECTILE_PROTECTION
            && damage_type.has_tag(&tag::DamageType::MINECRAFT_IS_PROJECTILE))
    {
        1.5
    } else {
        return 0;
    };
    ((6 + level * level) as f32 * type_modifier / 3.0).floor() as i32
}

/// 1.8 `EnchantmentHelper.getEnchantmentModifierDamage` + `applyPotionDamageCalculations`.
///
/// The summed protection factor is capped at 25, rolled to 50-100 % (`roll` in `0..=total/2`),
/// capped at 20 and reduces the damage by 4 % per point.
#[must_use]
pub fn protection_absorb(damage: f32, total_epf: i32, roll: Option<i32>) -> f32 {
    let total = total_epf.clamp(0, 25);
    let factor = roll
        .map_or(total, |roll| ((total + 1) >> 1) + roll.clamp(0, total >> 1))
        .min(20);
    if factor <= 0 {
        return damage;
    }
    damage * (25 - factor) as f32 / 25.0
}

/// The random part of [`protection_absorb`]: `rand.nextInt((total >> 1) + 1)`.
#[must_use]
pub fn protection_roll(total_epf: i32) -> i32 {
    let half = total_epf.clamp(0, 25) >> 1;
    (rand::random::<u32>() % (half as u32 + 1)) as i32
}

/// 1.8 `EntityPlayer.damageEntity` sword blocking: `(1 + damage) / 2`.
#[must_use]
pub fn blocked_damage(damage: f32) -> f32 {
    if damage > 0.0 {
        f32::midpoint(1.0, damage)
    } else {
        damage
    }
}

/// One `Random.nextGaussian` sample (Box-Muller).
fn gaussian() -> f64 {
    let u1 = rand::random::<f64>().max(f64::MIN_POSITIVE);
    let u2 = rand::random::<f64>();
    (-2.0 * u1.ln()).sqrt() * (std::f64::consts::TAU * u2).cos()
}

/// 1.8 `EntityArrow(world, shooter, velocity)`: spawn position and motion of a bow arrow.
///
/// The shooter's own movement is not inherited.
/// The spawn point is on the look ray like 26.x (0.1 below the eyes): 1.8 also moved it 0.16 to the
/// shooter's right, so every arrow landed 0.16 right of the crosshair. That offset is left out on
/// purpose; launch speed, spread and flight stay 1.8.
#[must_use]
pub fn arrow_launch(
    feet: Vector3<f64>,
    eye_height: f64,
    yaw: f32,
    pitch: f32,
    force: f32,
    spread: Option<(f64, f64, f64)>,
) -> (Vector3<f64>, Vector3<f64>) {
    let yaw_rad = f64::from(yaw.to_radians());
    let pitch_rad = f64::from(pitch.to_radians());
    let position = Vector3::new(feet.x, feet.y + eye_height - 0.1, feet.z);
    let direction = Vector3::new(
        -yaw_rad.sin() * pitch_rad.cos(),
        -pitch_rad.sin(),
        yaw_rad.cos() * pitch_rad.cos(),
    )
    .normalize();
    let (gx, gy, gz) = spread.unwrap_or_else(|| (gaussian(), gaussian(), gaussian()));
    let speed = f64::from(force) * ARROW_SPEED;
    let velocity = Vector3::new(
        (direction.x + gx * LAUNCH_INACCURACY) * speed,
        (direction.y + gy * LAUNCH_INACCURACY) * speed,
        (direction.z + gz * LAUNCH_INACCURACY) * speed,
    );
    (position, velocity)
}

/// 1.8 eye height of a player (sneaking lowers it by 0.08).
#[must_use]
pub const fn player_eye_height(sneaking: bool) -> f64 {
    if sneaking { 1.54 } else { 1.62 }
}

/// Whether the block at the entity's feet can be climbed (ladder, vines, scaffolding ...).
fn is_climbing(entity: &Entity) -> bool {
    let world = entity.world.load();
    let block: &Block = world.get_block(&entity.block_pos.load());
    block.has_tag(&tag::Block::MINECRAFT_CLIMBABLE)
}

impl Player {
    /// Motion the 1.8 server would have for this player, the knockback input (`knockBack` halves it).
    /// It is the server's own copy, not the movement of the client: it only knows the jumps (see [`server_jump`]),
    /// and 1.8 restores it after a melee hit has sent the knockback, so consecutive hits do not stack.
    pub fn legacy_motion(&self) -> Vector3<f64> {
        self.legacy_server_motion.load()
    }

    /// Advances [`Self::legacy_motion`] by one tick (1.8: one movement packet): [`server_motion_tick`] with the
    /// previous ground state, then [`server_jump`] if the player left the ground going up.
    pub fn tick_legacy_motion(&self) {
        let entity = &self.living_entity.entity;
        let on_ground = entity.on_ground.load(Ordering::Relaxed);
        let was_on_ground = self.legacy_on_ground.swap(on_ground, Ordering::Relaxed);
        let mut motion = server_motion_tick(self.legacy_server_motion.load(), was_on_ground);
        let (delta, at_tick) = self.legacy_movement.load();
        let moved_up = entity
            .tick_count
            .load(Ordering::Relaxed)
            .saturating_sub(at_tick)
            <= 1
            && delta.y > 0.0;
        if was_on_ground && !on_ground && moved_up {
            let jump_boost = self
                .living_entity
                .get_effect(&StatusEffect::JUMP_BOOST)
                .map_or(0, |e| i32::from(e.amplifier) + 1);
            motion = server_jump(motion, entity.yaw.load(), entity.is_sprinting(), jump_boost);
        }
        self.legacy_server_motion.store(motion);
    }

    /// Records the movement of one client position packet for [`Self::legacy_motion`].
    pub fn record_legacy_movement(&self, delta: Vector3<f64>) {
        let tick = self.living_entity.entity.tick_count.load(Ordering::Relaxed);
        self.legacy_movement.store((delta, tick));
    }

    /// Whether the player is blocking with a sword (using a sword item, 1.8 style).
    pub fn is_legacy_sword_blocking(&self) -> bool {
        let item_in_use = self
            .living_entity
            .item_in_use
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        item_in_use.as_ref().is_some_and(ItemStack::is_sword)
    }

    /// 1.8 `EntityPlayer.attackTargetEntityWithCurrentItem`.
    #[expect(clippy::too_many_lines)]
    pub fn legacy_attack(&self, victim: &Arc<dyn EntityBase>, cfg: &LegacyCombatConfig) {
        let world = self.world();
        let attacker = &self.living_entity.entity;
        // 1.8 `NetHandlerPlayServer.processUseEntity`: attacks from 6 blocks (feet to feet) or more are ignored
        if attacker
            .pos
            .load()
            .squared_distance_to_vec(&victim.get_entity().pos.load())
            >= MAX_ATTACK_DISTANCE_SQUARED
        {
            return;
        }
        let item_stack = self.inventory().held_item();
        if !item_stack.is_empty() {
            self.increment_stat(
                crate::entity::player::statistics::StatisticCategory::Used,
                item_stack.item.id as i32,
                1,
            );
        }

        let strength = self
            .living_entity
            .get_effect(&StatusEffect::STRENGTH)
            .map_or(0, |e| i32::from(e.amplifier) + 1);
        let weakness = self
            .living_entity
            .get_effect(&StatusEffect::WEAKNESS)
            .map_or(0, |e| i32::from(e.amplifier) + 1);
        let critical = is_critical(
            self.living_entity.fall_distance.load(),
            attacker.on_ground.load(Ordering::Relaxed),
            is_climbing(attacker),
            attacker.is_in_water(),
            self.living_entity.has_effect(&StatusEffect::BLINDNESS),
            attacker.has_vehicle(),
        );
        let sharpness = item_stack.get_enchantment_level(&Enchantment::SHARPNESS);
        let damage = melee_damage(
            weapon_damage(item_stack.item),
            strength,
            weakness,
            critical,
            sharpness,
        );
        let mut knockback_level = item_stack.get_enchantment_level(&Enchantment::KNOCKBACK);
        if attacker.is_sprinting() {
            knockback_level += 1;
        }
        let fire_aspect = item_stack.get_enchantment_level(&Enchantment::FIRE_ASPECT);
        self.last_attacked_ticks.store(0, Ordering::Relaxed);

        let victim_entity = victim.get_entity();
        // 1.8 keeps the victim's motion from before the hit and restores it once the velocity packet is out; the
        // knockback starts from that motion (an extra push without base knockback too)
        let victim_motion = victim.get_player().map(|player| {
            let motion = player.legacy_motion();
            victim_entity.velocity.store(motion);
            motion
        });
        let hit = victim.damage_with_context(
            victim.as_ref(),
            damage,
            DamageType::PLAYER_ATTACK,
            None,
            Some(self),
            Some(self),
        );
        if !hit {
            return;
        }

        if victim.get_living_entity().is_some() {
            if knockback_level > 0 {
                let push = extra_knockback(attacker.yaw.load(), knockback_level, cfg);
                victim_entity
                    .velocity
                    .store(victim_entity.velocity.load() + push);
                victim_entity.velocity_dirty.store(true, Ordering::SeqCst);
                attacker.set_sprinting(false);
            }
            // 1.8 sends the victim's new motion right away (S12PacketEntityVelocity) and restores the old one
            if let (Some(player), Some(motion)) = (victim.get_player(), victim_motion)
                && victim_entity.velocity_dirty.swap(false, Ordering::SeqCst)
            {
                victim_entity.send_velocity();
                victim_entity.velocity.store(motion);
                player.legacy_server_motion.store(motion);
            }
            if fire_aspect > 0 {
                victim_entity.set_on_fire_for_ticks((fire_aspect * 4 * 20) as u32);
            }
        }

        if critical {
            world.broadcast_packet_all(&CEntityAnimation::new(
                victim_entity.entity_id.into(),
                Animation::CriticalEffect,
            ));
        }
        if sharpness > 0 {
            world.broadcast_packet_all(&CEntityAnimation::new(
                victim_entity.entity_id.into(),
                Animation::MagicCriticaleffect,
            ));
        }

        self.living_entity
            .last_attacking_id
            .store(victim_entity.entity_id, Ordering::Relaxed);
        self.damage_held_item(Self::legacy_weapon_durability_cost(&item_stack));
        // 1.8 `addExhaustion(0.3F)` per hit
        self.add_exhaustion(0.3);
    }

    fn legacy_weapon_durability_cost(stack: &ItemStack) -> i32 {
        // 1.8 swords lose 1, tools 2 (`ItemTool.hitEntity`), everything else nothing
        if stack.is_sword() {
            1
        } else if weapon_damage(stack.item) > 1.0 {
            2
        } else {
            0
        }
    }

    /// Sets the 1.8 attack speed so modern clients never show or apply an attack cooldown.
    pub fn apply_legacy_attack_speed(&self, cfg: &LegacyCombatConfig) {
        let attribute = &pumpkin_data::attributes::Attributes::ATTACK_SPEED;
        if (self.living_entity.get_attribute_base(attribute) - cfg.attack_speed).abs()
            > f64::EPSILON
        {
            self.living_entity
                .set_attribute_base(attribute, cfg.attack_speed);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cfg() -> LegacyCombatConfig {
        LegacyCombatConfig {
            enabled: true,
            ..LegacyCombatConfig::default()
        }
    }

    fn close(a: f64, b: f64) -> bool {
        (a - b).abs() < 1e-9
    }

    #[test]
    fn weapon_damage_uses_1_8_totals() {
        assert!((weapon_damage(&Item::WOODEN_SWORD) - 5.0).abs() < f32::EPSILON);
        assert!((weapon_damage(&Item::IRON_SWORD) - 7.0).abs() < f32::EPSILON);
        assert!((weapon_damage(&Item::DIAMOND_SWORD) - 8.0).abs() < f32::EPSILON);
        assert!((weapon_damage(&Item::STONE_PICKAXE) - 4.0).abs() < f32::EPSILON);
        assert!((weapon_damage(&Item::STICK) - 1.0).abs() < f32::EPSILON);
    }

    #[test]
    fn melee_damage_follows_1_8_order() {
        // iron sword, Sharpness II: 7 + 2.5
        assert!((melee_damage(7.0, 0, 0, false, 2) - 9.5).abs() < 1e-5);
        // critical only multiplies the base, not sharpness: 7 * 1.5 + 2.5
        assert!((melee_damage(7.0, 0, 0, true, 2) - 13.0).abs() < 1e-5);
        // Strength I is +130 %: wooden sword 5 -> 11.5
        assert!((melee_damage(5.0, 1, 0, false, 0) - 11.5).abs() < 1e-5);
        // fist with weakness I: 1 * 0.5
        assert!((melee_damage(1.0, 0, 1, false, 0) - 0.5).abs() < 1e-5);
    }

    #[test]
    fn critical_needs_falling_but_allows_sprinting() {
        assert!(is_critical(0.3, false, false, false, false, false));
        assert!(!is_critical(0.0, false, false, false, false, false));
        assert!(!is_critical(0.3, true, false, false, false, false));
        assert!(!is_critical(0.3, false, true, false, false, false));
        assert!(!is_critical(0.3, false, false, true, false, false));
    }

    #[test]
    fn base_knockback_of_a_resting_player() {
        // victim stands, attacker 1 block to the -x side: pushed to +x with 0.4, up with 0.36
        let motion = Vector3::new(0.0, RESTING_MOTION_Y, 0.0);
        let v = base_knockback(motion, -1.0, 0.0, &cfg());
        assert!(close(v.x, 0.4) && close(v.z, 0.0));
        assert!(close(v.y, RESTING_MOTION_Y / 2.0 + 0.4));
        // vertical motion is capped at 0.4
        let flying = base_knockback(Vector3::new(0.0, 0.5, 0.0), -1.0, 0.0, &cfg());
        assert!(close(flying.y, 0.4));
        // existing motion is halved (combos)
        let moving = base_knockback(Vector3::new(0.3, 0.0, 0.0), -1.0, 0.0, &cfg());
        assert!(close(moving.x, 0.55));
    }

    #[test]
    fn extra_knockback_points_along_attacker_yaw() {
        // yaw 0 looks to +z
        let v = extra_knockback(0.0, 1, &cfg());
        assert!(close(v.x, 0.0) && close(v.y, 0.1) && close(v.z, 0.5));
        let v = extra_knockback(90.0, 2, &cfg());
        assert!((v.x + 1.0).abs() < 1e-6 && v.z.abs() < 1e-6);
    }

    /// Vertical knockback on a player hit `n` client ticks after it jumped, as a real 1.8.9 server sent it
    /// (mineflayer 1.8.9 bots, velocity packets in 1/8000): the cap of 0.4 while the jump still rises, then less.
    #[test]
    fn knockback_on_a_jumping_player_matches_1_8_9() {
        let rest = server_motion_tick(Vector3::new(0.0, 0.0, 0.0), true);
        let measured = [(1, 0.4), (2, 0.4), (3, 0.4), (4, 0.4), (6, 0.4), (8, 0.322)];
        for (n, expected) in measured {
            // the packet that leaves the ground: physics on the ground, then the server side jump
            let mut motion = server_jump(server_motion_tick(rest, true), 0.0, false, 0);
            for _ in 1..n {
                motion = server_motion_tick(motion, false);
            }
            let sent = (base_knockback(motion, -1.0, 0.0, &cfg()).y * 8000.0).floor() / 8000.0;
            assert!(
                (sent - expected).abs() < 0.001,
                "{n} ticks: {sent} instead of {expected}"
            );
        }
        // standing: 0.3608 (1.8.9 sent 0.361)
        assert!(close(base_knockback(rest, -1.0, 0.0, &cfg()).y, 0.360_8));
    }

    #[test]
    fn server_motion_decays_and_sprint_jumps_push_along_the_yaw() {
        assert!(close(
            server_motion_tick(Vector3::new(0.0, 0.0, 0.0), true).y,
            RESTING_MOTION_Y
        ));
        // horizontal motion decays (ground 0.546, air 0.91) and snaps to 0 below 0.005
        let pushed = server_motion_tick(Vector3::new(0.4, 0.0, 0.004), true);
        assert!(close(pushed.x, 0.4 * GROUND_FRICTION) && pushed.z == 0.0);
        assert!(close(
            server_motion_tick(Vector3::new(0.4, 0.2, 0.0), false).x,
            0.4 * AIR_FRICTION
        ));
        // yaw 0 looks to +z; Jump Boost II adds 0.2
        let jump = server_jump(Vector3::new(0.0, RESTING_MOTION_Y, 0.0), 0.0, true, 2);
        assert!(close(jump.x, 0.0) && close(jump.y, 0.62) && close(jump.z, 0.2));
    }

    #[test]
    fn armour_and_protection_follow_1_8() {
        // full iron (15 points): 10 damage -> 4
        assert!((armour_absorb(10.0, 15.0) - 4.0).abs() < 1e-5);
        // Protection II on one piece: floor((6+4)*0.75/3) = 2
        assert_eq!(
            protection_epf(&Enchantment::PROTECTION, 2, &DamageType::PLAYER_ATTACK),
            2
        );
        // Projectile Protection does nothing against melee
        assert_eq!(
            protection_epf(
                &Enchantment::PROJECTILE_PROTECTION,
                4,
                &DamageType::PLAYER_ATTACK
            ),
            0
        );
        // total 10, roll 0 -> factor 5 -> 20 %
        assert!((protection_absorb(10.0, 10, Some(0)) - 8.0).abs() < 1e-5);
        // total 10, max roll 5 -> factor 10 -> 40 %
        assert!((protection_absorb(10.0, 10, Some(5)) - 6.0).abs() < 1e-5);
        // capped at 20
        assert!((protection_absorb(10.0, 25, Some(12)) - 2.0).abs() < 1e-5);
    }

    #[test]
    fn sword_blocking_halves_after_adding_one() {
        assert!((blocked_damage(7.0) - 4.0).abs() < 1e-5);
        assert!(blocked_damage(0.0).abs() < 1e-5);
    }

    #[test]
    fn arrow_launch_has_no_momentum_and_starts_on_the_look_ray() {
        // yaw 0 (towards +z), level: spawn 0.1 below the eyes, no sideways offset
        let (pos, vel) = arrow_launch(
            Vector3::new(0.0, 100.0, 0.0),
            1.62,
            0.0,
            0.0,
            1.0,
            Some((0.0, 0.0, 0.0)),
        );
        assert!(close(pos.x, 0.0) && close(pos.y, 101.52) && close(pos.z, 0.0));
        assert!(close(vel.x, 0.0) && close(vel.y, 0.0) && close(vel.z, 3.0));
        // half drawn: half speed
        let (_, vel) = arrow_launch(
            Vector3::new(0.0, 100.0, 0.0),
            1.62,
            0.0,
            -90.0,
            0.5,
            Some((0.0, 0.0, 0.0)),
        );
        assert!(close(vel.y, 1.5));
        // spread is one Gaussian step of 0.0075 per unit
        let (_, vel) = arrow_launch(
            Vector3::new(0.0, 100.0, 0.0),
            1.62,
            0.0,
            0.0,
            1.0,
            Some((1.0, 0.0, 0.0)),
        );
        assert!(close(vel.x, LAUNCH_INACCURACY * 3.0));
    }
}
