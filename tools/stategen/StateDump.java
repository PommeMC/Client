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
 * Dumps per-block-state properties by running vanilla's own code: the baked
 * light set, hasCollision, blocksMotion/isSolid/canBeReplaced, full-face
 * sturdiness, the empty-context collision and outline shapes, and the
 * position offset parameters. Bootstraps the block registry from the server
 * jar on the classpath, then iterates Block.BLOCK_STATE_REGISTRY in state-id
 * order.
 *
 * Everything is reflection so one binary covers the 26.x API
 * (getLightDampening), the 1.21.2+ API (getLightBlock, getOffset(BlockPos)),
 * and the older world-context API (getLightBlock/getOffset taking a
 * BlockGetter, fed the empty getter exactly like vanilla's own state cache);
 * blocksMotion is omitted for versions without it (26.3+). It also means the
 * tool compiles against nothing but the JDK.
 *
 * Face-occlusion shapes are emitted as 16x16 bitmasks over the face plane.
 * Vanilla's faceShapeOccludes(a, b) tests whether the union of two face
 * shapes covers the full block; since face shapes span their slice axis,
 * that reduces to 2D coverage — exact as long as every shape is 1/16-aligned,
 * which this tool hard-fails on if violated.
 *
 * Usage: java -cp <server-classes-jar>;<bundled-libs...>;. StateDump <version> <out.json>
 */
public final class StateDump {
    // Direction.values() order (DOWN, UP, NORTH, SOUTH, WEST, EAST) -> slice axis:
    // Y for down/up, Z for north/south, X for west/east.
    private static final char[] AXIS_BY_ORDINAL = {'Y', 'Y', 'Z', 'Z', 'X', 'X'};

    public static void main(String[] args) throws Exception {
        if (args.length != 2) {
            System.err.println("usage: StateDump <version> <out.json>");
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
        List<Integer> hasCollision = new ArrayList<>();
        List<Integer> blocksMotion = new ArrayList<>();
        List<Integer> legacySolid = new ArrayList<>();
        List<Integer> replaceable = new ArrayList<>();
        // Packed Direction ordinal bits for isFaceSturdy(..., SupportType.FULL).
        List<Integer> fullFaceSturdy = new ArrayList<>();
        // Flattened exact AABBs per state. Vanilla uses non-grid coordinates
        // for some shapes (for example lectern thirds), so preserve the doubles
        // emitted by VoxelShape.toAabbs and let blockgen dedupe their bit patterns.
        List<double[]> collisionShapes = new ArrayList<>();
        List<double[]> outlineShapes = new ArrayList<>();
        // Whether vanilla actually applies BlockState.getOffset(pos) to each
        // logical shape. Render-model offsets and logical shape offsets are
        // independent: several plants are visually offset but keep an
        // unshifted interaction/collision shape.
        List<Integer> collisionShapeUsesOffset = new ArrayList<>();
        List<Integer> outlineShapeUsesOffset = new ArrayList<>();
        // Runtime parameters for BlockBehaviour.OffsetType. Keep these generated
        // from vanilla so Pomme does not duplicate the block registration list.
        List<Integer> positionOffsetType = new ArrayList<>();
        List<Float> maxHorizontalOffset = new ArrayList<>();
        List<Float> maxVerticalOffset = new ArrayList<>();
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
            dampening.add((Integer) m.invokeWorld(m.getLightDampening, state));
            propagates.add(((Boolean) m.invokeWorld(m.propagatesSkylightDown, state)) ? 1 : 0);
            canOcclude.add(occludes ? 1 : 0);
            useShape.add(shaped ? 1 : 0);
            hasCollision.add(m.hasCollision.getBoolean(m.getBlock.invoke(state)) ? 1 : 0);
            if (m.blocksMotion != null) {
                blocksMotion.add(((Boolean) m.blocksMotion.invoke(state)) ? 1 : 0);
            }
            legacySolid.add(((Boolean) m.isSolid.invoke(state)) ? 1 : 0);
            replaceable.add(((Boolean) m.canBeReplaced.invoke(state)) ? 1 : 0);
            int sturdyMask = 0;
            for (int d = 0; d < 6; d++) {
                if ((Boolean) m.isFaceSturdy.invoke(state, m.emptyGetter, m.zeroPos, directions[d])) {
                    sturdyMask |= 1 << d;
                }
            }
            fullFaceSturdy.add(sturdyMask);
            double[] collisionZero = shapeBoxes(
                    m.getCollisionShape.invoke(state, m.emptyGetter, m.zeroPos), m, id);
            double[] collisionProbe = shapeBoxes(
                    m.getCollisionShape.invoke(state, m.emptyGetter, m.probePos), m, id);
            double[] outlineZero = shapeBoxes(
                    m.getShape.invoke(state, m.emptyGetter, m.zeroPos), m, id);
            double[] outlineProbe = shapeBoxes(
                    m.getShape.invoke(state, m.emptyGetter, m.probePos), m, id);
            collisionShapes.add(collisionZero);
            outlineShapes.add(outlineZero);
            double[] zeroOffset = m.offset(state, m.zeroPos);
            double[] probeOffset = m.offset(state, m.probePos);
            collisionShapeUsesOffset.add(shapeUsesPositionOffset(
                    collisionZero, collisionProbe, zeroOffset, probeOffset, id, "collision") ? 1 : 0);
            outlineShapeUsesOffset.add(shapeUsesPositionOffset(
                    outlineZero, outlineProbe, zeroOffset, probeOffset, id, "outline") ? 1 : 0);
            Object block = m.getBlock.invoke(state);
            boolean hasOffset = (Boolean) m.hasOffsetFunction.invoke(state);
            int offsetType = 0;
            float maxHorizontal = 0.0f;
            float maxVertical = 0.0f;
            if (hasOffset) {
                offsetType = zeroOffset[1] == 0.0 ? 1 : 2;
                maxHorizontal = ((Float) m.getMaxHorizontalOffset.invoke(block)).floatValue();
                if (offsetType == 2) {
                    maxVertical = ((Float) m.getMaxVerticalOffset.invoke(block)).floatValue();
                }
            }
            positionOffsetType.add(offsetType);
            maxHorizontalOffset.add(maxHorizontal);
            maxVerticalOffset.add(maxVertical);
            if (occludes && shaped) {
                String[] masks = new String[6];
                for (int d = 0; d < 6; d++) {
                    Object shape = m.invokeWorld(m.getFaceOcclusionShape, state, directions[d]);
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
            writeIntArray(w, "has_collision", hasCollision);
            if (m.blocksMotion != null) {
                writeIntArray(w, "blocks_motion", blocksMotion);
            }
            writeIntArray(w, "legacy_solid", legacySolid);
            writeIntArray(w, "replaceable", replaceable);
            writeIntArray(w, "full_face_sturdy", fullFaceSturdy);
            writeIntArray(w, "collision_shape_uses_offset", collisionShapeUsesOffset);
            writeIntArray(w, "outline_shape_uses_offset", outlineShapeUsesOffset);
            writeIntArray(w, "position_offset_type", positionOffsetType);
            writeFloatArray(w, "max_horizontal_offset", maxHorizontalOffset);
            writeFloatArray(w, "max_vertical_offset", maxVerticalOffset);
            writeShapeArray(w, "collision_shapes", collisionShapes);
            writeShapeArray(w, "outline_shapes", outlineShapes);
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

    private static void writeFloatArray(BufferedWriter w, String key, List<Float> values) throws IOException {
        w.write("  \"" + key + "\": [");
        for (int i = 0; i < values.size(); i++) {
            if (i > 0) {
                w.write(',');
            }
            w.write(Float.toString(values.get(i)));
        }
        w.write("],\n");
    }

    private static void writeShapeArray(BufferedWriter w, String key, List<double[]> shapes)
            throws IOException {
        w.write("  \"" + key + "\": [");
        for (int i = 0; i < shapes.size(); i++) {
            if (i > 0) {
                w.write(',');
            }
            double[] shape = shapes.get(i);
            w.write('[');
            for (int j = 0; j < shape.length; j++) {
                if (j > 0) {
                    w.write(',');
                }
                w.write(Double.toString(shape[j]));
            }
            w.write(']');
        }
        w.write("],\n");
    }

    private static boolean shapeUsesPositionOffset(
            double[] zeroShape,
            double[] probeShape,
            double[] zeroOffset,
            double[] probeOffset,
            int stateId,
            String kind) {
        if (java.util.Arrays.equals(zeroShape, probeShape)) {
            return false;
        }
        if (zeroShape.length != probeShape.length) {
            throw new IllegalStateException(kind + " shape changes topology by position at state " + stateId);
        }

        double dx = probeOffset[0] - zeroOffset[0];
        double dy = probeOffset[1] - zeroOffset[1];
        double dz = probeOffset[2] - zeroOffset[2];
        double[] delta = { dx, dy, dz, dx, dy, dz };
        for (int i = 0; i < zeroShape.length; i++) {
            double expected = zeroShape[i] + delta[i % 6];
            if (Math.abs(expected - probeShape[i]) > 1.0e-12) {
                throw new IllegalStateException(kind + " shape has unexpected positional variation at state "
                        + stateId + " coord " + i + ": expected " + expected + " got " + probeShape[i]);
            }
        }
        if (dx == 0.0 && dy == 0.0 && dz == 0.0) {
            throw new IllegalStateException(kind + " shape changed with zero BlockState offset at state " + stateId);
        }
        return true;
    }

    /** Converts a vanilla VoxelShape to the exact AABBs returned by vanilla. */
    private static double[] shapeBoxes(Object shape, Methods m, int stateId) throws Exception {
        if ((Boolean) m.shapeIsEmpty.invoke(shape)) {
            return new double[0];
        }
        List<?> boxes = (List<?>) m.toAabbs.invoke(shape);
        double[] out = new double[boxes.size() * 6];
        int i = 0;
        for (Object box : boxes) {
            out[i++] = finite(m.aabb("minX").getDouble(box), stateId);
            out[i++] = finite(m.aabb("minY").getDouble(box), stateId);
            out[i++] = finite(m.aabb("minZ").getDouble(box), stateId);
            out[i++] = finite(m.aabb("maxX").getDouble(box), stateId);
            out[i++] = finite(m.aabb("maxY").getDouble(box), stateId);
            out[i++] = finite(m.aabb("maxZ").getDouble(box), stateId);
        }
        return out;
    }

    private static double finite(double coord, int stateId) {
        if (!Double.isFinite(coord)) {
            throw new IllegalStateException("non-finite block shape coordinate " + coord
                    + " at state " + stateId);
        }
        return coord;
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
        /** Null where the version has no blocksMotion (26.3+). */
        final Method blocksMotion;
        final Method isSolid;
        final Method canBeReplaced;
        final Method isFaceSturdy;
        final Method getOffset;
        private final boolean getOffsetTakesGetter;
        final Method hasOffsetFunction;
        final Method getMaxHorizontalOffset;
        final Method getMaxVerticalOffset;
        final Method getFaceOcclusionShape;
        final Method getCollisionShape;
        final Method getShape;
        final Method shapeIsEmpty;
        final Method toAabbs;
        final Method getBlock;
        final Field hasCollision;
        private final Class<?> aabbClass;
        private final Map<String, Field> aabbFields = new LinkedHashMap<>();

        final Object[] worldArgs;
        final Object emptyGetter;
        final Object zeroPos;
        final Object probePos;
        private final Class<?> vec3Class;
        private final Map<String, Field> vec3Fields = new LinkedHashMap<>();

        Methods(Class<?> stateClass) throws Exception {
            Class<?> direction = Class.forName("net.minecraft.core.Direction");
            Class<?> getter = Class.forName("net.minecraft.world.level.BlockGetter");
            Class<?> pos = Class.forName("net.minecraft.core.BlockPos");
            emptyGetter = Class.forName("net.minecraft.world.level.EmptyBlockGetter")
                    .getEnumConstants()[0];
            zeroPos = pos.getField("ZERO").get(null);
            probePos = pos.getConstructor(int.class, int.class, int.class).newInstance(5, 0, -7);
            getCollisionShape = stateClass.getMethod("getCollisionShape", getter, pos);
            getShape = stateClass.getMethod("getShape", getter, pos);
            Method offset;
            boolean offsetTakesGetter = false;
            try {
                offset = stateClass.getMethod("getOffset", pos);
            } catch (NoSuchMethodException e) {
                // Pre-1.21.2: getOffset(BlockGetter, BlockPos).
                offset = stateClass.getMethod("getOffset", getter, pos);
                offsetTakesGetter = true;
            }
            getOffset = offset;
            getOffsetTakesGetter = offsetTakesGetter;
            hasOffsetFunction = stateClass.getMethod("hasOffsetFunction");
            Class<?> blockBehaviour = Class.forName("net.minecraft.world.level.block.state.BlockBehaviour");
            getMaxHorizontalOffset = blockBehaviour.getDeclaredMethod("getMaxHorizontalOffset");
            getMaxHorizontalOffset.setAccessible(true);
            getMaxVerticalOffset = blockBehaviour.getDeclaredMethod("getMaxVerticalOffset");
            getMaxVerticalOffset.setAccessible(true);
            getLightEmission = stateClass.getMethod("getLightEmission");
            canOcclude = stateClass.getMethod("canOcclude");
            useShapeForLightOcclusion = stateClass.getMethod("useShapeForLightOcclusion");
            Method motion;
            try {
                motion = stateClass.getMethod("blocksMotion");
            } catch (NoSuchMethodException e) {
                motion = null;
            }
            blocksMotion = motion;
            isSolid = stateClass.getMethod("isSolid");
            canBeReplaced = stateClass.getMethod("canBeReplaced");
            isFaceSturdy = stateClass.getMethod("isFaceSturdy", getter, pos, direction);

            Method dampening;
            Object[] wa;
            Class<?>[] worldTypes;
            try {
                dampening = firstMethod(stateClass, "getLightDampening", "getLightBlock");
                wa = new Object[0];
                worldTypes = new Class<?>[0];
            } catch (NoSuchMethodException e) {
                // Pre-1.21.2: the light/shape getters take a world context,
                // which vanilla's own state cache fed with the empty getter.
                wa = new Object[] { emptyGetter, zeroPos };
                worldTypes = new Class<?>[] { getter, pos };
                dampening = stateClass.getMethod("getLightBlock", getter, pos);
            }
            worldArgs = wa;
            getLightDampening = dampening;
            propagatesSkylightDown = stateClass.getMethod("propagatesSkylightDown", worldTypes);
            Class<?>[] faceTypes = new Class<?>[worldTypes.length + 1];
            System.arraycopy(worldTypes, 0, faceTypes, 0, worldTypes.length);
            faceTypes[worldTypes.length] = direction;
            getFaceOcclusionShape = stateClass.getMethod("getFaceOcclusionShape", faceTypes);
            Class<?> voxelShape = Class.forName("net.minecraft.world.phys.shapes.VoxelShape");
            shapeIsEmpty = voxelShape.getMethod("isEmpty");
            toAabbs = voxelShape.getMethod("toAabbs");
            getBlock = stateClass.getMethod("getBlock");
            hasCollision = Class.forName("net.minecraft.world.level.block.state.BlockBehaviour")
                    .getDeclaredField("hasCollision");
            hasCollision.setAccessible(true);
            aabbClass = Class.forName("net.minecraft.world.phys.AABB");
            vec3Class = Class.forName("net.minecraft.world.phys.Vec3");
        }

        Field aabb(String name) throws Exception {
            Field f = aabbFields.get(name);
            if (f == null) {
                f = aabbClass.getField(name);
                aabbFields.put(name, f);
            }
            return f;
        }

        Field vec3(String name) throws Exception {
            Field f = vec3Fields.get(name);
            if (f == null) {
                f = vec3Class.getField(name);
                vec3Fields.put(name, f);
            }
            return f;
        }

        /** BlockState.getOffset(pos) as {x, y, z}. */
        double[] offset(Object state, Object pos) throws Exception {
            Object vec = getOffsetTakesGetter
                    ? getOffset.invoke(state, emptyGetter, pos)
                    : getOffset.invoke(state, pos);
            return new double[] {
                vec3("x").getDouble(vec), vec3("y").getDouble(vec), vec3("z").getDouble(vec)
            };
        }

        Object invokeWorld(Method method, Object state, Object... extra) throws Exception {
            Object[] args = new Object[worldArgs.length + extra.length];
            System.arraycopy(worldArgs, 0, args, 0, worldArgs.length);
            System.arraycopy(extra, 0, args, worldArgs.length, extra.length);
            return method.invoke(state, args);
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

    private StateDump() {}
}
