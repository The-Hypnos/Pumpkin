use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicI32, Ordering};

use pumpkin_data::attributes::Attributes;
use pumpkin_data::damage::DamageType;
use pumpkin_data::entity::{EntityStatus, EntityType};
use pumpkin_data::environment_attribute::Activity;
use pumpkin_data::sound::{Sound, SoundCategory};
use pumpkin_data::tracked_data;
use pumpkin_nbt::compound::NbtCompound;
use pumpkin_util::math::boundingbox::EntityDimensions;

use crate::entity::ai::brain::activity::ActivityData;
use crate::entity::ai::brain::behavior::utils::{
    is_alive, is_other_target_much_further_away_than_current_attack_target,
};
use crate::entity::ai::brain::behavior::{self, BehaviorControl};
use crate::entity::ai::brain::memory::{
    MemoryModuleId, NearestVisibleLivingEntities, PackedMemories, types,
};
use crate::entity::ai::brain::sensing::{SensorType, is_entity_attackable};
use crate::entity::ai::brain::{Brain, BrainProvider, BrainTick};
use crate::entity::mob::hoglin_base::{self, ATTACK_ANIMATION_DURATION};
use crate::entity::mob::sounds::{self, AmbientSoundTimer};
use crate::entity::mob::{Mob, MobEntity};
use crate::entity::{Entity, EntityBase};

const ATTACK_INTERVAL: i32 = 40;
const BABY_ATTACK_INTERVAL: i32 = 15;
const ATTACK_DURATION: i64 = 200;
const SPEED_MULTIPLIER_WHEN_IDLING: f32 = 0.4;
const BABY_ATTACK_DAMAGE: f64 = 0.5;
const MAX_LOOK_DIST: f32 = 8.0;

pub struct ZoglinEntity {
    pub mob_entity: MobEntity,
    pub is_baby: AtomicBool,
    attack_animation_remaining_ticks: AtomicI32,
    ambient_sound_timer: AmbientSoundTimer,
}

impl ZoglinEntity {
    pub const XP_REWARD: u32 = 5;
    pub const BABY_DIMENSIONS: EntityDimensions = EntityDimensions {
        width: 0.75,
        height: 0.85,
        eye_height: 0.625,
    };

    pub fn new(entity: Entity) -> Arc<Self> {
        let mob_entity = MobEntity::new(entity);
        {
            let mut attributes = mob_entity
                .living_entity
                .attributes
                .write()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            if let Some(health) = attributes.get_mut(&Attributes::MAX_HEALTH.id) {
                health.base_value = 40.0;
                health.dirty.store(true, Ordering::Relaxed);
            }
            if let Some(speed) = attributes.get_mut(&Attributes::MOVEMENT_SPEED.id) {
                speed.base_value = 0.3;
                speed.dirty.store(true, Ordering::Relaxed);
            }
            if let Some(knockback_res) = attributes.get_mut(&Attributes::KNOCKBACK_RESISTANCE.id) {
                knockback_res.base_value = 0.6;
                knockback_res.dirty.store(true, Ordering::Relaxed);
            }
            if let Some(attack_kb) = attributes.get_mut(&Attributes::ATTACK_KNOCKBACK.id) {
                attack_kb.base_value = 1.0;
                attack_kb.dirty.store(true, Ordering::Relaxed);
            }
            if let Some(damage) = attributes.get_mut(&Attributes::ATTACK_DAMAGE.id) {
                damage.base_value = 6.0;
                damage.dirty.store(true, Ordering::Relaxed);
            }
        }
        mob_entity.living_entity.health.store(40.0);

        let zoglin = Arc::new(Self {
            mob_entity,
            is_baby: AtomicBool::new(false),
            attack_animation_remaining_ticks: AtomicI32::new(0),
            ambient_sound_timer: AmbientSoundTimer::new(),
        });
        zoglin.mob_entity.init_brain(zoglin.as_ref());
        zoglin
    }

    #[must_use]
    pub fn is_adult(&self) -> bool {
        !self.is_baby.load(Ordering::Relaxed)
    }

    fn make_sound(&self, sound: Sound) {
        sounds::make_sound(self, sound, SoundCategory::Hostile);
    }

    #[must_use]
    pub fn is_baby(&self) -> bool {
        self.is_baby.load(Ordering::Relaxed)
    }

