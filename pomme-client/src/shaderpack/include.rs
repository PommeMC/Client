//! Iris's `#include` expansion: `"/lib/x.glsl"` from the `shaders/` root,
//! anything else beside the including file.

use super::source::PackSource;

/// Deeper than any pack nests, shallow enough to stop a cycle.
const MAX_DEPTH: usize = 32;

#[derive(Debug, thiserror::Error)]
pub enum IncludeError {
    #[error("{from}: can't find included {path}")]
    Missing { from: String, path: String },
    #[error("{0}: includes nest too deep")]
    TooDeep(String),
}

/// `path`'s source with every `#include` replaced by the file's contents.
pub fn expand(source: &PackSource, path: &str) -> Result<String, IncludeError> {
    let text = source.read(path).ok_or_else(|| IncludeError::Missing {
        from: "(program)".into(),
        path: path.into(),
    })?;
    let mut out = String::with_capacity(text.len() * 4);
    expand_into(source, path, &text, 0, &mut out)?;
    Ok(out)
}

fn expand_into(
    source: &PackSource,
    path: &str,
    text: &str,
    depth: usize,
    out: &mut String,
) -> Result<(), IncludeError> {
    if depth > MAX_DEPTH {
        return Err(IncludeError::TooDeep(path.into()));
    }
    for line in text.lines() {
        let Some(target) = include_target(line) else {
            out.push_str(line);
            out.push('\n');
            continue;
        };
        let resolved = resolve(path, target);
        let included = source
            .read(&resolved)
            .ok_or_else(|| IncludeError::Missing {
                from: path.into(),
                path: resolved.clone(),
            })?;
        expand_into(source, &resolved, &included, depth + 1, out)?;
    }
    Ok(())
}

/// The quoted path of an `#include "..."` line.
fn include_target(line: &str) -> Option<&str> {
    let rest = line.trim_start().strip_prefix('#')?.trim_start();
    let rest = rest.strip_prefix("include")?.trim();
    rest.strip_prefix('"')?.split('"').next()
}

/// `target` from the file at `from`, normalised to a `shaders/`-relative path.
fn resolve(from: &str, target: &str) -> String {
    let base: Vec<&str> = match target.strip_prefix('/') {
        Some(_) => Vec::new(),
        None => {
            let mut dirs: Vec<&str> = from.split('/').collect();
            dirs.pop();
            dirs
        }
    };
    let mut parts = base;
    for part in target.trim_start_matches('/').split('/') {
        match part {
            "" | "." => {}
            ".." => {
                parts.pop();
            }
            _ => parts.push(part),
        }
    }
    parts.join("/")
}

#[cfg(test)]
mod tests {
    use std::collections::HashMap;

    use super::*;

    fn pack(files: &[(&str, &str)]) -> PackSource {
        PackSource::Zip(
            files
                .iter()
                .map(|(p, s)| ((*p).to_owned(), s.as_bytes().to_vec()))
                .collect::<HashMap<_, _>>(),
        )
    }

    #[test]
    fn absolute_and_relative_includes() {
        let pack = pack(&[
            (
                "world0/a.fsh",
                "#version 130\n#include \"/program/a.glsl\"\n",
            ),
            (
                "program/a.glsl",
                "#include \"../lib/x.glsl\"\nvoid main() {}\n",
            ),
            ("lib/x.glsl", "  #  include \"y.glsl\"\nfloat x;\n"),
            ("lib/y.glsl", "float y;\n"),
        ]);
        assert_eq!(
            expand(&pack, "world0/a.fsh").unwrap(),
            "#version 130\nfloat y;\nfloat x;\nvoid main() {}\n"
        );
    }

    #[test]
    fn cycles_stop() {
        let pack = pack(&[("a.glsl", "#include \"/a.glsl\"\n")]);
        assert!(matches!(
            expand(&pack, "a.glsl"),
            Err(IncludeError::TooDeep(_))
        ));
    }
}
