use std::sync::Arc;

use pumpkin_data::attributes::Attributes;
use pumpkin_data::damage::DamageType;
use pumpkin_data::effect::StatusEffect;
use pumpkin_data::sound::{Sound, SoundCategory};
use pumpkin_util::math::vector3::Vector3;
use pumpkin_util::math::{cos, sin};

use crate::entity::EntityBase;
use crate::entity::ai::target_predicate::TargetPredicate;
use crate::entity::combat::knockback_after_resistance;
use crate::entity::mob::Mob;

use super::super::BrainTick;
use super::super::memory::{MemoryModuleId, MemoryStatus, types};
use super::timed::Behavior;

const DEGREES_TO_RADIANS: f32 = std::f32::consts::PI / 180.0;

/// Vanilla `ChargeAttack`: dashes at the attack target, hurting and throwing what it hits.
pub struct ChargeAttack {
    conditions: [(MemoryModuleId, MemoryStatus); 2],
    time_between_attacks: i32,
    charge_targeting: TargetPredicate,
    speed: f32,
    knockback_force: f32,
    max_charge_distance: f64,
    max_target_detection_distance: f64,
    charge_sound: Sound,
    is_tame: fn(&dyn Mob) -> bool,
    charge_velocity: Vector3<f64>,
    start_position: Vector3<f64>,
}

impl ChargeAttack {
    #[must_use]
    #[expect(clippy::too_many_arguments, reason = "mirrors the vanilla constructor")]
    pub const fn new(
        time_between_attacks: i32,
        charge_targeting: TargetPredicate,
        speed: f32,
        knockback_force: f32,
        max_charge_distance: f64,
        max_target_detection_distance: f64,
        charge_sound: Sound,
        is_tame: fn(&dyn Mob) -> bool,
    ) -> Self {
        Self {
            conditions: [
                (types::CHARGE_COOLDOWN_TICKS.id(), MemoryStatus::ValueAbsent),
                (types::ATTACK_TARGET.id(), MemoryStatus::ValuePresent),
            ],
            time_between_attacks,
            charge_targeting,
            speed,
            knockback_force,
            max_charge_distance,
            max_target_detection_distance,
            charge_sound,
            is_tame,
            charge_velocity: Vector3::new(0.0, 0.0, 0.0),
            start_position: Vector3::new(0.0, 0.0, 0.0),
        }
    }

    fn effect_level(mob: &dyn Mob, effect: &'static StatusEffect) -> i32 {
        mob.get_mob_entity()
            .living_entity
            .get_effect(effect)
            .map_or(0, |effect| i32::from(effect.amplifier) + 1)
    }

    /// Vanilla `Entity.lookAt(target, 360, 360)`: faces the target's eyes at once.
    fn look_at(mob: &dyn Mob, target: &dyn EntityBase) {
        let entity = mob.get_entity();
        let from = entity.pos.load();
        let to = target.get_entity().pos.load();
        let dx = to.x - from.x;
        let dz = to.z - from.z;
        let dy = target.get_eye_pos().y - entity.get_eye_pos().y;
        let horizontal = dx.hypot(dz);
        let yaw = (dz.atan2(dx).to_degrees() as f32) - 90.0;
        let pitch = -(dy.atan2(horizontal).to_degrees() as f32);
        entity.set_rotation(yaw, pitch);
    }

    fn deal_damage_to_target(mob: &dyn Mob, target: &dyn EntityBase) {
        let damage = mob
            .get_mob_entity()
            .living_entity
            .get_attribute_value(&Attributes::ATTACK_DAMAGE) as f32;
        target.damage_with_context(
            target,
            damage,
            DamageType::MOB_ATTACK,
            None,
            Some(mob as &dyn EntityBase),
            Some(mob as &dyn EntityBase),
        );
    }

