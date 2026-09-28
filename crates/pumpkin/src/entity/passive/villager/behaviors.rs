//! Villager-only brain behaviours from vanilla's `ai.behavior` package: the ones typed on
//! `Villager` there.

use std::sync::Arc;

use pumpkin_data::block_properties::WhiteBedLikeProperties as BedProperties;
use pumpkin_data::entity::{EntityStatus, EntityType};
use pumpkin_data::environment_attribute::Activity;
use pumpkin_data::tag::{self, Taggable};
use pumpkin_data::{Block, BlockDirection};
use pumpkin_util::math::position::BlockPos;
use pumpkin_util::math::vector3::Vector3;
use rand::RngExt;
use rand::seq::SliceRandom;

use crate::entity::EntityBase;
use crate::entity::ageable::is_baby;
use crate::entity::ai::brain::BrainTick;
use crate::entity::ai::brain::behavior::utils::{
    global_pos_in, set_walk_and_look_target_memories_to_block,
};
use crate::entity::ai::brain::behavior::{Behavior, NO_TIMEOUT, OneShot};
use crate::entity::ai::brain::memory::position_tracker::{
    BlockPosTracker, EntityTracker, PositionTracker,
};
use crate::entity::ai::brain::memory::walk_target::WalkTarget;
use crate::entity::ai::brain::memory::{
    GlobalPos, MemoryModuleId, MemoryModuleType, MemoryStatus, types,
};
use crate::entity::ai::util::{default_random_pos, land_random_pos};
use crate::entity::mob::Mob;
use crate::world::poi_manager::{PoiType, section_of};

use super::{VillagerEntity, VillagerProfession, as_villager};

/// Vanilla `VillagerProfession.heldJobSite`: the POI type a profession works at.
#[must_use]
pub const fn held_job_site(profession: VillagerProfession) -> Option<PoiType> {
    Some(match profession {
        VillagerProfession::Armorer => PoiType::Armorer,
        VillagerProfession::Butcher => PoiType::Butcher,
        VillagerProfession::Cartographer => PoiType::Cartographer,
        VillagerProfession::Cleric => PoiType::Cleric,
        VillagerProfession::Farmer => PoiType::Farmer,
        VillagerProfession::Fisherman => PoiType::Fisherman,
        VillagerProfession::Fletcher => PoiType::Fletcher,
        VillagerProfession::Leatherworker => PoiType::Leatherworker,
        VillagerProfession::Librarian => PoiType::Librarian,
        VillagerProfession::Mason => PoiType::Mason,
        VillagerProfession::Shepherd => PoiType::Shepherd,
        VillagerProfession::Toolsmith => PoiType::Toolsmith,
        VillagerProfession::Weaponsmith => PoiType::Weaponsmith,
        VillagerProfession::None | VillagerProfession::Nitwit => return None,
    })
}

/// The profession whose `heldJobSite` is `poi_type`; vanilla finds it by scanning the registry.
#[must_use]
pub const fn profession_for_job_site(poi_type: PoiType) -> Option<VillagerProfession> {
    Some(match poi_type {
        PoiType::Armorer => VillagerProfession::Armorer,
        PoiType::Butcher => VillagerProfession::Butcher,
        PoiType::Cartographer => VillagerProfession::Cartographer,
        PoiType::Cleric => VillagerProfession::Cleric,
        PoiType::Farmer => VillagerProfession::Farmer,
        PoiType::Fisherman => VillagerProfession::Fisherman,
        PoiType::Fletcher => VillagerProfession::Fletcher,
        PoiType::Leatherworker => VillagerProfession::Leatherworker,
        PoiType::Librarian => VillagerProfession::Librarian,
        PoiType::Mason => VillagerProfession::Mason,
        PoiType::Shepherd => VillagerProfession::Shepherd,
        PoiType::Toolsmith => VillagerProfession::Toolsmith,
        PoiType::Weaponsmith => VillagerProfession::Weaponsmith,
        _ => return None,
    })
}

/// Vanilla `VillagerProfession.heldJobSite().test(type)`.
#[must_use]
pub fn holds_job_site(profession: VillagerProfession, poi_type: PoiType) -> bool {
    held_job_site(profession) == Some(poi_type)
}

/// Vanilla `VillagerProfession.acquirableJobSite().test(type)`: the unemployed take any job site.
#[must_use]
pub fn can_acquire_job_site(profession: VillagerProfession, poi_type: PoiType) -> bool {
    match profession {
        VillagerProfession::None => {
            poi_type.is_in(&tag::PointOfInterestType::MINECRAFT_ACQUIRABLE_JOB_SITE)
        }
        _ => holds_job_site(profession, poi_type),
    }
}

/// Vanilla `BlockPos.closerToCenterThan`.
fn closer_to_center_than(pos: &BlockPos, position: Vector3<f64>, distance: f64) -> bool {
    pos.to_centered_f64().squared_distance_to_vec(&position) < distance * distance
}

fn villager<'a>(tick: &BrainTick<'a>) -> Option<&'a VillagerEntity> {
    as_villager(tick.mob)
}

/// Other living villagers in `NEAREST_LIVING_ENTITIES`, vanilla's `v instanceof Villager && v != body`.
fn nearby_villagers(tick: &BrainTick<'_>) -> Vec<Arc<dyn EntityBase>> {
    let body_id = tick.mob.get_entity().entity_id;
    tick.brain
        .get(types::NEAREST_LIVING_ENTITIES)
        .map(|entities| {
            entities
                .iter()
                .filter(|entity| {
                    entity.get_entity().entity_id != body_id
                        && as_villager(entity.as_ref()).is_some()
                        && crate::entity::ai::brain::behavior::utils::is_alive(entity.as_ref())
                })
                .cloned()
                .collect()
        })
        .unwrap_or_default()
}

/// Vanilla `VillagerPanicTrigger`.
pub struct VillagerPanicTrigger;

fn is_hurt(tick: &BrainTick<'_>) -> bool {
    tick.brain.has_memory_value(types::HURT_BY.id())
}

fn has_hostile(tick: &BrainTick<'_>) -> bool {
    tick.brain.has_memory_value(types::NEAREST_HOSTILE.id())
}

impl Behavior for VillagerPanicTrigger {
    fn entry_conditions(&self) -> &[(MemoryModuleId, MemoryStatus)] {
        &[]
    }

    fn can_still_use(&mut self, tick: &BrainTick<'_>) -> bool {
        is_hurt(tick) || has_hostile(tick)
    }

    fn start(&mut self, tick: &mut BrainTick<'_>) {
        if !is_hurt(tick) && !has_hostile(tick) {
            return;
        }
        if !tick.brain.is_active(Activity::Panic) {
            tick.brain.erase(types::PATH.id());
            tick.brain.erase(types::WALK_TARGET.id());
            tick.brain.erase(types::LOOK_TARGET.id());
            tick.brain.erase(types::BREED_TARGET.id());
            tick.brain.erase(types::INTERACTION_TARGET.id());
        }
        tick.brain.set_active_activity_if_possible(Activity::Panic);
    }

    fn tick(&mut self, tick: &mut BrainTick<'_>) {
        if tick.time % 100 == 0
            && let Some(villager) = villager(tick)
        {
            villager.spawn_golem_if_needed(tick, 3);
        }
    }

    fn debug_name(&self) -> &'static str {
        "VillagerPanicTrigger"
    }
}

/// Vanilla `WakeUp`.
#[must_use]
pub fn wake_up() -> OneShot {
    OneShot::new("WakeUp", Vec::new(), |tick| {
        let Some(villager) = villager(tick) else {
            return false;
        };
        if tick.brain.is_active(Activity::Rest) || !villager.is_sleeping() {
            return false;
        }
        villager.stop_sleeping(tick);
        true
    })
}

/// Vanilla `ValidateNearbyPoi`: forgets a POI memory once the POI is gone or its bed is taken.
#[must_use]
pub fn validate_nearby_poi(
    poi_type: impl Fn(PoiType) -> bool + Send + Sync + 'static,
    memory: MemoryModuleType<GlobalPos>,
) -> OneShot {
    const MAX_DISTANCE: f64 = 16.0;
    OneShot::new(
        "ValidateNearbyPoi",
        vec![(memory.id(), MemoryStatus::ValuePresent)],
        move |tick| {
            let Some(global) = tick.brain.get(memory).copied() else {
                return false;
            };
            let pos = global.pos;
            let body_pos = tick.mob.get_entity().pos.load();
            if global_pos_in(tick.world, pos) != Some(global)
                || !closer_to_center_than(&pos, body_pos, MAX_DISTANCE)
            {
                return false;
            }
            let poi_manager = &tick.world.poi_manager;
            if !poi_manager.exists(&pos, &poi_type) {
                tick.brain.erase(memory.id());
            } else if bed_is_occupied(tick, &pos) {
                tick.brain.erase(memory.id());
                if !bed_is_occupied_by_villager(tick, &pos) {
                    poi_manager.release(tick.world, &pos);
                }
            }
            true
        },
    )
}

fn bed_is_occupied(tick: &BrainTick<'_>, pos: &BlockPos) -> bool {
    let (block, state) = tick.world.get_block_and_state(pos);
    block.has_tag(&tag::Block::MINECRAFT_VILLAGERS_CAN_SLEEP_ON_BED)
        && BedProperties::from_state_id(state.id).occupied
        && !villager(tick).is_some_and(VillagerEntity::is_sleeping)
}

fn bed_is_occupied_by_villager(tick: &BrainTick<'_>, pos: &BlockPos) -> bool {
    let min = Vector3::new(f64::from(pos.0.x), f64::from(pos.0.y), f64::from(pos.0.z));
    let bounds = pumpkin_util::math::boundingbox::BoundingBox {
        min,
        max: min + Vector3::new(1.0, 1.0, 1.0),
    };
    let mut occupied = false;
    tick.world
        .entity_grid
        .load()
        .for_each_in_box(&bounds, |entity| {
            occupied |= as_villager(entity.as_ref()).is_some_and(VillagerEntity::is_sleeping);
        });
    occupied
}

/// Vanilla `PoiCompetitorScan`: of the villagers holding the same job site, the most
/// experienced keeps it.
#[must_use]
pub fn poi_competitor_scan() -> OneShot {
    OneShot::new(
        "PoiCompetitorScan",
        vec![
            (types::JOB_SITE.id(), MemoryStatus::ValuePresent),
            (
                types::NEAREST_LIVING_ENTITIES.id(),
                MemoryStatus::ValuePresent,
            ),
        ],
        |tick| {
            let Some(job_site) = tick.brain.get(types::JOB_SITE).copied() else {
                return false;
            };
            let Some(poi_type) = tick.world.poi_manager.get_type(&job_site.pos) else {
                return true;
            };
            let Some(body) = villager(tick) else {
                return true;
            };
            // The current winner's experience, and the villager itself unless it is the body.
            let mut winner: (i32, Option<Arc<dyn EntityBase>>) = (body.villager_xp(), None);
            for other in nearby_villagers(tick) {
                let Some(other_villager) = as_villager(other.as_ref()) else {
                    continue;
                };
                if other_villager.mirrored_job_site() != Some(job_site.pos)
                    || !holds_job_site(other_villager.profession(), poi_type)
                {
                    continue;
                }
                let other_entry = (other_villager.villager_xp(), Some(Arc::clone(&other)));
                // Vanilla `selectWinner`: the first keeps it only with strictly more experience.
                let loser = if winner.0 > other_entry.0 {
                    other_entry.1
                } else {
                    std::mem::replace(&mut winner, other_entry).1
                };
                match loser {
                    None => tick.brain.erase(types::JOB_SITE.id()),
                    Some(loser) => {
                        if let Some(loser_mob) = loser.as_mob_entity() {
                            loser_mob.post_to_brain(Box::new(|loser_tick| {
                                loser_tick.brain.erase(types::JOB_SITE.id());
                            }));
                        }
                    }
                }
            }
            true
        },
    )
}

const WALK_AND_LOOK_REGISTERED: &[(MemoryModuleId, MemoryStatus)] = &[
    (types::WALK_TARGET.id(), MemoryStatus::Registered),
    (types::LOOK_TARGET.id(), MemoryStatus::Registered),
];

/// Vanilla `LookAndFollowTradingPlayerSink`: stays by the player it is trading with.
pub struct LookAndFollowTradingPlayerSink {
    speed_modifier: f32,
}

impl LookAndFollowTradingPlayerSink {
    const MAX_DISTANCE_SQUARED: f64 = 16.0;

    #[must_use]
    pub const fn new(speed_modifier: f32) -> Self {
        Self { speed_modifier }
    }

