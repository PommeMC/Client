//! Vanilla `ItemCooldowns`: the cooldown groups the server put on cooldown,
//! counted down by the player's tick.

use std::collections::HashMap;

use azalea_inventory::ItemStackData;
use azalea_inventory::components::UseCooldown;
use azalea_registry::builtin::ItemKind;

use super::interaction::stack_component;

/// A cooldown group's identifier. An item's id is held as the item, so the
/// common lookup (an item without a `use_cooldown` group) doesn't allocate.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub enum CooldownGroup {
    Item(ItemKind),
    Other(String),
}

impl CooldownGroup {
    fn parse(id: String) -> Self {
        match id.parse() {
            Ok(kind) => Self::Item(kind),
            Err(_) => Self::Other(id),
        }
    }

    /// Vanilla `ItemCooldowns.getCooldownGroup`: the `use_cooldown` group,
    /// else the item's id.
    pub fn of(stack: &ItemStackData) -> Self {
        match stack_component::<UseCooldown>(stack).and_then(|c| c.cooldown_group) {
            Some(group) => Self::parse(group.to_string()),
            None => Self::Item(stack.kind),
        }
    }
}

#[derive(Default)]
pub struct ItemCooldowns {
    /// Each group's `(start, end)` tick.
    cooldowns: HashMap<CooldownGroup, (i32, i32)>,
    tick_count: i32,
}

impl ItemCooldowns {
    pub fn is_on_cooldown(&self, stack: &ItemStackData) -> bool {
        self.cooldown_percent(&CooldownGroup::of(stack), 0.0) > 0.0
    }

    pub fn cooldown_percent(&self, group: &CooldownGroup, partial_tick: f32) -> f32 {
        let Some(&(start, end)) = self.cooldowns.get(group) else {
            return 0.0;
        };
        let remaining = end as f32 - (self.tick_count as f32 + partial_tick);
        (remaining / (end - start) as f32).clamp(0.0, 1.0)
    }

    pub fn tick(&mut self) {
        self.tick_count = self.tick_count.wrapping_add(1);
        let now = self.tick_count;
        self.cooldowns.retain(|_, &mut (_, end)| end > now);
    }

    /// Vanilla `ClientPacketListener.handleItemCooldown`: a zero duration
    /// ends the group's cooldown.
    pub fn set(&mut self, group: String, duration: i32) {
        let group = CooldownGroup::parse(group);
        if duration == 0 {
            self.cooldowns.remove(&group);
        } else {
            let start = self.tick_count;
            self.cooldowns
                .insert(group, (start, start.wrapping_add(duration)));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn shield() -> ItemStackData {
        ItemStackData::new(ItemKind::Shield, 1)
    }

    #[test]
    fn a_cooldown_ends_after_its_duration() {
        let mut cooldowns = ItemCooldowns::default();
        cooldowns.set("minecraft:shield".into(), 2);
        assert!(cooldowns.is_on_cooldown(&shield()));
        assert_eq!(
            cooldowns.cooldown_percent(&CooldownGroup::Item(ItemKind::Shield), 0.5),
            0.75
        );
        cooldowns.tick();
        assert!(cooldowns.is_on_cooldown(&shield()));
        cooldowns.tick();
        assert!(!cooldowns.is_on_cooldown(&shield()));
    }

    #[test]
    fn a_zero_duration_clears_the_group() {
        let mut cooldowns = ItemCooldowns::default();
        cooldowns.set("minecraft:shield".into(), 100);
        cooldowns.set("minecraft:shield".into(), 0);
        assert!(!cooldowns.is_on_cooldown(&shield()));
        assert!(!cooldowns.is_on_cooldown(&ItemStackData::new(ItemKind::Stick, 1)));
    }

    #[test]
    fn only_an_item_id_parses_as_the_item() {
        assert_eq!(
            CooldownGroup::parse("minecraft:shield".into()),
            CooldownGroup::Item(ItemKind::Shield)
        );
        assert_eq!(
            CooldownGroup::parse("pomme:shield".into()),
            CooldownGroup::Other("pomme:shield".into())
        );
    }
}
