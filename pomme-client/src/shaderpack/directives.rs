//! The comment and `const` directives packs steer the pipeline with, read
//! from the include-expanded source before preprocessing strips comments.

use std::collections::BTreeMap;
use std::sync::LazyLock;

use regex::Regex;

/// `/* DRAWBUFFERS:0127 */` (one hex digit per buffer) and
/// `/* RENDERTARGETS: 0,1,12 */`, turned into marker declarations so the
/// preprocessor keeps only the active branch's, as Iris reads them.
pub fn mark_draw_buffers(source: &str) -> String {
    static DIRECTIVE: LazyLock<Regex> = LazyLock::new(|| {
        Regex::new(r"/\*\s*(DRAWBUFFERS|RENDERTARGETS)\s*:\s*([0-9A-Fa-f,\s]*?)\s*\*/")
            .expect("static regex")
    });
    DIRECTIVE
        .replace_all(source, |caps: &regex::Captures| {
            let list: String = caps[2]
                .chars()
                .filter(|c| !c.is_whitespace())
                .map(|c| if c == ',' { '_' } else { c })
                .collect();
            format!("int pomme_{}_{list};", caps[1].to_ascii_lowercase())
        })
        .into_owned()
}

/// The colortex indices a program's outputs write, in output order, from
/// the last [`mark_draw_buffers`] marker left after preprocessing.
pub fn draw_buffers(source: &str) -> Option<Vec<u8>> {
    static MARKER: LazyLock<Regex> = LazyLock::new(|| {
        Regex::new(r"pomme_(drawbuffers|rendertargets)_([0-9A-Fa-f_]*)").expect("static regex")
    });
    let caps = MARKER.captures_iter(source).last()?;
    let list = &caps[2];
    if &caps[1] == "drawbuffers" {
        list.chars()
            .map(|c| c.to_digit(16).map(|d| d as u8))
            .collect()
    } else {
        list.split('_')
            .map(|n| n.parse().ok())
            .collect::<Option<Vec<u8>>>()
    }
}

/// Render target settings from `const` declarations across a pack's
/// programs.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct TargetSettings {
    pub colortex: BufferSettings,
    pub shadowcolor: BufferSettings,
    pub shadow: ShadowSettings,
}

/// One family of buffers' settings, keyed by index.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct BufferSettings {
    /// `<buffer>Format`, the GL internal-format name (e.g. `RGBA16F`).
    pub formats: BTreeMap<u8, String>,
    /// `<buffer>Clear = false` buffers keep last frame's contents.
    pub no_clear: Vec<u8>,
    /// `<buffer>ClearColor`.
    pub clear_colors: BTreeMap<u8, [f32; 4]>,
}

/// Iris's `PackShadowDirectives`, with its defaults.
#[derive(Clone, Debug, PartialEq)]
pub struct ShadowSettings {
    /// `shadowMapResolution`: the shadow maps' width and height.
    pub resolution: u32,
    /// `shadowDistance`: the orthographic projection's half width.
    pub distance: f32,
    /// `shadowIntervalSize`: the grid the shadow view snaps to.
    pub interval: f32,
    /// `sunPathRotation`: the sun and moon's tilt, in degrees.
    pub sun_path_rotation: f32,
    /// `shadowtexNNearest`.
    pub depth_nearest: [bool; 2],
    /// `shadowcolorNNearest`.
    pub color_nearest: [bool; 2],
}

impl Default for ShadowSettings {
    fn default() -> Self {
        Self {
            resolution: 1024,
            distance: 160.0,
            interval: 2.0,
            sun_path_rotation: 0.0,
            depth_nearest: [false; 2],
            color_nearest: [false; 2],
        }
    }
}

/// The legacy names of colortex0-7.
pub fn buffer_index(name: &str) -> Option<u8> {
    Some(match name {
        "gcolor" => 0,
        "gdepth" => 1,
        "gnormal" => 2,
        "composite" => 3,
        "gaux1" => 4,
        "gaux2" => 5,
        "gaux3" => 6,
        "gaux4" => 7,
        _ => return name.strip_prefix("colortex")?.parse().ok(),
    })
}

/// `shadowcolor0`-`7`, `shadowcolor` naming the first.
pub fn shadow_buffer_index(name: &str) -> Option<u8> {
    match name.strip_prefix("shadowcolor")? {
        "" => Some(0),
        n => n.parse().ok().filter(|&i| i < 8),
    }
}

impl TargetSettings {
    /// The settings of the buffer `name`, and its index.
    fn buffer(&mut self, name: &str) -> Option<(&mut BufferSettings, u8)> {
        if let Some(i) = buffer_index(name) {
            Some((&mut self.colortex, i))
        } else {
            shadow_buffer_index(name).map(|i| (&mut self.shadowcolor, i))
        }
    }

