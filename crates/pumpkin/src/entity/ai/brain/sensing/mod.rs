use std::sync::Arc;

use pumpkin_data::BlockStateId;
use pumpkin_util::math::position::BlockPos;
use rand::{Rng, RngExt};

use crate::entity::ai::target_predicate::TargetPredicate;
use crate::entity::passive::{armadillo_ai, frog_ai};
use crate::entity::{Entity, EntityBase};
use crate::world::World;

use super::memory::{MemoryModuleId, types};
use super::{BrainTick, VisibilityContext};

pub mod adult;
pub mod axolotl_attackables;
pub mod dummy;
pub mod frog_attackables;
pub mod golem;
pub mod hoglin_specific;
pub mod hurt_by;
pub mod is_in_water;
pub mod mob_sensor;
pub mod nearest_items;
pub mod nearest_living_entities;
pub mod nearest_players;
pub mod piglin_brute_specific;
pub mod piglin_specific;
pub mod tempting;

pub use adult::{AdultSensor, AdultSensorAnyType};
pub use axolotl_attackables::AxolotlAttackablesSensor;
pub use dummy::DummySensor;
pub use frog_attackables::FrogAttackablesSensor;
pub use golem::{GOLEM_SCAN_RATE, GolemSensor};
pub use hoglin_specific::HoglinSpecificSensor;
pub use hurt_by::HurtBySensor;
pub use is_in_water::IsInWaterSensor;
pub use mob_sensor::MobSensor;
pub use nearest_items::NearestItemSensor;
pub use nearest_living_entities::NearestLivingEntitySensor;
pub use nearest_players::PlayerSensor;
pub use piglin_brute_specific::PiglinBruteSpecificSensor;
pub use piglin_specific::PiglinSpecificSensor;
pub use tempting::TemptingSensor;

pub const DEFAULT_SCAN_RATE: i32 = 20;

const ARMADILLO_SCARE_REQUIRES: &[MemoryModuleId] = &[
    types::NEAREST_LIVING_ENTITIES.id(),
    types::DANGER_DETECTED_RECENTLY.id(),
];

