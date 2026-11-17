//! Vanilla's client-side answer to a right click on an entity
//! (`Player.interactOn`): whether the entity took the click, which ends it,
//! or passed, which lets the held item be used. The server decides what
//! really happens from the interact packet; this only has to agree with the
//! vanilla client's guess, including where that guess differs from the
//! server's.
// TODO: state the store doesn't keep yet, and what stands in for it:
// - leash links: nothing is leashed, so an entity leashed to the player doesn't
//   take the click, shears cut no leash, and a lead is always taken;
// - a pet's owner: every tame animal is the player's;
// - `AgeableMob.ageLockParticleTimer`, the 40 ticks a baby refuses a second
//   golden dandelion;
// - `prevent_armor_change` on a mob's equipment, which stops shears;
// - a pig's saddle before 1.21.5, which was a synced flag and isn't read, so a
//   saddled pig there passes an empty hand.

use azalea_inventory::components::{CustomName, Damage, Dye, EquipmentSlot, Equippable};
use glam::DVec3;
use pomme_item::ItemStackData;
use pomme_protocol::serverbound::InteractionHand;
use pomme_registry::builtin::{EntityKind, ItemKind};
use pomme_registry::tags;

use super::interaction::{can_be_equipped_by, stack_component};
use crate::entity::{LivingEntity, is_ageable_mob, is_boat, is_chest_boat, is_equine, is_llama};
use crate::recipe::ItemTags;

/// A click the entity took.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Taken {
    /// Vanilla `SUCCESS`: the hand swings.
    Swing,
    /// Vanilla `SUCCESS_SERVER` and `CONSUME`: no swing from the client.
    Quiet,
}

const SWING: Option<Taken> = Some(Taken::Swing);
const QUIET: Option<Taken> = Some(Taken::Quiet);

fn swing_if(taken: bool) -> Option<Taken> {
    taken.then_some(Taken::Swing)
}

fn quiet_if(taken: bool) -> Option<Taken> {
    taken.then_some(Taken::Quiet)
}

/// The clicked entity and how many ride it.
#[derive(Clone, Copy)]
pub struct Target<'a> {
    pub entity: &'a LivingEntity,
    pub passengers: usize,
}

/// The local player, for one hand's click.
#[derive(Clone, Copy)]
pub struct Clicker<'a> {
    pub held: Option<&'a ItemStackData>,
    pub hand: InteractionHand,
    pub item_tags: &'a ItemTags,
    pub sneaking: bool,
    /// Vanilla `Player.isHolding(GOLDEN_DANDELION)`: either hand.
    pub holding_golden_dandelion: bool,
    pub position: DVec3,
    pub thundering: bool,
}

impl Clicker<'_> {
    fn holds(&self, item: ItemKind) -> bool {
        self.held.is_some_and(|stack| stack.kind == item)
    }

    fn in_tag(&self, tag: &str) -> bool {
        self.held.is_some_and(|stack| self.tagged(stack, tag))
    }

    /// `stack` is in `#minecraft:<tag>` as the server synced it, or as the
    /// game's own data pack has it on a server that doesn't sync the tag.
    fn tagged(&self, stack: &ItemStackData, tag: &str) -> bool {
        match self.item_tags.get(&format!("minecraft:{tag}")) {
            Some(synced) => synced.contains(&(stack.kind as u32)),
            None => tags::items::named(tag).is_some_and(|tag| tag.contains(&stack.kind)),
        }
    }

    fn equippable(&self) -> Option<Equippable> {
        self.held.and_then(stack_component::<Equippable>)
    }

    /// Holds a dye of another colour than `colour`, a `DyeColor` id.
    fn dyes_over(&self, colour: u8) -> bool {
        self.held
            .and_then(stack_component::<Dye>)
            .is_some_and(|dye| dye.color as u8 != colour)
    }
}

