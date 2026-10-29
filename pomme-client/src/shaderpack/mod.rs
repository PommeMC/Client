//! OptiFine/Iris-format shader packs: loading, option and directive parsing,
//! and translating their GLSL for Pomme's Vulkan renderer. Iris's behaviour
//! is the spec.

pub mod directives;
pub mod expr;
pub mod glsl;
pub mod include;
pub mod options;
pub mod prelude;
pub mod preprocess;
pub mod programs;
pub mod properties;
pub mod source;
pub mod uniforms;

use std::collections::BTreeMap;
use std::path::Path;

use glsl::compile::{self, CompileError, ShaderStage};
use glsl::translate::{self, ProgramSource, TranslateError, Translated};
use include::IncludeError;
use prelude::{Geometry, Preludes};
use preprocess::Overrides;
use programs::Gbuffers;
use source::{PackSource, SourceError};
use uniforms::CustomUniforms;

pub struct ShaderPack {
    pub name: String,
    source: PackSource,
    properties: BTreeMap<String, String>,
    dimensions: BTreeMap<String, String>,
    overrides: Overrides,
    /// The option and standard macros `shaders.properties` is read with.
    macros: BTreeMap<String, String>,
    pub custom_uniforms: CustomUniforms,
}

#[derive(Debug, thiserror::Error)]
pub enum PackError {
    #[error("{0}")]
    Source(#[from] SourceError),
    #[error("{program}: {error}")]
    Include {
        program: String,
        error: IncludeError,
    },
    #[error("{program}: {error}")]
    Translate {
        program: String,
        error: TranslateError,
    },
    #[error("{program}: {error}")]
    Compile {
        program: String,
        error: CompileError,
    },
}

/// A program's stages, ready to translate.
#[derive(Clone)]
pub struct ProgramText {
    /// The program's path below `shaders/`, without extension, e.g.
    /// `world0/gbuffers_terrain`.
    pub path: String,
    pub vertex: String,
    pub fragment: String,
}

/// A compiled program: SPIR-V and the interface the renderer binds.
pub struct CompiledProgram {
    pub path: String,
    pub vertex: Vec<u32>,
    pub fragment: Vec<u32>,
    pub interface: Translated,
    /// The colortex indices its outputs write, from the active
    /// `DRAWBUFFERS` / `RENDERTARGETS` directive.
    pub draw_buffers: Option<Vec<u8>>,
    /// Both stages' source, for the `const` render-target settings.
    pub text: String,
}

impl ShaderPack {
    /// Opens the pack at `path` (a folder or zip), with the player's option
    /// overrides from `shaderpacks/<pack>.txt` beside it.
    pub fn open(path: &Path) -> Result<Self, PackError> {
        let source = PackSource::open(path)?;
        let name = path
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_default();
        let overrides = std::fs::read_to_string(path.with_file_name(format!("{name}.txt")))
            .map(|text| preprocess::parse_overrides(&text))
            .unwrap_or_default();
        let mut macros = options::macros(&options::discover(&source), &overrides);
        macros.extend(standard_macros());
        let read_properties = |file| {
            source
                .read(file)
                .map(|text| properties::parse_ordered(&properties::preprocess(&text, &macros)))
                .unwrap_or_default()
        };
        let ordered = read_properties("shaders.properties");
        let custom_uniforms = CustomUniforms::parse(&ordered);
        let dimensions = read_properties("dimension.properties")
            .into_iter()
            .collect();
        Ok(Self {
            name,
            source,
            properties: ordered.into_iter().collect(),
            dimensions,
            overrides,
            macros,
            custom_uniforms,
        })
    }

    /// `name`'s program for `dimension` (e.g. `minecraft:overworld`): from
    /// the dimension's folder if the pack has one, as Iris uses it alone,
    /// else from the pack root; `None` if the pack lacks it or disables it.
    pub fn program(&self, dimension: &str, name: &str) -> Result<Option<ProgramText>, PackError> {
        let folder = properties::dimension_folder(&self.dimensions, dimension);
        let path = if self.source.has_folder(&folder) {
            format!("{folder}/{name}")
        } else {
            name.to_owned()
        };
        let exists = |ext| self.source.exists(&format!("{path}.{ext}"));
        if !exists("vsh") || !exists("fsh") || self.disabled(&path) {
            return Ok(None);
        }
        let expand = |file: String| {
            include::expand(&self.source, &file).map_err(|error| PackError::Include {
                program: path.clone(),
                error,
            })
        };
        let vertex = expand(format!("{path}.vsh"))?;
        let fragment = expand(format!("{path}.fsh"))?;
        Ok(Some(ProgramText {
            vertex: preprocess::prepare(&vertex, &self.overrides),
            fragment: preprocess::prepare(&fragment, &self.overrides),
            path,
        }))
    }