pub trait Sensor: Send + Sync {
    fn requires(&self) -> &'static [MemoryModuleId];
    fn do_tick(&mut self, tick: &mut BrainTick<'_>);
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum SensorType {
    Dummy,
    NearestLivingEntities,
    NearestPlayers,
    NearestItems,
    HurtBy,
    PiglinSpecific,
    PiglinBruteSpecific,
    GolemDetected,
    NearestAdult,
    NearestAdultAnyType,
    FoodTemptations,
    IsInWater,
    HoglinSpecific,
    FrogTemptations,
    FrogAttackables,
    AxolotlAttackables,
    ArmadilloScareDetected,
}

impl SensorType {
    #[must_use]
    pub const fn name(self) -> &'static str {
        match self {
            Self::Dummy => "minecraft:dummy",
            Self::NearestLivingEntities => "minecraft:nearest_living_entities",
            Self::NearestPlayers => "minecraft:nearest_players",
            Self::NearestItems => "minecraft:nearest_items",
            Self::HurtBy => "minecraft:hurt_by",
            Self::PiglinSpecific => "minecraft:piglin_specific_sensor",
            Self::PiglinBruteSpecific => "minecraft:piglin_brute_specific_sensor",
            Self::GolemDetected => "minecraft:golem_detected",
            Self::NearestAdult => "minecraft:nearest_adult",
            Self::NearestAdultAnyType => "minecraft:nearest_adult_any_type",
            Self::FoodTemptations => "minecraft:food_temptations",
            Self::IsInWater => "minecraft:is_in_water",
            Self::HoglinSpecific => "minecraft:hoglin_specific_sensor",
            Self::FrogTemptations => "minecraft:frog_temptations",
            Self::FrogAttackables => "minecraft:frog_attackables",
            Self::AxolotlAttackables => "minecraft:axolotl_attackables",
            Self::ArmadilloScareDetected => "minecraft:armadillo_scare_detected",
        }
    }

    #[must_use]
    pub const fn scan_rate(self) -> i32 {
        match self {
            Self::Dummy
            | Self::NearestLivingEntities
            | Self::NearestPlayers
            | Self::NearestItems
            | Self::HurtBy
            | Self::PiglinSpecific
            | Self::PiglinBruteSpecific
            | Self::NearestAdult
            | Self::NearestAdultAnyType
            | Self::FoodTemptations
            | Self::IsInWater
            | Self::HoglinSpecific
            | Self::FrogTemptations
            | Self::FrogAttackables
            | Self::AxolotlAttackables => DEFAULT_SCAN_RATE,
            Self::GolemDetected => GOLEM_SCAN_RATE,
            Self::ArmadilloScareDetected => armadillo_ai::SCARE_SCAN_RATE,
        }
    }

    fn create_sensor(self) -> Box<dyn Sensor> {
        match self {
            Self::Dummy => Box::new(DummySensor),
            Self::NearestLivingEntities => Box::new(NearestLivingEntitySensor),
            Self::NearestPlayers => Box::new(PlayerSensor),
            Self::NearestItems => Box::new(NearestItemSensor),
            Self::HurtBy => Box::new(HurtBySensor),
            Self::PiglinSpecific => Box::new(PiglinSpecificSensor),
            Self::PiglinBruteSpecific => Box::new(PiglinBruteSpecificSensor),
            Self::GolemDetected => Box::new(GolemSensor),
            Self::NearestAdult => Box::new(AdultSensor),
            Self::NearestAdultAnyType => Box::new(AdultSensorAnyType),
            Self::FoodTemptations => Box::new(TemptingSensor::for_animal()),
            Self::IsInWater => Box::new(IsInWaterSensor),
            Self::HoglinSpecific => Box::new(HoglinSpecificSensor),
            Self::FrogTemptations => Box::new(TemptingSensor::new(frog_ai::is_temptation)),
            Self::FrogAttackables => Box::new(FrogAttackablesSensor),
            Self::AxolotlAttackables => Box::new(AxolotlAttackablesSensor),
            Self::ArmadilloScareDetected => Box::new(MobSensor::new(
                ARMADILLO_SCARE_REQUIRES,
                armadillo_ai::is_scared_by,
                armadillo_ai::can_stay_rolled_up,
                types::DANGER_DETECTED_RECENTLY,
                armadillo_ai::SCARE_MEMORY_TIME_TO_LIVE,
            )),
        }
    }

    pub fn create<R: Rng>(self, rng: &mut R) -> SensorEntry {
        SensorEntry::new(self, rng)
    }
}

pub struct SensorEntry {
    ty: SensorType,
    sensor: Box<dyn Sensor>,
    scan_rate: i32,
    time_to_tick: i64,
}

impl SensorEntry {
    pub fn new<R: Rng>(ty: SensorType, rng: &mut R) -> Self {
        let scan_rate = ty.scan_rate();
        Self {
            ty,
            sensor: ty.create_sensor(),
            scan_rate,
            time_to_tick: i64::from(rng.random_range(0..scan_rate)),
        }
    }

    #[must_use]
    pub const fn sensor_type(&self) -> SensorType {
        self.ty
    }

    #[must_use]
    pub fn requires(&self) -> &'static [MemoryModuleId] {
        self.sensor.requires()
    }

    pub fn tick(&mut self, tick: &mut BrainTick<'_>) {
        self.time_to_tick -= 1;
        if self.time_to_tick <= 0 {
            self.time_to_tick = i64::from(self.scan_rate);
            self.sensor.do_tick(tick);
        }
    }
}

/// Entities meeting `body`'s box inflated by the given extents, nearest first.
fn entities_in_inflated_box(
    world: &Arc<World>,
    body: &Entity,
    x: f64,
    y: f64,
    z: f64,
    filter: impl Fn(&Arc<dyn EntityBase>) -> bool,
) -> Vec<Arc<dyn EntityBase>> {
    let bounds = body.bounding_box.load().expand(x, y, z);
    let body_pos = body.pos.load();
    let body_id = body.entity_id;

    let entities = world.entities.load();
    let players = world.players.load();
    // Players are not in world.entities
    let mut found: Vec<(f64, Arc<dyn EntityBase>)> = entities
        .iter()
        .map(Arc::clone)
        .chain(
            players
                .iter()
                .map(|player| Arc::clone(player) as Arc<dyn EntityBase>),
        )
        .filter(|entity| {
            entity.get_entity().entity_id != body_id
                && entity.get_entity().bounding_box.load().intersects(&bounds)
                && filter(entity)
        })
        .map(|entity| {
            (
                entity
                    .get_entity()
                    .pos
                    .load()
                    .squared_distance_to_vec(&body_pos),
                entity,
            )
        })
        .collect();
    found.sort_by(|a, b| a.0.total_cmp(&b.0));
    found.into_iter().map(|(_, entity)| entity).collect()
}

