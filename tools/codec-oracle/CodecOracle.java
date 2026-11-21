import com.google.gson.GsonBuilder;
import com.google.gson.JsonArray;
import com.mojang.brigadier.arguments.DoubleArgumentType;
import com.mojang.brigadier.arguments.IntegerArgumentType;
import com.mojang.brigadier.arguments.StringArgumentType;
import com.mojang.brigadier.builder.LiteralArgumentBuilder;
import com.mojang.brigadier.builder.RequiredArgumentBuilder;
import com.mojang.brigadier.tree.ArgumentCommandNode;
import com.mojang.brigadier.tree.CommandNode;
import com.mojang.brigadier.tree.LiteralCommandNode;
import com.mojang.brigadier.tree.RootCommandNode;
import com.google.gson.JsonObject;
import com.mojang.datafixers.util.Pair;
import com.mojang.serialization.Lifecycle;
import io.netty.buffer.ByteBufUtil;
import io.netty.buffer.Unpooled;
import it.unimi.dsi.fastutil.ints.IntList;
import java.nio.file.Files;
import java.nio.file.Path;
import java.time.Instant;
import java.util.BitSet;
import java.util.Comparator;
import java.util.EnumSet;
import java.util.LinkedHashMap;
import java.util.LinkedHashSet;
import java.util.List;
import java.util.Map;
import java.util.Optional;
import java.util.Set;
import java.util.TreeMap;
import java.util.UUID;
import java.util.function.BiConsumer;
import net.minecraft.ChatFormatting;
import net.minecraft.advancements.Advancement;
import net.minecraft.advancements.AdvancementHolder;
import net.minecraft.advancements.AdvancementProgress;
import net.minecraft.advancements.AdvancementRequirements;
import net.minecraft.advancements.AdvancementRewards;
import net.minecraft.advancements.AdvancementType;
import net.minecraft.advancements.CriterionProgress;
import net.minecraft.advancements.DisplayInfo;
import net.minecraft.commands.arguments.EntityArgument;
import net.minecraft.commands.arguments.MessageArgument;
import net.minecraft.commands.arguments.ResourceKeyArgument;
import net.minecraft.commands.arguments.ScoreHolderArgument;
import net.minecraft.commands.arguments.TimeArgument;
import net.minecraft.commands.arguments.UuidArgument;
import net.minecraft.commands.arguments.coordinates.Vec3Argument;
import net.minecraft.SharedConstants;
import net.minecraft.core.BlockPos;
import net.minecraft.core.GlobalPos;
import net.minecraft.core.Holder;
import net.minecraft.core.MappedRegistry;
import net.minecraft.core.PositionAndRotation;
import net.minecraft.core.RegistrationInfo;
import net.minecraft.core.Registry;
import net.minecraft.core.RegistryAccess;
import net.minecraft.core.Vec3i;
import net.minecraft.core.registries.Registries;
import net.minecraft.data.registries.VanillaRegistries;
import net.minecraft.network.FriendlyByteBuf;
import net.minecraft.network.ProtocolInfo;
import net.minecraft.network.RegistryFriendlyByteBuf;
import net.minecraft.network.chat.Component;
import net.minecraft.network.chat.LastSeenMessages;
import net.minecraft.network.chat.Style;
import net.minecraft.network.chat.numbers.BlankFormat;
import net.minecraft.network.chat.numbers.FixedFormat;
import net.minecraft.network.chat.numbers.NumberFormat;
import net.minecraft.network.chat.numbers.StyledFormat;
import net.minecraft.network.protocol.common.ClientboundDisconnectPacket;
import net.minecraft.network.protocol.common.ClientboundKeepAlivePacket;
import net.minecraft.network.protocol.common.ClientboundPingPacket;
import net.minecraft.network.protocol.common.ClientboundResourcePackPopPacket;
import net.minecraft.network.protocol.common.ClientboundResourcePackPushPacket;
import net.minecraft.network.protocol.common.ClientboundShowDialogPacket;
import net.minecraft.network.protocol.common.ClientboundUpdateTagsPacket;
import net.minecraft.network.protocol.game.ClientboundAddEntityPacket;
import net.minecraft.network.protocol.game.ClientboundAnimatePacket;
import net.minecraft.network.protocol.game.ClientboundBlockChangedAckPacket;
import net.minecraft.network.protocol.game.ClientboundBlockEntityDataPacket;
import net.minecraft.network.protocol.game.ClientboundBlockEventPacket;
import net.minecraft.network.protocol.game.ClientboundBlockUpdatePacket;
import net.minecraft.network.protocol.game.ClientboundBossEventPacket;
import net.minecraft.network.protocol.game.ClientboundChunkBatchFinishedPacket;
import net.minecraft.network.protocol.game.ClientboundChunkBatchStartPacket;
import net.minecraft.network.protocol.game.ClientboundChunksBiomesPacket;
import net.minecraft.network.protocol.game.ClientboundClearTitlesPacket;
import net.minecraft.network.protocol.game.ClientboundCommandsPacket;
import net.minecraft.network.protocol.game.ClientboundContainerClosePacket;
import net.minecraft.network.protocol.game.ClientboundContainerSetContentPacket;
import net.minecraft.network.protocol.game.ClientboundContainerSetDataPacket;
import net.minecraft.network.protocol.game.ClientboundContainerSetSlotPacket;
import net.minecraft.network.protocol.game.ClientboundCooldownPacket;
import net.minecraft.network.protocol.game.ClientboundDamageEventPacket;
import net.minecraft.network.protocol.game.ClientboundEntityEventPacket;
import net.minecraft.network.protocol.game.ClientboundEntityPositionSyncPacket;
import net.minecraft.network.protocol.game.ClientboundForgetLevelChunkPacket;
import net.minecraft.network.protocol.game.ClientboundGameEventPacket;
import net.minecraft.network.protocol.game.ClientboundHurtAnimationPacket;
import net.minecraft.network.protocol.game.ClientboundInitializeBorderPacket;
import net.minecraft.network.protocol.game.ClientboundLevelChunkPacketData;
import net.minecraft.network.protocol.game.ClientboundLevelChunkWithLightPacket;
import net.minecraft.network.protocol.game.ClientboundLevelEventPacket;
import net.minecraft.network.protocol.game.ClientboundLevelParticlesPacket;
import net.minecraft.network.protocol.game.ClientboundLightUpdatePacket;
import net.minecraft.network.protocol.game.ClientboundLightUpdatePacketData;
import net.minecraft.network.protocol.game.ClientboundLoginPacket;
import net.minecraft.network.protocol.game.ClientboundMoveEntityPacket;
import net.minecraft.network.protocol.game.ClientboundMoveVehiclePacket;
import net.minecraft.network.protocol.game.ClientboundOpenScreenPacket;
import net.minecraft.network.protocol.game.ClientboundOpenSignEditorPacket;
import net.minecraft.network.protocol.game.ClientboundPlayerAbilitiesPacket;
import net.minecraft.network.protocol.game.ClientboundPlayerCombatKillPacket;
import net.minecraft.network.protocol.game.ClientboundPlayerInfoRemovePacket;
import net.minecraft.network.protocol.game.ClientboundPlayerInfoUpdatePacket;
import net.minecraft.network.protocol.game.ClientboundPlayerPositionPacket;
import net.minecraft.network.protocol.game.ClientboundPlayerRotationPacket;
import net.minecraft.network.protocol.game.ClientboundRemoveEntitiesPacket;
import net.minecraft.network.protocol.game.ClientboundRemoveMobEffectPacket;
import net.minecraft.network.protocol.game.ClientboundResetScorePacket;
import net.minecraft.network.protocol.game.ClientboundRespawnPacket;
import net.minecraft.network.protocol.game.ClientboundRotateHeadPacket;
import net.minecraft.network.protocol.game.ClientboundSectionBlocksUpdatePacket;
import net.minecraft.network.protocol.game.ClientboundSetBorderCenterPacket;
import net.minecraft.network.protocol.game.ClientboundSetBorderLerpSizePacket;
import net.minecraft.network.protocol.game.ClientboundSetBorderSizePacket;
import net.minecraft.network.protocol.game.ClientboundSetBorderWarningDelayPacket;
import net.minecraft.network.protocol.game.ClientboundSetBorderWarningDistancePacket;
import net.minecraft.network.protocol.game.ClientboundSetCameraPacket;
import net.minecraft.network.protocol.game.ClientboundSetChunkCacheCenterPacket;
import net.minecraft.network.protocol.game.ClientboundSetChunkCacheRadiusPacket;
import net.minecraft.network.protocol.game.ClientboundSetCursorItemPacket;
import net.minecraft.network.protocol.game.ClientboundSetDisplayObjectivePacket;
import net.minecraft.network.protocol.game.ClientboundSetEntityDataPacket;
import net.minecraft.network.protocol.game.ClientboundSetEntityMotionPacket;
import net.minecraft.network.protocol.game.ClientboundSetEquipmentPacket;
import net.minecraft.network.protocol.game.ClientboundSetExperiencePacket;
import net.minecraft.network.protocol.game.ClientboundSetHealthPacket;
import net.minecraft.network.protocol.game.ClientboundSetHeldSlotPacket;
import net.minecraft.network.protocol.game.ClientboundSetObjectivePacket;
import net.minecraft.network.protocol.game.ClientboundSetPassengersPacket;
import net.minecraft.network.protocol.game.ClientboundSetPlayerInventoryPacket;
import net.minecraft.network.protocol.game.ClientboundSetPlayerTeamPacket;
import net.minecraft.network.protocol.game.ClientboundSetScorePacket;
import net.minecraft.network.protocol.game.ClientboundSetSimulationDistancePacket;
import net.minecraft.network.protocol.game.ClientboundSetSubtitleTextPacket;
import net.minecraft.network.protocol.game.ClientboundSetTimePacket;
import net.minecraft.network.protocol.game.ClientboundSetTitleTextPacket;
import net.minecraft.network.protocol.game.ClientboundSetTitlesAnimationPacket;
import net.minecraft.network.protocol.game.ClientboundSoundEntityPacket;
import net.minecraft.network.protocol.game.ClientboundSoundPacket;
import net.minecraft.network.protocol.game.ClientboundStopSoundPacket;
import net.minecraft.network.protocol.game.ClientboundTabListPacket;
import net.minecraft.network.protocol.game.ClientboundTakeItemEntityPacket;
import net.minecraft.network.protocol.game.ClientboundTeleportEntityPacket;
import net.minecraft.network.protocol.game.ClientboundTrackedWaypointPacket;
import net.minecraft.network.protocol.game.ClientboundUpdateAdvancementsPacket;
import net.minecraft.network.protocol.game.ClientboundUpdateAttributesPacket;
import net.minecraft.network.protocol.game.ClientboundUpdateMobEffectPacket;
import net.minecraft.network.protocol.game.CommonPlayerSpawnInfo;
import net.minecraft.network.protocol.game.GameProtocols;
import net.minecraft.network.protocol.game.ServerboundAcceptTeleportationPacket;
import net.minecraft.network.protocol.game.ServerboundChangeGameModePacket;
import net.minecraft.network.protocol.game.ServerboundChatAckPacket;
import net.minecraft.network.protocol.game.ServerboundChatCommandPacket;
import net.minecraft.network.protocol.game.ServerboundChatCommandSignedPacket;
import net.minecraft.network.protocol.game.ServerboundChatPacket;
import net.minecraft.network.protocol.game.ServerboundChatSessionUpdatePacket;
import net.minecraft.network.protocol.game.ServerboundChunkBatchReceivedPacket;
import net.minecraft.network.protocol.game.ServerboundClientCommandPacket;
import net.minecraft.network.protocol.game.ServerboundClientTickEndPacket;
import net.minecraft.network.protocol.game.ServerboundCommandSuggestionPacket;
import net.minecraft.network.protocol.game.ServerboundConfigurationAcknowledgedPacket;
import net.minecraft.network.protocol.game.ServerboundContainerButtonClickPacket;
import net.minecraft.network.protocol.game.ServerboundContainerClickPacket;
import net.minecraft.network.protocol.game.ServerboundContainerClosePacket;
import net.minecraft.network.protocol.game.ServerboundContainerSlotStateChangedPacket;
import net.minecraft.network.protocol.game.ServerboundMovePlayerPacket;
import net.minecraft.network.protocol.game.ServerboundMoveVehiclePacket;
import net.minecraft.network.protocol.game.ServerboundPaddleBoatPacket;
import net.minecraft.network.protocol.game.ServerboundPlayerAbilitiesPacket;
import net.minecraft.network.protocol.game.ServerboundPlayerActionPacket;
import net.minecraft.network.protocol.game.ServerboundPlayerCommandPacket;
import net.minecraft.network.protocol.game.ServerboundPlayerInputPacket;
import net.minecraft.network.protocol.game.ServerboundPlayerLoadedPacket;
import net.minecraft.network.protocol.game.ServerboundRecipeBookChangeSettingsPacket;
import net.minecraft.network.protocol.game.ServerboundRenameItemPacket;
import net.minecraft.network.protocol.game.ServerboundSelectBundleItemPacket;
import net.minecraft.network.protocol.game.ServerboundSetCarriedItemPacket;
import net.minecraft.network.protocol.game.ServerboundSignUpdatePacket;
import net.minecraft.network.protocol.game.ServerboundPunchPacket;
import net.minecraft.network.protocol.game.ServerboundTeleportToEntityPacket;
import net.minecraft.network.protocol.game.ServerboundUseItemOnPacket;
import net.minecraft.network.protocol.game.ServerboundUseItemPacket;
import net.minecraft.network.protocol.game.VecDelta;
import net.minecraft.network.syncher.EntityDataAccessor;
import net.minecraft.network.syncher.EntityDataSerializer;
import net.minecraft.network.syncher.EntityDataSerializers;
import net.minecraft.network.syncher.SynchedEntityData;
import net.minecraft.resources.Identifier;
import net.minecraft.resources.ResourceKey;
import net.minecraft.server.Bootstrap;
import net.minecraft.server.dialog.Dialog;
import net.minecraft.server.dialog.Dialogs;
import net.minecraft.sounds.SoundEvent;
import net.minecraft.sounds.SoundEvents;
import net.minecraft.sounds.SoundSource;
import net.minecraft.tags.TagNetworkSerialization;
import net.minecraft.world.BossEvent;
import net.minecraft.world.clock.ClockNetworkState;
import net.minecraft.world.clock.WorldClock;
import net.minecraft.world.clock.WorldClocks;
import net.minecraft.world.entity.EntityType;
import net.minecraft.world.entity.EquipmentSlot;
import net.minecraft.world.entity.PositionMoveRotation;
import net.minecraft.world.entity.PositionPath;
import net.minecraft.world.entity.Relative;
import net.minecraft.world.entity.animal.armadillo.Armadillo;
import net.minecraft.world.entity.animal.golem.CopperGolemState;
import net.minecraft.world.entity.animal.sniffer.Sniffer;
import net.minecraft.world.item.Item;
import net.minecraft.world.item.ItemStack;
import net.minecraft.world.item.Items;
import net.minecraft.world.item.equipment.trim.TrimPattern;
import net.minecraft.world.level.ChunkPos;
import net.minecraft.world.level.GameType;
import net.minecraft.world.level.Level;
import net.minecraft.world.level.biome.Biome;
import net.minecraft.world.level.border.WorldBorder;
import net.minecraft.world.level.block.Block;
import net.minecraft.world.level.block.WeatheringCopper;
import net.minecraft.world.level.block.entity.SignTextSlot;
import net.minecraft.world.level.block.state.BlockState;
import net.minecraft.world.level.chunk.LevelChunkSection;
import net.minecraft.world.level.chunk.PalettedContainer;
import net.minecraft.world.level.chunk.PalettedContainerFactory;
import net.minecraft.world.level.dimension.BuiltinDimensionTypes;
import net.minecraft.world.level.dimension.DimensionType;
import net.minecraft.world.level.levelgen.Heightmap;
import net.minecraft.world.phys.Vec3;
import net.minecraft.world.scores.DisplaySlot;
import net.minecraft.world.scores.Objective;
import net.minecraft.world.scores.PlayerTeam;
import net.minecraft.world.scores.Scoreboard;
import net.minecraft.world.scores.Team;
import net.minecraft.world.scores.TeamColor;
import net.minecraft.world.scores.criteria.ObjectiveCriteria;
import net.minecraft.world.waypoints.Waypoint;
import net.minecraft.world.waypoints.WaypointStyleAssets;

