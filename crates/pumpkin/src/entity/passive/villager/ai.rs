//! Vanilla `VillagerGoalPackages` and the villager's `Brain.Provider`.

use pumpkin_data::entity::{EntityStatus, EntityType, MobCategory};
use pumpkin_data::environment_attribute::Activity;
use pumpkin_data::tag::{self, Taggable};
use pumpkin_util::math::position::BlockPos;

use crate::entity::ageable::is_baby;
use crate::entity::ai::brain::activity::ActivityData;
use crate::entity::ai::brain::behavior::{
    self, BehaviorControl, DoNothing, GateBehavior, OrderPolicy, RunningPolicy, Timed,
};
use crate::entity::ai::brain::memory::{MemoryModuleId, MemoryStatus, types};
use crate::entity::ai::brain::sensing::SensorType;
use crate::entity::ai::brain::{BrainProvider, BrainTick};
use crate::entity::mob::Mob;
use crate::world::World;
use crate::world::poi_manager::PoiType;

use crate::entity::ai::brain::behavior::one_shot::sequence;

use super::behaviors::{
    CelebrateVillagersSurvivedRaid, GiveGiftToHero, GoToPotentialJobSite, HarvestFarmland,
    JumpOnBed, LookAndFollowTradingPlayerSink, ShowTradesToPlayer, SleepInBed, TradeWithVillager,
    UseBonemeal, VillagerMakeLove, VillagerPanicTrigger, WorkAtPoi,
    assign_profession_from_job_site, can_acquire_job_site, go_to_closest_village, holds_job_site,
    inside_brownian_walk, locate_hiding_place, move_to_sky_seeing_spot, play_tag_with_other_kids,
    poi_competitor_scan, raid_at, react_to_bell, reset_profession, reset_raid_status, ring_bell,
    set_closest_home_as_walk_target, set_hidden_state, set_raid_status,
    set_walk_target_from_block_memory, socialize_at_bell, stroll_to_poi_list,
    update_activity_from_schedule, validate_nearby_poi, village_bound_random_stroll,
    village_bound_random_stroll_with_range, villager_calm_down, wake_up, yield_job_site,
};
use super::{VillagerEntity, VillagerProfession, as_villager};

/// Vanilla `Villager.SPEED_MODIFIER`.
pub const SPEED_MODIFIER: f32 = 0.5;
const STROLL_SPEED_MODIFIER: f32 = 0.4;

type Package = Vec<(i32, Box<dyn BehaviorControl>)>;

/// Vanilla `VillagerGoalPackages.validateBedPoi`.
fn validate_bed_poi(world: &World, pos: &BlockPos) -> bool {
    let (block, state) = world.get_block_and_state(pos);
    block.has_tag(&tag::Block::MINECRAFT_VILLAGERS_CAN_SLEEP_ON_BED)
        && !pumpkin_data::block_properties::WhiteBedLikeProperties::from_state_id(state.id).occupied
}

const fn villager_happy() -> EntityStatus {
    EntityStatus::VillagerHappy
}

fn get_core_package(profession: VillagerProfession, speed_modifier: f32) -> Package {
    vec![
        (0, Box::new(Timed::new(behavior::Swim::new(0.8)))),
        (0, Box::new(behavior::interact_with_door())),
        (
            0,
            Box::new(Timed::new(behavior::LookAtTargetSink::new(45, 90))),
        ),
        (0, Box::new(Timed::new(VillagerPanicTrigger))),
        (0, Box::new(wake_up())),
        (0, Box::new(react_to_bell())),
        (0, Box::new(set_raid_status())),
        (
            0,
            Box::new(validate_nearby_poi(
                move |poi_type| holds_job_site(profession, poi_type),
                types::JOB_SITE,
            )),
        ),
        (
            0,
            Box::new(validate_nearby_poi(
                move |poi_type| can_acquire_job_site(profession, poi_type),
                types::POTENTIAL_JOB_SITE,
            )),
        ),
        (
            1,
            Box::new(Timed::new(behavior::MoveToTargetSink::default())),
        ),
        (2, Box::new(poi_competitor_scan())),
        (
            3,
            Box::new(Timed::new(LookAndFollowTradingPlayerSink::new(
                speed_modifier,
            ))),
        ),
        (
            5,
            Box::new(behavior::go_to_wanted_item(
                |_| true,
                speed_modifier,
                false,
                4,
            )),
        ),
        (
            6,
            Box::new(behavior::acquire_poi(
                move |poi_type| can_acquire_job_site(profession, poi_type),
                types::JOB_SITE,
                types::POTENTIAL_JOB_SITE,
                true,
                None,
                |_, _| true,
            )),
        ),
        (
            7,
            Box::new(Timed::new(GoToPotentialJobSite::new(speed_modifier))),
        ),
        (8, Box::new(yield_job_site(speed_modifier))),
        (
            10,
            Box::new(behavior::acquire_poi(
                |poi_type| poi_type == PoiType::Home,
                types::HOME,
                types::HOME,
                false,
                Some(villager_happy),
                validate_bed_poi,
            )),
        ),
        (
            10,
            Box::new(behavior::acquire_poi(
                |poi_type| poi_type == PoiType::Meeting,
                types::MEETING_POINT,
                types::MEETING_POINT,
                true,
                Some(villager_happy),
                |_, _| true,
            )),
        ),
        (10, Box::new(assign_profession_from_job_site())),
        (10, Box::new(reset_profession())),
    ]
}