impl Target<'_> {
    fn kind(&self) -> EntityKind {
        self.entity.entity_type
    }

    /// Vanilla `LivingEntity.isAlive`.
    fn alive(&self) -> bool {
        self.entity.health > 0.0
    }

    fn adult(&self) -> bool {
        !self.entity.is_baby
    }

    fn hurt(&self) -> bool {
        self.entity.health < self.entity.max_health
    }

    fn wears(&self, slot: EquipmentSlot) -> Option<&ItemStackData> {
        self.entity
            .equipment(slot)
            .and_then(|stack| stack.as_present())
    }

    /// Vanilla `canUseSlot` for the saddle and body slots.
    fn can_use_slot(&self, slot: EquipmentSlot) -> bool {
        let grown = self.alive() && self.adult();
        match (self.kind(), slot) {
            (EntityKind::Pig, EquipmentSlot::Saddle) => grown,
            (EntityKind::SulfurCube, EquipmentSlot::Body) => grown,
            (EntityKind::Donkey | EntityKind::Mule, EquipmentSlot::Saddle) => {
                grown && self.entity.is_tame
            }
            _ => true,
        }
    }

    /// Vanilla `LivingEntity.isEquippableInSlot`.
    fn equippable_in(&self, equippable: &Equippable, slot: EquipmentSlot) -> bool {
        equippable.slot == slot
            && self.can_use_slot(slot)
            && can_be_equipped_by(equippable, self.kind())
    }

    /// Vanilla `Mob.isSaddled` and `isWearingBodyArmor`.
    fn wears_fitting(&self, slot: EquipmentSlot) -> bool {
        self.wears(slot)
            .and_then(stack_component::<Equippable>)
            .is_some_and(|equippable| self.equippable_in(&equippable, slot))
    }
}

/// Vanilla `Player.interactOn`, short of its spectator check: the entity's
/// own `interact`, then the held item's `interactLivingEntity`.
pub fn interact_on(target: Target, clicker: Clicker) -> Option<Taken> {
    let kind = target.kind();
    if is_boat(kind) {
        return boat_interact(target, clicker);
    }
    let taken = if kind == EntityKind::Player {
        None
    } else {
        mob_interact(target, clicker)
    };
    taken.or_else(|| item_interact(target, clicker))
}

/// Vanilla `AbstractBoat.interact` and `AbstractChestBoat.interact`.
fn boat_interact(target: Target, clicker: Clicker) -> Option<Taken> {
    let sim = &target.entity.boat_sim;
    if let Some(taken) = leashable_interact(clicker) {
        return Some(taken);
    }
    if !clicker.sneaking && sim.in_control() {
        return SWING;
    }
    // Vanilla `canAddPassenger`: a chest boat seats one, out of water.
    let full = target.passengers >= 1 || sim.is_under_water();
    swing_if(is_chest_boat(target.kind()) && (full || clicker.sneaking))
}

/// Vanilla `Entity.interact`'s lead rule for a `Leashable` that is alive.
fn leashable_interact(clicker: Clicker) -> Option<Taken> {
    quiet_if(clicker.holds(ItemKind::Lead))
}

/// Vanilla `Mob.interact`.
fn mob_interact(target: Target, clicker: Clicker) -> Option<Taken> {
    if !target.alive() {
        return None;
    }
    // `checkAndHandleImportantInteractions`.
    if clicker.holds(ItemKind::NameTag)
        && let Some(taken) = item_interact(target, clicker)
    {
        return Some(taken);
    }
    if clicker
        .held
        .is_some_and(|stack| stack.kind.to_string().ends_with("_spawn_egg"))
    {
        return QUIET;
    }
    // `Entity.interact`; `Wolf.canShearEquipment` wants its owner.
    let owned = target.kind() != EntityKind::Wolf || target.entity.is_tame;
    if clicker.holds(ItemKind::Shears)
        && target.passengers == 0
        && owned
        && !clicker.sneaking
        && target.entity.equipment.values().any(|stack| {
            stack
                .as_present()
                .and_then(stack_component::<Equippable>)
                .is_some_and(|equippable| equippable.can_be_sheared)
        })
    {
        return SWING;
    }
    leashable_interact(clicker).or_else(|| kind_interact(target, clicker))
}

