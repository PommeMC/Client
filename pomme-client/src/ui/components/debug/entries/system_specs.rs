use std::sync::OnceLock;

use sysinfo::{CpuRefreshKind, System};
use winit::window::Window;

use crate::renderer::{GpuType, Renderer};
use crate::ui::components::debug::displayer::DebugScreenDisplayer;
use crate::ui::components::debug::entry::DebugEntry;
use crate::ui::components::debug::groups::DebugGroup;
use crate::ui::components::debug::info::DebugInfo;

pub struct SystemSpecsDebugEntry;

pub struct SystemSpecsInfo<'a> {
    pub display_width: u32,
    pub display_height: u32,
    pub vendor_name: &'a str,
    pub window_width: u32,
    pub window_height: u32,
    pub pixel_density: f64,
    pub gpu_name: &'a str,
    pub gpu_type: GpuType,
    pub vulkan_version: &'a str,
    pub driver_info: Option<&'a str>,
}

impl<'a> SystemSpecsInfo<'a> {
    pub fn new(window: &Window, renderer: &'a Renderer) -> Self {
        let display_size = window.inner_size();
        let pixel_density = window.scale_factor();
        let window_size = window.inner_size().to_logical(pixel_density);

        Self {
            display_width: display_size.width,
            display_height: display_size.height,
            vendor_name: vendor_name(renderer.gpu_vendor_id()),
            window_width: window_size.width,
            window_height: window_size.height,
            pixel_density,
            gpu_name: renderer.gpu_name(),
            gpu_type: renderer.gpu_type(),
            vulkan_version: renderer.vulkan_version(),
            driver_info: renderer.driver_info(),
        }
    }
}

fn cpu_info() -> &'static str {
    static CPU: OnceLock<String> = OnceLock::new();
    CPU.get_or_init(|| {
        let mut sys = System::new();
        sys.refresh_cpu_specifics(CpuRefreshKind::nothing());
        match sys.cpus().first() {
            Some(cpu) => {
                let name = cpu.brand().split_whitespace().collect::<Vec<_>>().join(" ");
                format!("{}x {name}", sys.cpus().len())
            }
            None => "<unknown>".to_owned(),
        }
    })
}

fn vendor_name(id: u32) -> &'static str {
    match id {
        0x10DE => "NVIDIA",
        0x1002 => "AMD",
        0x8086 => "INTEL",
        0x106B => "APPLE",
        0x13B5 => "ARM",
        0x5143 => "QUALCOMM",
        0x1010 => "IMAGINATION",
        _ => "Unknown",
    }
}

fn first_line(s: &str) -> &str {
    s.lines().next().unwrap_or(s)
}

impl DebugEntry for SystemSpecsDebugEntry {
    fn display(&mut self, displayer: &mut DebugScreenDisplayer, info: &DebugInfo) {
        let s = &info.system;

        displayer.add_fact_to_group(DebugGroup::SystemSpecs, "Rust", |f| {
            f.value(env!("RUSTC_VERSION"));
        });
        displayer.add_fact_to_group(DebugGroup::SystemSpecs, "CPU", |f| {
            f.value(cpu_info());
        });
        displayer.add_fact_to_group(DebugGroup::SystemSpecs, "Display", |f| {
            f.value(s.display_width)
                .text("x")
                .value(s.display_height)
                .text(" (")
                .value(s.vendor_name)
                .text(")");
        });
        displayer.add_fact_to_group(DebugGroup::SystemSpecs, "Window", |f| {
            f.value(s.window_width)
                .text("x")
                .value(s.window_height)
                .text(" (")
                .value(format!("{:.2}", s.pixel_density))
                .text("x pixel density)");
        });

        let type_name = match s.gpu_type {
            GpuType::Other => "",
            GpuType::Integrated => " (iGPU)",
            GpuType::Discrete => " (dGPU)",
            GpuType::Virtual => " (vGPU)",
            GpuType::Cpu => " (software)",
        };
        displayer.add_to_group(
            DebugGroup::SystemSpecs,
            format!("{}{type_name}", s.gpu_name),
        );
        if let Some(driver_info) = s.driver_info {
            displayer.add_to_group(
                DebugGroup::SystemSpecs,
                format!("{} {}", s.vulkan_version, first_line(driver_info)),
            );
        }
    }
}
