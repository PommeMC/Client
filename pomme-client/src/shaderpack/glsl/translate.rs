//! Rewrites a pack program's OpenGL-style GLSL into Vulkan GLSL 450 that naga
//! accepts, Iris's `TransformPatcher` in spirit: built-ins become prelude
//! globals, varyings get matching locations, loose uniforms gather into one
//! block, and combined samplers split into texture + sampler bindings.

use std::collections::{BTreeMap, BTreeSet};

use glsl_lang::ast::{
    self, ExprData, ExternalDeclarationData, FunIdentifierData, InterpolationQualifierData,
    LayoutQualifierSpecData, StorageQualifierData, TypeQualifierSpecData,
    TypeSpecifierNonArrayData,
};
use glsl_lang::parse::DefaultParse;
use glsl_lang::transpiler::glsl::{
    FormattingState, show_array_spec, show_translation_unit, show_type_specifier,
};
use glsl_lang::visitor::{HostMut, Visit, VisitorMut};

use super::compile::ShaderStage;

/// The pack's uniforms block, set 0 binding 0; the prelude's frame data is
/// binding 1.
const UNIFORM_SET: u32 = 0;
/// The pack's samplers: texture `2n`, sampler `2n + 1`.
pub const SAMPLER_SET: u32 = 1;
/// A geometry program's albedo texture (`gtexture`, `texture` or `tex`),
/// texture at 0 and sampler at 1: the renderer swaps it per draw.
pub const ALBEDO_SET: u32 = 3;
/// The albedo texture's names in a geometry program.
const ALBEDO_NAMES: &[&str] = &["gtexture", "texture", "tex"];

/// One stage's GLSL, already preprocessed.
pub struct ProgramSource<'a> {
    pub vertex: &'a str,
    pub fragment: &'a str,
}

/// What Pomme supplies around a pack stage: `decls` before the pack's code
/// (the `pomme_*` built-ins), `main` after it, calling `pack_main()`.
#[derive(Clone, Copy)]
pub struct Prelude<'a> {
    pub decls: &'a str,
    pub main: &'a str,
}

/// A loose uniform the pack declared, at its std140 offset in the block.
#[derive(Clone, Debug, PartialEq)]
pub struct Uniform {
    pub name: String,
    pub ty: UniformType,
    /// Array length, 1 when not an array.
    pub count: u32,
    pub offset: u32,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum UniformType {
    Float,
    Int,
    UInt,
    /// Stored as an `int`: SPIR-V has no bool in uniform blocks.
    Bool,
    Vec2,
    Vec3,
    Vec4,
    IVec2,
    IVec3,
    IVec4,
    Mat2,
    Mat3,
    Mat4,
}

impl UniformType {
    fn of(ty: &TypeSpecifierNonArrayData) -> Option<Self> {
        use TypeSpecifierNonArrayData as T;
        Some(match ty {
            T::Float => Self::Float,
            T::Int => Self::Int,
            T::UInt => Self::UInt,
            T::Bool => Self::Bool,
            T::Vec2 => Self::Vec2,
            T::Vec3 => Self::Vec3,
            T::Vec4 => Self::Vec4,
            T::IVec2 => Self::IVec2,
            T::IVec3 => Self::IVec3,
            T::IVec4 => Self::IVec4,
            T::Mat2 => Self::Mat2,
            T::Mat3 => Self::Mat3,
            T::Mat4 => Self::Mat4,
            _ => return None,
        })
    }

    fn glsl(self) -> &'static str {
        match self {
            Self::Float => "float",
            Self::Int | Self::Bool => "int",
            Self::UInt => "uint",
            Self::Vec2 => "vec2",
            Self::Vec3 => "vec3",
            Self::Vec4 => "vec4",
            Self::IVec2 => "ivec2",
            Self::IVec3 => "ivec3",
            Self::IVec4 => "ivec4",
            Self::Mat2 => "mat2",
            Self::Mat3 => "mat3",
            Self::Mat4 => "mat4",
        }
    }

    /// std140 (alignment, size) of one element outside an array.
    fn std140(self) -> (u32, u32) {
        match self {
            Self::Float | Self::Int | Self::UInt | Self::Bool => (4, 4),
            Self::Vec2 | Self::IVec2 => (8, 8),
            Self::Vec3 | Self::IVec3 => (16, 12),
            Self::Vec4 | Self::IVec4 => (16, 16),
            Self::Mat2 => (16, 32),
            Self::Mat3 => (16, 48),
            Self::Mat4 => (16, 64),
        }
    }
}