/// Vanilla `findBlocksInBoxByManhattanDistance(..).filterState(..).findFirst()`.
///
/// The palette check skips the block-by-block walk when nothing in range can match, the usual
/// case, which keeps thousands of scanning mobs cheap.
pub fn find_nearest_block_state(
    world: &World,
    center: BlockPos,
    reach_xz: i32,
    reach_y: i32,
    predicate: impl Fn(BlockStateId) -> bool,
) -> Option<BlockPos> {
    let min = center.add(-reach_xz, -reach_y, -reach_xz);
    let max = center.add(reach_xz, reach_y, reach_xz);
    if !world.may_contain_block_state(min, max, &predicate) {
        return None;
    }
    find_first_in_box_by_manhattan_distance(center, reach_xz, reach_y, |pos| {
        world
            .get_block_state_id_if_loaded(pos)
            .is_some_and(&predicate)
    })
}

/// First position matching `predicate`, in vanilla `BlockPos.withinBoxByManhattanDistance` order.
pub fn find_first_in_box_by_manhattan_distance(
    center: BlockPos,
    reach_xz: i32,
    reach_y: i32,
    mut predicate: impl FnMut(&BlockPos) -> bool,
) -> Option<BlockPos> {
    let max_depth = reach_xz + reach_y + reach_xz;
    for depth in 0..=max_depth {
        let max_x = reach_xz.min(depth);
        for x in -max_x..=max_x {
            let max_y = reach_y.min(depth - x.abs());
            for y in -max_y..=max_y {
                let z = depth - x.abs() - y.abs();
                if z > reach_xz {
                    continue;
                }
                let pos = BlockPos::new(center.0.x + x, center.0.y + y, center.0.z + z);
                if predicate(&pos) {
                    return Some(pos);
                }
                if z != 0 {
                    let mirrored = BlockPos::new(pos.0.x, pos.0.y, center.0.z - z);
                    if predicate(&mirrored) {
                        return Some(mirrored);
                    }
                }
            }
        }
    }
    None
}

/// Vanilla `NearestVisibleLivingEntitySensor.doTick`: stores the nearest visible entity
/// matching `is_matching` in `memory`, or erases it.
pub fn set_nearest_visible_matching(
    tick: &mut BrainTick<'_>,
    memory: super::memory::MemoryModuleType<Arc<dyn EntityBase>>,
    is_matching: impl Fn(&VisibilityContext<'_>, &Arc<dyn EntityBase>) -> bool,
) {
    let nearest = {
        let ctx = tick.visibility();
        ctx.brain
            .get(types::NEAREST_VISIBLE_LIVING_ENTITIES)
            .and_then(|visible| visible.find_closest(&ctx, |entity| is_matching(&ctx, entity)))
    };
    tick.brain.set_optional(memory, nearest);
}

fn is_current_attack_target(ctx: &VisibilityContext<'_>, target: &dyn EntityBase) -> bool {
    ctx.brain
        .get(types::ATTACK_TARGET)
        .is_some_and(|current| current.get_entity().entity_id == target.get_entity().entity_id)
}

// ignoreInvisibilityTesting drops the range scaling, not line of sight
fn targeting_predicate(
    ctx: &VisibilityContext<'_>,
    target: &dyn EntityBase,
    attackable: bool,
) -> TargetPredicate {
    let predicate = if attackable {
        TargetPredicate::create_attackable()
    } else {
        TargetPredicate::create_non_attackable()
    }
    .set_base_max_distance(ctx.follow_range);
    if is_current_attack_target(ctx, target) {
        predicate.ignore_distance_scaling_factor()
    } else {
        predicate
    }
}

#[must_use]
pub fn is_entity_targetable(ctx: &VisibilityContext<'_>, target: &dyn EntityBase) -> bool {
    targeting_predicate(ctx, target, false).test(ctx.world, Some(ctx.mob), target)
}

#[must_use]
pub fn is_entity_attackable(ctx: &VisibilityContext<'_>, target: &dyn EntityBase) -> bool {
    targeting_predicate(ctx, target, true).test(ctx.world, Some(ctx.mob), target)
}

#[must_use]
pub fn is_entity_attackable_ignoring_line_of_sight(
    ctx: &VisibilityContext<'_>,
    target: &dyn EntityBase,
) -> bool {
    targeting_predicate(ctx, target, true)
        .ignore_visibility()
        .test(ctx.world, Some(ctx.mob), target)
}

pub fn remember_positives<T>(
    invocations: i32,
    predicate: impl Fn(&T) -> bool,
) -> impl FnMut(&T) -> bool {
    let mut positives_left = 0;
    move |value| {
        if predicate(value) {
            positives_left = invocations;
            true
        } else {
            positives_left -= 1;
            positives_left >= 0
        }
    }
}