fn get_work_package(profession: VillagerProfession, speed_modifier: f32) -> Package {
    let is_farmer = profession == VillagerProfession::Farmer;
    vec![
        get_minimal_look_behavior(),
        (
            5,
            Box::new(behavior::run_one(vec![
                (Box::new(Timed::new(WorkAtPoi::new(is_farmer))), 7),
                (
                    Box::new(behavior::stroll_around_poi(
                        types::JOB_SITE,
                        STROLL_SPEED_MODIFIER,
                        4,
                    )),
                    2,
                ),
                (
                    Box::new(behavior::stroll_to_poi(
                        types::JOB_SITE,
                        STROLL_SPEED_MODIFIER,
                        1,
                        10,
                    )),
                    5,
                ),
                (
                    Box::new(stroll_to_poi_list(
                        types::SECONDARY_JOB_SITE,
                        speed_modifier,
                        1,
                        6,
                        types::JOB_SITE,
                    )),
                    5,
                ),
                (
                    Box::new(Timed::new(HarvestFarmland::default())),
                    if is_farmer { 2 } else { 5 },
                ),
                (
                    Box::new(Timed::new(UseBonemeal::default())),
                    if is_farmer { 4 } else { 7 },
                ),
            ])),
        ),
        (10, Box::new(Timed::new(ShowTradesToPlayer::new(400, 1600)))),
        (
            10,
            Box::new(behavior::set_look_and_interact(&EntityType::PLAYER, 4)),
        ),
        (
            2,
            Box::new(set_walk_target_from_block_memory(
                types::JOB_SITE,
                speed_modifier,
                9,
                100,
                1200,
            )),
        ),
        (3, Box::new(Timed::new(GiveGiftToHero::new(100)))),
        (99, Box::new(update_activity_from_schedule())),
    ]
}

fn interact_with_villager(speed_modifier: f32) -> Box<dyn BehaviorControl> {
    Box::new(behavior::interact_with(
        &EntityType::VILLAGER,
        8,
        |_| true,
        |_| true,
        types::INTERACTION_TARGET,
        speed_modifier,
        2,
    ))
}

fn interact_with_cat(speed_modifier: f32) -> Box<dyn BehaviorControl> {
    Box::new(behavior::interact_with(
        &EntityType::CAT,
        8,
        |_| true,
        |_| true,
        types::INTERACTION_TARGET,
        speed_modifier,
        2,
    ))
}

fn get_play_package(speed_modifier: f32) -> Package {
    vec![
        (
            0,
            Box::new(Timed::new(behavior::MoveToTargetSink::new(80, 120))),
        ),
        get_full_look_behavior(),
        (5, Box::new(play_tag_with_other_kids())),
        (
            5,
            Box::new(behavior::run_one_with_conditions(
                vec![(
                    types::VISIBLE_VILLAGER_BABIES.id(),
                    MemoryStatus::ValueAbsent,
                )],
                vec![
                    (interact_with_villager(speed_modifier), 2),
                    (interact_with_cat(speed_modifier), 1),
                    (Box::new(village_bound_random_stroll(speed_modifier)), 1),
                    (
                        Box::new(behavior::set_walk_target_from_look_target(
                            |_| true,
                            speed_modifier,
                            2,
                        )),
                        1,
                    ),
                    (Box::new(Timed::new(JumpOnBed::new(speed_modifier))), 2),
                    (Box::new(DoNothing::new(20, 40)), 2),
                ],
            )),
        ),
        (99, Box::new(update_activity_from_schedule())),
    ]
}

