use std::sync::Arc;

use pumpkin_data::data_component_impl::EquipmentSlot;
use pumpkin_data::item_stack::ItemStack;
use pumpkin_data::tag::Taggable;
use pumpkin_data::{Block, BlockDirection, BlockState};
use pumpkin_inventory::Inventory;
use pumpkin_util::math::boundingbox::BoundingBox;
use pumpkin_util::math::position::BlockPos;
use pumpkin_util::math::vector2::Vector2;
use pumpkin_util::math::vector3::Vector3;
use rustc_hash::FxHashSet;

use crate::block::blocks::chests;
use crate::block::entities::BlockEntity;
use crate::block::entities::chest::ChestBlockEntity;
use crate::block::entities::trapped_chest::TrappedChestBlockEntity;
use crate::world::World;

use super::super::BrainTick;
use super::super::memory::position_tracker::{BlockPosTracker, PositionTracker};
use super::super::memory::{GlobalPos, MemoryModuleId, MemoryStatus, types};
use super::timed::{Behavior, NO_TIMEOUT};
use super::utils::{global_pos_in, set_walk_and_look_target_memories};

pub const TARGET_INTERACTION_TIME: i32 = 60;
const VISITED_POSITIONS_MEMORY_TIME: i64 = 6000;
const TRANSPORTED_ITEM_MAX_STACK_SIZE: u8 = 16;
const MAX_VISITED_POSITIONS: usize = 10;
const MAX_UNREACHABLE_POSITIONS: usize = 50;
const PASSENGER_MOB_TARGET_SEARCH_DISTANCE: i32 = 1;
const IDLE_COOLDOWN: i32 = 140;
const CLOSE_ENOUGH_TO_START_QUEUING_DISTANCE: f64 = 3.0;
const CLOSE_ENOUGH_TO_START_INTERACTING_WITH_TARGET_DISTANCE: f64 = 0.5;
const CLOSE_ENOUGH_TO_START_INTERACTING_WITH_TARGET_PATH_END_DISTANCE: f64 = 1.0;
const CLOSE_ENOUGH_TO_CONTINUE_INTERACTING_WITH_TARGET: f64 = 2.0;

