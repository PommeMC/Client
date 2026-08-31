//! Mob effect registry data and the local player's active-effect set.
//!
//! Table transcribed from `MobEffects.java` in registry order (protocol ids
//! 0..=39); the comparator and client tick mirror `MobEffectInstance.java`.

pub struct MobEffectInfo {
    /// Registry path, also the icon filename under `textures/mob_effect/`.
    pub name: &'static str,
    /// `MobEffectCategory.BENEFICIAL` only; NEUTRAL shares the harmful HUD row.
    pub beneficial: bool,
    /// Packed RGB, used as the vanilla sort tie-break.
    pub color: u32,
}

const fn effect(name: &'static str, beneficial: bool, color: u32) -> MobEffectInfo {
    MobEffectInfo {
        name,
        beneficial,
        color,
    }
}

pub const MOB_EFFECTS: [MobEffectInfo; 40] = [
    effect("speed", true, 3402751),
    effect("slowness", false, 9154528),
    effect("haste", true, 14270531),
    effect("mining_fatigue", false, 4866583),
    effect("strength", true, 16762624),
    effect("instant_health", true, 16262179),
    effect("instant_damage", false, 11101546),
    effect("jump_boost", true, 16646020),
    effect("nausea", false, 5578058),
    effect("regeneration", true, 13458603),
    effect("resistance", true, 9520880),
    effect("fire_resistance", true, 0xFF9900),
    effect("water_breathing", true, 10017472),
    effect("invisibility", true, 0xF6F6F6),
    effect("blindness", false, 2039587),
    effect("night_vision", true, 12779366),
    effect("hunger", false, 5797459),
    effect("weakness", false, 0x484D48),
    effect("poison", false, 8889187),
    effect("wither", false, 7561558),
    effect("health_boost", true, 16284963),
    effect("absorption", true, 0x2552A5),
    effect("saturation", true, 16262179),
    effect("glowing", false, 9740385),
    effect("levitation", false, 0xCEFFFF),
    effect("luck", true, 5882118),
    effect("unluck", false, 12624973),
    effect("slow_falling", true, 15978425),
    effect("conduit_power", true, 1950417),
    effect("dolphins_grace", true, 8954814),
    effect("bad_omen", false, 745784),
    effect("hero_of_the_village", true, 0x44FF44),
    effect("darkness", false, 2696993),
    effect("trial_omen", false, 0x16A6A6),
    effect("raid_omen", false, 14565464),
    effect("wind_charged", false, 12438015),
    effect("weaving", false, 7891290),
    effect("oozing", false, 10092451),
    effect("infested", false, 9214860),
    effect("breath_of_the_nautilus", true, 65518),
];

pub fn info(effect_id: u32) -> Option<&'static MobEffectInfo> {
    MOB_EFFECTS.get(effect_id as usize)
}

/// Vanilla `MobEffectInstance.INFINITE_DURATION`.
pub const INFINITE_DURATION: i32 = -1;

// TODO: amplifier and show_particles once the inventory effect panel and
// effect particles are ported.
#[derive(Clone)]
pub struct MobEffectInstance {
    pub effect_id: u32,
    /// Remaining ticks; `-1` = infinite.
    pub duration: i32,
    pub ambient: bool,
    pub show_icon: bool,
}

impl MobEffectInstance {
    fn is_infinite(&self) -> bool {
        self.duration == INFINITE_DURATION
    }

    /// Vanilla `endsWithin`.
    pub fn ends_within(&self, ticks: i32) -> bool {
        !self.is_infinite() && self.duration <= ticks
    }
}

/// Vanilla `MobEffectInstance.compareTo`.
fn cmp_vanilla(a: &MobEffectInstance, b: &MobEffectInstance) -> std::cmp::Ordering {
    let color = |e: &MobEffectInstance| info(e.effect_id).map_or(0, |i| i.color);
    if (a.duration > 32147 && b.duration > 32147) || (a.ambient && b.ambient) {
        a.ambient.cmp(&b.ambient).then(color(a).cmp(&color(b)))
    } else {
        a.ambient
            .cmp(&b.ambient)
            .then(a.is_infinite().cmp(&b.is_infinite()))
            .then(a.duration.cmp(&b.duration))
            .then(color(a).cmp(&color(b)))
    }
}

