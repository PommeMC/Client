import java.io.BufferedWriter;
import java.io.IOException;
import java.lang.reflect.Field;
import java.lang.reflect.Method;
import java.nio.file.Files;
import java.nio.file.Path;
import java.util.ArrayList;
import java.util.LinkedHashMap;
import java.util.List;
import java.util.Map;

/**
 * Dumps per-block-state light properties by running vanilla's own code:
 * bootstraps the block registry from the server jar on the classpath, then
 * iterates Block.BLOCK_STATE_REGISTRY in state-id order.
 *
 * Everything is reflection so one binary covers both the 26.x API
 * (getLightDampening) and the 1.21.x API (getLightBlock); it also means the
 * tool compiles against nothing but the JDK.
 *
 * Face-occlusion shapes are emitted as 16x16 bitmasks over the face plane.
 * Vanilla's faceShapeOccludes(a, b) tests whether the union of two face
 * shapes covers the full block; since face shapes span their slice axis,
 * that reduces to 2D coverage — exact as long as every shape is 1/16-aligned,
 * which this tool hard-fails on if violated.
 *
 * Usage: java -cp <server-classes-jar>;<bundled-libs...>;. LightDump <version> <out.json>
 */
public final class LightDump {
    // Direction.values() order (DOWN, UP, NORTH, SOUTH, WEST, EAST) -> slice axis:
    // Y for down/up, Z for north/south, X for west/east.
    private static final char[] AXIS_BY_ORDINAL = {'Y', 'Y', 'Z', 'Z', 'X', 'X'};

    public static void main(String[] args) throws Exception {
        if (args.length != 2) {
            System.err.println("usage: LightDump <version> <out.json>");
            System.exit(2);
        }
        String version = args[0];
        Path out = Path.of(args[1]);

        Class.forName("net.minecraft.SharedConstants").getMethod("tryDetectVersion").invoke(null);
        Class.forName("net.minecraft.server.Bootstrap").getMethod("bootStrap").invoke(null);

        Field registryField = Class.forName("net.minecraft.world.level.block.Block")
                .getField("BLOCK_STATE_REGISTRY");
        Iterable<?> registry = (Iterable<?>) registryField.get(null);
        Object[] directions = Class.forName("net.minecraft.core.Direction").getEnumConstants();
        if (directions.length != 6) {
            throw new IllegalStateException("expected 6 directions, got " + directions.length);
        }

        List<Integer> emission = new ArrayList<>();
        List<Integer> dampening = new ArrayList<>();
        List<Integer> propagates = new ArrayList<>();
        List<Integer> canOcclude = new ArrayList<>();
        List<Integer> useShape = new ArrayList<>();
        // state id -> 6 face masks (64 hex chars each), only for canOcclude && useShape states
        Map<Integer, String[]> faceMasks = new LinkedHashMap<>();

        Methods m = null;
        int id = 0;
        for (Object state : registry) {
            if (m == null) {
                m = new Methods(state.getClass());
            }
            boolean occludes = (Boolean) m.canOcclude.invoke(state);
            boolean shaped = (Boolean) m.useShapeForLightOcclusion.invoke(state);
            emission.add((Integer) m.getLightEmission.invoke(state));
            dampening.add((Integer) m.getLightDampening.invoke(state));
            propagates.add(((Boolean) m.propagatesSkylightDown.invoke(state)) ? 1 : 0);
            canOcclude.add(occludes ? 1 : 0);
            useShape.add(shaped ? 1 : 0);
            if (occludes && shaped) {
                String[] masks = new String[6];
                for (int d = 0; d < 6; d++) {
                    Object shape = m.getFaceOcclusionShape.invoke(state, directions[d]);
                    masks[d] = maskHex(projectFace(shape, m, AXIS_BY_ORDINAL[d], id, d));
                }
                faceMasks.put(id, masks);
            }
            id++;
        }

        try (BufferedWriter w = Files.newBufferedWriter(out)) {
            w.write("{\n");
            w.write("  \"version\": \"" + version + "\",\n");
            w.write("  \"state_count\": " + id + ",\n");
            writeIntArray(w, "emission", emission);
            writeIntArray(w, "dampening", dampening);
            writeIntArray(w, "propagates_skylight_down", propagates);
            writeIntArray(w, "can_occlude", canOcclude);
            writeIntArray(w, "use_shape_for_light_occlusion", useShape);
            w.write("  \"face_masks\": {");
            boolean first = true;
            for (Map.Entry<Integer, String[]> e : faceMasks.entrySet()) {
                if (!first) {
                    w.write(",");
                }
                first = false;
                w.write("\n    \"" + e.getKey() + "\": [");
                for (int d = 0; d < 6; d++) {
                    if (d > 0) {
                        w.write(", ");
                    }
                    w.write("\"" + e.getValue()[d] + "\"");
                }
                w.write("]");
            }
            w.write("\n  }\n}\n");
        }
        System.out.println("wrote " + id + " states (" + faceMasks.size()
                + " with face-occlusion shapes) to " + out);
    }

