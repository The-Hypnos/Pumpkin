use pumpkin_data::entity::EntityStatus;
use pumpkin_util::math::position::BlockPos;
use rand::RngExt;
use rustc_hash::FxHashMap;

use crate::entity::ageable::is_baby;
use crate::entity::ai::pathfinder::path::Path;
use crate::entity::mob::Mob;
use crate::world::World;
use crate::world::poi_manager::{Occupancy, PoiType};

use super::super::memory::{GlobalPos, MemoryModuleType, MemoryStatus};
use super::one_shot::OneShot;
use super::utils::global_pos_in;

pub const SCAN_RANGE: i32 = 48;
const BATCH_SIZE: usize = 5;
const RATE: i64 = 20;

/// Vanilla `AcquirePoi.findPathToPois`: one path search toward all of `pois`.
pub fn find_path_to_pois(mob: &dyn Mob, pois: &[(PoiType, BlockPos)]) -> Option<Path> {
    if pois.is_empty() {
        return None;
    }
    let max_range = pois
        .iter()
        .map(|(poi_type, _)| poi_type.valid_range())
        .fold(1, i32::max);
    let targets: Vec<BlockPos> = pois.iter().map(|(_, pos)| *pos).collect();
    let mob_entity = mob.get_mob_entity();
    mob_entity
        .navigator
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .create_path_to_any(&mob_entity.living_entity, &targets, max_range)
}

/// Vanilla `AcquirePoi.JitteredLinearRetry`: backs off retrying a spot that could not be
/// reached, up to 20 seconds between tries.
struct JitteredLinearRetry {
    previous_attempt: i64,
    next_scheduled_attempt: i64,
    current_delay: i64,
}

impl JitteredLinearRetry {
    const MIN_INTERVAL_INCREASE: i64 = 40;
    const MAX_INTERVAL_INCREASE: i64 = 80;
    const MAX_RETRY_PATHFINDING_INTERVAL: i64 = 400;

    fn new(mob: &dyn Mob, time: i64) -> Self {
        let mut retry = Self {
            previous_attempt: 0,
            next_scheduled_attempt: 0,
            current_delay: 0,
        };
        retry.mark_attempt(mob, time);
        retry
    }

    fn mark_attempt(&mut self, mob: &dyn Mob, time: i64) {
        self.previous_attempt = time;
        let suggested = self.current_delay
            + mob
                .get_random()
                .random_range(0..Self::MAX_INTERVAL_INCREASE - Self::MIN_INTERVAL_INCREASE)
            + Self::MIN_INTERVAL_INCREASE;
        self.current_delay = suggested.min(Self::MAX_RETRY_PATHFINDING_INTERVAL);
        self.next_scheduled_attempt = time + self.current_delay;
    }

    const fn is_still_valid(&self, time: i64) -> bool {
        time - self.previous_attempt < Self::MAX_RETRY_PATHFINDING_INTERVAL
    }

    const fn should_retry(&self, time: i64) -> bool {
        time >= self.next_scheduled_attempt
    }
}

/// Vanilla `AcquirePoi.create`: every second or so, claims the nearest free POI of a type it can
/// path to, remembering it in `memory_to_acquire`.
#[must_use]
pub fn acquire_poi(
    poi_type: fn(PoiType) -> bool,
    memory_to_validate: MemoryModuleType<GlobalPos>,
    memory_to_acquire: MemoryModuleType<GlobalPos>,
    only_if_adult: bool,
    on_acquisition_event: Option<fn() -> EntityStatus>,
    valid_poi: fn(&World, &BlockPos) -> bool,
) -> OneShot {
    let mut next_scheduled_start: i64 = 0;
    let mut batch_cache: FxHashMap<BlockPos, JitteredLinearRetry> = FxHashMap::default();
    let mut conditions = vec![(memory_to_acquire.id(), MemoryStatus::ValueAbsent)];
    if memory_to_validate.id() != memory_to_acquire.id() {
        conditions.push((memory_to_validate.id(), MemoryStatus::ValueAbsent));
    }
    OneShot::new("AcquirePoi", conditions, move |tick| {
        if only_if_adult && is_baby(tick.mob) {
            return false;
        }
        let time = tick.time;
        if next_scheduled_start == 0 {
            next_scheduled_start = time + tick.mob.get_random().random_range(0..RATE);
            return false;
        }
        if time < next_scheduled_start {
            return false;
        }
        next_scheduled_start = time + RATE + tick.mob.get_random().random_range(0..RATE);

        batch_cache.retain(|_, retry| retry.is_still_valid(time));
        let mob = tick.mob;
        let center = mob.get_entity().block_pos.load();
        let pois: Vec<(PoiType, BlockPos)> = tick
            .world
            .poi_manager
            .find_all_closest_first_with_type(
                poi_type,
                |pos| match batch_cache.get_mut(pos) {
                    None => true,
                    Some(retry) if retry.should_retry(time) => {
                        retry.mark_attempt(mob, time);
                        true
                    }
                    Some(_) => false,
                },
                &center,
                SCAN_RANGE,
                Occupancy::HasSpace,
            )
            .into_iter()
            .take(BATCH_SIZE)
            .filter(|(_, pos)| valid_poi(tick.world, pos))
            .collect();

        match find_path_to_pois(mob, &pois).filter(Path::can_reach) {
            Some(path) => {
                let target = path.get_target();
                let poi_manager = &tick.world.poi_manager;
                // Vanilla ignores whether `take` got a ticket, since only one mob ticks at a
                // time there. Mobs tick in parallel here, so another one may have just won it.
                if poi_manager.get_type(&target).is_some()
                    && poi_manager
                        .take(tick.world, poi_type, |_, pos| *pos == target, &target, 1)
                        .is_some()
                {
                    if let Some(global) = global_pos_in(tick.world, target) {
                        tick.brain.set(memory_to_acquire, global);
                    }
                    if let Some(event) = on_acquisition_event {
                        tick.world
                            .send_entity_status(mob.get_entity(), event(), None);
                    }
                    batch_cache.clear();
                }
            }
            None => {
                for (_, pos) in &pois {
                    batch_cache
                        .entry(*pos)
                        .or_insert_with(|| JitteredLinearRetry::new(mob, time));
                }
            }
        }
        true
    })
}
