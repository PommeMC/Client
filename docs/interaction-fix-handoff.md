# Interaction fix handoff (draft, not merge-ready)

PR #900 head `2c8c8e7719ad0f1715ebe043beaeef71ca8a2f80` contains cooldown
support and regression fixtures, **not the complete interaction-result fix**. Twenty
igniter regressions last ran RED; they were not rerun after cooldown implementation.
No full-library GREEN result or manual gameplay validation is claimed.
The user has authorized continuation beyond the earlier pause, with a minimal but
complete interaction model, not an igniter-only workaround. This continuation
records further static research and a proposal only; no source changes, builds,
or tests were run during this documentation-only continuation.

## Scope and definition of complete

The target is the **entire native 26.2 client right-click/use model**: block,
item and entity dispatch; targeting and use gates; ongoing use, release and finish;
and the local state required for subsequent interactions to produce native results.
Igniters are an example, not the scope. This is not full server simulation or an
unrelated mining, attack, renderer or UI redesign. Native **26.2/protocol 776** is
normative; do not import 26.3 `useOn` after-use wrappers or `block_transformer`.
Wire translation to another server version does not establish semantic equivalence.
Complete means all registered/inherited/component-driven interaction paths are
accounted for, not merely that the original candle reproduction stops failing.

## Problem and diagnosis

Observed against a Vanilla 26.3 server: flint and steel in the main hand with an
empty offhand leaves a clicked candle unlit; putting a stick in the offhand works.
The diagnosis is server lighting followed by empty-offhand extinguishing, not a
particle rendering failure. Pomme lacks the shared held-item `Item.useOn` result
at the block-use stage, so it incorrectly falls through to generic main-hand use
and then offhand block use. Other item classes can have the same missing-result gap.

Vanilla computes the result synchronously, with different rules at the outer hand
loop and inner block callbacks. The verified native contract below is authoritative
for this investigation; the nearby 26.3 source is only a versioned comparator.

Do **not** globally skip an empty offhand. A stick in the main hand legitimately
passes, allowing empty-offhand use (including extinguishing a lit candle).
The passing control retains main `UseItemOn(1)`, main `UseItem(2)`, offhand
`UseItemOn(3)`. Igniter fixtures currently produce that same incorrect trace.
Historical direct-light/supported-fire fixtures expect main `UseItemOn(1)` plus main
swing; negative/unknown fixtures expect only main `UseItemOn(1)`, without local
mutation. Those expectations do not establish unknown-as-terminal as a complete fix.

## Verified native 26.2 interaction contract

### Reproducible provenance

This is static inspection of the official **26.2** client (native protocol 776),
not a runtime parity test or a claim about every item implementation. The client
SHA-1 was recomputed as `2dc72797acbc1b63fc16a11c4ac393605f453754` and matches
`downloads.client.sha1` in the cached official metadata:

