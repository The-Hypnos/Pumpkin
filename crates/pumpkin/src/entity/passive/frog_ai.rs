use pumpkin_data::item_stack::ItemStack;
use pumpkin_data::tag::{self, Taggable};

use crate::entity::mob::Mob;

/// Vanilla `FrogAi.getTemptations`, shared by frogs and tadpoles.
#[must_use]
pub fn is_temptation(_mob: &dyn Mob, stack: &ItemStack) -> bool {
    stack.item.has_tag(&tag::Item::MINECRAFT_FROG_FOOD)
}
