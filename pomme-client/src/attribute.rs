use std::collections::HashMap;

/// Pomme-native attribute identifiers matching vanilla 26.2's built-in
/// `minecraft:attribute` registry.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub enum AttributeKind {
    AirDragModifier,
    Armor,
    ArmorToughness,
    AttackDamage,
    AttackKnockback,
    AttackSpeed,
    BelowNameDistance,
    BlockBreakSpeed,
    BlockInteractionRange,
    Bounciness,
    BurningTime,
    CameraDistance,
    ExplosionKnockbackResistance,
    EntityInteractionRange,
    FallDamageMultiplier,
    FlyingSpeed,
    FollowRange,
    FrictionModifier,
    Gravity,
    JumpStrength,
    KnockbackResistance,
    Luck,
    MaxAbsorption,
    MaxHealth,
    MiningEfficiency,
    MovementEfficiency,
    MovementSpeed,
    NameTagDistance,
    OxygenBonus,
    SafeFallDistance,
    Scale,
    SneakingSpeed,
    SpawnReinforcements,
    StepHeight,
    SubmergedMiningSpeed,
    SweepingDamageRatio,
    TemptRange,
    WaterMovementEfficiency,
    WaypointTransmitRange,
    WaypointReceiveRange,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct AttributeDefinition {
    pub default_value: f64,
    pub min_value: f64,
    pub max_value: f64,
}

impl AttributeDefinition {
    pub fn sanitize(self, value: f64) -> f64 {
        if value.is_nan() {
            self.min_value
        } else {
            value.clamp(self.min_value, self.max_value)
        }
    }
}

impl AttributeKind {
    /// Vanilla 26.2 `Attributes` defaults and `RangedAttribute` bounds.
    pub const fn definition(self) -> AttributeDefinition {
        use AttributeKind::*;
        match self {
            AirDragModifier => AttributeDefinition::new(1.0, 0.0, 2048.0),
            Armor => AttributeDefinition::new(0.0, 0.0, 30.0),
            ArmorToughness => AttributeDefinition::new(0.0, 0.0, 20.0),
            AttackDamage => AttributeDefinition::new(2.0, 0.0, 2048.0),
            AttackKnockback => AttributeDefinition::new(0.0, 0.0, 5.0),
            AttackSpeed => AttributeDefinition::new(4.0, 0.0, 1024.0),
            BelowNameDistance => AttributeDefinition::new(10.0, 0.0, 512.0),
            BlockBreakSpeed => AttributeDefinition::new(1.0, 0.0, 1024.0),
            BlockInteractionRange => AttributeDefinition::new(4.5, 0.0, 64.0),
            Bounciness => AttributeDefinition::new(0.0, 0.0, 1.0),
            BurningTime => AttributeDefinition::new(1.0, 0.0, 1024.0),
            CameraDistance => AttributeDefinition::new(4.0, 0.0, 32.0),
            ExplosionKnockbackResistance => AttributeDefinition::new(0.0, 0.0, 1.0),
            EntityInteractionRange => AttributeDefinition::new(3.0, 0.0, 64.0),
            FallDamageMultiplier => AttributeDefinition::new(1.0, 0.0, 100.0),
            FlyingSpeed => AttributeDefinition::new(0.4, 0.0, 1024.0),
            FollowRange => AttributeDefinition::new(32.0, 0.0, 2048.0),
            FrictionModifier => AttributeDefinition::new(1.0, 0.0, 2048.0),
            Gravity => AttributeDefinition::new(0.08, -1.0, 1.0),
            JumpStrength => AttributeDefinition::new(0.42_f32 as f64, 0.0, 32.0),
            KnockbackResistance => AttributeDefinition::new(0.0, -2.0, 1.0),
            Luck => AttributeDefinition::new(0.0, -1024.0, 1024.0),
            MaxAbsorption => AttributeDefinition::new(0.0, 0.0, 2048.0),
            MaxHealth => AttributeDefinition::new(20.0, 1.0, 1024.0),
            MiningEfficiency => AttributeDefinition::new(0.0, 0.0, 1024.0),
            MovementEfficiency => AttributeDefinition::new(0.0, 0.0, 1.0),
            MovementSpeed => AttributeDefinition::new(0.7, 0.0, 1024.0),
            NameTagDistance => AttributeDefinition::new(64.0, 0.0, 512.0),
            OxygenBonus => AttributeDefinition::new(0.0, 0.0, 1024.0),
            SafeFallDistance => AttributeDefinition::new(3.0, -1024.0, 1024.0),
            Scale => AttributeDefinition::new(1.0, 0.0625, 16.0),
            SneakingSpeed => AttributeDefinition::new(0.3, 0.0, 1.0),
            SpawnReinforcements => AttributeDefinition::new(0.0, 0.0, 1.0),
            StepHeight => AttributeDefinition::new(0.6, 0.0, 10.0),
            SubmergedMiningSpeed => AttributeDefinition::new(0.2, 0.0, 20.0),
            SweepingDamageRatio => AttributeDefinition::new(0.0, 0.0, 1.0),
            TemptRange => AttributeDefinition::new(10.0, 0.0, 2048.0),
            WaterMovementEfficiency => AttributeDefinition::new(0.0, 0.0, 1.0),
            WaypointTransmitRange => AttributeDefinition::new(0.0, 0.0, 60_000_000.0),
            WaypointReceiveRange => AttributeDefinition::new(0.0, 0.0, 60_000_000.0),
        }
    }