const CONDITIONS: &[(MemoryModuleId, MemoryStatus)] = &[
    (
        types::VISITED_BLOCK_POSITIONS.id(),
        MemoryStatus::Registered,
    ),
    (
        types::UNREACHABLE_TRANSPORT_BLOCK_POSITIONS.id(),
        MemoryStatus::Registered,
    ),
    (
        types::TRANSPORT_ITEMS_COOLDOWN_TICKS.id(),
        MemoryStatus::ValueAbsent,
    ),
    (types::IS_PANICKING.id(), MemoryStatus::ValueAbsent),
];

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum ContainerInteractionState {
    PickupItem,
    PickupNoItem,
    PlaceItem,
    PlaceNoItem,
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum TransportItemState {
    Travelling,
    Queuing,
    Interacting,
}

/// Vanilla `TransportItemsBetweenContainers.TransportItemTarget`.
pub struct TransportItemTarget {
    pub pos: BlockPos,
    pub container: Arc<dyn Inventory>,
    pub block_entity: Arc<dyn BlockEntity>,
    pub state: &'static BlockState,
}

impl TransportItemTarget {
    fn try_create(block_entity: Arc<dyn BlockEntity>, world: &World) -> Option<Self> {
        let pos = block_entity.get_position();
        let (block, state) = world.get_block_and_state(&pos);
        let container = if is_chest_block(block) {
            chests::get_container(world, &pos)?
        } else {
            Arc::clone(&block_entity).get_inventory()?
        };
        Some(Self {
            pos,
            container,
            block_entity,
            state,
        })
    }

    fn try_create_at(pos: &BlockPos, world: &World) -> Option<Self> {
        Self::try_create(world.get_block_entity(pos)?, world)
    }
}

fn is_chest_block(block: &Block) -> bool {
    block == &Block::CHEST
        || block == &Block::TRAPPED_CHEST
        || block.has_tag(&pumpkin_data::tag::Block::MINECRAFT_COPPER_CHESTS)
}

/// Vanilla `instanceof ChestBlockEntity`; trapped chests extend it there.
fn chest_viewer_count(block_entity: &dyn BlockEntity) -> Option<u16> {
    let any = block_entity.as_any();
    any.downcast_ref::<ChestBlockEntity>()
        .map(ChestBlockEntity::get_viewer_count)
        .or_else(|| {
            any.downcast_ref::<TrappedChestBlockEntity>()
                .map(TrappedChestBlockEntity::get_viewer_count)
        })
}

/// Whether a player or mob has the target chest open, which copper golems queue behind.
#[must_use]
pub fn is_chest_open(target: &TransportItemTarget) -> bool {
    chest_viewer_count(target.block_entity.as_ref()).is_some_and(|count| count > 0)
}

pub type OnTargetReached =
    fn(&mut BrainTick<'_>, ContainerInteractionState, &TransportItemTarget, i32);

/// Vanilla `TransportItemsBetweenContainers`: carries items from source containers to
/// destination containers, one stack at a time.
pub struct TransportItemsBetweenContainers {
    speed_modifier: f32,
    is_source_block: fn(&Block) -> bool,
    is_destination_block: fn(&Block) -> bool,
    horizontal_search_distance: i32,
    vertical_search_distance: i32,
    on_target_reached: OnTargetReached,
    on_start_travelling: fn(&BrainTick<'_>),
    should_queue_for_target: fn(&TransportItemTarget) -> bool,
    target: Option<TransportItemTarget>,
    state: TransportItemState,
    interaction_state: Option<ContainerInteractionState>,
    ticks_since_reaching_target: i32,
}

impl TransportItemsBetweenContainers {
    #[must_use]
    #[expect(clippy::too_many_arguments, reason = "mirrors the vanilla constructor")]
    pub const fn new(
        speed_modifier: f32,
        is_source_block: fn(&Block) -> bool,
        is_destination_block: fn(&Block) -> bool,
        horizontal_search_distance: i32,
        vertical_search_distance: i32,
        on_target_reached: OnTargetReached,
        on_start_travelling: fn(&BrainTick<'_>),
        should_queue_for_target: fn(&TransportItemTarget) -> bool,
    ) -> Self {
        Self {
            speed_modifier,
            is_source_block,
            is_destination_block,
            horizontal_search_distance,
            vertical_search_distance,
            on_target_reached,
            on_start_travelling,
            should_queue_for_target,
            target: None,
            state: TransportItemState::Travelling,
            interaction_state: None,
            ticks_since_reaching_target: 0,
        }
    }

    fn set_can_path_below_surface(tick: &BrainTick<'_>, can_path: bool) {
        tick.mob
            .get_mob_entity()
            .navigator
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .set_can_path_to_targets_below_surface(can_path);
    }

    fn update_invalid_target(&mut self, tick: &mut BrainTick<'_>) -> bool {
        if self.has_valid_target(tick) {
            return false;
        }
        self.stop_targeting_current_target(tick);
        match self.get_transport_target(tick) {
            Some(target) => {
                let pos = target.pos;
                self.target = Some(target);
                self.on_start_travelling(tick);
                self.set_visited_block_pos(tick, pos);
            }
            None => self.enter_cooldown_after_no_matching_target_found(tick),
        }
        true
    }

    fn on_queuing_for_target(&mut self, tick: &mut BrainTick<'_>) {
        let Some(target) = &self.target else {
            return;
        };
        if !self.is_another_mob_interacting_with_target(target, tick.world) {
            self.resume_travelling(tick);
        }
    }

    fn on_travel_to_target(&mut self, tick: &mut BrainTick<'_>) {
        let Some(target) = &self.target else {
            return;
        };
        let center = center_pos(tick);
        if is_within_target_distance(CLOSE_ENOUGH_TO_START_QUEUING_DISTANCE, target, tick, center)
            && self.is_another_mob_interacting_with_target(target, tick.world)
        {
            self.start_queuing(tick);
        } else if is_within_target_distance(interaction_range(tick), target, tick, center) {
            self.start_on_reached_target_interaction(tick);
        } else {
            self.walk_towards_target(tick);
        }
    }

    fn on_reached_target(&mut self, tick: &mut BrainTick<'_>) {
        let Some(target) = self.target.take() else {
            return;
        };
        if !is_within_target_distance(
            CLOSE_ENOUGH_TO_CONTINUE_INTERACTING_WITH_TARGET,
            &target,
            tick,
            center_pos(tick),
        ) {
            self.target = Some(target);
            self.on_start_travelling(tick);
            return;
        }
        self.ticks_since_reaching_target += 1;
        self.on_target_interaction(tick, &target);
        let container = Arc::clone(&target.container);
        self.target = Some(target);
        if self.ticks_since_reaching_target < TARGET_INTERACTION_TIME {
            return;
        }
        match reached_target_interaction(tick, container.as_ref()) {
            ContainerInteractionState::PickupItem => self.pick_up_items(tick, container.as_ref()),
            ContainerInteractionState::PlaceItem => self.put_down_item(tick, container.as_ref()),
            ContainerInteractionState::PickupNoItem | ContainerInteractionState::PlaceNoItem => {
                self.stop_targeting_current_target(tick);
            }
        }
        self.on_start_travelling(tick);
    }

    fn start_queuing(&mut self, tick: &BrainTick<'_>) {
        tick.mob.get_mob_entity().stop_in_place();
        self.state = TransportItemState::Queuing;
    }

    fn resume_travelling(&mut self, tick: &mut BrainTick<'_>) {
        self.state = TransportItemState::Travelling;
        self.walk_towards_target(tick);
    }

    fn walk_towards_target(&self, tick: &mut BrainTick<'_>) {
        if let Some(target) = &self.target {
            set_walk_and_look_target_memories(
                tick.brain,
                Arc::new(BlockPosTracker::new(target.pos)),
                self.speed_modifier,
                0,
            );
        }
    }

    fn start_on_reached_target_interaction(&mut self, tick: &BrainTick<'_>) {
        if let Some(target) = &self.target {
            self.interaction_state =
                Some(reached_target_interaction(tick, target.container.as_ref()));
        }
        self.state = TransportItemState::Interacting;
    }

    fn on_start_travelling(&mut self, tick: &BrainTick<'_>) {
        (self.on_start_travelling)(tick);
        self.state = TransportItemState::Travelling;
        self.interaction_state = None;
        self.ticks_since_reaching_target = 0;
    }

    fn on_target_interaction(&self, tick: &mut BrainTick<'_>, target: &TransportItemTarget) {
        tick.brain.set(
            types::LOOK_TARGET,
            Arc::new(BlockPosTracker::new(target.pos)) as Arc<dyn PositionTracker>,
        );
        tick.mob.get_mob_entity().stop_in_place();
        if let Some(state) = self.interaction_state {
            (self.on_target_reached)(tick, state, target, self.ticks_since_reaching_target);
        }
    }

    /// Vanilla `getTransportTarget`: the nearest valid chest in loaded chunks around the mob.
    fn get_transport_target(&self, tick: &BrainTick<'_>) -> Option<TransportItemTarget> {
        let search_area = self.target_search_area(tick);
        let visited = tick
            .brain
            .get(types::VISITED_BLOCK_POSITIONS)
            .cloned()
            .unwrap_or_default();
        let unreachable = tick
            .brain
            .get(types::UNREACHABLE_TRANSPORT_BLOCK_POSITIONS)
            .cloned()
            .unwrap_or_default();
        let body_pos = tick.mob.get_entity().pos.load();
        let center_chunk = tick.mob.get_entity().block_pos.load().chunk_position();
        let chunk_radius = self.horizontal_search_distance(tick).div_euclid(16) + 1;

        let mut target = None;
        let mut closest_distance = f64::from(f32::MAX);
        for chunk_z in -chunk_radius..=chunk_radius {
            for chunk_x in -chunk_radius..=chunk_radius {
                let chunk = Vector2::new(center_chunk.x + chunk_x, center_chunk.y + chunk_z);
                // Collected first so the checks below never run under the map guard.
                let chests: Vec<Arc<dyn BlockEntity>> = tick
                    .world
                    .block_entities
                    .get(&chunk)
                    .map(|entities| {
                        entities
                            .values()
                            .filter(|entity| chest_viewer_count(entity.as_ref()).is_some())
                            .cloned()
                            .collect()
                    })
                    .unwrap_or_default();
                for chest in chests {
                    let distance = chest
                        .get_position()
                        .to_centered_f64()
                        .squared_distance_to_vec(&body_pos);
                    if distance < closest_distance
                        && let Some(valid) = self.is_target_valid_to_pick(
                            tick,
                            chest,
                            &visited,
                            &unreachable,
                            &search_area,
                        )
                    {
                        target = Some(valid);
                        closest_distance = distance;
                    }
                }
            }
        }
        target
    }

    fn is_target_valid_to_pick(
        &self,
        tick: &BrainTick<'_>,
        block_entity: Arc<dyn BlockEntity>,
        visited: &FxHashSet<GlobalPos>,
        unreachable: &FxHashSet<GlobalPos>,
        search_area: &BoundingBox,
    ) -> Option<TransportItemTarget> {
        let pos = block_entity.get_position();
        let (x, y, z) = (f64::from(pos.0.x), f64::from(pos.0.y), f64::from(pos.0.z));
        let within_search_area = x >= search_area.min.x
            && x < search_area.max.x
            && y >= search_area.min.y
            && y < search_area.max.y
            && z >= search_area.min.z
            && z < search_area.max.z;
        if !within_search_area {
            return None;
        }
        let target = TransportItemTarget::try_create(block_entity, tick.world)?;
        // Vanilla also skips containers locked with a key item; Pumpkin chests have no lock.
        (self.is_wanted_block(tick, target.state)
            && !is_position_already_visited(visited, unreachable, &target, tick.world))
        .then_some(target)
    }

    fn has_valid_target(&mut self, tick: &mut BrainTick<'_>) -> bool {
        let Some(target) = &self.target else {
            return false;
        };
        let target_is_valid_type =
            self.is_wanted_block(tick, target.state) && target_has_not_changed(tick.world, target);
        if !target_is_valid_type || chests::is_chest_blocked(tick.world, &target.pos) {
            return false;
        }
        if self.state != TransportItemState::Travelling || has_valid_travelling_path(tick, target) {
            return true;
        }
        let pos = target.pos;
        self.mark_visited_block_pos_as_unreachable(tick, pos);
        false
    }

    fn target_search_area(&self, tick: &BrainTick<'_>) -> BoundingBox {
        let horizontal = f64::from(self.horizontal_search_distance(tick));
        let vertical = f64::from(self.vertical_search_distance(tick));
        let pos = tick.mob.get_entity().block_pos.load();
        let min = Vector3::new(f64::from(pos.0.x), f64::from(pos.0.y), f64::from(pos.0.z));
        BoundingBox {
            min,
            max: min + Vector3::new(1.0, 1.0, 1.0),
        }
        .expand(horizontal, vertical, horizontal)
    }

    fn horizontal_search_distance(&self, tick: &BrainTick<'_>) -> i32 {
        if tick.mob.get_entity().has_vehicle() {
            PASSENGER_MOB_TARGET_SEARCH_DISTANCE
        } else {
            self.horizontal_search_distance
        }
    }

    fn vertical_search_distance(&self, tick: &BrainTick<'_>) -> i32 {
        if tick.mob.get_entity().has_vehicle() {
            PASSENGER_MOB_TARGET_SEARCH_DISTANCE
        } else {
            self.vertical_search_distance
        }
    }

    fn is_wanted_block(&self, tick: &BrainTick<'_>, state: &BlockState) -> bool {
        let block = Block::from_state_id(state.id);
        if is_picking_up_items(tick) {
            (self.is_source_block)(block)
        } else {
            (self.is_destination_block)(block)
        }
    }

    fn is_another_mob_interacting_with_target(
        &self,
        target: &TransportItemTarget,
        world: &World,
    ) -> bool {
        (self.should_queue_for_target)(target)
            || connected_target(target, world)
                .is_some_and(|other| (self.should_queue_for_target)(&other))
    }

    fn set_visited_block_pos(&mut self, tick: &mut BrainTick<'_>, pos: BlockPos) {
        let mut visited = tick
            .brain
            .get(types::VISITED_BLOCK_POSITIONS)
            .cloned()
            .unwrap_or_default();
        if let Some(global) = global_pos_in(tick.world, pos) {
            visited.insert(global);
        }
        if visited.len() > MAX_VISITED_POSITIONS {
            self.enter_cooldown_after_no_matching_target_found(tick);
        } else {
            tick.brain.set_with_expiry(
                types::VISITED_BLOCK_POSITIONS,
                visited,
                VISITED_POSITIONS_MEMORY_TIME,
            );
        }
    }

    fn mark_visited_block_pos_as_unreachable(&mut self, tick: &mut BrainTick<'_>, pos: BlockPos) {
        let Some(global) = global_pos_in(tick.world, pos) else {
            return;
        };
        let mut visited = tick
            .brain
            .get(types::VISITED_BLOCK_POSITIONS)
            .cloned()
            .unwrap_or_default();
        visited.remove(&global);
        let mut unreachable = tick
            .brain
            .get(types::UNREACHABLE_TRANSPORT_BLOCK_POSITIONS)
            .cloned()
            .unwrap_or_default();
        unreachable.insert(global);
        if unreachable.len() > MAX_UNREACHABLE_POSITIONS {
            self.enter_cooldown_after_no_matching_target_found(tick);
        } else {
            tick.brain.set_with_expiry(
                types::VISITED_BLOCK_POSITIONS,
                visited,
                VISITED_POSITIONS_MEMORY_TIME,
            );
            tick.brain.set_with_expiry(
                types::UNREACHABLE_TRANSPORT_BLOCK_POSITIONS,
                unreachable,
                VISITED_POSITIONS_MEMORY_TIME,
            );
        }
    }

    fn pick_up_items(&mut self, tick: &mut BrainTick<'_>, container: &dyn Inventory) {
        tick.mob
            .get_mob_entity()
            .set_item_slot_and_drop_when_killed(
                &EquipmentSlot::MAIN_HAND,
                pickup_item_from_container(container),
            );
        container.mark_dirty();
        self.clear_memories_after_matching_target_found(tick);
    }

    fn put_down_item(&mut self, tick: &mut BrainTick<'_>, container: &dyn Inventory) {
        let left_over = add_items_to_container(main_hand_item(tick), container);
        container.mark_dirty();
        let emptied = left_over.is_empty();
        tick.mob
            .get_mob_entity()
            .set_item_slot(&EquipmentSlot::MAIN_HAND, left_over);
        if emptied {
            self.clear_memories_after_matching_target_found(tick);
        } else {
            self.stop_targeting_current_target(tick);
        }
    }

    fn stop_targeting_current_target(&mut self, tick: &mut BrainTick<'_>) {
        self.ticks_since_reaching_target = 0;
        self.target = None;
        tick.mob
            .get_mob_entity()
            .navigator
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .stop();
        tick.brain.erase(types::WALK_TARGET.id());
    }

    fn clear_memories_after_matching_target_found(&mut self, tick: &mut BrainTick<'_>) {
        self.stop_targeting_current_target(tick);
        tick.brain.erase(types::VISITED_BLOCK_POSITIONS.id());
        tick.brain
            .erase(types::UNREACHABLE_TRANSPORT_BLOCK_POSITIONS.id());
    }

    fn enter_cooldown_after_no_matching_target_found(&mut self, tick: &mut BrainTick<'_>) {
        self.stop_targeting_current_target(tick);
        tick.brain
            .set(types::TRANSPORT_ITEMS_COOLDOWN_TICKS, IDLE_COOLDOWN);
        tick.brain.erase(types::VISITED_BLOCK_POSITIONS.id());
        tick.brain
            .erase(types::UNREACHABLE_TRANSPORT_BLOCK_POSITIONS.id());
    }
}

fn main_hand_item(tick: &BrainTick<'_>) -> ItemStack {
    tick.mob
        .get_mob_entity()
        .living_entity
        .entity_equipment
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .get(&EquipmentSlot::MAIN_HAND)
}

fn is_picking_up_items(tick: &BrainTick<'_>) -> bool {
    main_hand_item(tick).is_empty()
}

/// Vanilla `doReachedTargetInteraction`, reduced to which of its four branches applies.
fn reached_target_interaction(
    tick: &BrainTick<'_>,
    container: &dyn Inventory,
) -> ContainerInteractionState {
    if is_picking_up_items(tick) {
        if container.is_empty() {
            ContainerInteractionState::PickupNoItem
        } else {
            ContainerInteractionState::PickupItem
        }
    } else if container.is_empty() || has_item_matching_hand_item(tick, container) {
        ContainerInteractionState::PlaceItem
    } else {
        ContainerInteractionState::PlaceNoItem
    }
}

fn has_item_matching_hand_item(tick: &BrainTick<'_>, container: &dyn Inventory) -> bool {
    let hand = main_hand_item(tick);
    (0..container.size()).any(|slot| container.get_stack(slot).item.id == hand.item.id)
}

fn pickup_item_from_container(container: &dyn Inventory) -> ItemStack {
    (0..container.size())
        .find(|&slot| !container.get_stack(slot).is_empty())
        .map_or_else(
            || ItemStack::EMPTY.clone(),
            |slot| {
                let count = container
                    .get_stack(slot)
                    .item_count
                    .min(TRANSPORTED_ITEM_MAX_STACK_SIZE);
                container.remove_stack_specific(slot, count)
            },
        )
}

fn add_items_to_container(mut stack: ItemStack, container: &dyn Inventory) -> ItemStack {
    for slot in 0..container.size() {
        let mut container_stack = container.get_stack(slot);
        if container_stack.is_empty() {
            container.set_stack(slot, stack);
            return ItemStack::EMPTY.clone();
        }
        if container_stack.are_items_and_components_equal(&stack)
            && container_stack.item_count < container_stack.get_max_stack_size()
        {
            let can_be_added = container_stack.get_max_stack_size() - container_stack.item_count;
            container_stack.item_count += can_be_added.min(stack.item_count);
            // Vanilla takes the free space off the hand, not the amount it moved.
            stack.item_count = stack.item_count.saturating_sub(can_be_added);
            container.set_stack(slot, container_stack);
            if stack.is_empty() {
                return ItemStack::EMPTY.clone();
            }
        }
    }
    stack
}

fn connected_target(target: &TransportItemTarget, world: &World) -> Option<TransportItemTarget> {
    let connected = chests::get_connected_block_pos(&target.pos, target.state.id)?;
    TransportItemTarget::try_create_at(&connected, world)
}

fn is_position_already_visited(
    visited: &FxHashSet<GlobalPos>,
    unreachable: &FxHashSet<GlobalPos>,
    target: &TransportItemTarget,
    world: &World,
) -> bool {
    std::iter::once(target.pos)
        .chain(connected_target(target, world).map(|other| other.pos))
        .filter_map(|pos| global_pos_in(world, pos))
        .any(|pos| visited.contains(&pos) || unreachable.contains(&pos))
}

fn target_has_not_changed(world: &World, target: &TransportItemTarget) -> bool {
    world
        .get_block_entity(&target.pos)
        .is_some_and(|current| Arc::ptr_eq(&current, &target.block_entity))
}

fn has_finished_path(tick: &BrainTick<'_>) -> bool {
    tick.mob
        .get_mob_entity()
        .navigator
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .get_path()
        .is_some_and(crate::entity::ai::pathfinder::path::Path::is_done)
}

fn interaction_range(tick: &BrainTick<'_>) -> f64 {
    if has_finished_path(tick) {
        CLOSE_ENOUGH_TO_START_INTERACTING_WITH_TARGET_PATH_END_DISTANCE
    } else {
        CLOSE_ENOUGH_TO_START_INTERACTING_WITH_TARGET_DISTANCE
    }
}

fn middle_y_position(tick: &BrainTick<'_>, pos: Vector3<f64>) -> Vector3<f64> {
    let bounding_box = tick.mob.get_entity().bounding_box.load();
    pos + Vector3::new(0.0, (bounding_box.max.y - bounding_box.min.y) / 2.0, 0.0)
}

fn center_pos(tick: &BrainTick<'_>) -> Vector3<f64> {
    middle_y_position(tick, tick.mob.get_entity().pos.load())
}

/// Vanilla `hasValidTravellingPath`.
fn has_valid_travelling_path(tick: &BrainTick<'_>, target: &TransportItemTarget) -> bool {
    // Outer `None`: no path at all. Inner `None`: a path without an end node.
    let end_node = {
        let mob_entity = tick.mob.get_mob_entity();
        let mut navigator = mob_entity
            .navigator
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let current = navigator
            .get_path()
            .map(|path| path.get_end_node().map(|node| node.pos));
        current.or_else(|| {
            navigator
                .create_path(&mob_entity.living_entity, target.pos.to_centered_f64(), 0)
                .map(|path| path.get_end_node().map(|node| node.pos))
        })
    };
    let has_path = end_node.is_some();
    let bottom_center = end_node.flatten().map_or_else(
        || tick.mob.get_entity().pos.load(),
        |pos| {
            Vector3::new(
                f64::from(pos.0.x) + 0.5,
                f64::from(pos.0.y),
                f64::from(pos.0.z) + 0.5,
            )
        },
    );
    let reach_from = middle_y_position(tick, bottom_center);
    let can_reach_target =
        is_within_target_distance(interaction_range(tick), target, tick, reach_from);
    let has_not_yet_created_path_to_target = !has_path && !can_reach_target;
    has_not_yet_created_path_to_target
        || (can_reach_target && can_see_any_target_side(target, tick.world, reach_from))
}

/// Vanilla `isWithinTargetDistance`: whether the mob's box, moved to `from_pos`, touches the
/// target's collision bounds grown by `distance`.
fn is_within_target_distance(
    distance: f64,
    target: &TransportItemTarget,
    tick: &BrainTick<'_>,
    from_pos: Vector3<f64>,
) -> bool {
    let body_box = tick.mob.get_entity().bounding_box.load();
    let half = Vector3::new(
        (body_box.max.x - body_box.min.x) / 2.0,
        (body_box.max.y - body_box.min.y) / 2.0,
        (body_box.max.z - body_box.min.z) / 2.0,
    );
    let moved_box = BoundingBox {
        min: from_pos - half,
        max: from_pos + half,
    };
    let Some(bounds) = target
        .state
        .get_block_collision_shapes()
        .reduce(|a, b| BoundingBox {
            min: Vector3::new(
                a.min.x.min(b.min.x),
                a.min.y.min(b.min.y),
                a.min.z.min(b.min.z),
            ),
            max: Vector3::new(
                a.max.x.max(b.max.x),
                a.max.y.max(b.max.y),
                a.max.z.max(b.max.z),
            ),
        })
    else {
        return false;
    };
    let offset = Vector3::new(
        f64::from(target.pos.0.x),
        f64::from(target.pos.0.y),
        f64::from(target.pos.0.z),
    );
    bounds
        .expand(distance, 0.5, distance)
        .shift(offset)
        .intersects(&moved_box)
}

/// Vanilla `canSeeAnyTargetSide`: a clear line to the middle of any face of the target block.
fn can_see_any_target_side(
    target: &TransportItemTarget,
    world: &Arc<World>,
    eye: Vector3<f64>,
) -> bool {
    let center = target.pos.to_centered_f64();
    BlockDirection::all().iter().any(|direction| {
        let step = direction.to_offset();
        let face = center
            + Vector3::new(
                0.5 * f64::from(step.x),
                0.5 * f64::from(step.y),
                0.5 * f64::from(step.z),
            );
        world
            .raycast(eye, face, |pos, world| {
                world
                    .get_block_state(pos)
                    .get_block_collision_shapes()
                    .next()
                    .is_some()
            })
            .is_some_and(|(hit, _)| hit == target.pos)
    })
}

impl Behavior for TransportItemsBetweenContainers {
    fn entry_conditions(&self) -> &[(MemoryModuleId, MemoryStatus)] {
        CONDITIONS
    }

    // Vanilla overrides `timedOut` to never time out.
    fn min_duration(&self) -> i32 {
        NO_TIMEOUT
    }

    fn max_duration(&self) -> i32 {
        NO_TIMEOUT
    }

    fn check_extra_start_conditions(&mut self, tick: &mut BrainTick<'_>) -> bool {
        !tick.mob.get_entity().is_leashed()
    }

    fn can_still_use(&mut self, tick: &BrainTick<'_>) -> bool {
        !tick
            .brain
            .has_memory_value(types::TRANSPORT_ITEMS_COOLDOWN_TICKS.id())
            && !tick.brain.has_memory_value(types::IS_PANICKING.id())
            && !tick.mob.get_entity().is_leashed()
    }

    fn start(&mut self, tick: &mut BrainTick<'_>) {
        Self::set_can_path_below_surface(tick, true);
    }

    fn tick(&mut self, tick: &mut BrainTick<'_>) {
        let updated_invalid_target = self.update_invalid_target(tick);
        if self.target.is_none() {
            // Vanilla calls its own `stop` here while the behaviour keeps running.
            self.stop(tick);
            return;
        }
        if updated_invalid_target {
            return;
        }
        if self.state == TransportItemState::Queuing {
            self.on_queuing_for_target(tick);
        }
        if self.state == TransportItemState::Travelling {
            self.on_travel_to_target(tick);
        }
        if self.state == TransportItemState::Interacting {
            self.on_reached_target(tick);
        }
    }

    fn stop(&mut self, tick: &mut BrainTick<'_>) {
        self.on_start_travelling(tick);
        Self::set_can_path_below_surface(tick, false);
    }

    fn debug_name(&self) -> &'static str {
        "TransportItemsBetweenContainers"
    }
}
