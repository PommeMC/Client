import com.google.gson.GsonBuilder;
import com.google.gson.JsonObject;
import com.mojang.serialization.Lifecycle;
import io.netty.buffer.ByteBufUtil;
import io.netty.buffer.Unpooled;
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
import java.util.function.BiConsumer;
import net.minecraft.ChatFormatting;
import net.minecraft.SharedConstants;
import net.minecraft.core.BlockPos;
import net.minecraft.core.GlobalPos;
import net.minecraft.core.Holder;
import net.minecraft.core.MappedRegistry;
import net.minecraft.core.RegistrationInfo;
import net.minecraft.core.Registry;
import net.minecraft.core.RegistryAccess;
import net.minecraft.core.registries.Registries;
import net.minecraft.data.registries.VanillaRegistries;
import net.minecraft.network.RegistryFriendlyByteBuf;
import net.minecraft.network.chat.Component;
import net.minecraft.network.protocol.common.ClientboundDisconnectPacket;
import net.minecraft.network.protocol.common.ClientboundKeepAlivePacket;
import net.minecraft.network.protocol.common.ClientboundPingPacket;
import net.minecraft.network.protocol.game.ClientboundLoginPacket;
import net.minecraft.network.protocol.game.ClientboundPlayerPositionPacket;
import net.minecraft.network.protocol.game.ClientboundPlayerRotationPacket;
import net.minecraft.network.protocol.game.ClientboundRespawnPacket;
import net.minecraft.network.protocol.game.ClientboundSetSimulationDistancePacket;
import net.minecraft.network.protocol.game.ClientboundSetTimePacket;
import net.minecraft.network.protocol.game.ClientboundTeleportEntityPacket;
import net.minecraft.network.protocol.game.CommonPlayerSpawnInfo;
import net.minecraft.resources.ResourceKey;
import net.minecraft.server.Bootstrap;
import net.minecraft.world.clock.ClockNetworkState;
import net.minecraft.world.clock.WorldClock;
import net.minecraft.world.clock.WorldClocks;
import net.minecraft.world.entity.PositionMoveRotation;
import net.minecraft.world.entity.Relative;
import net.minecraft.world.level.GameType;
import net.minecraft.world.level.Level;
import net.minecraft.world.level.dimension.BuiltinDimensionTypes;
import net.minecraft.world.level.dimension.DimensionType;
import net.minecraft.world.phys.Vec3;

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
        registries = new RegistryAccess.ImmutableRegistryAccess(List.of(dimensionTypes, clocks));

        lifecycle(dimensionTypes, clocks);

        JsonObject out = new JsonObject();
        out.addProperty("version", SharedConstants.getCurrentVersion().name());
        JsonObject ids = new JsonObject();
        idsOf(ids, "dimension_type", dimensionTypes);
        idsOf(ids, "world_clock", clocks);
        out.add("registry_ids", ids);
        JsonObject cases = new JsonObject();
        CASES.forEach(cases::addProperty);
        out.add("cases", cases);
        Files.writeString(Path.of(args[0]), new GsonBuilder().setPrettyPrinting().create().toJson(out) + "\n");
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