    pub fn read(&mut self, source: &str) {
        static FORMAT: LazyLock<Regex> = LazyLock::new(|| {
            Regex::new(r"const\s+int\s+(\w+)Format\s*=\s*(\w+)\s*;").expect("static regex")
        });
        static CLEAR: LazyLock<Regex> = LazyLock::new(|| {
            Regex::new(r"const\s+bool\s+(\w+)Clear\s*=\s*false\s*;").expect("static regex")
        });
        static CLEAR_COLOR: LazyLock<Regex> = LazyLock::new(|| {
            Regex::new(r"const\s+vec4\s+(\w+)ClearColor\s*=\s*vec4\s*\(([^)]*)\)\s*;")
                .expect("static regex")
        });
        static SCALAR: LazyLock<Regex> = LazyLock::new(|| {
            Regex::new(r"const\s+(?:int|float|bool)\s+(\w+)\s*=\s*([\w.+-]+)\s*;")
                .expect("static regex")
        });
        for caps in FORMAT.captures_iter(source) {
            if let Some((buffers, i)) = self.buffer(&caps[1]) {
                buffers.formats.insert(i, caps[2].to_owned());
            }
        }
        for caps in CLEAR.captures_iter(source) {
            if let Some((buffers, i)) = self.buffer(&caps[1])
                && !buffers.no_clear.contains(&i)
            {
                buffers.no_clear.push(i);
            }
        }
        for caps in SCALAR.captures_iter(source) {
            self.shadow.read(&caps[1], &caps[2]);
        }
        for caps in CLEAR_COLOR.captures_iter(source) {
            let Some((buffers, i)) = self.buffer(&caps[1]) else {
                continue;
            };
            let parts: Vec<f32> = caps[2]
                .split(',')
                .filter_map(|p| p.trim().trim_end_matches('f').parse().ok())
                .collect();
            let color = match parts[..] {
                [v] => [v; 4],
                [r, g, b, a] => [r, g, b, a],
                _ => continue,
            };
            buffers.clear_colors.insert(i, color);
        }
    }
}

impl ShadowSettings {
    /// Takes `const <type> name = value;` if it's a shadow setting.
    fn read(&mut self, name: &str, value: &str) {
        let float = || value.trim_end_matches('f').parse::<f32>().ok();
        let nearest = match name {
            "shadowMapResolution" => {
                self.resolution = float().map_or(self.resolution, |v| v as u32);
                return;
            }
            "shadowDistance" => {
                self.distance = float().unwrap_or(self.distance);
                return;
            }
            "shadowIntervalSize" => {
                self.interval = float().unwrap_or(self.interval);
                return;
            }
            "sunPathRotation" => {
                self.sun_path_rotation = float().unwrap_or(0.0);
                return;
            }
            "shadowtexNearest" | "shadowtex0Nearest" | "shadow0MinMagNearest" => {
                &mut self.depth_nearest[0]
            }
            "shadowtex1Nearest" | "shadow1MinMagNearest" => &mut self.depth_nearest[1],
            "shadowcolor0Nearest" | "shadowColor0Nearest" | "shadowColor0MinMagNearest" => {
                &mut self.color_nearest[0]
            }
            "shadowcolor1Nearest" | "shadowColor1Nearest" | "shadowColor1MinMagNearest" => {
                &mut self.color_nearest[1]
            }
            _ => return,
        };
        if let Ok(value) = value.parse() {
            *nearest = value;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn directive(source: &str) -> Option<Vec<u8>> {
        draw_buffers(&mark_draw_buffers(source))
    }

    #[test]
    fn draw_buffer_directives() {
        assert_eq!(directive("/* DRAWBUFFERS:024 */"), Some(vec![0, 2, 4]));
        assert_eq!(directive("/*DRAWBUFFERS:0A*/"), Some(vec![0, 10]));
        assert_eq!(
            directive("/* RENDERTARGETS: 0, 3,19 */"),
            Some(vec![0, 3, 19])
        );
        assert_eq!(
            directive("/* DRAWBUFFERS:0 */ x /* RENDERTARGETS: 1,2 */"),
            Some(vec![1, 2])
        );
        assert_eq!(directive("void main() {}"), None);
        assert_eq!(
            mark_draw_buffers("/* RENDERTARGETS: 0, 5 */"),
            "int pomme_rendertargets_0_5;"
        );
    }

    #[test]
    fn target_settings() {
        let mut settings = TargetSettings::default();
        settings.read(
            "/*\nconst int colortex0Format = RGBA16F;\nconst int gnormal Format = RGB8;\n\
             const int gaux1Format = R11F_G11F_B10F;\nconst bool colortex3Clear = false;\n\
             const vec4 colortex2ClearColor = vec4(1.0, 0.0, 0.0, 1.0);\n*/",
        );
        let colortex = &settings.colortex;
        assert_eq!(
            colortex.formats.get(&0).map(String::as_str),
            Some("RGBA16F")
        );
        assert_eq!(
            colortex.formats.get(&4).map(String::as_str),
            Some("R11F_G11F_B10F")
        );
        assert!(!colortex.formats.contains_key(&2));
        assert_eq!(colortex.no_clear, [3]);
        assert_eq!(colortex.clear_colors.get(&2), Some(&[1.0, 0.0, 0.0, 1.0]));
    }

    #[test]
    fn shadow_settings() {
        let mut settings = TargetSettings::default();
        assert_eq!(settings.shadow.resolution, 1024);
        settings.read(
            "const int shadowMapResolution = 2048;\nconst float shadowDistance = 192.0f;\n\
             const float sunPathRotation = -40.0;\nconst bool shadowHardwareFiltering = true;\n\
             const bool shadowtex1Nearest = true;\n\
             /*\nconst int shadowcolor0Format = RGBA16;\nconst bool shadowcolor1Clear = false;\n*/",
        );
        let shadow = &settings.shadow;
        assert_eq!(shadow.resolution, 2048);
        assert_eq!(shadow.distance, 192.0);
        assert_eq!(shadow.interval, 2.0);
        assert_eq!(shadow.sun_path_rotation, -40.0);
        assert_eq!(shadow.depth_nearest, [false, true]);
        assert_eq!(
            settings.shadowcolor.formats.get(&0).map(String::as_str),
            Some("RGBA16")
        );
        assert_eq!(settings.shadowcolor.no_clear, [1]);
        assert!(settings.colortex.formats.is_empty());
    }
}
