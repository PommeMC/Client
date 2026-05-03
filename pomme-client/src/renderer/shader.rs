use ash::vk;

pub fn create_shader_module(device: &ash::Device, spirv: &[u8]) -> vk::ShaderModule {
    let code = ash::util::read_spv(&mut std::io::Cursor::new(spirv))
        .expect("failed to read SPIR-V bytecode");

    let create_info = vk::ShaderModuleCreateInfo::default().code(&code);

    unsafe {
        device
            .create_shader_module(&create_info, None)
            .expect("failed to create shader module")
    }
}

macro_rules! include_spirv {
    ($name:literal) => {
        include_bytes!(concat!(env!("OUT_DIR"), "/", $name))
    };
}

pub(crate) use include_spirv;