/// A combined sampler the pack declared, split into a texture at `binding`
/// and a sampler at `binding + 1` in [`SAMPLER_SET`].
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Sampler {
    pub name: String,
    pub kind: SamplerKind,
    pub binding: u32,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SamplerKind {
    Tex2D,
    Tex2DShadow,
    Tex2DArray,
    Tex3D,
    Cube,
    ITex2D,
    UTex2D,
}

impl SamplerKind {
    fn of(ty: &TypeSpecifierNonArrayData) -> Option<Self> {
        use TypeSpecifierNonArrayData as T;
        Some(match ty {
            T::Sampler2D => Self::Tex2D,
            T::Sampler2DShadow => Self::Tex2DShadow,
            T::Sampler2DArray => Self::Tex2DArray,
            T::Sampler3D => Self::Tex3D,
            T::SamplerCube => Self::Cube,
            T::ISampler2D => Self::ITex2D,
            T::USampler2D => Self::UTex2D,
            _ => return None,
        })
    }

    /// (texture type, sampler type, combined constructor).
    fn glsl(self) -> (&'static str, &'static str, &'static str) {
        match self {
            Self::Tex2D => ("texture2D", "sampler", "sampler2D"),
            Self::Tex2DShadow => ("texture2D", "samplerShadow", "sampler2DShadow"),
            Self::Tex2DArray => ("texture2DArray", "sampler", "sampler2DArray"),
            Self::Tex3D => ("texture3D", "sampler", "sampler3D"),
            Self::Cube => ("textureCube", "sampler", "samplerCube"),
            Self::ITex2D => ("itexture2D", "sampler", "isampler2D"),
            Self::UTex2D => ("utexture2D", "sampler", "usampler2D"),
        }
    }
}

/// The translated program and the interface the renderer binds.
#[derive(Debug)]
pub struct Translated {
    pub vertex: String,
    pub fragment: String,
    pub uniforms: Vec<Uniform>,
    /// The uniforms block's std140 size, at least 16.
    pub uniform_size: u32,
    pub samplers: Vec<Sampler>,
    /// Fragment outputs, as `location` indices `0..outputs`.
    pub outputs: u32,
}

#[derive(Debug, thiserror::Error)]
pub enum TranslateError {
    #[error("{stage:?} shader doesn't parse: {message}")]
    Parse { stage: ShaderStage, message: String },
    #[error("{0}")]
    Unsupported(String),
}

/// Words the GLSL 4.50 lexer reserves that older packs use as plain names;
/// renamed in the source before parsing. The interpolation qualifiers aren't
/// here, since newer packs use them as qualifiers.
const RESERVED: &[&str] = &[
    "sample",
    "patch",
    "subroutine",
    "input",
    "output",
    "common",
    "partition",
    "active",
    "filter",
    "resource",
];

/// Pack-visible built-ins and the prelude globals that stand in for them.
fn builtin(name: &str) -> Option<&'static str> {
    Some(match name {
        "gl_Vertex" => "pomme_Vertex",
        "gl_Normal" => "pomme_Normal",
        "gl_Color" => "pomme_Color",
        "gl_MultiTexCoord0" => "pomme_MultiTexCoord0",
        "gl_MultiTexCoord1" => "pomme_MultiTexCoord1",
        "gl_MultiTexCoord2" => "pomme_MultiTexCoord2",
        "gl_ModelViewMatrix" => "pomme_ModelViewMatrix",
        "gl_ModelViewMatrixInverse" => "pomme_ModelViewMatrixInverse",
        "gl_ProjectionMatrix" => "pomme_ProjectionMatrix",
        "gl_ProjectionMatrixInverse" => "pomme_ProjectionMatrixInverse",
        "gl_ModelViewProjectionMatrix" => "pomme_ModelViewProjectionMatrix",
        "gl_NormalMatrix" => "pomme_NormalMatrix",
        "gl_TextureMatrix" => "pomme_TextureMatrix",
        "gl_FogFragCoord" => "pomme_FogFragCoord",
        "gl_FrontColor" => "pomme_FrontColor",
        "gl_TexCoord" => "pomme_TexCoord",
        "gl_Fog" => "pomme_Fog",
        "gl_FragColor" => "pomme_FragData0",
        "renderStage" => "pomme_render_stage",
        "ftransform" => "pomme_ftransform",
        // naga's GLSL frontend lacks `modf`; the preludes polyfill it.
        "modf" => "pomme_modf",
        _ => return None,
    })
}

/// Pre-4.0 texture functions and their 4.50 names.
fn texture_function(name: &str) -> Option<&'static str> {
    Some(match name {
        "texture2D" | "texture3D" | "textureCube" | "texture2DArray" => "texture",
        "texture2DLod" | "texture3DLod" | "textureCubeLod" | "texture2DLodEXT" => "textureLod",
        "texture2DGrad" | "texture2DGradARB" | "texture2DGradEXT" => "textureGrad",
        "texture2DProj" => "textureProj",
        "texture2DProjLod" => "textureProjLod",
        "texelFetch2D" => "texelFetch",
        _ => return None,
    })
}

/// One stage's globals, lifted out of its AST.
#[derive(Default)]
struct Globals {
    /// Vertex inputs (`attribute`, or `in` in the vertex stage).
    attributes: Vec<Decl>,
    /// Varyings: vertex `out` / fragment `in`.
    varyings: Vec<Decl>,
    uniforms: Vec<Decl>,
    samplers: Vec<(String, SamplerKind)>,
    /// Fragment `out`s, with their explicit location if any.
    outputs: Vec<(Decl, Option<u32>)>,
}

#[derive(Clone)]
struct Decl {
    name: String,
    /// The GLSL type, e.g. `vec4`, without the array part.
    ty: String,
    /// The array part, e.g. `[4]`, or empty.
    array: String,
    flat: bool,
    is_int: bool,
    uniform: Option<UniformType>,
    count: u32,
}

