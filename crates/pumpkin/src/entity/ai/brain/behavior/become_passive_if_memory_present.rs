use super::super::memory::{MemoryModuleId, MemoryStatus, types};
use super::one_shot::OneShot;

#[must_use]
pub fn become_passive_if_memory_present(
    pacifying_memory: MemoryModuleId,
    pacify_duration: i64,
) -> OneShot {
    OneShot::with_required(
        "BecomePassiveIfMemoryPresent",
        vec![
            (types::PACIFIED.id(), MemoryStatus::ValueAbsent),
            (pacifying_memory, MemoryStatus::ValuePresent),
        ],
        vec![types::ATTACK_TARGET.id()],
        move |tick| {
            tick.brain
                .set_with_expiry(types::PACIFIED, true, pacify_duration);
            tick.brain.erase(types::ATTACK_TARGET.id());
            true
        },
    )
}
