import com.google.gson.GsonBuilder;
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
import com.mojang.serialization.Lifecycle;
import io.netty.buffer.ByteBufUtil;
import io.netty.buffer.Unpooled;
import it.unimi.dsi.fastutil.ints.IntList;
import java.nio.file.Files;
import java.nio.file.Path;
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
import net.minecraft.core.RegistrationInfo;
import net.minecraft.core.Registry;
import net.minecraft.core.RegistryAccess;
import net.minecraft.core.Vec3i;
import net.minecraft.core.registries.Registries;
import net.minecraft.data.registries.VanillaRegistries;
import net.minecraft.network.FriendlyByteBuf;
import net.minecraft.network.RegistryFriendlyByteBuf;
import net.minecraft.network.chat.Component;
import net.minecraft.network.protocol.common.ClientboundDisconnectPacket;
import net.minecraft.network.protocol.common.ClientboundKeepAlivePacket;
import net.minecraft.network.protocol.common.ClientboundPingPacket;
import net.minecraft.network.protocol.common.ClientboundResourcePackPopPacket;
import net.minecraft.network.protocol.common.ClientboundResourcePackPushPacket;
import net.minecraft.network.protocol.common.ClientboundShowDialogPacket;
import net.minecraft.network.protocol.common.ClientboundUpdateTagsPacket;
import net.minecraft.network.protocol.game.ClientboundAddEntityPacket;
import net.minecraft.network.protocol.game.ClientboundAnimatePacket;
import net.minecraft.network.protocol.game.ClientboundChunksBiomesPacket;
import net.minecraft.network.protocol.game.ClientboundCommandsPacket;
import net.minecraft.network.protocol.game.ClientboundCooldownPacket;
import net.minecraft.network.protocol.game.ClientboundDamageEventPacket;
import net.minecraft.network.protocol.game.ClientboundEntityEventPacket;
import net.minecraft.network.protocol.game.ClientboundEntityPositionSyncPacket;
import net.minecraft.network.protocol.game.ClientboundHurtAnimationPacket;
import net.minecraft.network.protocol.game.ClientboundInitializeBorderPacket;
import net.minecraft.network.protocol.game.ClientboundLoginPacket;
import net.minecraft.network.protocol.game.ClientboundMoveEntityPacket;
import net.minecraft.network.protocol.game.ClientboundMoveVehiclePacket;
import net.minecraft.network.protocol.game.ClientboundOpenSignEditorPacket;
import net.minecraft.network.protocol.game.ClientboundPlayerPositionPacket;
import net.minecraft.network.protocol.game.ClientboundPlayerRotationPacket;
import net.minecraft.network.protocol.game.ClientboundRemoveEntitiesPacket;
import net.minecraft.network.protocol.game.ClientboundRespawnPacket;
import net.minecraft.network.protocol.game.ClientboundRotateHeadPacket;
import net.minecraft.network.protocol.game.ClientboundSetBorderCenterPacket;
import net.minecraft.network.protocol.game.ClientboundSetBorderLerpSizePacket;
import net.minecraft.network.protocol.game.ClientboundSetBorderSizePacket;
import net.minecraft.network.protocol.game.ClientboundSetBorderWarningDelayPacket;
import net.minecraft.network.protocol.game.ClientboundSetBorderWarningDistancePacket;
import net.minecraft.network.protocol.game.ClientboundSetEntityDataPacket;
import net.minecraft.network.protocol.game.ClientboundSetEntityMotionPacket;
import net.minecraft.network.protocol.game.ClientboundSetPassengersPacket;
import net.minecraft.network.protocol.game.ClientboundSetSimulationDistancePacket;
import net.minecraft.network.protocol.game.ClientboundSetTimePacket;
import net.minecraft.network.protocol.game.ClientboundTakeItemEntityPacket;
import net.minecraft.network.protocol.game.ClientboundTeleportEntityPacket;
import net.minecraft.network.protocol.game.ClientboundTrackedWaypointPacket;
import net.minecraft.network.protocol.game.CommonPlayerSpawnInfo;
import net.minecraft.network.protocol.game.ServerboundAcceptTeleportationPacket;
import net.minecraft.network.protocol.game.ServerboundChangeGameModePacket;
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
import net.minecraft.network.protocol.game.ServerboundRenameItemPacket;
import net.minecraft.network.protocol.game.ServerboundSelectBundleItemPacket;
import net.minecraft.network.protocol.game.ServerboundSetCarriedItemPacket;
import net.minecraft.network.protocol.game.ServerboundSignUpdatePacket;
import net.minecraft.network.protocol.game.ServerboundSwingPacket;
import net.minecraft.network.protocol.game.ServerboundTeleportToEntityPacket;
import net.minecraft.network.protocol.game.ServerboundUseItemOnPacket;
import net.minecraft.network.protocol.game.ServerboundUseItemPacket;
import net.minecraft.network.syncher.EntityDataAccessor;
import net.minecraft.network.syncher.EntityDataSerializer;
import net.minecraft.network.syncher.EntityDataSerializers;
import net.minecraft.network.syncher.SynchedEntityData;
import net.minecraft.resources.Identifier;
import net.minecraft.resources.ResourceKey;
import net.minecraft.server.Bootstrap;
import net.minecraft.server.dialog.Dialog;
import net.minecraft.server.dialog.Dialogs;
import net.minecraft.tags.TagNetworkSerialization;
import net.minecraft.world.clock.ClockNetworkState;
import net.minecraft.world.clock.WorldClock;
import net.minecraft.world.clock.WorldClocks;
import net.minecraft.world.entity.PositionMoveRotation;
import net.minecraft.world.entity.Relative;
import net.minecraft.world.entity.animal.armadillo.Armadillo;
import net.minecraft.world.entity.animal.golem.CopperGolemState;
import net.minecraft.world.entity.animal.sniffer.Sniffer;
import net.minecraft.world.item.equipment.trim.TrimPattern;
import net.minecraft.world.level.ChunkPos;
import net.minecraft.world.level.GameType;
import net.minecraft.world.level.Level;
import net.minecraft.world.level.biome.Biome;
import net.minecraft.world.level.border.WorldBorder;
import net.minecraft.world.level.block.Block;
import net.minecraft.world.level.block.WeatheringCopper;
import net.minecraft.world.level.block.state.BlockState;
import net.minecraft.world.level.chunk.LevelChunkSection;
import net.minecraft.world.level.chunk.PalettedContainer;
import net.minecraft.world.level.chunk.PalettedContainerFactory;
import net.minecraft.world.level.dimension.BuiltinDimensionTypes;
import net.minecraft.world.level.dimension.DimensionType;
import net.minecraft.world.phys.Vec3;
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
                new ServerboundMoveVehiclePacket(new Vec3(1.5, 64.0, -2.25), 90.0f, 10.0f, true));
        encode("paddle_boat", ServerboundPaddleBoatPacket.STREAM_CODEC::encode,
                new ServerboundPaddleBoatPacket(true, false));
        encode("cooldown/shield", ClientboundCooldownPacket.STREAM_CODEC::encode,
                new ClientboundCooldownPacket(Identifier.withDefaultNamespace("shield"), 100));
        encode("move_vehicle/clientbound", ClientboundMoveVehiclePacket.STREAM_CODEC::encode,
                new ClientboundMoveVehiclePacket(new Vec3(1.5, 64.0, -2.25), 90.0f, -10.0f));
        encode("open_sign_editor/back", ClientboundOpenSignEditorPacket.STREAM_CODEC::encode,
                new ClientboundOpenSignEditorPacket(new BlockPos(-12, 70, 345), false));
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
        Holder<Dialog> serverLinks = dialogs.getOrThrow(Dialogs.SERVER_LINKS);
        encode("show_dialog/reference", ClientboundShowDialogPacket.STREAM_CODEC::encode,
                new ClientboundShowDialogPacket(serverLinks));
        encode("show_dialog/inline", ClientboundShowDialogPacket.STREAM_CODEC::encode,
                new ClientboundShowDialogPacket(Holder.direct(serverLinks.value())));
        commands();
        serverbound();
        entities();
        entityData();
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
        JsonObject ids = new JsonObject();
        idsOf(ids, "dimension_type", dimensionTypes);
        idsOf(ids, "world_clock", clocks);
        idsOf(ids, "dialog", dialogs);
        idsOf(ids, "enchantment", enchantments);
        idsOf(ids, "banner_pattern", bannerPatterns);
        // The entries `entities` and `entityData` send, out of registries too large to list whole.
        for (String entry : List.of("damage_type/mob_attack", "entity_type/zombie", "villager_type/swamp",
                "villager_profession/farmer", "item/stone")) {
            String[] parts = entry.split("/");
            Identifier name = Identifier.withDefaultNamespace(parts[1]);
            Registry<?> registry = registries.lookupOrThrow(ResourceKey.createRegistryKey(
                    Identifier.withDefaultNamespace(parts[0])));
            ids.addProperty(parts[0] + "/" + name, idOf(registry, name));
        }
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

    private static void login() {
        UUID uuid = new UUID(0x0123456789abcdefL, 0xfedcba9876543210L);
        Identifier cookie = Identifier.withDefaultNamespace("session");
        encode("login/serverbound/hello", net.minecraft.network.protocol.login.ServerboundHelloPacket.STREAM_CODEC::encode,
                new net.minecraft.network.protocol.login.ServerboundHelloPacket("Player", uuid));
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
        var properties = new com.mojang.authlib.properties.PropertyMap(
                com.google.common.collect.ImmutableMultimap.of("textures",
                        new com.mojang.authlib.properties.Property("textures", "value", "signature")));
        encode("login/clientbound/login_finished",
                net.minecraft.network.protocol.login.ClientboundLoginFinishedPacket.STREAM_CODEC::encode,
                new net.minecraft.network.protocol.login.ClientboundLoginFinishedPacket(
                        new com.mojang.authlib.GameProfile(uuid, "Player", properties),
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
        encode("serverbound/accept_teleportation",
                ServerboundAcceptTeleportationPacket.STREAM_CODEC::encode,
                new ServerboundAcceptTeleportationPacket(300));
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
                new ServerboundSignUpdatePacket(pos, false, "one", "", "thrée", "4"));
        encode("serverbound/swing",
                ServerboundSwingPacket.STREAM_CODEC::encode,
                new ServerboundSwingPacket(
                        net.minecraft.world.InteractionHand.OFF_HAND));
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
                overworld, Level.OVERWORLD, 42L, GameType.CREATIVE, null, false, true, Optional.empty(), 4, 63);
        CommonPlayerSpawnInfo died = new CommonPlayerSpawnInfo(
                nether, Level.NETHER, -7L, GameType.SURVIVAL, GameType.ADVENTURE, true, false,
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
        VanillaRegistries.createLookup().lookupOrThrow(key).listElements()
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
        component("map_color", net.minecraft.core.component.DataComponents.MAP_COLOR,
                new net.minecraft.world.item.component.MapItemColor(0x46402e));
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
                net.minecraft.world.item.equipment.Equippable.builder(net.minecraft.world.entity.EquipmentSlot.HEAD)
                        .setEquipSound(Holder.direct(net.minecraft.sounds.SoundEvent.createFixedRangeEvent(
                                Identifier.withDefaultNamespace("custom"), 16.0f)))
                        .build());
        component("lore", net.minecraft.core.component.DataComponents.LORE,
                new net.minecraft.world.item.component.ItemLore(List.of(
                        Component.literal("First"), Component.literal("Second").withStyle(ChatFormatting.RED))));
        component("trim", net.minecraft.core.component.DataComponents.TRIM,
                new net.minecraft.world.item.equipment.trim.ArmorTrim(
                        Holder.direct(new net.minecraft.world.item.equipment.trim.TrimMaterial(
                                net.minecraft.world.item.equipment.trim.MaterialAssetGroup.create("test"),
                                Component.literal("Test Material"))),
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
        component("swing_animation", net.minecraft.core.component.DataComponents.SWING_ANIMATION,
                new net.minecraft.world.item.component.SwingAnimation(
                        net.minecraft.world.item.SwingAnimationType.STAB, 10));
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
                        new net.minecraft.world.item.consume_effects.TeleportRandomlyConsumeEffect(8.0f),
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
        encode("add_entity/zombie", ClientboundAddEntityPacket.STREAM_CODEC::encode,
                new ClientboundAddEntityPacket(7, new UUID(0L, 42L),
                        position.x, position.y, position.z, -90.0f, 45.0f,
                        net.minecraft.world.entity.EntityTypes.ZOMBIE, 5, movement, 22.5));
        encode("move_entity_pos/on_ground",
                ClientboundMoveEntityPacket.Pos.STREAM_CODEC::encode,
                new ClientboundMoveEntityPacket.Pos(
                        7, (short) 4096, (short) -2048, (short) 1, true));
        encode("move_entity_pos_rot/airborne",
                ClientboundMoveEntityPacket.PosRot.STREAM_CODEC::encode,
                new ClientboundMoveEntityPacket.PosRot(
                        7, (short) 4096, (short) -2048, (short) 1, (byte) 64, (byte) -32, false));
        encode("move_entity_rot/on_ground",
                ClientboundMoveEntityPacket.Rot.STREAM_CODEC::encode,
                new ClientboundMoveEntityPacket.Rot(7, (byte) 64, (byte) -32, true));
        encode("entity_position_sync/on_ground",
                ClientboundEntityPositionSyncPacket.STREAM_CODEC::encode,
                new ClientboundEntityPositionSyncPacket(
                        7, new PositionMoveRotation(position, movement, 45.0f, -10.0f), true));
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

    /// An instance of `type` whose constructor is skipped, with `fields` set.
    private static <P> P withFields(Class<P> type, Map<String, Object> fields) throws Exception {
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
        // A stack reads its item's default components, which only a loaded world binds.
        net.minecraft.world.item.Items.STONE.builtInRegistryHolder()
                .bindComponents(net.minecraft.core.component.DataComponentMap.EMPTY);
        data(values, EntityDataSerializers.ITEM_STACK,
                new net.minecraft.world.item.ItemStack(net.minecraft.world.item.Items.STONE, 2));
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