/// `geometry` programs read their albedo texture from [`ALBEDO_SET`];
/// full-screen ones see `texture` and `tex` as plain samplers.
pub fn translate(
    source: ProgramSource,
    vertex_prelude: Prelude,
    fragment_prelude: Prelude,
    geometry: bool,
) -> Result<Translated, TranslateError> {
    let mut vertex = parse(ShaderStage::Vertex, source.vertex)?;
    let mut fragment = parse(ShaderStage::Fragment, source.fragment)?;
    let mut vertex_globals = lift_globals(ShaderStage::Vertex, &mut vertex)?;
    let mut fragment_globals = lift_globals(ShaderStage::Fragment, &mut fragment)?;
    for varying in builtin_varyings(source) {
        vertex_globals.varyings.push(varying.clone());
        fragment_globals.varyings.push(varying);
    }
    hoist_global_initializers(&mut vertex);
    hoist_global_initializers(&mut fragment);

    let (uniforms, uniform_size) = layout_uniforms(&vertex_globals, &fragment_globals);
    let (samplers, albedo) = sampler_bindings(&vertex_globals, &fragment_globals, geometry);
    let mut sampler_names: BTreeMap<String, (String, SamplerKind)> = samplers
        .iter()
        .map(|s| (s.name.clone(), (s.name.clone(), s.kind)))
        .collect();
    if albedo {
        for name in ALBEDO_NAMES {
            sampler_names.insert(
                (*name).to_owned(),
                ("gtexture".to_owned(), SamplerKind::Tex2D),
            );
        }
    }
    let bools: BTreeSet<String> = uniforms
        .iter()
        .filter(|u| u.ty == UniformType::Bool)
        .map(|u| u.name.clone())
        .collect();

    let vertex_outputs = rewrite(ShaderStage::Vertex, &mut vertex, &sampler_names, &bools);
    let fragment_outputs = rewrite(ShaderStage::Fragment, &mut fragment, &sampler_names, &bools);
    debug_assert!(vertex_outputs.is_empty());

    // Fragment-only varyings go after every slot the vertex ones take.
    let (locations, mut next) = varying_locations(&vertex_globals.varyings);
    let shared = shared_decls(&uniforms, uniform_size, &samplers, albedo);

    let mut vs = header(vertex_prelude.decls, &shared);
    for a in &vertex_globals.attributes {
        // Pack-specific attributes (mc_Entity, at_tangent, ...) aren't fed in
        // milestone 1: they read their defaults.
        let default = if a.name == "mc_Entity" { "-1.0" } else { "0.0" };
        vs.push_str(&format!(
            "{} {}{} = {}({default});\n",
            a.ty, a.name, a.array, a.ty
        ));
    }
    for v in &vertex_globals.varyings {
        vs.push_str(&interface_decl("out", locations[&v.name], v));
    }
    vs.push_str(&show(&vertex));
    vs.push_str(vertex_prelude.main);

    let mut fs = header(fragment_prelude.decls, &shared);
    for v in &fragment_globals.varyings {
        let location = locations.get(&v.name).copied().unwrap_or_else(|| {
            let l = next;
            next += slots(v);
            l
        });
        fs.push_str(&interface_decl("in", location, v));
    }
    let mut outputs = 0;
    for &index in &fragment_outputs {
        fs.push_str(&format!(
            "layout(location = {index}) out vec4 pomme_FragData{index};\n"
        ));
        outputs = outputs.max(index + 1);
    }
    for (position, (o, location)) in fragment_globals.outputs.iter().enumerate() {
        let location = location.unwrap_or(position as u32);
        fs.push_str(&format!(
            "layout(location = {location}) out {} {}{};\n",
            o.ty, o.name, o.array
        ));
        outputs = outputs.max(location + 1);
    }
    fs.push_str(&show(&fragment));
    fs.push_str(fragment_prelude.main);

    Ok(Translated {
        vertex: vs,
        fragment: fs,
        uniforms,
        uniform_size,
        samplers,
        outputs,
    })
}

fn parse(stage: ShaderStage, source: &str) -> Result<ast::TranslationUnit, TranslateError> {
    static RESERVED_WORDS: std::sync::LazyLock<regex::Regex> = std::sync::LazyLock::new(|| {
        regex::Regex::new(&format!(r"\b({})\b", RESERVED.join("|"))).expect("static regex")
    });
    let source = RESERVED_WORDS.replace_all(source, "pk_$1");
    ast::TranslationUnit::parse(&source).map_err(|e| TranslateError::Parse {
        stage,
        message: e.to_string(),
    })
}

fn show(tu: &ast::TranslationUnit) -> String {
    let mut out = String::new();
    show_translation_unit(&mut out, tu, FormattingState::default())
        .expect("writing to a String can't fail");
    out
}

fn header(prelude: &str, shared: &str) -> String {
    format!("#version 450\n{prelude}\n{shared}")
}

fn interface_decl(dir: &str, location: u32, v: &Decl) -> String {
    let flat = if v.flat || v.is_int { "flat " } else { "" };
    format!(
        "layout(location = {location}) {flat}{dir} {} {}{};\n",
        v.ty, v.name, v.array
    )
}

/// Locations a varying takes: a matrix column each, times its array length.
fn slots(v: &Decl) -> u32 {
    let per = match v.ty.as_str() {
        "mat2" => 2,
        "mat3" => 3,
        "mat4" => 4,
        _ => 1,
    };
    per * v.count
}

/// The vertex outputs' locations, by name, so the fragment inputs match,
/// and the first location after them.
fn varying_locations(varyings: &[Decl]) -> (BTreeMap<String, u32>, u32) {
    let mut sorted: Vec<&Decl> = varyings.iter().collect();
    sorted.sort_by(|a, b| a.name.cmp(&b.name));
    let mut next = 0;
    let locations = sorted
        .into_iter()
        .map(|v| {
            let location = next;
            next += slots(v);
            (v.name.clone(), location)
        })
        .collect();
    (locations, next)
}

/// Both stages' loose uniforms, by name, laid out std140.
fn layout_uniforms(vertex: &Globals, fragment: &Globals) -> (Vec<Uniform>, u32) {
    let mut by_name: BTreeMap<&str, &Decl> = BTreeMap::new();
    for d in vertex.uniforms.iter().chain(&fragment.uniforms) {
        by_name.entry(&d.name).or_insert(d);
    }
    let mut offset: u32 = 0;
    let uniforms = by_name
        .values()
        .map(|d| {
            let ty = d.uniform.expect("lifted uniforms have a type");
            let (align, size) = ty.std140();
            let (align, stride) = if d.count > 1 {
                (16, size.next_multiple_of(16))
            } else {
                (align, size)
            };
            offset = offset.next_multiple_of(align);
            let uniform = Uniform {
                name: d.name.clone(),
                ty,
                count: d.count,
                offset,
            };
            offset += stride * d.count;
            uniform
        })
        .collect();
    (uniforms, offset.next_multiple_of(16).max(16))
}

/// Both stages' samplers in [`SAMPLER_SET`], and whether a geometry program
/// uses the albedo texture.
fn sampler_bindings(vertex: &Globals, fragment: &Globals, geometry: bool) -> (Vec<Sampler>, bool) {
    let mut by_name: BTreeMap<&str, SamplerKind> = BTreeMap::new();
    let mut albedo = false;
    for (name, kind) in vertex.samplers.iter().chain(&fragment.samplers) {
        if geometry && ALBEDO_NAMES.contains(&name.as_str()) {
            albedo = true;
        } else {
            by_name.entry(name).or_insert(*kind);
        }
    }
    let samplers = by_name
        .into_iter()
        .enumerate()
        .map(|(i, (name, kind))| Sampler {
            name: name.to_owned(),
            kind,
            binding: 2 * i as u32,
        })
        .collect();
    (samplers, albedo)
}

