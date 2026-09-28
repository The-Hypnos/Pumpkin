use std::sync::atomic::{AtomicI32, Ordering};

use pumpkin_data::sound::{Sound, SoundCategory};

use crate::entity::ageable::is_baby;
use crate::entity::mob::Mob;

/// Vanilla `Mob.makeSound`: full volume at the mob's `getVoicePitch`.
pub fn make_sound(mob: &dyn Mob, sound: Sound, category: SoundCategory) {
    let entity = mob.get_entity();
    let base_pitch = if is_baby(mob) { 1.5 } else { 1.0 };
    let pitch = (rand::random::<f32>() - rand::random::<f32>()).mul_add(0.2, base_pitch);
    entity
        .world
        .load()
        .play_sound_fine(sound, category, &entity.pos.load(), 1.0, pitch);
}

/// The ambient sound roll from vanilla `Mob.baseTick`, for mobs that choose their own sound.
pub struct AmbientSoundTimer(AtomicI32);

impl Default for AmbientSoundTimer {
    fn default() -> Self {
        Self::new()
    }
}

impl AmbientSoundTimer {
    /// Vanilla `Mob.getAmbientSoundInterval`.
    pub const DEFAULT_INTERVAL: i32 = 80;

    #[must_use]
    pub const fn new() -> Self {
        Self(AtomicI32::new(0))
    }

    /// Whether the ambient sound plays this tick; resets the timer when it does.
    pub fn tick(&self) -> bool {
        if rand::random_range(0..1000) < self.0.fetch_add(1, Ordering::Relaxed) {
            self.reset();
            true
        } else {
            false
        }
    }

    pub fn reset(&self) {
        self.0.store(-Self::DEFAULT_INTERVAL, Ordering::Relaxed);
    }
}
