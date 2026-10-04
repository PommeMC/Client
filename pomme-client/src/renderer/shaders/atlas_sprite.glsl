// Level-0 sprite rectangles `(x, y, width, height)` in atlas texels, indexed by
// the vertex's sprite id; index 0 is the missing tile.
layout(set = 1, binding = 1) readonly buffer SpriteRects {
    uvec4 sprite_rects[];
};

vec4 sample_atlas_sprite(sampler2D atlas_texture, vec2 sprite_uv, uint sprite) {
    uvec4 atlas_rect = sprite_rects[sprite];
    vec2 atlas_size = vec2(textureSize(atlas_texture, 0));
    vec2 sprite_size = vec2(atlas_rect.zw);
    vec2 atlas_origin = vec2(atlas_rect.xy);

    // `sprite_uv` remains unwrapped for derivatives. Taking derivatives after
    // fract() would create a huge gradient at every block boundary and force a
    // coarser mip exactly where a greedy quad repeats the texture.
    vec2 atlas_scale = sprite_size / atlas_size;
    vec2 atlas_uv = (atlas_origin + fract(sprite_uv) * sprite_size) / atlas_size;
    return textureGrad(
        atlas_texture,
        atlas_uv,
        dFdx(sprite_uv) * atlas_scale,
        dFdy(sprite_uv) * atlas_scale
    );
}
