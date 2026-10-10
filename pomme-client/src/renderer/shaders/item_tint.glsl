// The per-frame item tint palette (set 2, binding 0) shared by the item
// vertex shaders. Each item pushes the base/count of its slice; a vertex's
// raw model `tintindex` past that count is untinted.
layout(set = 2, binding = 0, std430) readonly buffer ItemTintPalette {
    uint item_tints[];
};

vec4 resolve_item_tint(uint tint_index, uint base, uint count) {
    if (tint_index >= count) {
        return vec4(1.0);
    }
    uint color = item_tints[base + tint_index];
    return vec4(
        float((color >> 16) & 255u),
        float((color >> 8) & 255u),
        float(color & 255u),
        float((color >> 24) & 255u)
    ) / 255.0;
}