/**
 * Encodes packets with vanilla's own STREAM_CODECs and writes the payloads
 * (without the packet id) as hex, so pomme's decoders are tested against
 * bytes vanilla produced rather than a reading of its source. Registry holders
 * come from vanilla's built-in data registries; the id each was encoded as is
 * written alongside, so tests don't depend on registry order.
 *
 * Usage: java -cp <server classes>:<bundled libs>:<this> CodecOracle <out.json>
 */
public final class CodecOracle {
    private static RegistryAccess registries;
    private static final Map<String, String> CASES = new TreeMap<>();

    public static void main(String[] args) throws Exception {
        if (args.length != 1) {
            System.err.println("usage: CodecOracle <out.json>");
            System.exit(2);
        }
        SharedConstants.tryDetectVersion();
        Bootstrap.bootStrap();

        Registry<DimensionType> dimensionTypes = builtIn(Registries.DIMENSION_TYPE);
        Registry<WorldClock> clocks = builtIn(Registries.WORLD_CLOCK);
        Registry<Biome> biomes = builtIn(Registries.BIOME);
        Registry<Dialog> dialogs = builtIn(Registries.DIALOG);
        Registry<net.minecraft.world.item.enchantment.Enchantment> enchantments = builtIn(Registries.ENCHANTMENT);
        Registry<net.minecraft.world.level.block.entity.BannerPattern> bannerPatterns =
                builtIn(Registries.BANNER_PATTERN);
        registries = new RegistryAccess.ImmutableRegistryAccess(List.of(dimensionTypes, clocks, biomes, dialogs,
                enchantments, bannerPatterns,
                net.minecraft.core.registries.BuiltInRegistries.ATTRIBUTE,
                net.minecraft.core.registries.BuiltInRegistries.ITEM,
                net.minecraft.core.registries.BuiltInRegistries.DATA_COMPONENT_TYPE,
                net.minecraft.core.registries.BuiltInRegistries.MOB_EFFECT,
                net.minecraft.core.registries.BuiltInRegistries.POTION,
                net.minecraft.core.registries.BuiltInRegistries.SOUND_EVENT,
                net.minecraft.core.registries.BuiltInRegistries.ENTITY_TYPE,
                net.minecraft.core.registries.BuiltInRegistries.CONSUME_EFFECT_TYPE,
                net.minecraft.core.registries.BuiltInRegistries.VILLAGER_TYPE,
                net.minecraft.core.registries.BuiltInRegistries.BLOCK_ENTITY_TYPE,
                net.minecraft.core.registries.BuiltInRegistries.MAP_DECORATION_TYPE,
                net.minecraft.core.registries.BuiltInRegistries.PARTICLE_TYPE,
                net.minecraft.core.registries.BuiltInRegistries.BLOCK,
                net.minecraft.core.registries.BuiltInRegistries.MENU,
                net.minecraft.core.registries.BuiltInRegistries.NUMBER_FORMAT_TYPE,
                net.minecraft.core.registries.BuiltInRegistries.VILLAGER_PROFESSION,
                builtIn(Registries.DAMAGE_TYPE),
                builtIn(Registries.CAT_VARIANT), builtIn(Registries.CAT_SOUND_VARIANT),
                builtIn(Registries.CHICKEN_VARIANT), builtIn(Registries.CHICKEN_SOUND_VARIANT),
                builtIn(Registries.COW_VARIANT), builtIn(Registries.COW_SOUND_VARIANT),
                builtIn(Registries.WOLF_VARIANT), builtIn(Registries.WOLF_SOUND_VARIANT),
                builtIn(Registries.FROG_VARIANT), builtIn(Registries.PIG_VARIANT),
                builtIn(Registries.PIG_SOUND_VARIANT), builtIn(Registries.ZOMBIE_NAUTILUS_VARIANT),
                builtIn(Registries.PAINTING_VARIANT)));

        lifecycle(dimensionTypes, clocks);
        status();
        login();
        configuration();
        sections();
        border();
        components(enchantments);
        hashedComponents(bannerPatterns);
        encode("trim_pattern/direct", TrimPattern.STREAM_CODEC::encode, Holder.direct(new TrimPattern(
                Identifier.withDefaultNamespace("test"), Component.literal("Test Trim").withStyle(ChatFormatting.GOLD), true)));
        encode("set_entity_data/sniffer_state", ClientboundSetEntityDataPacket.STREAM_CODEC::encode,
                new ClientboundSetEntityDataPacket(7, List.of(SynchedEntityData.DataValue.create(
                        new EntityDataAccessor<>(18, EntityDataSerializers.SNIFFER_STATE), Sniffer.State.DIGGING))));
        encode("set_entity_data/copper_golem", ClientboundSetEntityDataPacket.STREAM_CODEC::encode,
                new ClientboundSetEntityDataPacket(7, List.of(
                        SynchedEntityData.DataValue.create(new EntityDataAccessor<>(16,
                                EntityDataSerializers.WEATHERING_COPPER_STATE), WeatheringCopper.WeatherState.WEATHERED),
                        SynchedEntityData.DataValue.create(new EntityDataAccessor<>(17,
                                EntityDataSerializers.COPPER_GOLEM_STATE), CopperGolemState.DROPPING_ITEM))));
        encode("set_entity_data/armadillo_state", ClientboundSetEntityDataPacket.STREAM_CODEC::encode,
                new ClientboundSetEntityDataPacket(7, List.of(SynchedEntityData.DataValue.create(
                        new EntityDataAccessor<>(18, EntityDataSerializers.ARMADILLO_STATE),
                        Armadillo.ArmadilloState.UNROLLING))));
        encode("move_vehicle", ServerboundMoveVehiclePacket.STREAM_CODEC::encode,
                new ServerboundMoveVehiclePacket(
                        PositionAndRotation.of(new Vec3(1.5, 64.0, -2.25), 90.0f, 10.0f), true));
        encode("paddle_boat", ServerboundPaddleBoatPacket.STREAM_CODEC::encode,
                new ServerboundPaddleBoatPacket(true, false));
        encode("cooldown/shield", ClientboundCooldownPacket.STREAM_CODEC::encode,
                new ClientboundCooldownPacket(Identifier.withDefaultNamespace("shield"), 100));
        encode("move_vehicle/clientbound", ClientboundMoveVehiclePacket.STREAM_CODEC::encode,
                new ClientboundMoveVehiclePacket(
                        PositionAndRotation.of(new Vec3(1.5, 64.0, -2.25), 90.0f, -10.0f)));
        encode("open_sign_editor/back", ClientboundOpenSignEditorPacket.STREAM_CODEC::encode,
                new ClientboundOpenSignEditorPacket(new BlockPos(-12, 70, 345), SignTextSlot.BACK));
        UUID waypointId = new UUID(0x0123456789abcdefL, 0xfedcba9876543210L);
        Waypoint.Icon icon = new Waypoint.Icon();
        icon.style = WaypointStyleAssets.BOWTIE;
        icon.color = Optional.of(0x12ab34);
        encode("waypoint/track_position", ClientboundTrackedWaypointPacket.STREAM_CODEC::encode,
                ClientboundTrackedWaypointPacket.addWaypointPosition(waypointId, icon, new Vec3i(1, -64, 300)));
        encode("waypoint/update_chunk", ClientboundTrackedWaypointPacket.STREAM_CODEC::encode,
                ClientboundTrackedWaypointPacket.updateWaypointChunk(waypointId, new Waypoint.Icon(), new ChunkPos(-3, 7)));
        encode("waypoint/track_azimuth", ClientboundTrackedWaypointPacket.STREAM_CODEC::encode,
                ClientboundTrackedWaypointPacket.addWaypointAzimuth(waypointId, new Waypoint.Icon(), 1.25f));
        encode("waypoint/untrack", ClientboundTrackedWaypointPacket.STREAM_CODEC::encode,
                ClientboundTrackedWaypointPacket.removeWaypoint(waypointId));
        encode("resource_pack_push/prompt", ClientboundResourcePackPushPacket.STREAM_CODEC::encode,
                new ClientboundResourcePackPushPacket(waypointId, "https://example.com/pack.zip",
                        "0123456789abcdef0123456789abcdef01234567", true, Optional.of(Component.literal("Please"))));
        encode("resource_pack_pop/all", ClientboundResourcePackPopPacket.STREAM_CODEC::encode,
                new ClientboundResourcePackPopPacket(Optional.empty()));
        encode("resource_pack_pop/one", ClientboundResourcePackPopPacket.STREAM_CODEC::encode,
                new ClientboundResourcePackPopPacket(Optional.of(waypointId)));
        Holder<Dialog> serverLinks = dialogs.getOrThrow(Dialogs.SERVER_LINKS);
        encode("show_dialog/reference", ClientboundShowDialogPacket.STREAM_CODEC::encode,
                new ClientboundShowDialogPacket(serverLinks));
        encode("show_dialog/inline", ClientboundShowDialogPacket.STREAM_CODEC::encode,
                new ClientboundShowDialogPacket(Holder.direct(serverLinks.value())));
        commands();
        serverbound();
        entities();
        entityData();
        player();
        blocks();
        chunks();
        containers();
        hud();
        scoreboard();
        playerList();
        sounds();
        advancements();
        Map<Identifier, IntList> blockTags = new LinkedHashMap<>();
        blockTags.put(Identifier.withDefaultNamespace("logs"), IntList.of(1, 300));
        blockTags.put(Identifier.withDefaultNamespace("empty"), IntList.of());
        // In a map that keeps its order: `Map.of` changes it from run to run.
        Map<ResourceKey<? extends Registry<?>>, TagNetworkSerialization.NetworkPayload> tags = new LinkedHashMap<>();
        tags.put(Registries.BLOCK, new TagNetworkSerialization.NetworkPayload(blockTags));
        tags.put(Registries.DIALOG, new TagNetworkSerialization.NetworkPayload(Map.of(
                Identifier.withDefaultNamespace("pause_screen_additions"), IntList.of(2))));
        encode("update_tags/two_registries", ClientboundUpdateTagsPacket.STREAM_CODEC::encode,
                new ClientboundUpdateTagsPacket(tags));

        JsonObject out = new JsonObject();
        out.addProperty("version", SharedConstants.getCurrentVersion().name());
        out.addProperty("protocol", SharedConstants.getProtocolVersion());
        JsonObject packets = new JsonObject();
        packets.add("handshake", phase(net.minecraft.network.protocol.handshake.HandshakeProtocols.SERVERBOUND_TEMPLATE,
                null));
        packets.add("status", phase(net.minecraft.network.protocol.status.StatusProtocols.SERVERBOUND_TEMPLATE,
                net.minecraft.network.protocol.status.StatusProtocols.CLIENTBOUND_TEMPLATE));
        packets.add("login", phase(net.minecraft.network.protocol.login.LoginProtocols.SERVERBOUND_TEMPLATE,
                net.minecraft.network.protocol.login.LoginProtocols.CLIENTBOUND_TEMPLATE));
        packets.add("configuration", phase(
                net.minecraft.network.protocol.configuration.ConfigurationProtocols.SERVERBOUND_TEMPLATE,
                net.minecraft.network.protocol.configuration.ConfigurationProtocols.CLIENTBOUND_TEMPLATE));
        packets.add("game", phase(GameProtocols.SERVERBOUND_TEMPLATE, GameProtocols.CLIENTBOUND_TEMPLATE));
        out.add("packets", packets);
        JsonObject dimensions = new JsonObject();
        for (EntityType<?> type : net.minecraft.core.registries.BuiltInRegistries.ENTITY_TYPE) {
            net.minecraft.world.entity.EntityDimensions of = type.getDimensions();
            JsonArray size = new JsonArray();
            size.add(of.width());
            size.add(of.height());
            size.add(of.eyeHeight());
            dimensions.add(EntityType.getKey(type).toString(), size);
        }
        out.add("entity_dimensions", dimensions);
        JsonObject ids = new JsonObject();
        idsOf(ids, "dimension_type", dimensionTypes);
        idsOf(ids, "world_clock", clocks);
        idsOf(ids, "dialog", dialogs);
        idsOf(ids, "enchantment", enchantments);
        idsOf(ids, "banner_pattern", bannerPatterns);
        // The entries `entities` and `entityData` send, out of registries too large to list whole.
        for (String entry : List.of("damage_type/mob_attack", "entity_type/zombie", "villager_type/swamp",
                "villager_profession/farmer", "item/stone", "attribute/max_health", "mob_effect/haste",
                "block/chest", "block_entity_type/sign",
                "menu/generic_9x3", "sound_event/entity.generic.explode")) {
            String[] parts = entry.split("/");
            Identifier name = Identifier.withDefaultNamespace(parts[1]);
            Registry<?> registry = registries.lookupOrThrow(ResourceKey.createRegistryKey(
                    Identifier.withDefaultNamespace(parts[0])));
            ids.addProperty(parts[0] + "/" + name, idOf(registry, name));
        }
        ids.addProperty("block_state/minecraft:chest", Block.getId(chest()));
        out.add("registry_ids", ids);
        JsonObject sizes = new JsonObject();
        sizes.addProperty("block_state", Block.BLOCK_STATE_REGISTRY.size());
        sizes.addProperty("worldgen/biome", biomes.size());
        for (ResourceKey<? extends Registry<?>> key : List.of(Registries.CAT_VARIANT, Registries.CHICKEN_VARIANT,
                Registries.COW_VARIANT, Registries.WOLF_VARIANT)) {
            sizes.addProperty(key.identifier().getPath(), registries.lookupOrThrow(key).size());
        }
        out.add("registry_sizes", sizes);
        JsonObject cases = new JsonObject();
        CASES.forEach(cases::addProperty);
        out.add("cases", cases);
        Files.writeString(Path.of(args[0]), new GsonBuilder().setPrettyPrinting().create().toJson(out) + "\n");
    }