/// The uniforms block and sampler bindings, identical in both stages.
fn shared_decls(uniforms: &[Uniform], size: u32, samplers: &[Sampler], albedo: bool) -> String {
    let mut out = String::new();
    if !uniforms.is_empty() {
        out.push_str(&format!(
            "layout(std140, set = {UNIFORM_SET}, binding = 0) uniform PackUniforms {{\n"
        ));
        for u in uniforms {
            let array = if u.count > 1 {
                format!("[{}]", u.count)
            } else {
                String::new()
            };
            let name = if u.ty == UniformType::Bool {
                format!("pomme_bool_{}", u.name)
            } else {
                u.name.clone()
            };
            // std140 places the members at the offsets computed above.
            out.push_str(&format!("    {} {name}{array};\n", u.ty.glsl()));
        }
        out.push_str("};\n");
        debug_assert!(size >= 16);
    }
    if albedo {
        out.push_str(&format!(
            "layout(set = {ALBEDO_SET}, binding = 0) uniform texture2D pt_gtexture;\n\
             layout(set = {ALBEDO_SET}, binding = 1) uniform sampler ps_gtexture;\n"
        ));
    }
    for s in samplers {
        let (texture, sampler, _) = s.kind.glsl();
        out.push_str(&format!(
            "layout(set = {SAMPLER_SET}, binding = {}) uniform {texture} pt_{};\n\
             layout(set = {SAMPLER_SET}, binding = {}) uniform {sampler} ps_{};\n",
            s.binding,
            s.name,
            s.binding + 1,
            s.name
        ));
    }
    out
}

fn storage(spec: &ast::TypeQualifierSpecData) -> Option<&StorageQualifierData> {
    match spec {
        TypeQualifierSpecData::Storage(s) => Some(&s.content),
        _ => None,
    }
}

/// Removes every global interface declaration from `tu` into [`Globals`],
/// along with preprocessor leftovers and precision statements.
fn lift_globals(
    stage: ShaderStage,
    tu: &mut ast::TranslationUnit,
) -> Result<Globals, TranslateError> {
    let mut globals = Globals::default();
    let mut kept = Vec::with_capacity(tu.0.len());
    for decl in std::mem::take(&mut tu.0) {
        match &decl.content {
            ExternalDeclarationData::Preprocessor(_) => continue,
            ExternalDeclarationData::Declaration(d) => match &d.content {
                ast::DeclarationData::Precision(..) => continue,
                ast::DeclarationData::InitDeclaratorList(list)
                    if lift_list(stage, list, &mut globals)? =>
                {
                    continue;
                }
                _ => {}
            },
            ExternalDeclarationData::FunctionDefinition(_) => {}
        }
        kept.push(decl);
    }
    tu.0 = kept;
    Ok(globals)
}

/// Lifts an interface declaration list; `false` leaves it in the program.
fn lift_list(
    stage: ShaderStage,
    list: &ast::InitDeclaratorList,
    globals: &mut Globals,
) -> Result<bool, TranslateError> {
    let head = &list.head;
    let Some(qualifier) = &head.ty.qualifier else {
        return Ok(false);
    };
    let mut flat = false;
    let mut location = None;
    let mut kind = None;
    for spec in &qualifier.qualifiers {
        match &spec.content {
            TypeQualifierSpecData::Interpolation(i) => {
                flat |= i.content == InterpolationQualifierData::Flat;
            }
            TypeQualifierSpecData::Layout(layout) => {
                for id in &layout.ids {
                    if let LayoutQualifierSpecData::Identifier(name, Some(value)) = &id.content
                        && name.as_str() == "location"
                        && let ExprData::IntConst(l) = value.content
                    {
                        location = Some(l as u32);
                    }
                }
            }
            _ => {}
        }
        if let Some(s) = storage(&spec.content) {
            kind = Some(s.clone());
        }
    }
    let Some(kind) = kind else {
        return Ok(false);
    };
    let ty = &head.ty.ty;
    let mut names: Vec<(String, Option<ast::ArraySpecifier>)> = Vec::new();
    if let Some(name) = &head.name {
        names.push((name.as_str().to_owned(), head.array_specifier.clone()));
    }
    for tail in &list.tail {
        names.push((
            tail.ident.ident.as_str().to_owned(),
            tail.ident.array_spec.clone(),
        ));
    }
    let mut type_text = String::new();
    let base = ast::TypeSpecifierData {
        ty: ty.ty.clone(),
        array_specifier: None,
    }
    .into();
    show_type_specifier(&mut type_text, &base, &mut FormattingState::default())
        .expect("writing to a String can't fail");
    let is_int = matches!(
        ty.ty.content,
        TypeSpecifierNonArrayData::Int
            | TypeSpecifierNonArrayData::UInt
            | TypeSpecifierNonArrayData::IVec2
            | TypeSpecifierNonArrayData::IVec3
            | TypeSpecifierNonArrayData::IVec4
            | TypeSpecifierNonArrayData::UVec2
            | TypeSpecifierNonArrayData::UVec3
            | TypeSpecifierNonArrayData::UVec4
    );
    let decl = |name: String, array: Option<ast::ArraySpecifier>| -> Decl {
        let array_spec = array.or_else(|| ty.array_specifier.clone());
        let (array, count) = match &array_spec {
            Some(spec) => {
                let mut text = String::new();
                show_array_spec(&mut text, spec, &mut FormattingState::default())
                    .expect("writing to a String can't fail");
                let count = array_len(spec).unwrap_or(1);
                (text, count)
            }
            None => (String::new(), 1),
        };
        Decl {
            name,
            ty: type_text.clone(),
            array,
            flat,
            is_int,
            uniform: UniformType::of(&ty.ty.content),
            count,
        }
    };
    match (kind, stage) {
        (StorageQualifierData::Attribute, _) | (StorageQualifierData::In, ShaderStage::Vertex) => {
            for (n, a) in names {
                globals.attributes.push(decl(n, a));
            }
        }
        (StorageQualifierData::Varying, _)
        | (StorageQualifierData::Out, ShaderStage::Vertex)
        | (StorageQualifierData::In, ShaderStage::Fragment) => {
            for (n, a) in names {
                globals.varyings.push(decl(n, a));
            }
        }
        (StorageQualifierData::Out, ShaderStage::Fragment) => {
            for (n, a) in names {
                globals.outputs.push((decl(n, a), location));
            }
        }
        (StorageQualifierData::Uniform, _) => {
            if let Some(kind) = SamplerKind::of(&ty.ty.content) {
                for (n, _) in names {
                    globals.samplers.push((n, kind));
                }
            } else if UniformType::of(&ty.ty.content).is_some() {
                // The prelude sets `renderStage` per draw.
                for (n, a) in names.into_iter().filter(|(n, _)| n != "renderStage") {
                    globals.uniforms.push(decl(n, a));
                }
            } else {
                return Err(TranslateError::Unsupported(format!(
                    "uniform of type {type_text}"
                )));
            }
        }
        _ => return Ok(false),
    }
    Ok(true)
}

