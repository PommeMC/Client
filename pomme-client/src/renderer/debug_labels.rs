use std::ffi::{CStr, c_char};
use std::mem::transmute;

use pyronyx::vk;

type VoidFunction = Option<extern "system" fn()>;
type GetDeviceProcAddr = unsafe extern "system" fn(vk::vkDevice, *const c_char) -> VoidFunction;
type BeginDebugUtilsLabel =
    unsafe extern "system" fn(vk::vkCommandBuffer, *const vk::DebugUtilsLabelEXT);
type EndDebugUtilsLabel = unsafe extern "system" fn(vk::vkCommandBuffer);

/// Optional command-buffer labels. When disabled, creating a scope is a no-op.
pub(super) struct DebugLabels {
    begin: Option<BeginDebugUtilsLabel>,
    end: Option<EndDebugUtilsLabel>,
}

impl DebugLabels {
    pub fn new(enabled: bool, instance: &vk::Instance, device: &vk::Device) -> Self {
        if !enabled {
            return Self {
                begin: None,
                end: None,
            };
        }

        // VK_EXT_debug_utils is an instance extension. Pyronyx only adds its
        // command-buffer table when it appears in DeviceCreateInfo (where it
        // is invalid), so load these two device commands directly instead.
        let Some(get_device_proc_addr) = (unsafe {
            vk::get_instance_proc_addr(instance.handle(), c"vkGetDeviceProcAddr".as_ptr())
        }) else {
            tracing::warn!("--debug-labels requested, but vkGetDeviceProcAddr is unavailable");
            return Self {
                begin: None,
                end: None,
            };
        };
        let get_device_proc_addr: GetDeviceProcAddr = unsafe { transmute(get_device_proc_addr) };
        let begin: Option<BeginDebugUtilsLabel> = unsafe {
            transmute(get_device_proc_addr(
                device.handle(),
                c"vkCmdBeginDebugUtilsLabelEXT".as_ptr(),
            ))
        };
        let end: Option<EndDebugUtilsLabel> = unsafe {
            transmute(get_device_proc_addr(
                device.handle(),
                c"vkCmdEndDebugUtilsLabelEXT".as_ptr(),
            ))
        };
        if begin.is_none() || end.is_none() {
            tracing::warn!(
                "--debug-labels requested, but VK_EXT_debug_utils commands are unavailable"
            );
        }
        Self { begin, end }
    }

    pub fn scope(&self, cmd: vk::CommandBuffer, name: &CStr) -> DebugLabelScope {
        self.colored_scope(cmd, name, [0.22, 0.55, 0.9, 1.0])
    }

    pub fn colored_scope(
        &self,
        cmd: vk::CommandBuffer,
        name: &CStr,
        color: [f32; 4],
    ) -> DebugLabelScope {
        if let Some(begin) = self.begin {
            unsafe {
                begin(
                    cmd.handle(),
                    &vk::DebugUtilsLabelEXT {
                        label_name: name.as_ptr(),
                        color,
                        ..Default::default()
                    },
                );
            }
        }
        DebugLabelScope { cmd, end: self.end }
    }
}

/// Ends its label on drop, or explicitly through [`Self::end`].
pub(super) struct DebugLabelScope {
    cmd: vk::CommandBuffer,
    end: Option<EndDebugUtilsLabel>,
}

impl DebugLabelScope {
    pub fn end(mut self) {
        self.finish();
    }

    fn finish(&mut self) {
        if let Some(end) = self.end.take() {
            unsafe { end(self.cmd.handle()) };
        }
    }
}

impl Drop for DebugLabelScope {
    fn drop(&mut self) {
        self.finish();
    }
}
