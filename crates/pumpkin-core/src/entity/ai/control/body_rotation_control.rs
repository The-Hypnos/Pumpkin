use crate::entity::ai::control::Control;
use crate::entity::mob::MobEntity;
use pumpkin_util::math::rotate_if_necessary;

pub struct BodyRotationControl {
    head_stable_time: i32,
    last_stable_y_head_rot: f32,
}

impl Default for BodyRotationControl {
    fn default() -> Self {
        Self::new()
    }
}

impl BodyRotationControl {
    #[must_use]
    pub const fn new() -> Self {
        Self {
            head_stable_time: 0,
            last_stable_y_head_rot: 0.0,
        }
    }

    pub fn client_tick(&mut self, mob: &MobEntity, max_head_rotation: f32) {
        let entity = &mob.living_entity.entity;

        if Self::is_moving(mob) {
            entity.body_yaw.store(entity.yaw.load());
            Self::rotate_head_if_necessary(mob, max_head_rotation);
            self.last_stable_y_head_rot = entity.head_yaw.load();
            self.head_stable_time = 0;
        } else if Self::not_carrying_mob_passengers(mob) {
            if (entity.head_yaw.load() - self.last_stable_y_head_rot).abs() > 15.0 {
                self.head_stable_time = 0;
                self.last_stable_y_head_rot = entity.head_yaw.load();
                Self::rotate_body_if_necessary(mob, max_head_rotation);
            } else {
                self.head_stable_time += 1;
                if self.head_stable_time > 10 {
                    self.rotate_head_towards_front(mob, max_head_rotation);
                }
            }
        }
    }

    fn rotate_body_if_necessary(mob: &MobEntity, max_head_rotation: f32) {
        let entity = &mob.living_entity.entity;
        entity.body_yaw.store(rotate_if_necessary(
            entity.body_yaw.load(),
            entity.head_yaw.load(),
            max_head_rotation,
        ));
    }

    fn rotate_head_if_necessary(mob: &MobEntity, max_head_rotation: f32) {
        let entity = &mob.living_entity.entity;
        entity.head_yaw.store(rotate_if_necessary(
            entity.head_yaw.load(),
            entity.body_yaw.load(),
            max_head_rotation,
        ));
    }

    fn rotate_head_towards_front(&self, mob: &MobEntity, max_head_rotation: f32) {
        let time_since_starting = self.head_stable_time - 10;
        let face_forward_fraction = (time_since_starting as f32 / 10.0).clamp(0.0, 1.0);
        let angle_remaining = max_head_rotation * (1.0 - face_forward_fraction);
        let entity = &mob.living_entity.entity;
        entity.body_yaw.store(rotate_if_necessary(
            entity.body_yaw.load(),
            entity.head_yaw.load(),
            angle_remaining,
        ));
    }

    fn not_carrying_mob_passengers(mob: &MobEntity) -> bool {
        mob.living_entity
            .entity
            .passengers
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .first()
            .is_none_or(|passenger| passenger.get_mob().is_none())
    }

    fn is_moving(mob: &MobEntity) -> bool {
        let entity = &mob.living_entity.entity;
        let delta = entity.pos.load() - entity.last_pos.load();
        delta.x * delta.x + delta.z * delta.z > 2.500_000_3e-7
    }
}

impl Control for BodyRotationControl {}
