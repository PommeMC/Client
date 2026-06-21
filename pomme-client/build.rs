use std::fs;
use std::path::Path;

fn main() {
    // Releases bundle the Vulkan loader (libvulkan.1.dylib) next to the binary,
    // since macOS has no system Vulkan; this rpath lets it be found at runtime.
    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() == Ok("macos") {
        println!("cargo:rustc-link-arg=-Wl,-rpath,@executable_path");
    }

    let shader_dir = Path::new("src/renderer/shaders");
    let out_dir = std::env::var("OUT_DIR").unwrap();

    let compiler = shaderc::Compiler::new().expect("failed to create shaderc compiler");
    let mut options = shaderc::CompileOptions::new().expect("failed to create compile options");
    options.set_target_env(
        shaderc::TargetEnv::Vulkan,
        shaderc::EnvVersion::Vulkan1_0 as u32,
    );
    options.set_source_language(shaderc::SourceLanguage::GLSL);

    let include_dir = shader_dir.to_path_buf();
    options.set_include_callback(move |requested, _ty, _requesting, _depth| {
        let path = include_dir.join(requested);
        let content = fs::read_to_string(&path)
            .map_err(|e| format!("failed to read shader include {requested}: {e}"))?;
        Ok(shaderc::ResolvedInclude {
            resolved_name: path.to_string_lossy().into_owned(),
            content,
        })
    });
    println!(
        "cargo:rerun-if-changed={}",
        shader_dir.join("fog.glsl").display()
    );

    let shaders = [
        ("chunk.vert", shaderc::ShaderKind::Vertex),
        ("chunk.frag", shaderc::ShaderKind::Fragment),
        ("cube.vert", shaderc::ShaderKind::Vertex),
        ("cube.frag", shaderc::ShaderKind::Fragment),
        ("panorama.vert", shaderc::ShaderKind::Vertex),
        ("panorama.frag", shaderc::ShaderKind::Fragment),
        ("hand.vert", shaderc::ShaderKind::Vertex),
        ("hand.frag", shaderc::ShaderKind::Fragment),
        ("menu_overlay.vert", shaderc::ShaderKind::Vertex),
        ("menu_overlay.frag", shaderc::ShaderKind::Fragment),
        ("block_overlay.vert", shaderc::ShaderKind::Vertex),
        ("block_overlay.frag", shaderc::ShaderKind::Fragment),
        ("sky.vert", shaderc::ShaderKind::Vertex),
        ("sky.frag", shaderc::ShaderKind::Fragment),
        ("cull.comp", shaderc::ShaderKind::Compute),
        ("blur.vert", shaderc::ShaderKind::Vertex),
        ("blur.frag", shaderc::ShaderKind::Fragment),
        ("entity.vert", shaderc::ShaderKind::Vertex),
        ("entity.frag", shaderc::ShaderKind::Fragment),
        ("chunk_border.vert", shaderc::ShaderKind::Vertex),
        ("chunk_border.frag", shaderc::ShaderKind::Fragment),
        ("item_entity.vert", shaderc::ShaderKind::Vertex),
        ("item_entity.frag", shaderc::ShaderKind::Fragment),
        ("weather.vert", shaderc::ShaderKind::Vertex),
        ("weather.frag", shaderc::ShaderKind::Fragment),
        ("clouds.vert", shaderc::ShaderKind::Vertex),
        ("clouds.frag", shaderc::ShaderKind::Fragment),
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