    /// The serverbound game packets pomme encodes itself.
    /// A phase's packet names in id order, for each direction it has.
    private static JsonObject phase(ProtocolInfo.DetailsProvider serverbound,
            ProtocolInfo.DetailsProvider clientbound) {
        JsonObject phase = new JsonObject();
        phase.add("serverbound", packetNames(serverbound));
        phase.add("clientbound", packetNames(clientbound));
        return phase;
    }

    private static JsonArray packetNames(ProtocolInfo.DetailsProvider protocol) {
        JsonArray names = new JsonArray();
        if (protocol != null) {
            protocol.details().listPackets((type, id) -> {
                if (id != names.size()) {
                    throw new IllegalStateException(type + " is listed out of id order");
                }
                names.add(type.id().getPath());
            });
        }
        return names;
    }

    private static void status() {
        long time = 0x0123456789abcdefL;
        encode("handshake/intention",
                net.minecraft.network.protocol.handshake.ClientIntentionPacket.STREAM_CODEC::encode,
                new net.minecraft.network.protocol.handshake.ClientIntentionPacket(776, "play.example.org", 25565,
                        net.minecraft.network.protocol.handshake.ClientIntent.LOGIN));
        encode("status/ping_request",
                net.minecraft.network.protocol.ping.ServerboundPingRequestPacket.STREAM_CODEC::encode,
                new net.minecraft.network.protocol.ping.ServerboundPingRequestPacket(time));
        encode("status/pong_response",
                net.minecraft.network.protocol.ping.ClientboundPongResponsePacket.STREAM_CODEC::encode,
                new net.minecraft.network.protocol.ping.ClientboundPongResponsePacket(time));
        var players = new net.minecraft.network.protocol.status.ServerStatus.Players(20, 1, List.of(
                new net.minecraft.server.players.NameAndId(new UUID(time, 0xfedcba9876543210L), "Player")));
        encode("status/status_response",
                net.minecraft.network.protocol.status.ClientboundStatusResponsePacket.STREAM_CODEC::encode,
                new net.minecraft.network.protocol.status.ClientboundStatusResponsePacket(
                        new net.minecraft.network.protocol.status.ServerStatus(Component.literal("A Minecraft Server"),
                                java.util.Optional.of(players),
                                java.util.Optional.of(new net.minecraft.network.protocol.status.ServerStatus.Version(
                                        "26.2", 776)),
                                java.util.Optional.of(new net.minecraft.network.protocol.status.ServerStatus.Favicon(
                                        new byte[] {1, 2, 3})),
                                true)));
    }

    private static void login() throws Exception {
        UUID uuid = new UUID(0x0123456789abcdefL, 0xfedcba9876543210L);
        Identifier cookie = Identifier.withDefaultNamespace("session");
        encode("login/serverbound/hello", net.minecraft.network.protocol.login.ServerboundHelloPacket.STREAM_CODEC::encode,
                new net.minecraft.network.protocol.login.ServerboundHelloPacket("Player", uuid));
        // A real key packet encrypts a fresh secret, which differs from run to run.
        encode("login/serverbound/key", net.minecraft.network.protocol.login.ServerboundKeyPacket.STREAM_CODEC::encode,
                withFields(net.minecraft.network.protocol.login.ServerboundKeyPacket.class,
                        Map.of("keybytes", new byte[] {1, 2, 3}, "encryptedChallenge", new byte[] {4, 5})));
        encode("login/serverbound/custom_query_answer",
                net.minecraft.network.protocol.login.ServerboundCustomQueryAnswerPacket.STREAM_CODEC::encode,
                new net.minecraft.network.protocol.login.ServerboundCustomQueryAnswerPacket(300, null));
        encode("login/serverbound/login_acknowledged",
                net.minecraft.network.protocol.login.ServerboundLoginAcknowledgedPacket.STREAM_CODEC::encode,
                net.minecraft.network.protocol.login.ServerboundLoginAcknowledgedPacket.INSTANCE);
        encode("login/serverbound/cookie_response",
                net.minecraft.network.protocol.cookie.ServerboundCookieResponsePacket.STREAM_CODEC::encode,
                new net.minecraft.network.protocol.cookie.ServerboundCookieResponsePacket(cookie, new byte[] {1, 2, 3}));

        encode("login/clientbound/login_disconnect",
                net.minecraft.network.protocol.login.ClientboundLoginDisconnectPacket.STREAM_CODEC::encode,
                new net.minecraft.network.protocol.login.ClientboundLoginDisconnectPacket(Component.literal("Kicked")));
        encode("login/clientbound/hello", net.minecraft.network.protocol.login.ClientboundHelloPacket.STREAM_CODEC::encode,
                new net.minecraft.network.protocol.login.ClientboundHelloPacket("server", new byte[] {1, 2, 3}, new byte[] {4, 5, 6, 7}, true));
        encode("login/clientbound/login_finished",
                net.minecraft.network.protocol.login.ClientboundLoginFinishedPacket.STREAM_CODEC::encode,
                new net.minecraft.network.protocol.login.ClientboundLoginFinishedPacket(
                        new com.mojang.authlib.GameProfile(uuid, "Player", textures("value", "signature")),
                        new UUID(0x1032547698badcfeL, 0xefcdab8967452301L)));
        encode("login/clientbound/login_compression",
                net.minecraft.network.protocol.login.ClientboundLoginCompressionPacket.STREAM_CODEC::encode,
                new net.minecraft.network.protocol.login.ClientboundLoginCompressionPacket(256));
        encode("login/clientbound/custom_query", net.minecraft.network.protocol.login.ClientboundCustomQueryPacket.STREAM_CODEC::encode,
                new net.minecraft.network.protocol.login.ClientboundCustomQueryPacket(300,
                        new net.minecraft.network.protocol.login.custom.DiscardedQueryPayload(Identifier.withDefaultNamespace("query"))));
        encode("login/clientbound/cookie_request",
                net.minecraft.network.protocol.cookie.ClientboundCookieRequestPacket.STREAM_CODEC::encode,
                new net.minecraft.network.protocol.cookie.ClientboundCookieRequestPacket(cookie));
    }

    private static void configuration() {
        Identifier cookie = Identifier.withDefaultNamespace("session");
        List<net.minecraft.server.packs.repository.KnownPack> packs = List.of(
                new net.minecraft.server.packs.repository.KnownPack("minecraft", "core", "26.2"));
        encode("configuration/serverbound/finish_configuration",
                net.minecraft.network.protocol.configuration.ServerboundFinishConfigurationPacket.STREAM_CODEC::encode,
                net.minecraft.network.protocol.configuration.ServerboundFinishConfigurationPacket.INSTANCE);
        encode("configuration/serverbound/select_known_packs",
                net.minecraft.network.protocol.configuration.ServerboundSelectKnownPacks.STREAM_CODEC::encode,
                new net.minecraft.network.protocol.configuration.ServerboundSelectKnownPacks(packs));
        encode("configuration/serverbound/cookie_response",
                net.minecraft.network.protocol.cookie.ServerboundCookieResponsePacket.STREAM_CODEC::encode,
                new net.minecraft.network.protocol.cookie.ServerboundCookieResponsePacket(cookie, new byte[] {1, 2, 3}));

        net.minecraft.nbt.CompoundTag overworld = new net.minecraft.nbt.CompoundTag();
        overworld.putInt("height", 384);
        encode("configuration/clientbound/registry_data",
                net.minecraft.network.protocol.configuration.ClientboundRegistryDataPacket.STREAM_CODEC::encode,
                new net.minecraft.network.protocol.configuration.ClientboundRegistryDataPacket(Registries.DIMENSION_TYPE, List.of(
                        new net.minecraft.core.RegistrySynchronization.PackedRegistryEntry(
                                Identifier.withDefaultNamespace("overworld"), java.util.Optional.of(overworld)),
                        new net.minecraft.core.RegistrySynchronization.PackedRegistryEntry(
                                Identifier.withDefaultNamespace("the_nether"), java.util.Optional.empty()))));
        encode("configuration/clientbound/select_known_packs",
                net.minecraft.network.protocol.configuration.ClientboundSelectKnownPacks.STREAM_CODEC::encode,
                new net.minecraft.network.protocol.configuration.ClientboundSelectKnownPacks(packs));
        encode("configuration/clientbound/finish_configuration",
                net.minecraft.network.protocol.configuration.ClientboundFinishConfigurationPacket.STREAM_CODEC::encode,
                net.minecraft.network.protocol.configuration.ClientboundFinishConfigurationPacket.INSTANCE);
        encode("configuration/clientbound/cookie_request",
                net.minecraft.network.protocol.cookie.ClientboundCookieRequestPacket.STREAM_CODEC::encode,
                new net.minecraft.network.protocol.cookie.ClientboundCookieRequestPacket(cookie));
    }