/// Moves non-`const` global initializers into assignments at the start of
/// `main`. Desktop GLSL runs them at startup, and they often read uniforms;
/// naga doesn't take them. An unsized array is sized from its constructor.
fn hoist_global_initializers(tu: &mut ast::TranslationUnit) {
    let mut assignments = Vec::new();
    for decl in &mut tu.0 {
        let ExternalDeclarationData::Declaration(d) = &mut decl.content else {
            continue;
        };
        let ast::DeclarationData::InitDeclaratorList(list) = &mut d.content else {
            continue;
        };
        let is_const = list.head.ty.qualifier.as_ref().is_some_and(|q| {
            q.qualifiers
                .iter()
                .any(|s| storage(&s.content) == Some(&StorageQualifierData::Const))
        });
        if is_const {
            continue;
        }
        let head = &mut list.head.content;
        if let Some(name) = &head.name {
            let name = name.as_str().to_owned();
            if let Some(init) = take_simple_initializer(&mut head.initializer) {
                size_unsized(&mut head.ty.ty.array_specifier, &init);
                size_unsized(&mut head.array_specifier, &init);
                assignments.push((name, init));
            }
        }
        for tail in &mut list.tail {
            let name = tail.ident.ident.as_str().to_owned();
            if let Some(init) = take_simple_initializer(&mut tail.initializer) {
                size_unsized(&mut tail.ident.array_spec, &init);
                assignments.push((name, init));
            }
        }
    }
    if assignments.is_empty() {
        return;
    }
    let statements = assignments.into_iter().map(|(name, init)| {
        let assign = ExprData::Assignment(
            Box::new(variable(&name)),
            ast::AssignmentOpData::Equal.into(),
            Box::new(init),
        );
        ast::StatementData::Expression(ast::ExprStatementData(Some(assign.into())).into()).into()
    });
    for decl in &mut tu.0 {
        if let ExternalDeclarationData::FunctionDefinition(f) = &mut decl.content
            && f.prototype.name.as_str() == "main"
        {
            let body = &mut f.statement.statement_list;
            body.splice(0..0, statements);
            return;
        }
    }
}

fn take_simple_initializer(initializer: &mut Option<ast::Initializer>) -> Option<ast::Expr> {
    match initializer.take()?.content {
        ast::InitializerData::Simple(expr) => Some(*expr),
        list => {
            *initializer = Some(list.into());
            None
        }
    }
}

/// `[]` sized from an array constructor's argument count.
fn size_unsized(spec: &mut Option<ast::ArraySpecifier>, init: &ast::Expr) {
    let Some(spec) = spec else {
        return;
    };
    let ExprData::FunCall(_, args) = &init.content else {
        return;
    };
    for dim in &mut spec.dimensions {
        if dim.content == ast::ArraySpecifierDimensionData::Unsized {
            *dim = ast::ArraySpecifierDimensionData::ExplicitlySized(Box::new(
                ExprData::IntConst(args.len() as i32).into(),
            ))
            .into();
        }
    }
}

fn array_len(spec: &ast::ArraySpecifier) -> Option<u32> {
    match &spec.dimensions.first()?.content {
        ast::ArraySpecifierDimensionData::ExplicitlySized(e) => match e.content {
            ExprData::IntConst(n) => Some(n as u32),
            ExprData::UIntConst(n) => Some(n),
            _ => None,
        },
        ast::ArraySpecifierDimensionData::Unsized => None,
    }
}

/// The built-in varyings OpenGL passes between the stages that `source`
/// uses: the vertex `gl_FrontColor` the fragment reads as `gl_Color`,
/// `gl_TexCoord[]` and `gl_FogFragCoord`.
fn builtin_varyings(source: ProgramSource) -> Vec<Decl> {
    static TEX_COORD: std::sync::LazyLock<regex::Regex> = std::sync::LazyLock::new(|| {
        regex::Regex::new(r"\bgl_TexCoord\s*\[\s*(\d+)\s*\]").expect("static regex")
    });
    let varying = |name: &str, ty: &str, count: u32| Decl {
        name: name.to_owned(),
        ty: ty.to_owned(),
        array: if count > 1 {
            format!("[{count}]")
        } else {
            String::new()
        },
        flat: false,
        is_int: false,
        uniform: None,
        count,
    };
    let mut varyings = Vec::new();
    if source.vertex.contains("gl_FrontColor") || source.fragment.contains("gl_Color") {
        varyings.push(varying("pomme_FrontColor", "vec4", 1));
    }
    let tex_coords = [source.vertex, source.fragment]
        .iter()
        .flat_map(|s| TEX_COORD.captures_iter(s))
        .filter_map(|c| c[1].parse::<u32>().ok())
        .max();
    if let Some(last) = tex_coords {
        // One past the highest index used, and at least two so it stays an
        // array.
        varyings.push(varying("pomme_TexCoord", "vec4", (last + 1).max(2)));
    }
    if source.vertex.contains("gl_FogFragCoord") || source.fragment.contains("gl_FogFragCoord") {
        varyings.push(varying("pomme_FogFragCoord", "float", 1));
    }
    varyings
}