    /// Vanilla `Zoglin.setBaby`; growing up keeps the lowered attack damage, as in vanilla.
    pub fn set_baby(&self, baby: bool) {
        self.mob_entity
            .set_baby_flag(&self.is_baby, tracked_data::zoglin::DATA_BABY_ID, baby);
        let living = &self.mob_entity.living_entity;
        living.entity.entity_dimension.store(if baby {
            Self::BABY_DIMENSIONS
        } else {
            Entity::type_dimensions(living.entity.entity_type)
        });
        if baby {
            living.set_attribute_base(&Attributes::ATTACK_DAMAGE, BABY_ATTACK_DAMAGE);
        }
    }
}

fn init_core_activity() -> ActivityData {
    ActivityData::with_priority_start(
        Activity::Core,
        0,
        vec![
            Box::new(behavior::Timed::new(behavior::LookAtTargetSink::new(
                45, 90,
            ))),
            Box::new(behavior::Timed::new(behavior::MoveToTargetSink::default())),
        ],
    )
}

fn init_idle_activity() -> ActivityData {
    ActivityData::with_priority_start(
        Activity::Idle,
        10,
        vec![
            Box::new(behavior::start_attacking(
                |_| true,
                find_nearest_valid_attack_target,
            )),
            Box::new(behavior::set_entity_look_target_sometimes(
                |_| true,
                MAX_LOOK_DIST,
                30,
                60,
            )),
            idle_movement_behaviors(),
        ],
    )
}

fn idle_movement_behaviors() -> Box<dyn BehaviorControl> {
    Box::new(behavior::run_one(vec![
        (
            Box::new(behavior::stroll(SPEED_MULTIPLIER_WHEN_IDLING, true)),
            2,
        ),
        (
            Box::new(behavior::set_walk_target_from_look_target(
                |_| true,
                SPEED_MULTIPLIER_WHEN_IDLING,
                3,
            )),
            2,
        ),
        (Box::new(behavior::DoNothing::new(30, 60)), 1),
    ]))
}

fn init_fight_activity() -> ActivityData {
    ActivityData::with_gate_memory(
        Activity::Fight,
        10,
        vec![
            Box::new(behavior::set_walk_target_from_attack_target_if_target_out_of_reach(1.0)),
            Box::new(behavior::melee_attack(is_adult, ATTACK_INTERVAL)),
            Box::new(behavior::melee_attack(
                |tick| !is_adult(tick),
                BABY_ATTACK_INTERVAL,
            )),
            Box::new(behavior::stop_attacking_if_target_invalid(
                |_, _| false,
                |_, _| {},
                true,
            )),
        ],
        types::ATTACK_TARGET.id(),
    )
}

fn is_adult(tick: &BrainTick<'_>) -> bool {
    as_zoglin(tick.mob).is_some_and(ZoglinEntity::is_adult)
}

fn as_zoglin(entity: &dyn EntityBase) -> Option<&ZoglinEntity> {
    entity.cast_any().downcast_ref::<ZoglinEntity>()
}

/// Vanilla `Zoglin.findNearestValidAttackTarget`.
fn find_nearest_valid_attack_target(tick: &BrainTick<'_>) -> Option<Arc<dyn EntityBase>> {
    let ctx = tick.visibility();
    let empty = NearestVisibleLivingEntities::empty();
    let visible = ctx
        .brain
        .get(types::NEAREST_VISIBLE_LIVING_ENTITIES)
        .unwrap_or(&empty);
    visible.find_closest(&ctx, |target| {
        let target_type = target.get_entity().entity_type;
        target_type != &EntityType::ZOGLIN
            && target_type != &EntityType::CREEPER
            && is_entity_attackable(&ctx, target.as_ref())
    })
}

/// Vanilla `Zoglin.updateActivity`.
fn update_activity(tick: &mut BrainTick<'_>) {
    let old_activity = tick.brain.get_active_non_core_activity();
    tick.brain
        .set_active_activity_to_first_valid(&[Activity::Fight, Activity::Idle]);
    let new_activity = tick.brain.get_active_non_core_activity();
    if new_activity == Some(Activity::Fight)
        && old_activity != Some(Activity::Fight)
        && let Some(zoglin) = as_zoglin(tick.mob)
    {
        zoglin.make_sound(Sound::EntityZoglinAngry);
    }
    let has_attack_target = tick.brain.has_memory_value(types::ATTACK_TARGET.id());
    tick.mob.get_mob_entity().set_attacking(has_attack_target);
}

fn set_attack_target(brain: &mut Brain, target: Arc<dyn EntityBase>) {
    brain.erase(types::CANT_REACH_WALK_TARGET_SINCE.id());
    brain.set_with_expiry(types::ATTACK_TARGET, target, ATTACK_DURATION);
}