    fn can_follow(tick: &BrainTick<'_>) -> Option<Arc<crate::entity::player::Player>> {
        let villager = villager(tick)?;
        let living = &tick.mob.get_mob_entity().living_entity;
        let player = villager.get_trading_player()?;
        let close = player
            .get_entity()
            .pos
            .load()
            .squared_distance_to_vec(&living.entity.pos.load())
            <= Self::MAX_DISTANCE_SQUARED;
        (crate::entity::ai::brain::behavior::utils::is_alive(tick.mob)
            && !living.entity.is_in_water()
            && !living.was_hurt_recently()
            && close)
            .then_some(player)
    }

    fn follow_player(&self, tick: &mut BrainTick<'_>) {
        let Some(player) = Self::can_follow(tick) else {
            return;
        };
        let player: Arc<dyn EntityBase> = player;
        tick.brain.set(
            types::WALK_TARGET,
            WalkTarget::new(
                Arc::new(EntityTracker::new(Arc::clone(&player), false)),
                self.speed_modifier,
                2,
            ),
        );
        tick.brain.set(
            types::LOOK_TARGET,
            Arc::new(EntityTracker::new(player, true)) as Arc<dyn PositionTracker>,
        );
    }
}

impl Behavior for LookAndFollowTradingPlayerSink {
    fn entry_conditions(&self) -> &[(MemoryModuleId, MemoryStatus)] {
        WALK_AND_LOOK_REGISTERED
    }

    // Vanilla overrides `timedOut` to never time out.
    fn min_duration(&self) -> i32 {
        NO_TIMEOUT
    }

    fn max_duration(&self) -> i32 {
        NO_TIMEOUT
    }

    fn check_extra_start_conditions(&mut self, tick: &mut BrainTick<'_>) -> bool {
        Self::can_follow(tick).is_some()
    }

    fn can_still_use(&mut self, tick: &BrainTick<'_>) -> bool {
        Self::can_follow(tick).is_some()
    }

    fn start(&mut self, tick: &mut BrainTick<'_>) {
        self.follow_player(tick);
    }

    fn tick(&mut self, tick: &mut BrainTick<'_>) {
        self.follow_player(tick);
    }

    fn stop(&mut self, tick: &mut BrainTick<'_>) {
        tick.brain.erase(types::WALK_TARGET.id());
        tick.brain.erase(types::LOOK_TARGET.id());
    }

    fn debug_name(&self) -> &'static str {
        "LookAndFollowTradingPlayerSink"
    }
}

const POTENTIAL_JOB_SITE_PRESENT: &[(MemoryModuleId, MemoryStatus)] =
    &[(types::POTENTIAL_JOB_SITE.id(), MemoryStatus::ValuePresent)];

/// Vanilla `GoToPotentialJobSite`: walks to a job site it has claimed but not taken up yet.
pub struct GoToPotentialJobSite {
    speed_modifier: f32,
}

impl GoToPotentialJobSite {
    const TICKS_UNTIL_TIMEOUT: i32 = 1200;

    #[must_use]
    pub const fn new(speed_modifier: f32) -> Self {
        Self { speed_modifier }
    }
}

impl Behavior for GoToPotentialJobSite {
    fn entry_conditions(&self) -> &[(MemoryModuleId, MemoryStatus)] {
        POTENTIAL_JOB_SITE_PRESENT
    }

    fn min_duration(&self) -> i32 {
        Self::TICKS_UNTIL_TIMEOUT
    }

    fn max_duration(&self) -> i32 {
        Self::TICKS_UNTIL_TIMEOUT
    }

    fn check_extra_start_conditions(&mut self, tick: &mut BrainTick<'_>) -> bool {
        tick.brain
            .get_active_non_core_activity()
            .is_none_or(|activity| {
                matches!(activity, Activity::Idle | Activity::Work | Activity::Play)
            })
    }

    fn can_still_use(&mut self, tick: &BrainTick<'_>) -> bool {
        tick.brain.has_memory_value(types::POTENTIAL_JOB_SITE.id())
    }

    fn tick(&mut self, tick: &mut BrainTick<'_>) {
        if let Some(site) = tick.brain.get(types::POTENTIAL_JOB_SITE).copied() {
            set_walk_and_look_target_memories_to_block(
                tick.brain,
                site.pos,
                self.speed_modifier,
                1,
            );
        }
    }

    fn stop(&mut self, tick: &mut BrainTick<'_>) {
        if let Some(site) = tick.brain.get(types::POTENTIAL_JOB_SITE).copied() {
            let poi_manager = &tick.world.poi_manager;
            if poi_manager.exists(&site.pos, |_| true) {
                poi_manager.release(tick.world, &site.pos);
            }
        }
        tick.brain.erase(types::POTENTIAL_JOB_SITE.id());
    }

    fn debug_name(&self) -> &'static str {
        "GoToPotentialJobSite"
    }
}

/// Vanilla `YieldJobSite`: an unemployed villager hands a job site it cannot use to a nearby
/// villager whose profession works there.
#[must_use]
pub fn yield_job_site(speed_modifier: f32) -> OneShot {
    OneShot::new(
        "YieldJobSite",
        vec![
            (types::POTENTIAL_JOB_SITE.id(), MemoryStatus::ValuePresent),
            (types::JOB_SITE.id(), MemoryStatus::ValueAbsent),
            (
                types::NEAREST_LIVING_ENTITIES.id(),
                MemoryStatus::ValuePresent,
            ),
            (types::WALK_TARGET.id(), MemoryStatus::Registered),
            (types::LOOK_TARGET.id(), MemoryStatus::Registered),
        ],
        move |tick| {
            if is_baby(tick.mob) {
                return false;
            }
            let Some(body) = villager(tick) else {
                return false;
            };
            if body.profession() != VillagerProfession::None {
                return false;
            }
            let Some(site) = tick.brain.get(types::POTENTIAL_JOB_SITE).copied() else {
                return false;
            };
            let poi_pos = site.pos;
            let Some(poi_type) = tick.world.poi_manager.get_type(&poi_pos) else {
                return true;
            };
            let taker = nearby_villagers(tick).into_iter().find(|other| {
                as_villager(other.as_ref())
                    .is_some_and(|other| nearby_wants_job_site(other, poi_type, poi_pos))
            });
            if let Some(taker) = taker {
                tick.brain.erase(types::WALK_TARGET.id());
                tick.brain.erase(types::LOOK_TARGET.id());
                tick.brain.erase(types::POTENTIAL_JOB_SITE.id());
                let taker_has_job = as_villager(taker.as_ref())
                    .is_some_and(|taker| taker.mirrored_job_site().is_some());
                if !taker_has_job && let Some(taker_mob) = taker.as_mob_entity() {
                    taker_mob.post_to_brain(Box::new(move |taker_tick| {
                        set_walk_and_look_target_memories_to_block(
                            taker_tick.brain,
                            poi_pos,
                            speed_modifier,
                            1,
                        );
                        taker_tick.brain.set(types::POTENTIAL_JOB_SITE, site);
                    }));
                }
            }
            true
        },
    )
}

/// Vanilla `YieldJobSite.nearbyWantsJobsite`.
fn nearby_wants_job_site(nearby: &VillagerEntity, poi_type: PoiType, poi_pos: BlockPos) -> bool {
    if nearby.mirrored_potential_job_site().is_some()
        || !holds_job_site(nearby.profession(), poi_type)
    {
        return false;
    }
    nearby.mirrored_job_site().map_or_else(
        || nearby.can_reach(poi_pos, poi_type.valid_range()),
        |job_site| job_site == poi_pos,
    )
}

/// Vanilla `AssignProfessionFromJobSite`: takes up the job site it walked to.
#[must_use]
pub fn assign_profession_from_job_site() -> OneShot {
    const CLOSE_ENOUGH: f64 = 2.0;
    OneShot::new(
        "AssignProfessionFromJobSite",
        vec![
            (types::POTENTIAL_JOB_SITE.id(), MemoryStatus::ValuePresent),
            (types::JOB_SITE.id(), MemoryStatus::Registered),
        ],
        |tick| {
            let Some(site) = tick.brain.get(types::POTENTIAL_JOB_SITE).copied() else {
                return false;
            };
            if !closer_to_center_than(&site.pos, tick.mob.get_entity().pos.load(), CLOSE_ENOUGH) {
                return false;
            }
            tick.brain.erase(types::POTENTIAL_JOB_SITE.id());
            tick.brain.set(types::JOB_SITE, site);
            tick.world
                .send_entity_status(tick.mob.get_entity(), EntityStatus::VillagerHappy, None);
            let Some(body) = villager(tick) else {
                return true;
            };
            if body.profession() != VillagerProfession::None {
                return true;
            }
            if let Some(poi_type) = tick.world.poi_manager.get_type(&site.pos)
                && let Some(profession) = profession_for_job_site(poi_type)
            {
                body.set_profession(profession);
                body.request_brain_refresh();
            }
            true
        },
    )
}

/// Vanilla `ResetProfession`: a villager that never traded loses its profession with its job site.
#[must_use]
pub fn reset_profession() -> OneShot {
    OneShot::new(
        "ResetProfession",
        vec![(types::JOB_SITE.id(), MemoryStatus::ValueAbsent)],
        |tick| {
            let Some(body) = villager(tick) else {
                return false;
            };
            let data = body.villager_data();
            let profession = data.profession_enum();
            let can_be_fired =
                profession != VillagerProfession::None && profession != VillagerProfession::Nitwit;
            if can_be_fired && body.villager_xp() == 0 && data.level.0 <= 1 {
                body.set_profession(VillagerProfession::None);
                body.request_brain_refresh();
                true
            } else {
                false
            }
        },
    )
}

const WORK_AT_POI_CONDITIONS: &[(MemoryModuleId, MemoryStatus)] = &[
    (types::JOB_SITE.id(), MemoryStatus::ValuePresent),
    (types::LOOK_TARGET.id(), MemoryStatus::Registered),
];

/// Vanilla `WorkAtPoi`, and `WorkAtComposter` when `composter` is set.
pub struct WorkAtPoi {
    last_check: i64,
    composter: bool,
}

impl WorkAtPoi {
    const CHECK_COOLDOWN: i64 = 300;
    const DISTANCE: f64 = 1.73;

    #[must_use]
    pub const fn new(composter: bool) -> Self {
        Self {
            last_check: 0,
            composter,
        }
    }

    fn job_site_in_reach(tick: &BrainTick<'_>) -> bool {
        tick.brain.get(types::JOB_SITE).is_some_and(|site| {
            global_pos_in(tick.world, site.pos) == Some(*site)
                && closer_to_center_than(
                    &site.pos,
                    tick.mob.get_entity().pos.load(),
                    Self::DISTANCE,
                )
        })
    }
}

impl Behavior for WorkAtPoi {
    fn entry_conditions(&self) -> &[(MemoryModuleId, MemoryStatus)] {
        WORK_AT_POI_CONDITIONS
    }

    fn check_extra_start_conditions(&mut self, tick: &mut BrainTick<'_>) -> bool {
        if tick.time - self.last_check < Self::CHECK_COOLDOWN {
            return false;
        }
        if tick.mob.get_random().random_range(0..2) != 0 {
            return false;
        }
        self.last_check = tick.time;
        Self::job_site_in_reach(tick)
    }

    fn can_still_use(&mut self, tick: &BrainTick<'_>) -> bool {
        Self::job_site_in_reach(tick)
    }

    fn start(&mut self, tick: &mut BrainTick<'_>) {
        let Some(villager) = villager(tick) else {
            return;
        };
        tick.brain.set(types::LAST_WORKED_AT_POI, tick.time);
        if let Some(site) = tick.brain.get(types::JOB_SITE).copied() {
            tick.brain.set(
                types::LOOK_TARGET,
                Arc::new(BlockPosTracker::new(site.pos)) as Arc<dyn PositionTracker>,
            );
            if self.composter {
                work_at_composter(tick, villager, site.pos);
            }
        }
        villager.play_work_sound();
        if villager.should_restock(tick) {
            villager.restock(tick.time);
        }
    }

    fn debug_name(&self) -> &'static str {
        if self.composter {
            "WorkAtComposter"
        } else {
            "WorkAtPoi"
        }
    }
}

