use pumpkin_data::entity::EntityType;
use pumpkin_data::environment_attribute::Activity;
use pumpkin_util::math::position::BlockPos;

use crate::entity::ageable::is_baby;
use crate::entity::ai::brain::activity::ActivityData;
use crate::entity::ai::brain::behavior::{self, Behavior, Timed};
use crate::entity::ai::brain::memory::walk_target::WalkTarget;
use crate::entity::ai::brain::memory::{MemoryModuleId, MemoryStatus, types};
use crate::entity::ai::brain::sensing::SensorType;
use crate::entity::ai::brain::{BrainProvider, BrainTick};
use crate::entity::passive::sniffer::{SnifferEntity, SnifferState};

const MAX_LOOK_DISTANCE: f32 = 6.0;
const SNIFFING_COOLDOWN_TICKS: i64 = 9600;
const SPEED_MULTIPLIER_WHEN_IDLING: f32 = 1.0;
const SPEED_MULTIPLIER_WHEN_PANICKING: f32 = 2.0;
const SPEED_MULTIPLIER_WHEN_SNIFFING: f32 = 1.25;
const SPEED_MULTIPLIER_WHEN_TEMPTED: f32 = 1.25;
const SEARCHING_TIMEOUT: i32 = 600;

fn as_sniffer<'a>(tick: &BrainTick<'a>) -> Option<&'a SnifferEntity> {
    tick.mob.cast_any().downcast_ref::<SnifferEntity>()
}

fn transition_to(tick: &BrainTick<'_>, state: SnifferState) {
    if let Some(sniffer) = as_sniffer(tick) {
        sniffer.transition_to(state);
    }
}

/// Vanilla `SnifferAi.resetSniffing`.
fn reset_sniffing(tick: &mut BrainTick<'_>) {
    tick.brain.erase(types::SNIFFER_DIGGING.id());
    tick.brain.erase(types::SNIFFER_SNIFFING_TARGET.id());
    transition_to(tick, SnifferState::Idling);
}

/// Vanilla `Sniffer.isTempted`.
#[must_use]
pub fn is_tempted(tick: &BrainTick<'_>) -> bool {
    tick.brain.has_memory_value(types::TEMPTING_PLAYER.id())
}

/// Vanilla `Mob.isPanicking` read from the brain being ticked.
fn is_panicking(tick: &BrainTick<'_>) -> bool {
    tick.brain.has_memory_value(types::IS_PANICKING.id())
}

/// Vanilla `Sniffer.canSniff`.
fn can_sniff(tick: &BrainTick<'_>) -> bool {
    let entity = tick.mob.get_entity();
    !is_tempted(tick)
        && !is_panicking(tick)
        && !entity.is_in_water()
        && !tick.mob.is_in_love()
        && entity.on_ground.load(std::sync::atomic::Ordering::Relaxed)
        && !entity.has_vehicle()
        && !entity.is_leashed()
}

/// Vanilla `Sniffer.canDig()`.
fn can_dig(tick: &BrainTick<'_>) -> bool {
    let entity = tick.mob.get_entity();
    !is_panicking(tick)
        && !is_tempted(tick)
        && !is_baby(tick.mob)
        && !entity.is_in_water()
        && entity.on_ground.load(std::sync::atomic::Ordering::Relaxed)
        && !entity.has_vehicle()
        && as_sniffer(tick)
            .is_some_and(|sniffer| sniffer.can_dig_at(tick, sniffer.get_head_block().down()))
}

/// Vanilla `Sniffer.calculateDigPosition`.
fn calculate_dig_position(tick: &BrainTick<'_>) -> Option<BlockPos> {
    let sniffer = as_sniffer(tick)?;
    (0..5)
        .filter_map(|idx| {
            crate::entity::ai::util::land_random_pos::get_pos(tick.mob, 10 + 2 * idx, 3)
        })
        .map(BlockPos::floored_v)
        .filter(|position| {
            tick.world
                .worldborder
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .contains_block(position.0.x, position.0.z)
        })
        .map(|position| position.down())
        .find(|position| sniffer.can_dig_at(tick, *position))
}

