use super::BrainTick;
use super::memory::MemoryModuleId;

pub mod acquire_poi;
pub mod animal_make_love;
pub mod animal_panic;
pub mod baby_follow_adult;
pub mod back_up_if_too_close;
pub mod become_passive_if_memory_present;
pub mod charge_attack;
pub mod copy_memory_with_expiry;
pub mod count_down_cooldown_ticks;
pub mod crossbow_attack;
pub mod dismount_or_skip_mounting;
pub mod do_nothing;
pub mod erase_memory_if;
pub mod follow_temptation;
pub mod gate;
pub mod go_and_give_items_to_target;
pub mod go_to_target_location;
pub mod go_to_wanted_item;
pub mod interact_with;
pub mod interact_with_door;
pub mod long_jump;
pub mod look_at_target_sink;
pub mod melee_attack;
pub mod mount;
pub mod move_to_target_sink;
pub mod one_shot;
pub mod ram;
pub mod random_look_around;
pub mod random_stroll;
pub mod set_entity_look_target;
pub mod set_entity_look_target_sometimes;
pub mod set_look_and_interact;
pub mod set_walk_target_away_from;
pub mod set_walk_target_from_attack_target;
pub mod set_walk_target_from_look_target;
pub mod shuffling_list;
pub mod start_attacking;
pub mod start_celebrating_if_target_dead;
pub mod stay_close_to_target;
pub mod stop_attacking_if_target_invalid;
pub mod stop_being_angry_if_target_dead;
pub mod stroll_to_poi;
pub mod swim;
pub mod timed;
pub mod transport_items_between_containers;
pub mod try_find_land;
pub mod try_lay_spawn_on_fluid_near_land;
pub mod utils;

pub use acquire_poi::acquire_poi;
pub use animal_make_love::AnimalMakeLove;
pub use animal_panic::AnimalPanic;
pub use baby_follow_adult::{baby_follow, baby_follow_adult};
pub use back_up_if_too_close::back_up_if_too_close;
pub use become_passive_if_memory_present::become_passive_if_memory_present;
pub use charge_attack::ChargeAttack;
pub use copy_memory_with_expiry::copy_memory_with_expiry;
pub use count_down_cooldown_ticks::CountDownCooldownTicks;
pub use crossbow_attack::CrossbowAttack;
pub use dismount_or_skip_mounting::dismount_or_skip_mounting;
pub use do_nothing::DoNothing;
pub use erase_memory_if::erase_memory_if;
pub use follow_temptation::FollowTemptation;
pub use gate::{GateBehavior, OrderPolicy, RunningPolicy, run_one, run_one_with_conditions};
pub use go_and_give_items_to_target::GoAndGiveItemsToTarget;
pub use go_to_target_location::go_to_target_location;
pub use go_to_wanted_item::go_to_wanted_item;
pub use interact_with::interact_with;
pub use interact_with_door::interact_with_door;
pub use long_jump::{LongJumpMidJump, LongJumpToRandomPos, default_acceptable_landing_spot};
pub use look_at_target_sink::LookAtTargetSink;
pub use melee_attack::melee_attack;
pub use mount::mount;
pub use move_to_target_sink::MoveToTargetSink;
pub use one_shot::{OneShot, Trigger};
pub use ram::{PrepareRamNearestTarget, RamTarget};
pub use random_look_around::RandomLookAround;
pub use random_stroll::{fly, stroll, stroll_with_range, swim};
pub use set_entity_look_target::{
    set_entity_look_target, set_entity_look_target_any, set_entity_look_target_of_type,
};
pub use set_entity_look_target_sometimes::{Ticker, set_entity_look_target_sometimes};
pub use set_look_and_interact::set_look_and_interact;
pub use set_walk_target_away_from::{
    entity as set_walk_target_away_from_entity, pos as set_walk_target_away_from_pos,
};
pub use set_walk_target_from_attack_target::{
    set_walk_target_from_attack_target_if_target_out_of_reach,
    set_walk_target_from_attack_target_if_target_out_of_reach_with_speed,
};
pub use set_walk_target_from_look_target::{
    set_walk_target_from_look_target, set_walk_target_from_look_target_with_speed,
};
pub use shuffling_list::ShufflingList;
pub use start_attacking::start_attacking;
pub use start_celebrating_if_target_dead::start_celebrating_if_target_dead;
pub use stay_close_to_target::stay_close_to_target;
pub use stop_attacking_if_target_invalid::stop_attacking_if_target_invalid;
pub use stop_being_angry_if_target_dead::stop_being_angry_if_target_dead;
pub use stroll_to_poi::{stroll_around_poi, stroll_to_poi};
pub use swim::Swim;
pub use timed::{Behavior, DEFAULT_DURATION, NO_TIMEOUT, Timed};
pub use try_find_land::{try_find_land, try_find_land_near_liquid, try_find_liquid};
pub use try_lay_spawn_on_fluid_near_land::try_lay_spawn_on_fluid_near_land;

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Status {
    Stopped,
    Running,
}

pub trait BehaviorControl: Send + Sync {
    fn status(&self) -> Status;
    fn required_memories(&self) -> &[MemoryModuleId];
    fn try_start(&mut self, tick: &mut BrainTick<'_>) -> bool;
    fn tick_or_stop(&mut self, tick: &mut BrainTick<'_>);
    fn do_stop(&mut self, tick: &mut BrainTick<'_>);
    fn debug_string(&self) -> String;
}

pub struct BehaviorEntry {
    pub priority: i32,
    pub activity: pumpkin_data::environment_attribute::Activity,
    pub behavior: Box<dyn BehaviorControl>,
}

#[must_use]
pub fn required_memories_of(
    conditions: &[(MemoryModuleId, super::memory::MemoryStatus)],
) -> Box<[MemoryModuleId]> {
    conditions.iter().map(|(id, _)| *id).collect()
}