    private static void writeIntArray(BufferedWriter w, String key, List<Integer> values)
            throws IOException {
        w.write("  \"" + key + "\": [");
        StringBuilder sb = new StringBuilder();
        for (int i = 0; i < values.size(); i++) {
            if (i > 0) {
                sb.append(',');
            }
            sb.append(values.get(i));
        }
        w.write(sb.toString());
        w.write("],\n");
    }

    /** Projects a face shape's boxes onto the face plane as a 16x16 bit grid. */
    private static int[] projectFace(Object shape, Methods m, char axis, int stateId, int dir)
            throws Exception {
        int[] rows = new int[16]; // rows[v] bits over u
        if ((Boolean) m.shapeIsEmpty.invoke(shape)) {
            return rows;
        }
        List<?> boxes = (List<?>) m.toAabbs.invoke(shape);
        for (Object box : boxes) {
            double minX = m.aabb("minX").getDouble(box);
            double minY = m.aabb("minY").getDouble(box);
            double minZ = m.aabb("minZ").getDouble(box);
            double maxX = m.aabb("maxX").getDouble(box);
            double maxY = m.aabb("maxY").getDouble(box);
            double maxZ = m.aabb("maxZ").getDouble(box);
            double u0;
            double u1;
            double v0;
            double v1;
            switch (axis) {
                case 'Y' -> { u0 = minX; u1 = maxX; v0 = minZ; v1 = maxZ; }
                case 'Z' -> { u0 = minX; u1 = maxX; v0 = minY; v1 = maxY; }
                default -> { u0 = minZ; u1 = maxZ; v0 = minY; v1 = maxY; }
            }
            int iu0 = toSixteenth(u0, stateId, dir);
            int iu1 = toSixteenth(u1, stateId, dir);
            int iv0 = toSixteenth(v0, stateId, dir);
            int iv1 = toSixteenth(v1, stateId, dir);
            for (int v = iv0; v < iv1; v++) {
                for (int u = iu0; u < iu1; u++) {
                    rows[v] |= 1 << u;
                }
            }
        }
        return rows;
    }

    private static int toSixteenth(double coord, int stateId, int dir) {
        double scaled = coord * 16.0;
        long rounded = Math.round(scaled);
        if (Math.abs(scaled - rounded) > 1e-5) {
            throw new IllegalStateException("occlusion shape not 1/16-aligned: coord " + coord
                    + " at state " + stateId + " dir " + dir);
        }
        return (int) Math.max(0, Math.min(16, rounded));
    }

    private static String maskHex(int[] rows) {
        StringBuilder sb = new StringBuilder(64);
        for (int v = 0; v < 16; v++) {
            sb.append(String.format("%04x", rows[v] & 0xFFFF));
        }
        return sb.toString();
    }

    /** Resolved reflection handles; falls back across the 26.x / 1.21.x renames. */
    private static final class Methods {
        final Method getLightEmission;
        final Method getLightDampening;
        final Method propagatesSkylightDown;
        final Method canOcclude;
        final Method useShapeForLightOcclusion;
        final Method getFaceOcclusionShape;
        final Method shapeIsEmpty;
        final Method toAabbs;
        private final Class<?> aabbClass;
        private final Map<String, Field> aabbFields = new LinkedHashMap<>();

        Methods(Class<?> stateClass) throws Exception {
            getLightEmission = stateClass.getMethod("getLightEmission");
            getLightDampening = firstMethod(stateClass, "getLightDampening", "getLightBlock");
            propagatesSkylightDown = stateClass.getMethod("propagatesSkylightDown");
            canOcclude = stateClass.getMethod("canOcclude");
            useShapeForLightOcclusion = stateClass.getMethod("useShapeForLightOcclusion");
            getFaceOcclusionShape = stateClass.getMethod("getFaceOcclusionShape",
                    Class.forName("net.minecraft.core.Direction"));
            Class<?> voxelShape = Class.forName("net.minecraft.world.phys.shapes.VoxelShape");
            shapeIsEmpty = voxelShape.getMethod("isEmpty");
            toAabbs = voxelShape.getMethod("toAabbs");
            aabbClass = Class.forName("net.minecraft.world.phys.AABB");
        }

        Field aabb(String name) throws Exception {
            Field f = aabbFields.get(name);
            if (f == null) {
                f = aabbClass.getField(name);
                aabbFields.put(name, f);
            }
            return f;
        }

        private static Method firstMethod(Class<?> cls, String... names) throws NoSuchMethodException {
            for (String name : names) {
                try {
                    return cls.getMethod(name);
                } catch (NoSuchMethodException ignored) {
                    // try the next name
                }
            }
            throw new NoSuchMethodException(String.join("/", names));
        }
    }

    private LightDump() {}
}
