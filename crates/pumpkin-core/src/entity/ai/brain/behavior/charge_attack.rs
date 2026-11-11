use std::sync::Arc;

use pumpkin_data::attributes::Attributes;
use pumpkin_data::damage::DamageType;
use pumpkin_data::effect::StatusEffect;
use pumpkin_data::sound::Sound;
use pumpkin_util::math::vector3::Vector3;

use super::super::BrainTick;
use super::super::memory::{MemoryModuleId, MemoryStatus, types};
use super::timed::Behavior;
use crate::enchantment::EnchantmentHelper;
use crate::entity::EntityBase;
use crate::entity::ai::target_predicate::TargetPredicate;

const ENTRY_CONDITIONS: [(MemoryModuleId, MemoryStatus); 2] = [
    (types::CHARGE_COOLDOWN_TICKS.id(), MemoryStatus::ValueAbsent),
    (types::ATTACK_TARGET.id(), MemoryStatus::ValuePresent),
];

pub struct ChargeAttack {
    time_between_attacks: i32,
    charge_targeting: TargetPredicate,
    speed: f32,
    knockback_force: f32,
    max_charge_distance: f64,
    max_target_detection_distance: f64,
    charge_sound: Sound,
    charge_velocity_vector: Vector3<f64>,
    start_position: Vector3<f64>,
}

impl ChargeAttack {
    #[must_use]
    pub const fn new(
        time_between_attacks: i32,
        charge_targeting: TargetPredicate,
        speed: f32,
        knockback_force: f32,
        max_charge_distance: f64,
        max_target_detection_distance: f64,
        charge_sound: Sound,
    ) -> Self {
        Self {
            time_between_attacks,
            charge_targeting,
            speed,
            knockback_force,
            max_charge_distance,
            max_target_detection_distance,
            charge_sound,
            charge_velocity_vector: Vector3::new(0.0, 0.0, 0.0),
            start_position: Vector3::new(0.0, 0.0, 0.0),
        }
    }

    fn deal_damage_to_target(tick: &BrainTick<'_>, target: &dyn EntityBase) {
        let body = tick.mob;
        let damage = body
            .get_mob_entity()
            .living_entity
            .get_attribute_value(&Attributes::ATTACK_DAMAGE) as f32;
        if target.damage_with_context(
            target,
            damage,
            DamageType::MOB_ATTACK,
            None,
            Some(body),
            Some(body),
        ) {
            EnchantmentHelper::on_post_attack(
                body,
                target,
                &body.get_mob_entity().living_entity.held_item(body),
                DamageType::MOB_ATTACK,
            );
        }
    }

    fn deal_knock_back(&self, tick: &BrainTick<'_>, target: &dyn EntityBase) {
        let living = &tick.mob.get_mob_entity().living_entity;
        let effect_level = |effect| {
            living
                .get_effect(effect)
                .map_or(0, |effect| i32::from(effect.amplifier) + 1)
        };
        let speed_boost_power = 0.25
            * (effect_level(&StatusEffect::SPEED) - effect_level(&StatusEffect::SLOWNESS)) as f32;
        let speed_factor = (self.speed
            * living.get_attribute_value(&Attributes::MOVEMENT_SPEED) as f32)
            .clamp(0.2, 2.0)
            + speed_boost_power;
        let knockback = speed_factor * self.knockback_force;
        if knockback > 0.0 {
            let damage = living.get_attribute_value(&Attributes::ATTACK_DAMAGE) as f32;
            crate::entity::combat::handle_knockback(
                &living.entity,
                target,
                f64::from(knockback) * 2.0,
                damage,
            );
        }
    }
}

impl Behavior for ChargeAttack {
    fn entry_conditions(&self) -> &[(MemoryModuleId, MemoryStatus)] {
        &ENTRY_CONDITIONS
    }

    fn check_extra_start_conditions(&mut self, tick: &mut BrainTick<'_>) -> bool {
        tick.state.has_memory_value(types::ATTACK_TARGET.id())
    }

    fn can_still_use(&mut self, tick: &mut BrainTick<'_>) -> bool {
        let Some(target) = tick.state.get(types::ATTACK_TARGET) else {
            return false;
        };
        if tick.mob.is_tamed() {
            return false;
        }
        let entity = tick.mob.get_entity();
        let position = entity.pos.load();
        position.squared_distance_to_vec(&self.start_position)
            < self.max_charge_distance * self.max_charge_distance
            && target
                .get_entity()
                .pos
                .load()
                .squared_distance_to_vec(&position)
                < self.max_target_detection_distance * self.max_target_detection_distance
            && entity.has_line_of_sight(target.get_entity())
            && !tick
                .state
                .has_memory_value(types::CHARGE_COOLDOWN_TICKS.id())
    }

    fn start(&mut self, tick: &mut BrainTick<'_>) {
        let Some(target) = tick.state.get(types::ATTACK_TARGET).cloned() else {
            return;
        };
        let position = tick.mob.get_entity().pos.load();
        self.start_position = position;
        let direction = (target.get_entity().pos.load() - position).normalize();
        self.charge_velocity_vector = direction * f64::from(self.speed);
        if self.can_still_use(tick) {
            tick.mob.play_entity_sound(self.charge_sound, 1.0, 1.0);
        }
    }

    fn tick(&mut self, tick: &mut BrainTick<'_>) {
        let Some(target) = tick.state.get(types::ATTACK_TARGET).cloned() else {
            return;
        };
        let mob_entity = tick.mob.get_mob_entity();
        mob_entity.look_at_entity(target.as_ref(), 360.0, 360.0);
        let entity = &mob_entity.living_entity.entity;
        entity.velocity.store(self.charge_velocity_vector);
        let bounding_box = entity.bounding_box.load();
        let colliding: Option<Arc<dyn EntityBase>> = tick
            .world
            .get_entities_at_box(&bounding_box)
            .into_iter()
            .chain(
                tick.world
                    .get_players_at_box(&bounding_box)
                    .into_iter()
                    .map(|player| player as Arc<dyn EntityBase>),
            )
            .find(|other| {
                other.get_living_entity().is_some()
                    && self
                        .charge_targeting
                        .test(tick.world, Some(tick.mob), other.as_ref())
            });
        let Some(closest_attack_target) = colliding else {
            return;
        };
        if tick.mob.has_passenger(closest_attack_target.as_ref()) {
            return;
        }
        Self::deal_damage_to_target(tick, closest_attack_target.as_ref());
        self.deal_knock_back(tick, closest_attack_target.as_ref());
        self.stop(tick, false);
    }

    fn stop(&mut self, tick: &mut BrainTick<'_>, _timed_out: bool) {
        tick.state
            .set(types::CHARGE_COOLDOWN_TICKS, self.time_between_attacks);
        tick.state.erase(types::ATTACK_TARGET.id());
    }

    fn debug_name(&self) -> &'static str {
        "ChargeAttack"
    }
}