/// The local player's active effects (vanilla `LivingEntity.activeEffects`).
#[derive(Default)]
pub struct ActiveMobEffects(Vec<MobEffectInstance>);

impl ActiveMobEffects {
    /// Vanilla `forceAddEffect`: replaces any instance of the same effect.
    pub fn update(&mut self, instance: MobEffectInstance) {
        match self
            .0
            .iter_mut()
            .find(|e| e.effect_id == instance.effect_id)
        {
            Some(existing) => *existing = instance,
            None => self.0.push(instance),
        }
    }

    pub fn remove(&mut self, effect_id: u32) {
        self.0.retain(|e| e.effect_id != effect_id);
    }

    pub fn clear(&mut self) {
        self.0.clear();
    }

    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }

    /// Vanilla `tickClient`: durations floor at 0 and the entry stays until
    /// the server's remove packet.
    pub fn tick(&mut self) {
        for e in &mut self.0 {
            if e.duration > 0 {
                e.duration -= 1;
            }
        }
    }

    /// Vanilla `Ordering.natural().reverse().sortedCopy(activeEffects)`.
    pub fn sorted_desc(&self) -> Vec<MobEffectInstance> {
        let mut sorted = self.0.clone();
        sorted.sort_by(|a, b| cmp_vanilla(b, a));
        sorted
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn inst(effect_id: u32, duration: i32, ambient: bool) -> MobEffectInstance {
        MobEffectInstance {
            effect_id,
            duration,
            ambient,
            show_icon: true,
        }
    }

    #[test]
    fn table_anchors() {
        assert_eq!(MOB_EFFECTS[0].name, "speed");
        assert_eq!(MOB_EFFECTS[23].name, "glowing");
        assert!(!MOB_EFFECTS[23].beneficial); // NEUTRAL -> bottom row
        assert_eq!(MOB_EFFECTS[39].name, "breath_of_the_nautilus");
        assert!(info(40).is_none());
    }

    #[test]
    fn comparator_duration_then_ambient() {
        assert_eq!(
            cmp_vanilla(&inst(0, 100, false), &inst(1, 200, false)),
            std::cmp::Ordering::Less
        );
        assert_eq!(
            cmp_vanilla(&inst(0, 100, false), &inst(1, 100, true)),
            std::cmp::Ordering::Less
        );
    }

    #[test]
    fn comparator_infinite_beats_finite() {
        assert_eq!(
            cmp_vanilla(
                &inst(0, INFINITE_DURATION, false),
                &inst(1, 1_000_000, false)
            ),
            std::cmp::Ordering::Greater
        );
    }

    #[test]
    fn comparator_special_branch_ignores_duration() {
        // Both > 32147: only ambient then color count. speed(3402751) <
        // slowness(9154528).
        assert_eq!(
            cmp_vanilla(&inst(0, 40000, false), &inst(1, 50000, false)),
            std::cmp::Ordering::Less
        );
        // Both ambient: same rule even at short durations.
        assert_eq!(
            cmp_vanilla(&inst(0, 10, true), &inst(1, 5000, true)),
            std::cmp::Ordering::Less
        );
    }

    #[test]
    fn comparator_color_tie_break() {
        assert_eq!(
            cmp_vanilla(&inst(0, 100, false), &inst(1, 100, false)),
            std::cmp::Ordering::Less
        );
        assert_eq!(
            cmp_vanilla(&inst(0, 100, false), &inst(0, 100, false)),
            std::cmp::Ordering::Equal
        );
    }

    #[test]
    fn tick_floors_at_zero_and_skips_infinite() {
        let mut effects = ActiveMobEffects::default();
        effects.update(inst(0, 1, false));
        effects.update(inst(1, INFINITE_DURATION, false));
        effects.tick();
        effects.tick();
        let sorted = effects.sorted_desc();
        assert_eq!(sorted.len(), 2);
        assert_eq!(sorted[0].duration, INFINITE_DURATION);
        assert_eq!(sorted[1].duration, 0);
    }

    #[test]
    fn update_replaces_same_effect() {
        let mut effects = ActiveMobEffects::default();
        effects.update(inst(0, 100, false));
        effects.update(inst(0, 300, true));
        let sorted = effects.sorted_desc();
        assert_eq!(sorted.len(), 1);
        assert_eq!(sorted[0].duration, 300);
        assert!(sorted[0].ambient);
    }
}
