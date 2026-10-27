use std::fs;
use std::path::Path;

// Single source of truth for the vertex packing constants, shared with the
// shaders via the generated packing.glsl.
include!("src/renderer/chunk/packing_consts.rs");

/// `PackedVertex::uv` back to a sprite-local UV, for `packing.glsl`.
const UNPACK_SPRITE_UV: &str = "vec2 unpack_sprite_uv(uint uv) {
    uint mask = (1u << TERRAIN_UV_BITS) - 1u;
    return vec2(uv & mask, (uv >> TERRAIN_UV_BITS) & mask) / TERRAIN_UV_FIXED_SCALE;
}
";

fn main() {
    // `mobile` covers what Android and iOS share: touch, the soft keyboard,
    // no launcher.
    println!("cargo::rustc-check-cfg=cfg(mobile)");
    if matches!(
        std::env::var("CARGO_CFG_TARGET_OS").as_deref(),
        Ok("android" | "ios")
    ) {
        println!("cargo:rustc-cfg=mobile");
    }

    // Releases bundle the Vulkan loader (libvulkan.1.dylib) next to the binary,
    // since macOS has no system Vulkan; this rpath lets it be found at runtime.
    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() == Ok("macos") {
        println!("cargo:rustc-link-arg=-Wl,-rpath,@executable_path");
    }

    // The exe's own icon, for Explorer, the taskbar and shortcuts.
    #[cfg(windows)]
    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() == Ok("windows") {
        winresource::WindowsResource::new()
            .set_icon("assets/icon.ico")
            .compile()
            .expect("failed to embed the exe icon");
    }

    let shader_dir = Path::new("src/renderer/shaders");
    let out_dir = std::env::var("OUT_DIR").unwrap();

    compress_block_data(Path::new(&out_dir));

    println!("cargo:rerun-if-changed=src/renderer/chunk/packing_consts.rs");
    fs::write(
        Path::new(&out_dir).join("packing.glsl"),
        format!(
            "const float POS_RANGE = {POS_RANGE:?};\nconst float POS_BIAS = {POS_BIAS:?};\nconst float TERRAIN_UV_FIXED_SCALE = {TERRAIN_UV_FIXED_SCALE:?};\nconst uint TERRAIN_UV_BITS = {TERRAIN_UV_BITS}u;\n{UNPACK_SPRITE_UV}"
        ),
    )
    .expect("failed to write packing.glsl");

    let compiler = shaderc::Compiler::new().expect("failed to create shaderc compiler");
    let mut options = shaderc::CompileOptions::new().expect("failed to create compile options");
    options.set_target_env(
        shaderc::TargetEnv::Vulkan,
        shaderc::EnvVersion::Vulkan1_0 as u32,
    );
    options.set_source_language(shaderc::SourceLanguage::GLSL);

    // Includes resolve from the shader dir, falling back to OUT_DIR for
    // generated headers (packing.glsl).
    let include_dir = shader_dir.to_path_buf();
    let generated_dir = Path::new(&out_dir).to_path_buf();
    options.set_include_callback(move |requested, _ty, _requesting, _depth| {
        let mut path = include_dir.join(requested);
        if !path.exists() {
            path = generated_dir.join(requested);
        }
        let content = fs::read_to_string(&path)
            .map_err(|e| format!("failed to read shader include {requested}: {e}"))?;
        Ok(shaderc::ResolvedInclude {
            resolved_name: path.to_string_lossy().into_owned(),
            content,
        })
    });
    for include in [
        "fog.glsl",
        "camera_ubo.glsl",
        "atlas_sprite.glsl",
        "item_tint.glsl",
        "item_instance.glsl",
    ] {
        println!(
            "cargo:rerun-if-changed={}",
            shader_dir.join(include).display()
        );
    }

    let shaders = [
        ("chunk.vert", shaderc::ShaderKind::Vertex),
        ("chunk.frag", shaderc::ShaderKind::Fragment),
        ("chunk_solid.frag", shaderc::ShaderKind::Fragment),
        ("water.vert", shaderc::ShaderKind::Vertex),
        ("water.frag", shaderc::ShaderKind::Fragment),
        ("cube.vert", shaderc::ShaderKind::Vertex),
        ("cube.frag", shaderc::ShaderKind::Fragment),
        ("panorama.vert", shaderc::ShaderKind::Vertex),
        ("panorama.frag", shaderc::ShaderKind::Fragment),
        ("hand.vert", shaderc::ShaderKind::Vertex),
        ("hand.frag", shaderc::ShaderKind::Fragment),
        ("preview.frag", shaderc::ShaderKind::Fragment),
        ("menu_overlay.vert", shaderc::ShaderKind::Vertex),
        ("menu_overlay.frag", shaderc::ShaderKind::Fragment),
        ("block_overlay.vert", shaderc::ShaderKind::Vertex),
        ("block_overlay.frag", shaderc::ShaderKind::Fragment),
        ("sky.vert", shaderc::ShaderKind::Vertex),
        ("sky.frag", shaderc::ShaderKind::Fragment),
        ("cull.comp", shaderc::ShaderKind::Compute),
        ("blur.vert", shaderc::ShaderKind::Vertex),
        ("entity.vert", shaderc::ShaderKind::Vertex),
        ("entity.frag", shaderc::ShaderKind::Fragment),
        ("entity_outline.frag", shaderc::ShaderKind::Fragment),
        ("entity_sobel.frag", shaderc::ShaderKind::Fragment),
        (
            "entity_outline_box_blur.frag",
            shaderc::ShaderKind::Fragment,
        ),
        (
            "entity_outline_composite.frag",
            shaderc::ShaderKind::Fragment,
        ),
        ("post_blit.frag", shaderc::ShaderKind::Fragment),
        ("lightmap.frag", shaderc::ShaderKind::Fragment),
        ("post_flip.frag", shaderc::ShaderKind::Fragment),
        ("post_invert.frag", shaderc::ShaderKind::Fragment),
        ("post_color_convolve.frag", shaderc::ShaderKind::Fragment),
        ("post_bits.frag", shaderc::ShaderKind::Fragment),
        ("post_box_blur.frag", shaderc::ShaderKind::Fragment),
        ("post_rotscale.vert", shaderc::ShaderKind::Vertex),
        ("post_spiderclip.frag", shaderc::ShaderKind::Fragment),
        ("block_entity.vert", shaderc::ShaderKind::Vertex),
        ("chunk_border.vert", shaderc::ShaderKind::Vertex),
        ("chunk_border.frag", shaderc::ShaderKind::Fragment),
        ("item_entity.vert", shaderc::ShaderKind::Vertex),
        ("item_entity_world.vert", shaderc::ShaderKind::Vertex),
        ("item_entity.frag", shaderc::ShaderKind::Fragment),
        ("item_outline.vert", shaderc::ShaderKind::Vertex),
        ("weather.vert", shaderc::ShaderKind::Vertex),
        ("weather.frag", shaderc::ShaderKind::Fragment),
        ("particle.vert", shaderc::ShaderKind::Vertex),
        ("particle.frag", shaderc::ShaderKind::Fragment),
        ("clouds.vert", shaderc::ShaderKind::Vertex),
        ("clouds.frag", shaderc::ShaderKind::Fragment),
        ("entity_shadow.vert", shaderc::ShaderKind::Vertex),
        ("entity_shadow.frag", shaderc::ShaderKind::Fragment),
    ];

    for (file, kind) in &shaders {
        let path = shader_dir.join(file);
        println!("cargo:rerun-if-changed={}", path.display());

        let source = fs::read_to_string(&path)
            .unwrap_or_else(|e| panic!("failed to read {}: {e}", path.display()));

        let artifact = compiler
            .compile_into_spirv(&source, *kind, file, "main", Some(&options))
            .unwrap_or_else(|e| panic!("failed to compile {file}: {e}"));

        let out_name = format!("{file}.spv");
        let out_path = Path::new(&out_dir).join(&out_name);
        fs::write(&out_path, artifact.as_binary_u8())
            .unwrap_or_else(|e| panic!("failed to write {}: {e}", out_path.display()));
    }
}

/// Deflates every block data table into `OUT_DIR/block-data/`, which
/// `world::block` embeds: the JSON is mostly repeated shape and state data
/// and compresses about twentyfold.
fn compress_block_data(out_dir: &Path) {
    use std::io::Read;

    let data_dir = Path::new("src/world/block/data");
    println!("cargo:rerun-if-changed={}", data_dir.display());
    let out = out_dir.join("block-data");
    fs::create_dir_all(&out).expect("failed to create the block data dir");
    for entry in fs::read_dir(data_dir).expect("failed to read the block data dir") {
        let path = entry.expect("failed to read a block data entry").path();
        let json =
            fs::read(&path).unwrap_or_else(|e| panic!("failed to read {}: {e}", path.display()));
        let mut deflated = Vec::new();
        flate2::read::DeflateEncoder::new(&json[..], flate2::Compression::best())
            .read_to_end(&mut deflated)
            .expect("failed to deflate block data");
        let out_path = out.join(path.file_name().expect("block data file name"));
        fs::write(&out_path, deflated)
            .unwrap_or_else(|e| panic!("failed to write {}: {e}", out_path.display()));
    }
}