/// Vanilla `WorkAtComposter.useWorkstation`: bakes bread, empties a full composter and fills
/// it with spare seeds.
fn work_at_composter(tick: &BrainTick<'_>, villager: &VillagerEntity, pos: BlockPos) {
    use crate::block::blocks::composter::ComposterBlock;
    use pumpkin_data::block_properties::ComposterLikeProperties;
    use pumpkin_data::item::Item;
    use pumpkin_data::world::WorldEvent;

    const COMPOSTABLE_ITEMS: [&Item; 2] = [&Item::WHEAT_SEEDS, &Item::BEETROOT_SEEDS];
    const TOTAL_ITEMS_TO_USE: u8 = 20;
    const MIN_STACK_SIZE: i32 = 10;

    let (block, state) = tick.world.get_block_and_state_id(&pos);
    if block != &Block::COMPOSTER {
        return;
    }
    villager.make_bread();
    let mut state = state;
    if ComposterLikeProperties::from_state_id(state).level == 8 {
        ComposterBlock.clear_composter(tick.world, &pos, state, block);
        state = tick.world.get_block_state_id(&pos);
    }
    let original_state = state;
    let mut items_to_use = TOTAL_ITEMS_TO_USE;
    let mut seen_so_far = [0i32; COMPOSTABLE_ITEMS.len()];
    villager.with_inventory(|inventory| {
        'slots: for stack in inventory.iter_mut().rev() {
            if items_to_use == 0 {
                break;
            }
            let Some(index) = COMPOSTABLE_ITEMS
                .iter()
                .position(|item| stack.item == *item)
            else {
                continue;
            };
            let stack_size = i32::from(stack.item_count);
            seen_so_far[index] += stack_size;
            let to_use = (seen_so_far[index] - MIN_STACK_SIZE)
                .min(i32::from(items_to_use))
                .min(stack_size);
            if to_use <= 0 {
                continue;
            }
            items_to_use -= to_use as u8;
            for _ in 0..to_use {
                state = insert_into_composter(tick, &pos, state, stack);
                if ComposterLikeProperties::from_state_id(state).level == 7 {
                    break 'slots;
                }
            }
        }
    });
    tick.world.sync_world_event(
        WorldEvent::ComposterFill,
        pos,
        i32::from(state != original_state),
    );
}

/// Vanilla `ComposterBlock.insertItem`: one item in, the level rising by the item's chance.
fn insert_into_composter(
    tick: &BrainTick<'_>,
    pos: &BlockPos,
    state: pumpkin_data::BlockStateId,
    stack: &mut pumpkin_data::item_stack::ItemStack,
) -> pumpkin_data::BlockStateId {
    use crate::block::blocks::composter::ComposterBlock;
    use pumpkin_data::block_properties::ComposterLikeProperties;
    use pumpkin_data::data_component_impl::CompostableImpl;

    let level = ComposterLikeProperties::from_state_id(state).level;
    let Some(chance) = stack
        .get_data_component::<CompostableImpl>()
        .map(|compostable| compostable.chance)
    else {
        return state;
    };
    if level >= 7 {
        return state;
    }
    stack.decrement(1);
    if (level != 0 || chance <= 0.0) && tick.mob.get_random().random::<f64>() >= f64::from(chance) {
        return state;
    }
    ComposterBlock.update_level_composter(tick.world, pos, state, &Block::COMPOSTER, level + 1);
    tick.world.get_block_state_id(pos)
}

/// Vanilla `StrollToPoiList`: now and then walks to one of the positions in `list_memory`,
/// while staying near `stay_close_memory`.
#[must_use]
pub fn stroll_to_poi_list(
    list_memory: MemoryModuleType<Vec<GlobalPos>>,
    speed_modifier: f32,
    close_enough_dist: i32,
    max_distance_from_poi: i32,
    stay_close_memory: MemoryModuleType<GlobalPos>,
) -> OneShot {
    const RETRY_DELAY: i64 = 100;
    let mut next_ok_start_time = 0;
    OneShot::new(
        "StrollToPoiList",
        vec![
            (types::WALK_TARGET.id(), MemoryStatus::Registered),
            (list_memory.id(), MemoryStatus::ValuePresent),
            (stay_close_memory.id(), MemoryStatus::ValuePresent),
        ],
        move |tick| {
            let (Some(list), Some(stay_close)) = (
                tick.brain.get(list_memory).cloned(),
                tick.brain.get(stay_close_memory).copied(),
            ) else {
                return false;
            };
            if list.is_empty() {
                return false;
            }
            let target = list[tick.mob.get_random().random_range(0..list.len())];
            if global_pos_in(tick.world, target.pos) != Some(target)
                || !closer_to_center_than(
                    &stay_close.pos,
                    tick.mob.get_entity().pos.load(),
                    f64::from(max_distance_from_poi),
                )
            {
                return false;
            }
            if tick.time > next_ok_start_time {
                tick.brain.set(
                    types::WALK_TARGET,
                    WalkTarget::from_block_pos(target.pos, speed_modifier, close_enough_dist),
                );
                next_ok_start_time = tick.time + RETRY_DELAY;
            }
            true
        },
    )
}

/// Vanilla `SetWalkTargetFromBlockMemory`: heads back to a remembered POI, giving it up when it
/// is out of reach for too long.
#[must_use]
pub fn set_walk_target_from_block_memory(
    memory: MemoryModuleType<GlobalPos>,
    speed_modifier: f32,
    close_enough_dist: i32,
    too_far_distance: i32,
    too_long_unreachable_duration: i64,
) -> OneShot {
    const MAX_TRIES: i32 = 1000;
    OneShot::new(
        "SetWalkTargetFromBlockMemory",
        vec![
            (
                types::CANT_REACH_WALK_TARGET_SINCE.id(),
                MemoryStatus::Registered,
            ),
            (types::WALK_TARGET.id(), MemoryStatus::ValueAbsent),
            (memory.id(), MemoryStatus::ValuePresent),
        ],
        move |tick| {
            let Some(target) = tick.brain.get(memory).copied() else {
                return false;
            };
            let cant_reach_since = tick.brain.get(types::CANT_REACH_WALK_TARGET_SINCE).copied();
            let give_up = |tick: &mut BrainTick<'_>| {
                if let Some(villager) = villager(tick) {
                    villager.release_poi(tick, memory);
                }
                tick.brain.erase(memory.id());
                tick.brain
                    .set(types::CANT_REACH_WALK_TARGET_SINCE, tick.time);
            };
            if global_pos_in(tick.world, target.pos) != Some(target)
                || cant_reach_since
                    .is_some_and(|since| tick.time - since > too_long_unreachable_duration)
            {
                give_up(tick);
                return true;
            }
            let body_pos = tick.mob.get_entity().block_pos.load();
            let distance = crate::entity::ai::brain::behavior::move_to_target_sink::dist_manhattan(
                &target.pos,
                &body_pos,
            );
            if distance > too_far_distance {
                let towards = Vector3::new(
                    f64::from(target.pos.0.x) + 0.5,
                    f64::from(target.pos.0.y),
                    f64::from(target.pos.0.z) + 0.5,
                );
                let mut tries = 0;
                let position = loop {
                    let candidate = crate::entity::ai::util::default_random_pos::get_pos_towards(
                        tick.mob,
                        15,
                        7,
                        towards,
                        std::f64::consts::FRAC_PI_2,
                    );
                    if let Some(candidate) = candidate
                        && crate::entity::ai::brain::behavior::move_to_target_sink::dist_manhattan(
                            &BlockPos::floored_v(candidate),
                            &body_pos,
                        ) <= too_far_distance
                    {
                        break candidate;
                    }
                    tries += 1;
                    if tries == MAX_TRIES {
                        give_up(tick);
                        return true;
                    }
                };
                tick.brain.set(
                    types::WALK_TARGET,
                    WalkTarget::from_vec(position, speed_modifier, close_enough_dist),
                );
            } else if distance > close_enough_dist {
                tick.brain.set(
                    types::WALK_TARGET,
                    WalkTarget::from_block_pos(target.pos, speed_modifier, close_enough_dist),
                );
            }
            true
        },
    )
}

const INTERACTION_TARGET_PRESENT: &[(MemoryModuleId, MemoryStatus)] =
    &[(types::INTERACTION_TARGET.id(), MemoryStatus::ValuePresent)];

/// Vanilla `ShowTradesToPlayer`: holds up the trades a nearby player could pay for.
pub struct ShowTradesToPlayer {
    min_duration: i32,
    max_duration: i32,
    player_item: Option<pumpkin_data::item_stack::ItemStack>,
    display_items: Vec<pumpkin_data::item_stack::ItemStack>,
    cycle_counter: i32,
    display_index: usize,
    look_time: i32,
}

impl ShowTradesToPlayer {
    const MAX_LOOK_TIME: i32 = 900;
    const STARTING_LOOK_TIME: i32 = 40;
    const MAX_DISTANCE_SQUARED: f64 = 17.0;

    #[must_use]
    pub const fn new(min_duration: i32, max_duration: i32) -> Self {
        Self {
            min_duration,
            max_duration,
            player_item: None,
            display_items: Vec::new(),
            cycle_counter: 0,
            display_index: 0,
            look_time: 0,
        }
    }

    fn valid_target(tick: &BrainTick<'_>) -> Option<Arc<dyn EntityBase>> {
        let target = tick.brain.get(types::INTERACTION_TARGET)?.clone();
        let close = target
            .get_entity()
            .pos
            .load()
            .squared_distance_to_vec(&tick.mob.get_entity().pos.load())
            <= Self::MAX_DISTANCE_SQUARED;
        (target.get_entity().entity_type == &EntityType::PLAYER
            && crate::entity::ai::brain::behavior::utils::is_alive(tick.mob)
            && crate::entity::ai::brain::behavior::utils::is_alive(target.as_ref())
            && !is_baby(tick.mob)
            && close)
            .then_some(target)
    }

    fn look_at_target(tick: &mut BrainTick<'_>) -> Option<Arc<dyn EntityBase>> {
        let target = tick.brain.get(types::INTERACTION_TARGET)?.clone();
        tick.brain.set(
            types::LOOK_TARGET,
            Arc::new(EntityTracker::new(Arc::clone(&target), true)) as Arc<dyn PositionTracker>,
        );
        Some(target)
    }
}

impl Behavior for ShowTradesToPlayer {
    fn entry_conditions(&self) -> &[(MemoryModuleId, MemoryStatus)] {
        INTERACTION_TARGET_PRESENT
    }

    fn min_duration(&self) -> i32 {
        self.min_duration
    }

    fn max_duration(&self) -> i32 {
        self.max_duration
    }

    fn check_extra_start_conditions(&mut self, tick: &mut BrainTick<'_>) -> bool {
        Self::valid_target(tick).is_some()
    }

    fn can_still_use(&mut self, tick: &BrainTick<'_>) -> bool {
        Self::valid_target(tick).is_some() && self.look_time > 0
    }

    fn start(&mut self, tick: &mut BrainTick<'_>) {
        Self::look_at_target(tick);
        self.cycle_counter = 0;
        self.display_index = 0;
        self.look_time = Self::STARTING_LOOK_TIME;
    }

    fn tick(&mut self, tick: &mut BrainTick<'_>) {
        let Some(villager) = villager(tick) else {
            return;
        };
        let Some(target) = Self::look_at_target(tick) else {
            return;
        };
        let held = target.get_living_entity().map_or_else(
            || pumpkin_data::item_stack::ItemStack::EMPTY.clone(),
            |living| {
                living
                    .entity_equipment
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner)
                    .get(&pumpkin_data::data_component_impl::EquipmentSlot::MAIN_HAND)
            },
        );
        // Vanilla `findItemsToDisplay`.
        if self
            .player_item
            .as_ref()
            .is_none_or(|previous| previous.item.id != held.item.id)
        {
            self.display_items.clear();
            if !held.is_empty() {
                self.display_items = villager.trade_outputs_costing(&held);
                if let Some(first) = self.display_items.first() {
                    self.look_time = Self::MAX_LOOK_TIME;
                    villager.set_held_item(first.clone());
                }
            }
            self.player_item = Some(held);
        }
        if self.display_items.is_empty() {
            villager.set_held_item(pumpkin_data::item_stack::ItemStack::EMPTY.clone());
            self.look_time = self.look_time.min(Self::STARTING_LOOK_TIME);
        } else if self.display_items.len() >= 2 {
            self.cycle_counter += 1;
            if self.cycle_counter >= Self::STARTING_LOOK_TIME {
                self.display_index = (self.display_index + 1) % self.display_items.len();
                self.cycle_counter = 0;
                villager.set_held_item(self.display_items[self.display_index].clone());
            }
        }
        self.look_time -= 1;
    }

    fn stop(&mut self, tick: &mut BrainTick<'_>) {
        tick.brain.erase(types::INTERACTION_TARGET.id());
        if let Some(villager) = villager(tick) {
            villager.set_held_item(pumpkin_data::item_stack::ItemStack::EMPTY.clone());
        }
        self.player_item = None;
    }

    fn debug_name(&self) -> &'static str {
        "ShowTradesToPlayer"
    }
}