fn get_rest_package(speed_modifier: f32) -> Package {
    vec![
        (
            2,
            Box::new(set_walk_target_from_block_memory(
                types::HOME,
                speed_modifier,
                1,
                150,
                1200,
            )),
        ),
        (
            3,
            Box::new(validate_nearby_poi(
                |poi_type| poi_type == PoiType::Home,
                types::HOME,
            )),
        ),
        (3, Box::new(Timed::new(SleepInBed::default()))),
        (
            5,
            Box::new(behavior::run_one_with_conditions(
                vec![(types::HOME.id(), MemoryStatus::ValueAbsent)],
                vec![
                    (Box::new(set_closest_home_as_walk_target(speed_modifier)), 1),
                    (Box::new(inside_brownian_walk(speed_modifier)), 4),
                    (Box::new(go_to_closest_village(speed_modifier, 4)), 2),
                    (Box::new(DoNothing::new(20, 40)), 2),
                ],
            )),
        ),
        get_minimal_look_behavior(),
        (99, Box::new(update_activity_from_schedule())),
    ]
}

fn trade_with_villager_gate() -> Box<dyn BehaviorControl> {
    Box::new(GateBehavior::new(
        "GateBehavior",
        Vec::new(),
        vec![types::INTERACTION_TARGET.id()],
        OrderPolicy::Ordered,
        RunningPolicy::RunOne,
        vec![(Box::new(Timed::new(TradeWithVillager::default())), 1)],
    ))
}

fn get_meet_package(speed_modifier: f32) -> Package {
    vec![
        (
            2,
            Box::new(behavior::trigger_one_shuffled_of(
                "TriggerGate",
                vec![
                    (
                        behavior::stroll_around_poi(
                            types::MEETING_POINT,
                            STROLL_SPEED_MODIFIER,
                            40,
                        ),
                        2,
                    ),
                    (socialize_at_bell(), 2),
                ],
            )),
        ),
        (10, Box::new(Timed::new(ShowTradesToPlayer::new(400, 1600)))),
        (
            10,
            Box::new(behavior::set_look_and_interact(&EntityType::PLAYER, 4)),
        ),
        (
            2,
            Box::new(set_walk_target_from_block_memory(
                types::MEETING_POINT,
                speed_modifier,
                6,
                100,
                200,
            )),
        ),
        (3, Box::new(Timed::new(GiveGiftToHero::new(100)))),
        (
            3,
            Box::new(validate_nearby_poi(
                |poi_type| poi_type == PoiType::Meeting,
                types::MEETING_POINT,
            )),
        ),
        (3, trade_with_villager_gate()),
        get_full_look_behavior(),
        (99, Box::new(update_activity_from_schedule())),
    ]
}

fn can_breed(tick: &BrainTick<'_>) -> bool {
    as_villager(tick.mob).is_some_and(VillagerEntity::can_breed)
}