/// Rewrites built-ins, texture calls, sampler and bool-uniform uses and
/// `main`, returning the `gl_FragData` indices written.
fn rewrite(
    stage: ShaderStage,
    tu: &mut ast::TranslationUnit,
    samplers: &BTreeMap<String, (String, SamplerKind)>,
    bools: &BTreeSet<String>,
) -> BTreeSet<u32> {
    let mut rewriter = Rewriter {
        stage,
        samplers,
        bools,
        frag_data: BTreeSet::new(),
        sampler_functions: sampler_parameter_positions(tu),
        params: BTreeMap::new(),
        shadowed: BTreeSet::new(),
    };
    tu.visit_mut(&mut rewriter);
    rewriter.frag_data
}

struct Rewriter<'a> {
    stage: ShaderStage,
    /// Each sampler name's binding name and kind.
    samplers: &'a BTreeMap<String, (String, SamplerKind)>,
    bools: &'a BTreeSet<String>,
    frag_data: BTreeSet<u32>,
    /// Functions taking samplers, and those parameters' positions.
    sampler_functions: BTreeMap<String, Vec<usize>>,
    /// The current function's sampler parameters.
    params: BTreeMap<String, SamplerKind>,
    /// Names the current function declares, which hide globals of that name.
    shadowed: BTreeSet<String>,
}

fn variable(name: &str) -> ast::Expr {
    ExprData::variable(name).into()
}

fn call(name: &str, args: Vec<ast::Expr>) -> ast::Expr {
    ExprData::FunCall(FunIdentifierData::ident(name).into(), args).into()
}

/// The combined sampler `name` stands for, as its texture and sampler.
fn combined(name: &str, kind: SamplerKind) -> ast::Expr {
    let (_, _, constructor) = kind.glsl();
    call(
        constructor,
        vec![
            variable(&format!("pt_{name}")),
            variable(&format!("ps_{name}")),
        ],
    )
}

fn sampler_param(param: &ast::FunctionParameterDeclaration) -> Option<(String, SamplerKind)> {
    let ast::FunctionParameterDeclarationData::Named(_, declarator) = &param.content else {
        return None;
    };
    Some((
        declarator.ident.ident.as_str().to_owned(),
        sampler_kind(param)?,
    ))
}

fn sampler_kind(param: &ast::FunctionParameterDeclaration) -> Option<SamplerKind> {
    let ty = match &param.content {
        ast::FunctionParameterDeclarationData::Named(_, declarator) => &declarator.ty,
        ast::FunctionParameterDeclarationData::Unnamed(_, ty) => ty,
    };
    SamplerKind::of(&ty.ty.content)
}

/// Functions with sampler parameters, which naga can't take: they get a
/// texture and a sampler for each instead.
fn sampler_parameter_positions(tu: &ast::TranslationUnit) -> BTreeMap<String, Vec<usize>> {
    let mut functions = BTreeMap::new();
    for decl in &tu.0 {
        let prototype = match &decl.content {
            ExternalDeclarationData::FunctionDefinition(f) => &f.prototype,
            ExternalDeclarationData::Declaration(d) => match &d.content {
                ast::DeclarationData::FunctionPrototype(p) => p,
                _ => continue,
            },
            ExternalDeclarationData::Preprocessor(_) => continue,
        };
        let positions: Vec<usize> = prototype
            .parameters
            .iter()
            .enumerate()
            .filter(|(_, p)| sampler_kind(p).is_some())
            .map(|(i, _)| i)
            .collect();
        if !positions.is_empty() {
            functions.insert(prototype.name.as_str().to_owned(), positions);
        }
    }
    functions
}

/// A parameter of the type named `ty` (e.g. `texture2D`).
fn typed_param(ty: &str, name: &str) -> ast::FunctionParameterDeclaration {
    let ty = ast::TypeSpecifierData::from(ast::TypeSpecifierNonArrayData::TypeName(
        ast::TypeNameData::from(ty).into(),
    ));
    named_param(None, ty.into(), name)
}

fn named_param(
    qualifier: Option<ast::TypeQualifier>,
    ty: ast::TypeSpecifier,
    name: &str,
) -> ast::FunctionParameterDeclaration {
    ast::FunctionParameterDeclarationData::Named(
        qualifier,
        ast::FunctionParameterDeclaratorData {
            ty,
            ident: ast::ArrayedIdentifierData {
                ident: ast::IdentifierData::from(name).into(),
                array_spec: None,
            }
            .into(),
        }
        .into(),
    )
    .into()
}

/// Splits `prototype`'s sampler parameters into a texture and a sampler
/// each, returning them by name. Unnamed parameters, which a forward
/// declaration may have and naga can't take, are named by position.
fn split_sampler_params(prototype: &mut ast::FunctionPrototype) -> BTreeMap<String, SamplerKind> {
    let mut split = BTreeMap::new();
    let mut params = Vec::with_capacity(prototype.parameters.len());
    for (i, mut param) in std::mem::take(&mut prototype.parameters)
        .into_iter()
        .enumerate()
    {
        if let ast::FunctionParameterDeclarationData::Unnamed(qualifier, ty) = &param.content {
            param = named_param(qualifier.clone(), ty.clone(), &format!("pomme_param{i}"));
        }
        match sampler_param(&param) {
            Some((name, kind)) => {
                let (texture, sampler, _) = kind.glsl();
                params.push(typed_param(texture, &format!("pt_{name}")));
                params.push(typed_param(sampler, &format!("ps_{name}")));
                split.insert(name, kind);
            }
            None => params.push(param),
        }
    }
    prototype.parameters = params;
    split
}

/// Every name a function declares: its parameters and its locals.
struct Declared(BTreeSet<String>);

