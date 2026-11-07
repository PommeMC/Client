# Interaction fix handoff (draft, not merge-ready)

Development stopped at the user's request. This standalone work contains cooldown
support and regression fixtures, **not the igniter hand-fallback fix**. Twenty
igniter regressions last ran RED; they were not rerun after cooldown implementation.
No full-library GREEN result or manual gameplay validation is claimed.

## Problem and diagnosis

Observed against a Vanilla 26.3 server: flint and steel in the main hand with an
empty offhand leaves a clicked candle unlit; putting a stick in the offhand works.
The diagnosis is server lighting followed by empty-offhand extinguishing, not a
particle rendering failure. Pomme lacks the shared held-item `Item.useOn` result
at the block-use stage, so it incorrectly falls through to generic main-hand use
and then offhand block use. Other item classes can have the same missing-result gap.

Vanilla computes the result synchronously: `Minecraft.startUseItem` tries hands
in order; block-stage SUCCESS or FAIL stops the click, while PASS permits generic
item use and then the other hand. Predicted success supplies the appropriate swing.
`MultiPlayerGameMode.useItemOn` calls `performUseItemOn` within prediction; block
interaction precedes the held item's cooldown gate and `ItemStack.useOn` call.
`ItemStack.useOn` also applies Adventure eligibility before the item's method.
`ClientboundBlockChangedAckPacket` acknowledges only a sequence, not SUCCESS/PASS/
FAIL; waiting for it cannot recover the missing synchronous result.

Do **not** globally skip an empty offhand. A stick in the main hand legitimately
passes, allowing empty-offhand use (including extinguishing a lit candle).
The passing control retains main `UseItemOn(1)`, main `UseItem(2)`, offhand
`UseItemOn(3)`. Igniter fixtures currently produce that same incorrect trace.
Direct-light/supported-fire fixtures expect main `UseItemOn(1)` plus main swing;
negative/unknown fixtures expect only main `UseItemOn(1)`, without local mutation.

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
- Negative durations are stored but immediately inactive, unlike Vanilla 26.3's
  negative/negative percentage before its next tick. This is a disclosed policy
  difference. No client-local cooldown creation, UI, wall-clock timer, or full
  client-local cooldown parity is implemented.
- `Interaction-Fix: igniter-hand-fallback`: twenty failing packet-trace regressions
  and exact native-state fixtures only; the production igniter TODO remains.
- Audio and atlas changes are small `cfg(test)` headless constructors, not unrelated
  production changes. `interaction_cooldown_tests.rs` is the new tracker/gate module.
  Fixtures construct a particle store only to satisfy the existing interaction API;
  they do not test particle behavior. SteelMC was initialized only in this worktree.

## Recorded verification (not rerun for this handoff)

Saved command/exit/output records were inspected and all twelve current Rust-file
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

## Unfinished decisions and checks

The preserved particle prototype alone has Pomme-specific `NonPassingUnpredicted`;
it is **not in this standalone branch**. The inspected igniter class methods never
return PASS once eligible, which motivates a possible nonpassing control result
when exact fire success is unknown. That approximation avoids extra packets but
misses a swing when the unknown outcome was success. It is a proposal for review,
not an implemented or accepted final standalone design, and must follow eligibility.

Full fire survival/portal behavior, Adventure `CanPlaceOn`, global border/feature
gates, and other item useOn behaviors remain incomplete. The 26.3 `block_transformer`
component is lost when translating to the native 26.2 component set; cooldown work
does not solve that. Native client wire is **26.2/protocol 776**, distinct from the
26.3 reference/server. No universal interaction framework is prescribed here.

Unfinished next checks: independent read-only review; resolve the igniter result
path; retain the already-RED lit/wet/occupied/unsupported and positive fixtures;
check eligibility and unknown outcomes without fake success/world mutation;
compare manual Vanilla server state and packet order, including legitimate PASS.
Those tasks were not executed as part of this documentation handoff.

## Scope and reconciliation

Standalone branch `fix/igniter-hand-fallback` is based on
`df656521343fe546511487377e47d02e53e84c28`. Current source work is uncommitted:
`pomme-protocol/src/player.rs`; under `pomme-client/src/`: `app/core.rs`,
`audio/mod.rs`, `net/azalea_compat.rs`, `net/handler.rs`, `net/mod.rs`,
`net/player_wire.rs`, `net/translate/mod.rs`, `net/translate/v767.rs`,
`player/interaction.rs`, `player/interaction_cooldown_tests.rs` (new),
and `renderer/chunk/atlas.rs`. This handoff adds only this repository document.

[Particle PR #899](https://github.com/PommeMC/Client/pull/899) is unaffected.
Its local working fix and all twelve preserved files remain unchanged at particle
HEAD `5b0307fd53375cd0df9e24f9ae3bb384b8ee02ea` (not the standalone baseline).
Local, unpushed backup `backup/particles-before-igniter-extraction-20261107` points
to `1aa1bee6152f6cbcfe950d66d7d2c540f9d1708b`; do not merge that WIP snapshot.
Reconcile later by `Interaction-Fix: cooldown-support` and
`Interaction-Fix: igniter-hand-fallback`, with actual upstream SHAs when they exist.
Do not automatically synchronize or discard the working particle-local fix.
No standalone fix commit, push, or new PR publication has happened in this handoff.

## Source references

Pinned Pomme links describe the starting bug, not these uncommitted changes:

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