    /// Base value supplied by vanilla 26.2 `Player.createAttributes()`.
    ///
    /// `None` means the attribute is not part of the player's supplier, so a
    /// server snapshot for it must be ignored just like vanilla's
    /// `ClientPacketListener.handleUpdateAttributes` does.
    pub const fn player_base_value(self) -> Option<f64> {
        use AttributeKind::*;
        match self {
            FlyingSpeed | FollowRange | SpawnReinforcements | TemptRange => None,
            AttackDamage => Some(1.0),
            MovementSpeed => Some(0.1_f32 as f64),
            WaypointTransmitRange | WaypointReceiveRange => Some(60_000_000.0),
            _ => Some(self.definition().default_value),
        }
    }
}

impl AttributeDefinition {
    const fn new(default_value: f64, min_value: f64, max_value: f64) -> Self {
        Self {
            default_value,
            min_value,
            max_value,
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum AttributeModifierOperation {
    Value,
    MultipliedBase,
    MultipliedTotal,
}

#[derive(Clone, Debug, PartialEq)]
pub struct AttributeModifier {
    pub id: String,
    pub amount: f64,
    pub operation: AttributeModifierOperation,
}

#[derive(Clone, Debug, PartialEq)]
pub struct AttributeSnapshot {
    pub attribute: AttributeKind,
    pub base: f64,
    pub modifiers: Vec<AttributeModifier>,
}

#[derive(Clone, Debug)]
pub struct AttributeInstance {
    attribute: AttributeKind,
    base_value: f64,
    modifiers: HashMap<String, AttributeModifier>,
}

impl AttributeInstance {
    pub fn new(attribute: AttributeKind, base_value: f64) -> Self {
        Self {
            attribute,
            base_value,
            modifiers: HashMap::new(),
        }
    }

    pub fn add_or_update_modifier(&mut self, modifier: AttributeModifier) {
        self.modifiers.insert(modifier.id.clone(), modifier);
    }

    pub fn clear_modifiers(&mut self) {
        self.modifiers.clear();
    }

    pub fn apply_snapshot(&mut self, snapshot: &AttributeSnapshot) {
        debug_assert_eq!(self.attribute, snapshot.attribute);
        self.base_value = snapshot.base;
        self.modifiers.clear();
        for modifier in &snapshot.modifiers {
            self.add_or_update_modifier(modifier.clone());
        }
    }

    /// Vanilla 26.2 `AttributeInstance.calculateValue`: modifier operation
    /// phases are ordered, regardless of the order modifiers were received.
    pub fn value(&self) -> f64 {
        let mut base = self.base_value;
        for modifier in self.modifiers.values() {
            if modifier.operation == AttributeModifierOperation::Value {
                base += modifier.amount;
            }
        }

        let mut result = base;
        for modifier in self.modifiers.values() {
            if modifier.operation == AttributeModifierOperation::MultipliedBase {
                result += base * modifier.amount;
            }
        }

        for modifier in self.modifiers.values() {
            if modifier.operation == AttributeModifierOperation::MultipliedTotal {
                result *= 1.0 + modifier.amount;
            }
        }

        self.attribute.definition().sanitize(result)
    }
}

#[derive(Clone, Debug, Default)]
pub struct AttributeMap {
    instances: HashMap<AttributeKind, AttributeInstance>,
}

impl AttributeMap {
    pub fn player() -> Self {
        use AttributeKind::*;
        const ALL: [AttributeKind; 40] = [
            AirDragModifier,
            Armor,
            ArmorToughness,
            AttackDamage,
            AttackKnockback,
            AttackSpeed,
            BelowNameDistance,
            BlockBreakSpeed,
            BlockInteractionRange,
            Bounciness,
            BurningTime,
            CameraDistance,
            ExplosionKnockbackResistance,
            EntityInteractionRange,
            FallDamageMultiplier,
            FlyingSpeed,
            FollowRange,
            FrictionModifier,
            Gravity,
            JumpStrength,
            KnockbackResistance,
            Luck,
            MaxAbsorption,
            MaxHealth,
            MiningEfficiency,
            MovementEfficiency,
            MovementSpeed,
            NameTagDistance,
            OxygenBonus,
            SafeFallDistance,
            Scale,
            SneakingSpeed,
            SpawnReinforcements,
            StepHeight,
            SubmergedMiningSpeed,
            SweepingDamageRatio,
            TemptRange,
            WaterMovementEfficiency,
            WaypointTransmitRange,
            WaypointReceiveRange,
        ];

        let mut map = Self::default();
        for attribute in ALL {
            if let Some(base_value) = attribute.player_base_value() {
                map.instances
                    .insert(attribute, AttributeInstance::new(attribute, base_value));
            }
        }
        map
    }

    pub fn instance(&self, attribute: AttributeKind) -> Option<&AttributeInstance> {
        self.instances.get(&attribute)
    }

    pub fn value(&self, attribute: AttributeKind) -> Option<f64> {
        self.instance(attribute).map(AttributeInstance::value)
    }

    /// Apply a server snapshot only when this map's vanilla supplier contains
    /// the attribute. Used for the local player to mirror vanilla's warning +
    /// ignore behavior for unsupported attributes.
    pub fn apply_snapshot(&mut self, snapshot: &AttributeSnapshot) -> bool {
        let Some(instance) = self.instances.get_mut(&snapshot.attribute) else {
            return false;
        };
        instance.apply_snapshot(snapshot);
        true
    }

    /// Retain a snapshot even when Pomme does not yet model the remote
    /// entity's complete attribute supplier. Existing instances are replaced
    /// with vanilla snapshot semantics; new ones start from the packet base.
    pub fn apply_snapshot_or_insert(&mut self, snapshot: &AttributeSnapshot) {
        self.instances
            .entry(snapshot.attribute)
            .or_insert_with(|| AttributeInstance::new(snapshot.attribute, snapshot.base))
            .apply_snapshot(snapshot);
    }

    /// Vanilla respawn `assignBaseValues`: keep every base value but drop the
    /// old player's modifiers from the newly constructed attribute map.
    pub fn clear_modifiers(&mut self) {
        for instance in self.instances.values_mut() {
            instance.clear_modifiers();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn modifier(id: &str, amount: f64, operation: AttributeModifierOperation) -> AttributeModifier {
        AttributeModifier {
            id: id.to_owned(),
            amount,
            operation,
        }
    }

    #[test]
    fn player_supplier_matches_vanilla_overrides_and_absences() {
        let attributes = AttributeMap::player();

        assert_eq!(
            attributes
                .instance(AttributeKind::MovementSpeed)
                .map(|instance| instance.base_value),
            Some(0.1_f32 as f64)
        );
        assert_eq!(
            attributes
                .instance(AttributeKind::AttackDamage)
                .map(|instance| instance.base_value),
            Some(1.0)
        );
        assert_eq!(
            attributes
                .instance(AttributeKind::WaypointTransmitRange)
                .map(|instance| instance.base_value),
            Some(60_000_000.0)
        );
        assert_eq!(
            attributes
                .instance(AttributeKind::WaypointReceiveRange)
                .map(|instance| instance.base_value),
            Some(60_000_000.0)
        );
        assert_eq!(attributes.value(AttributeKind::MaxHealth), Some(20.0));
        assert_eq!(attributes.instances.len(), 36);

        assert!(attributes.instance(AttributeKind::FlyingSpeed).is_none());
        assert!(attributes.instance(AttributeKind::FollowRange).is_none());
        assert!(
            attributes
                .instance(AttributeKind::SpawnReinforcements)
                .is_none()
        );
        assert!(attributes.instance(AttributeKind::TemptRange).is_none());
    }

    #[test]
    fn calculation_groups_modifiers_by_vanilla_operation_order() {
        let mut instance = AttributeInstance::new(AttributeKind::AttackSpeed, 10.0);
        // Deliberately add these out of operation order.
        instance.add_or_update_modifier(modifier(
            "minecraft:total",
            0.5,
            AttributeModifierOperation::MultipliedTotal,
        ));
        instance.add_or_update_modifier(modifier(
            "minecraft:base",
            0.25,
            AttributeModifierOperation::MultipliedBase,
        ));
        instance.add_or_update_modifier(modifier(
            "minecraft:add",
            2.0,
            AttributeModifierOperation::Value,
        ));

        // base = 10 + 2 = 12
        // multiplied-base = 12 + 12 * .25 = 15
        // multiplied-total = 15 * 1.5 = 22.5
        assert_eq!(instance.value(), 22.5);
    }

    #[test]
    fn ranged_attribute_sanitization_matches_vanilla() {
        let mut armor = AttributeInstance::new(AttributeKind::Armor, 100.0);
        assert_eq!(armor.value(), 30.0);

        armor.base_value = f64::NAN;
        assert_eq!(armor.value(), 0.0);

        let health = AttributeInstance::new(AttributeKind::MaxHealth, -5.0);
        assert_eq!(health.value(), 1.0);
    }

    #[test]
    fn snapshot_replaces_old_base_and_modifiers_by_id() {
        let mut instance = AttributeInstance::new(AttributeKind::BlockBreakSpeed, 1.0);
        instance.add_or_update_modifier(modifier(
            "minecraft:old",
            2.0,
            AttributeModifierOperation::Value,
        ));

        instance.apply_snapshot(&AttributeSnapshot {
            attribute: AttributeKind::BlockBreakSpeed,
            base: 3.0,
            modifiers: vec![modifier(
                "minecraft:new",
                0.5,
                AttributeModifierOperation::MultipliedTotal,
            )],
        });

        assert_eq!(instance.base_value, 3.0);
        assert!(
            instance
                .modifiers
                .values()
                .all(|modifier| modifier.id != "minecraft:old")
        );
        assert!(
            instance
                .modifiers
                .values()
                .any(|modifier| modifier.id == "minecraft:new")
        );
        assert_eq!(instance.value(), 4.5);
    }

    #[test]
    fn local_player_ignores_attributes_outside_its_supplier() {
        let mut attributes = AttributeMap::player();
        let snapshot = AttributeSnapshot {
            attribute: AttributeKind::FollowRange,
            base: 100.0,
            modifiers: vec![],
        };

        assert!(!attributes.apply_snapshot(&snapshot));
        assert_eq!(attributes.value(AttributeKind::FollowRange), None);
    }

    #[test]
    fn respawn_base_copy_behavior_drops_modifiers_without_resetting_base() {
        let mut attributes = AttributeMap::player();
        let instance = attributes
            .instances
            .get_mut(&AttributeKind::MaxHealth)
            .expect("player has max health");
        instance.base_value = 40.0;
        instance.add_or_update_modifier(modifier(
            "minecraft:bonus",
            10.0,
            AttributeModifierOperation::Value,
        ));
        assert_eq!(instance.value(), 50.0);

        attributes.clear_modifiers();

        assert_eq!(
            attributes
                .instance(AttributeKind::MaxHealth)
                .map(|instance| instance.base_value),
            Some(40.0)
        );
        assert_eq!(attributes.value(AttributeKind::MaxHealth), Some(40.0));
    }
}