    /// `program` or the first program down its fallback chain the pack has.
    pub fn gbuffers(
        &self,
        dimension: &str,
        program: Gbuffers,
    ) -> Result<Option<ProgramText>, PackError> {
        let mut at = Some(program);
        while let Some(p) = at {
            if let Some(text) = self.program(dimension, p.name())? {
                return Ok(Some(text));
            }
            at = p.fallback();
        }
        Ok(None)
    }

    /// `program.<path>.enabled` in `shaders.properties` reading false: a
    /// boolean expression over the pack's options, such as `!SSAO`.
    fn disabled(&self, path: &str) -> bool {
        let name = path.rsplit('/').next().unwrap_or(path);
        [path, name].iter().any(|p| {
            self.properties
                .get(&format!("program.{p}.enabled"))
                .is_some_and(|v| !properties::condition(v, &self.macros))
        })
    }
}

/// Translates `program` for `geometry`'s vertex format.
fn translate(program: &ProgramText, geometry: Geometry) -> Result<Translated, PackError> {
    let preludes = Preludes::new(geometry);
    translate::translate(
        ProgramSource {
            vertex: &program.vertex,
            fragment: &program.fragment,
        },
        preludes.vertex(),
        preludes.fragment(),
        geometry.is_geometry(),
    )
    .map_err(|error| PackError::Translate {
        program: program.path.clone(),
        error,
    })
}

/// Translates and compiles `program` for `geometry`'s vertex format.
pub fn compile(program: &ProgramText, geometry: Geometry) -> Result<CompiledProgram, PackError> {
    let interface = translate(program, geometry)?;
    let compile_stage = |stage, source: &str| {
        compile::compile(stage, source).map_err(|error| PackError::Compile {
            program: program.path.clone(),
            error,
        })
    };
    Ok(CompiledProgram {
        path: program.path.clone(),
        vertex: compile_stage(ShaderStage::Vertex, &interface.vertex)?,
        fragment: compile_stage(ShaderStage::Fragment, &interface.fragment)?,
        draw_buffers: directives::draw_buffers(&interface.fragment),
        interface,
        text: {
            let macros = standard_macros();
            [&program.vertex, &program.fragment]
                .map(|stage| properties::preprocess(stage, &macros))
                .join("\n")
        },
    })
}

/// Iris's standard macros, by name.
fn standard_macros() -> BTreeMap<String, String> {
    preprocess::standard_macros().into_iter().collect()
}

/// What Iris runs as `final` when a pack has none: `colortex0` as it is.
pub fn copy_program() -> ProgramText {
    ProgramText {
        path: "final (built in)".to_owned(),
        vertex: "#version 120
varying vec2 texcoord;
void main() {
    gl_Position = ftransform();
    texcoord = gl_MultiTexCoord0.xy;
}
"
        .to_owned(),
        fragment: "#version 120
uniform sampler2D colortex0;
varying vec2 texcoord;
void main() {
    gl_FragColor = texture2D(colortex0, texcoord);
}
"
        .to_owned(),
    }
}

/// The geometry a `gbuffers_*` program draws in milestone 1.
pub fn gbuffers_geometry(program: Gbuffers) -> Geometry {
    match program {
        Gbuffers::Entities => Geometry::Entity,
        Gbuffers::Block => Geometry::BlockEntity,
        Gbuffers::Water => Geometry::Water,
        Gbuffers::SkyBasic | Gbuffers::SkyTextured => Geometry::Sky,
        _ => Geometry::Terrain,
    }
}

#[cfg(test)]
mod corpus {
    //! `just shaderpack-corpus <dir>`: compiles every milestone-1 program of
    //! every pack in a local folder and reports which fail.

    use programs::full_screen_names;

    use super::*;

    /// The programs milestone 1 draws with, and their geometry.
    fn milestone_programs(pack: &ShaderPack, dimension: &str) -> Vec<(ProgramText, Geometry)> {
        let mut out = Vec::new();
        for program in [
            Gbuffers::SkyBasic,
            Gbuffers::SkyTextured,
            Gbuffers::Terrain,
            Gbuffers::Water,
            Gbuffers::Entities,
            Gbuffers::Block,
        ] {
            match pack.gbuffers(dimension, program) {
                Ok(Some(text)) => out.push((text, gbuffers_geometry(program))),
                Ok(None) => {}
                Err(e) => println!("  LOAD {program:?}: {e}"),
            }
        }
        for prefix in ["deferred", "composite"] {
            for name in full_screen_names(prefix) {
                if let Ok(Some(text)) = pack.program(dimension, &name) {
                    out.push((text, Geometry::FullScreen));
                }
            }
        }
        if let Ok(Some(text)) = pack.program(dimension, "final") {
            out.push((text, Geometry::Final));
        }
        if let Ok(Some(text)) = pack.program(dimension, "shadow") {
            out.push((text.clone(), Geometry::Terrain));
            out.push((text, Geometry::Water));
        }
        out
    }

