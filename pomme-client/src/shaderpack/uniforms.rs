//! `shaders.properties` custom uniforms: `uniform.<type>.<name>=<expr>`
//! lines become uniforms the pack's programs can declare, `variable.` lines
//! values later lines read. Evaluated in file order each frame.

use glam::{Vec2, Vec4};

use super::expr::{Context, Expr, Value};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Type {
    Float,
    Int,
    Bool,
    Vec2,
    Vec3,
    Vec4,
}

impl Type {
    fn of(name: &str) -> Option<Self> {
        Some(match name {
            "float" => Self::Float,
            "int" => Self::Int,
            "bool" => Self::Bool,
            "vec2" => Self::Vec2,
            "vec3" => Self::Vec3,
            "vec4" => Self::Vec4,
            _ => return None,
        })
    }

    fn convert(self, value: Value) -> Value {
        let v = value.components().map_or(Vec4::ZERO, |(c, _)| c);
        match self {
            Self::Float => Value::Float(value.as_f32()),
            Self::Int => Value::Int(value.as_i32()),
            Self::Bool => Value::Bool(value.as_bool()),
            Self::Vec2 => Value::Vec2(Vec2::new(v.x, v.y)),
            Self::Vec3 => Value::Vec3(v.truncate()),
            Self::Vec4 => Value::Vec4(v),
        }
    }
}

#[derive(Clone)]
struct Entry {
    name: String,
    ty: Type,
    /// A `uniform.` line, rather than a `variable.` one.
    uniform: bool,
    expr: Expr,
}

#[derive(Clone, Default)]
pub struct CustomUniforms {
    entries: Vec<Entry>,
    /// Each entry's value from the last evaluation.
    values: Vec<Value>,
    smooth: Vec<Option<f32>>,
    warned: bool,
}

impl CustomUniforms {
    /// The custom uniforms and variables among `properties`, in file order;
    /// a line that doesn't parse is skipped.
    pub fn parse(properties: &[(String, String)]) -> Self {
        let mut entries = Vec::new();
        let mut smooth_slots = 0;
        for (key, text) in properties {
            let mut parts = key.splitn(3, '.');
            let uniform = match parts.next() {
                Some("uniform") => true,
                Some("variable") => false,
                _ => continue,
            };
            let (Some(ty), Some(name)) = (parts.next().and_then(Type::of), parts.next()) else {
                continue;
            };
            let expr = match Expr::parse(text, &mut smooth_slots) {
                Ok(expr) => expr,
                Err(error) => {
                    tracing::warn!("Shader pack: skipped {key}: {error}");
                    continue;
                }
            };
            entries.push(Entry {
                name: name.to_owned(),
                ty,
                uniform,
                expr,
            });
        }
        Self {
            values: vec![Value::Float(0.0); entries.len()],
            entries,
            smooth: vec![None; smooth_slots],
            warned: false,
        }
    }

    /// Evaluates every line in order: each reads the lines before it, then
    /// `builtin`.
    pub fn evaluate(&mut self, builtin: &dyn Fn(&str) -> Option<Value>, frame_time: f32) {
        let mut missing = Vec::new();
        for i in 0..self.entries.len() {
            let (before, _) = self.entries.split_at(i);
            let values = &self.values;
            let lookup = |name: &str| {
                before
                    .iter()
                    .rposition(|e| e.name == name)
                    .map(|j| values[j])
                    .or_else(|| builtin(name))
            };
            let mut cx = Context {
                lookup: &lookup,
                frame_time,
                smooth: &mut self.smooth,
                missing: Vec::new(),
            };
            let entry = &self.entries[i];
            let value = entry.ty.convert(entry.expr.eval(&mut cx));
            missing.append(&mut cx.missing);
            self.values[i] = value;
        }
        if !missing.is_empty() && !self.warned {
            missing.sort_unstable();
            missing.dedup();
            // TODO: the Iris inputs Pomme doesn't feed yet read zero.
            tracing::warn!(
                "Shader pack custom uniforms read unknown names as 0: {}",
                missing.join(", ")
            );
            self.warned = true;
        }
    }

    /// A `uniform.` line's value from the last evaluation.
    pub fn get(&self, name: &str) -> Option<Value> {
        self.entries
            .iter()
            .rposition(|e| e.uniform && e.name == name)
            .map(|i| self.values[i])
    }
}

#[cfg(test)]
mod tests {
    use glam::Vec3;

    use super::*;
    use crate::shaderpack::properties::parse_ordered;

    #[test]
    fn lines_read_earlier_lines_and_builtins() {
        let properties = parse_ordered(
            "uniform.float.framemod2 = frameCounter % 2\n\
             variable.float.difX = cameraPosition.x - previousCameraPosition.x\n\
             variable.float.moving = if(abs(difX) > 0.0 && abs(difX) < 1.0, 1, 0)\n\
             uniform.float.starter = smooth(3, moving, 20, 20)\n\
             uniform.vec3.tint = vec3(difX, 1, 2)\n\
             uniform.bool.even = framemod2 == 0\n\
             screen = A B\n",
        );
        let mut uniforms = CustomUniforms::parse(&properties);
        let builtin = |name: &str| match name {
            "frameCounter" => Some(Value::Int(5)),
            "cameraPosition" => Some(Value::Vec3(Vec3::new(0.5, 0.0, 0.0))),
            "previousCameraPosition" => Some(Value::Vec3(Vec3::ZERO)),
            _ => None,
        };
        uniforms.evaluate(&builtin, 1.0 / 60.0);
        assert_eq!(uniforms.get("framemod2"), Some(Value::Float(1.0)));
        assert_eq!(uniforms.get("starter"), Some(Value::Float(1.0)));
        assert_eq!(
            uniforms.get("tint"),
            Some(Value::Vec3(Vec3::new(0.5, 1.0, 2.0)))
        );
        assert_eq!(uniforms.get("even"), Some(Value::Bool(false)));
        assert_eq!(uniforms.get("difX"), None);
    }

    #[test]
    fn bad_lines_are_skipped() {
        let properties = parse_ordered(
            "uniform.float.broken = 1 +\n\
             uniform.bool.same = equals(1, 1)\n\
             uniform.float.noise = random(2, 3)\n",
        );
        let mut uniforms = CustomUniforms::parse(&properties);
        uniforms.evaluate(&|_| None, 0.0);
        assert_eq!(uniforms.get("broken"), None);
        assert_eq!(uniforms.get("same"), Some(Value::Bool(true)));
        let noise = uniforms.get("noise").unwrap().as_f32();
        assert!((2.0..3.0).contains(&noise));
    }
}