fn get_idle_package(speed_modifier: f32) -> Package {
    vec![
        (
            2,
            Box::new(behavior::run_one(vec![
                (interact_with_villager(speed_modifier), 2),
                (
                    Box::new(behavior::interact_with(
                        &EntityType::VILLAGER,
                        8,
                        can_breed,
                        |target| {
                            as_villager(target.as_ref()).is_some_and(VillagerEntity::can_breed)
                        },
                        types::BREED_TARGET,
                        speed_modifier,
                        2,
                    )),
                    1,
                ),
                (interact_with_cat(speed_modifier), 1),
                (Box::new(village_bound_random_stroll(speed_modifier)), 1),
                (
                    Box::new(behavior::set_walk_target_from_look_target(
                        |_| true,
                        speed_modifier,
                        2,
                    )),
                    1,
                ),
                (Box::new(Timed::new(JumpOnBed::new(speed_modifier))), 1),
                (Box::new(DoNothing::new(30, 60)), 1),
            ])),
        ),
        (3, Box::new(Timed::new(GiveGiftToHero::new(100)))),
        (
            3,
            Box::new(behavior::set_look_and_interact(&EntityType::PLAYER, 4)),
        ),
        (3, Box::new(Timed::new(ShowTradesToPlayer::new(400, 1600)))),
        (3, trade_with_villager_gate()),
        (
            3,
            Box::new(GateBehavior::new(
                "GateBehavior",
                Vec::new(),
                vec![types::BREED_TARGET.id()],
                OrderPolicy::Ordered,
                RunningPolicy::RunOne,
                vec![(Box::new(Timed::new(VillagerMakeLove::default())), 1)],
            )),
        ),
        get_full_look_behavior(),
        (99, Box::new(update_activity_from_schedule())),
    ]
}

fn get_panic_package(speed_modifier: f32) -> Package {
    let runaway_speed = speed_modifier * 1.5;
    vec![
        (0, Box::new(villager_calm_down())),
        (
            1,
            Box::new(behavior::set_walk_target_away_from_entity(
                types::NEAREST_HOSTILE,
                runaway_speed,
                6,
                false,
            )),
        ),
        (
            1,
            Box::new(behavior::set_walk_target_away_from_entity(
                types::HURT_BY_ENTITY,
                runaway_speed,
                6,
                false,
            )),
        ),
        (
            3,
            Box::new(village_bound_random_stroll_with_range(runaway_speed, 2, 2)),
        ),
        get_minimal_look_behavior(),
    ]
}

fn get_pre_raid_package(speed_modifier: f32) -> Package {
    vec![
        (0, Box::new(ring_bell())),
        (
            0,
            Box::new(behavior::trigger_one_shuffled_of(
                "TriggerGate",
                vec![
                    (
                        set_walk_target_from_block_memory(
                            types::MEETING_POINT,
                            speed_modifier * 1.5,
                            2,
                            150,
                            200,
                        ),
                        6,
                    ),
                    (village_bound_random_stroll(speed_modifier * 1.5), 2),
                ],
            )),
        ),
        get_minimal_look_behavior(),
        (99, Box::new(reset_raid_status())),
    ]
}

/// Vanilla `raidExistsAndNotVictory`, which despite its name wants a won raid.
fn raid_exists_and_not_victory(tick: &mut BrainTick<'_>) -> bool {
    raid_at(tick.world, &tick.mob.get_entity().block_pos.load()).is_some_and(|raid| raid.victory)
}

/// Vanilla `raidExistsAndActive`.
fn raid_exists_and_active(tick: &mut BrainTick<'_>) -> bool {
    raid_at(tick.world, &tick.mob.get_entity().block_pos.load())
        .is_some_and(|raid| raid.active && !raid.victory && !raid.loss)
}

fn get_raid_package(speed_modifier: f32) -> Package {
    vec![
        (
            0,
            Box::new(sequence(
                "Sequence",
                raid_exists_and_not_victory,
                behavior::trigger_one_shuffled_of(
                    "TriggerGate",
                    vec![
                        (move_to_sky_seeing_spot(speed_modifier), 5),
                        (village_bound_random_stroll(speed_modifier * 1.1), 2),
                    ],
                ),
            )),
        ),
        (
            0,
            Box::new(Timed::new(CelebrateVillagersSurvivedRaid::new(600))),
        ),
        (
            2,
            Box::new(sequence(
                "Sequence",
                raid_exists_and_active,
                locate_hiding_place(24, speed_modifier * 1.4, 1),
            )),
        ),
        get_minimal_look_behavior(),
        (99, Box::new(reset_raid_status())),
    ]
}

fn get_hide_package(speed_modifier: f32) -> Package {
    vec![
        (0, Box::new(set_hidden_state(15, 3))),
        (
            1,
            Box::new(locate_hiding_place(32, speed_modifier * 1.25, 2)),
        ),
        get_minimal_look_behavior(),
    ]
}

