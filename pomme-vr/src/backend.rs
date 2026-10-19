//! `StereoBackend` over OpenXR's `XR_KHR_vulkan_enable`, where the client
//! creates the Vulkan instance and device itself to the runtime's
//! requirements (pyronyx can't wrap ones the runtime would create).

use std::ffi::CString;
use std::time::Duration;

use glam::{Quat, Vec3};
use openxr as xr;
use pomme_client::stereo::{
    EyeView, Fov, PanelPlacement, RawHandle, StereoBackend, StereoDevice, StereoFrame, StereoImages,
};

const VIEW_TYPE: xr::ViewConfigurationType = xr::ViewConfigurationType::PRIMARY_STEREO;
/// Between polls while the headset isn't showing the app, so the loop
/// doesn't spin without `xrWaitFrame` to pace it.
const IDLE_POLL: Duration = Duration::from_millis(10);

pub struct OpenXrBackend {
    instance: xr::Instance,
    system: xr::SystemId,
    blend_mode: xr::EnvironmentBlendMode,
    session: Option<Session>,
    exit: bool,
}

/// What `start` creates on the client's device.
struct Session {
    session: xr::Session<xr::Vulkan>,
    frame_waiter: xr::FrameWaiter,
    frame_stream: xr::FrameStream<xr::Vulkan>,
    eyes: Swapchain,
    panel: Swapchain,
    /// The seated space: where the head sat when the session began or was
    /// last recentred, `origin` in the runtime's LOCAL space.
    space: xr::Space,
    origin: xr::Posef,
    head: xr::Space,
    running: bool,
    /// The frame `begin_frame` began: its display time, and the views it was
    /// rendered for once there was something to render.
    frame: Option<(xr::Time, Option<[xr::View; 2]>)>,
    last_time: Option<xr::Time>,
    last_eyes: Option<[EyeView; 2]>,
}

/// A runtime swapchain and whether this frame holds one of its images.
struct Swapchain {
    handle: xr::Swapchain<xr::Vulkan>,
    rect: xr::Rect2Di,
    acquired: bool,
}

impl Swapchain {
    fn new(handle: xr::Swapchain<xr::Vulkan>, width: u32, height: u32) -> Self {
        Self {
            handle,
            rect: xr::Rect2Di {
                offset: xr::Offset2Di { x: 0, y: 0 },
                extent: xr::Extent2Di {
                    width: width as i32,
                    height: height as i32,
                },
            },
            acquired: false,
        }
    }

    /// Takes the next image, once the compositor is done reading it.
    fn acquire(&mut self) -> Option<usize> {
        let index = self.handle.acquire_image().ok()?;
        self.acquired = true;
        self.handle.wait_image(xr::Duration::INFINITE).ok()?;
        Some(index as usize)
    }

    /// Hands the image back; whether there was one to.
    fn release(&mut self) -> bool {
        std::mem::take(&mut self.acquired) && self.handle.release_image().is_ok()
    }

    fn sub_image(&self) -> xr::SwapchainSubImage<'_, xr::Vulkan> {
        xr::SwapchainSubImage::new()
            .swapchain(&self.handle)
            .image_rect(self.rect)
    }
}