/// Runs vanilla `resetSniffing` before the wrapped behaviour starts, as the anonymous
/// subclasses in `SnifferAi` do.
struct ResetSniffingOnStart<B: Behavior>(B);

impl<B: Behavior> Behavior for ResetSniffingOnStart<B> {
    fn entry_conditions(&self) -> &[(MemoryModuleId, MemoryStatus)] {
        self.0.entry_conditions()
    }

    fn min_duration(&self) -> i32 {
        self.0.min_duration()
    }

    fn max_duration(&self) -> i32 {
        self.0.max_duration()
    }

    fn check_extra_start_conditions(&mut self, tick: &mut BrainTick<'_>) -> bool {
        self.0.check_extra_start_conditions(tick)
    }

    fn start(&mut self, tick: &mut BrainTick<'_>) {
        reset_sniffing(tick);
        self.0.start(tick);
    }

    fn tick(&mut self, tick: &mut BrainTick<'_>) {
        self.0.tick(tick);
    }

    fn stop_with_timeout(&mut self, tick: &mut BrainTick<'_>, timed_out: bool) {
        self.0.stop_with_timeout(tick, timed_out);
    }

    fn can_still_use(&mut self, tick: &BrainTick<'_>) -> bool {
        self.0.can_still_use(tick)
    }

    fn debug_name(&self) -> &'static str {
        self.0.debug_name()
    }
}

fn init_core_activity() -> ActivityData {
    ActivityData::with_priority_start(
        Activity::Core,
        0,
        vec![
            Box::new(Timed::new(behavior::Swim::new(0.8))),
            Box::new(Timed::new(ResetSniffingOnStart(
                behavior::AnimalPanic::new(SPEED_MULTIPLIER_WHEN_PANICKING),
            ))),
            Box::new(Timed::new(behavior::MoveToTargetSink::new(500, 700))),
            Box::new(Timed::new(behavior::CountDownCooldownTicks::new(
                types::TEMPTATION_COOLDOWN_TICKS,
            ))),
        ],
    )
}

fn init_sniffing_activity() -> ActivityData {
    ActivityData::with_pairs_and_conditions(
        Activity::Sniff,
        vec![(0, Box::new(Timed::new(Searching)))],
        vec![
            (types::IS_PANICKING.id(), MemoryStatus::ValueAbsent),
            (
                types::SNIFFER_SNIFFING_TARGET.id(),
                MemoryStatus::ValuePresent,
            ),
            (types::WALK_TARGET.id(), MemoryStatus::ValuePresent),
        ],
    )
}

fn init_dig_activity() -> ActivityData {
    ActivityData::with_pairs_and_conditions(
        Activity::Dig,
        vec![
            (0, Box::new(Timed::new(Digging))),
            (0, Box::new(Timed::new(FinishedDigging))),
        ],
        vec![
            (types::IS_PANICKING.id(), MemoryStatus::ValueAbsent),
            (types::WALK_TARGET.id(), MemoryStatus::ValueAbsent),
            (types::SNIFFER_DIGGING.id(), MemoryStatus::ValuePresent),
        ],
    )
}

