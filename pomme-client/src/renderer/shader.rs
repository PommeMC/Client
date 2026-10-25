/// The SPIR-V build.rs compiled for `name`.
macro_rules! include_shader {
    ($name:literal) => {
        include_bytes!(concat!(env!("OUT_DIR"), "/", $name, ".spv"))
    };
}

pub(crate) use include_shader;
