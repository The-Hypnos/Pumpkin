use std::sync::Arc;

use pumpkin_util::math::position::BlockPos;
use pumpkin_util::math::vector3::Vector3;
use pumpkin_util::math::{cos, sin, wrap_degrees};
use rand::RngExt;

use super::super::BrainTick;
use super::super::memory::position_tracker::{BlockPosTracker, PositionTracker};
use super::super::memory::{MemoryModuleId, MemoryStatus, types};
use super::timed::Behavior;

const DEGREES_TO_RADIANS: f32 = std::f32::consts::PI / 180.0;

/// Vanilla `RandomLookAround`: glances somewhere nearby, then waits out a gaze cooldown.
pub struct RandomLookAround {
    conditions: [(MemoryModuleId, MemoryStatus); 2],
    interval: (i32, i32),
    max_yaw: f32,
    min_pitch: f32,
    pitch_range: f32,
}

impl RandomLookAround {
    #[must_use]
    pub const fn new(interval: (i32, i32), max_yaw: f32, min_pitch: f32, max_pitch: f32) -> Self {
        Self {
            conditions: [
                (types::LOOK_TARGET.id(), MemoryStatus::ValueAbsent),
                (types::GAZE_COOLDOWN_TICKS.id(), MemoryStatus::ValueAbsent),
            ],
            interval,
            max_yaw,
            min_pitch,
            pitch_range: max_pitch - min_pitch,
        }
    }
}

/// Vanilla `Vec3.directionFromRotation`.
#[must_use]
pub fn direction_from_rotation(pitch: f32, yaw: f32) -> Vector3<f64> {
    let y_cos = cos(-yaw * DEGREES_TO_RADIANS - std::f32::consts::PI);
    let y_sin = sin(-yaw * DEGREES_TO_RADIANS - std::f32::consts::PI);
    let x_cos = -cos(-pitch * DEGREES_TO_RADIANS);
    let x_sin = sin(-pitch * DEGREES_TO_RADIANS);
    Vector3::new(
        f64::from(y_sin * x_cos),
        f64::from(x_sin),
        f64::from(y_cos * x_cos),
    )
}

impl Behavior for RandomLookAround {
    fn entry_conditions(&self) -> &[(MemoryModuleId, MemoryStatus)] {
        &self.conditions
    }

    fn start(&mut self, tick: &mut BrainTick<'_>) {
        let mut rng = tick.mob.get_random();
        let entity = tick.mob.get_entity();
        let pitch = rng
            .random::<f32>()
            .mul_add(self.pitch_range, self.min_pitch)
            .clamp(-90.0, 90.0);
        let rotation = wrap_degrees(
            (2.0 * rng.random::<f32>()).mul_add(self.max_yaw, entity.yaw.load()) - self.max_yaw,
        );
        let look_at = entity.get_eye_pos() + direction_from_rotation(pitch, rotation);
        tick.brain.set(
            types::LOOK_TARGET,
            Arc::new(BlockPosTracker::new(BlockPos::floored_v(look_at)))
                as Arc<dyn PositionTracker>,
        );
        let cooldown = rng.random_range(self.interval.0..=self.interval.1);
        tick.brain.set(types::GAZE_COOLDOWN_TICKS, cooldown);
    }

    fn debug_name(&self) -> &'static str {
        "RandomLookAround"
    }
}