impl OpenXrBackend {
    pub fn new() -> Result<Self, String> {
        let entry = xr::Entry::linked(&()).map_err(|e| format!("OpenXR loader: {e}"))?;
        let available = entry
            .enumerate_extensions()
            .map_err(|e| format!("no OpenXR runtime: {e}"))?;
        if !available.khr_vulkan_enable {
            return Err("the OpenXR runtime has no Vulkan support".into());
        }
        let mut extensions = xr::ExtensionSet::default();
        extensions.khr_vulkan_enable = true;
        let instance = entry
            .create_instance(
                &xr::ApplicationInfo {
                    application_name: "Pomme",
                    engine_name: "Pomme",
                    api_version: xr::Version::new(1, 0, 0),
                    ..Default::default()
                },
                &extensions,
                &[],
                &(),
            )
            .map_err(|e| format!("OpenXR instance: {e}"))?;
        let system = instance
            .system(xr::FormFactor::HEAD_MOUNTED_DISPLAY)
            .map_err(|e| format!("no headset: {e}"))?;
        let blend_mode = instance
            .enumerate_environment_blend_modes(system, VIEW_TYPE)
            .map_err(|e| e.to_string())?
            .first()
            .copied()
            .ok_or("the headset offers no blend mode")?;
        if let Ok(props) = instance.properties() {
            tracing::info!(
                "OpenXR runtime: {} {}",
                props.runtime_name,
                props.runtime_version
            );
        }
        Ok(Self {
            instance,
            system,
            blend_mode,
            session: None,
            exit: false,
        })
    }

    fn poll_events(&mut self) {
        let mut buffer = xr::EventDataBuffer::new();
        while let Ok(Some(event)) = self.instance.poll_event(&mut buffer) {
            match event {
                xr::Event::SessionStateChanged(change) => {
                    let Some(session) = &mut self.session else {
                        continue;
                    };
                    match change.state() {
                        xr::SessionState::READY => {
                            session.running = session.session.begin(VIEW_TYPE).is_ok();
                        }
                        xr::SessionState::STOPPING => {
                            let _ = session.session.end();
                            session.running = false;
                        }
                        xr::SessionState::EXITING | xr::SessionState::LOSS_PENDING => {
                            self.exit = true;
                        }
                        _ => {}
                    }
                }
                xr::Event::InstanceLossPending(_) => self.exit = true,
                _ => {}
            }
        }
    }
}

/// The runtime's space-separated extension list.
fn extension_names(list: xr::Result<String>) -> Result<Vec<CString>, String> {
    let list = list.map_err(|e| e.to_string())?;
    list.split_ascii_whitespace()
        .map(|name| CString::new(name).map_err(|e| e.to_string()))
        .collect()
}

/// A `kind` space with its origin moved to `origin`.
fn reference_space(
    session: &xr::Session<xr::Vulkan>,
    kind: xr::ReferenceSpaceType,
    origin: xr::Posef,
) -> Result<xr::Space, String> {
    session
        .create_reference_space(kind, origin)
        .map_err(|e| e.to_string())
}

fn quat(pose: xr::Posef) -> Quat {
    let o = pose.orientation;
    Quat::from_xyzw(o.x, o.y, o.z, o.w)
}

fn vec3(pose: xr::Posef) -> Vec3 {
    let p = pose.position;
    Vec3::new(p.x, p.y, p.z)
}

fn posef(orientation: Quat, position: Vec3) -> xr::Posef {
    xr::Posef {
        orientation: xr::Quaternionf {
            x: orientation.x,
            y: orientation.y,
            z: orientation.z,
            w: orientation.w,
        },
        position: xr::Vector3f {
            x: position.x,
            y: position.y,
            z: position.z,
        },
    }
}

fn eye_view(view: &xr::View) -> EyeView {
    EyeView {
        orientation: quat(view.pose),
        position: vec3(view.pose),
        fov: Fov {
            left: view.fov.angle_left,
            right: view.fov.angle_right,
            up: view.fov.angle_up,
            down: view.fov.angle_down,
        },
    }
}

/// A `VK_MAKE_API_VERSION` value as OpenXR's version.
fn xr_version(vk: u32) -> xr::Version {
    xr::Version::new(
        (vk >> 22 & 0x7f) as u16,
        (vk >> 12 & 0x3ff) as u16,
        vk & 0xfff,
    )
}

impl StereoBackend for OpenXrBackend {
    fn instance_extensions(&self) -> Result<Vec<CString>, String> {
        extension_names(self.instance.vulkan_legacy_instance_extensions(self.system))
    }