fn init_idle_activity() -> ActivityData {
    ActivityData::with_pairs_and_conditions(
        Activity::Idle,
        vec![
            (
                0,
                Box::new(Timed::new(ResetSniffingOnStart(
                    behavior::AnimalMakeLove::new(&EntityType::SNIFFER, 1.0, 2),
                ))),
            ),
            (
                1,
                Box::new(Timed::new(ResetSniffingOnStart(
                    behavior::FollowTemptation::with_distance(
                        |_| SPEED_MULTIPLIER_WHEN_TEMPTED,
                        |mob| if is_baby(mob) { 2.5 } else { 3.5 },
                        false,
                    ),
                ))),
            ),
            (
                2,
                Box::new(Timed::new(behavior::LookAtTargetSink::new(45, 90))),
            ),
            (3, Box::new(Timed::new(FeelingHappy))),
            (
                4,
                Box::new(behavior::run_one(vec![
                    (
                        Box::new(behavior::set_walk_target_from_look_target(
                            |_| true,
                            SPEED_MULTIPLIER_WHEN_IDLING,
                            3,
                        )),
                        2,
                    ),
                    (Box::new(Timed::new(Scenting)), 1),
                    (Box::new(Timed::new(Sniffing)), 1),
                    (
                        Box::new(behavior::set_entity_look_target_of_type(
                            &EntityType::PLAYER,
                            MAX_LOOK_DISTANCE,
                        )),
                        1,
                    ),
                    (
                        Box::new(behavior::stroll(SPEED_MULTIPLIER_WHEN_IDLING, true)),
                        1,
                    ),
                    (Box::new(behavior::DoNothing::new(5, 20)), 2),
                ])),
            ),
        ],
        vec![(types::SNIFFER_DIGGING.id(), MemoryStatus::ValueAbsent)],
    )
}

/// Vanilla `SnifferAi.updateActivity`.
pub fn update_activity(tick: &mut BrainTick<'_>) {
    tick.brain.set_active_activity_to_first_valid(&[
        Activity::Dig,
        Activity::Sniff,
        Activity::Idle,
    ]);
}

const DIGGING_CONDITIONS: &[(MemoryModuleId, MemoryStatus)] = &[
    (types::IS_PANICKING.id(), MemoryStatus::ValueAbsent),
    (types::WALK_TARGET.id(), MemoryStatus::ValueAbsent),
    (types::SNIFFER_DIGGING.id(), MemoryStatus::ValuePresent),
    (types::SNIFF_COOLDOWN.id(), MemoryStatus::ValueAbsent),
];

/// Vanilla `SnifferAi.Digging`.
struct Digging;

impl Behavior for Digging {
    fn entry_conditions(&self) -> &[(MemoryModuleId, MemoryStatus)] {
        DIGGING_CONDITIONS
    }

    fn min_duration(&self) -> i32 {
        160
    }

    fn max_duration(&self) -> i32 {
        180
    }

    fn check_extra_start_conditions(&mut self, tick: &mut BrainTick<'_>) -> bool {
        can_sniff(tick)
    }

    fn can_still_use(&mut self, tick: &BrainTick<'_>) -> bool {
        tick.brain.has_memory_value(types::SNIFFER_DIGGING.id())
            && can_dig(tick)
            && !tick.mob.is_in_love()
    }

    fn start(&mut self, tick: &mut BrainTick<'_>) {
        transition_to(tick, SnifferState::Digging);
    }

    fn stop_with_timeout(&mut self, tick: &mut BrainTick<'_>, timed_out: bool) {
        if timed_out {
            tick.brain
                .set_with_expiry(types::SNIFF_COOLDOWN, (), SNIFFING_COOLDOWN_TICKS);
        } else {
            reset_sniffing(tick);
        }
    }

    fn debug_name(&self) -> &'static str {
        "Digging"
    }
}

const FEELING_HAPPY_CONDITIONS: &[(MemoryModuleId, MemoryStatus)] =
    &[(types::SNIFFER_HAPPY.id(), MemoryStatus::ValuePresent)];

/// Vanilla `SnifferAi.FeelingHappy`.
struct FeelingHappy;

impl Behavior for FeelingHappy {
    fn entry_conditions(&self) -> &[(MemoryModuleId, MemoryStatus)] {
        FEELING_HAPPY_CONDITIONS
    }

    fn min_duration(&self) -> i32 {
        40
    }

    fn max_duration(&self) -> i32 {
        100
    }

    fn can_still_use(&mut self, _tick: &BrainTick<'_>) -> bool {
        true
    }

    fn start(&mut self, tick: &mut BrainTick<'_>) {
        transition_to(tick, SnifferState::FeelingHappy);
    }

    fn stop(&mut self, tick: &mut BrainTick<'_>) {
        transition_to(tick, SnifferState::Idling);
        tick.brain.erase(types::SNIFFER_HAPPY.id());
    }

