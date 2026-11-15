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
import net.minecraft.network.protocol.game.ClientboundChunksBiomesPacket;
import net.minecraft.network.protocol.game.ClientboundCommandsPacket;
import net.minecraft.network.protocol.game.ClientboundCooldownPacket;
import net.minecraft.network.protocol.game.ClientboundLoginPacket;
import net.minecraft.network.protocol.game.ClientboundMoveVehiclePacket;
import net.minecraft.network.protocol.game.ClientboundOpenSignEditorPacket;
import net.minecraft.network.protocol.game.ClientboundPlayerPositionPacket;
import net.minecraft.network.protocol.game.ClientboundPlayerRotationPacket;
import net.minecraft.network.protocol.game.ClientboundRespawnPacket;
import net.minecraft.network.protocol.game.ClientboundSetEntityDataPacket;
import net.minecraft.network.protocol.game.ClientboundSetSimulationDistancePacket;
import net.minecraft.network.protocol.game.ClientboundSetTimePacket;
import net.minecraft.network.protocol.game.ClientboundTeleportEntityPacket;
import net.minecraft.network.protocol.game.ClientboundTrackedWaypointPacket;
import net.minecraft.network.protocol.game.CommonPlayerSpawnInfo;
import net.minecraft.network.protocol.game.ServerboundMoveVehiclePacket;
import net.minecraft.network.protocol.game.ServerboundPaddleBoatPacket;
import net.minecraft.network.syncher.EntityDataAccessor;
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
        registries = new RegistryAccess.ImmutableRegistryAccess(List.of(dimensionTypes, clocks, biomes, dialogs,
                net.minecraft.core.registries.BuiltInRegistries.ITEM,
                net.minecraft.core.registries.BuiltInRegistries.DATA_COMPONENT_TYPE));

        lifecycle(dimensionTypes, clocks);
        status();
        login();
        configuration();
        sections();
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
        Map<Identifier, IntList> blockTags = new LinkedHashMap<>();
        blockTags.put(Identifier.withDefaultNamespace("logs"), IntList.of(1, 300));
        blockTags.put(Identifier.withDefaultNamespace("empty"), IntList.of());
        encode("update_tags/two_registries", ClientboundUpdateTagsPacket.STREAM_CODEC::encode,
                new ClientboundUpdateTagsPacket(Map.of(
                        Registries.BLOCK, new TagNetworkSerialization.NetworkPayload(blockTags),
                        Registries.DIALOG, new TagNetworkSerialization.NetworkPayload(Map.of(
                                Identifier.withDefaultNamespace("pause_screen_additions"), IntList.of(2))))));

        JsonObject out = new JsonObject();
        out.addProperty("version", SharedConstants.getCurrentVersion().name());
        JsonObject ids = new JsonObject();
        idsOf(ids, "dimension_type", dimensionTypes);
        idsOf(ids, "world_clock", clocks);
        idsOf(ids, "dialog", dialogs);
        out.add("registry_ids", ids);
        JsonObject sizes = new JsonObject();
        sizes.addProperty("block_state", Block.BLOCK_STATE_REGISTRY.size());
        sizes.addProperty("worldgen/biome", biomes.size());
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
                net.minecraft.network.protocol.game.ServerboundAcceptTeleportationPacket.STREAM_CODEC::encode,
                new net.minecraft.network.protocol.game.ServerboundAcceptTeleportationPacket(300));
        encode("serverbound/bundle_item_selected",
                net.minecraft.network.protocol.game.ServerboundSelectBundleItemPacket.STREAM_CODEC::encode,
                new net.minecraft.network.protocol.game.ServerboundSelectBundleItemPacket(36, -1));
        encode("serverbound/change_game_mode",
                net.minecraft.network.protocol.game.ServerboundChangeGameModePacket.STREAM_CODEC::encode,
                new net.minecraft.network.protocol.game.ServerboundChangeGameModePacket(GameType.SPECTATOR));
        encode("serverbound/chunk_batch_received",
                net.minecraft.network.protocol.game.ServerboundChunkBatchReceivedPacket.STREAM_CODEC::encode,
                new net.minecraft.network.protocol.game.ServerboundChunkBatchReceivedPacket(12.5f));
        encode("serverbound/client_command",
                net.minecraft.network.protocol.game.ServerboundClientCommandPacket.STREAM_CODEC::encode,
                new net.minecraft.network.protocol.game.ServerboundClientCommandPacket(
                        net.minecraft.network.protocol.game.ServerboundClientCommandPacket.Action.REQUEST_STATS));
        encode("serverbound/client_information",
                net.minecraft.network.protocol.common.ServerboundClientInformationPacket.STREAM_CODEC::encode,
                new net.minecraft.network.protocol.common.ServerboundClientInformationPacket(
                        new net.minecraft.server.level.ClientInformation("en_gb", 12,
                                net.minecraft.world.entity.player.ChatVisiblity.SYSTEM, true, 0x7f,
                                net.minecraft.world.entity.HumanoidArm.LEFT, false, true,
                                net.minecraft.server.level.ParticleStatus.MINIMAL)));
        encode("serverbound/client_tick_end",
                net.minecraft.network.protocol.game.ServerboundClientTickEndPacket.STREAM_CODEC::encode,
                net.minecraft.network.protocol.game.ServerboundClientTickEndPacket.INSTANCE);
        encode("serverbound/command_suggestion",
                net.minecraft.network.protocol.game.ServerboundCommandSuggestionPacket.STREAM_CODEC::encode,
                new net.minecraft.network.protocol.game.ServerboundCommandSuggestionPacket(7, "/give @s sto"));
        encode("serverbound/configuration_acknowledged",
                net.minecraft.network.protocol.game.ServerboundConfigurationAcknowledgedPacket.STREAM_CODEC::encode,
                net.minecraft.network.protocol.game.ServerboundConfigurationAcknowledgedPacket.INSTANCE);
        encode("serverbound/container_button_click",
                net.minecraft.network.protocol.game.ServerboundContainerButtonClickPacket.STREAM_CODEC::encode,
                new net.minecraft.network.protocol.game.ServerboundContainerButtonClickPacket(3, 200));
        var changed = new it.unimi.dsi.fastutil.ints.Int2ObjectOpenHashMap<net.minecraft.network.HashedStack>();
        changed.put(5, new net.minecraft.network.HashedStack.ActualItem(
                net.minecraft.core.registries.BuiltInRegistries.ITEM.wrapAsHolder(
                        net.minecraft.world.item.Items.DIAMOND_SWORD),
                1,
                new net.minecraft.network.HashedPatchMap(
                        Map.of(net.minecraft.core.component.DataComponents.DAMAGE, 0x12345678),
                        Set.of(net.minecraft.core.component.DataComponents.CUSTOM_NAME))));
        encode("serverbound/container_click",
                net.minecraft.network.protocol.game.ServerboundContainerClickPacket.STREAM_CODEC::encode,
                new net.minecraft.network.protocol.game.ServerboundContainerClickPacket(3, 300, (short) -999, (byte) 1,
                        net.minecraft.world.inventory.ContainerInput.THROW, changed,
                        net.minecraft.network.HashedStack.EMPTY));
        encode("serverbound/container_close",
                net.minecraft.network.protocol.game.ServerboundContainerClosePacket.STREAM_CODEC::encode,
                new net.minecraft.network.protocol.game.ServerboundContainerClosePacket(3));
        encode("serverbound/container_slot_state_changed",
                net.minecraft.network.protocol.game.ServerboundContainerSlotStateChangedPacket.STREAM_CODEC::encode,
                new net.minecraft.network.protocol.game.ServerboundContainerSlotStateChangedPacket(8, 3, true));
        encode("serverbound/custom_payload",
                net.minecraft.network.protocol.common.ServerboundCustomPayloadPacket.STREAM_CODEC::encode,
                new net.minecraft.network.protocol.common.ServerboundCustomPayloadPacket(
                        new net.minecraft.network.protocol.common.custom.BrandPayload("pomme")));
        encode("serverbound/keep_alive",
                net.minecraft.network.protocol.common.ServerboundKeepAlivePacket.STREAM_CODEC::encode,
                new net.minecraft.network.protocol.common.ServerboundKeepAlivePacket(-2L));
        encode("serverbound/move_player_pos",
                net.minecraft.network.protocol.game.ServerboundMovePlayerPacket.Pos.STREAM_CODEC::encode,
                new net.minecraft.network.protocol.game.ServerboundMovePlayerPacket.Pos(1.5, 64.0, -2.25, true, false));
        encode("serverbound/move_player_pos_rot",
                net.minecraft.network.protocol.game.ServerboundMovePlayerPacket.PosRot.STREAM_CODEC::encode,
                new net.minecraft.network.protocol.game.ServerboundMovePlayerPacket.PosRot(
                        1.5, 64.0, -2.25, 90.0f, -10.0f, false, true));
        encode("serverbound/move_player_rot",
                net.minecraft.network.protocol.game.ServerboundMovePlayerPacket.Rot.STREAM_CODEC::encode,
                new net.minecraft.network.protocol.game.ServerboundMovePlayerPacket.Rot(90.0f, -10.0f, true, true));
        encode("serverbound/move_player_status_only",
                net.minecraft.network.protocol.game.ServerboundMovePlayerPacket.StatusOnly.STREAM_CODEC::encode,
                new net.minecraft.network.protocol.game.ServerboundMovePlayerPacket.StatusOnly(false, false));
        var abilities = new net.minecraft.world.entity.player.Abilities();
        abilities.flying = true;
        encode("serverbound/player_abilities",
                net.minecraft.network.protocol.game.ServerboundPlayerAbilitiesPacket.STREAM_CODEC::encode,
                new net.minecraft.network.protocol.game.ServerboundPlayerAbilitiesPacket(abilities));
        encode("serverbound/player_action",
                net.minecraft.network.protocol.game.ServerboundPlayerActionPacket.STREAM_CODEC::encode,
                new net.minecraft.network.protocol.game.ServerboundPlayerActionPacket(
                        net.minecraft.network.protocol.game.ServerboundPlayerActionPacket.Action.STOP_DESTROY_BLOCK,
                        pos, net.minecraft.core.Direction.WEST, 300));
        // The constructor wants an entity, so this one decodes the bytes
        // pomme writes (entity 300, START_RIDING_JUMP, 100) and encodes them back.
        encode("serverbound/player_command",
                net.minecraft.network.protocol.game.ServerboundPlayerCommandPacket.STREAM_CODEC::encode,
                net.minecraft.network.protocol.game.ServerboundPlayerCommandPacket.STREAM_CODEC.decode(
                        new FriendlyByteBuf(Unpooled.wrappedBuffer(new byte[] {(byte) 0xAC, 2, 3, 100}))));
        encode("serverbound/player_input",
                net.minecraft.network.protocol.game.ServerboundPlayerInputPacket.STREAM_CODEC::encode,
                new net.minecraft.network.protocol.game.ServerboundPlayerInputPacket(
                        new net.minecraft.world.entity.player.Input(true, false, false, true, true, false, true)));
        encode("serverbound/player_loaded",
                net.minecraft.network.protocol.game.ServerboundPlayerLoadedPacket.STREAM_CODEC::encode,
                new net.minecraft.network.protocol.game.ServerboundPlayerLoadedPacket());
        encode("serverbound/pong",
                net.minecraft.network.protocol.common.ServerboundPongPacket.STREAM_CODEC::encode,
                new net.minecraft.network.protocol.common.ServerboundPongPacket(-5));
        encode("serverbound/rename_item",
                net.minecraft.network.protocol.game.ServerboundRenameItemPacket.STREAM_CODEC::encode,
                new net.minecraft.network.protocol.game.ServerboundRenameItemPacket("Sting"));
        encode("serverbound/resource_pack",
                net.minecraft.network.protocol.common.ServerboundResourcePackPacket.STREAM_CODEC::encode,
                new net.minecraft.network.protocol.common.ServerboundResourcePackPacket(uuid,
                        net.minecraft.network.protocol.common.ServerboundResourcePackPacket.Action.FAILED_RELOAD));
        encode("serverbound/set_carried_item",
                net.minecraft.network.protocol.game.ServerboundSetCarriedItemPacket.STREAM_CODEC::encode,
                new net.minecraft.network.protocol.game.ServerboundSetCarriedItemPacket(8));
        encode("serverbound/sign_update",
                net.minecraft.network.protocol.game.ServerboundSignUpdatePacket.STREAM_CODEC::encode,
                new net.minecraft.network.protocol.game.ServerboundSignUpdatePacket(pos, false, "one", "", "thrée", "4"));
        encode("serverbound/swing",
                net.minecraft.network.protocol.game.ServerboundSwingPacket.STREAM_CODEC::encode,
                new net.minecraft.network.protocol.game.ServerboundSwingPacket(
                        net.minecraft.world.InteractionHand.OFF_HAND));
        encode("serverbound/teleport_to_entity",
                net.minecraft.network.protocol.game.ServerboundTeleportToEntityPacket.STREAM_CODEC::encode,
                new net.minecraft.network.protocol.game.ServerboundTeleportToEntityPacket(uuid));
        encode("serverbound/use_item",
                net.minecraft.network.protocol.game.ServerboundUseItemPacket.STREAM_CODEC::encode,
                new net.minecraft.network.protocol.game.ServerboundUseItemPacket(
                        net.minecraft.world.InteractionHand.MAIN_HAND, 300, 90.0f, -10.0f));
        encode("serverbound/use_item_on",
                net.minecraft.network.protocol.game.ServerboundUseItemOnPacket.STREAM_CODEC::encode,
                new net.minecraft.network.protocol.game.ServerboundUseItemOnPacket(
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

    private static <T> void idsOf(JsonObject ids, String name, Registry<T> registry) {
        registry.listElements().forEach(entry ->
                ids.addProperty(name + "/" + entry.key().identifier(), registry.getId(entry.value())));
    }

    private static <P> void encode(String name, BiConsumer<RegistryFriendlyByteBuf, P> codec, P packet) {
        RegistryFriendlyByteBuf buf = new RegistryFriendlyByteBuf(Unpooled.buffer(), registries);
        codec.accept(buf, packet);
        CASES.put(name, ByteBufUtil.hexDump(buf));
    }
}