/// Vanilla `CropBlock.isMaxAge`, or `None` for blocks that are not a `CropBlock`.
fn crop_is_max_age(state: pumpkin_data::BlockStateId) -> Option<bool> {
    use pumpkin_data::block_properties::{
        NetherWartLikeProperties, TorchflowerCropLikeProperties, WheatLikeProperties,
    };
    let block = state.to_block();
    if block == &Block::WHEAT || block == &Block::CARROTS || block == &Block::POTATOES {
        Some(WheatLikeProperties::from_state_id(state).age >= 7)
    } else if block == &Block::BEETROOTS {
        Some(NetherWartLikeProperties::from_state_id(state).age >= 3)
    } else if block == &Block::TORCHFLOWER_CROP {
        Some(TorchflowerCropLikeProperties::from_state_id(state).age >= 2)
    } else {
        None
    }
}

const HARVEST_CONDITIONS: &[(MemoryModuleId, MemoryStatus)] = &[
    (types::LOOK_TARGET.id(), MemoryStatus::ValueAbsent),
    (types::WALK_TARGET.id(), MemoryStatus::ValueAbsent),
    (types::SECONDARY_JOB_SITE.id(), MemoryStatus::ValuePresent),
];

/// Vanilla `HarvestFarmland`: a farmer breaks ripe crops around it and replants.
#[derive(Default)]
pub struct HarvestFarmland {
    above_farmland_pos: Option<BlockPos>,
    next_ok_start_time: i64,
    time_worked_so_far: i32,
    valid_farmland: Vec<BlockPos>,
}

impl HarvestFarmland {
    const HARVEST_DURATION: i32 = 200;
    const SPEED_MODIFIER: f32 = 0.5;

    fn valid_pos(tick: &BrainTick<'_>, pos: &BlockPos) -> bool {
        let state = tick.world.get_block_state_id(pos);
        crop_is_max_age(state) == Some(true)
            || (state.to_state().is_air() && tick.world.get_block(&pos.down()) == &Block::FARMLAND)
    }

    fn pick(&self, tick: &BrainTick<'_>) -> Option<BlockPos> {
        (!self.valid_farmland.is_empty()).then(|| {
            self.valid_farmland[tick
                .mob
                .get_random()
                .random_range(0..self.valid_farmland.len())]
        })
    }

    fn walk_to(tick: &mut BrainTick<'_>, pos: BlockPos) {
        let tracker: Arc<dyn PositionTracker> = Arc::new(BlockPosTracker::new(pos));
        tick.brain.set(types::LOOK_TARGET, Arc::clone(&tracker));
        tick.brain.set(
            types::WALK_TARGET,
            WalkTarget::new(tracker, Self::SPEED_MODIFIER, 1),
        );
    }
}

impl Behavior for HarvestFarmland {
    fn entry_conditions(&self) -> &[(MemoryModuleId, MemoryStatus)] {
        HARVEST_CONDITIONS
    }

    fn check_extra_start_conditions(&mut self, tick: &mut BrainTick<'_>) -> bool {
        if !tick.world.level_info.load().game_rules.mob_griefing
            || villager(tick).is_none_or(|v| v.profession() != VillagerProfession::Farmer)
        {
            return false;
        }
        let body = tick.mob.get_entity().pos.load();
        self.valid_farmland.clear();
        for x in -1..=1 {
            for y in -1..=1 {
                for z in -1..=1 {
                    let pos = BlockPos::floored(
                        body.x + f64::from(x),
                        body.y + f64::from(y),
                        body.z + f64::from(z),
                    );
                    if Self::valid_pos(tick, &pos) {
                        self.valid_farmland.push(pos);
                    }
                }
            }
        }
        self.above_farmland_pos = self.pick(tick);
        self.above_farmland_pos.is_some()
    }

    fn can_still_use(&mut self, _tick: &BrainTick<'_>) -> bool {
        self.time_worked_so_far < Self::HARVEST_DURATION
    }

    fn start(&mut self, tick: &mut BrainTick<'_>) {
        if tick.time > self.next_ok_start_time
            && let Some(pos) = self.above_farmland_pos
        {
            Self::walk_to(tick, pos);
        }
    }

    fn tick(&mut self, tick: &mut BrainTick<'_>) {
        let body = tick.mob.get_entity().pos.load();
        if self
            .above_farmland_pos
            .is_some_and(|pos| !closer_to_center_than(&pos, body, 1.0))
        {
            return;
        }
        if let Some(pos) = self.above_farmland_pos
            && tick.time > self.next_ok_start_time
        {
            let state = tick.world.get_block_state_id(&pos);
            if crop_is_max_age(state) == Some(true) {
                tick.world
                    .break_block(&pos, None, pumpkin_world::world::BlockFlags::NOTIFY_ALL);
            }
            let above_farmland = tick.world.get_block(&pos.down()) == &Block::FARMLAND;
            if state.to_state().is_air()
                && above_farmland
                && let Some(villager) = villager(tick)
                && villager.has_farm_seeds()
            {
                villager.plant_seed(tick, pos);
            }
            if crop_is_max_age(state) == Some(false) {
                self.valid_farmland.retain(|candidate| *candidate != pos);
                self.above_farmland_pos = self.pick(tick);
                if let Some(next) = self.above_farmland_pos {
                    self.next_ok_start_time = tick.time + 20;
                    Self::walk_to(tick, next);
                }
            }
        }
        self.time_worked_so_far += 1;
    }

    fn stop(&mut self, tick: &mut BrainTick<'_>) {
        tick.brain.erase(types::LOOK_TARGET.id());
        tick.brain.erase(types::WALK_TARGET.id());
        self.time_worked_so_far = 0;
        self.next_ok_start_time = tick.time + 40;
    }

    fn debug_name(&self) -> &'static str {
        "HarvestFarmland"
    }
}

const USE_BONEMEAL_CONDITIONS: &[(MemoryModuleId, MemoryStatus)] = &[
    (types::LOOK_TARGET.id(), MemoryStatus::ValueAbsent),
    (types::WALK_TARGET.id(), MemoryStatus::ValueAbsent),
];

/// Vanilla `UseBonemeal`: a villager with bone meal grows the crops around it.
#[derive(Default)]
pub struct UseBonemeal {
    next_work_cycle_time: i64,
    last_bonemealing_session: i32,
    time_worked_so_far: i32,
    crop_pos: Option<BlockPos>,
}

impl UseBonemeal {
    const BONEMEALING_DURATION: i32 = 80;
    const SESSION_COOLDOWN: i32 = 160;

    fn pick_next_target(tick: &BrainTick<'_>) -> Option<BlockPos> {
        let center = tick.mob.get_entity().block_pos.load();
        let mut rng = tick.mob.get_random();
        let mut result = None;
        let mut count = 0;
        for x in -1..=1 {
            for y in -1..=1 {
                for z in -1..=1 {
                    let pos = BlockPos::new(center.0.x + x, center.0.y + y, center.0.z + z);
                    if crop_is_max_age(tick.world.get_block_state_id(&pos)) == Some(false) {
                        count += 1;
                        if rng.random_range(0..count) == 0 {
                            result = Some(pos);
                        }
                    }
                }
            }
        }
        result
    }

    fn target_current_crop(&self, tick: &mut BrainTick<'_>) {
        if let Some(pos) = self.crop_pos {
            let tracker: Arc<dyn PositionTracker> = Arc::new(BlockPosTracker::new(pos));
            tick.brain.set(types::LOOK_TARGET, Arc::clone(&tracker));
            tick.brain
                .set(types::WALK_TARGET, WalkTarget::new(tracker, 0.5, 1));
        }
    }
}

impl Behavior for UseBonemeal {
    fn entry_conditions(&self) -> &[(MemoryModuleId, MemoryStatus)] {
        USE_BONEMEAL_CONDITIONS
    }

    fn check_extra_start_conditions(&mut self, tick: &mut BrainTick<'_>) -> bool {
        let Some(tick_count) = villager(tick).map(VillagerEntity::tick_count) else {
            return false;
        };
        if tick_count % 10 != 0
            || (self.last_bonemealing_session != 0
                && self.last_bonemealing_session + Self::SESSION_COOLDOWN > tick_count)
        {
            return false;
        }
        if villager(tick).is_none_or(|v| v.count_item(&pumpkin_data::item::Item::BONE_MEAL) <= 0) {
            return false;
        }
        self.crop_pos = Self::pick_next_target(tick);
        self.crop_pos.is_some()
    }

    fn can_still_use(&mut self, _tick: &BrainTick<'_>) -> bool {
        self.time_worked_so_far < Self::BONEMEALING_DURATION && self.crop_pos.is_some()
    }

    fn start(&mut self, tick: &mut BrainTick<'_>) {
        self.target_current_crop(tick);
        if let Some(villager) = villager(tick) {
            villager.set_held_item(pumpkin_data::item_stack::ItemStack::new(
                1,
                &pumpkin_data::item::Item::BONE_MEAL,
            ));
        }
        self.next_work_cycle_time = tick.time;
        self.time_worked_so_far = 0;
    }

    fn tick(&mut self, tick: &mut BrainTick<'_>) {
        let Some(target) = self.crop_pos else {
            return;
        };
        if tick.time < self.next_work_cycle_time
            || !closer_to_center_than(&target, tick.mob.get_entity().pos.load(), 1.0)
        {
            return;
        }
        if villager(tick).is_some_and(|v| v.use_bone_meal_on(tick, target)) {
            tick.world.sync_world_event(
                pumpkin_data::world::WorldEvent::ParticlesAndSoundPlantGrowth,
                target,
                15,
            );
            self.crop_pos = Self::pick_next_target(tick);
            self.target_current_crop(tick);
            self.next_work_cycle_time = tick.time + 40;
        }
        self.time_worked_so_far += 1;
    }

    fn stop(&mut self, tick: &mut BrainTick<'_>) {
        if let Some(villager) = villager(tick) {
            villager.set_held_item(pumpkin_data::item_stack::ItemStack::EMPTY.clone());
        }
        if let Some(villager) = villager(tick) {
            self.last_bonemealing_session = villager.tick_count();
        }
    }

    fn debug_name(&self) -> &'static str {
        "UseBonemeal"
    }
}

const GIFT_CONDITIONS: &[(MemoryModuleId, MemoryStatus)] = &[
    (types::WALK_TARGET.id(), MemoryStatus::Registered),
    (types::LOOK_TARGET.id(), MemoryStatus::Registered),
    (types::INTERACTION_TARGET.id(), MemoryStatus::Registered),
    (
        types::NEAREST_VISIBLE_PLAYER.id(),
        MemoryStatus::ValuePresent,
    ),
];

/// Vanilla `GiveGiftToHero`: throws a present at a nearby Hero of the Village.
pub struct GiveGiftToHero {
    timeout: i32,
    time_until_next_gift: i32,
    gift_given_during_this_run: bool,
    time_since_start: i64,
}

impl GiveGiftToHero {
    const THROW_GIFT_AT_DISTANCE: f64 = 5.0;
    const MIN_TIME_BETWEEN_GIFTS: i32 = 600;
    const MAX_TIME_BETWEEN_GIFTS: i32 = 6600;
    const TIME_TO_DELAY_FOR_HEAD_TO_FINISH_TURNING: i64 = 20;
    const SPEED_MODIFIER: f32 = 0.5;

    #[must_use]
    pub const fn new(timeout: i32) -> Self {
        Self {
            timeout,
            time_until_next_gift: Self::MIN_TIME_BETWEEN_GIFTS,
            gift_given_during_this_run: false,
            time_since_start: 0,
        }
    }

    fn nearest_hero(tick: &BrainTick<'_>) -> Option<Arc<crate::entity::player::Player>> {
        tick.brain
            .get(types::NEAREST_VISIBLE_PLAYER)
            .filter(|player| {
                player
                    .living_entity
                    .has_effect(&pumpkin_data::effect::StatusEffect::HERO_OF_THE_VILLAGE)
            })
            .cloned()
    }