fn get_full_look_behavior() -> (i32, Box<dyn BehaviorControl>) {
    let category = |category, weight| -> (Box<dyn BehaviorControl>, i32) {
        (
            Box::new(behavior::set_entity_look_target_of_category(category, 8.0)),
            weight,
        )
    };
    (
        5,
        Box::new(behavior::run_one(vec![
            (
                Box::new(behavior::set_entity_look_target_of_type(
                    &EntityType::CAT,
                    8.0,
                )),
                8,
            ),
            (
                Box::new(behavior::set_entity_look_target_of_type(
                    &EntityType::VILLAGER,
                    8.0,
                )),
                2,
            ),
            (
                Box::new(behavior::set_entity_look_target_of_type(
                    &EntityType::PLAYER,
                    8.0,
                )),
                2,
            ),
            category(&MobCategory::CREATURE, 1),
            category(&MobCategory::WATER_CREATURE, 1),
            category(&MobCategory::AXOLOTLS, 1),
            category(&MobCategory::UNDERGROUND_WATER_CREATURE, 1),
            category(&MobCategory::WATER_AMBIENT, 1),
            category(&MobCategory::MONSTER, 1),
            (Box::new(DoNothing::new(30, 60)), 2),
        ])),
    )
}

fn get_minimal_look_behavior() -> (i32, Box<dyn BehaviorControl>) {
    (
        5,
        Box::new(behavior::run_one(vec![
            (
                Box::new(behavior::set_entity_look_target_of_type(
                    &EntityType::VILLAGER,
                    8.0,
                )),
                2,
            ),
            (
                Box::new(behavior::set_entity_look_target_of_type(
                    &EntityType::PLAYER,
                    8.0,
                )),
                2,
            ),
            (Box::new(DoNothing::new(30, 60)), 8),
        ])),
    )
}

/// The activities vanilla's `BRAIN_PROVIDER` builds for this villager's profession and age.
fn activities(mob: &dyn Mob) -> Vec<ActivityData> {
    let profession = as_villager(mob).map_or(VillagerProfession::None, VillagerEntity::profession);
    let mut activities = Vec::with_capacity(6);
    if is_baby(mob) {
        activities.push(ActivityData::with_pairs(
            Activity::Play,
            get_play_package(SPEED_MODIFIER),
        ));
    } else {
        activities.push(ActivityData::with_pairs_and_conditions(
            Activity::Work,
            get_work_package(profession, SPEED_MODIFIER),
            vec![(types::JOB_SITE.id(), MemoryStatus::ValuePresent)],
        ));
    }
    activities.push(ActivityData::with_pairs(
        Activity::Core,
        get_core_package(profession, SPEED_MODIFIER),
    ));
    activities.push(ActivityData::with_pairs_and_conditions(
        Activity::Meet,
        get_meet_package(SPEED_MODIFIER),
        vec![(types::MEETING_POINT.id(), MemoryStatus::ValuePresent)],
    ));
    activities.push(ActivityData::with_pairs(
        Activity::Rest,
        get_rest_package(SPEED_MODIFIER),
    ));
    activities.push(ActivityData::with_pairs(
        Activity::Idle,
        get_idle_package(SPEED_MODIFIER),
    ));
    activities.push(ActivityData::with_pairs(
        Activity::Panic,
        get_panic_package(SPEED_MODIFIER),
    ));
    activities.push(ActivityData::with_pairs(
        Activity::PreRaid,
        get_pre_raid_package(SPEED_MODIFIER),
    ));
    activities.push(ActivityData::with_pairs(
        Activity::Raid,
        get_raid_package(SPEED_MODIFIER),
    ));
    activities.push(ActivityData::with_pairs(
        Activity::Hide,
        get_hide_package(SPEED_MODIFIER),
    ));
    activities
}

const MEMORY_TYPES: &[MemoryModuleId] = &[];

const SENSOR_TYPES: &[SensorType] = &[
    SensorType::NearestLivingEntities,
    SensorType::NearestPlayers,
    SensorType::NearestItems,
    SensorType::NearestBed,
    SensorType::HurtBy,
    SensorType::VillagerHostiles,
    SensorType::VillagerBabies,
    SensorType::SecondaryPois,
    SensorType::GolemDetected,
];

pub static VILLAGER_PROVIDER: BrainProvider = BrainProvider {
    memory_types: MEMORY_TYPES,
    sensor_types: SENSOR_TYPES,
    activities,
};