    private static void serverbound() {
        UUID uuid = new UUID(0x0123456789abcdefL, 0xfedcba9876543210L);
        BlockPos pos = new BlockPos(-12, 70, 345);
        // Messages 0, 2 and 19 of the last twenty seen.
        LastSeenMessages.Update seen = new LastSeenMessages.Update(3, BitSet.valueOf(new long[] {0x080005L}),
                (byte) 0x42);
        encode("serverbound/chat", ServerboundChatPacket.STREAM_CODEC::encode,
                new ServerboundChatPacket("hi", Instant.ofEpochMilli(5), -6L, Optional.of(signature(9)), seen));
        encode("serverbound/chat_unsigned", ServerboundChatPacket.STREAM_CODEC::encode,
                new ServerboundChatPacket("hi", Instant.ofEpochMilli(5), -6L, Optional.empty(), seen));
        encode("serverbound/chat_command", ServerboundChatCommandPacket.STREAM_CODEC::encode,
                new ServerboundChatCommandPacket("help"));
        encode("serverbound/chat_command_signed", ServerboundChatCommandSignedPacket.STREAM_CODEC::encode,
                new ServerboundChatCommandSignedPacket("msg a hi", Instant.ofEpochMilli(7), 8L,
                        new net.minecraft.commands.arguments.ArgumentSignatures(List.of(
                                new net.minecraft.commands.arguments.ArgumentSignatures.Entry("message",
                                        signature(4)))),
                        seen));
        encode("serverbound/chat_session_update", ServerboundChatSessionUpdatePacket.STREAM_CODEC::encode,
                new ServerboundChatSessionUpdatePacket(chatSession(new UUID(0L, 7L), 1234, new byte[] {1, 2},
                        new byte[] {3})));
        encode("serverbound/chat_ack", ServerboundChatAckPacket.STREAM_CODEC::encode,
                new ServerboundChatAckPacket(300));
        encode("serverbound/recipe_book_change_settings",
                ServerboundRecipeBookChangeSettingsPacket.STREAM_CODEC::encode,
                new ServerboundRecipeBookChangeSettingsPacket(
                        net.minecraft.world.inventory.RecipeBookType.BLAST_FURNACE, true, false));
        encode("serverbound/accept_teleportation",
                ServerboundAcceptTeleportationPacket.STREAM_CODEC::encode,
                new ServerboundAcceptTeleportationPacket(300, 1.5, 64.0, -2.25, 90.0f, -10.0f));
        encode("serverbound/bundle_item_selected",
                ServerboundSelectBundleItemPacket.STREAM_CODEC::encode,
                new ServerboundSelectBundleItemPacket(36, -1));
        encode("serverbound/change_game_mode",
                ServerboundChangeGameModePacket.STREAM_CODEC::encode,
                new ServerboundChangeGameModePacket(GameType.SPECTATOR));
        encode("serverbound/chunk_batch_received",
                ServerboundChunkBatchReceivedPacket.STREAM_CODEC::encode,
                new ServerboundChunkBatchReceivedPacket(12.5f));
        encode("serverbound/client_command",
                ServerboundClientCommandPacket.STREAM_CODEC::encode,
                new ServerboundClientCommandPacket(
                        ServerboundClientCommandPacket.Action.REQUEST_STATS));
        encode("serverbound/client_information",
                net.minecraft.network.protocol.common.ServerboundClientInformationPacket.STREAM_CODEC::encode,
                new net.minecraft.network.protocol.common.ServerboundClientInformationPacket(
                        new net.minecraft.server.level.ClientInformation("en_gb", 12,
                                net.minecraft.world.entity.player.ChatVisiblity.SYSTEM, true, 0x7f,
                                net.minecraft.world.entity.HumanoidArm.LEFT, false, true,
                                net.minecraft.server.level.ParticleStatus.MINIMAL)));
        encode("serverbound/client_tick_end",
                ServerboundClientTickEndPacket.STREAM_CODEC::encode,
                ServerboundClientTickEndPacket.INSTANCE);
        encode("serverbound/command_suggestion",
                ServerboundCommandSuggestionPacket.STREAM_CODEC::encode,
                new ServerboundCommandSuggestionPacket(7, "/give @s sto"));
        encode("serverbound/configuration_acknowledged",
                ServerboundConfigurationAcknowledgedPacket.STREAM_CODEC::encode,
                ServerboundConfigurationAcknowledgedPacket.INSTANCE);
        encode("serverbound/container_button_click",
                ServerboundContainerButtonClickPacket.STREAM_CODEC::encode,
                new ServerboundContainerButtonClickPacket(3, 200));
        var changed = new it.unimi.dsi.fastutil.ints.Int2ObjectOpenHashMap<net.minecraft.network.HashedStack>();
        changed.put(5, new net.minecraft.network.HashedStack.ActualItem(
                net.minecraft.core.registries.BuiltInRegistries.ITEM.wrapAsHolder(
                        net.minecraft.world.item.Items.DIAMOND_SWORD),
                1,
                new net.minecraft.network.HashedPatchMap(
                        Map.of(net.minecraft.core.component.DataComponents.DAMAGE, 0x12345678),
                        Set.of(net.minecraft.core.component.DataComponents.CUSTOM_NAME))));
        encode("serverbound/container_click",
                ServerboundContainerClickPacket.STREAM_CODEC::encode,
                new ServerboundContainerClickPacket(3, 300, (short) -999, (byte) 1,
                        net.minecraft.world.inventory.ContainerInput.THROW, changed,
                        net.minecraft.network.HashedStack.EMPTY));
        encode("serverbound/container_close",
                ServerboundContainerClosePacket.STREAM_CODEC::encode,
                new ServerboundContainerClosePacket(3));
        encode("serverbound/container_slot_state_changed",
                ServerboundContainerSlotStateChangedPacket.STREAM_CODEC::encode,
                new ServerboundContainerSlotStateChangedPacket(8, 3, true));
        encode("serverbound/custom_payload",
                net.minecraft.network.protocol.common.ServerboundCustomPayloadPacket.STREAM_CODEC::encode,
                new net.minecraft.network.protocol.common.ServerboundCustomPayloadPacket(
                        new net.minecraft.network.protocol.common.custom.BrandPayload("pomme")));
        encode("serverbound/keep_alive",
                net.minecraft.network.protocol.common.ServerboundKeepAlivePacket.STREAM_CODEC::encode,
                new net.minecraft.network.protocol.common.ServerboundKeepAlivePacket(-2L));
        encode("serverbound/move_player_pos",
                ServerboundMovePlayerPacket.Pos.STREAM_CODEC::encode,
                new ServerboundMovePlayerPacket.Pos(1.5, 64.0, -2.25, true, false));
        encode("serverbound/move_player_pos_rot",
                ServerboundMovePlayerPacket.PosRot.STREAM_CODEC::encode,
                new ServerboundMovePlayerPacket.PosRot(
                        1.5, 64.0, -2.25, 90.0f, -10.0f, false, true));
        encode("serverbound/move_player_rot",
                ServerboundMovePlayerPacket.Rot.STREAM_CODEC::encode,
                new ServerboundMovePlayerPacket.Rot(90.0f, -10.0f, true, true));
        encode("serverbound/move_player_status_only",
                ServerboundMovePlayerPacket.StatusOnly.STREAM_CODEC::encode,
                new ServerboundMovePlayerPacket.StatusOnly(false, false));
        var abilities = new net.minecraft.world.entity.player.Abilities();
        abilities.flying = true;
        encode("serverbound/player_abilities",
                ServerboundPlayerAbilitiesPacket.STREAM_CODEC::encode,
                new ServerboundPlayerAbilitiesPacket(abilities));
        encode("serverbound/player_action",
                ServerboundPlayerActionPacket.STREAM_CODEC::encode,
                new ServerboundPlayerActionPacket(
                        ServerboundPlayerActionPacket.Action.STOP_DESTROY_BLOCK,
                        pos, net.minecraft.core.Direction.WEST, 300));
        // The constructor wants an entity, so this one decodes the bytes
        // pomme writes (entity 300, START_RIDING_JUMP, 100) and encodes them back.
        encode("serverbound/player_command",
                ServerboundPlayerCommandPacket.STREAM_CODEC::encode,
                ServerboundPlayerCommandPacket.STREAM_CODEC.decode(
                        new FriendlyByteBuf(Unpooled.wrappedBuffer(new byte[] {(byte) 0xAC, 2, 3, 100}))));
        encode("serverbound/player_input",
                ServerboundPlayerInputPacket.STREAM_CODEC::encode,
                new ServerboundPlayerInputPacket(
                        new net.minecraft.world.entity.player.Input(true, false, false, true, true, false, true)));
        encode("serverbound/player_loaded",
                ServerboundPlayerLoadedPacket.STREAM_CODEC::encode,
                new ServerboundPlayerLoadedPacket());
        encode("serverbound/pong",
                net.minecraft.network.protocol.common.ServerboundPongPacket.STREAM_CODEC::encode,
                new net.minecraft.network.protocol.common.ServerboundPongPacket(-5));
        encode("serverbound/rename_item",
                ServerboundRenameItemPacket.STREAM_CODEC::encode,
                new ServerboundRenameItemPacket("Sting"));
        encode("serverbound/resource_pack",
                net.minecraft.network.protocol.common.ServerboundResourcePackPacket.STREAM_CODEC::encode,
                new net.minecraft.network.protocol.common.ServerboundResourcePackPacket(uuid,
                        net.minecraft.network.protocol.common.ServerboundResourcePackPacket.Action.FAILED_RELOAD));
        encode("serverbound/set_carried_item",
                ServerboundSetCarriedItemPacket.STREAM_CODEC::encode,
                new ServerboundSetCarriedItemPacket(8));
        encode("serverbound/sign_update",
                ServerboundSignUpdatePacket.STREAM_CODEC::encode,
                new ServerboundSignUpdatePacket(pos, List.of("one", "", "thrée", "4"), SignTextSlot.BACK));
        encode("serverbound/punch", ServerboundPunchPacket.STREAM_CODEC::encode, new ServerboundPunchPacket());
        encode("serverbound/teleport_to_entity",
                ServerboundTeleportToEntityPacket.STREAM_CODEC::encode,
                new ServerboundTeleportToEntityPacket(uuid));
        encode("serverbound/use_item",
                ServerboundUseItemPacket.STREAM_CODEC::encode,
                new ServerboundUseItemPacket(
                        net.minecraft.world.InteractionHand.MAIN_HAND, 300, 90.0f, -10.0f));
        encode("serverbound/use_item_on",
                ServerboundUseItemOnPacket.STREAM_CODEC::encode,
                new ServerboundUseItemOnPacket(
                        net.minecraft.world.InteractionHand.OFF_HAND,
                        new net.minecraft.world.phys.BlockHitResult(new Vec3(-11.25, 71.0, 345.5),
                                net.minecraft.core.Direction.UP, pos, true, false),
                        300));
    }

    private static void commands() {
        RootCommandNode<Object> root = new RootCommandNode<>();
        LiteralCommandNode<Object> tp = LiteralArgumentBuilder.literal("tp")
                .then(RequiredArgumentBuilder.argument("target", EntityArgument.entity())
                        .then(RequiredArgumentBuilder.argument("pos", Vec3Argument.vec3()).executes(c -> 1)))
                .build();
        root.addChild(tp);
        root.addChild(LiteralArgumentBuilder.literal("say")
                .then(RequiredArgumentBuilder.argument("message", MessageArgument.message()).executes(c -> 1))
                .build());
        root.addChild(LiteralArgumentBuilder.literal("give")
                .then(RequiredArgumentBuilder.argument("item", ResourceKeyArgument.key(Registries.ITEM))
                        .then(RequiredArgumentBuilder.argument("count", IntegerArgumentType.integer(1, 64))
                                .then(RequiredArgumentBuilder.argument("scale",
                                                DoubleArgumentType.doubleArg(-Double.MAX_VALUE, 2.5))
                                        .then(RequiredArgumentBuilder.argument("word", StringArgumentType.greedyString())
                                                .then(RequiredArgumentBuilder.argument("delay", TimeArgument.time(1))
                                                        .then(RequiredArgumentBuilder.argument("holders",
                                                                        ScoreHolderArgument.scoreHolders())
                                                                .then(RequiredArgumentBuilder.argument("id",
                                                                        UuidArgument.uuid()).executes(c -> 1))))))))
                .build());
        root.addChild(LiteralArgumentBuilder.literal("alias").redirect(tp).build());
        encode("commands/tree", ClientboundCommandsPacket.STREAM_CODEC::encode,
                new ClientboundCommandsPacket(root, new ClientboundCommandsPacket.NodeInspector<Object>() {
                    @Override
                    public Identifier suggestionId(ArgumentCommandNode<Object, ?> node) {
                        return node.getName().equals("message") ? Identifier.withDefaultNamespace("ask_server") : null;
                    }

                    @Override
                    public boolean isExecutable(CommandNode<Object> node) {
                        return node.getCommand() != null;
                    }

                    @Override
                    public boolean isRestricted(CommandNode<Object> node) {
                        return node.getName().equals("say");
                    }
                }));
    }

    private static void lifecycle(Registry<DimensionType> dimensionTypes, Registry<WorldClock> clocks) {
        Holder<DimensionType> overworld = dimensionTypes.getOrThrow(BuiltinDimensionTypes.OVERWORLD);
        Holder<DimensionType> nether = dimensionTypes.getOrThrow(BuiltinDimensionTypes.NETHER);
        CommonPlayerSpawnInfo spawn = new CommonPlayerSpawnInfo(
                overworld, Level.OVERWORLD, 42L, GameType.CREATIVE, Optional.empty(), false, true, Optional.empty(),
                4, 63);
        CommonPlayerSpawnInfo died = new CommonPlayerSpawnInfo(
                nether, Level.NETHER, -7L, GameType.SURVIVAL, Optional.of(GameType.ADVENTURE), true, false,
                Optional.of(GlobalPos.of(Level.NETHER, new BlockPos(5, 70, -3))), 0, 32);
        Set<ResourceKey<Level>> levels = new LinkedHashSet<>(List.of(Level.OVERWORLD, Level.NETHER, Level.END));

        encode("login/fresh", ClientboundLoginPacket.STREAM_CODEC::encode, new ClientboundLoginPacket(
                7, false, levels, 20, 10, 8, false, true, false, spawn, true, false));
        encode("login/after_death", ClientboundLoginPacket.STREAM_CODEC::encode, new ClientboundLoginPacket(
                -1, true, Set.of(Level.NETHER), 0, 2, 2, true, false, true, died, false, true));
        encode("respawn/keep_all", ClientboundRespawnPacket.STREAM_CODEC::encode, new ClientboundRespawnPacket(died, (byte) 3));
        encode("keep_alive/negative", ClientboundKeepAlivePacket.STREAM_CODEC::encode, new ClientboundKeepAlivePacket(-2L));
        encode("ping/negative", ClientboundPingPacket.STREAM_CODEC::encode, new ClientboundPingPacket(-5));
        encode("disconnect/styled", ClientboundDisconnectPacket.STREAM_CODEC::encode, new ClientboundDisconnectPacket(
                Component.literal("bye").withStyle(ChatFormatting.RED)));

        PositionMoveRotation change = new PositionMoveRotation(
                new Vec3(1.5, 64.0, -2.0), new Vec3(0.0, 0.5, 0.0), 90.0f, 10.0f);
        Set<Relative> relatives = EnumSet.of(Relative.Y, Relative.X_ROT, Relative.ROTATE_DELTA);
        encode("player_position/mixed_relative", ClientboundPlayerPositionPacket.STREAM_CODEC::encode,
                new ClientboundPlayerPositionPacket(3, change, relatives));
        encode("teleport_entity/mixed_relative", ClientboundTeleportEntityPacket.STREAM_CODEC::encode,
                new ClientboundTeleportEntityPacket(9, change, relatives, true));
        encode("player_rotation/relative_yaw", ClientboundPlayerRotationPacket.STREAM_CODEC::encode,
                new ClientboundPlayerRotationPacket(45.0f, true, -30.0f, false));

        Map<Holder<WorldClock>, ClockNetworkState> updates = new LinkedHashMap<>();
        updates.put(clocks.getOrThrow(WorldClocks.OVERWORLD), new ClockNetworkState(6000L, 0.5f, 1.0f));
        updates.put(clocks.getOrThrow(WorldClocks.THE_END), new ClockNetworkState(300L, 0.0f, 0.0f));
        encode("set_time/two_clocks", ClientboundSetTimePacket.STREAM_CODEC::encode, new ClientboundSetTimePacket(12000L, updates));
        encode("set_simulation_distance/twelve", ClientboundSetSimulationDistancePacket.STREAM_CODEC::encode,
                new ClientboundSetSimulationDistancePacket(12));
    }

    /**
     * Chunk sections as {@code LevelChunkSection.write} sends them, one per
     * palette kind. Every block takes state id {@code 1 + i % blockStates} (none
     * when 0) and every biome cell biome id {@code i % biomes} (the default
     * plains when 0), {@code i} being the entry's container index.
     */
    private static void sections() {
        PalettedContainerFactory factory = PalettedContainerFactory.create(registries);
        section(factory, "chunk_section/empty", 0, 0);
        section(factory, "chunk_section/linear", 3, 3);
        section(factory, "chunk_section/hashmap", 20, 1);
        section(factory, "chunk_section/global", 300, 20);
        chunkBiomes(factory);
    }

    /**
     * Two columns as {@code ChunkBiomeData} frames them: one with an all-plains
     * section then one whose cell {@code i} holds biome {@code i % 3}, and an
     * empty one.
     */
    private static void chunkBiomes(PalettedContainerFactory factory) {
        Registry<Biome> biomeRegistry = registries.lookupOrThrow(Registries.BIOME);
        PalettedContainer<Holder<Biome>> mixed = factory.createForBiomes();
        for (int i = 0; i < 64; ++i) {
            mixed.set(i & 3, i >> 4, i >> 2 & 3, biomeRegistry.get(i % 3).orElseThrow());
        }
        FriendlyByteBuf column = new FriendlyByteBuf(Unpooled.buffer());
        factory.createForBiomes().write(column);
        mixed.write(column);
        encode("chunks_biomes/two_columns", ClientboundChunksBiomesPacket.STREAM_CODEC::encode,
                new ClientboundChunksBiomesPacket(List.of(
                        new ClientboundChunksBiomesPacket.ChunkBiomeData(new ChunkPos(3, -7), ByteBufUtil.getBytes(column)),
                        new ClientboundChunksBiomesPacket.ChunkBiomeData(new ChunkPos(-1, 0), new byte[0]))));
    }

    private static void section(PalettedContainerFactory factory, String name, int blockStates, int biomes) {
        PalettedContainer<BlockState> states = factory.createForBlockStates();
        for (int i = 0; blockStates > 0 && i < 4096; ++i) {
            states.set(i & 15, i >> 8, i >> 4 & 15, Block.BLOCK_STATE_REGISTRY.byId(1 + i % blockStates));
        }
        PalettedContainer<Holder<Biome>> biomeCells = factory.createForBiomes();
        Registry<Biome> biomeRegistry = registries.lookupOrThrow(Registries.BIOME);
        for (int i = 0; biomes > 0 && i < 64; ++i) {
            biomeCells.set(i & 3, i >> 4, i >> 2 & 3, biomeRegistry.get(i % biomes).orElseThrow());
        }
        encode(name, (buf, section) -> section.write(buf), new LevelChunkSection(states, biomeCells));
    }