    /// Vanilla `getLootTableToThrow`.
    fn gift_loot_table(tick: &BrainTick<'_>, villager: &VillagerEntity) -> &'static str {
        if is_baby(tick.mob) {
            return "minecraft:gameplay/hero_of_the_village/baby_gift";
        }
        match villager.profession() {
            VillagerProfession::Armorer => "minecraft:gameplay/hero_of_the_village/armorer_gift",
            VillagerProfession::Butcher => "minecraft:gameplay/hero_of_the_village/butcher_gift",
            VillagerProfession::Cartographer => {
                "minecraft:gameplay/hero_of_the_village/cartographer_gift"
            }
            VillagerProfession::Cleric => "minecraft:gameplay/hero_of_the_village/cleric_gift",
            VillagerProfession::Farmer => "minecraft:gameplay/hero_of_the_village/farmer_gift",
            VillagerProfession::Fisherman => {
                "minecraft:gameplay/hero_of_the_village/fisherman_gift"
            }
            VillagerProfession::Fletcher => "minecraft:gameplay/hero_of_the_village/fletcher_gift",
            VillagerProfession::Leatherworker => {
                "minecraft:gameplay/hero_of_the_village/leatherworker_gift"
            }
            VillagerProfession::Librarian => {
                "minecraft:gameplay/hero_of_the_village/librarian_gift"
            }
            VillagerProfession::Mason => "minecraft:gameplay/hero_of_the_village/mason_gift",
            VillagerProfession::Shepherd => "minecraft:gameplay/hero_of_the_village/shepherd_gift",
            VillagerProfession::Toolsmith => {
                "minecraft:gameplay/hero_of_the_village/toolsmith_gift"
            }
            VillagerProfession::Weaponsmith => {
                "minecraft:gameplay/hero_of_the_village/weaponsmith_gift"
            }
            VillagerProfession::None | VillagerProfession::Nitwit => {
                "minecraft:gameplay/hero_of_the_village/unemployed_gift"
            }
        }
    }
}

impl Behavior for GiveGiftToHero {
    fn entry_conditions(&self) -> &[(MemoryModuleId, MemoryStatus)] {
        GIFT_CONDITIONS
    }

    fn min_duration(&self) -> i32 {
        self.timeout
    }

    fn max_duration(&self) -> i32 {
        self.timeout
    }

    fn check_extra_start_conditions(&mut self, tick: &mut BrainTick<'_>) -> bool {
        if Self::nearest_hero(tick).is_none() {
            return false;
        }
        if self.time_until_next_gift > 0 {
            self.time_until_next_gift -= 1;
            return false;
        }
        true
    }

    fn can_still_use(&mut self, tick: &BrainTick<'_>) -> bool {
        Self::nearest_hero(tick).is_some() && !self.gift_given_during_this_run
    }

    fn start(&mut self, tick: &mut BrainTick<'_>) {
        self.gift_given_during_this_run = false;
        self.time_since_start = tick.time;
        let Some(hero) = Self::nearest_hero(tick) else {
            return;
        };
        let hero: Arc<dyn EntityBase> = hero;
        tick.brain.set(types::INTERACTION_TARGET, Arc::clone(&hero));
        crate::entity::ai::brain::behavior::utils::look_at_entity(tick.brain, hero);
    }

    fn tick(&mut self, tick: &mut BrainTick<'_>) {
        let Some(hero) = Self::nearest_hero(tick) else {
            return;
        };
        let hero_pos = hero.get_entity().pos.load();
        let hero: Arc<dyn EntityBase> = hero;
        crate::entity::ai::brain::behavior::utils::look_at_entity(tick.brain, Arc::clone(&hero));
        let villager_block = tick.mob.get_entity().block_pos.load().to_f64();
        let hero_block = hero.get_entity().block_pos.load().to_f64();
        if villager_block.squared_distance_to_vec(&hero_block)
            < Self::THROW_GIFT_AT_DISTANCE * Self::THROW_GIFT_AT_DISTANCE
        {
            if tick.time - self.time_since_start > Self::TIME_TO_DELAY_FOR_HEAD_TO_FINISH_TURNING
                && let Some(villager) = villager(tick)
            {
                let table = Self::gift_loot_table(tick, villager);
                if let Some(loot) = crate::world::loot::get_loot_table(table) {
                    for stack in loot.generate_loot(tick.mob.get_random().random()) {
                        crate::entity::ai::brain::behavior::utils::throw_item(
                            tick.mob,
                            stack,
                            hero_pos,
                            crate::entity::ai::brain::behavior::utils::DEFAULT_THROW_VELOCITY,
                            crate::entity::ai::brain::behavior::utils::DEFAULT_THROW_HAND_Y_DISTANCE,
                        );
                    }
                }
                self.gift_given_during_this_run = true;
            }
        } else {
            crate::entity::ai::brain::behavior::utils::set_walk_and_look_target_memories_to_entity(
                tick.brain,
                hero,
                Self::SPEED_MODIFIER,
                5,
            );
        }
    }

    fn stop(&mut self, tick: &mut BrainTick<'_>) {
        self.time_until_next_gift = Self::MIN_TIME_BETWEEN_GIFTS
            + tick
                .mob
                .get_random()
                .random_range(0..=Self::MAX_TIME_BETWEEN_GIFTS - Self::MIN_TIME_BETWEEN_GIFTS);
        tick.brain.erase(types::INTERACTION_TARGET.id());
        tick.brain.erase(types::WALK_TARGET.id());
        tick.brain.erase(types::LOOK_TARGET.id());
    }

    fn debug_name(&self) -> &'static str {
        "GiveGiftToHero"
    }
}

/// Vanilla `BehaviorUtils.targetIsValid(brain, memory, VILLAGER)`.
fn villager_target_is_valid(
    tick: &BrainTick<'_>,
    memory: MemoryModuleType<Arc<dyn EntityBase>>,
) -> bool {
    crate::entity::ai::brain::behavior::utils::target_is_valid(
        &tick.visibility(),
        memory,
        |target| target.get_entity().entity_type == &EntityType::VILLAGER,
    )
}

fn within_interaction_range(tick: &BrainTick<'_>, target: &dyn EntityBase) -> bool {
    const INTERACT_DIST_SQR: f64 = 5.0;
    target
        .get_entity()
        .pos
        .load()
        .squared_distance_to_vec(&tick.mob.get_entity().pos.load())
        <= INTERACT_DIST_SQR
}

const TRADE_WITH_VILLAGER_CONDITIONS: &[(MemoryModuleId, MemoryStatus)] = &[
    (types::INTERACTION_TARGET.id(), MemoryStatus::ValuePresent),
    (
        types::NEAREST_VISIBLE_LIVING_ENTITIES.id(),
        MemoryStatus::ValuePresent,
    ),
];

/// Vanilla `TradeWithVillager`: two villagers meet, swap gossip and hand over spare items.
#[derive(Default)]
pub struct TradeWithVillager {
    trades: Vec<&'static pumpkin_data::item::Item>,
}

impl TradeWithVillager {
    const SPEED_MODIFIER: f32 = 0.5;
    const CLOSE_ENOUGH: i32 = 2;
}

impl Behavior for TradeWithVillager {
    fn entry_conditions(&self) -> &[(MemoryModuleId, MemoryStatus)] {
        TRADE_WITH_VILLAGER_CONDITIONS
    }

    fn check_extra_start_conditions(&mut self, tick: &mut BrainTick<'_>) -> bool {
        villager_target_is_valid(tick, types::INTERACTION_TARGET)
    }

    fn can_still_use(&mut self, tick: &BrainTick<'_>) -> bool {
        villager_target_is_valid(tick, types::INTERACTION_TARGET)
    }

    fn start(&mut self, tick: &mut BrainTick<'_>) {
        let Some(target) = tick.brain.get(types::INTERACTION_TARGET).cloned() else {
            return;
        };
        crate::entity::ai::brain::behavior::utils::lock_gaze_and_walk_to_each_other(
            tick,
            &target,
            Self::SPEED_MODIFIER,
            Self::CLOSE_ENOUGH,
        );
        // Vanilla `figureOutWhatIAmWillingToTrade`.
        let (Some(body), Some(other)) = (villager(tick), as_villager(target.as_ref())) else {
            return;
        };
        let own = body.profession().requested_items();
        self.trades = other
            .profession()
            .requested_items()
            .iter()
            .filter(|item| !own.contains(item))
            .copied()
            .collect();
    }

    fn tick(&mut self, tick: &mut BrainTick<'_>) {
        let Some(target) = tick.brain.get(types::INTERACTION_TARGET).cloned() else {
            return;
        };
        if !within_interaction_range(tick, target.as_ref()) {
            return;
        }
        crate::entity::ai::brain::behavior::utils::lock_gaze_and_walk_to_each_other(
            tick,
            &target,
            Self::SPEED_MODIFIER,
            Self::CLOSE_ENOUGH,
        );
        let (Some(body), Some(other)) = (villager(tick), as_villager(target.as_ref())) else {
            return;
        };
        body.gossip(tick, other);
        let target_pos = target.get_entity().pos.load();
        let is_farmer = body.profession() == VillagerProfession::Farmer;
        if body.has_excess_food() && (is_farmer || other.wants_more_food()) {
            body.throw_half_stack(
                tick,
                |stack| super::get_food_points(stack.item) > 0,
                target_pos,
            );
        }
        if is_farmer
            && body.count_item(&pumpkin_data::item::Item::WHEAT)
                > i32::from(
                    pumpkin_data::item_stack::ItemStack::new(1, &pumpkin_data::item::Item::WHEAT)
                        .get_max_stack_size(),
                ) / 2
        {
            body.throw_half_stack(
                tick,
                |stack| stack.item == &pumpkin_data::item::Item::WHEAT,
                target_pos,
            );
        }
        if !self.trades.is_empty() {
            let trades = &self.trades;
            body.throw_half_stack(tick, |stack| trades.contains(&stack.item), target_pos);
        }
    }

    fn stop(&mut self, tick: &mut BrainTick<'_>) {
        tick.brain.erase(types::INTERACTION_TARGET.id());
    }

    fn debug_name(&self) -> &'static str {
        "TradeWithVillager"
    }
}

const MAKE_LOVE_CONDITIONS: &[(MemoryModuleId, MemoryStatus)] = &[
    (types::BREED_TARGET.id(), MemoryStatus::ValuePresent),
    (
        types::NEAREST_VISIBLE_LIVING_ENTITIES.id(),
        MemoryStatus::ValuePresent,
    ),
];

/// Vanilla `VillagerMakeLove`: two fed villagers court and, if a free bed is in reach, have a
/// baby that claims it.
#[derive(Default)]
pub struct VillagerMakeLove {
    birth_timestamp: i64,
}

impl VillagerMakeLove {
    const DURATION: i32 = 350;
    const SPEED_MODIFIER: f32 = 0.5;
    const CLOSE_ENOUGH: i32 = 2;

    fn breed_target(tick: &BrainTick<'_>) -> Option<Arc<dyn EntityBase>> {
        tick.brain
            .get(types::BREED_TARGET)
            .filter(|target| target.get_entity().entity_type == &EntityType::VILLAGER)
            .cloned()
    }

    fn is_breeding_possible(tick: &BrainTick<'_>) -> bool {
        let Some(target) = Self::breed_target(tick) else {
            return false;
        };
        villager_target_is_valid(tick, types::BREED_TARGET)
            && villager(tick).is_some_and(VillagerEntity::can_breed)
            && as_villager(target.as_ref()).is_some_and(VillagerEntity::can_breed)
    }

    /// Vanilla `takeVacantBed`: the first free bed in reach, claimed.
    fn take_vacant_bed(tick: &BrainTick<'_>, body: &VillagerEntity) -> Option<BlockPos> {
        let center = tick.mob.get_entity().block_pos.load();
        let poi_manager = &tick.world.poi_manager;
        // Candidates are gathered first so the path searches run outside the POI lock.
        let beds = poi_manager.find_all_with_type(
            |poi_type| poi_type == PoiType::Home,
            |_| true,
            &center,
            crate::entity::ai::brain::behavior::acquire_poi::SCAN_RANGE,
            crate::world::poi_manager::Occupancy::HasSpace,
        );
        beds.into_iter().find_map(|(poi_type, pos)| {
            if !body.can_reach(pos, poi_type.valid_range()) {
                return None;
            }
            poi_manager.take(
                tick.world,
                |poi_type| poi_type == PoiType::Home,
                |_, candidate| *candidate == pos,
                &pos,
                1,
            )
        })
    }
}

impl Behavior for VillagerMakeLove {
    fn entry_conditions(&self) -> &[(MemoryModuleId, MemoryStatus)] {
        MAKE_LOVE_CONDITIONS
    }

    fn min_duration(&self) -> i32 {
        Self::DURATION
    }

    fn max_duration(&self) -> i32 {
        Self::DURATION
    }

    fn check_extra_start_conditions(&mut self, tick: &mut BrainTick<'_>) -> bool {
        Self::is_breeding_possible(tick)
    }

    fn can_still_use(&mut self, tick: &BrainTick<'_>) -> bool {
        tick.time <= self.birth_timestamp && Self::is_breeding_possible(tick)
    }