    fn physical_device(&self, instance: RawHandle) -> Result<RawHandle, String> {
        // SAFETY: `instance` is the client's live VkInstance, created with
        // the extensions above.
        unsafe { self.instance.vulkan_graphics_device(self.system, instance) }
            .map_err(|e| e.to_string())
    }

    fn device_extensions(&self) -> Result<Vec<CString>, String> {
        extension_names(self.instance.vulkan_legacy_device_extensions(self.system))
    }

    fn start(
        &mut self,
        device: StereoDevice,
        formats: &[i64],
        (panel_width, panel_height): (u32, u32),
    ) -> Result<StereoImages, String> {
        let requirements = self
            .instance
            .graphics_requirements::<xr::Vulkan>(self.system)
            .map_err(|e| e.to_string())?;
        let version = xr_version(device.api_version);
        if version < requirements.min_api_version_supported {
            return Err(format!(
                "the runtime needs Vulkan {}",
                requirements.min_api_version_supported
            ));
        }
        // SAFETY: the handles are the client's live instance, device and
        // queue, made to the runtime's requirements on the GPU it named; the
        // client ends the session before destroying them.
        let (session, frame_waiter, frame_stream) = unsafe {
            self.instance.create_session::<xr::Vulkan>(
                self.system,
                &xr::vulkan::SessionCreateInfo {
                    instance: device.instance,
                    physical_device: device.physical_device,
                    device: device.device,
                    queue_family_index: device.queue_family,
                    queue_index: device.queue_index,
                },
            )
        }
        .map_err(|e| format!("OpenXR session: {e}"))?;

        let offered = session
            .enumerate_swapchain_formats()
            .map_err(|e| e.to_string())?;
        let format = formats
            .iter()
            .copied()
            .find(|&f| offered.iter().any(|&o| i64::from(o) == f))
            .ok_or("the headset offers no 8-bit sRGB format")?;

        let views = self
            .instance
            .enumerate_view_configuration_views(self.system, VIEW_TYPE)
            .map_err(|e| e.to_string())?;
        let view = views.first().ok_or("the headset has no views")?;
        let (width, height) = (
            view.recommended_image_rect_width,
            view.recommended_image_rect_height,
        );
        let swapchain = |width, height, array_size, usage| {
            session
                .create_swapchain(&xr::SwapchainCreateInfo {
                    create_flags: xr::SwapchainCreateFlags::EMPTY,
                    usage_flags: xr::SwapchainUsageFlags::COLOR_ATTACHMENT | usage,
                    format: format as u32,
                    sample_count: 1,
                    width,
                    height,
                    face_count: 1,
                    array_size,
                    mip_count: 1,
                })
                .map_err(|e| format!("OpenXR swapchain: {e}"))
        };
        // The eyes are copied from for the window's mirror; the panel is
        // copied into from the client's own panel image.
        let eyes = swapchain(width, height, 2, xr::SwapchainUsageFlags::TRANSFER_SRC)?;
        let panel = swapchain(
            panel_width,
            panel_height,
            1,
            xr::SwapchainUsageFlags::TRANSFER_DST,
        )?;
        let images = eyes.enumerate_images().map_err(|e| e.to_string())?;
        let panel_images = panel.enumerate_images().map_err(|e| e.to_string())?;
        let space = reference_space(&session, xr::ReferenceSpaceType::LOCAL, xr::Posef::IDENTITY)?;
        let head = reference_space(&session, xr::ReferenceSpaceType::VIEW, xr::Posef::IDENTITY)?;

        self.session = Some(Session {
            session,
            frame_waiter,
            frame_stream,
            eyes: Swapchain::new(eyes, width, height),
            panel: Swapchain::new(panel, panel_width, panel_height),
            space,
            origin: xr::Posef::IDENTITY,
            head,
            running: false,
            frame: None,
            last_time: None,
            last_eyes: None,
        });
        Ok(StereoImages {
            format,
            width,
            height,
            images,
            panel_images,
        })
    }

