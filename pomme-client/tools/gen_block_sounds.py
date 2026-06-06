#!/usr/bin/env python3
"""Generate block_sounds.json: block id -> [hit_event, volume, pitch].

Vanilla plays a block's `SoundType` hit sound while it is being mined
(MultiPlayerGameMode.continueDestroyBlock). azalea-block does not expose sound
type, so we extract the full block -> SoundType mapping from the decompiled
vanilla source and emit a table the client embeds.

This is a one-off generator: run it after a version bump and commit the output
(pomme-client/src/world/block/block_sounds.json). It needs nothing but the
decompiled reference tree.

    python pomme-client/tools/gen_block_sounds.py
"""
import json
import re
import sys
from pathlib import Path

VER = "26.2-pre-4"
ROOT = Path(__file__).resolve().parents[2]  # repo root (D:\MC Client)
DECOMP = ROOT / "reference" / VER / "decompiled" / "net" / "minecraft"
SOUND_EVENTS = DECOMP / "sounds" / "SoundEvents.java"
SOUND_TYPE = DECOMP / "world" / "level" / "block" / "SoundType.java"
BLOCKS = DECOMP / "world" / "level" / "block" / "Blocks.java"
OUT = ROOT / "pomme-client" / "src" / "world" / "block" / "block_sounds.json"

# DyeColor.values() order is irrelevant for sound (all share one SoundType);
# only the id suffixes matter.
DYE_COLORS = [
    "white", "orange", "magenta", "light_blue", "yellow", "lime", "pink",
    "gray", "light_gray", "cyan", "purple", "blue", "brown", "green", "red",
    "black",
]

# ColorCollection.registerBlocks(BlockItemIds.<CONST>, ...) -> per-color id
# suffix. The SoundType is still resolved from the lambda body like any other
# Properties expression; only the id expansion is encoded here.
FAMILY_SUFFIX = {
    "WOOL": "wool",
    "CARPET": "carpet",
    "BED": "bed",
    "STAINED_GLASS": "stained_glass",
    "STAINED_GLASS_PANE": "stained_glass_pane",
    "CONCRETE": "concrete",
    "CONCRETE_POWDER": "concrete_powder",
    "BANNER": "banner",
    "WALL_BANNER": "wall_banner",
    "DYED_TERRACOTTA": "terracotta",
    "GLAZED_TERRACOTTA": "glazed_terracotta",
    "DYED_SHULKER_BOX": "shulker_box",
    "DYED_CANDLE": "candle",
    "DYED_CANDLE_CAKE": "candle_cake",
}

# WeatheringCopperCollection registers 8 blocks per family (4 weather states x
# waxed/unwaxed). Naming is regular ("exposed_<base>", "waxed_<base>", ...)
# except COPPER_BLOCK, whose variants drop the "_block" suffix.
COPPER_BLOCK_IDS = [
    "copper_block", "exposed_copper", "weathered_copper", "oxidized_copper",
    "waxed_copper_block", "waxed_exposed_copper", "waxed_weathered_copper",
    "waxed_oxidized_copper",
]


def copper_ids(idc):
    if idc == "COPPER_BLOCK":
        return COPPER_BLOCK_IDS
    b = idc.lower()
    return [
        b, f"exposed_{b}", f"weathered_{b}", f"oxidized_{b}",
        f"waxed_{b}", f"waxed_exposed_{b}", f"waxed_weathered_{b}",
        f"waxed_oxidized_{b}",
    ]


# Doors / trapdoors / buttons / pressure plates take a BlockSetType, and signs /
# wall signs / fence gates take a WoodType; their constructors call
# `properties.sound(type.soundType())`, overriding the Properties sound. Both
# enums share the same names, so one table covers `<Type>.soundType()`.
TYPE_SOUND = {
    "OAK": "WOOD", "SPRUCE": "WOOD", "BIRCH": "WOOD", "ACACIA": "WOOD",
    "JUNGLE": "WOOD", "DARK_OAK": "WOOD", "PALE_OAK": "WOOD", "MANGROVE": "WOOD",
    "CHERRY": "CHERRY_WOOD", "CRIMSON": "NETHER_WOOD", "WARPED": "NETHER_WOOD",
    "BAMBOO": "BAMBOO_WOOD",
    "IRON": "IRON", "COPPER": "COPPER", "GOLD": "METAL",
    "STONE": "STONE", "POLISHED_BLACKSTONE": "STONE",
}

# Hanging signs (Ceiling/WallHangingSignBlock) use `WoodType.hangingSignSoundType()`.
WOOD_HANGING_SOUND = {
    "CHERRY": "CHERRY_WOOD_HANGING_SIGN",
    "CRIMSON": "NETHER_WOOD_HANGING_SIGN",
    "WARPED": "NETHER_WOOD_HANGING_SIGN",
    "BAMBOO": "BAMBOO_WOOD_HANGING_SIGN",
}  # all other wood types -> HANGING_SIGN