const MEMORY_TYPES: &[MemoryModuleId] = &[];

const SENSOR_TYPES: &[SensorType] = &[
    SensorType::NearestLivingEntities,
    SensorType::NearestPlayers,
];

static ZOGLIN_PROVIDER: BrainProvider = BrainProvider {
    memory_types: MEMORY_TYPES,
    sensor_types: SENSOR_TYPES,
    activities: |_| {
        vec![
            init_core_activity(),
            init_idle_activity(),
            init_fight_activity(),
        ]
    },
};

impl Mob for ZoglinEntity {
    fn get_mob_entity(&self) -> &MobEntity {
        &self.mob_entity
    }

    fn mob_is_baby(&self) -> bool {
        self.is_baby.load(std::sync::atomic::Ordering::Relaxed)
    }

    fn make_brain(&self, packed: &PackedMemories) -> Brain {
        ZOGLIN_PROVIDER.make_brain(self, packed)
    }

    fn after_brain_tick(&self, tick: &mut BrainTick<'_>) {
        update_activity(tick);
    }

    fn custom_server_ai_step(&self, _caller: &dyn EntityBase) {
        if !is_alive(self) {
            self.mob_entity.apply_brain_inbox(self);
            return;
        }
        self.mob_entity.tick_brain(self);
    }

    fn mob_tick(&self, _caller: &dyn EntityBase) {
        if !is_alive(self) {
            return;
        }
        if self
            .attack_animation_remaining_ticks
            .load(Ordering::Relaxed)
            > 0
        {
            self.attack_animation_remaining_ticks
                .fetch_sub(1, Ordering::Relaxed);
        }
        if self.ambient_sound_timer.tick() {
            let angry = self.mob_entity.with_brain(self, |tick| {
                tick.brain.has_memory_value(types::ATTACK_TARGET.id())
            });
            self.make_sound(if angry {
                Sound::EntityZoglinAngry
            } else {
                Sound::EntityZoglinAmbient
            });
        }
    }

    fn do_hurt_target(&self, target: &dyn EntityBase) {
        if target.get_living_entity().is_none() {
            return;
        }
        let entity = &self.mob_entity.living_entity.entity;
        self.attack_animation_remaining_ticks
            .store(ATTACK_ANIMATION_DURATION, Ordering::Relaxed);
        entity
            .world
            .load()
            .send_entity_status(entity, EntityStatus::StartAttacking, None);
        self.make_sound(Sound::EntityZoglinAttack);
        hoglin_base::hurt_and_throw_target(self, target);
    }

    fn on_damage(&self, _damage_type: DamageType, source: Option<&dyn EntityBase>) {
        self.ambient_sound_timer.reset();
        let Some(attacker) = source else {
            return;
        };
        let world = self.mob_entity.living_entity.entity.world.load_full();
        let Some(attacker) = world
            .get_entity_by_id(attacker.get_entity().entity_id)
            .filter(|attacker| attacker.get_living_entity().is_some())
        else {
            return;
        };
        // Queued: taking the victim's brain here deadlocks two mobs fighting
        self.mob_entity.post_to_brain(Box::new(move |tick| {
            let Some(living) = attacker.get_living_entity() else {
                return;
            };
            if tick.mob.can_attack(living)
                && !is_other_target_much_further_away_than_current_attack_target(
                    tick.brain,
                    tick.mob,
                    attacker.as_ref(),
                    4.0,
                )
            {
                set_attack_target(tick.brain, Arc::clone(&attacker));
            }
        }));
    }

    fn get_base_experience_reward(&self) -> u32 {
        Self::XP_REWARD
    }

    /// Vanilla `Zoglin.finalizeSpawn`: one in five spawns as a baby.
    fn finalize_spawn(
        &self,
        _world: &Arc<crate::world::World>,
        group_data: Option<crate::entity::mob::spawn::SpawnGroupData>,
    ) -> Option<crate::entity::mob::spawn::SpawnGroupData> {
        if rand::random::<f32>() < 0.2 {
            self.set_baby(true);
        }
        self.mob_entity.finalize_spawn_base();
        group_data
    }

    fn spawn_as_baby(&self) -> bool {
        self.set_baby(true);
        true
    }

    fn mob_write_nbt(&self, nbt: &mut NbtCompound) {
        nbt.put_bool("IsBaby", !self.is_adult());
    }

    fn mob_read_nbt(&self, nbt: &NbtCompound) {
        self.set_baby(nbt.get_bool("IsBaby").unwrap_or(false));
    }
}
