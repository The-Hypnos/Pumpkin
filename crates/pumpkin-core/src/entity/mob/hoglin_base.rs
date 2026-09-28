use pumpkin_data::attributes::Attributes;
use pumpkin_data::damage::DamageType;
use pumpkin_util::math::vector3::Vector3;
use pumpkin_util::math::{cos, sin};
use rand::RngExt;

use crate::entity::EntityBase;
use crate::entity::ageable::is_baby;
use crate::entity::mob::Mob;

pub const ATTACK_ANIMATION_DURATION: i32 = 10;

/// Vanilla `HoglinBase.hurtAndThrowTarget`, shared by hoglins and zoglins.
pub fn hurt_and_throw_target(body: &dyn Mob, target: &dyn EntityBase) -> bool {
    let attack_damage = body
        .get_mob_entity()
        .living_entity
        .get_attribute_value(&Attributes::ATTACK_DAMAGE) as f32;
    let baby = is_baby(body);
    let actual_damage = if !baby && attack_damage as i32 > 0 {
        attack_damage / 2.0 + body.get_random().random_range(0..attack_damage as i32) as f32
    } else {
        attack_damage
    };

    let was_hurt = target.damage_with_context(
        target,
        actual_damage,
        DamageType::MOB_ATTACK,
        None,
        Some(body as &dyn EntityBase),
        Some(body as &dyn EntityBase),
    );
    if was_hurt && !baby {
        throw_target(body, target);
    }
    was_hurt
}

/// Vanilla `HoglinBase.throwTarget`.
pub fn throw_target(body: &dyn Mob, target: &dyn EntityBase) {
    let Some(target_living) = target.get_living_entity() else {
        return;
    };
    let knockback_power = body
        .get_mob_entity()
        .living_entity
        .get_attribute_value(&Attributes::ATTACK_KNOCKBACK);
    let knockback_resistance = target_living.get_attribute_value(&Attributes::KNOCKBACK_RESISTANCE);
    let effective_knockback_power = knockback_power - knockback_resistance;
    if effective_knockback_power <= 0.0 {
        return;
    }

    let body_pos = body.get_entity().pos.load();
    let target_pos = target.get_entity().pos.load();
    let mut rng = body.get_random();
    // Vanilla hands this to `Vec3.yRot`, which takes radians, so the spread is -10..=10 rad.
    let horizontal_push_angle = (rng.random_range(0..21) - 10) as f32;
    let horizontal_scale =
        effective_knockback_power * f64::from(rng.random::<f32>().mul_add(0.5, 0.2));
    let push = Vector3::new(target_pos.x - body_pos.x, 0.0, target_pos.z - body_pos.z).normalize()
        * horizontal_scale;
    let (angle_cos, angle_sin) = (
        f64::from(cos(horizontal_push_angle)),
        f64::from(sin(horizontal_push_angle)),
    );
    let vertical_scale = effective_knockback_power * f64::from(rng.random::<f32>()) * 0.5;
    target.get_entity().add_velocity(Vector3::new(
        push.x.mul_add(angle_cos, push.z * angle_sin),
        vertical_scale,
        push.z.mul_add(angle_cos, -(push.x * angle_sin)),
    ));
}
