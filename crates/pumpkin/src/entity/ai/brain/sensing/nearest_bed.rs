use pumpkin_util::math::position::BlockPos;
use rand::RngExt;
use rustc_hash::FxHashMap;

use crate::entity::ageable::is_baby;
use crate::entity::ai::pathfinder::path::Path;
use crate::world::poi_manager::{Occupancy, PoiType};

use super::super::BrainTick;
use super::super::behavior::acquire_poi::{SCAN_RANGE, find_path_to_pois};
use super::super::memory::{MemoryModuleId, types};
use super::Sensor;

pub const NEAREST_BED_SCAN_RATE: i32 = 20;
const CACHE_TIMEOUT: i64 = 40;
const BATCH_SIZE: i32 = 5;

const REQUIRES: &[MemoryModuleId] = &[types::NEAREST_BED.id()];

/// Vanilla `NearestBedSensor`: baby villagers look for a bed they can walk to.
#[derive(Default)]
pub struct NearestBedSensor {
    batch_cache: FxHashMap<BlockPos, i64>,
    tried_count: i32,
    last_update: i64,
}

impl Sensor for NearestBedSensor {
    fn requires(&self) -> &'static [MemoryModuleId] {
        REQUIRES
    }

    fn do_tick(&mut self, tick: &mut BrainTick<'_>) {
        if !is_baby(tick.mob) {
            return;
        }
        self.tried_count = 0;
        self.last_update =
            tick.time + i64::from(tick.mob.get_random().random_range(0..NEAREST_BED_SCAN_RATE));
        let center = tick.mob.get_entity().block_pos.load();
        let poi_manager = &tick.world.poi_manager;
        let pois = poi_manager.find_all_with_type(
            |poi_type| poi_type == PoiType::Home,
            |pos| {
                if self.batch_cache.contains_key(pos) {
                    return false;
                }
                self.tried_count += 1;
                if self.tried_count >= BATCH_SIZE {
                    return false;
                }
                self.batch_cache
                    .insert(*pos, self.last_update + CACHE_TIMEOUT);
                true
            },
            &center,
            SCAN_RANGE,
            Occupancy::Any,
        );
        match find_path_to_pois(tick.mob, &pois).filter(Path::can_reach) {
            Some(path) => {
                let target = path.get_target();
                if poi_manager.get_type(&target).is_some() {
                    tick.brain.set(types::NEAREST_BED, target);
                }
            }
            None if self.tried_count < BATCH_SIZE => {
                let last_update = self.last_update;
                self.batch_cache.retain(|_, expiry| *expiry >= last_update);
            }
            None => {}
        }
    }
}