    /**
     * A frozen copy of a built-in data registry, sorted by identifier: the
     * lookup's own order varies between runs, which would churn the ids.
     */
    private static <T> Registry<T> builtIn(ResourceKey<? extends Registry<T>> key) {
        MappedRegistry<T> registry = new MappedRegistry<>(key, Lifecycle.stable());
        VanillaRegistries.createWorldLookup().lookupOrThrow(key).listElements()
                .sorted(Comparator.comparing(entry -> entry.key().identifier().toString()))
                .forEach(entry -> registry.register(entry.key(), entry.value(), RegistrationInfo.BUILT_IN));
        return registry.freeze();
    }

    private static <T> int idOf(Registry<T> registry, Identifier name) {
        return registry.getId(registry.getValue(name));
    }

    private static <T> void idsOf(JsonObject ids, String name, Registry<T> registry) {
        registry.listElements().forEach(entry ->
                ids.addProperty(name + "/" + entry.key().identifier(), registry.getId(entry.value())));
    }

    /**
     * The six world border packets, each built from one border: centred on
     * (1.5, -2.5), shrinking from 60 to 20 over 300 ticks, at most 1000 wide,
     * warning 7 blocks and 40 ticks ahead.
     */
    private static void border() {
        WorldBorder border = new WorldBorder();
        border.setCenter(1.5, -2.5);
        border.setAbsoluteMaxSize(1000);
        border.setWarningBlocks(7);
        border.setWarningTime(40);
        border.lerpSizeBetween(60.0, 20.0, 300L, 0L);
        encode("border/initialize", ClientboundInitializeBorderPacket.STREAM_CODEC::encode,
                new ClientboundInitializeBorderPacket(border));
        encode("border/center", ClientboundSetBorderCenterPacket.STREAM_CODEC::encode,
                new ClientboundSetBorderCenterPacket(border));
        encode("border/lerp_size", ClientboundSetBorderLerpSizePacket.STREAM_CODEC::encode,
                new ClientboundSetBorderLerpSizePacket(border));
        encode("border/size", ClientboundSetBorderSizePacket.STREAM_CODEC::encode,
                new ClientboundSetBorderSizePacket(border));
        encode("border/warning_delay", ClientboundSetBorderWarningDelayPacket.STREAM_CODEC::encode,
                new ClientboundSetBorderWarningDelayPacket(border));
        encode("border/warning_distance", ClientboundSetBorderWarningDistancePacket.STREAM_CODEC::encode,
                new ClientboundSetBorderWarningDistancePacket(border));
    }

    /**
     * Item component values no item has by default, or has only empty, each
     * through its type's own stream codec.
     */
    private static void components(Registry<net.minecraft.world.item.enchantment.Enchantment> enchantments) {
        component("dyed_color", net.minecraft.core.component.DataComponents.DYED_COLOR,
                new net.minecraft.world.item.component.DyedItemColor(0x123456));
        component("unbreakable", net.minecraft.core.component.DataComponents.UNBREAKABLE,
                net.minecraft.util.Unit.INSTANCE);
        component("custom_name", net.minecraft.core.component.DataComponents.CUSTOM_NAME,
                Component.literal("Excalibur").withStyle(ChatFormatting.GOLD));
        component("custom_model_data", net.minecraft.core.component.DataComponents.CUSTOM_MODEL_DATA,
                new net.minecraft.world.item.component.CustomModelData(List.of(1.5f), List.of(true, false),
                        List.of("a"), List.of(0xff0000)));
        component("firework_explosion", net.minecraft.core.component.DataComponents.FIREWORK_EXPLOSION,
                new net.minecraft.world.item.component.FireworkExplosion(
                        net.minecraft.world.item.component.FireworkExplosion.Shape.STAR, IntList.of(1, 2),
                        IntList.of(3), true, false));
        component("profile", net.minecraft.core.component.DataComponents.PROFILE,
                net.minecraft.world.item.component.ResolvableProfile.createUnresolved("Steve"));
        net.minecraft.nbt.CompoundTag tag = new net.minecraft.nbt.CompoundTag();
        tag.putInt("level", 3);
        component("custom_data", net.minecraft.core.component.DataComponents.CUSTOM_DATA,
                net.minecraft.world.item.component.CustomData.of(tag));
        net.minecraft.world.item.enchantment.ItemEnchantments.Mutable levels =
                new net.minecraft.world.item.enchantment.ItemEnchantments.Mutable(
                        net.minecraft.world.item.enchantment.ItemEnchantments.EMPTY);
        levels.set(enchantments.getOrThrow(net.minecraft.world.item.enchantment.Enchantments.SHARPNESS), 5);
        component("enchantments", net.minecraft.core.component.DataComponents.ENCHANTMENTS, levels.toImmutable());
        component("potion_contents", net.minecraft.core.component.DataComponents.POTION_CONTENTS,
                new net.minecraft.world.item.alchemy.PotionContents(
                        Optional.of(net.minecraft.world.item.alchemy.Potions.STRONG_SWIFTNESS), Optional.of(0x123456),
                        List.of(new net.minecraft.world.effect.MobEffectInstance(
                                net.minecraft.world.effect.MobEffects.SPEED, 200, 1)),
                        Optional.of("custom")));
        component("tooltip_display", net.minecraft.core.component.DataComponents.TOOLTIP_DISPLAY,
                new net.minecraft.world.item.component.TooltipDisplay(true,
                        new it.unimi.dsi.fastutil.objects.ReferenceLinkedOpenHashSet<>(List.of(
                                net.minecraft.core.component.DataComponents.DAMAGE,
                                net.minecraft.core.component.DataComponents.LORE))));
        component("use_cooldown", net.minecraft.core.component.DataComponents.USE_COOLDOWN,
                new net.minecraft.world.item.component.UseCooldown(1.5f,
                        Optional.of(Identifier.withDefaultNamespace("pearl"))));
        component("equippable", net.minecraft.core.component.DataComponents.EQUIPPABLE,
                net.minecraft.world.item.equipment.Equippable.builder(EquipmentSlot.HEAD)
                        .setEquipSound(Holder.direct(net.minecraft.sounds.SoundEvent.createFixedRangeEvent(
                                Identifier.withDefaultNamespace("custom"), 16.0f)))
                        .build());
        component("lore", net.minecraft.core.component.DataComponents.LORE,
                new net.minecraft.world.item.component.ItemLore(List.of(
                        Component.literal("First"), Component.literal("Second").withStyle(ChatFormatting.RED))));
        component("trim", net.minecraft.core.component.DataComponents.TRIM,
                new net.minecraft.world.item.equipment.trim.ArmorTrim(
                        Holder.direct(new net.minecraft.world.item.equipment.trim.TrimMaterial(
                                Identifier.withDefaultNamespace("test"), Component.literal("Test Material"))),
                        Holder.direct(new TrimPattern(Identifier.withDefaultNamespace("test"),
                                Component.literal("Test Trim"), true))));
        net.minecraft.nbt.CompoundTag bee = new net.minecraft.nbt.CompoundTag();
        bee.putInt("Age", 7);
        component("bees", net.minecraft.core.component.DataComponents.BEES,
                new net.minecraft.world.item.component.Bees(List.of(
                        new net.minecraft.world.level.block.entity.BeehiveBlockEntity.Occupant(
                                net.minecraft.world.item.component.TypedEntityData.of(
                                        net.minecraft.world.entity.EntityTypes.BEE, bee), 5, 10))));
        component("attribute_modifiers", net.minecraft.core.component.DataComponents.ATTRIBUTE_MODIFIERS,
                net.minecraft.world.item.component.ItemAttributeModifiers.builder()
                        .add(net.minecraft.world.entity.ai.attributes.Attributes.ARMOR,
                                new net.minecraft.world.entity.ai.attributes.AttributeModifier(
                                        Identifier.withDefaultNamespace("a"), 2.5,
                                        net.minecraft.world.entity.ai.attributes.AttributeModifier.Operation.ADD_MULTIPLIED_BASE),
                                net.minecraft.world.entity.EquipmentSlotGroup.CHEST,
                                new net.minecraft.world.item.component.ItemAttributeModifiers.Display.OverrideText(
                                        Component.literal("Sturdy")))
                        .build());
    }

    /** Values of the components pomme only hashes, none of them an item's default. */
    private static void hashedComponents(Registry<net.minecraft.world.level.block.entity.BannerPattern> bannerPatterns) {
        component("banner_patterns", net.minecraft.core.component.DataComponents.BANNER_PATTERNS,
                new net.minecraft.world.level.block.entity.BannerPatternLayers(List.of(
                        new net.minecraft.world.level.block.entity.BannerPatternLayers.Layer(
                                bannerPatterns.getOrThrow(
                                        net.minecraft.world.level.block.entity.BannerPatterns.CREEPER),
                                net.minecraft.world.item.DyeColor.LIME))));
        component("sulfur_cube_content", net.minecraft.core.component.DataComponents.SULFUR_CUBE_CONTENT,
                new net.minecraft.world.item.component.SulfurCubeContent(
                        new net.minecraft.world.item.ItemStackTemplate(net.minecraft.world.item.Items.TNT, 2)));
        Identifier custom = Identifier.withDefaultNamespace("custom");
        component("item_model", net.minecraft.core.component.DataComponents.ITEM_MODEL, custom);
        component("tooltip_style", net.minecraft.core.component.DataComponents.TOOLTIP_STYLE, custom);
        component("note_block_sound", net.minecraft.core.component.DataComponents.NOTE_BLOCK_SOUND, custom);
        component("repair_cost", net.minecraft.core.component.DataComponents.REPAIR_COST, 3);
        component("map_id", net.minecraft.core.component.DataComponents.MAP_ID,
                new net.minecraft.world.level.saveddata.maps.MapId(7));
        component("ominous_bottle_amplifier", net.minecraft.core.component.DataComponents.OMINOUS_BOTTLE_AMPLIFIER,
                new net.minecraft.world.item.component.OminousBottleAmplifier(2));
        component("enchantable", net.minecraft.core.component.DataComponents.ENCHANTABLE,
                new net.minecraft.world.item.enchantment.Enchantable(15));
        component("enchantment_glint_override",
                net.minecraft.core.component.DataComponents.ENCHANTMENT_GLINT_OVERRIDE, true);
        component("potion_duration_scale", net.minecraft.core.component.DataComponents.POTION_DURATION_SCALE, 0.5f);
        component("base_color", net.minecraft.core.component.DataComponents.BASE_COLOR,
                net.minecraft.world.item.DyeColor.RED);
        component("break_sound", net.minecraft.core.component.DataComponents.BREAK_SOUND,
                Holder.direct(net.minecraft.sounds.SoundEvent.createFixedRangeEvent(custom, 8.0f)));
        component("weapon", net.minecraft.core.component.DataComponents.WEAPON,
                new net.minecraft.world.item.component.Weapon(2, 1.5f));
        component("attack_range", net.minecraft.core.component.DataComponents.ATTACK_RANGE,
                new net.minecraft.world.item.component.AttackRange(1.0f, 4.0f, 2.0f, 6.0f, 0.5f, 0.75f));
        component("piercing_weapon", net.minecraft.core.component.DataComponents.PIERCING_WEAPON,
                new net.minecraft.world.item.component.PiercingWeapon(false, true,
                        java.util.Optional.of(net.minecraft.sounds.SoundEvents.ARMOR_EQUIP_GENERIC), java.util.Optional.empty()));
        var stab = new net.minecraft.world.item.component.SwingAnimation(
                net.minecraft.world.item.SwingAnimationType.STAB, 10);
        component("attack_animation", net.minecraft.core.component.DataComponents.ATTACK_ANIMATION, stab);
        component("interact_animation", net.minecraft.core.component.DataComponents.INTERACT_ANIMATION, stab);
        component("repairable", net.minecraft.core.component.DataComponents.REPAIRABLE,
                new net.minecraft.world.item.enchantment.Repairable(net.minecraft.core.HolderSet.direct(
                        net.minecraft.world.item.Items.DIAMOND.builtInRegistryHolder(),
                        net.minecraft.world.item.Items.STICK.builtInRegistryHolder())));
        var sword = new net.minecraft.world.item.ItemStackTemplate(net.minecraft.world.item.Items.DIAMOND_SWORD,
                net.minecraft.core.component.DataComponentPatch.builder()
                        .set(net.minecraft.core.component.DataComponents.DAMAGE, 5)
                        .remove(net.minecraft.core.component.DataComponents.REPAIR_COST)
                        .build());
        component("use_remainder", net.minecraft.core.component.DataComponents.USE_REMAINDER,
                new net.minecraft.world.item.component.UseRemainder(
                        sword));
        component("death_protection", net.minecraft.core.component.DataComponents.DEATH_PROTECTION,
                new net.minecraft.world.item.component.DeathProtection(List.of(
                        new net.minecraft.world.item.consume_effects.TeleportRandomlyConsumeEffect(8.0f, false),
                        net.minecraft.world.item.consume_effects.ClearAllStatusEffectsConsumeEffect.INSTANCE)));
        component("suspicious_stew_effects", net.minecraft.core.component.DataComponents.SUSPICIOUS_STEW_EFFECTS,
                new net.minecraft.world.item.component.SuspiciousStewEffects(List.of(
                        new net.minecraft.world.item.component.SuspiciousStewEffects.Entry(
                                net.minecraft.world.effect.MobEffects.BLINDNESS, 160),
                        new net.minecraft.world.item.component.SuspiciousStewEffects.Entry(
                                net.minecraft.world.effect.MobEffects.SATURATION, 7))));
        component("writable_book_content", net.minecraft.core.component.DataComponents.WRITABLE_BOOK_CONTENT,
                new net.minecraft.world.item.component.WritableBookContent(List.of(
                        net.minecraft.server.network.Filterable.passThrough("one"),
                        new net.minecraft.server.network.Filterable<>("two", java.util.Optional.of("2")))));
        component("written_book_content", net.minecraft.core.component.DataComponents.WRITTEN_BOOK_CONTENT,
                new net.minecraft.world.item.component.WrittenBookContent(
                        net.minecraft.server.network.Filterable.passThrough("Title"), "Author", 2,
                        List.of(net.minecraft.server.network.Filterable.passThrough(
                                Component.literal("page").withStyle(ChatFormatting.BOLD))), true));
        component("fireworks", net.minecraft.core.component.DataComponents.FIREWORKS,
                new net.minecraft.world.item.component.Fireworks(2, List.of(
                        new net.minecraft.world.item.component.FireworkExplosion(
                                net.minecraft.world.item.component.FireworkExplosion.Shape.BURST,
                                it.unimi.dsi.fastutil.ints.IntList.of(5), it.unimi.dsi.fastutil.ints.IntList.of(),
                                false, true))));
        try {
            // Slots with a gap, which only the private constructor makes without a bound item.
            var constructor = net.minecraft.world.item.component.ItemContainerContents.class
                    .getDeclaredConstructor(List.class);
            constructor.setAccessible(true);
            component("container", net.minecraft.core.component.DataComponents.CONTAINER,
                    constructor.newInstance(List.of(java.util.Optional.empty(), java.util.Optional.of(sword),
                            java.util.Optional.of(new net.minecraft.world.item.ItemStackTemplate(
                                    net.minecraft.world.item.Items.DIRT, 3)))));
        } catch (ReflectiveOperationException e) {
            throw new IllegalStateException(e);
        }
        component("bundle_of_components", net.minecraft.core.component.DataComponents.BUNDLE_CONTENTS,
                new net.minecraft.world.item.component.BundleContents(List.of(
                        sword)));
        component("lodestone_tracker", net.minecraft.core.component.DataComponents.LODESTONE_TRACKER,
                new net.minecraft.world.item.component.LodestoneTracker(java.util.Optional.of(
                        net.minecraft.core.GlobalPos.of(net.minecraft.world.level.Level.NETHER,
                                new net.minecraft.core.BlockPos(-3, 70, 12))), false));
        component("fox_variant", net.minecraft.core.component.DataComponents.FOX_VARIANT,
                net.minecraft.world.entity.animal.fox.Fox.Variant.SNOW);
        component("rabbit_variant", net.minecraft.core.component.DataComponents.RABBIT_VARIANT,
                net.minecraft.world.entity.animal.rabbit.Rabbit.Variant.EVIL);
        component("tropical_fish_pattern", net.minecraft.core.component.DataComponents.TROPICAL_FISH_PATTERN,
                net.minecraft.world.entity.animal.fish.TropicalFish.Pattern.CLAYFISH);
        component("horse_variant", net.minecraft.core.component.DataComponents.HORSE_VARIANT,
                net.minecraft.world.entity.animal.equine.Variant.DARK_BROWN);
        component("axolotl_variant", net.minecraft.core.component.DataComponents.AXOLOTL_VARIANT,
                net.minecraft.world.entity.animal.axolotl.Axolotl.Variant.BLUE);
        component("salmon_size", net.minecraft.core.component.DataComponents.SALMON_SIZE,
                net.minecraft.world.entity.animal.fish.Salmon.Variant.LARGE);
        component("parrot_variant", net.minecraft.core.component.DataComponents.PARROT_VARIANT,
                net.minecraft.world.entity.animal.parrot.Parrot.Variant.YELLOW_BLUE);
        component("llama_variant", net.minecraft.core.component.DataComponents.LLAMA_VARIANT,
                net.minecraft.world.entity.animal.equine.Llama.Variant.GRAY);
        component("mooshroom_variant", net.minecraft.core.component.DataComponents.MOOSHROOM_VARIANT,
                net.minecraft.world.entity.animal.cow.MushroomCow.Variant.BROWN);
        net.minecraft.nbt.CompoundTag data = new net.minecraft.nbt.CompoundTag();
        data.putBoolean("NoAI", true);
        data.putLong("Seed", 9L);
        component("entity_data", net.minecraft.core.component.DataComponents.ENTITY_DATA,
                net.minecraft.world.item.component.TypedEntityData.of(
                        net.minecraft.world.entity.EntityTypes.PIG, data));
        component("bucket_entity_data", net.minecraft.core.component.DataComponents.BUCKET_ENTITY_DATA,
                net.minecraft.world.item.component.CustomData.of(data));
        component("container_loot", net.minecraft.core.component.DataComponents.CONTAINER_LOOT,
                new net.minecraft.world.item.component.SeededContainerLoot(
                        net.minecraft.resources.ResourceKey.create(
                                net.minecraft.core.registries.Registries.LOOT_TABLE, custom), 11L));
        component("block_entity_data", net.minecraft.core.component.DataComponents.BLOCK_ENTITY_DATA,
                net.minecraft.world.item.component.TypedEntityData.of(
                        net.minecraft.world.level.block.entity.BlockEntityTypes.CHEST, data));
        component("villager_variant", net.minecraft.core.component.DataComponents.VILLAGER_VARIANT,
                net.minecraft.core.registries.BuiltInRegistries.VILLAGER_TYPE.getOrThrow(
                        net.minecraft.world.entity.npc.villager.VillagerType.DESERT));
        component("map_decorations", net.minecraft.core.component.DataComponents.MAP_DECORATIONS,
                new net.minecraft.world.item.component.MapDecorations(java.util.Map.of("spot",
                        new net.minecraft.world.item.component.MapDecorations.Entry(
                                net.minecraft.world.level.saveddata.maps.MapDecorationTypes.RED_X, 1.5, -2.0,
                                90.0f))));
        var none = java.util.Optional.<net.minecraft.world.item.component.KineticWeapon.Condition>empty();
        component("kinetic_negative_zero", net.minecraft.core.component.DataComponents.KINETIC_WEAPON,
                new net.minecraft.world.item.component.KineticWeapon(10, 0, none, none, none, -0.0f, 1.0f,
                        java.util.Optional.empty(), java.util.Optional.empty()));
        component("sprite_name", net.minecraft.core.component.DataComponents.CUSTOM_NAME,
                Component.object(new net.minecraft.network.chat.contents.objects.PlayerSprite(
                        net.minecraft.world.item.component.ResolvableProfile.createUnresolved("Steve"), false)));
        component("item_name", net.minecraft.core.component.DataComponents.ITEM_NAME,
                Component.literal("a").withStyle(style -> style.withBold(true).withItalic(false)
                                .withHoverEvent(new net.minecraft.network.chat.HoverEvent.ShowText(
                                        Component.literal("tip").withStyle(ChatFormatting.UNDERLINE))))
                        .append(Component.translatable("b.key", "x", Component.literal("y").withStyle(
                                ChatFormatting.OBFUSCATED))));
    }