/// Vanilla `ItemStack.interactLivingEntity`: `Equippable.equipOnTarget`,
/// then `NameTagItem` and `DyeItem`.
fn item_interact(target: Target, clicker: Clicker) -> Option<Taken> {
    let held = clicker.held?;
    if let Some(equippable) = clicker.equippable().filter(|e| e.equip_on_interact)
        && target.equippable_in(&equippable, equippable.slot)
        && target.wears(equippable.slot).is_none()
        && target.alive()
    {
        return SWING;
    }
    match held.kind {
        // `EntityType.canSerialize`: a player can't be named.
        ItemKind::NameTag => swing_if(
            stack_component::<CustomName>(held).is_some() && target.kind() != EntityKind::Player,
        ),
        _ if target.kind() == EntityKind::Sheep => {
            let sheep = target.entity;
            swing_if(
                target.alive()
                    && !sheep.is_sheared
                    && clicker.dyes_over(sheep.wool_color.unwrap_or(0)),
            )
        }
        _ => None,
    }
}

/// Vanilla `AgeableMob.mobInteract`: a golden dandelion on a baby.
fn ageable_interact(target: Target, clicker: Clicker) -> Option<Taken> {
    let kind = target.kind();
    swing_if(
        clicker.holds(ItemKind::GoldenDandelion)
            && is_ageable_mob(kind)
            && target.entity.is_baby
            && !tags::entities::CANNOT_BE_AGE_LOCKED.contains(&kind),
    )
}

/// Vanilla `Animal.mobInteract`. Food always takes the click here; the
/// server passes for an adult that can't breed.
fn animal_interact(target: Target, clicker: Clicker, food: bool) -> Option<Taken> {
    if !food {
        return ageable_interact(target, clicker);
    }
    // `canAgeUp`, which the undead horses refuse.
    let ages_up = target.entity.is_baby
        && !target.entity.age_locked
        && !matches!(
            target.kind(),
            EntityKind::SkeletonHorse | EntityKind::ZombieHorse
        );
    if ages_up { SWING } else { QUIET }
}