def parse_sound_events():
    """const name -> event string, e.g. STONE_HIT -> 'block.stone.hit'."""
    text = SOUND_EVENTS.read_text(encoding="utf8")
    out = {}
    for m in re.finditer(r'(\w+)\s*=\s*SoundEvents\.register\("([^"]+)"\)', text):
        out[m.group(1)] = m.group(2)
    return out


def parse_sound_types(events):
    """SoundType name -> (volume, pitch, hit_event, break_event); None = silent."""
    text = SOUND_TYPE.read_text(encoding="utf8")

    def event_for(arg):
        const = arg.split(".")[-1]  # SoundEvents.STONE_HIT -> STONE_HIT
        return None if const == "EMPTY" else events[const]

    out = {}
    for m in re.finditer(
        r"public static final SoundType (\w+) = new SoundType\(([^;]+)\);", text
    ):
        name = m.group(1)
        args = [a.strip() for a in m.group(2).split(",")]
        vol = float(args[0].rstrip("f"))
        pitch = float(args[1].rstrip("f"))
        # SoundType(volume, pitch, break, step, place, hit, fall)
        out[name] = (vol, pitch, event_for(args[5]), event_for(args[2]))
    return out


def parse_block_decls():
    """List of (field_name, expr) for every block / collection declaration.

    The RHS of a decl can span many lines and embed semicolons inside lambda
    bodies / switch expressions, so we balance ()/{}/[] and end the statement
    only at a depth-0 ';'.
    """
    text = BLOCKS.read_text(encoding="utf8")
    decls = []
    start_re = re.compile(
        r"public static final (?:Block|\w*Collection<Block>) (\w+) = "
    )
    n = len(text)
    for m in start_re.finditer(text):
        field = m.group(1)
        depth = 0
        j = m.end()
        while j < n:
            c = text[j]
            if c in "([{":
                depth += 1
            elif c in ")]}":
                depth -= 1
            elif c == ";" and depth == 0:
                break
            j += 1
        decls.append((field, text[m.end():j]))
    return decls


def first_id_const(expr):
    m = re.search(r"\b(?:BlockItemIds|BlockIds)\.(\w+)", expr)
    return m.group(1) if m else None