    fn start(&mut self, tick: &mut BrainTick<'_>) {
        let Some(target) = Self::breed_target(tick) else {
            return;
        };
        crate::entity::ai::brain::behavior::utils::lock_gaze_and_walk_to_each_other(
            tick,
            &target,
            Self::SPEED_MODIFIER,
            Self::CLOSE_ENOUGH,
        );
        tick.world
            .send_entity_status(target.get_entity(), EntityStatus::InLoveHearts, None);
        tick.world
            .send_entity_status(tick.mob.get_entity(), EntityStatus::InLoveHearts, None);
        let duration = 275 + tick.mob.get_random().random_range(0..50);
        self.birth_timestamp = tick.time + i64::from(duration);
    }

    fn tick(&mut self, tick: &mut BrainTick<'_>) {
        let Some(target) = Self::breed_target(tick) else {
            return;
        };
        if !within_interaction_range(tick, target.as_ref()) {
            return;
        }
        crate::entity::ai::brain::behavior::utils::lock_gaze_and_walk_to_each_other(
            tick,
            &target,
            Self::SPEED_MODIFIER,
            Self::CLOSE_ENOUGH,
        );
        let (Some(body), Some(other)) = (villager(tick), as_villager(target.as_ref())) else {
            return;
        };
        if tick.time >= self.birth_timestamp {
            body.eat_and_digest_food();
            other.eat_and_digest_food();
            match Self::take_vacant_bed(tick, body) {
                None => {
                    tick.world.send_entity_status(
                        target.get_entity(),
                        EntityStatus::VillagerAngry,
                        None,
                    );
                    tick.world.send_entity_status(
                        tick.mob.get_entity(),
                        EntityStatus::VillagerAngry,
                        None,
                    );
                }
                Some(bed) => {
                    if !body.breed_with(tick, other, bed) {
                        tick.world.poi_manager.release(tick.world, &bed);
                    }
                }
            }
        } else if tick.mob.get_random().random_range(0..35) == 0 {
            tick.world
                .send_entity_status(target.get_entity(), EntityStatus::LoveHearts, None);
            tick.world
                .send_entity_status(tick.mob.get_entity(), EntityStatus::LoveHearts, None);
        }
    }

    fn stop(&mut self, tick: &mut BrainTick<'_>) {
        tick.brain.erase(types::BREED_TARGET.id());
    }

    fn debug_name(&self) -> &'static str {
        "VillagerMakeLove"
    }
}

/// Vanilla `SocializeAtBell`: now and then walks over to another villager at the meeting point.
#[must_use]
pub fn socialize_at_bell() -> OneShot {
    const SPEED_MODIFIER: f32 = 0.3;
    const MEETING_RANGE: f64 = 4.0;
    const MAX_DISTANCE_SQUARED: f64 = 32.0;
    OneShot::new(
        "SocializeAtBell",
        vec![
            (types::WALK_TARGET.id(), MemoryStatus::Registered),
            (types::LOOK_TARGET.id(), MemoryStatus::Registered),
            (types::MEETING_POINT.id(), MemoryStatus::ValuePresent),
            (
                types::NEAREST_VISIBLE_LIVING_ENTITIES.id(),
                MemoryStatus::ValuePresent,
            ),
            (types::INTERACTION_TARGET.id(), MemoryStatus::ValueAbsent),
        ],
        |tick| {
            let Some(meeting) = tick.brain.get(types::MEETING_POINT).copied() else {
                return false;
            };
            let body_pos = tick.mob.get_entity().pos.load();
            if tick.mob.get_random().random_range(0..100) != 0
                || global_pos_in(tick.world, meeting.pos) != Some(meeting)
                || !closer_to_center_than(&meeting.pos, body_pos, MEETING_RANGE)
            {
                return false;
            }
            let is_villager =
                |mob: &Arc<dyn EntityBase>| mob.get_entity().entity_type == &EntityType::VILLAGER;
            let partner = {
                let ctx = tick.visibility();
                let Some(visible) = ctx.brain.get(types::NEAREST_VISIBLE_LIVING_ENTITIES) else {
                    return false;
                };
                if visible.find_closest(&ctx, is_villager).is_none() {
                    return false;
                }
                visible.find_closest(&ctx, |mob| {
                    is_villager(mob)
                        && mob
                            .get_entity()
                            .pos
                            .load()
                            .squared_distance_to_vec(&body_pos)
                            <= MAX_DISTANCE_SQUARED
                })
            };
            if let Some(partner) = partner {
                tick.brain
                    .set(types::INTERACTION_TARGET, Arc::clone(&partner));
                tick.brain.set(
                    types::LOOK_TARGET,
                    Arc::new(EntityTracker::new(Arc::clone(&partner), true))
                        as Arc<dyn PositionTracker>,
                );
                tick.brain.set(
                    types::WALK_TARGET,
                    WalkTarget::new(
                        Arc::new(EntityTracker::new(partner, false)),
                        SPEED_MODIFIER,
                        1,
                    ),
                );
            }
            true
        },
    )
}

/// Vanilla `PlayTagWithOtherKids`: baby villagers chase each other around the village.
#[must_use]
pub fn play_tag_with_other_kids() -> OneShot {
    const MAX_FLEE_XZ_DIST: i32 = 20;
    const MAX_FLEE_Y_DIST: i32 = 8;
    const FLEE_SPEED_MODIFIER: f32 = 0.6;
    const CHASE_SPEED_MODIFIER: f32 = 0.6;
    const MAX_CHASERS_PER_TARGET: usize = 5;
    const AVERAGE_WAIT_TIME_BETWEEN_RUNS: i32 = 10;
    OneShot::new(
        "PlayTagWithOtherKids",
        vec![
            (
                types::VISIBLE_VILLAGER_BABIES.id(),
                MemoryStatus::ValuePresent,
            ),
            (types::WALK_TARGET.id(), MemoryStatus::ValueAbsent),
            (types::LOOK_TARGET.id(), MemoryStatus::Registered),
            (types::INTERACTION_TARGET.id(), MemoryStatus::Registered),
        ],
        |tick| {
            if tick
                .mob
                .get_random()
                .random_range(0..AVERAGE_WAIT_TIME_BETWEEN_RUNS)
                != 0
            {
                return false;
            }
            let friends = tick
                .brain
                .get(types::VISIBLE_VILLAGER_BABIES)
                .cloned()
                .unwrap_or_default();
            let me = tick.mob.get_entity().entity_id;
            let chasing = |friend: &Arc<dyn EntityBase>| {
                as_villager(friend.as_ref()).and_then(VillagerEntity::mirrored_interaction_target)
            };
            if friends.iter().any(|friend| chasing(friend) == Some(me)) {
                for _ in 0..10 {
                    if let Some(pos) = crate::entity::ai::util::land_random_pos::get_pos(
                        tick.mob,
                        MAX_FLEE_XZ_DIST,
                        MAX_FLEE_Y_DIST,
                    ) && tick.world.poi_manager.is_village(&BlockPos::floored_v(pos))
                    {
                        tick.brain.set(
                            types::WALK_TARGET,
                            WalkTarget::from_vec(pos, FLEE_SPEED_MODIFIER, 0),
                        );
                        break;
                    }
                }
                return true;
            }
            // Vanilla `findSomeoneBeingChased`: the kid with the fewest chasers, if any.
            let mut chased: Vec<(i32, usize)> = Vec::new();
            for target in friends.iter().filter_map(chasing) {
                match chased.iter_mut().find(|(id, _)| *id == target) {
                    Some((_, count)) => *count += 1,
                    None => chased.push((target, 1)),
                }
            }
            chased.sort_by_key(|(_, count)| *count);
            let being_chased = chased
                .iter()
                .find(|(_, count)| *count <= MAX_CHASERS_PER_TARGET)
                .and_then(|(id, _)| {
                    friends
                        .iter()
                        .find(|friend| friend.get_entity().entity_id == *id)
                });
            if let Some(kid) = being_chased.or_else(|| friends.first()).cloned() {
                tick.brain.set(types::INTERACTION_TARGET, Arc::clone(&kid));
                tick.brain.set(
                    types::LOOK_TARGET,
                    Arc::new(EntityTracker::new(Arc::clone(&kid), true))
                        as Arc<dyn PositionTracker>,
                );
                tick.brain.set(
                    types::WALK_TARGET,
                    WalkTarget::new(
                        Arc::new(EntityTracker::new(kid, false)),
                        CHASE_SPEED_MODIFIER,
                        1,
                    ),
                );
            }
            true
        },
    )
}

const JUMP_ON_BED_CONDITIONS: &[(MemoryModuleId, MemoryStatus)] = &[
    (types::NEAREST_BED.id(), MemoryStatus::ValuePresent),
    (types::WALK_TARGET.id(), MemoryStatus::ValueAbsent),
];

/// Vanilla `JumpOnBed`: baby villagers bounce on a bed.
pub struct JumpOnBed {
    speed_modifier: f32,
    target_bed: Option<BlockPos>,
    remaining_time_to_reach_bed: i32,
    remaining_jumps: i32,
    remaining_cooldown_until_next_jump: i32,
}

impl JumpOnBed {
    const MAX_TIME_TO_REACH_BED: i32 = 100;
    const MIN_JUMPS: i32 = 3;
    const MAX_JUMPS: i32 = 6;
    const COOLDOWN_BETWEEN_JUMPS: i32 = 5;

    #[must_use]
    pub const fn new(speed_modifier: f32) -> Self {
        Self {
            speed_modifier,
            target_bed: None,
            remaining_time_to_reach_bed: 0,
            remaining_jumps: 0,
            remaining_cooldown_until_next_jump: 0,
        }
    }

    fn is_jumpable(tick: &BrainTick<'_>, pos: &BlockPos) -> bool {
        tick.world
            .get_block(pos)
            .has_tag(&tag::Block::MINECRAFT_VILLAGER_BABIES_CAN_JUMP_ON_BED)
    }

    fn on_or_over_bed(tick: &BrainTick<'_>) -> bool {
        let pos = tick.mob.get_entity().block_pos.load();
        Self::is_jumpable(tick, &pos) || Self::is_jumpable(tick, &pos.down())
    }
}

impl Behavior for JumpOnBed {
    fn entry_conditions(&self) -> &[(MemoryModuleId, MemoryStatus)] {
        JUMP_ON_BED_CONDITIONS
    }

    // Vanilla overrides `timedOut` to never time out.
    fn min_duration(&self) -> i32 {
        NO_TIMEOUT
    }

    fn max_duration(&self) -> i32 {
        NO_TIMEOUT
    }

    fn check_extra_start_conditions(&mut self, tick: &mut BrainTick<'_>) -> bool {
        is_baby(tick.mob)
            && (Self::on_or_over_bed(tick) || tick.brain.has_memory_value(types::NEAREST_BED.id()))
    }

    fn can_still_use(&mut self, tick: &BrainTick<'_>) -> bool {
        let over_bed = Self::on_or_over_bed(tick);
        let tired_of_walking = !over_bed && self.remaining_time_to_reach_bed <= 0;
        let tired_of_jumping = over_bed && self.remaining_jumps <= 0;
        is_baby(tick.mob)
            && self
                .target_bed
                .is_some_and(|bed| Self::is_jumpable(tick, &bed))
            && !tired_of_walking
            && !tired_of_jumping
    }

    fn start(&mut self, tick: &mut BrainTick<'_>) {
        let Some(bed) = tick.brain.get(types::NEAREST_BED).copied() else {
            return;
        };
        self.target_bed = Some(bed);
        self.remaining_time_to_reach_bed = Self::MAX_TIME_TO_REACH_BED;
        self.remaining_jumps = Self::MIN_JUMPS
            + tick
                .mob
                .get_random()
                .random_range(0..=Self::MAX_JUMPS - Self::MIN_JUMPS);
        self.remaining_cooldown_until_next_jump = 0;
        tick.brain.set(
            types::WALK_TARGET,
            WalkTarget::from_block_pos(bed, self.speed_modifier, 0),
        );
    }

    fn tick(&mut self, tick: &mut BrainTick<'_>) {
        if !Self::on_or_over_bed(tick) {
            self.remaining_time_to_reach_bed -= 1;
        } else if self.remaining_cooldown_until_next_jump > 0 {
            self.remaining_cooldown_until_next_jump -= 1;
        } else if Self::is_jumpable(tick, &tick.mob.get_entity().block_pos.load()) {
            tick.mob
                .get_mob_entity()
                .living_entity
                .jumping
                .store(true, std::sync::atomic::Ordering::SeqCst);
            self.remaining_jumps -= 1;
            self.remaining_cooldown_until_next_jump = Self::COOLDOWN_BETWEEN_JUMPS;
        }
    }

