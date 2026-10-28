//! Translated pack GLSL to SPIR-V through naga, in pure Rust.

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ShaderStage {
    Vertex,
    Fragment,
}

#[derive(Debug, thiserror::Error)]
pub enum CompileError {
    #[error("{stage:?} shader: {message}")]
    Parse { stage: ShaderStage, message: String },
    #[error("{stage:?} shader doesn't validate: {message}")]
    Validate { stage: ShaderStage, message: String },
    #[error("{stage:?} shader: SPIR-V output failed: {message}")]
    Output { stage: ShaderStage, message: String },
}

/// Compiles Vulkan GLSL 450 to SPIR-V words.
pub fn compile(stage: ShaderStage, source: &str) -> Result<Vec<u32>, CompileError> {
    let naga_stage = match stage {
        ShaderStage::Vertex => naga::ShaderStage::Vertex,
        ShaderStage::Fragment => naga::ShaderStage::Fragment,
    };
    let module = naga::front::glsl::Frontend::default()
        .parse(&naga::front::glsl::Options::from(naga_stage), source)
        .map_err(|e| CompileError::Parse {
            stage,
            message: e.emit_to_string(source),
        })?;
    let info = naga::valid::Validator::new(
        naga::valid::ValidationFlags::all(),
        naga::valid::Capabilities::all(),
    )
    .validate(&module)
    .map_err(|e| CompileError::Validate {
        stage,
        message: e.emit_to_string(source),
    })?;
    // Packs draw in OpenGL's orientation; naga's default flips `gl_Position.y`
    // into Vulkan's, which would turn every pass, full-screen ones too.
    let mut options = naga::back::spv::Options::default();
    options
        .flags
        .remove(naga::back::spv::WriterFlags::ADJUST_COORDINATE_SPACE);
    naga::back::spv::write_vec(&module, &info, &options, None).map_err(|e| CompileError::Output {
        stage,
        message: e.to_string(),
    })
}