/// Each mob's `mobInteract`.
fn kind_interact(target: Target, clicker: Clicker) -> Option<Taken> {
    use EntityKind as E;
    use ItemKind as I;
    let (t, c) = (target, clicker);
    let entity = t.entity;
    let animal = |tag| animal_interact(t, c, c.in_tag(tag));
    let milked = |food| swing_if(c.holds(I::Bucket) && t.adult()).or_else(|| animal(food));
    let carrying = t.wears(EquipmentSlot::Mainhand).is_some();
    match t.kind() {
        E::Chicken => animal("chicken_food"),
        E::Rabbit => animal("rabbit_food"),
        E::Fox => animal("fox_food"),
        E::Turtle => animal("turtle_food"),
        E::Sniffer => animal("sniffer_food"),
        E::PolarBear | E::Squid | E::GlowSquid => ageable_interact(t, c),
        E::Cow => milked("cow_food"),
        E::Goat => milked("goat_food"),
        E::Mooshroom => {
            // `SuspiciousEffectHolder.tryGet`: the flower blocks, which are
            // the small flowers.
            let stew = entity.variant == MOOSHROOM_BROWN
                && c.held
                    .is_some_and(|stack| tags::items::SMALL_FLOWERS.contains(&stack.kind));
            swing_if(t.adult() && (c.holds(I::Bowl) || c.holds(I::Shears) || stew))
                .or_else(|| milked("cow_food"))
        }
        // Shears are taken whether or not there is wool to cut.
        E::Sheep => quiet_if(c.holds(I::Shears)).or_else(|| animal("sheep_food")),
        E::Pig => {
            let food = c.in_tag("pig_food");
            let ride = !food && t.wears_fitting(EquipmentSlot::Saddle);
            swing_if(ride && t.passengers == 0 && !c.sneaking)
                .or_else(|| animal_interact(t, c, food))
        }
        E::Bee => {
            // `getBeeInteractionEffect`: the flowers that hurt a bee.
            let harmful = c.holds(I::WitherRose) || c.holds(I::OpenEyeblossom);
            let food = c.in_tag("bee_food");
            swing_if(food && harmful).or_else(|| animal_interact(t, c, food))
        }
        E::Armadillo => {
            if c.holds(I::Brush) && t.adult() {
                SWING
            } else if entity.armadillo.is_scared() {
                None
            } else {
                animal("armadillo_food")
            }
        }
        E::Axolotl => bucket_pickup(c).or_else(|| animal("axolotl_food")),
        E::Cod | E::Salmon | E::TropicalFish | E::Pufferfish => bucket_pickup(c),
        E::Dolphin => swing_if(c.in_tag("fishes")).or_else(|| ageable_interact(t, c)),
        E::Panda => panda_interact(t, c),
        E::Parrot => swing_if(
            (!entity.is_tame && c.in_tag("parrot_food"))
                || c.in_tag("parrot_poisonous_food")
                || (entity.on_ground && entity.is_tame),
        ),
        E::Ocelot => {
            let food = c.in_tag("ocelot_food");
            let near = c.position.distance_squared(entity.position.into()) < 9.0;
            swing_if(!entity.is_trusting && food && near).or_else(|| animal_interact(t, c, food))
        }
        E::Cat => cat_interact(t, c),
        E::Wolf => wolf_interact(t, c),
        E::Allay => {
            let duplicates =
                entity.allay.dancing && c.in_tag("duplicates_allays") && entity.can_duplicate;
            let gives = !carrying && c.held.is_some();
            let takes = carrying && c.hand == InteractionHand::MainHand && c.held.is_none();
            swing_if(duplicates || gives || takes)
        }
        E::IronGolem => swing_if(c.holds(I::IronIngot) && t.hurt()),
        E::SnowGolem => swing_if(c.holds(I::Shears) && entity.has_pumpkin),
        E::CopperGolem => {
            // The antenna flower rides in the saddle slot.
            let antenna = t
                .wears(EquipmentSlot::Saddle)
                .is_some_and(|flower| c.tagged(flower, "shearable_from_copper_golem"));
            swing_if((c.held.is_none() && carrying) || (c.holds(I::Shears) && antenna))
        }
        E::Creeper => swing_if(c.in_tag("creeper_igniters")),
        E::Bogged => swing_if(c.holds(I::Shears) && !entity.is_sheared),
        // Weakness isn't sent for another entity, so never the cure's swing.
        E::ZombieVillager => quiet_if(c.holds(I::GoldenApple)),
        // An unemployed villager shakes its head on the server; the client
        // takes the click all the same.
        E::Villager => swing_if(entity.sleeping_pos.is_none()),
        E::WanderingTrader => {
            if t.adult() {
                QUIET
            } else {
                ageable_interact(t, c)
            }
        }
        E::SulfurCube => sulfur_cube_interact(t, c),
        kind if is_equine(&kind) || is_llama(kind) => horse_interact(t, c),
        _ => None,
    }
}

/// `MushroomCow.Variant.BROWN`.
const MOOSHROOM_BROWN: u32 = 1;
/// `Panda.Gene.WORRIED`.
const PANDA_WORRIED: u32 = 2;

/// Vanilla `Bucketable.bucketMobPickup`.
fn bucket_pickup(clicker: Clicker) -> Option<Taken> {
    swing_if(clicker.holds(ItemKind::WaterBucket))
}

/// Vanilla `Panda.mobInteract`. An adult offered bamboo passes here though
/// the server takes it.
fn panda_interact(target: Target, clicker: Clicker) -> Option<Taken> {
    let entity = target.entity;
    if entity.panda.variant() == PANDA_WORRIED && clicker.thundering {
        return None;
    }
    if entity.panda.is_on_back() {
        return SWING;
    }
    if clicker.in_tag("panda_food") {
        return quiet_if(entity.is_baby && !entity.age_locked);
    }
    if target.adult() || !clicker.holding_golden_dandelion {
        return None;
    }
    ageable_interact(target, clicker)
}

