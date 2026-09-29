use crate::entity::ai::control::{Control, MoveControlTrait};
use crate::entity::mob::Mob;
use pumpkin_data::attributes::Attributes;
use pumpkin_data::tag::{self, Taggable};
use pumpkin_util::math::vector3::Vector3;
use std::sync::atomic::Ordering;

pub const MIN_SPEED_SQR: f64 = 2.500_000_3E-7;

#[derive(Default, Clone, Copy, PartialEq, Eq)]
pub enum Operation {
    #[default]
    Wait,
    MoveTo,
    Strafe,
    Jumping,
}

pub struct MoveControl {
    pub wanted_x: f64,
    pub wanted_y: f64,
    pub wanted_z: f64,
    pub speed_modifier: f64,
    pub strafe_forwards: f32,
    pub strafe_right: f32,
    pub operation: Operation,
}

impl Default for MoveControl {
    fn default() -> Self {
        Self {
            wanted_x: 0.0,
            wanted_y: 0.0,
            wanted_z: 0.0,
            speed_modifier: 0.0,
            strafe_forwards: 0.0,
            strafe_right: 0.0,
            operation: Operation::Wait,
        }
    }
}

impl Control for MoveControl {}

impl MoveControlTrait for MoveControl {
    fn tick(&mut self, mob: &dyn Mob) {
        let mob_entity = mob.get_mob_entity();
        let living_entity = &mob_entity.living_entity;
        let entity = &living_entity.entity;
        match self.operation {
            Operation::Strafe => {
                // TODO: is_walkable check
                let movement_speed = living_entity.get_attribute_value(&Attributes::MOVEMENT_SPEED);
                living_entity.set_speed(self.speed_modifier * movement_speed);
                living_entity.movement_input.store(Vector3::new(
                    f64::from(self.strafe_right),
                    0.0,
                    f64::from(self.strafe_forwards),
                ));
                self.operation = Operation::Wait;
            }
            Operation::MoveTo => {
                self.operation = Operation::Wait;
                let pos = entity.pos.load();
                let xd = self.wanted_x - pos.x;
                let zd = self.wanted_z - pos.z;
                let yd = self.wanted_y - pos.y;
                let dd = xd * xd + yd * yd + zd * zd;

                if dd < MIN_SPEED_SQR {
                    living_entity.set_zza(0.0);
                    return;
                }

                let y_rot_d = (zd.atan2(xd).to_degrees() as f32) - 90.0;
                entity
                    .yaw
                    .store(self.change_angle(entity.yaw.load(), y_rot_d, 90.0));

                let movement_speed = living_entity.get_attribute_value(&Attributes::MOVEMENT_SPEED);
                living_entity.set_speed(self.speed_modifier * movement_speed);

                let block_pos = entity.block_pos.load();
                let world = entity.world.load();
                let (block, state) = world.get_block_and_state(&block_pos);
                // The top of the block the mob stands in; stairs, slabs and the like.
                let shape_top = state
                    .get_block_collision_shapes_at(&block_pos)
                    .map(|shape| shape.max.y)
                    .reduce(f64::max);

                let step_height = living_entity.get_attribute_value(&Attributes::STEP_HEIGHT);
                let width = f64::from(entity.entity_dimension.load().width);
                if yd > step_height && xd * xd + zd * zd < 1.0f64.max(width)
                    || shape_top.is_some_and(|top| {
                        pos.y < top + f64::from(block_pos.0.y)
                            && !block.has_tag(&tag::Block::MINECRAFT_DOORS)
                            && !block.has_tag(&tag::Block::MINECRAFT_FENCES)
                    })
                {
                    mob_entity
                        .jump_control
                        .lock()
                        .unwrap_or_else(std::sync::PoisonError::into_inner)
                        .jump();
                    self.operation = Operation::Jumping;
                }
            }
            Operation::Jumping => {
                let movement_speed = living_entity.get_attribute_value(&Attributes::MOVEMENT_SPEED);
                living_entity.set_speed(self.speed_modifier * movement_speed);

                let in_liquid = entity.touching_water.load(Ordering::Relaxed)
                    || entity.touching_lava.load(Ordering::Relaxed);
                if entity.on_ground.load(Ordering::Relaxed) || in_liquid {
                    self.operation = Operation::Wait;
                }
            }
            Operation::Wait => living_entity.set_zza(0.0),
        }
    }

    fn set_wanted_position(&mut self, x: f64, y: f64, z: f64, speed_modifier: f64) {
        self.wanted_x = x;
        self.wanted_y = y;
        self.wanted_z = z;
        self.speed_modifier = speed_modifier;
        if self.operation != Operation::Jumping {
            self.operation = Operation::MoveTo;
        }
    }

    fn strafe(&mut self, forwards: f32, right: f32) {
        self.operation = Operation::Strafe;
        self.strafe_forwards = forwards;
        self.strafe_right = right;
        self.speed_modifier = 0.25;
    }

    fn has_wanted(&self) -> bool {
        self.operation == Operation::MoveTo
    }
}

impl MoveControl {
    #[must_use]
    pub fn has_wanted(&self) -> bool {
        self.operation == Operation::MoveTo
    }

    #[must_use]
    pub const fn get_speed_modifier(&self) -> f64 {
        self.speed_modifier
    }

    pub fn set_wanted_position(&mut self, x: f64, y: f64, z: f64, speed_modifier: f64) {
        self.wanted_x = x;
        self.wanted_y = y;
        self.wanted_z = z;
        self.speed_modifier = speed_modifier;
        if self.operation != Operation::Jumping {
            self.operation = Operation::MoveTo;
        }
    }

    pub const fn strafe(&mut self, forwards: f32, right: f32) {
        self.operation = Operation::Strafe;
        self.strafe_forwards = forwards;
        self.strafe_right = right;
        self.speed_modifier = 0.25;
    }
}