    private static <T> void component(String name, net.minecraft.core.component.DataComponentType<T> type, T value) {
        encode("component/" + name, type.streamCodec()::encode, value);
        // The hash a click sends for the value: HashOps over its CODEC.
        new net.minecraft.core.component.TypedDataComponent<>(type, value)
                .encodeValue(registries.createSerializationContext(net.minecraft.util.HashOps.CRC32C_INSTANCE))
                .ifSuccess(hash -> CASES.put("component_hash/" + name, String.format("%08x", hash.asInt())));
    }

    /// The entity packets pomme reads (`pomme-protocol/src/entity.rs`).
    private static void entities() throws Exception {
        Vec3 position = new Vec3(1.5, -60.0, 3.25);
        Vec3 movement = new Vec3(0.5, 0.25, -0.75);
        VecDelta delta = new VecDelta.Linear((short) 4096, (short) -2048, (short) 1);
        encode("add_entity/zombie", ClientboundAddEntityPacket.STREAM_CODEC::encode,
                new ClientboundAddEntityPacket(7, new UUID(0L, 42L),
                        position.x, position.y, position.z, -90.0f, 45.0f,
                        net.minecraft.world.entity.EntityTypes.ZOMBIE, 5, movement, 22.5));
        encode("move_entity_pos/on_ground",
                ClientboundMoveEntityPacket.Pos.STREAM_CODEC::encode,
                new ClientboundMoveEntityPacket.Pos(7, delta, true));
        encode("move_entity_pos_rot/airborne",
                ClientboundMoveEntityPacket.PosRot.STREAM_CODEC::encode,
                new ClientboundMoveEntityPacket.PosRot(7, delta, (byte) 64, (byte) -32, false));
        encode("move_entity_rot/on_ground",
                ClientboundMoveEntityPacket.Rot.STREAM_CODEC::encode,
                new ClientboundMoveEntityPacket.Rot(7, (byte) 64, (byte) -32, true));
        // Two steps, of one tick and then three.
        encode("move_entity_pos/stepped", ClientboundMoveEntityPacket.Pos.STREAM_CODEC::encode,
                new ClientboundMoveEntityPacket.Pos(7, new VecDelta.Stepped(List.of(
                        new VecDelta.Stepped.DeltaStep((short) 4096, (short) 0, (short) 0, 1),
                        new VecDelta.Stepped.DeltaStep((short) 0, (short) -2048, (short) 1, 3))), false));
        encode("entity_position_sync/stepped", ClientboundEntityPositionSyncPacket.STREAM_CODEC::encode,
                new ClientboundEntityPositionSyncPacket(7, PositionPath.stepped(List.of(
                        new net.minecraft.world.entity.PositionStep(movement, 1),
                        new net.minecraft.world.entity.PositionStep(position, 3))), 45.0f, -10.0f, false));
        encode("entity_position_sync/on_ground",
                ClientboundEntityPositionSyncPacket.STREAM_CODEC::encode,
                new ClientboundEntityPositionSyncPacket(
                        7, PositionPath.of(position), 45.0f, -10.0f, true));
        encode("set_entity_motion/fast",
                ClientboundSetEntityMotionPacket.STREAM_CODEC::encode,
                new ClientboundSetEntityMotionPacket(7, new Vec3(6.0, -1.0, 0.0)));
        encode("hurt_animation/behind",
                ClientboundHurtAnimationPacket.STREAM_CODEC::encode,
                new ClientboundHurtAnimationPacket(7, 135.0f));
        encode("damage_event/mob_attack",
                ClientboundDamageEventPacket.STREAM_CODEC::encode,
                new ClientboundDamageEventPacket(7,
                        registries.lookupOrThrow(Registries.DAMAGE_TYPE)
                                .getOrThrow(net.minecraft.world.damagesource.DamageTypes.MOB_ATTACK),
                        3, -1, Optional.of(position)));
        encode("take_item_entity/three",
                ClientboundTakeItemEntityPacket.STREAM_CODEC::encode,
                new ClientboundTakeItemEntityPacket(9, 1, 3));
        encode("remove_entities/two",
                ClientboundRemoveEntitiesPacket.STREAM_CODEC::encode,
                new ClientboundRemoveEntitiesPacket(5, 7));
        // These four are only built from a live entity, so their fields are set directly.
        encode("rotate_head/left",
                ClientboundRotateHeadPacket.STREAM_CODEC::encode,
                withFields(ClientboundRotateHeadPacket.class,
                        Map.of("entityId", 7, "yHeadRot", (byte) -96)));
        encode("animate/wake_up",
                ClientboundAnimatePacket.STREAM_CODEC::encode,
                withFields(ClientboundAnimatePacket.class,
                        Map.of("id", 7, "action", 2)));
        encode("entity_event/death",
                ClientboundEntityEventPacket.STREAM_CODEC::encode,
                withFields(ClientboundEntityEventPacket.class,
                        Map.of("entityId", 300, "eventId", (byte) 34)));
        encode("set_passengers/two",
                ClientboundSetPassengersPacket.STREAM_CODEC::encode,
                withFields(ClientboundSetPassengersPacket.class,
                        Map.of("vehicle", 7, "passengers", new int[] {8, 9})));
    }

    /// The player-state packets pomme reads (`pomme-protocol/src/player.rs`).
    private static void player() throws Exception {
        encode("set_health/hungry", ClientboundSetHealthPacket.STREAM_CODEC::encode,
                new ClientboundSetHealthPacket(15.5f, 18, 2.0f));
        encode("set_experience/level_30", ClientboundSetExperiencePacket.STREAM_CODEC::encode,
                new ClientboundSetExperiencePacket(0.25f, 1395, 30));
        encode("update_attributes/max_health", ClientboundUpdateAttributesPacket.STREAM_CODEC::encode,
                withFields(ClientboundUpdateAttributesPacket.class, Map.of("entityId", 9, "attributes", List.of(
                        new ClientboundUpdateAttributesPacket.AttributeSnapshot(
                                net.minecraft.world.entity.ai.attributes.Attributes.MAX_HEALTH, 20.0, List.of(
                                        new net.minecraft.world.entity.ai.attributes.AttributeModifier(
                                                Identifier.withDefaultNamespace("test"), 4.0,
                                                net.minecraft.world.entity.ai.attributes.AttributeModifier.Operation
                                                        .ADD_MULTIPLIED_TOTAL)))))));
        // Ambient, no particles, an icon.
        encode("update_mob_effect/haste", ClientboundUpdateMobEffectPacket.STREAM_CODEC::encode,
                new ClientboundUpdateMobEffectPacket(1, new net.minecraft.world.effect.MobEffectInstance(
                        net.minecraft.world.effect.MobEffects.HASTE, 200, 1, true, false, true), false));
        encode("remove_mob_effect/haste", ClientboundRemoveMobEffectPacket.STREAM_CODEC::encode,
                new ClientboundRemoveMobEffectPacket(1, net.minecraft.world.effect.MobEffects.HASTE));
        encode("set_held_slot/fifth", ClientboundSetHeldSlotPacket.STREAM_CODEC::encode,
                new ClientboundSetHeldSlotPacket(4));
        net.minecraft.world.entity.player.Abilities abilities = new net.minecraft.world.entity.player.Abilities();
        abilities.flying = true;
        abilities.mayfly = true;
        encode("player_abilities/flying", ClientboundPlayerAbilitiesPacket.STREAM_CODEC::encode,
                new ClientboundPlayerAbilitiesPacket(abilities));
        encode("game_event/thunder", ClientboundGameEventPacket.STREAM_CODEC::encode,
                new ClientboundGameEventPacket(ClientboundGameEventPacket.THUNDER_LEVEL_CHANGE, 0.5f));
        encode("player_combat_kill/fell", ClientboundPlayerCombatKillPacket.STREAM_CODEC::encode,
                new ClientboundPlayerCombatKillPacket(3, Component.literal("Tester fell")));
        encode("set_camera/other", ClientboundSetCameraPacket.STREAM_CODEC::encode,
                withFields(ClientboundSetCameraPacket.class, Map.of("cameraId", 300)));
    }

    private static BlockState chest() {
        return net.minecraft.world.level.block.Blocks.CHEST.defaultBlockState();
    }