/// Vanilla `Cat.mobInteract`.
fn cat_interact(target: Target, clicker: Clicker) -> Option<Taken> {
    let food = clicker.in_tag("cat_food");
    if !target.entity.is_tame {
        return swing_if(food).or_else(|| animal_interact(target, clicker, food));
    }
    let dye = clicker.in_tag("cat_collar_dyes");
    let recolours = dye && clicker.dyes_over(target.entity.collar_color);
    if recolours || (!dye && food && target.hurt()) {
        return SWING;
    }
    // Sits or stands when nothing else took the click.
    animal_interact(target, clicker, food).or(SWING)
}

/// Vanilla `Wolf.mobInteract`. A bone on a wild wolf passes here though the
/// server takes it.
fn wolf_interact(target: Target, clicker: Clicker) -> Option<Taken> {
    let entity = target.entity;
    let food = clicker.in_tag("wolf_food");
    if !entity.is_tame {
        return animal_interact(target, clicker, food);
    }
    if food && target.hurt() {
        return SWING;
    }
    if clicker.in_tag("wolf_collar_dyes") {
        return if clicker.dyes_over(entity.collar_color) {
            SWING
        } else {
            animal_interact(target, clicker, food)
        };
    }
    let armor = target.wears(EquipmentSlot::Body);
    let dresses = armor.is_none()
        && target.adult()
        && clicker
            .equippable()
            .is_some_and(|e| target.equippable_in(&e, EquipmentSlot::Body));
    let repairs = entity.is_sitting
        && armor
            .and_then(stack_component::<Damage>)
            .is_some_and(|damage| damage.amount > 0)
        && clicker.in_tag("repairs_wolf_armor");
    if dresses || repairs {
        return SWING;
    }
    animal_interact(target, clicker, food).or(SWING)
}

/// Vanilla `SulfurCube.mobInteract`. Its explosive form never lights here:
/// `explosionData` is only set on the server.
fn sulfur_cube_interact(target: Target, clicker: Clicker) -> Option<Taken> {
    use ItemKind as I;
    let entity = target.entity;
    if entity.is_baby {
        return swing_if(clicker.in_tag("sulfur_cube_food") && !entity.age_locked)
            .or_else(|| ageable_interact(target, clicker));
    }
    if entity.sulfur_fuse >= 0 {
        return None;
    }
    let body = target.wears(EquipmentSlot::Body);
    if clicker.holds(I::Shears) && body.is_some() {
        return SWING;
    }
    if clicker.in_tag("sulfur_cube_swallowable") {
        let swallowed = body.is_some_and(|body| clicker.holds(body.kind));
        return quiet_if(!swallowed);
    }
    swing_if(clicker.holds(I::Bucket))
}

/// Vanilla `Horse`, `AbstractChestedHorse`, `SkeletonHorse` and
/// `ZombieHorse.mobInteract`, each ending in `AbstractHorse`'s.
fn horse_interact(target: Target, clicker: Clicker) -> Option<Taken> {
    use EntityKind as E;
    let entity = target.entity;
    let kind = target.kind();
    let tame = entity.is_tame;
    let food = clicker.in_tag(match kind {
        E::Llama | E::TraderLlama => "llama_food",
        E::ZombieHorse => "zombie_horse_food",
        _ => "horse_food",
    });
    if kind == E::SkeletonHorse {
        if !tame {
            return None;
        }
    } else {
        let dandelion =
            kind != E::ZombieHorse && entity.is_baby && clicker.holding_golden_dandelion;
        let to_base =
            target.passengers > 0 || (target.adult() && tame && clicker.sneaking) || dandelion;
        if !to_base && clicker.held.is_some() {
            // `fedFood` always takes the click on the client.
            if food {
                return QUIET;
            }
            let chest = !matches!(kind, E::Horse | E::ZombieHorse)
                && !entity.has_chest
                && clicker.holds(ItemKind::Chest);
            if !tame || chest {
                return SWING;
            }
        }
    }
    // `AbstractHorse.mobInteract`: ridden or a baby is an animal; any other
    // click opens the inventory, equips or mounts.
    if target.passengers > 0 || entity.is_baby {
        animal_interact(target, clicker, food)
    } else {
        SWING
    }
}