    fn debug_name(&self) -> &'static str {
        "FeelingHappy"
    }
}

const FINISHED_DIGGING_DURATION: i32 = 40;
const FINISHED_DIGGING_CONDITIONS: &[(MemoryModuleId, MemoryStatus)] = &[
    (types::IS_PANICKING.id(), MemoryStatus::ValueAbsent),
    (types::WALK_TARGET.id(), MemoryStatus::ValueAbsent),
    (types::SNIFFER_DIGGING.id(), MemoryStatus::ValuePresent),
    (types::SNIFF_COOLDOWN.id(), MemoryStatus::ValuePresent),
];

/// Vanilla `SnifferAi.FinishedDigging`.
struct FinishedDigging;

impl Behavior for FinishedDigging {
    fn entry_conditions(&self) -> &[(MemoryModuleId, MemoryStatus)] {
        FINISHED_DIGGING_CONDITIONS
    }

    fn min_duration(&self) -> i32 {
        FINISHED_DIGGING_DURATION
    }

    fn max_duration(&self) -> i32 {
        FINISHED_DIGGING_DURATION
    }

    fn can_still_use(&mut self, tick: &BrainTick<'_>) -> bool {
        tick.brain.has_memory_value(types::SNIFFER_DIGGING.id())
    }

    fn start(&mut self, tick: &mut BrainTick<'_>) {
        transition_to(tick, SnifferState::Rising);
    }

    fn stop_with_timeout(&mut self, tick: &mut BrainTick<'_>, timed_out: bool) {
        if let Some(sniffer) = as_sniffer(tick) {
            sniffer.transition_to(SnifferState::Idling);
            sniffer.on_digging_complete(tick, timed_out);
        }
        tick.brain.erase(types::SNIFFER_DIGGING.id());
        tick.brain.set(types::SNIFFER_HAPPY, true);
    }

    fn debug_name(&self) -> &'static str {
        "FinishedDigging"
    }
}

const SCENTING_CONDITIONS: &[(MemoryModuleId, MemoryStatus)] = &[
    (types::IS_PANICKING.id(), MemoryStatus::ValueAbsent),
    (types::SNIFFER_DIGGING.id(), MemoryStatus::ValueAbsent),
    (
        types::SNIFFER_SNIFFING_TARGET.id(),
        MemoryStatus::ValueAbsent,
    ),
    (types::SNIFFER_HAPPY.id(), MemoryStatus::ValueAbsent),
    (types::BREED_TARGET.id(), MemoryStatus::ValueAbsent),
];

/// Vanilla `SnifferAi.Scenting`.
struct Scenting;

impl Behavior for Scenting {
    fn entry_conditions(&self) -> &[(MemoryModuleId, MemoryStatus)] {
        SCENTING_CONDITIONS
    }

    fn min_duration(&self) -> i32 {
        40
    }

    fn max_duration(&self) -> i32 {
        80
    }

    fn check_extra_start_conditions(&mut self, tick: &mut BrainTick<'_>) -> bool {
        !is_tempted(tick)
    }

    fn can_still_use(&mut self, _tick: &BrainTick<'_>) -> bool {
        true
    }

    fn start(&mut self, tick: &mut BrainTick<'_>) {
        transition_to(tick, SnifferState::Scenting);
    }

    fn stop(&mut self, tick: &mut BrainTick<'_>) {
        transition_to(tick, SnifferState::Idling);
    }

    fn debug_name(&self) -> &'static str {
        "Scenting"
    }
}

const SEARCHING_CONDITIONS: &[(MemoryModuleId, MemoryStatus)] = &[
    (types::WALK_TARGET.id(), MemoryStatus::ValuePresent),
    (types::IS_PANICKING.id(), MemoryStatus::ValueAbsent),
    (
        types::SNIFFER_SNIFFING_TARGET.id(),
        MemoryStatus::ValuePresent,
    ),
];

/// Vanilla `SnifferAi.Searching`: walks to the spot it sniffed out.
struct Searching;