    fn stop(&mut self, _tick: &mut BrainTick<'_>) {
        self.target_bed = None;
        self.remaining_time_to_reach_bed = 0;
        self.remaining_jumps = 0;
        self.remaining_cooldown_until_next_jump = 0;
    }

    fn debug_name(&self) -> &'static str {
        "JumpOnBed"
    }
}

const SLEEP_IN_BED_CONDITIONS: &[(MemoryModuleId, MemoryStatus)] = &[
    (types::HOME.id(), MemoryStatus::ValuePresent),
    (types::LAST_WOKEN.id(), MemoryStatus::Registered),
    (types::LAST_SLEPT.id(), MemoryStatus::Registered),
    (types::WALK_TARGET.id(), MemoryStatus::Registered),
    (
        types::CANT_REACH_WALK_TARGET_SINCE.id(),
        MemoryStatus::Registered,
    ),
];

/// Vanilla `SleepInBed`.
#[derive(Default)]
pub struct SleepInBed {
    next_ok_start_time: i64,
}

impl SleepInBed {
    pub const COOLDOWN_AFTER_BEING_WOKEN: i64 = 100;
    const RESTART_DELAY: i64 = 40;
}

impl Behavior for SleepInBed {
    fn entry_conditions(&self) -> &[(MemoryModuleId, MemoryStatus)] {
        SLEEP_IN_BED_CONDITIONS
    }

    // Vanilla overrides `timedOut` to never time out.
    fn min_duration(&self) -> i32 {
        NO_TIMEOUT
    }

    fn max_duration(&self) -> i32 {
        NO_TIMEOUT
    }

    fn check_extra_start_conditions(&mut self, tick: &mut BrainTick<'_>) -> bool {
        if tick.mob.get_entity().has_vehicle() {
            return false;
        }
        let Some(home) = tick.brain.get(types::HOME).copied() else {
            return false;
        };
        if global_pos_in(tick.world, home.pos) != Some(home) {
            return false;
        }
        if let Some(last_woken) = tick.brain.get(types::LAST_WOKEN).copied() {
            let since = tick.time - last_woken;
            if since > 0 && since < Self::COOLDOWN_AFTER_BEING_WOKEN {
                return false;
            }
        }
        let (block, state) = tick.world.get_block_and_state(&home.pos);
        closer_to_center_than(&home.pos, tick.mob.get_entity().pos.load(), 2.0)
            && block.has_tag(&tag::Block::MINECRAFT_VILLAGERS_CAN_SLEEP_ON_BED)
            && !BedProperties::from_state_id(state.id).occupied
    }

    fn can_still_use(&mut self, tick: &BrainTick<'_>) -> bool {
        let Some(home) = tick.brain.get(types::HOME) else {
            return false;
        };
        let body = tick.mob.get_entity().pos.load();
        tick.brain.is_active(Activity::Rest)
            && body.y > f64::from(home.pos.0.y) + 0.4
            && closer_to_center_than(&home.pos, body, 1.14)
    }

    fn start(&mut self, tick: &mut BrainTick<'_>) {
        if tick.time <= self.next_ok_start_time {
            return;
        }
        crate::entity::ai::brain::behavior::interact_with_door::close_doors_i_passed_through(tick);
        if let Some(home) = tick.brain.get(types::HOME).copied()
            && let Some(villager) = villager(tick)
            && villager.start_sleeping(tick, home.pos)
        {
            tick.brain.set(types::LAST_SLEPT, tick.time);
        }
        tick.brain.erase(types::WALK_TARGET.id());
        tick.brain.erase(types::CANT_REACH_WALK_TARGET_SINCE.id());
    }

    fn stop(&mut self, tick: &mut BrainTick<'_>) {
        if let Some(villager) = villager(tick)
            && villager.is_sleeping()
        {
            villager.stop_sleeping(tick);
            self.next_ok_start_time = tick.time + Self::RESTART_DELAY;
        }
    }

    fn debug_name(&self) -> &'static str {
        "SleepInBed"
    }
}

/// Vanilla `SetClosestHomeAsWalkTarget`: a villager without a bed heads for the nearest one
/// it can reach.
#[must_use]
pub fn set_closest_home_as_walk_target(speed_modifier: f32) -> OneShot {
    const CACHE_TIMEOUT: i64 = 40;
    const BATCH_SIZE: i32 = 5;
    const RATE: i64 = 20;
    const OK_DISTANCE_SQR: i64 = 4;
    let mut batch_cache: rustc_hash::FxHashMap<BlockPos, i64> = rustc_hash::FxHashMap::default();
    let mut last_update: i64 = 0;
    OneShot::new(
        "SetClosestHomeAsWalkTarget",
        vec![
            (types::WALK_TARGET.id(), MemoryStatus::ValueAbsent),
            (types::HOME.id(), MemoryStatus::ValueAbsent),
        ],
        move |tick| {
            if tick.time - last_update < RATE {
                return false;
            }
            let poi_manager = &tick.world.poi_manager;
            let center = tick.mob.get_entity().block_pos.load();
            let is_home = |poi_type| poi_type == PoiType::Home;
            let Some(closest) = poi_manager.find_closest(
                is_home,
                |_| true,
                &center,
                crate::entity::ai::brain::behavior::acquire_poi::SCAN_RANGE,
                crate::world::poi_manager::Occupancy::Any,
            ) else {
                return false;
            };
            let delta = closest.0 - center.0;
            if i64::from(delta.x).pow(2) + i64::from(delta.y).pow(2) + i64::from(delta.z).pow(2)
                <= OK_DISTANCE_SQR
            {
                return false;
            }
            let mut tried_count = 0;
            last_update = tick.time + i64::from(tick.mob.get_random().random_range(0..20));
            let pois = poi_manager.find_all_with_type(
                is_home,
                |pos| {
                    if batch_cache.contains_key(pos) {
                        return false;
                    }
                    tried_count += 1;
                    if tried_count >= BATCH_SIZE {
                        return false;
                    }
                    batch_cache.insert(*pos, last_update + CACHE_TIMEOUT);
                    true
                },
                &center,
                crate::entity::ai::brain::behavior::acquire_poi::SCAN_RANGE,
                crate::world::poi_manager::Occupancy::Any,
            );
            match crate::entity::ai::brain::behavior::acquire_poi::find_path_to_pois(
                tick.mob, &pois,
            )
            .filter(crate::entity::ai::pathfinder::path::Path::can_reach)
            {
                Some(path) => {
                    let target = path.get_target();
                    if poi_manager.get_type(&target).is_some() {
                        tick.brain.set(
                            types::WALK_TARGET,
                            WalkTarget::from_block_pos(target, speed_modifier, 1),
                        );
                    }
                }
                None if tried_count < BATCH_SIZE => {
                    batch_cache.retain(|_, expiry| *expiry >= last_update);
                }
                None => {}
            }
            true
        },
    )
}

/// Vanilla `InsideBrownianWalk`: shuffles around under a roof, one block at a time.
#[must_use]
pub fn inside_brownian_walk(speed_modifier: f32) -> OneShot {
    OneShot::new(
        "InsideBrownianWalk",
        vec![(types::WALK_TARGET.id(), MemoryStatus::ValueAbsent)],
        move |tick| {
            let body_pos = tick.mob.get_entity().block_pos.load();
            if tick.world.can_see_sky(&body_pos) {
                return false;
            }
            let mut poses = Vec::with_capacity(27);
            for x in -1..=1 {
                for y in -1..=1 {
                    for z in -1..=1 {
                        poses.push(body_pos.offset(Vector3::new(x, y, z)));
                    }
                }
            }
            poses.shuffle(&mut tick.mob.get_random());
            let body_box = tick.mob.get_entity().bounding_box.load();
            // Vanilla checks the body's own box here, not the target's.
            let target = poses.into_iter().find(|pos| {
                !tick.world.can_see_sky(pos)
                    && tick
                        .world
                        .get_block_state_if_loaded(pos)
                        .is_some_and(|state| state.is_side_solid(BlockDirection::Up))
                    && tick.world.is_space_empty(body_box)
            });
            if let Some(target) = target {
                tick.brain.set(
                    types::WALK_TARGET,
                    WalkTarget::from_block_pos(target, speed_modifier, 0),
                );
            }
            true
        },
    )
}

/// Vanilla `GoToClosestVillage`: a villager outside a village walks toward one.
#[must_use]
pub fn go_to_closest_village(speed_modifier: f32, close_enough_distance: i32) -> OneShot {
    OneShot::new(
        "GoToClosestVillage",
        vec![(types::WALK_TARGET.id(), MemoryStatus::ValueAbsent)],
        move |tick| {
            let body_pos = tick.mob.get_entity().block_pos.load();
            let poi_manager = &tick.world.poi_manager;
            if poi_manager.is_village(&body_pos) {
                return false;
            }
            let sections_to_village = poi_manager.sections_to_village(section_of(&body_pos));
            let mut target_pos = None;
            for _ in 0..5 {
                let Some(land_pos) = land_random_pos::get_pos_weighted(tick.mob, 15, 7, |pos| {
                    -f64::from(poi_manager.sections_to_village(section_of(pos)))
                }) else {
                    continue;
                };
                let land_sections =
                    poi_manager.sections_to_village(section_of(&BlockPos::floored_v(land_pos)));
                if land_sections < sections_to_village {
                    target_pos = Some(land_pos);
                    break;
                }
                if land_sections == sections_to_village {
                    target_pos = Some(land_pos);
                }
            }
            if let Some(pos) = target_pos {
                tick.brain.set(
                    types::WALK_TARGET,
                    WalkTarget::from_vec(pos, speed_modifier, close_enough_distance),
                );
            }
            true
        },
    )
}

/// Vanilla `VillageBoundRandomStroll`: strolls within a village, or drifts back toward one.
#[must_use]
pub fn village_bound_random_stroll(speed_modifier: f32) -> OneShot {
    village_bound_random_stroll_with_range(speed_modifier, 10, 7)
}

#[must_use]
pub fn village_bound_random_stroll_with_range(
    speed_modifier: f32,
    max_xz_dist: i32,
    max_y_dist: i32,
) -> OneShot {
    OneShot::new(
        "VillageBoundRandomStroll",
        vec![(types::WALK_TARGET.id(), MemoryStatus::ValueAbsent)],
        move |tick| {
            let body_pos = tick.mob.get_entity().block_pos.load();
            let poi_manager = &tick.world.poi_manager;
            let land_pos = if poi_manager.is_village(&body_pos) {
                land_random_pos::get_pos(tick.mob, max_xz_dist, max_y_dist)
            } else {
                let section = section_of(&body_pos);
                let optimal = poi_manager.find_section_closest_to_village(section, 2);
                if optimal == section {
                    land_random_pos::get_pos(tick.mob, max_xz_dist, max_y_dist)
                } else {
                    // `Vec3.atBottomCenterOf(optimalSectionPos.center())`.
                    let towards = Vector3::new(
                        f64::from((optimal.0 << 4) + 8) + 0.5,
                        f64::from((optimal.1 << 4) + 8),
                        f64::from((optimal.2 << 4) + 8) + 0.5,
                    );
                    default_random_pos::get_pos_towards(
                        tick.mob,
                        max_xz_dist,
                        max_y_dist,
                        towards,
                        std::f64::consts::FRAC_PI_2,
                    )
                }
            };
            match land_pos {
                Some(pos) => tick.brain.set(
                    types::WALK_TARGET,
                    WalkTarget::from_vec(pos, speed_modifier, 0),
                ),
                None => tick.brain.erase(types::WALK_TARGET.id()),
            }
            true
        },
    )
}

/// Vanilla `VillagerCalmDown`: once nothing is scaring it, forgets who hurt it and goes back to
/// its schedule.
#[must_use]
pub fn villager_calm_down() -> OneShot {
    const SAFE_DISTANCE_FROM_DANGER: f64 = 36.0;
    OneShot::new(
        "VillagerCalmDown",
        vec![
            (types::HURT_BY.id(), MemoryStatus::Registered),
            (types::HURT_BY_ENTITY.id(), MemoryStatus::Registered),
            (types::NEAREST_HOSTILE.id(), MemoryStatus::Registered),
        ],
        |tick| {
            let body = tick.mob.get_entity().pos.load();
            let feel_scared = tick.brain.has_memory_value(types::HURT_BY.id())
                || tick.brain.has_memory_value(types::NEAREST_HOSTILE.id())
                || tick.brain.get(types::HURT_BY_ENTITY).is_some_and(|entity| {
                    entity
                        .get_entity()
                        .pos
                        .load()
                        .squared_distance_to_vec(&body)
                        <= SAFE_DISTANCE_FROM_DANGER
                });
            if !feel_scared {
                tick.brain.erase(types::HURT_BY.id());
                tick.brain.erase(types::HURT_BY_ENTITY.id());
                tick.brain
                    .update_activity_from_schedule(tick.world, tick.time);
            }
            true
        },
    )
}