def build():
    events = parse_sound_events()
    soundtypes = parse_sound_types(events)
    decls = parse_block_decls()
    exprs = {field: expr for field, expr in decls}

    memo = {}

    def resolve_sound(expr, stack):
        # 0. set/wood-type blocks: the constructor applies the type's sound last,
        # so it wins over the Properties chain. Hanging signs use a separate
        # hanging-sign sound.
        if "HangingSign" in expr:
            m = re.search(r"WoodType\.(\w+)", expr)
            if m:
                return WOOD_HANGING_SOUND.get(m.group(1), "HANGING_SIGN")
        m = re.search(r"\b(?:WoodType|BlockSetType)\.(\w+)", expr)
        if m:
            return TYPE_SOUND[m.group(1)]
        # 1. inline .sound(SoundType.X) wins (last occurrence = final override).
        inline = re.findall(r"\.sound\(SoundType\.(\w+)\)", expr)
        if inline:
            return inline[-1]
        # 2. property-helper methods that carry a sound. logProperties /
        # leavesProperties take the SoundType as an arg, and it is the only
        # SoundType reference in such a decl.
        if "logProperties(" in expr or "leavesProperties(" in expr:
            m = re.search(r"SoundType\.(\w+)", expr)
            if m:
                return m.group(1)
        if "netherStemProperties(" in expr:
            return "STEM"
        if "candleProperties(" in expr:
            return "CANDLE"
        # 3. copied properties inherit the source block's sound. The base may be
        # followed by a chain (e.g. ofFullCopy(COPPER_BLOCK.weathering().pick(..))).
        m = re.search(r"of(?:Full|Legacy)Copy\((?:Blocks\.)?(\w+)", expr)
        if m:
            return resolve_field(m.group(1), stack)
        # 4. stair / slab / wall helpers copy from their base block (2nd arg).
        m = re.search(
            r"register(?:Legacy)?(?:Stair|Slab|Wall)\([^,]+,\s*(?:Blocks\.)?(\w+)\)",
            expr,
        )
        if m:
            return resolve_field(m.group(1), stack)
        # 5. BlockBehaviour.Properties default.
        return "STONE"

    def resolve_field(field, stack=()):
        if field in memo:
            return memo[field]
        if field in stack:  # copy cycle guard
            return "STONE"
        expr = exprs.get(field)
        if expr is None:  # base lives in another class; assume default
            return "STONE"
        st = resolve_sound(expr, stack + (field,))
        memo[field] = st
        return st

    out = {}
    by_soundtype = {}
    skipped = []
    for field, expr in decls:
        idc = first_id_const(expr)
        if idc is None:
            skipped.append(field)
            continue
        st = resolve_field(field)
        if st not in soundtypes:
            sys.exit(f"ERROR: {field} resolved to unknown SoundType {st!r}")
        vol, pitch, hit, brk = soundtypes[st]
        entry = [hit or "", brk or "", vol, pitch]
        stripped = expr.strip()
        if stripped.startswith("ColorCollection.registerBlocks"):
            suffix = FAMILY_SUFFIX.get(idc)
            if suffix is None:
                sys.exit(f"ERROR: unknown color family {idc} ({field})")
            ids = [f"{c}_{suffix}" for c in DYE_COLORS]
        elif stripped.startswith("WeatheringCopperCollection.registerBlocks"):
            ids = copper_ids(idc)
        else:
            ids = [idc.lower()]
        for bid in ids:
            out[bid] = entry
            by_soundtype.setdefault(st, []).append(bid)

    items = sorted(out.items())
    lines = ["{"]
    for i, (k, v) in enumerate(items):
        comma = "," if i < len(items) - 1 else ""
        lines.append(f"  {json.dumps(k)}: {json.dumps(v)}{comma}")
    lines.append("}")
    OUT.write_text("\n".join(lines) + "\n", encoding="utf8")

    # ---- report ----------------------------------------------------------
    print(f"wrote {len(out)} block ids -> {OUT}")
    print(f"SoundTypes used: {len(by_soundtype)}")
    for st in sorted(by_soundtype, key=lambda k: -len(by_soundtype[k])):
        print(f"  {st:24} {len(by_soundtype[st]):4}")
    if skipped:
        print(f"decls with no id const (skipped): {skipped}")

    checks = [
        ("stone", "block.stone.hit"),
        ("oak_planks", "block.wood.hit"),
        ("oak_log", "block.wood.hit"),
        ("oak_stairs", "block.wood.hit"),
        ("dirt", "block.gravel.hit"),
        ("gravel", "block.gravel.hit"),
        ("grass_block", "block.grass.hit"),
        ("sand", "block.sand.hit"),
        ("glass", "block.glass.hit"),
        ("white_wool", "block.wool.hit"),
        ("white_carpet", "block.wool.hit"),
        ("white_concrete", "block.stone.hit"),
        ("white_terracotta", "block.stone.hit"),
        ("white_candle", "block.candle.hit"),
        ("white_stained_glass", "block.glass.hit"),
        ("deepslate", "block.deepslate.hit"),
        ("netherrack", "block.netherrack.hit"),
        ("oak_leaves", "block.grass.hit"),
        ("copper_block", "block.copper.hit"),
        ("exposed_copper", "block.copper.hit"),
        ("waxed_oxidized_copper", "block.copper.hit"),
        ("cut_copper_stairs", "block.copper.hit"),
        ("copper_bulb", "block.copper_bulb.hit"),
        ("copper_grate", "block.copper_grate.hit"),
        ("copper_chain", "block.chain.hit"),
        ("lightning_rod", "block.copper.hit"),
        ("oak_door", "block.wood.hit"),
        ("oak_trapdoor", "block.wood.hit"),
        ("oak_button", "block.wood.hit"),
        ("oak_pressure_plate", "block.wood.hit"),
        ("oak_fence_gate", "block.wood.hit"),
        ("oak_sign", "block.wood.hit"),
        ("oak_hanging_sign", "block.hanging_sign.hit"),
        ("bamboo_door", "block.bamboo_wood.hit"),
        ("warped_door", "block.nether_wood.hit"),
        ("cherry_hanging_sign", "block.cherry_wood_hanging_sign.hit"),
        ("iron_door", "block.iron.hit"),
        ("iron_trapdoor", "block.iron.hit"),
        ("copper_door", "block.copper.hit"),
        ("polished_blackstone_button", "block.stone.hit"),
        ("pale_oak_log", "block.wood.hit"),
        ("cactus_flower", ""),  # silent (hit = EMPTY)
    ]
    break_checks = [
        ("stone", "block.stone.break"),
        ("oak_planks", "block.wood.break"),
        ("dirt", "block.gravel.break"),
        ("white_wool", "block.wool.break"),
        ("copper_block", "block.copper.break"),
        ("cactus_flower", "block.cactus_flower.break"),  # break present, hit silent
    ]
    print("spot checks:")
    bad = 0
    for label, idx, items in (("hit", 0, checks), ("break", 1, break_checks)):
        for bid, want in items:
            got = out.get(bid)
            got_ev = got[idx] if got else "<MISSING>"
            ok = got is not None and got_ev == want
            if not ok:
                bad += 1
            print(f"  {'ok ' if ok else 'BAD'} {bid:22} {label}={got_ev!r} (want {want!r})")
    if bad:
        print(f"{bad} spot-check mismatch(es) -- review before committing")


if __name__ == "__main__":
    build()
