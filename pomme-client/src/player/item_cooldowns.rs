//! Vanilla `ItemCooldowns`: the cooldown groups the server put on cooldown,
//! counted down by the player's tick.

use std::collections::HashMap;

use azalea_inventory::ItemStackData;
use azalea_inventory::components::UseCooldown;

use super::interaction::stack_component;

#[derive(Default)]
pub struct ItemCooldowns {
    /// Each group's `(start, end)` tick.
    cooldowns: HashMap<String, (i32, i32)>,
    tick_count: i32,
}

impl ItemCooldowns {
    pub fn is_on_cooldown(&self, stack: &ItemStackData) -> bool {
        self.cooldown_percent(stack, 0.0) > 0.0
    }

    pub fn cooldown_percent(&self, stack: &ItemStackData, partial_tick: f32) -> f32 {
        let Some(&(start, end)) = self.cooldowns.get(&cooldown_group(stack)) else {
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
        if duration == 0 {
            self.cooldowns.remove(&group);
        } else {
            let start = self.tick_count;
            self.cooldowns
                .insert(group, (start, start.wrapping_add(duration)));
        }
    }
}

/// Vanilla `ItemCooldowns.getCooldownGroup`: the `use_cooldown` group, else
/// the item's id.
fn cooldown_group(stack: &ItemStackData) -> String {
    stack_component::<UseCooldown>(stack)
        .and_then(|c| c.cooldown_group.map(|group| group.to_string()))
        .unwrap_or_else(|| stack.kind.to_string())
}

#[cfg(test)]
mod tests {
    use azalea_registry::builtin::ItemKind;

    use super::*;

    fn shield() -> ItemStackData {
        ItemStackData::new(ItemKind::Shield, 1)
    }

    #[test]
    fn a_cooldown_ends_after_its_duration() {
        let mut cooldowns = ItemCooldowns::default();
        cooldowns.set("minecraft:shield".into(), 2);
        assert!(cooldowns.is_on_cooldown(&shield()));
        assert_eq!(cooldowns.cooldown_percent(&shield(), 0.5), 0.75);
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
}
