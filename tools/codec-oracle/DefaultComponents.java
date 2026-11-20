import com.google.gson.GsonBuilder;
import com.google.gson.JsonArray;
import com.google.gson.JsonObject;
import io.netty.buffer.ByteBufUtil;
import io.netty.buffer.Unpooled;
import java.nio.file.Files;
import java.nio.file.Path;
import com.mojang.serialization.Lifecycle;
import java.util.ArrayList;
import java.util.Comparator;
import java.util.LinkedHashMap;
import java.util.List;
import java.util.Map;
import java.util.TreeMap;
import java.util.TreeSet;
import net.minecraft.SharedConstants;
import net.minecraft.core.HolderLookup;
import net.minecraft.core.MappedRegistry;
import net.minecraft.core.RegistrationInfo;
import net.minecraft.core.Registry;
import net.minecraft.core.RegistryAccess;
import net.minecraft.core.component.DataComponentMap;
import net.minecraft.core.component.DataComponentType;
import net.minecraft.core.component.TypedDataComponent;
import net.minecraft.core.registries.BuiltInRegistries;
import net.minecraft.core.registries.Registries;
import net.minecraft.data.registries.VanillaRegistries;
import net.minecraft.network.RegistryFriendlyByteBuf;
import net.minecraft.resources.ResourceKey;
import net.minecraft.server.Bootstrap;
import net.minecraft.world.item.Item;

/**
 * Dumps every item's default components as the bytes each component's own
 * STREAM_CODEC writes, for pomme-item to read them with the readers it reads
 * a stack's patch with.
 *
 * The components are built as the game builds them
 * (DataComponentInitializers, the way RegistryComponentsReport does). A
 * value that holds an entry of a registry a server sends is written with the
 * id that entry has in the game's own data pack, whose registries load in
 * identifier order; "server_registries" lists the components and items that
 * applies to, since a server with other entries numbers them differently.
 *
 * Each value also gets the hash the client sends for it in a click
 * (HashedPatchMap, HashOps over the component's CODEC), for pomme-item's own
 * hashes to be checked against; a component with no CODEC has none.
 *
 * Output: {"version", "values": [hex...], "items": {item: {component: index
 * into values}}, "hashes": {component: {index into values: hash}},
 * "server_registries": {component: [item...]}}, items in registry order.
 *
 * Usage: java -cp <server classes>:<bundled libs>:<this> DefaultComponents <out.json>
 */
public final class DefaultComponents {
    public static void main(String[] args) throws Exception {
        if (args.length != 1) {
            System.err.println("usage: DefaultComponents <out.json>");
            System.exit(2);
        }
        SharedConstants.tryDetectVersion();
        Bootstrap.bootStrap();
        HolderLookup.Provider lookup = VanillaRegistries.createLookup();
        RegistryAccess builtIn = RegistryAccess.fromRegistryOfRegistries(BuiltInRegistries.REGISTRY);
        List<Registry<?>> all = new ArrayList<>();
        lookup.listRegistryKeys().forEach(key -> all.add(builtIn.lookup(key).isPresent()
                ? builtIn.lookupOrThrow(key) : inIdentifierOrder(lookup, key)));
        RegistryAccess withDataPack = new RegistryAccess.ImmutableRegistryAccess(all);

        Map<Item, DataComponentMap> components = new LinkedHashMap<>();
        BuiltInRegistries.DATA_COMPONENT_INITIALIZERS.build(lookup).forEach(pending -> {
            if (pending.key() == Registries.ITEM) {
                pending.forEach((item, map) -> components.put((Item) item.value(), map));
            }
        });

        Map<String, Integer> valueIndex = new LinkedHashMap<>();
        Map<String, TreeSet<String>> serverRegistries = new TreeMap<>();
        Map<String, TreeMap<Integer, Integer>> hashes = new TreeMap<>();
        var hashOps = lookup.createSerializationContext(net.minecraft.util.HashOps.CRC32C_INSTANCE);
        JsonObject items = new JsonObject();
        for (Item item : BuiltInRegistries.ITEM) {
            String itemName = BuiltInRegistries.ITEM.getKey(item).getPath();
            JsonObject entries = new JsonObject();
            components.get(item).stream()
                    .sorted(Comparator.comparingInt(c -> BuiltInRegistries.DATA_COMPONENT_TYPE.getId(c.type())))
                    .forEach(component -> {
                        String name = BuiltInRegistries.DATA_COMPONENT_TYPE.getKey(component.type()).getPath();
                        String hex = encode(builtIn, component);
                        if (hex == null) {
                            serverRegistries.computeIfAbsent(name, k -> new TreeSet<>()).add(itemName);
                            hex = encode(withDataPack, component);
                        }
                        int index = valueIndex.computeIfAbsent(hex, k -> valueIndex.size());
                        entries.addProperty(name, index);
                        component.encodeValue(hashOps).ifSuccess(hash -> hashes
                                .computeIfAbsent(name, k -> new TreeMap<>()).put(index, hash.asInt()));
                    });
            items.add(itemName, entries);
        }

        JsonObject root = new JsonObject();
        root.addProperty("version", SharedConstants.getCurrentVersion().name());
        JsonArray values = new JsonArray();
        valueIndex.keySet().forEach(values::add);
        root.add("values", values);
        root.add("items", items);
        JsonObject hashed = new JsonObject();
        hashes.forEach((component, byValue) -> {
            JsonObject object = new JsonObject();
            byValue.forEach((index, hash) -> object.addProperty(Integer.toString(index), hash));
            hashed.add(component, object);
        });
        root.add("hashes", hashed);
        JsonObject bound = new JsonObject();
        serverRegistries.forEach((component, names) -> {
            JsonArray array = new JsonArray();
            names.forEach(array::add);
            bound.add(component, array);
        });
        root.add("server_registries", bound);
        Files.writeString(Path.of(args[0]), new GsonBuilder().create().toJson(root) + "\n");
        System.err.println(items.size() + " items, " + valueIndex.size() + " distinct values, on server registries: "
                + serverRegistries.keySet());
    }

    /** A data pack registry as a server loads it: its entries in identifier order. */
    private static <T> Registry<T> inIdentifierOrder(HolderLookup.Provider lookup, ResourceKey<? extends Registry<?>> key) {
        @SuppressWarnings("unchecked")
        ResourceKey<? extends Registry<T>> typed = (ResourceKey<? extends Registry<T>>) key;
        MappedRegistry<T> registry = new MappedRegistry<>(typed, Lifecycle.stable());
        lookup.lookupOrThrow(typed).listElements()
                .sorted(Comparator.comparing(entry -> entry.key().identifier().toString()))
                .forEach(entry -> registry.register(entry.key(), entry.value(), RegistrationInfo.BUILT_IN));
        return registry.freeze();
    }

    /** The value's STREAM_CODEC bytes as hex, or null when `registries` lacks one it needs. */
    private static <T> String encode(RegistryAccess registries, TypedDataComponent<T> component) {
        DataComponentType<T> type = component.type();
        RegistryFriendlyByteBuf buf = new RegistryFriendlyByteBuf(Unpooled.buffer(), registries);
        try {
            type.streamCodec().encode(buf, component.value());
        } catch (RuntimeException e) {
            return null;
        }
        return ByteBufUtil.hexDump(buf);
    }
}