- [Official 26.2 metadata](https://piston-meta.mojang.com/v1/packages/09165f570b16394790c4e37c93b0918729615d17/26.2.json).
- [Official client JAR](https://piston-data.mojang.com/v1/objects/2dc72797acbc1b63fc16a11c4ac393605f453754/client.jar).

To reproduce, obtain that JAR as `client-26.2.jar`, verify its hash, then inspect
the named classes with a compatible JDK's `javap`. Names are unobfuscated; no
local decompiler output or temporary investigation report is required:

```sh
sha1sum client-26.2.jar
javap -classpath client-26.2.jar -c -p net.minecraft.client.Minecraft
javap -classpath client-26.2.jar -c -p net.minecraft.client.multiplayer.MultiPlayerGameMode
javap -classpath client-26.2.jar -c -p net.minecraft.world.InteractionResult 'net.minecraft.world.InteractionResult$Success'
javap -classpath client-26.2.jar -c -p net.minecraft.world.item.ItemStack
javap -classpath client-26.2.jar -c -p net.minecraft.network.protocol.game.ClientboundBlockChangedAckPacket
```

All offsets below are method-local JVM bytecode offsets, not source line numbers.
Use the same command for the additional fully qualified classes identified below.

### Result layers, eligibility, and packets

- **Outer dispatch:** `Minecraft.startUseItem` iterates `InteractionHand.values()`
  (45–62, 438–444); `net.minecraft.world.InteractionHand.$values` orders MAIN_HAND
  before OFF_HAND. Destroying/busy-hands guards precede dispatch; a disabled held
  item's feature gate aborts the click (75–90).
- **Final block result:** after `gameMode.useItemOn`, any `Success` stops the click
  (282–357), swinging only when its swing source is `CLIENT`. `Fail` stops without
  swing (358–366). `Pass` permits generic use for a nonempty held stack, then the
  next hand if generic use is non-Success. Generic `Fail` is **not terminal**
  (367–437): only generic `Success` stops, again with a swing only for `CLIENT`.
- **Entity result:** `startUseItem` calls the single `MultiPlayerGameMode.interact`
  pipeline; only `Success` stops (191–245). Non-Success, including `Fail`, proceeds
  to generic use. Do not recreate an older interactAt-then-interact pair. `interact`
  sends a relative-hit `ServerboundInteractPacket`, then returns spectator PASS or
  `player.interactOn` (0–74). Entity border rejection aborts before this send;
  out-of-range entities instead fall through to generic use (156–188).
- **Inner block pipeline:** `MultiPlayerGameMode.performUseItemOn` returns
  `CONSUME` for spectators (13–26). Otherwise secondary use (sneaking) suppresses
  block callbacks only when **either** hand holds an item (27–75). In the
  unsuppressed path, disabled block features return FAIL (92–113), then
  `blockState.useItemOn` returns early only for `consumesAction()` (114–148).
  `TryEmptyHandInteraction` invokes `useWithoutItem` only for MAIN_HAND, and that
  callback also stops only for a consuming result (149–192). Inner callback FAIL
  is therefore **not** final block FAIL: it continues toward held-item use.
- **Consumption is not swing:** `InteractionResult.consumesAction` defaults to
  false; only `InteractionResult$Success` overrides it to true. SUCCESS requests
  CLIENT swing, SUCCESS_SERVER requests SERVER, and CONSUME requests NONE. All
  three are `Success`; success does not always mean a local swing.
- **Held-item gates:** block callbacks precede the empty-stack/cooldown PASS gate
  (193–216), then `ItemStack.useOn` is called (217–273), restoring count for
  infinite materials. `net.minecraft.world.item.ItemStack.useOn` (10–47) returns
  PASS before `Item.useOn` when a player without building permission fails the
  Adventure `CanPlaceOn` gate. Thus an igniter's own FAIL/SUCCESS is not reached
  on every click merely because that item is held.
- **Packet order:** `MultiPlayerGameMode.useItemOn` synchronizes the carried slot,
  then rejects an out-of-border block before prediction or a use-on packet (0–27).
  Inside prediction, `lambda$useItemOn$0` computes `performUseItemOn` before
  constructing the normal `ServerboundUseItemOnPacket` (0–24), regardless of its
  result. Generic `lambda$useItem$0` constructs `ServerboundUseItemPacket` **before**
  checking cooldown (0–48); cooldown supplies PASS without calling `ItemStack.use`,
  but still returns that packet for sending. `startPrediction` invokes the
  prediction callback then sends its returned packet (14–30). Spectator generic
  use returns PASS before prediction. A local FAIL is not a blanket reason to
  suppress a packet.
- **Acknowledgement:** native `ClientboundBlockChangedAckPacket` has only an integer
  sequence payload: its buffer constructor reads one VarInt and `write` writes
  one VarInt (both 0–9 or shorter). It contains no SUCCESS/PASS/FAIL result;
  waiting for this acknowledgement cannot supply the missing synchronous result.

### Igniters illustrate the contract; they do not bound its scope

`net.minecraft.world.item.FlintAndSteelItem.useOn` and `FireChargeItem.useOn`
return SUCCESS for direct lighting or permitted adjacent fire placement, otherwise
FAIL. Direct lighting uses `canLight` in `net.minecraft.world.level.block`
`CandleBlock`, `CampfireBlock`, and `CandleCakeBlock`: eligible tagged candles and
campfires must have LIT/WATERLOGGED properties and be unlit and not waterlogged;
eligible tagged candle cakes must have LIT and be unlit. Otherwise the destination
is the clicked position offset by the hit face.
`BaseFireBlock.canBePlacedAt` requires that destination to be air **and** either
the selected fire state can survive or `BaseFireBlock.isPortal` succeeds. Ordinary
`FireBlock.canSurvive` accepts a sturdy upper face below or a flammable neighbor;
`BaseFireBlock.getState`/`SoulFireBlock.canSurviveOnBlock` select soul fire on
`SOUL_FIRE_BASE_BLOCKS`. The portal alternative uses dimension, neighboring
obsidian, axis, and `PortalShape.findEmptyPortalShape` checks, not just air/support.

`CandleBlock.useItemOn` (0–46), **not** `useWithoutItem`, extinguishes a lit candle
for an empty stack when the player may build, returning SUCCESS. This explains
why erroneous offhand fallback can undo lighting and why globally suppressing an
empty offhand would also remove legitimate behavior.

Native 26.2 `ItemStack.useOn` does not perform 26.3's after-use component processing,
and native `performUseItemOn` does not replace the held stack from a transformed
success result. Generic `useItem` already handles transformed success stacks;
that separate path must not be confused with block use. No 26.3 after-use or
`block_transformer` feature is claimed as native 26.2 behavior here.

### Shared lifecycle and client predicates: additionally verified

These findings come from the same official JAR, using `javap -c -p`; class names
below are under `net.minecraft` (`world.item`, `world.item.component`,
`world.entity`, `world.entity.player`, `world.food`, `world.level`, or
`client.player` as appropriate). They are static branch evidence, not new tests.

- `ItemStack.use` copies the pre-use stack and tests its original duration:
  shared after-use processing runs only for duration **<= 0 and Success**
  (0–87), using the transformed success stack when present. `finishUsingItem`
  always processes the returned stack (0–24); `releaseUsing` does so only when
  `Item.releaseUsing` returns true (14–42). There is **no such hook in useOn**.
- `ItemStack.applyAfterUseComponentSideEffects` reads components and original
  count from that pre-use copy. `UseRemainder.convertIntoRemainder` requires
  !infinite-materials and resulting count < original count; it creates one
  template stack, replacing an empty result or invoking the extra-item callback
  otherwise. It is not one remainder per item lost. `UseCooldown.apply` is
  independent of count reduction and has no server-side-only guard for players.
- `UseCooldown.ticks` and `Consumable.consumeTicks` multiply float seconds by
  20 then truncate toward zero (`f2i`). `ItemCooldowns.isOnCooldown` tests the
  clamped percentage > 0: a negative duration is immediately active until the
  next cooldown tick; zero produces NaN, whose > 0 test is false. `tick` first
  increments the tick counter, then removes entries with end <= current tick.
- `Consumable.canConsume` reads FOOD/canAlwaysEat and `Player.canEat`; the latter
  tests **abilities.invulnerable**, not instabuild or creative mode. Positive
  duration starts use and returns CONSUME; <= 0 calls `onConsume` immediately
  and returns transformed CONSUME. `FoodProperties.onConsume` applies nutrition
  locally through the listener before the server-only explicit consume-effect
  list; stack consumption is also local unless infinite materials.
- Default `Item.use` precedence is CONSUMABLE, swappable EQUIPPABLE,
  BLOCKS_ATTACKS, KINETIC_WEAPON, then PASS. The last two start ongoing use with
  CONSUME and duration 72000 (`Item.getUseDuration`), not just an animation result.
- `LivingEntity.updatingUsingItem` correctly compares **item kind** (`isSameItem`),
  adopts the current hand stack and preserves remaining ticks; component equality
  is not required. `updateUsingItem` runs the current stack's hook then decrements;
  automatic zero-timer finish is server-only. Client `Player.handleEntityEvent(9)`
  calls `completeUsingItem`/finish and hand replacement. `LocalPlayer.startUsingItem`
  maintains `startedUsingItem` and `usingItemHand` separately from synced flags;
  `onSyncedDataUpdated` reconciles starts/stops, not just a single shared flag.
- `KnowledgeBookItem.use` consumes and checks RECIPES emptiness before its server
  guard (23–48): dropping recipes can change SUCCESS to FAIL. The client needs
  nonemptiness, not server recipe validation. `SpawnEggItem.getType` reads
  ENTITY_DATA's **type** on the client; `useOn`/`use` check type/canSpawn before
  the ServerLevel guard, but liquid-target permission checks in `use` are beyond
  that guard. Do not restore the entire entity payload by default for this path.
- `Level.mayInteract` defaults true on the native client, not a game-mode test.
  mayBuild, instabuild, invulnerable and privileged-command permission are distinct;
  explicit server ability values must override simple game-mode inference.

### Breadth inventory and proof limits

Static inspection of that official JAR covered **10,372 classes**: 204 requested
method declarations in 171 declaring classes, including roots; **193 actual
overrides = 61 item + 77 block + 55 entity**. Another **389 selected dependent
hook declarations** cover placement/survival/replacement/growth/fluid/use helpers;
this is not a full transitive-call proof. The **41 organizational families** below
are 17 item + 15 block + 9 entity, not a minimal implementation count.

| Domain | Durable family coverage (each semicolon separates a family) |
| --- | --- |
| Item 1–6 | Component defaults; block-placement adapters; entity placement/collision; spawn eggs/spawners/liquid targeting; tools/transformation/scraping/waxing/shearing; ignition |
| Item 7–12 | Fertilization/aquatic growth; buckets/bottles/potion block use; thrown projectiles/potions; charged weapons/release; spyglass/instrument use; brushing/bundle ticks |
| Item 13–17 | Fireworks/fishing/ridden-mob boost; maps/lodestone/portal eyes; entity dye/name/leash; books/recipe learning/GUIs; debug-stick editing |
| Block 1–5 | Defaults; containers/access gates; workstation/configuration menus; toggles/redstone/bells/notes/fence leashing; food/harvest/compost |
| Block 6–10 | Block-entity insertion/extraction/display; sign text/applicators/commands; cauldron callback tables; lighting/extinguishing/cooking/TNT; honey harvest/pumpkin carving |
| Block 11–15 | Sleep/spawn/anchor charge/dimension explosion; copper-statue wax/oxidation; vault unlock; special world-state interactions (dragon egg/light/redstone ore/moving piston); privileged command/structure/test GUIs |
| Entity 1–5 | Common player/entity/mob dispatch; age/breeding/feeding/species behavior; bucket capture/milking; taming/healing/sitting/dye/ownership; mount/feed/equipment/container variants |
| Entity 6–9 | Item exchange/shearing/wax/repair (including SulfurCube/CopperGolem); trade/barter/cure/ignite; hit-location decorations/equipment/leash knot/response entity; vehicle boarding/container/fuel/command |

Inheritance, arbitrary component patches and callback interfaces are mandatory
coverage, not exceptions: `ShieldItem` inherits component-based `Item.use`;
`DoubleHighBlockItem` inherits `BlockItem.useOn`; `MobBucketItem` inherits bucket
use with extra-content hooks. `SignBlock.useItemOn` invokes `SignApplicator`
(dye/glow ink/honeycomb/ink); `AbstractCauldronBlock.useItemOn` invokes
`CauldronInteraction` dispatcher tables. Also follow `BonemealableBlock`,
`BucketPickup`, `LiquidBlockContainer`/`SimpleWaterloggedBlock`, placement hooks,
and `Bucketable.bucketMobPickup` (an Optional result, not a direct override).

Audited common flows are the dispatch/result/packet contract, shared lifecycle and
predicates above, plus selected entry bodies: `AxeItem.useOn` and
`DoorBlock.useWithoutItem` mutate local state; hoe/shovel block mutations are
server-guarded; `BlockItem.place`, bucket/boat/spawn-egg entry gates, sign/cauldron
delegation and `Player.interactOn`/`Mob.interact`/`Entity.interact` expose further
branches. This does **not** certify every predicate in those families. All other
family branches and uninspected transitive helpers remain inventoried/pending proof.
The inventory selects a boundary, not a branch-complete implementation estimate.

Optional ephemeral audit aids: `/tmp/opencode/pomme-pr900-vanilla-26.2/`
`NATIVE-INTERACTION-INVENTORY.md`, declaration/inheritance/family/reference TSVs,
`INTERACTION-CONTRACT.md`, `interaction-evidence/`, and
`/tmp/opencode/native262-lifecycle-proof/*.txt`. The durable facts, source methods
and official-JAR provenance above suffice to continue without these local files;
their counts must not be mistaken for runtime validation.

## What is present

- `Interaction-Fix: cooldown-support`: owned Identifier + signed VarInt decoding,
  ordered raw-player event delivery, and application to interaction state.
- Legacy numeric cooldown item IDs are remapped through the item-stack registry
  aliases, then emitted as native group names. Modern groups remain opaque;
  unknown legacy items are dropped. `net/translate/mod.rs` only removes the obsolete
  documented numeric-Azalea cooldown divergence; it does not remove a decoder.
- Server-originated timers use player ticks (live/dead), expire/replace by group,
  remove on zero, and reset with fresh player state. Effective `UseCooldown` groups
  respect defaults, overrides, namespaces, and component-removal tombstones.
- Generic **all-item** cooldown gates cover placement/food/equip/bundle prediction,
  not just igniters. Block/menu interaction comes first; generic `UseItem` is still
  sent before returning PASS, preserving legitimate same/offhand fallback.
- Negative durations are stored but immediately inactive, unlike the now-confirmed
  **native 26.2** negative/negative percentage before its next tick (also observed
  in the prior 26.3 comparison). This remains a disclosed parity gap, not a native
  rule. No client-local cooldown creation, UI, wall-clock timer, or full
  client-local cooldown parity is implemented.
- `Interaction-Fix: igniter-hand-fallback`: twenty failing packet-trace regressions
  and exact native-state fixtures only; the production igniter TODO remains.
- Audio and atlas changes are small `cfg(test)` headless constructors, not unrelated
  production changes. `interaction_cooldown_tests.rs` is the new tracker/gate module.
  Fixtures construct a particle store only to satisfy the existing interaction API;
  they do not test particle behavior. SteelMC was initialized only in this worktree.

## Recorded verification (historical; not rerun for this continuation)

At the earlier handoff, saved command/exit/output records were inspected and all twelve Rust-file
SHA-256 hashes matched the cooldown producer's manifest. Logs are author-local,
ephemeral evidence, **not a dependency for reading or continuing this PR**.
The earlier test-first baseline compiled and produced 29 runtime failures:
20 igniter, 4 protocol cooldown, 4 raw cooldown, 1 legacy translation; 7 controls
passed (5 protocol, authoritative hotbar selection, genuine PASS).

The following are the exact Cargo command forms and filters from recorded runs.
For every client command, the recorded prefix was
`source /home/cliffh/dev/vulkan/1.4.363.0/setup-env.sh --set-dep-ld &&`.
They used pinned `nightly-2026-08-21`, default features and a worktree-local target.
These are historical results, not instructions executed during packaging.

| Recorded command | Result |
| --- | --- |
| `cargo test --locked -p pomme-protocol --lib player::tests:: -- --nocapture` | 9 passed |
| `cargo test --locked -p pomme-client --lib net::handler::tests::raw_native_cooldown_ -- --nocapture` | 5 passed |
| `cargo test --locked -p pomme-client --lib net::azalea_compat::translate_cooldown_ -- --nocapture` | 4 passed |
| `cargo test --locked -p pomme-client --lib player::interaction::cooldown_tests:: -- --nocapture` | 6 passed |
| `cargo test --locked -p pomme-client --lib player::interaction::tests::genuine_pass_stick_allows_empty_offhand_use_on -- --exact --nocapture` | 1 passed |
| `cargo test --locked -p pomme-client --lib player::interaction::tests::igniter_ -- --nocapture` | Earlier 20 failed; not rerun after cooldown |

The translation run initially had 3 passes/1 fixture failure (`translation data`);
the native-session fixture was corrected to bypass translation, then all 4 passed.
Recorded checks also exited 0: `cargo check --locked -p pomme-protocol --lib`,
`cargo check --locked -p pomme-client --lib`, `cargo check --locked -p pomme-client`,
`git diff --check`, and changed-file
`rustfmt +nightly-2026-08-21 --check --edition 2024 --config skip_children=true`
over the twelve Rust paths listed in the scope below. No broad suite was run.

Exact remaining RED names under `player::interaction::tests::`:

```text
igniter_flint_and_steel_candle_stops_hand_fallback
igniter_fire_charge_candle_stops_hand_fallback
igniter_flint_and_steel_candle_cake_stops_hand_fallback
igniter_fire_charge_candle_cake_stops_hand_fallback
igniter_flint_and_steel_campfire_stops_hand_fallback
igniter_fire_charge_campfire_stops_hand_fallback
igniter_flint_lit_candle_occupied
igniter_charge_lit_candle_occupied
igniter_flint_wet_candle_occupied
igniter_charge_wet_candle_occupied
igniter_flint_lit_campfire_occupied
igniter_charge_lit_campfire_occupied
igniter_flint_wet_campfire_occupied
igniter_charge_wet_campfire_occupied
igniter_flint_lit_cake_occupied
igniter_charge_lit_cake_occupied
igniter_flint_unsupported_fire_stops_fallback
igniter_charge_unsupported_fire_stops_fallback
igniter_flint_supported_fire_stops_fallback
igniter_charge_supported_fire_stops_fallback
```

## Current Pomme gaps and required seams

Anchors refer to PR #900's current source, not the older baseline links below:
`I` = `pomme-client/src/player/interaction.rs`, `C` = `pomme-client/src/app/core.rs`.

| Seam / evidence | Required correction or investigation |
| --- | --- |
| Stage results: I:101,795,877 | Replace swing-bool-only payload with native Success swingSource + ItemContext/transformed stack. Preserve inner block continuation, final block FAIL versus nonterminal generic/entity non-Success. |
| Block callbacks: I:912,1685 | Menu-rendering whitelist is not native callback semantics; model useItemOn and main-hand-only TryEmptyHand/useWithoutItem, including access predicates and local writes independently of GUI support. |
| Held useOn / placement: I:921,936,1256; C:3863 | TODO falls through; void predict_place can return early yet caller reports Success. `world/block/block_state_table.rs:155` maps only same-named single-state blocks, not native placement adapters, validity or multistate selection. |
| Entity chain: I:829 | Unconditional swing/Success omits Player → Entity/Mob → item-on-living-entity chain/context (including EQUIPPABLE before item callback). Owner references are skipped at `pomme-protocol/src/entity_data.rs:133–134`, optional unsigned references at 142–143; typed fallback drops unsupported values (`net/entity_data.rs:256`), and `pomme-protocol/src/entity.rs` has no leash-link packet variant. Restore only required owner/leash/equipment/tag state. |
| Generic components: I:975–1020,1628,1909 | Partial consumable-first handling plus item-name successes is not override/inherited dispatch. Weapons report success without active use; equipment without swap/writeback. stack_component resurrects removed defaults; cooldown's dedicated resolver already handles tombstones correctly. |
| Target/use gates: I:795,903; C:3840; `player/mod.rs:271`, `net/player_wire.rs:84` | Inside-shape flag is hardcoded false; border/features are not retained for these gates. Pass explicit game mode and abilities, not just creative; retain needed border/feature state and distinguish mayBuild/instabuild/invulnerable/privileged permissions. |
| Lifecycle/writeback: C:3831–3912; I:1036–1242 | Cloned read-only hands cannot commit transformed hands/equipment. Release/finish omit native hooks and local food/stack updates. Same-kind continuity is **right**; adopt current stack data without resetting remaining ticks, and separate local start/hand state from synced flags. |
| Cooldowns/remainders: I:918,966,1893 | Server tracker and gates are PRESENT, not missing. Shared local USE_COOLDOWN/USE_REMAINDER execution is missing; negative duration mismatch applies to native 26.2 too. `net/item_codec.rs:186,257` already transcodes/retains UseRemainder, but interaction never applies it. |
| Lost predicate inputs: `net/item_codec.rs:31–47,173–221` | CanPlaceOn and RECIPES are explicitly dropped; bridge decode/exact-span rejection also needs review for native ENTITY_DATA type fidelity. Retain Adventure predicates, KnowledgeBook nonempty recipes, SpawnEgg type, and equipment/tag/owner inputs only where native client paths require them—not wholesale server payloads or server-only validation. |

## Minimal-complete proposal and workplan (not implemented)

The preserved particle prototype alone has Pomme-specific `NonPassingUnpredicted`;
it is **not in this standalone branch**. The earlier provisional proposal treated
an eligible but unknown igniter outcome as terminal without a swing. That can
avoid extra packets but misses the required swing when the result was SUCCESS.
The user rejected both igniter-only scope and unknown-as-final-fix: this is **not
complete parity or an accepted final design**. Unknown behavior must not be mapped
to PASS, FAIL, SUCCESS, or stop-without-swing and presented as a complete fix.

The reviewed proposal preserves the controller, prediction ledger, packet ordering
and authoritative slot updates. Use small **private, interaction-only** grouped
helpers/modules for item, block, entity and lifecycle behavior; file count is
flexible, not a prescribed universal framework. Carry native result payloads;
dispatch actual behavior/inherited mappings with the component-based default
branch. Borrow only required existing client state, read fresh hands at each
native seam, and write back hand/equipment/local-state changes. Restore targeted
lost inputs; keep native-local callback effects distinct from server-guarded work.
Use existing Azalea defaults/tags and selectively verified pure mappings, not
arbitrary item-name success lists. All 41 families must close before completion.

Do not import Steel-core: optional singleplayer's 26.3 server contexts require
concrete `Arc<Player>`, `Arc<World>` and `InventoryAccess`
(`third_party/SteelMC/steel-core/src/behavior/context.rs:628–697`), and its
`consumes_action` includes Fail (43–54), unlike native client results. It is not
a mobile/feature-disabled client dependency solution and has incomplete behaviors
(e.g. spawn-egg spawner TODO). Pure mappings still need native-version verification.

One proof/work sequence, with its ledger kept as tables in this document:

1. **Tests first:** establish the durable behavior ledger and reproducing tests
   before fixes. For each branch record source class/method, client/server guards,
   required inputs, exact result payload, packets, local writes and test coverage.
   Expand the family rows above through registered inheritance, arbitrary component
   additions/removals/combinations and delegated callbacks; no unknown-as-result rows.
2. Close shared predicates/data gaps, including effective components, abilities,
   Adventure/border/features, hit context, and only the required retained state.
3. Implement shared dispatch/result/component/lifecycle state seams using fresh
   hand read/writeback and existing prediction/correction mechanisms.
4. Close every item/block/entity/lifecycle family branch, including full fire
   survival/portal alternatives. Record audited versus pending branches explicitly;
   a family label or representative passing test does not prove its siblings.
5. Run narrow affected validation and native reference packet/state traces, then
   seek explicit approval for broad validation. Keep historical RED fixtures but
   review their expectations against exact eligibility rather than accepting the
   old unknown-terminal assumption. Compare manual server state and legitimate PASS.

Completion requires no uncovered registered/inherited/component paths, with cases
for two hands and target/use gates; cooldown/Adventure/border/features; invalid and
multistate placement; entity → generic fallback; inner FAIL/final block FAIL/generic
FAIL; equipment/transformed hands; repeated clicks before acknowledgement; axe/door
local writes versus hoe/shovel server guards; ongoing/finish/release/remainder/local
cooldown; server corrections; and default-component removals. Local inventory,
food, equipment, use state, block/entity state and server-owned updates must support
the next interaction, not merely the first packet trace.

**Proof status:** inventory and the identified common branches are statically
verified; complete branch parity, implementation size and runtime behavior are not.
No new source implementation, tests, builds or gameplay runs were performed in
this continuation. The proposal is not an implemented or accepted behavior proof;
historical verification above remains historical.

## Scope and reconciliation

Standalone branch `fix/igniter-hand-fallback` is based on
`df656521343fe546511487377e47d02e53e84c28`. The previously uncommitted source work
is now recorded in [PR #900](https://github.com/PommeMC/Client/pull/900), inspected
at committed head `2c8c8e7719ad0f1715ebe043beaeef71ca8a2f80`:
`pomme-protocol/src/player.rs`; under `pomme-client/src/`: `app/core.rs`,
`audio/mod.rs`, `net/azalea_compat.rs`, `net/handler.rs`, `net/mod.rs`,
`net/player_wire.rs`, `net/translate/mod.rs`, `net/translate/v767.rs`,
`player/interaction.rs`, `player/interaction_cooldown_tests.rs` (new),
and `renderer/chunk/atlas.rs`. This continuation edits only this existing document;
it was already modified by the preceding documentation investigation at entry.

[Particle PR #899](https://github.com/PommeMC/Client/pull/899) is unaffected.
Its local working fix and all twelve preserved files remain unchanged at particle
HEAD `5b0307fd53375cd0df9e24f9ae3bb384b8ee02ea` (not the standalone baseline).
Local, unpushed backup `backup/particles-before-igniter-extraction-20261107` points
to `1aa1bee6152f6cbcfe950d66d7d2c540f9d1708b`; do not merge that WIP snapshot.
Reconcile later by `Interaction-Fix: cooldown-support` and
`Interaction-Fix: igniter-hand-fallback`, with actual upstream SHAs when they exist.
Do not automatically synchronize or discard the working particle-local fix.
The earlier handoff preceded standalone publication; PR #900 now contains that
work. This documentation-only continuation makes no commit, push, or publication.

## Source references

Pinned Pomme links describe the starting bug, not the subsequent committed PR work:

- [Hand loop / missing item useOn result](https://github.com/PommeMC/Client/blob/df656521343fe546511487377e47d02e53e84c28/pomme-client/src/player/interaction.rs#L790-L902).
- [Owned player decoder](https://github.com/PommeMC/Client/blob/df656521343fe546511487377e47d02e53e84c28/pomme-protocol/src/player.rs), [raw dispatch](https://github.com/PommeMC/Client/blob/df656521343fe546511487377e47d02e53e84c28/pomme-client/src/net/player_wire.rs), [handler](https://github.com/PommeMC/Client/blob/df656521343fe546511487377e47d02e53e84c28/pomme-client/src/net/handler.rs).
- [Legacy cooldown translation](https://github.com/PommeMC/Client/blob/df656521343fe546511487377e47d02e53e84c28/pomme-client/src/net/translate/v767.rs#L120-L138), [documented Azalea divergences](https://github.com/PommeMC/Client/blob/df656521343fe546511487377e47d02e53e84c28/pomme-client/src/net/translate/mod.rs).
- [Pinned Azalea `c_cooldown.rs`](https://github.com/azalea-rs/azalea/blob/ffedf17c9b6ff9fbafbf7e0d14c8d2366b2d0a32/azalea-protocol/src/packets/game/c_cooldown.rs): numeric `ItemKind` + unsigned duration, not the native Identifier + signed duration. Repository/revision verified against `Cargo.toml`, `Cargo.lock`, and the local pinned checkout.

Vanilla **26.3** references were locally inspected under
`reference/26.3/decompiled/net/minecraft/` in the primary checkout, not public source
links: `client.Minecraft.startUseItem`; `client.multiplayer.MultiPlayerGameMode`
`useItemOn`, `performUseItemOn`, `useItem`; `world.item.FlintAndSteelItem.useOn`,
`FireChargeItem.useOn`, `ItemStack.useOn`; `world.item.ItemCooldowns`
`getCooldownGroup`, `getCooldownPercent`, `tick`, `addCooldown`, `removeCooldown`;
`network.protocol.game.ClientboundBlockChangedAckPacket`. No decompiled bodies
are reproduced here and no public Vanilla 26.3 source URL is implied.
