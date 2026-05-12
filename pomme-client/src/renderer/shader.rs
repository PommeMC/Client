use crate::renderer::util;
use pyronyx::vk;

pub fn create_shader_module(device: &vk::Device, spirv: &[u8]) -> vk::ShaderModule {
    let code =
        util::read_spv(&mut std::io::Cursor::new(spirv)).expect("failed to read SPIR-V bytecode");

    let create_info = vk::ShaderModuleCreateInfo {
        code_size: code.len() * 4,
        code: code.as_ptr(),
        ..Default::default()
    };

    device
        .create_shader_module(&create_info, None)
        .expect("failed to create shader module")
}

macro_rules! include_spirv {
    ($name:literal) => {
        include_bytes!(concat!(env!("OUT_DIR"), "/", $name))
    };
}

pub(crate) use include_spirv;
