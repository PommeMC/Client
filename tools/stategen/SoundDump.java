import java.io.BufferedWriter;
import java.lang.reflect.Method;
import java.nio.file.Files;
import java.nio.file.Path;
import java.util.Map;
import java.util.TreeMap;

/**
 * Dumps every block's SoundType by running vanilla's own code: each block's
 * default state's getSoundType(), as [volume, pitch, break, step, place, hit,
 * fall] with the events as ids without the namespace. Reflection only, so one
 * binary covers SoundEvent.getLocation() (pre-1.21.2) and location() (later).
 *
 * Usage: java -cp <server-classes-jar>;<bundled-libs...>;. SoundDump <out.json>
 */
public final class SoundDump {
    public static void main(String[] args) throws Exception {
        if (args.length != 1) {
            System.err.println("usage: SoundDump <out.json>");
            System.exit(2);
        }
        Class.forName("net.minecraft.SharedConstants").getMethod("tryDetectVersion").invoke(null);
        Class.forName("net.minecraft.server.Bootstrap").getMethod("bootStrap").invoke(null);

        Object registry = Class.forName("net.minecraft.core.registries.BuiltInRegistries")
                .getField("BLOCK").get(null);
        Method getKey = null;
        for (Method method : registry.getClass().getMethods()) {
            if (method.getName().equals("getKey") && method.getParameterCount() == 1) {
                getKey = method;
                break;
            }
        }
        if (getKey == null) {
            throw new IllegalStateException("no BLOCK.getKey");
        }
        Class<?> soundType = Class.forName("net.minecraft.world.level.block.SoundType");
        Class<?> soundEvent = Class.forName("net.minecraft.sounds.SoundEvent");
        Method location;
        try {
            location = soundEvent.getMethod("location");
        } catch (NoSuchMethodException e) {
            location = soundEvent.getMethod("getLocation");
        }
        String[] events = { "getBreakSound", "getStepSound", "getPlaceSound", "getHitSound", "getFallSound" };

        Map<String, String> entries = new TreeMap<>();
        for (Object block : (Iterable<?>) registry) {
            Object state = block.getClass().getMethod("defaultBlockState").invoke(block);
            Object sound = state.getClass().getMethod("getSoundType").invoke(state);
            StringBuilder sb = new StringBuilder("[");
            sb.append(soundType.getMethod("getVolume").invoke(sound)).append(", ");
            sb.append(soundType.getMethod("getPitch").invoke(sound));
            for (String event : events) {
                Object id = location.invoke(soundType.getMethod(event).invoke(sound));
                String path = (String) id.getClass().getMethod("getPath").invoke(id);
                sb.append(", \"").append(path).append('"');
            }
            sb.append(']');
            Object key = getKey.invoke(registry, block);
            entries.put((String) key.getClass().getMethod("getPath").invoke(key), sb.toString());
        }
        try (BufferedWriter w = Files.newBufferedWriter(Path.of(args[0]))) {
            w.write("{\n");
            int i = 0;
            for (Map.Entry<String, String> e : entries.entrySet()) {
                w.write("  \"" + e.getKey() + "\": " + e.getValue()
                        + (++i < entries.size() ? ",\n" : "\n"));
            }
            w.write("}\n");
        }
        System.out.println("wrote " + entries.size() + " block sound types to " + args[0]);
    }

    private SoundDump() {}
}