impl Behavior for Searching {
    fn entry_conditions(&self) -> &[(MemoryModuleId, MemoryStatus)] {
        SEARCHING_CONDITIONS
    }

    fn min_duration(&self) -> i32 {
        SEARCHING_TIMEOUT
    }

    fn max_duration(&self) -> i32 {
        SEARCHING_TIMEOUT
    }

    fn check_extra_start_conditions(&mut self, tick: &mut BrainTick<'_>) -> bool {
        can_sniff(tick)
    }

    fn can_still_use(&mut self, tick: &BrainTick<'_>) -> bool {
        if !can_sniff(tick) {
            transition_to(tick, SnifferState::Idling);
            return false;
        }
        let walk_target = tick
            .brain
            .get(types::WALK_TARGET)
            .map(|walk_target| walk_target.target().current_block_position());
        let sniffing_target = tick.brain.get(types::SNIFFER_SNIFFING_TARGET).copied();
        walk_target.is_some() && walk_target == sniffing_target
    }

    fn start(&mut self, tick: &mut BrainTick<'_>) {
        transition_to(tick, SnifferState::Searching);
    }

    fn stop(&mut self, tick: &mut BrainTick<'_>) {
        if can_dig(tick) && can_sniff(tick) {
            tick.brain.set(types::SNIFFER_DIGGING, true);
        }
        tick.brain.erase(types::WALK_TARGET.id());
        tick.brain.erase(types::SNIFFER_SNIFFING_TARGET.id());
    }

    fn debug_name(&self) -> &'static str {
        "Searching"
    }
}

const SNIFFING_CONDITIONS: &[(MemoryModuleId, MemoryStatus)] = &[
    (types::WALK_TARGET.id(), MemoryStatus::ValueAbsent),
    (
        types::SNIFFER_SNIFFING_TARGET.id(),
        MemoryStatus::ValueAbsent,
    ),
    (types::SNIFF_COOLDOWN.id(), MemoryStatus::ValueAbsent),
];

/// Vanilla `SnifferAi.Sniffing`: sniffs the air, then picks a spot to dig.
struct Sniffing;

impl Behavior for Sniffing {
    fn entry_conditions(&self) -> &[(MemoryModuleId, MemoryStatus)] {
        SNIFFING_CONDITIONS
    }

    fn min_duration(&self) -> i32 {
        40
    }

    fn max_duration(&self) -> i32 {
        80
    }

    fn check_extra_start_conditions(&mut self, tick: &mut BrainTick<'_>) -> bool {
        !is_baby(tick.mob) && can_sniff(tick)
    }

    fn can_still_use(&mut self, tick: &BrainTick<'_>) -> bool {
        can_sniff(tick)
    }

    fn start(&mut self, tick: &mut BrainTick<'_>) {
        transition_to(tick, SnifferState::Sniffing);
    }

    fn stop_with_timeout(&mut self, tick: &mut BrainTick<'_>, timed_out: bool) {
        transition_to(tick, SnifferState::Idling);
        if timed_out && let Some(position) = calculate_dig_position(tick) {
            tick.brain.set(types::SNIFFER_SNIFFING_TARGET, position);
            tick.brain.set(
                types::WALK_TARGET,
                WalkTarget::from_block_pos(position, SPEED_MULTIPLIER_WHEN_SNIFFING, 0),
            );
        }
    }

    fn debug_name(&self) -> &'static str {
        "Sniffing"
    }
}

const MEMORY_TYPES: &[MemoryModuleId] = &[types::SNIFFER_EXPLORED_POSITIONS.id()];

const SENSOR_TYPES: &[SensorType] = &[
    SensorType::NearestLivingEntities,
    SensorType::HurtBy,
    SensorType::NearestPlayers,
    SensorType::FoodTemptations,
];

pub static SNIFFER_PROVIDER: BrainProvider = BrainProvider {
    memory_types: MEMORY_TYPES,
    sensor_types: SENSOR_TYPES,
    activities: |_| {
        vec![
            init_core_activity(),
            init_idle_activity(),
            init_sniffing_activity(),
            init_dig_activity(),
        ]
    },
};