    /// `dealKnockBack` through vanilla `LivingEntity.causeExtraKnockback`.
    fn deal_knockback(&self, mob: &dyn Mob, target: &dyn EntityBase) {
        let living = &mob.get_mob_entity().living_entity;
        let speed_boost_power = 0.25
            * (Self::effect_level(mob, &StatusEffect::SPEED)
                - Self::effect_level(mob, &StatusEffect::SLOWNESS)) as f32;
        let movement_speed = living.get_attribute_value(&Attributes::MOVEMENT_SPEED) as f32;
        let speed_factor = (self.speed * movement_speed).clamp(0.2, 2.0) + speed_boost_power;
        let knockback = speed_factor * self.knockback_force;
        let Some(target_living) = target.get_living_entity() else {
            return;
        };
        if knockback <= 0.0 {
            return;
        }
        let yaw = mob.get_entity().yaw.load() * DEGREES_TO_RADIANS;
        let strength = knockback_after_resistance(
            f64::from(knockback),
            target_living.get_attribute_value(&Attributes::KNOCKBACK_RESISTANCE),
        );
        target
            .get_entity()
            .apply_knockback(strength, f64::from(sin(yaw)), f64::from(-cos(yaw)));
        let entity = mob.get_entity();
        entity
            .velocity
            .store(entity.velocity.load().multiply(0.6, 1.0, 0.6));
    }

    fn finish(&self, tick: &mut BrainTick<'_>) {
        tick.brain
            .set(types::CHARGE_COOLDOWN_TICKS, self.time_between_attacks);
        tick.brain.erase(types::ATTACK_TARGET.id());
    }
}

impl Behavior for ChargeAttack {
    fn entry_conditions(&self) -> &[(MemoryModuleId, MemoryStatus)] {
        &self.conditions
    }

    fn check_extra_start_conditions(&mut self, tick: &mut BrainTick<'_>) -> bool {
        tick.brain.has_memory_value(types::ATTACK_TARGET.id())
    }

    fn can_still_use(&mut self, tick: &BrainTick<'_>) -> bool {
        let Some(target) = tick.brain.get(types::ATTACK_TARGET) else {
            return false;
        };
        if (self.is_tame)(tick.mob) {
            return false;
        }
        let body_pos = tick.mob.get_entity().pos.load();
        if body_pos.squared_distance_to_vec(&self.start_position)
            >= self.max_charge_distance * self.max_charge_distance
        {
            return false;
        }
        if target
            .get_entity()
            .pos
            .load()
            .squared_distance_to_vec(&body_pos)
            >= self.max_target_detection_distance * self.max_target_detection_distance
        {
            return false;
        }
        tick.mob.has_line_of_sight(target.get_entity())
            && !tick
                .brain
                .has_memory_value(types::CHARGE_COOLDOWN_TICKS.id())
    }

    fn start(&mut self, tick: &mut BrainTick<'_>) {
        let Some(target) = tick.brain.get(types::ATTACK_TARGET).map(Arc::clone) else {
            return;
        };
        let body_pos = tick.mob.get_entity().pos.load();
        self.start_position = body_pos;
        let direction = (target.get_entity().pos.load() - body_pos).normalize();
        self.charge_velocity = direction * f64::from(self.speed);
        if self.can_still_use(tick) {
            tick.world.play_sound_fine(
                self.charge_sound,
                SoundCategory::Neutral,
                &body_pos,
                1.0,
                1.0,
            );
        }
    }

    fn tick(&mut self, tick: &mut BrainTick<'_>) {
        let Some(target) = tick.brain.get(types::ATTACK_TARGET).map(Arc::clone) else {
            return;
        };
        Self::look_at(tick.mob, target.as_ref());
        tick.mob.get_entity().set_velocity(self.charge_velocity);
        let body_box = tick.mob.get_entity().bounding_box.load();
        let hit = tick
            .world
            .entity_grid
            .load()
            .collect_in_box(&body_box)
            .into_iter()
            .find(|entity| {
                entity.get_living_entity().is_some()
                    && self
                        .charge_targeting
                        .test(tick.world, Some(tick.mob), entity.as_ref())
            });
        let Some(hit) = hit else {
            return;
        };
        if tick
            .mob
            .get_entity()
            .has_passenger(hit.get_entity().entity_id)
        {
            return;
        }
        Self::deal_damage_to_target(tick.mob, hit.as_ref());
        self.deal_knockback(tick.mob, hit.as_ref());
        self.finish(tick);
    }

    fn stop(&mut self, tick: &mut BrainTick<'_>) {
        self.finish(tick);
    }

    fn debug_name(&self) -> &'static str {
        "ChargeAttack"
    }
}