/// Vanilla `UpdateActivityFromSchedule`.
#[must_use]
pub fn update_activity_from_schedule() -> OneShot {
    OneShot::new("UpdateActivityFromSchedule", Vec::new(), |tick| {
        tick.brain
            .update_activity_from_schedule(tick.world, tick.time);
        true
    })
}

/// The parts of vanilla `ServerLevel.getRaidAt` the villager behaviours read, copied out so
/// the raid lock is held only for the lookup.
#[derive(Clone, Copy)]
pub struct RaidState {
    pub id: i32,
    pub active: bool,
    pub over: bool,
    pub victory: bool,
    pub loss: bool,
    pub stopped: bool,
    pub first_wave_spawned: bool,
    pub between_waves: bool,
}

/// Vanilla `ServerLevel.getRaidAt`.
#[must_use]
pub fn raid_at(world: &crate::world::World, pos: &BlockPos) -> Option<RaidState> {
    let raids = world
        .raids
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    raids
        .get_nearby_raid(pos, crate::world::raid::Raid::VALID_RAID_RADIUS_SQR)
        .map(|raid| RaidState {
            id: raid.id,
            active: raid.is_active(),
            over: raid.is_over(),
            victory: raid.is_victory(),
            loss: raid.is_loss(),
            stopped: raid.is_stopped(),
            first_wave_spawned: raid.has_first_wave_spawned(),
            between_waves: raid.is_between_waves(),
        })
}

/// Vanilla `ReactToBell`.
#[must_use]
pub fn react_to_bell() -> OneShot {
    OneShot::new(
        "ReactToBell",
        vec![(types::HEARD_BELL_TIME.id(), MemoryStatus::ValuePresent)],
        |tick| {
            if raid_at(tick.world, &tick.mob.get_entity().block_pos.load()).is_none() {
                tick.brain.set_active_activity_if_possible(Activity::Hide);
            }
            true
        },
    )
}

/// Vanilla `SetRaidStatus`.
#[must_use]
pub fn set_raid_status() -> OneShot {
    OneShot::new("SetRaidStatus", Vec::new(), |tick| {
        if tick.mob.get_random().random_range(0..20) != 0 {
            return false;
        }
        if let Some(raid) = raid_at(tick.world, &tick.mob.get_entity().block_pos.load()) {
            let activity = if raid.first_wave_spawned && !raid.between_waves {
                Activity::Raid
            } else {
                Activity::PreRaid
            };
            tick.brain.set_default_activity(activity);
            tick.brain.set_active_activity_if_possible(activity);
        }
        true
    })
}

/// Vanilla `ResetRaidStatus`.
#[must_use]
pub fn reset_raid_status() -> OneShot {
    OneShot::new("ResetRaidStatus", Vec::new(), |tick| {
        if tick.mob.get_random().random_range(0..20) != 0 {
            return false;
        }
        let raid = raid_at(tick.world, &tick.mob.get_entity().block_pos.load());
        if raid.is_none_or(|raid| raid.stopped || raid.loss) {
            tick.brain.set_default_activity(Activity::Idle);
            tick.brain
                .update_activity_from_schedule(tick.world, tick.time);
        }
        true
    })
}

/// Vanilla `RingBell`: a villager at the meeting point now and then rings its bell.
#[must_use]
pub fn ring_bell() -> OneShot {
    const BELL_RING_CHANCE: f32 = 0.95;
    const RING_BELL_FROM_DISTANCE: i32 = 3;
    OneShot::new(
        "RingBell",
        vec![(types::MEETING_POINT.id(), MemoryStatus::ValuePresent)],
        |tick| {
            if tick.mob.get_random().random::<f32>() <= BELL_RING_CHANCE {
                return false;
            }
            let Some(meeting) = tick.brain.get(types::MEETING_POINT).copied() else {
                return false;
            };
            let pos = meeting.pos;
            if pos.squared_distance(&tick.mob.get_entity().block_pos.load())
                < RING_BELL_FROM_DISTANCE * RING_BELL_FROM_DISTANCE
                && tick.world.get_block(&pos) == &Block::BELL
            {
                let ringer = villager(tick).and_then(|body| {
                    body.self_weak
                        .lock()
                        .unwrap_or_else(std::sync::PoisonError::into_inner)
                        .as_ref()
                        .and_then(std::sync::Weak::upgrade)
                        .map(|body| body as Arc<dyn EntityBase>)
                });
                crate::block::blocks::redstone::bell::ring_bell(pos, tick.world, None, ringer);
            }
            true
        },
    )
}

/// Vanilla `MoveToSkySeeingSpot.hasNoBlocksAbove`.
#[must_use]
pub fn has_no_blocks_above(tick: &BrainTick<'_>, target: &BlockPos) -> bool {
    tick.world.can_see_sky(target)
        && f64::from(tick.world.get_heightmap_height(
            pumpkin_world::chunk::ChunkHeightmapType::MotionBlocking,
            target.0.x,
            target.0.z,
        )) <= tick.mob.get_entity().pos.load().y
}

/// Vanilla `MoveToSkySeeingSpot`: heads outdoors to watch a raid end.
#[must_use]
pub fn move_to_sky_seeing_spot(speed_modifier: f32) -> OneShot {
    OneShot::new(
        "MoveToSkySeeingSpot",
        vec![(types::WALK_TARGET.id(), MemoryStatus::ValueAbsent)],
        move |tick| {
            let body_pos = tick.mob.get_entity().block_pos.load();
            if tick.world.can_see_sky(&body_pos) {
                return false;
            }
            // Vanilla `getOutdoorPosition`.
            for _ in 0..10 {
                let offset = {
                    let mut rng = tick.mob.get_random();
                    Vector3::new(
                        rng.random_range(0..20) - 10,
                        rng.random_range(0..6) - 3,
                        rng.random_range(0..20) - 10,
                    )
                };
                let candidate = body_pos.offset(offset);
                if has_no_blocks_above(tick, &candidate) {
                    tick.brain.set(
                        types::WALK_TARGET,
                        WalkTarget::from_vec(
                            Vector3::new(
                                f64::from(candidate.0.x) + 0.5,
                                f64::from(candidate.0.y),
                                f64::from(candidate.0.z) + 0.5,
                            ),
                            speed_modifier,
                            0,
                        ),
                    );
                    break;
                }
            }
            true
        },
    )
}

/// Vanilla `CelebrateVillagersSurvivedRaid`, without the fireworks: Pumpkin's rocket entity
/// cannot carry the firework item that gives the explosion its colour yet.
pub struct CelebrateVillagersSurvivedRaid {
    duration: i32,
    current_raid: Option<i32>,
}

impl CelebrateVillagersSurvivedRaid {
    #[must_use]
    pub const fn new(duration: i32) -> Self {
        Self {
            duration,
            current_raid: None,
        }
    }
}

impl Behavior for CelebrateVillagersSurvivedRaid {
    fn entry_conditions(&self) -> &[(MemoryModuleId, MemoryStatus)] {
        &[]
    }

    fn min_duration(&self) -> i32 {
        self.duration
    }

    fn max_duration(&self) -> i32 {
        self.duration
    }

    fn check_extra_start_conditions(&mut self, tick: &mut BrainTick<'_>) -> bool {
        let test_pos = tick.mob.get_entity().block_pos.load();
        let raid = raid_at(tick.world, &test_pos);
        self.current_raid = raid.map(|raid| raid.id);
        raid.is_some_and(|raid| raid.victory) && has_no_blocks_above(tick, &test_pos)
    }

    fn can_still_use(&mut self, tick: &BrainTick<'_>) -> bool {
        self.current_raid.is_some_and(|id| {
            tick.world
                .raids
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .get(id)
                .is_some_and(|raid| !raid.is_stopped())
        })
    }

    fn tick(&mut self, tick: &mut BrainTick<'_>) {
        if tick.mob.get_random().random_range(0..100) == 0 {
            tick.mob
                .get_entity()
                .play_sound(pumpkin_data::sound::Sound::EntityVillagerCelebrate);
        }
    }

    fn stop(&mut self, tick: &mut BrainTick<'_>) {
        self.current_raid = None;
        tick.brain
            .update_activity_from_schedule(tick.world, tick.time);
    }

    fn debug_name(&self) -> &'static str {
        "CelebrateVillagersSurvivedRaid"
    }
}

/// Vanilla `LocateHidingPlace`: runs for a bed right here, else any bed around, else its own.
#[must_use]
pub fn locate_hiding_place(radius: i32, speed_modifier: f32, close_enough_dist: i32) -> OneShot {
    OneShot::new(
        "LocateHidingPlace",
        vec![
            (types::WALK_TARGET.id(), MemoryStatus::ValueAbsent),
            (types::HOME.id(), MemoryStatus::Registered),
            (types::HIDING_PLACE.id(), MemoryStatus::Registered),
            (types::PATH.id(), MemoryStatus::Registered),
            (types::LOOK_TARGET.id(), MemoryStatus::Registered),
            (types::BREED_TARGET.id(), MemoryStatus::Registered),
            (types::INTERACTION_TARGET.id(), MemoryStatus::Registered),
        ],
        move |tick| {
            let body = tick.mob.get_entity().pos.load();
            let body_pos = tick.mob.get_entity().block_pos.load();
            let close_enough = f64::from(close_enough_dist);
            let is_home = |poi_type| poi_type == PoiType::Home;
            let poi_manager = &tick.world.poi_manager;
            // Vanilla `PoiManager.find`: the first match in search order, not the closest.
            let hiding_place = poi_manager
                .find_all_with_type(
                    is_home,
                    |_| true,
                    &body_pos,
                    close_enough_dist + 1,
                    crate::world::poi_manager::Occupancy::Any,
                )
                .first()
                .map(|(_, pos)| *pos)
                .filter(|pos| closer_to_center_than(pos, body, close_enough))
                .or_else(|| {
                    poi_manager.get_random(
                        is_home,
                        |_| true,
                        crate::world::poi_manager::Occupancy::Any,
                        &body_pos,
                        radius,
                        &mut tick.mob.get_random(),
                    )
                })
                .or_else(|| tick.brain.get(types::HOME).map(|home| home.pos));
            if let Some(pos) = hiding_place
                && let Some(global) = global_pos_in(tick.world, pos)
            {
                tick.brain.erase(types::PATH.id());
                tick.brain.erase(types::LOOK_TARGET.id());
                tick.brain.erase(types::BREED_TARGET.id());
                tick.brain.erase(types::INTERACTION_TARGET.id());
                tick.brain.set(types::HIDING_PLACE, global);
                if !closer_to_center_than(&pos, body, close_enough) {
                    tick.brain.set(
                        types::WALK_TARGET,
                        WalkTarget::from_block_pos(pos, speed_modifier, close_enough_dist),
                    );
                }
            }
            true
        },
    )
}

/// Vanilla `SetHiddenState`: stays hidden for `seconds`, or gives up 15 seconds after the bell,
/// then goes back to the schedule.
#[must_use]
pub fn set_hidden_state(seconds: i32, close_enough_dist: i32) -> OneShot {
    const HIDE_TIMEOUT: i64 = 300;
    let stay_hidden_ticks = seconds * 20;
    let mut ticks_hidden = 0;
    OneShot::new(
        "SetHiddenState",
        vec![
            (types::HIDING_PLACE.id(), MemoryStatus::ValuePresent),
            (types::HEARD_BELL_TIME.id(), MemoryStatus::ValuePresent),
        ],
        move |tick| {
            let (Some(time_triggered), Some(hiding_place)) = (
                tick.brain.get(types::HEARD_BELL_TIME).copied(),
                tick.brain.get(types::HIDING_PLACE).copied(),
            ) else {
                return false;
            };
            let timed_out_trying_to_hide = time_triggered + HIDE_TIMEOUT <= tick.time;
            if ticks_hidden <= stay_hidden_ticks && !timed_out_trying_to_hide {
                if hiding_place
                    .pos
                    .squared_distance(&tick.mob.get_entity().block_pos.load())
                    < close_enough_dist * close_enough_dist
                {
                    ticks_hidden += 1;
                }
            } else {
                tick.brain.erase(types::HEARD_BELL_TIME.id());
                tick.brain.erase(types::HIDING_PLACE.id());
                tick.brain
                    .update_activity_from_schedule(tick.world, tick.time);
                ticks_hidden = 0;
            }
            true
        },
    )
}
