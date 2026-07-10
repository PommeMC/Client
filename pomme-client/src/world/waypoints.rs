//! Client-side locator bar waypoints: vanilla `ClientWaypointManager` plus the
//! `TrackedWaypoint` yaw/pitch/distance math and the `Mth`/`ARGB` helpers it
//! depends on, ported exactly (including vanilla's table-based `atan2`).

use std::collections::HashMap;
use std::sync::LazyLock;

use azalea_protocol::packets::game::c_waypoint as packet;
use glam::{DVec3, IVec3, Mat4};

#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub enum WaypointId {
    Uuid(uuid::Uuid),
    Name(String),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum WaypointStyleId {
    Default,
    Bowtie,
    Missing,
}

impl WaypointStyleId {
    /// `waypoint_style/<name>.json`: near/far distances and dot sprite count.
    /// The two vanilla styles are stable data, hardcoded instead of loading
    /// the JSON registry.
    fn near_far_count(self) -> (f32, f32, usize) {
        match self {
            WaypointStyleId::Default => (128.0, 332.0, 4),
            WaypointStyleId::Bowtie => (64.0, 332.0, 5),
            WaypointStyleId::Missing => (128.0, 332.0, 1),
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum WaypointData {
    Empty,
    Pos(IVec3),
    Chunk { x: i32, z: i32 },
    Azimuth { angle_rad: f32 },
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PitchDirection {
    None,
    Up,
    Down,
}

pub struct TrackedWaypoint {
    pub id: WaypointId,
    pub style: WaypointStyleId,
    pub color: Option<[u8; 3]>,
    pub data: WaypointData,
}

/// Camera state the waypoint math reads, mirroring vanilla's
/// `TrackedWaypoint.Camera` and `TrackedWaypoint.Projector`.
pub struct WaypointCamera {
    /// Render camera position (eye plus any third-person offset).
    pub position: DVec3,
    /// Effective camera yaw in degrees (mirrored view is turned around).
    pub yaw_deg: f32,
    /// Effective camera pitch in degrees, positive looking down.
    pub pitch_deg: f32,
    /// Rotation-only view times GL-convention projection (no view bob, no
    /// Vulkan Y flip), vanilla `Camera.getViewRotationProjectionMatrix`.
    pub view_rot_proj: Mat4,
    pub fov_y_deg: f32,
}

/// One dot extracted for the locator bar, in vanilla GUI units.
pub struct LocatorDot {
    /// Horizontal offset from the bar's screen middle.
    pub dot_position: i32,
    pub style: WaypointStyleId,
    pub sprite_index: usize,
    /// ARGB tint.
    pub color: u32,
    pub pitch: PitchDirection,
}

#[derive(Default)]
pub struct WaypointMap {
    waypoints: HashMap<WaypointId, TrackedWaypoint>,
}

impl WaypointMap {
    pub fn has_waypoints(&self) -> bool {
        !self.waypoints.is_empty()
    }

    pub fn apply(
        &mut self,
        operation: packet::WaypointOperation,
        waypoint: packet::TrackedWaypoint,
    ) {
        let wp = TrackedWaypoint::from_packet(waypoint);
        match operation {
            packet::WaypointOperation::Track => {
                self.waypoints.insert(wp.id.clone(), wp);
            }
            packet::WaypointOperation::Untrack => {
                self.waypoints.remove(&wp.id);
            }
            // Vanilla `TrackedWaypoint.update` copies only the position payload
            // and only within the same variant; the icon never changes.
            packet::WaypointOperation::Update => match self.waypoints.get_mut(&wp.id) {
                Some(existing)
                    if std::mem::discriminant(&existing.data)
                        == std::mem::discriminant(&wp.data) =>
                {
                    existing.data = wp.data;
                }
                Some(_) => tracing::warn!("Unsupported waypoint update for {:?}", wp.id),
                None => tracing::warn!("Waypoint update for untracked {:?}", wp.id),
            },
        }
    }

    /// Extract the dots to draw, farthest first so nearer dots render on top
    /// (vanilla `ClientWaypointManager.forEachWaypoint` + `LocatorBar`).
    pub fn extract_dots(
        &self,
        cam: &WaypointCamera,
        from: DVec3,
        local_uuid: uuid::Uuid,
        entity_eye_pos: &dyn Fn(&uuid::Uuid) -> Option<(IVec3, DVec3)>,
    ) -> Vec<LocatorDot> {
        let mut sorted: Vec<(&TrackedWaypoint, f64)> = self
            .waypoints
            .values()
            .map(|wp| (wp, wp.distance_squared(from)))
            .collect();
        sorted.sort_by(|a, b| b.1.total_cmp(&a.1));

        let mut dots = Vec::new();
        for (wp, dist_sq) in sorted {
            if wp.id == WaypointId::Uuid(local_uuid) {
                continue;
            }
            let angle = wp.yaw_angle_to_camera(cam, entity_eye_pos);
            // NaN (Empty data) passes and pins the dot at the middle.
            if angle <= -60.0 || angle > 60.0 {
                continue;
            }
            let (near, far, count) = wp.style.near_far_count();
            dots.push(LocatorDot {
                dot_position: mth_floor(angle * 173.0 / 2.0 / 60.0),
                style: wp.style,
                sprite_index: sprite_index(near, far, count, (dist_sq as f32).sqrt()),
                color: wp.dot_color(),
                pitch: wp.pitch_direction_to_camera(cam, entity_eye_pos),
            });
        }
        dots
    }
}

impl TrackedWaypoint {
    fn from_packet(wp: packet::TrackedWaypoint) -> Self {
        let id = match wp.identifier {
            packet::WaypointIdentifier::Uuid(uuid) => WaypointId::Uuid(uuid),
            packet::WaypointIdentifier::String(name) => WaypointId::Name(name),
        };
        let style = match (wp.icon.style.namespace(), wp.icon.style.path()) {
            ("minecraft", "default") => WaypointStyleId::Default,
            ("minecraft", "bowtie") => WaypointStyleId::Bowtie,
            _ => {
                tracing::warn!("Unknown waypoint style {}", wp.icon.style);
                WaypointStyleId::Missing
            }
        };
        let color = wp.icon.color.map(|c| [c.red(), c.green(), c.blue()]);
        let data = match wp.data {
            packet::WaypointData::Empty => WaypointData::Empty,
            packet::WaypointData::Vec3i(v) => WaypointData::Pos(IVec3::new(v.x, v.y, v.z)),
            packet::WaypointData::Chunk { x, z } => WaypointData::Chunk { x, z },
            packet::WaypointData::Azimuth { angle } => WaypointData::Azimuth { angle_rad: angle },
        };
        Self {
            id,
            style,
            color,
            data,
        }
    }

    /// Explicit icon color, else derived from the identifier's Java hash
    /// normalized to 0.9 brightness (vanilla `LocatorBar`).
    fn dot_color(&self) -> u32 {
        if let Some([r, g, b]) = self.color {
            return 0xFF00_0000 | (r as u32) << 16 | (g as u32) << 8 | b as u32;
        }
        let hash = match &self.id {
            WaypointId::Uuid(uuid) => java_uuid_hash(uuid),
            WaypointId::Name(name) => java_string_hash(name),
        };
        argb_set_brightness(0xFF00_0000 | (hash as u32 & 0xFF_FFFF), 0.9)
    }

    /// Vec3i waypoints track the live entity's eye when it's loaded and within
    /// Manhattan distance 3 of the transmitted position, else the block center.
    fn pos_waypoint_position(
        &self,
        v: IVec3,
        entity_eye_pos: &dyn Fn(&uuid::Uuid) -> Option<(IVec3, DVec3)>,
    ) -> DVec3 {
        if let WaypointId::Uuid(uuid) = &self.id
            && let Some((block_pos, eye_pos)) = entity_eye_pos(uuid)
            && {
                let d = (block_pos - v).abs();
                d.x + d.y + d.z <= 3
            }
        {
            return eye_pos;
        }
        v.as_dvec3() + 0.5
    }

    fn yaw_angle_to_camera(
        &self,
        cam: &WaypointCamera,
        entity_eye_pos: &dyn Fn(&uuid::Uuid) -> Option<(IVec3, DVec3)>,
    ) -> f64 {
        match self.data {
            WaypointData::Empty => f64::NAN,
            WaypointData::Pos(v) => {
                yaw_to_position(cam, self.pos_waypoint_position(v, entity_eye_pos))
            }
            // Chunk middle at the camera's Y, truncated like Java's (int) cast.
            WaypointData::Chunk { x, z } => {
                yaw_to_position(cam, chunk_middle(x, z, cam.position.y as i32))
            }
            WaypointData::Azimuth { angle_rad } => {
                wrap_degrees(angle_rad * 57.295_776 - cam.yaw_deg) as f64
            }
        }
    }

    fn pitch_direction_to_camera(
        &self,
        cam: &WaypointCamera,
        entity_eye_pos: &dyn Fn(&uuid::Uuid) -> Option<(IVec3, DVec3)>,
    ) -> PitchDirection {
        match self.data {
            WaypointData::Empty => PitchDirection::None,
            WaypointData::Pos(v) => {
                let p = project_point_to_screen(cam, self.pos_waypoint_position(v, entity_eye_pos));
                let behind = p.z as f64 > 1.0;
                let y = if behind { -(p.y as f64) } else { p.y as f64 };
                if y < -1.0 {
                    PitchDirection::Down
                } else if y > 1.0 {
                    PitchDirection::Up
                } else if behind {
                    if p.y > 0.0 {
                        PitchDirection::Up
                    } else if p.y < 0.0 {
                        PitchDirection::Down
                    } else {
                        PitchDirection::None
                    }
                } else {
                    PitchDirection::None
                }
            }
            WaypointData::Chunk { .. } | WaypointData::Azimuth { .. } => {
                let horizon = project_horizon_to_screen(cam);
                if horizon < -1.0 {
                    PitchDirection::Down
                } else if horizon > 1.0 {
                    PitchDirection::Up
                } else {
                    PitchDirection::None
                }
            }
        }
    }

    fn distance_squared(&self, from: DVec3) -> f64 {
        match self.data {
            WaypointData::Pos(v) => (v.as_dvec3() + 0.5 - from).length_squared(),
            WaypointData::Chunk { x, z } => {
                (chunk_middle(x, z, mth_floor(from.y)) - from).length_squared()
            }
            WaypointData::Empty | WaypointData::Azimuth { .. } => f64::INFINITY,
        }
    }
}

fn chunk_middle(chunk_x: i32, chunk_z: i32, y: i32) -> DVec3 {
    IVec3::new(chunk_x * 16 + 8, y, chunk_z * 16 + 8).as_dvec3() + 0.5
}

fn yaw_to_position(cam: &WaypointCamera, position: DVec3) -> f64 {
    // (camera - waypoint) rotated clockwise 90°: (x, y, z) -> (-z, y, x).
    let o = cam.position - position;
    let waypoint_angle = mth_atan2(o.x, -o.z) as f32 * 57.295_776;
    wrap_degrees(waypoint_angle - cam.yaw_deg) as f64
}

/// Vanilla `GameRenderer.projectPointToScreen`: NDC via the rotation-only
/// view-projection, with JOML's `transformProject` w division.
fn project_point_to_screen(cam: &WaypointCamera, point: DVec3) -> glam::Vec3 {
    let offset = (point - cam.position).as_vec3();
    let clip = cam.view_rot_proj * offset.extend(1.0);
    clip.truncate() / clip.w
}

/// Vanilla `GameRenderer.projectHorizonToScreen`.
fn project_horizon_to_screen(cam: &WaypointCamera) -> f64 {
    let x_rot = cam.pitch_deg;
    if x_rot <= -90.0 {
        return f64::NEG_INFINITY;
    }
    if x_rot >= 90.0 {
        return f64::INFINITY;
    }
    let deg_to_rad = std::f32::consts::PI / 180.0;
    f64::tan((x_rot * deg_to_rad) as f64) / f64::tan((cam.fov_y_deg / 2.0 * deg_to_rad) as f64)
}

/// Vanilla `WaypointStyle.sprite`, as an index into the style's sprite list.
fn sprite_index(near: f32, far: f32, count: usize, distance: f32) -> usize {
    if distance < near {
        return 0;
    }
    if distance >= far {
        return count - 1;
    }
    if count == 1 {
        return 0;
    }
    if count == 3 {
        return 1;
    }
    lerp_int((distance - near) / (far - near), 1, count as i32 - 1) as usize
}

fn lerp_int(alpha: f32, p0: i32, p1: i32) -> i32 {
    p0 + mth_floor((alpha * (p1 - p0) as f32) as f64)
}

/// Java `Mth.floor`; NaN maps to 0 like Java's `(int)` cast.
pub fn mth_floor(v: f64) -> i32 {
    let i = v as i32;
    if v < i as f64 { i - 1 } else { i }
}

/// Java `Mth.wrapDegrees(float)`.
pub fn wrap_degrees(angle: f32) -> f32 {
    let mut a = angle % 360.0;
    if a >= 180.0 {
        a -= 360.0;
    }
    if a < -180.0 {
        a += 360.0;
    }
    a
}

/// Java `UUID.hashCode()`.
pub fn java_uuid_hash(uuid: &uuid::Uuid) -> i32 {
    let (most, least) = uuid.as_u64_pair();
    let hilo = most ^ least;
    (hilo >> 32) as i32 ^ hilo as i32
}

/// Java `String.hashCode()`, over UTF-16 code units.
pub fn java_string_hash(s: &str) -> i32 {
    let mut h = 0i32;
    for unit in s.encode_utf16() {
        h = h.wrapping_mul(31).wrapping_add(unit as i32);
    }
    h
}

/// Vanilla `ARGB.setBrightness`: keep hue/saturation, rebuild at `brightness`.
pub fn argb_set_brightness(color: u32, brightness: f32) -> u32 {
    let alpha = color & 0xFF00_0000;
    let red = (color >> 16 & 0xFF) as i32;
    let green = (color >> 8 & 0xFF) as i32;
    let blue = (color & 0xFF) as i32;
    let rgb_max = red.max(green).max(blue);
    let rgb_min = red.min(green).min(blue);
    let range = (rgb_max - rgb_min) as f32;
    let saturation = if rgb_max != 0 {
        range / rgb_max as f32
    } else {
        0.0
    };
    if saturation == 0.0 {
        let c = (brightness * 255.0).round() as u32;
        return alpha | c << 16 | c << 8 | c;
    }
    let constant_red = (rgb_max - red) as f32 / range;
    let constant_green = (rgb_max - green) as f32 / range;
    let constant_blue = (rgb_max - blue) as f32 / range;
    let mut hue = if red == rgb_max {
        constant_blue - constant_green
    } else if green == rgb_max {
        2.0 + constant_red - constant_blue
    } else {
        4.0 + constant_green - constant_red
    };
    hue /= 6.0;
    if hue < 0.0 {
        hue += 1.0;
    }
    let segment = (hue - hue.floor()) * 6.0;
    let offset = segment - segment.floor();
    let primary = brightness * (1.0 - saturation);
    let secondary = brightness * (1.0 - saturation * offset);
    let tertiary = brightness * (1.0 - saturation * (1.0 - offset));
    let (r, g, b) = match segment as i32 {
        0 => (brightness, tertiary, primary),
        1 => (secondary, brightness, primary),
        2 => (primary, brightness, tertiary),
        3 => (primary, secondary, brightness),
        4 => (tertiary, primary, brightness),
        _ => (brightness, primary, secondary),
    };
    let channel = |v: f32| (v * 255.0).round() as u32;
    alpha | channel(r) << 16 | channel(g) << 8 | channel(b)
}

const FRAC_BIAS: f64 = f64::from_bits(4805340802404319232);

static ASIN_COS_TAB: LazyLock<([f64; 257], [f64; 257])> = LazyLock::new(|| {
    let mut asin_tab = [0.0; 257];
    let mut cos_tab = [0.0; 257];
    for (i, (asin, cos)) in asin_tab.iter_mut().zip(cos_tab.iter_mut()).enumerate() {
        let asin_v = (i as f64 / 256.0).asin();
        *cos = asin_v.cos();
        *asin = asin_v;
    }
    (asin_tab, cos_tab)
});

/// Vanilla `Mth.atan2`, the table-based approximation.
pub fn mth_atan2(y: f64, x: f64) -> f64 {
    let d2 = x * x + y * y;
    if d2.is_nan() {
        return f64::NAN;
    }
    let neg_y = y < 0.0;
    let mut y = if neg_y { -y } else { y };
    let neg_x = x < 0.0;
    let mut x = if neg_x { -x } else { x };
    let steep = y > x;
    if steep {
        std::mem::swap(&mut x, &mut y);
    }
    let rinv = fast_inv_sqrt(d2);
    x *= rinv;
    y *= rinv;
    let yp = FRAC_BIAS + y;
    let index = yp.to_bits() as u32 as usize;
    let (asin_tab, cos_tab) = &*ASIN_COS_TAB;
    let phi = asin_tab[index];
    let c_phi = cos_tab[index];
    let s_phi = yp - FRAC_BIAS;
    let sd = y * c_phi - x * s_phi;
    let d = (6.0 + sd * sd) * sd * 0.166_666_666_666_666_66;
    let mut theta = phi + d;
    if steep {
        theta = std::f64::consts::FRAC_PI_2 - theta;
    }
    if neg_x {
        theta = std::f64::consts::PI - theta;
    }
    if neg_y {
        theta = -theta;
    }
    theta
}

/// Java `Mth.fastInvSqrt` (one Newton iteration).
fn fast_inv_sqrt(x: f64) -> f64 {
    let xhalf = 0.5 * x;
    let i = 6910469410427058090i64 - (x.to_bits() as i64 >> 1);
    let y = f64::from_bits(i as u64);
    y * (1.5 - xhalf * y * y)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn java_hashes() {
        // Java: new UUID(1, 0).hashCode() == 1; halves XORing to -1 give 0.
        assert_eq!(java_uuid_hash(&uuid::Uuid::from_u64_pair(1, 0)), 1);
        assert_eq!(
            java_uuid_hash(&uuid::Uuid::from_u64_pair(
                0x0123_4567_89AB_CDEF,
                0xFEDC_BA98_7654_3210
            )),
            0
        );
        // Known Java values: "abc".hashCode(), "Notch".hashCode().
        assert_eq!(java_string_hash("abc"), 96354);
        assert_eq!(java_string_hash("Notch"), 75456088);
        assert_eq!(java_string_hash(""), 0);
    }

    #[test]
    fn set_brightness() {
        // Grayscale input: all channels become round(0.9 * 255) = 230.
        assert_eq!(argb_set_brightness(0xFF40_4040, 0.9), 0xFFE6_E6E6);
        // Pure red keeps hue 0 at full saturation.
        assert_eq!(argb_set_brightness(0xFFFF_0000, 0.9), 0xFFE6_0000);
        // Alpha is preserved.
        assert_eq!(argb_set_brightness(0x80FF_0000, 0.9) >> 24, 0x80);
    }

    #[test]
    fn atan2_matches_std() {
        for i in -20..=20 {
            for j in -20..=20 {
                if i == 0 && j == 0 {
                    continue;
                }
                let (y, x) = (i as f64 * 0.37, j as f64 * 0.61);
                let err = (mth_atan2(y, x) - y.atan2(x)).abs();
                // Vanilla's fastInvSqrt bounds the approximation around 1e-6.
                assert!(err < 1e-5, "atan2({y}, {x}) off by {err}");
            }
        }
        assert!(mth_atan2(f64::NAN, 1.0).is_nan());
    }

    #[test]
    fn floor_and_wrap() {
        assert_eq!(mth_floor(1.9), 1);
        assert_eq!(mth_floor(-1.1), -2);
        assert_eq!(mth_floor(f64::NAN), 0);
        assert_eq!(wrap_degrees(180.0), -180.0);
        assert_eq!(wrap_degrees(-180.0), -180.0);
        assert_eq!(wrap_degrees(540.0), -180.0);
        assert_eq!(wrap_degrees(90.0), 90.0);
    }

    #[test]
    fn sprite_distance_steps() {
        // Default style: near 128, far 332, 4 sprites.
        assert_eq!(sprite_index(128.0, 332.0, 4, 127.99), 0);
        assert_eq!(sprite_index(128.0, 332.0, 4, 128.0), 1);
        assert_eq!(sprite_index(128.0, 332.0, 4, 331.99), 2);
        assert_eq!(sprite_index(128.0, 332.0, 4, 332.0), 3);
        assert_eq!(sprite_index(128.0, 332.0, 4, f32::INFINITY), 3);
        // Bowtie: near 64, 5 sprites.
        assert_eq!(sprite_index(64.0, 332.0, 5, 63.99), 0);
        assert_eq!(sprite_index(64.0, 332.0, 5, 64.0), 1);
        // Single sprite always wins.
        assert_eq!(sprite_index(128.0, 332.0, 1, 200.0), 0);
    }

    fn packet_waypoint(uuid: uuid::Uuid, data: packet::WaypointData) -> packet::TrackedWaypoint {
        packet::TrackedWaypoint {
            identifier: packet::WaypointIdentifier::Uuid(uuid),
            icon: packet::WaypointIcon {
                style: azalea_registry::identifier::Identifier::new("minecraft:default"),
                color: None,
            },
            data,
        }
    }

    fn track(map: &mut WaypointMap, uuid: uuid::Uuid, data: packet::WaypointData) {
        map.apply(
            packet::WaypointOperation::Track,
            packet_waypoint(uuid, data),
        );
    }

    #[test]
    fn update_variant_mismatch_is_noop() {
        let mut map = WaypointMap::default();
        let uuid = uuid::Uuid::from_u64_pair(1, 2);
        track(&mut map, uuid, packet::WaypointData::Azimuth { angle: 1.0 });
        map.apply(
            packet::WaypointOperation::Update,
            packet_waypoint(uuid, packet::WaypointData::Chunk { x: 1, z: 1 }),
        );
        let wp = map.waypoints.values().next().unwrap();
        assert_eq!(wp.data, WaypointData::Azimuth { angle_rad: 1.0 });

        map.apply(
            packet::WaypointOperation::Untrack,
            packet_waypoint(uuid, packet::WaypointData::Empty),
        );
        assert!(!map.has_waypoints());
    }

    #[test]
    fn dots_sorted_farthest_first_and_self_skipped() {
        let mut map = WaypointMap::default();
        let local = uuid::Uuid::from_u64_pair(9, 9);
        let near = uuid::Uuid::from_u64_pair(1, 1);
        let far = uuid::Uuid::from_u64_pair(2, 2);
        // Yaw 0 faces +Z, so waypoints at +Z sit in the visible arc.
        track(
            &mut map,
            local,
            packet::WaypointData::Vec3i(azalea_core::position::Vec3i::new(0, 0, 5)),
        );
        track(
            &mut map,
            near,
            packet::WaypointData::Vec3i(azalea_core::position::Vec3i::new(0, 0, 10)),
        );
        track(&mut map, far, packet::WaypointData::Azimuth { angle: 0.0 });

        let cam = WaypointCamera {
            position: DVec3::new(0.5, 0.5, 0.5),
            yaw_deg: 0.0,
            pitch_deg: 0.0,
            view_rot_proj: Mat4::IDENTITY,
            fov_y_deg: 70.0,
        };
        let dots = map.extract_dots(&cam, cam.position, local, &|_| None);
        // Self is skipped; azimuth (infinite distance) sorts first (drawn under).
        assert_eq!(dots.len(), 2);
        assert_eq!(dots[0].sprite_index, 3); // infinity -> farthest default sprite
        assert_eq!(dots[1].sprite_index, 0); // ~10 blocks -> nearest sprite
    }
}