    /// The block packets pomme reads (`pomme-protocol/src/block.rs`).
    private static void blocks() throws Exception {
        BlockPos pos = new BlockPos(-3, -60, 4);
        encode("block_update/chest", ClientboundBlockUpdatePacket.STREAM_CODEC::encode,
                new ClientboundBlockUpdatePacket(pos, chest()));
        // One block of section (-1, -4, 2), at (15, 3, 0) inside it.
        encode("section_blocks_update/one", ClientboundSectionBlocksUpdatePacket.STREAM_CODEC::encode,
                withFields(ClientboundSectionBlocksUpdatePacket.class, Map.of(
                        "sectionPos", net.minecraft.core.SectionPos.of(-1, -4, 2),
                        "positions", new short[] {(short) (15 << 8 | 0 << 4 | 3)},
                        "states", new BlockState[] {chest()})));
        encode("block_changed_ack/seventeen", ClientboundBlockChangedAckPacket.STREAM_CODEC::encode,
                new ClientboundBlockChangedAckPacket(17));
        encode("block_event/chest_lid", ClientboundBlockEventPacket.STREAM_CODEC::encode,
                new ClientboundBlockEventPacket(pos, net.minecraft.world.level.block.Blocks.CHEST, 1, 2));
        encode("block_entity_data/sign", ClientboundBlockEntityDataPacket.STREAM_CODEC::encode,
                withFields(ClientboundBlockEntityDataPacket.class, Map.of("pos", pos,
                        "type", net.minecraft.world.level.block.entity.BlockEntityTypes.SIGN, "tag", d5())));
        encode("level_event/global", ClientboundLevelEventPacket.STREAM_CODEC::encode,
                new ClientboundLevelEventPacket(2001, pos, 129, true));
    }

    /// The compound `{d: 5}`.
    private static net.minecraft.nbt.CompoundTag d5() {
        net.minecraft.nbt.CompoundTag tag = new net.minecraft.nbt.CompoundTag();
        tag.putInt("d", 5);
        return tag;
    }

    /// The chunk packets pomme reads (`pomme-protocol/src/chunk.rs`). A chunk
    /// packet is only built from a loaded chunk and its light engine, so
    /// these set its parts directly.
    private static void chunks() throws Exception {
        BitSet none = new BitSet();
        byte[] sky = new byte[2048];
        java.util.Arrays.fill(sky, (byte) 7);
        ClientboundLightUpdatePacketData light = withFields(ClientboundLightUpdatePacketData.class, Map.of(
                "skyYMask", BitSet.valueOf(new long[] {0b101L, 1L}),
                "blockYMask", BitSet.valueOf(new long[] {0b10L}),
                "emptySkyYMask", none,
                "emptyBlockYMask", BitSet.valueOf(new long[] {1L << 40}),
                "skyUpdates", List.of(sky),
                "blockUpdates", List.of()));
        Map<Heightmap.Types, long[]> heightmaps =
                new java.util.EnumMap<>(Heightmap.Types.class);
        heightmaps.put(Heightmap.Types.WORLD_SURFACE, new long[] {7L});
        heightmaps.put(Heightmap.Types.MOTION_BLOCKING, new long[] {8L, 9L});
        Object blockEntity = withFields(Class.forName(ClientboundLevelChunkPacketData.class.getName()
                + "$BlockEntityInfo"), Map.of("packedXZ", (byte) 0x59, "y", (short) -64,
                        "type", net.minecraft.world.level.block.entity.BlockEntityTypes.SIGN,
                        "tag", Optional.of(d5())));
        encode("level_chunk_with_light/small", ClientboundLevelChunkWithLightPacket.STREAM_CODEC::encode,
                withFields(ClientboundLevelChunkWithLightPacket.class, Map.of("x", 3, "z", -2,
                        "chunkData", withFields(ClientboundLevelChunkPacketData.class, Map.of(
                                "heightmaps", heightmaps, "buffer", new byte[] {1, 2, 3},
                                "blockEntitiesData", List.of(blockEntity))),
                        "lightData", light)));
        encode("light_update/sky", ClientboundLightUpdatePacket.STREAM_CODEC::encode,
                withFields(ClientboundLightUpdatePacket.class, Map.of("x", -5, "z", 6, "lightData", light)));
        encode("forget_level_chunk/negative_x", ClientboundForgetLevelChunkPacket.STREAM_CODEC::encode,
                new ClientboundForgetLevelChunkPacket(new ChunkPos(-3, 7)));
        encode("chunk_batch_start/empty", ClientboundChunkBatchStartPacket.STREAM_CODEC::encode,
                ClientboundChunkBatchStartPacket.INSTANCE);
        encode("chunk_batch_finished/sixty_four", ClientboundChunkBatchFinishedPacket.STREAM_CODEC::encode,
                new ClientboundChunkBatchFinishedPacket(64));
        encode("set_chunk_cache_center/negative_x", ClientboundSetChunkCacheCenterPacket.STREAM_CODEC::encode,
                new ClientboundSetChunkCacheCenterPacket(-1, 9));
        encode("set_chunk_cache_radius/twelve", ClientboundSetChunkCacheRadiusPacket.STREAM_CODEC::encode,
                new ClientboundSetChunkCacheRadiusPacket(12));
    }

    /// `count` of `item` with no components of its own. A stack reads its
    /// item's default components, which only a loaded world binds.
    private static ItemStack stack(Item item, int count) {
        item.builtInRegistryHolder().bindComponents(net.minecraft.core.component.DataComponentMap.EMPTY);
        return new ItemStack(item, count);
    }

    /// The container packets pomme reads (`pomme-protocol/src/container.rs`).
    private static void containers() {
        encode("open_screen/chest", ClientboundOpenScreenPacket.STREAM_CODEC::encode,
                new ClientboundOpenScreenPacket(3, net.minecraft.world.inventory.MenuType.GENERIC_9x3,
                        Component.literal("Chest")));
        encode("container_set_content/two_slots", ClientboundContainerSetContentPacket.STREAM_CODEC::encode,
                new ClientboundContainerSetContentPacket(2, 7,
                        List.of(stack(Items.DIAMOND, 3), ItemStack.EMPTY), stack(Items.STONE, 1)));
        encode("container_set_slot/hotbar", ClientboundContainerSetSlotPacket.STREAM_CODEC::encode,
                new ClientboundContainerSetSlotPacket(0, 4, 36, stack(Items.BREAD, 2)));
        encode("container_set_data/furnace", ClientboundContainerSetDataPacket.STREAM_CODEC::encode,
                new ClientboundContainerSetDataPacket(3, 1, 200));
        encode("container_close/third", ClientboundContainerClosePacket.STREAM_CODEC::encode,
                new ClientboundContainerClosePacket(3));
        encode("set_cursor_item/stone", ClientboundSetCursorItemPacket.STREAM_CODEC::encode,
                new ClientboundSetCursorItemPacket(stack(Items.STONE, 64)));
        encode("set_player_inventory/offhand", ClientboundSetPlayerInventoryPacket.STREAM_CODEC::encode,
                new ClientboundSetPlayerInventoryPacket(40, stack(Items.SHIELD, 1)));
        encode("set_equipment/sword_and_bare_head", ClientboundSetEquipmentPacket.STREAM_CODEC::encode,
                new ClientboundSetEquipmentPacket(9, List.of(
                        Pair.of(EquipmentSlot.MAINHAND, stack(Items.STONE_SWORD, 1)),
                        Pair.of(EquipmentSlot.HEAD, ItemStack.EMPTY))));
    }

    /// An `update_advancements` (`pomme-client/src/net/advancement_wire.rs`):
    /// a root with a display, a child without one, a removal and progress.
    private static void advancements() throws Exception {
        Identifier root = Identifier.withDefaultNamespace("story/root");
        DisplayInfo display = new DisplayInfo(new net.minecraft.world.item.ItemStackTemplate(Items.STONE),
                Component.literal("hi"), Component.literal("there"),
                Optional.of(new net.minecraft.core.ClientAsset.ResourceTexture(
                        Identifier.withDefaultNamespace("block/stone"))),
                AdvancementType.GOAL, true, false, false);
        // A granted criterion is stamped with the time it was granted, so
        // these are built with a fixed one, in a map that keeps its order.
        Map<String, CriterionProgress> criteria = new LinkedHashMap<>();
        criteria.put("a", new CriterionProgress(Instant.ofEpochMilli(5)));
        criteria.put("b", new CriterionProgress());
        var progress = AdvancementProgress.class.getDeclaredConstructor(Map.class);
        progress.setAccessible(true);
        encode("update_advancements/root_and_child", ClientboundUpdateAdvancementsPacket.STREAM_CODEC::encode,
                new ClientboundUpdateAdvancementsPacket(true, List.of(
                        advancement(root, Optional.empty(), Optional.of(display), List.of(List.of("a")), 1.5f),
                        advancement(Identifier.withDefaultNamespace("story/child"), Optional.of(root),
                                Optional.empty(), List.of(List.of("a", "b"), List.of("c")), 3.0f)),
                        Set.of(Identifier.withDefaultNamespace("old")),
                        Map.of(root, progress.newInstance(criteria)), true));
    }

    /// An advancement at `x` across its screen and two rows up.
    private static ClientboundUpdateAdvancementsPacket.PositionedAdvancement advancement(Identifier id,
            Optional<Identifier> parent, Optional<DisplayInfo> display, List<List<String>> requirements, float x) {
        return new ClientboundUpdateAdvancementsPacket.PositionedAdvancement(new AdvancementHolder(id,
                new Advancement(parent, display, AdvancementRewards.EMPTY, Map.of(),
                        new AdvancementRequirements(requirements), false)), x, -2.0f);
    }

    /// The title, tab list and boss bar packets (`pomme-protocol/src/hud.rs`).
    private static void hud() {
        encode("set_title_text/hello", ClientboundSetTitleTextPacket.STREAM_CODEC::encode,
                new ClientboundSetTitleTextPacket(Component.literal("Hello")));
        encode("set_subtitle_text/sub", ClientboundSetSubtitleTextPacket.STREAM_CODEC::encode,
                new ClientboundSetSubtitleTextPacket(Component.literal("sub")));
        encode("set_titles_animation/slow", ClientboundSetTitlesAnimationPacket.STREAM_CODEC::encode,
                new ClientboundSetTitlesAnimationPacket(10, 70, 20));
        encode("clear_titles/reset", ClientboundClearTitlesPacket.STREAM_CODEC::encode,
                new ClientboundClearTitlesPacket(true));
        encode("tab_list/both", ClientboundTabListPacket.STREAM_CODEC::encode,
                new ClientboundTabListPacket(Component.literal("head"), Component.literal("foot")));
        BossEvent boss = new BossEvent(new UUID(0L, 7L), Component.literal("Boss"), BossEvent.BossBarColor.RED,
                BossEvent.BossBarOverlay.NOTCHED_10) {};
        boss.setProgress(0.5f);
        boss.setDarkenScreen(true).setCreateWorldFog(true);
        bossEvent("add", ClientboundBossEventPacket.createAddPacket(boss));
        bossEvent("remove", ClientboundBossEventPacket.createRemovePacket(boss.getId()));
        bossEvent("progress", ClientboundBossEventPacket.createUpdateProgressPacket(boss));
        bossEvent("name", ClientboundBossEventPacket.createUpdateNamePacket(boss));
        bossEvent("style", ClientboundBossEventPacket.createUpdateStylePacket(boss));
        bossEvent("properties", ClientboundBossEventPacket.createUpdatePropertiesPacket(boss));
    }

    private static void bossEvent(String operation, ClientboundBossEventPacket packet) {
        encode("boss_event/" + operation, ClientboundBossEventPacket.STREAM_CODEC::encode, packet);
    }

    /// The scoreboard packets (`pomme-protocol/src/scoreboard.rs`).
    private static void scoreboard() {
        Scoreboard board = new Scoreboard();
        Objective kills = board.addObjective("kills", ObjectiveCriteria.DUMMY, Component.literal("Kills"),
                ObjectiveCriteria.RenderType.HEARTS, false, new StyledFormat(Style.EMPTY.withColor(ChatFormatting.RED)));
        Objective deaths = board.addObjective("deaths", ObjectiveCriteria.DUMMY, Component.literal("Deaths"),
                ObjectiveCriteria.RenderType.INTEGER, false, null);
        objective("add_styled", kills, ClientboundSetObjectivePacket.METHOD_ADD);
        objective("remove", kills, ClientboundSetObjectivePacket.METHOD_REMOVE);
        objective("change_plain", deaths, ClientboundSetObjectivePacket.METHOD_CHANGE);
        encode("set_display_objective/sidebar", ClientboundSetDisplayObjectivePacket.STREAM_CODEC::encode,
                new ClientboundSetDisplayObjectivePacket(DisplaySlot.SIDEBAR, kills));
        encode("set_display_objective/clear_list", ClientboundSetDisplayObjectivePacket.STREAM_CODEC::encode,
                new ClientboundSetDisplayObjectivePacket(DisplaySlot.LIST, null));
        score("plain", 9, Optional.empty(), Optional.empty());
        score("blank", 9, Optional.empty(), Optional.of(BlankFormat.INSTANCE));
        score("named_fixed", 5, Optional.of(Component.literal("Hero")),
                Optional.of(new FixedFormat(Component.literal("x"))));
        encode("reset_score/all", ClientboundResetScorePacket.STREAM_CODEC::encode,
                new ClientboundResetScorePacket("Bob", null));
        encode("reset_score/kills", ClientboundResetScorePacket.STREAM_CODEC::encode,
                new ClientboundResetScorePacket("Bob", "kills"));
        PlayerTeam team = board.addPlayerTeam("red");
        team.setDisplayName(Component.literal("Red"));
        team.setPlayerPrefix(Component.literal("["));
        team.setPlayerSuffix(Component.literal("]"));
        team.setNameTagVisibility(Team.Visibility.HIDE_FOR_OTHER_TEAMS);
        team.setCollisionRule(Team.CollisionRule.NEVER);
        team.setColor(Optional.of(TeamColor.RED));
        team.setAllowFriendlyFire(false);
        board.addPlayerToTeam("Bob", team);
        team("add", ClientboundSetPlayerTeamPacket.createAddOrModifyPacket(team, true));
        team("remove", ClientboundSetPlayerTeamPacket.createRemovePacket(team));
        team.setColor(Optional.empty());
        team("change_uncoloured", ClientboundSetPlayerTeamPacket.createAddOrModifyPacket(team, false));
        team("join", ClientboundSetPlayerTeamPacket.createPlayerPacket(team, "Amy",
                ClientboundSetPlayerTeamPacket.Action.ADD));
        team("leave", ClientboundSetPlayerTeamPacket.createPlayerPacket(team, "Amy",
                ClientboundSetPlayerTeamPacket.Action.REMOVE));
    }

    private static void objective(String name, Objective objective, int method) {
        encode("set_objective/" + name, ClientboundSetObjectivePacket.STREAM_CODEC::encode,
                new ClientboundSetObjectivePacket(objective, method));
    }