    fn begin_frame(&mut self) -> Option<StereoFrame> {
        self.poll_events();
        let session = self.session.as_mut()?;
        if !session.running {
            std::thread::sleep(IDLE_POLL);
            return None;
        }
        let state = session.frame_waiter.wait().ok()?;
        session.frame_stream.begin().ok()?;
        let time = state.predicted_display_time;
        session.last_time = Some(time);
        session.frame = Some((time, None));
        if !state.should_render {
            return None;
        }
        let (flags, views) = session
            .session
            .locate_views(VIEW_TYPE, time, &session.space)
            .ok()?;
        let views: [xr::View; 2] = views.try_into().ok()?;
        session.frame = Some((time, Some(views)));
        let image_index = session.eyes.acquire()?;
        let panel_index = session.panel.acquire()?;
        // Tracking lost: hold the last good pose rather than snap to origin.
        let eyes = views.map(|view| eye_view(&view));
        if flags.contains(xr::ViewStateFlags::ORIENTATION_VALID) {
            session.last_eyes = Some(eyes);
        }
        Some(StereoFrame {
            image_index,
            panel_index,
            eyes: session.last_eyes.unwrap_or(eyes),
        })
    }

    fn end_frame(&mut self, world: bool, panel: Option<PanelPlacement>) {
        let Some(session) = &mut self.session else {
            return;
        };
        let Some((display_time, views)) = session.frame.take() else {
            return;
        };
        let world = session.eyes.release() && world;
        let panel = panel.filter(|_| session.panel.release());

        let eyes = views.filter(|_| world).map(|views| {
            [0, 1].map(|layer| {
                let view = &views[layer as usize];
                xr::CompositionLayerProjectionView::new()
                    .pose(view.pose)
                    .fov(view.fov)
                    .sub_image(session.eyes.sub_image().image_array_index(layer))
            })
        });
        let projection = eyes.as_ref().map(|eyes| {
            xr::CompositionLayerProjection::new()
                .space(&session.space)
                .views(eyes)
        });
        // Source alpha lets the world show through the HUD; the 2D pass
        // blends unpremultiplied.
        let quad = panel.map(|panel| {
            xr::CompositionLayerQuad::new()
                .layer_flags(
                    xr::CompositionLayerFlags::BLEND_TEXTURE_SOURCE_ALPHA
                        | xr::CompositionLayerFlags::UNPREMULTIPLIED_ALPHA,
                )
                .space(&session.space)
                .eye_visibility(xr::EyeVisibility::BOTH)
                .sub_image(session.panel.sub_image())
                .pose(posef(panel.orientation, panel.position))
                .size(xr::Extent2Df {
                    width: panel.width,
                    height: panel.height,
                })
        });
        let mut layers: Vec<&xr::CompositionLayerBase<'_, xr::Vulkan>> = Vec::new();
        if let Some(projection) = &projection {
            layers.push(projection);
        }
        if let Some(quad) = &quad {
            layers.push(quad);
        }
        if let Err(e) = session
            .frame_stream
            .end(display_time, self.blend_mode, &layers)
        {
            tracing::warn!("OpenXR frame: {e}");
        }
    }

    fn recenter(&mut self) {
        let Some(session) = &mut self.session else {
            return;
        };
        let Some(time) = session.last_time else {
            return;
        };
        let Ok(head) = session.head.locate(&session.space, time) else {
            return;
        };
        let (rotation, position) = (quat(session.origin), vec3(session.origin));
        // Yaw only, so the seated space stays level.
        let forward = quat(head.pose) * Vec3::NEG_Z;
        let yaw = Quat::from_rotation_y((-forward.x).atan2(-forward.z));
        let origin = posef(rotation * yaw, position + rotation * vec3(head.pose));
        match reference_space(&session.session, xr::ReferenceSpaceType::LOCAL, origin) {
            Ok(space) => {
                session.space = space;
                session.origin = origin;
            }
            Err(e) => tracing::warn!("OpenXR recenter: {e}"),
        }
    }

    fn should_exit(&self) -> bool {
        self.exit
    }
}
