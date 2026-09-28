use pumpkin_data::Block;
use pumpkin_data::sound::{Sound, SoundCategory};
use pumpkin_data::tag::{self, Taggable};
use pumpkin_world::world::BlockFlags;

use crate::entity::ai::util::goal_utils::fluid_has_tag;

use super::super::memory::{MemoryStatus, types};
use super::one_shot::OneShot;

/// Vanilla `TryLaySpawnOnFluidNearLand`: a pregnant mob on the shore lays `spawn_block`
/// on an adjacent frogspawn-supporting surface.
#[must_use]
pub fn try_lay_spawn_on_fluid_near_land(spawn_block: &'static Block) -> OneShot {
    OneShot::new(
        "TryLaySpawnOnFluidNearLand",
        vec![
            (types::ATTACK_TARGET.id(), MemoryStatus::ValueAbsent),
            (types::WALK_TARGET.id(), MemoryStatus::ValuePresent),
            (types::IS_PREGNANT.id(), MemoryStatus::ValuePresent),
        ],
        move |tick| {
            let entity = tick.mob.get_entity();
            if entity.is_in_water() || !entity.on_ground.load(std::sync::atomic::Ordering::Relaxed)
            {
                return false;
            }
            let world = tick.world;
            let below = entity.block_pos.load().down();
            for relative in [below.north(), below.east(), below.south(), below.west()] {
                let (block, state) = world.get_block_and_state(&relative);
                // An empty top face: no collision box reaches the top of the block.
                let top_face_empty = !state
                    .get_block_collision_shapes_at(&relative)
                    .any(|shape| shape.max.y >= 1.0);
                let supports_spawn =
                    fluid_has_tag(state.id, &tag::Fluid::MINECRAFT_SUPPORTS_FROGSPAWN)
                        || block.has_tag(&tag::Block::MINECRAFT_SUPPORTS_FROGSPAWN);
                if !top_face_empty || !supports_spawn {
                    continue;
                }
                let spawn_pos = relative.up();
                if world.get_block_state(&spawn_pos).is_air() {
                    world.set_block_state(
                        &spawn_pos,
                        spawn_block.default_state.id,
                        BlockFlags::NOTIFY_ALL,
                    );
                    world.emit_game_event("block_place", spawn_pos.to_f64());
                    world.play_sound_fine(
                        Sound::EntityFrogLaySpawn,
                        SoundCategory::Blocks,
                        &entity.pos.load(),
                        1.0,
                        1.0,
                    );
                    tick.brain.erase(types::IS_PREGNANT.id());
                    return true;
                }
            }
            true
        },
    )
}