    /// Bob's score for `kills`.
    private static void score(String name, int score, Optional<Component> display, Optional<NumberFormat> format) {
        encode("set_score/" + name, ClientboundSetScorePacket.STREAM_CODEC::encode,
                new ClientboundSetScorePacket("Bob", "kills", score, display, format));
    }

    private static void team(String name, ClientboundSetPlayerTeamPacket packet) {
        encode("set_player_team/" + name, ClientboundSetPlayerTeamPacket.STREAM_CODEC::encode, packet);
    }

    /// The player list packets (`pomme-protocol/src/player_info.rs`). An
    /// update is only built from players on a server, so these set its parts
    /// directly.
    private static void playerList() throws Exception {
        UUID bob = new UUID(0L, 9L);
        var profile = new com.mojang.authlib.GameProfile(bob, "bob", textures("abc", "sig"));
        var session = chatSession(new UUID(0L, 7L), 1000, new byte[] {(byte) 0xAA, (byte) 0xBB},
                new byte[] {(byte) 0xCC});
        playerInfo("every_action", EnumSet.allOf(ClientboundPlayerInfoUpdatePacket.Action.class),
                new ClientboundPlayerInfoUpdatePacket.Entry(bob, profile, true, 42, GameType.CREATIVE,
                        Component.literal("Bob"), true, 3, session));
        playerInfo("cleared_name_and_session",
                EnumSet.of(ClientboundPlayerInfoUpdatePacket.Action.INITIALIZE_CHAT,
                        ClientboundPlayerInfoUpdatePacket.Action.UPDATE_DISPLAY_NAME),
                new ClientboundPlayerInfoUpdatePacket.Entry(bob, null, false, 0, GameType.SURVIVAL, null, false,
                        0, null));
        encode("player_info_remove/two", ClientboundPlayerInfoRemovePacket.STREAM_CODEC::encode,
                new ClientboundPlayerInfoRemovePacket(List.of(new UUID(0L, 1L), new UUID(0L, 2L))));
    }

    /// A chat session whose key encodes as `key`. A real key would differ from
    /// run to run, and only its bytes are sent.
    private static net.minecraft.network.chat.RemoteChatSession.Data chatSession(UUID id, long expiresAt,
            byte[] key, byte[] keySignature) {
        java.security.PublicKey encoded = new java.security.PublicKey() {
            @Override
            public String getAlgorithm() {
                return "RSA";
            }

            @Override
            public String getFormat() {
                return "X.509";
            }

            @Override
            public byte[] getEncoded() {
                return key;
            }
        };
        return new net.minecraft.network.chat.RemoteChatSession.Data(id,
                new net.minecraft.world.entity.player.ProfilePublicKey.Data(Instant.ofEpochMilli(expiresAt), encoded,
                        keySignature));
    }

    /// A message signature of 256 bytes of `fill`.
    private static net.minecraft.network.chat.MessageSignature signature(int fill) {
        byte[] bytes = new byte[256];
        java.util.Arrays.fill(bytes, (byte) fill);
        return new net.minecraft.network.chat.MessageSignature(bytes);
    }

    /// A profile's properties: a signed `textures`.
    private static com.mojang.authlib.properties.PropertyMap textures(String value, String signature) {
        return new com.mojang.authlib.properties.PropertyMap(com.google.common.collect.ImmutableMultimap.of(
                "textures", new com.mojang.authlib.properties.Property("textures", value, signature)));
    }

    private static void playerInfo(String name, EnumSet<ClientboundPlayerInfoUpdatePacket.Action> actions,
            ClientboundPlayerInfoUpdatePacket.Entry entry) throws Exception {
        encode("player_info_update/" + name, ClientboundPlayerInfoUpdatePacket.STREAM_CODEC::encode,
                withFields(ClientboundPlayerInfoUpdatePacket.class,
                        Map.of("actions", actions, "entries", List.of(entry))));
    }

    /// The sound packets (`pomme-protocol/src/sound.rs`).
    private static void sounds() throws Exception {
        Identifier test = Identifier.withDefaultNamespace("test.sound");
        encode("sound/explode", ClientboundSoundPacket.STREAM_CODEC::encode,
                new ClientboundSoundPacket(SoundEvents.GENERIC_EXPLODE, SoundSource.BLOCKS, -1.0, 65.0, 2.0, 0.5f,
                        2.0f, 7L));
        encode("sound/fixed_range", ClientboundSoundPacket.STREAM_CODEC::encode,
                new ClientboundSoundPacket(Holder.direct(SoundEvent.createFixedRangeEvent(test, 16.0f)),
                        SoundSource.MASTER, 0.0, 0.0, 0.0, 1.0f, 1.0f, 0L));
        // An entity's sound is only built from the entity.
        encode("sound_entity/direct", ClientboundSoundEntityPacket.STREAM_CODEC::encode,
                withFields(ClientboundSoundEntityPacket.class, Map.of(
                        "sound", Holder.direct(SoundEvent.createVariableRangeEvent(test)),
                        "source", SoundSource.VOICE, "id", 9, "volume", 1.0f, "pitch", 1.0f, "seed", 3L)));
        encode("stop_sound/music", ClientboundStopSoundPacket.STREAM_CODEC::encode,
                new ClientboundStopSoundPacket(Identifier.withDefaultNamespace("music.game"), SoundSource.MUSIC));
        // An end rod with a speed per axis, and a dust, whose options sit
        // between its type and the rest.
        encode("level_particles/end_rod", ClientboundLevelParticlesPacket.STREAM_CODEC::encode,
                new ClientboundLevelParticlesPacket(net.minecraft.core.particles.ParticleTypes.END_ROD, true, false,
                        1.5, 64.0, -2.25, 0.5f, 0.25f, 0.125f, 0.1f, 0.2f, 0.3f, 7,
                        ClientboundLevelParticlesPacket.RandomizationType.DEFAULT));
        encode("level_particles/dust", ClientboundLevelParticlesPacket.STREAM_CODEC::encode,
                new ClientboundLevelParticlesPacket(
                        new net.minecraft.core.particles.DustParticleOptions(0xff0000, 2.0f), false, false,
                        1.5, 64.0, -2.25, 0.5f, 0.25f, 0.125f, 0.1f, 3));
        encode("stop_sound/all", ClientboundStopSoundPacket.STREAM_CODEC::encode,
                new ClientboundStopSoundPacket(null, null));
    }

    /// An instance of `type` with `fields` set, for a packet the game only
    /// builds from a live world. A record goes through its canonical
    /// constructor, every component given; any other class skips its
    /// constructor and has its final fields written, which a later JDK may
    /// stop reflection from doing.
    private static <P> P withFields(Class<P> type, Map<String, Object> fields) throws Exception {
        if (type.isRecord()) {
            java.lang.reflect.RecordComponent[] components = type.getRecordComponents();
            Class<?>[] types = new Class<?>[components.length];
            Object[] values = new Object[components.length];
            for (int i = 0; i < components.length; i++) {
                types[i] = components[i].getType();
                values[i] = fields.get(components[i].getName());
            }
            var constructor = type.getDeclaredConstructor(types);
            constructor.setAccessible(true);
            return constructor.newInstance(values);
        }
        java.lang.reflect.Field theUnsafe = sun.misc.Unsafe.class.getDeclaredField("theUnsafe");
        theUnsafe.setAccessible(true);
        P packet = type.cast(((sun.misc.Unsafe) theUnsafe.get(null)).allocateInstance(type));
        for (Map.Entry<String, Object> field : fields.entrySet()) {
            java.lang.reflect.Field target = type.getDeclaredField(field.getKey());
            target.setAccessible(true);
            target.set(packet, field.getValue());
        }
        return packet;
    }

    /// One `set_entity_data` entry per serializer, each at the index of its
    /// serializer's id.
    private static void entityData() {
        List<SynchedEntityData.DataValue<?>> values = new java.util.ArrayList<>();
        BlockPos pos = new BlockPos(-12, 70, 345);
        Component text = Component.literal("hi");
        data(values, EntityDataSerializers.BYTE, (byte) 5);
        data(values, EntityDataSerializers.INT, 300);
        data(values, EntityDataSerializers.LONG, 127L);
        data(values, EntityDataSerializers.FLOAT, 1.5f);
        data(values, EntityDataSerializers.STRING, "hi");
        data(values, EntityDataSerializers.COMPONENT, text);
        data(values, EntityDataSerializers.OPTIONAL_COMPONENT, Optional.of(text));
        data(values, EntityDataSerializers.ITEM_STACK, stack(Items.STONE, 2));
        data(values, EntityDataSerializers.BOOLEAN, true);
        data(values, EntityDataSerializers.ROTATIONS, new net.minecraft.core.Rotations(1.0f, 2.0f, 3.0f));
        data(values, EntityDataSerializers.BLOCK_POS, pos);
        data(values, EntityDataSerializers.OPTIONAL_BLOCK_POS, Optional.of(pos));
        data(values, EntityDataSerializers.DIRECTION, net.minecraft.core.Direction.NORTH);
        data(values, EntityDataSerializers.OPTIONAL_LIVING_ENTITY_REFERENCE,
                Optional.of(net.minecraft.world.entity.EntityReference.of(new UUID(7L, 7L))));
        data(values, EntityDataSerializers.BLOCK_STATE,
                net.minecraft.world.level.block.Blocks.STONE.defaultBlockState());
        data(values, EntityDataSerializers.OPTIONAL_BLOCK_STATE, Optional.empty());
        data(values, EntityDataSerializers.PARTICLE, net.minecraft.core.particles.ParticleTypes.FLAME);
        data(values, EntityDataSerializers.PARTICLES, List.of(net.minecraft.core.particles.ParticleTypes.FLAME,
                new net.minecraft.core.particles.DustParticleOptions(0xFF0000, 1.0f)));
        data(values, EntityDataSerializers.VILLAGER_DATA, new net.minecraft.world.entity.npc.villager.VillagerData(
                net.minecraft.core.registries.BuiltInRegistries.VILLAGER_TYPE
                        .getOrThrow(net.minecraft.world.entity.npc.villager.VillagerType.SWAMP),
                net.minecraft.core.registries.BuiltInRegistries.VILLAGER_PROFESSION
                        .getOrThrow(net.minecraft.world.entity.npc.villager.VillagerProfession.FARMER),
                3));
        data(values, EntityDataSerializers.OPTIONAL_UNSIGNED_INT, java.util.OptionalInt.of(4));
        data(values, EntityDataSerializers.POSE, net.minecraft.world.entity.Pose.SLEEPING);
        data(values, EntityDataSerializers.CAT_VARIANT, last(Registries.CAT_VARIANT));
        data(values, EntityDataSerializers.CAT_SOUND_VARIANT, last(Registries.CAT_SOUND_VARIANT));
        data(values, EntityDataSerializers.COW_VARIANT, last(Registries.COW_VARIANT));
        data(values, EntityDataSerializers.COW_SOUND_VARIANT, last(Registries.COW_SOUND_VARIANT));
        data(values, EntityDataSerializers.WOLF_VARIANT, last(Registries.WOLF_VARIANT));
        data(values, EntityDataSerializers.WOLF_SOUND_VARIANT, last(Registries.WOLF_SOUND_VARIANT));
        data(values, EntityDataSerializers.FROG_VARIANT, last(Registries.FROG_VARIANT));
        data(values, EntityDataSerializers.PIG_VARIANT, last(Registries.PIG_VARIANT));
        data(values, EntityDataSerializers.PIG_SOUND_VARIANT, last(Registries.PIG_SOUND_VARIANT));
        data(values, EntityDataSerializers.CHICKEN_VARIANT, last(Registries.CHICKEN_VARIANT));
        data(values, EntityDataSerializers.CHICKEN_SOUND_VARIANT, last(Registries.CHICKEN_SOUND_VARIANT));
        data(values, EntityDataSerializers.ZOMBIE_NAUTILUS_VARIANT, last(Registries.ZOMBIE_NAUTILUS_VARIANT));
        data(values, EntityDataSerializers.OPTIONAL_GLOBAL_POS, Optional.of(GlobalPos.of(Level.OVERWORLD, pos)));
        data(values, EntityDataSerializers.PAINTING_VARIANT, last(Registries.PAINTING_VARIANT));
        data(values, EntityDataSerializers.SNIFFER_STATE, Sniffer.State.DIGGING);
        data(values, EntityDataSerializers.ARMADILLO_STATE, Armadillo.ArmadilloState.UNROLLING);
        data(values, EntityDataSerializers.COPPER_GOLEM_STATE, CopperGolemState.DROPPING_ITEM);
        data(values, EntityDataSerializers.WEATHERING_COPPER_STATE, WeatheringCopper.WeatherState.WEATHERED);
        data(values, EntityDataSerializers.VECTOR3, new org.joml.Vector3f(1.0f, 2.0f, 3.0f));
        data(values, EntityDataSerializers.QUATERNION, new org.joml.Quaternionf());
        data(values, EntityDataSerializers.RESOLVABLE_PROFILE,
                net.minecraft.world.item.component.ResolvableProfile.createUnresolved("Steve"));
        data(values, EntityDataSerializers.HUMANOID_ARM, net.minecraft.world.entity.HumanoidArm.RIGHT);
        data(values, EntityDataSerializers.DYE_COLOR, net.minecraft.world.item.DyeColor.RED);
        for (SynchedEntityData.DataValue<?> value : values) {
            if (EntityDataSerializers.getSerializedId(value.serializer()) != value.id()) {
                throw new IllegalStateException("serializer " + value.id() + " is out of order");
            }
        }
        encode("set_entity_data/every_serializer", ClientboundSetEntityDataPacket.STREAM_CODEC::encode,
                new ClientboundSetEntityDataPacket(42, values));
    }

    private static <T> void data(List<SynchedEntityData.DataValue<?>> values, EntityDataSerializer<T> serializer,
            T value) {
        values.add(SynchedEntityData.DataValue.create(new EntityDataAccessor<>(values.size(), serializer), value));
    }

    /// The entry of `registry` with the highest id.
    private static <T> Holder<T> last(ResourceKey<Registry<T>> registry) {
        Registry<T> entries = registries.lookupOrThrow(registry);
        return entries.get(entries.size() - 1).orElseThrow();
    }

    private static <P> void encode(String name, BiConsumer<RegistryFriendlyByteBuf, P> codec, P packet) {
        RegistryFriendlyByteBuf buf = new RegistryFriendlyByteBuf(Unpooled.buffer(), registries);
        codec.accept(buf, packet);
        CASES.put(name, ByteBufUtil.hexDump(buf));
    }
}