#[cfg(test)]
mod tests {
    use pomme_registry::builtin::{EntityKind as E, ItemKind as I};

    use super::*;
    use crate::entity::components::{LookDirection, Position};

    fn entity(kind: EntityKind) -> LivingEntity {
        let mut entity = LivingEntity::new(
            kind,
            Position::default(),
            LookDirection::default(),
            uuid::Uuid::nil(),
        );
        entity.health = 10.0;
        entity.max_health = 10.0;
        entity
    }

    /// The main hand's click on `entity` holding `item`, not sneaking, on a
    /// server that synced no tags.
    fn click(entity: &LivingEntity, item: Option<ItemKind>) -> Option<Taken> {
        click_as(entity, item, &ItemTags::default(), false)
    }

    fn click_as(
        entity: &LivingEntity,
        item: Option<ItemKind>,
        item_tags: &ItemTags,
        sneaking: bool,
    ) -> Option<Taken> {
        let held = item.map(|item| ItemStackData::new(item, 1));
        interact_on(
            Target {
                entity,
                passengers: 0,
            },
            Clicker {
                held: held.as_ref(),
                hand: InteractionHand::MainHand,
                item_tags,
                sneaking,
                holding_golden_dandelion: false,
                position: DVec3::ZERO,
                thundering: false,
            },
        )
    }

    #[test]
    fn a_tag_the_server_synced_replaces_the_built_in_one() {
        let pig = entity(E::Pig);
        let synced = ItemTags::from_entries([("minecraft:pig_food".into(), vec![I::Bread as u32])]);
        assert_eq!(click_as(&pig, Some(I::Bread), &synced, false), QUIET);
        assert_eq!(click_as(&pig, Some(I::Carrot), &synced, false), None);
    }

    #[test]
    fn hostile_mobs_and_players_pass_so_a_shield_can_rise() {
        for kind in [
            E::Zombie,
            E::Skeleton,
            E::Spider,
            E::Enderman,
            E::Witch,
            E::Player,
        ] {
            for item in [None, Some(I::Shield), Some(I::IronSword), Some(I::Bread)] {
                assert_eq!(click(&entity(kind), item), None, "{kind:?} {item:?}");
            }
        }
    }

    #[test]
    fn a_villager_takes_any_click_unless_asleep() {
        let mut villager = entity(E::Villager);
        assert_eq!(click(&villager, None), SWING);
        assert_eq!(click(&villager, Some(I::Shield)), SWING);
        villager.is_baby = true;
        assert_eq!(click(&villager, None), SWING);
        villager.sleeping_pos = Some(crate::position::BlockPos::new(0, 0, 0));
        assert_eq!(click(&villager, None), None);
        assert_eq!(click(&entity(E::WanderingTrader), None), QUIET);
    }

    #[test]
    fn an_animal_takes_only_its_food() {
        let mut pig = entity(E::Pig);
        assert_eq!(click(&pig, None), None);
        assert_eq!(click(&pig, Some(I::Bread)), None);
        // An adult's food is taken without a swing; a baby eats with one.
        assert_eq!(click(&pig, Some(I::Carrot)), QUIET);
        pig.is_baby = true;
        assert_eq!(click(&pig, Some(I::Carrot)), SWING);
        assert_eq!(click(&pig, Some(I::GoldenDandelion)), SWING);
        pig.age_locked = true;
        assert_eq!(click(&pig, Some(I::Carrot)), QUIET);
        // Not an ageable mob, whatever its baby flag says.
        let mut zombie = entity(E::Zombie);
        zombie.is_baby = true;
        assert_eq!(click(&zombie, Some(I::GoldenDandelion)), None);
    }