    #[test]
    #[ignore = "needs POMME_SHADERPACK_CORPUS"]
    fn compile_corpus() {
        let Ok(dir) = std::env::var("POMME_SHADERPACK_CORPUS") else {
            return;
        };
        let mut failed = 0;
        let mut total = 0;
        for entry in std::fs::read_dir(dir).expect("corpus folder").flatten() {
            let pack = match ShaderPack::open(&entry.path()) {
                Ok(pack) => pack,
                Err(e) => {
                    println!("{}: {e}", entry.path().display());
                    continue;
                }
            };
            println!("== {}", pack.name);
            let dimensions = [
                "minecraft:overworld",
                "minecraft:the_nether",
                "minecraft:the_end",
            ];
            for (text, geometry) in dimensions
                .into_iter()
                .flat_map(|dimension| milestone_programs(&pack, dimension))
            {
                total += 1;
                if let Ok(dump) = std::env::var("POMME_SHADERPACK_DUMP")
                    && let Ok(t) = translate(&text, geometry)
                {
                    let base = format!("{dump}/{}", text.path.replace('/', "_"));
                    std::fs::write(format!("{base}.vert"), &t.vertex).unwrap();
                    std::fs::write(format!("{base}.frag"), &t.fragment).unwrap();
                }
                match compile(&text, geometry) {
                    Ok(_) => println!("  ok   {}", text.path),
                    Err(e) => {
                        failed += 1;
                        let first = e
                            .to_string()
                            .lines()
                            .take(8)
                            .collect::<Vec<_>>()
                            .join("\n       ");
                        println!("  FAIL {first}");
                    }
                }
            }
        }
        println!("{} of {total} programs compiled", total - failed);
    }
}

#[cfg(test)]
mod tests {
    use std::collections::HashMap;

    use super::*;

    fn pack(files: &[&str]) -> ShaderPack {
        ShaderPack {
            name: "test".into(),
            source: PackSource::Zip(
                files
                    .iter()
                    .map(|f| {
                        (
                            (*f).to_owned(),
                            b"#version 120
void main() {}
"
                            .to_vec(),
                        )
                    })
                    .collect::<HashMap<_, _>>(),
            ),
            properties: BTreeMap::new(),
            dimensions: BTreeMap::new(),
            overrides: Overrides::new(),
            macros: BTreeMap::new(),
            custom_uniforms: CustomUniforms::default(),
        }
    }

    #[test]
    fn sky_programs_read_the_render_stage() {
        let program = ProgramText {
            path: "gbuffers_skybasic".to_owned(),
            vertex: "#version 120
void main() { gl_Position = ftransform(); }
"
            .to_owned(),
            fragment: "#version 120
uniform int renderStage;
void main() { gl_FragData[0] = vec4(renderStage == 4 ? 1.0 : 0.0); }
"
            .to_owned(),
        };
        let compiled = compile(&program, Geometry::Sky).unwrap();
        assert!(
            compiled
                .interface
                .uniforms
                .iter()
                .all(|u| u.name != "renderStage")
        );
        assert_eq!(standard_macros()["MC_RENDER_STAGE_SUN"], "4");
    }

    #[test]
    fn target_settings_follow_the_active_branch() {
        let mut program = copy_program();
        program.fragment = program.fragment.replace(
            "void main",
            "#define HQ
#ifdef HQ
/*
const int colortex0Format = RGBA16F;
*/
             #else
/*
const int colortex0Format = RGBA8;
*/
#endif
void main",
        );
        let compiled = compile(&program, Geometry::Final).unwrap();
        let mut settings = directives::TargetSettings::default();
        settings.read(&compiled.text);
        assert_eq!(settings.colortex.formats[&0], "RGBA16F");
    }

    #[test]
    fn a_dimension_folder_replaces_the_root() {
        let pack = pack(&[
            "composite.vsh",
            "composite.fsh",
            "final.vsh",
            "final.fsh",
            "world-1/final.vsh",
            "world-1/final.fsh",
        ]);
        let path = |dimension, name| pack.program(dimension, name).unwrap().map(|p| p.path);
        assert_eq!(
            path("minecraft:the_nether", "final").as_deref(),
            Some("world-1/final")
        );
        // The Nether's folder has no composite, and Iris doesn't fall back.
        assert_eq!(path("minecraft:the_nether", "composite"), None);
        assert_eq!(
            path("minecraft:overworld", "composite").as_deref(),
            Some("composite")
        );
    }
}