impl glsl_lang::visitor::Visitor for Declared {
    fn visit_single_declaration(&mut self, d: &ast::SingleDeclaration) -> Visit {
        if let Some(name) = &d.name {
            self.0.insert(name.as_str().to_owned());
        }
        Visit::Children
    }

    fn visit_single_declaration_no_type(&mut self, d: &ast::SingleDeclarationNoType) -> Visit {
        self.0.insert(d.ident.ident.as_str().to_owned());
        Visit::Children
    }

    fn visit_function_parameter_declarator(
        &mut self,
        d: &ast::FunctionParameterDeclarator,
    ) -> Visit {
        self.0.insert(d.ident.ident.as_str().to_owned());
        Visit::Children
    }
}

impl VisitorMut for Rewriter<'_> {
    fn visit_function_definition(&mut self, f: &mut ast::FunctionDefinition) -> Visit {
        use glsl_lang::visitor::Host;
        let mut declared = Declared(BTreeSet::new());
        f.visit(&mut declared);
        self.shadowed = declared.0;
        let prototype = &mut f.prototype;
        if prototype.name.as_str() == "main" {
            prototype.name = ast::IdentifierData::from("pack_main").into();
        }
        self.params = split_sampler_params(prototype);
        f.statement.visit_mut(self);
        self.shadowed.clear();
        self.params.clear();
        Visit::Parent
    }

    /// A forward declaration, whose sampler parameters split as its
    /// definition's do.
    fn visit_function_prototype(&mut self, prototype: &mut ast::FunctionPrototype) -> Visit {
        split_sampler_params(prototype);
        Visit::Parent
    }

    fn visit_expr(&mut self, expr: &mut ast::Expr) -> Visit {
        if let ExprData::FunCall(callee, args) = &mut expr.content {
            // A call's arguments are rewritten; its name only renamed, so a
            // sampler named like a function (`texture`) stays a call.
            for arg in args.iter_mut() {
                arg.visit_mut(self);
            }
            let name = match &mut callee.content {
                FunIdentifierData::Expr(e) => match &e.content {
                    ExprData::Variable(name) => Some(name.as_str().to_owned()),
                    _ => {
                        e.visit_mut(self);
                        None
                    }
                },
                FunIdentifierData::TypeSpecifier(_) => None,
            };
            let Some(name) = name else {
                return Visit::Parent;
            };
            if let Some(positions) = self.sampler_functions.get(&name) {
                // Each sampler argument is passed as its texture and sampler.
                for &position in positions.iter().rev() {
                    if position < args.len()
                        && let ExprData::FunCall(_, parts) = &mut args[position].content
                        && parts.len() == 2
                    {
                        let parts = std::mem::take(parts);
                        args.splice(position..=position, parts);
                    }
                }
            } else if name.starts_with("shadow2D") {
                // `shadow2D` returned a vec4; `texture` on a shadow sampler
                // returns the comparison.
                let lookup = if name.contains("Lod") {
                    "textureLod"
                } else {
                    "texture"
                };
                *expr = call("vec4", vec![call(lookup, std::mem::take(args))]);
            } else if let Some(renamed) = texture_function(&name).or_else(|| builtin(&name)) {
                *callee = FunIdentifierData::ident(renamed).into();
            }
            return Visit::Parent;
        }
        let replacement = match &expr.content {
            ExprData::Variable(name) => {
                let name = name.as_str();
                if let Some(kind) = self.params.get(name) {
                    Some(combined(name, *kind))
                } else if self.shadowed.contains(name) {
                    None
                } else if let Some((bound, kind)) = self.samplers.get(name) {
                    Some(combined(bound, *kind))
                } else if self.bools.contains(name) {
                    Some(
                        ExprData::Binary(
                            ast::BinaryOpData::NonEqual.into(),
                            Box::new(variable(&format!("pomme_bool_{name}"))),
                            Box::new(ExprData::IntConst(0).into()),
                        )
                        .into(),
                    )
                } else if name == "gl_Color" && self.stage == ShaderStage::Fragment {
                    // The fragment stage's `gl_Color` is the interpolated
                    // `gl_FrontColor`.
                    Some(variable("pomme_FrontColor"))
                } else if name == "gl_FragColor" {
                    self.frag_data.insert(0);
                    Some(variable("pomme_FragData0"))
                } else {
                    builtin(name).map(variable)
                }
            }
            ExprData::Bracket(array, index) => match (&array.content, &index.content) {
                (ExprData::Variable(name), ExprData::IntConst(i))
                    if name.as_str() == "gl_FragData" =>
                {
                    self.frag_data.insert(*i as u32);
                    Some(variable(&format!("pomme_FragData{i}")))
                }
                _ => None,
            },
            _ => None,
        };
        match replacement {
            Some(replacement) => {
                *expr = replacement;
                Visit::Parent
            }
            None => Visit::Children,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::shaderpack::glsl::compile;

    const VS_PRELUDE: Prelude = Prelude {
        decls: "vec4 pomme_Vertex; vec4 pomme_Color; vec4 pomme_MultiTexCoord0; vec3 pomme_Normal;\n\
                mat4 pomme_ModelViewMatrix; mat4 pomme_ProjectionMatrix; mat4 pomme_TextureMatrix[2];\n\
                mat3 pomme_NormalMatrix;\n\
                vec4 pomme_ftransform() { return pomme_ProjectionMatrix * pomme_ModelViewMatrix * pomme_Vertex; }\n",
        main: "void main() { pack_main(); }\n",
    };
    const FS_PRELUDE: Prelude = Prelude {
        decls: "",
        main: "void main() { pack_main(); }\n",
    };

    fn translated(vs: &str, fs: &str) -> Translated {
        let t = translate(
            ProgramSource {
                vertex: vs,
                fragment: fs,
            },
            VS_PRELUDE,
            FS_PRELUDE,
            true,
        )
        .unwrap();
        for (stage, src) in [
            (ShaderStage::Vertex, &t.vertex),
            (ShaderStage::Fragment, &t.fragment),
        ] {
            if let Err(e) = compile::compile(stage, src) {
                panic!("{e}\n{src}");
            }
        }
        t
    }

    #[test]
    fn old_style_program_compiles() {
        let t = translated(
            "#version 120\n\
             varying vec2 texcoord;\nvarying vec4 glcolor;\nattribute vec4 mc_Entity;\n\
             uniform float frameTimeCounter;\n\
             void main() {\n\
               gl_Position = ftransform();\n\
               texcoord = (gl_TextureMatrix[0] * gl_MultiTexCoord0).xy;\n\
               glcolor = gl_Color * (mc_Entity.x < 0.0 ? 1.0 : 0.5) * frameTimeCounter;\n\
             }\n",
            "#version 120\n\
             varying vec2 texcoord;\nvarying vec4 glcolor;\n\
             uniform sampler2D texture;\nuniform float rainStrength;\n\
             void main() {\n\
               vec4 color = texture2D(texture, texcoord) * glcolor;\n\
               color.rgb *= 1 - rainStrength * 0.5;\n\
               gl_FragData[0] = color;\n\
               gl_FragData[2] = vec4(0, 0, 1, 1);\n\
             }\n",
        );
        assert_eq!(t.outputs, 3);
        assert_eq!(
            t.uniforms
                .iter()
                .map(|u| u.name.as_str())
                .collect::<Vec<_>>(),
            ["frameTimeCounter", "rainStrength"]
        );
        // `texture` is the geometry program's albedo, in its own set.
        assert!(
            t.fragment
                .contains("set = 3, binding = 0) uniform texture2D pt_gtexture")
        );
        assert!(t.samplers.is_empty());
    }

    #[test]
    fn new_style_outputs_keep_their_locations() {
        let t = translated(
            "#version 330 compatibility\nout vec2 uv;\nvoid main() { gl_Position = ftransform(); uv = gl_MultiTexCoord0.xy; }\n",
            "#version 330 compatibility\nin vec2 uv;\nuniform sampler2D colortex0;\n\
             /* RENDERTARGETS: 0,3 */\n\
             layout(location = 0) out vec4 albedo;\nlayout(location = 1) out vec4 extra;\n\
             void main() { albedo = texture(colortex0, uv); extra = vec4(uv, 0, 1); }\n",
        );
        assert_eq!(t.outputs, 2);
    }

    #[test]
    fn std140_offsets() {
        let t = translated(
            "#version 120\nvoid main() { gl_Position = ftransform(); }\n",
            "#version 120\nuniform float a;\nuniform vec3 b;\nuniform mat4 c;\nuniform int d;\nuniform bool e;\n\
             void main() { gl_FragColor = vec4(a) + vec4(b, d) + c[0] + (e ? vec4(1) : vec4(0)); }\n",
        );
        let offsets: Vec<_> = t
            .uniforms
            .iter()
            .map(|u| (u.name.as_str(), u.offset))
            .collect();
        assert_eq!(
            offsets,
            [("a", 0), ("b", 16), ("c", 32), ("d", 96), ("e", 100)]
        );
        assert_eq!(t.uniform_size, 112);
    }

    #[test]
    fn shadow_lookups_and_reserved_names() {
        translated(
            "#version 120\nvarying float sample;\nvoid main() { gl_Position = ftransform(); sample = 1.0; }\n",
            "#version 120\nvarying float sample;\nuniform sampler2DShadow shadowtex0;\n\
             void main() { float s = shadow2D(shadowtex0, vec3(0.5, 0.5, sample)).r; gl_FragColor = vec4(s); }\n",
        );
    }

    #[test]
    fn builtin_varyings_pass_between_stages() {
        let t = translated(
            "#version 120
void main() {
               gl_Position = ftransform();
               gl_FrontColor = gl_Color;
               gl_TexCoord[0] = gl_MultiTexCoord0;
               gl_FogFragCoord = length(gl_Position.xyz);
             }
",
            "#version 120
void main() {
               gl_FragColor = gl_Color * gl_TexCoord[0] * gl_FogFragCoord;
             }
",
        );
        assert!(
            t.vertex.contains("out vec4 pomme_FrontColor;"),
            "{}",
            t.vertex
        );
        assert!(
            t.fragment.contains("in vec4 pomme_TexCoord[2];"),
            "{}",
            t.fragment
        );
        assert!(
            t.fragment.contains("in float pomme_FogFragCoord;"),
            "{}",
            t.fragment
        );
    }

    #[test]
    fn forward_declared_sampler_functions() {
        translated(
            "#version 120\nvoid main() { gl_Position = ftransform(); }\n",
            "#version 120\nuniform sampler2D colortex0;\n\
             vec4 sample_at(sampler2D s, vec2 uv);\n\
             vec4 unnamed(sampler2D, vec2);\n\
             void main() { gl_FragColor = unnamed(colortex0, vec2(0.5)); }\n\
             vec4 sample_at(sampler2D s, vec2 uv) { return texture2D(s, uv); }\n\
             vec4 unnamed(sampler2D s, vec2 uv) { return sample_at(s, uv); }\n",
        );
    }

    #[test]
    fn fragment_only_varyings_clear_array_slots() {
        let t = translated(
            "#version 120\nvarying vec4 a[6];\nvoid main() { gl_Position = ftransform(); a[5] = vec4(1); }\n",
            "#version 120\nvarying vec4 a[6];\nvarying float only_here;\n\
             void main() { gl_FragColor = a[5] * only_here; }\n",
        );
        assert!(
            t.fragment
                .contains("layout(location = 6) in float only_here;"),
            "{}",
            t.fragment
        );
    }

    #[test]
    fn integer_varyings_are_flat() {
        let t = translated(
            "#version 330\nout int id;\nvoid main() { gl_Position = ftransform(); id = 3; }\n",
            "#version 330\nin int id;\nout vec4 color;\nvoid main() { color = vec4(id); }\n",
        );
        assert!(t.fragment.contains("flat in int id"), "{}", t.fragment);
    }
}