    #[test]
    fn tools_are_taken_by_the_mobs_they_work_on() {
        let cow = entity(E::Cow);
        assert_eq!(click(&cow, Some(I::Bucket)), SWING);
        assert_eq!(click(&cow, Some(I::Wheat)), QUIET);
        assert_eq!(click(&entity(E::Sheep), Some(I::Shears)), QUIET);
        assert_eq!(click(&entity(E::Cod), Some(I::WaterBucket)), SWING);
        assert_eq!(click(&entity(E::Creeper), Some(I::FlintAndSteel)), SWING);
        assert_eq!(click(&entity(E::Creeper), Some(I::Shield)), None);
        let mut golem = entity(E::IronGolem);
        assert_eq!(click(&golem, Some(I::IronIngot)), None);
        golem.health = 5.0;
        assert_eq!(click(&golem, Some(I::IronIngot)), SWING);
        let mut bogged = entity(E::Bogged);
        assert_eq!(click(&bogged, Some(I::Shears)), SWING);
        bogged.is_sheared = true;
        assert_eq!(click(&bogged, Some(I::Shears)), None);
    }

    #[test]
    fn a_lead_or_spawn_egg_is_taken_by_any_living_mob() {
        let mut zombie = entity(E::Zombie);
        assert_eq!(click(&zombie, Some(I::Lead)), QUIET);
        assert_eq!(click(&zombie, Some(I::ZombieSpawnEgg)), QUIET);
        // An unnamed name tag does nothing.
        assert_eq!(click(&zombie, Some(I::NameTag)), None);
        zombie.health = 0.0;
        assert_eq!(click(&zombie, Some(I::Lead)), None);
        assert_eq!(click(&entity(E::Player), Some(I::Lead)), None);
    }

    #[test]
    fn a_tame_pet_sits_when_nothing_else_takes_the_click() {
        for kind in [E::Wolf, E::Cat] {
            let mut pet = entity(kind);
            assert_eq!(click(&pet, None), None, "{kind:?}");
            pet.is_tame = true;
            assert_eq!(click(&pet, None), SWING, "{kind:?}");
            assert_eq!(click(&pet, Some(I::Shield)), SWING, "{kind:?}");
        }
        let mut wolf = entity(E::Wolf);
        assert_eq!(click(&wolf, Some(I::Bone)), None);
        wolf.is_tame = true;
        wolf.health = 5.0;
        assert_eq!(click(&wolf, Some(I::Beef)), SWING);
    }

    #[test]
    fn a_horse_takes_every_click_unless_ridden_or_a_baby() {
        let mut horse = entity(E::Horse);
        assert_eq!(click(&horse, None), SWING);
        assert_eq!(click(&horse, Some(I::Shield)), SWING);
        assert_eq!(click(&horse, Some(I::Wheat)), QUIET);
        horse.is_baby = true;
        assert_eq!(click(&horse, None), None);
        // A wild skeleton horse ignores the player.
        let mut skeleton = entity(E::SkeletonHorse);
        assert_eq!(click(&skeleton, None), None);
        skeleton.is_tame = true;
        assert_eq!(click(&skeleton, None), SWING);
    }

    #[test]
    fn a_boat_is_boarded_unless_sneaking() {
        let sneak = |boat| click_as(boat, None, &ItemTags::default(), true);
        let boat = entity(E::OakBoat);
        assert_eq!(click(&boat, None), SWING);
        assert_eq!(sneak(&boat), None);
        assert_eq!(click(&boat, Some(I::Lead)), QUIET);
        assert_eq!(sneak(&entity(E::OakChestBoat)), SWING);
    }

    #[test]
    fn a_dye_is_taken_by_a_sheep_of_another_colour() {
        let mut sheep = entity(E::Sheep);
        sheep.wool_color = Some(0);
        assert_eq!(click(&sheep, Some(I::WhiteDye)), None);
        assert_eq!(click(&sheep, Some(I::RedDye)), SWING);
        sheep.is_sheared = true;
        assert_eq!(click(&sheep, Some(I::RedDye)), None);
        assert_eq!(click(&entity(E::Cow), Some(I::RedDye)), None);
    }
}
