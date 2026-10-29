//! Java-properties files a pack ships: `shaders.properties` and
//! `dimension.properties`.

use std::collections::BTreeMap;
use std::sync::LazyLock;

use regex::Regex;

use super::expr::{Context, Expr, Value};

/// `key=value` pairs in file order, with `#` comments and `\` line
/// continuations.
pub fn parse_ordered(text: &str) -> Vec<(String, String)> {
    let mut entries = Vec::new();
    let mut pending = String::new();
    for line in text.lines() {
        let line = line.trim();
        if pending.is_empty() && (line.starts_with('#') || line.starts_with('!')) {
            continue;
        }
        match line.strip_suffix('\\') {
            Some(continued) => {
                pending.push_str(continued);
                pending.push(' ');
                continue;
            }
            None => pending.push_str(line),
        }
        let entry = std::mem::take(&mut pending);
        if let Some((key, value)) = entry.split_once(['=', ':']) {
            entries.push((key.trim().to_owned(), value.trim().to_owned()));
        }
    }
    entries
}

/// Iris runs a properties file through the C preprocessor with the pack's
/// option macros first: `#if`/`#ifdef`/`#else` pick lines, `#define`s add
/// macros, and macros expand in the lines kept. Other `#` lines are comments.
pub fn preprocess(text: &str, macros: &BTreeMap<String, String>) -> String {
    struct Branch {
        /// Lines in it are kept.
        active: bool,
        /// One of its `#if`/`#elif` arms has been taken.
        taken: bool,
        /// The enclosing branch is active.
        parent: bool,
    }
    let mut macros = macros.clone();
    let mut branches: Vec<Branch> = Vec::new();
    let mut out = String::with_capacity(text.len());
    for line in text.lines() {
        let active = branches.last().is_none_or(|b| b.active);
        if let Some(directive) = line.trim_start().strip_prefix('#') {
            // A directive's trailing comment isn't part of its value.
            let directive = ["//", "/*"]
                .iter()
                .filter_map(|c| directive.find(c))
                .min()
                .map_or(directive, |end| &directive[..end])
                .trim();
            let (word, arg) = directive
                .split_once(char::is_whitespace)
                .map_or((directive, ""), |(w, a)| (w, a.trim()));
            let name = arg.split_whitespace().next().unwrap_or("");
            let condition = match word {
                "ifdef" => Some(macros.contains_key(name)),
                "ifndef" => Some(!macros.contains_key(name)),
                "if" | "elif" => Some(condition(arg, &macros)),
                _ => None,
            };
            match (word, condition) {
                ("if" | "ifdef" | "ifndef", Some(holds)) => {
                    branches.push(Branch {
                        active: active && holds,
                        taken: holds,
                        parent: active,
                    });
                    continue;
                }
                ("elif", Some(holds)) => {
                    if let Some(b) = branches.last_mut() {
                        b.active = b.parent && !b.taken && holds;
                        b.taken |= holds;
                    }
                    continue;
                }
                ("else", _) => {
                    if let Some(b) = branches.last_mut() {
                        b.active = b.parent && !b.taken;
                        b.taken = true;
                    }
                    continue;
                }
                ("endif", _) => {
                    branches.pop();
                    continue;
                }
                ("define", _) => {
                    if active {
                        let value = arg[name.len()..].trim();
                        macros.insert(name.to_owned(), value.to_owned());
                    }
                    continue;
                }
                ("undef", _) => {
                    if active {
                        macros.remove(name);
                    }
                    continue;
                }
                _ => {}
            }
        }
        if active {
            out.push_str(&expand(line, &macros));
            out.push('\n');
        }
    }
    out
}

/// `text` with each macro name replaced by its value, until none are left.
/// A macro with no value stays as its name, so a condition still reads it
/// as defined.
fn expand(text: &str, macros: &BTreeMap<String, String>) -> String {
    static IDENT: LazyLock<Regex> =
        LazyLock::new(|| Regex::new(r"\b[A-Za-z_]\w*\b").expect("static regex"));
    let mut text = text.to_owned();
    // Deep enough for macros defined in terms of others, and stops cycles.
    for _ in 0..8 {
        let expanded = IDENT.replace_all(&text, |caps: &regex::Captures| {
            macros
                .get(&caps[0])
                .filter(|value| !value.is_empty())
                .cloned()
                .unwrap_or_else(|| caps[0].to_owned())
        });
        if expanded == text {
            break;
        }
        text = expanded.into_owned();
    }
    text
}

/// An `#if` condition or a `program.X.enabled` value: `defined` resolved,
/// macros expanded, and a name left reading 1 if it's defined, else 0.
pub fn condition(text: &str, macros: &BTreeMap<String, String>) -> bool {
    static DEFINED: LazyLock<Regex> =
        LazyLock::new(|| Regex::new(r"defined\s*(?:\(\s*(\w+)\s*\)|(\w+))").expect("static regex"));
    let text = DEFINED.replace_all(text, |caps: &regex::Captures| {
        let name = caps.get(1).or(caps.get(2)).map_or("", |m| m.as_str());
        if macros.contains_key(name) { "1" } else { "0" }
    });
    let text = expand(&text, macros);
    let Ok(expr) = Expr::parse(&text, &mut 0) else {
        tracing::warn!("Shader pack: can't read the condition `{text}`");
        return false;
    };
    expr.eval(&mut Context {
        lookup: &|name| Some(Value::Int(i32::from(macros.contains_key(name)))),
        frame_time: 0.0,
        smooth: &mut [],
        missing: Vec::new(),
    })
    .as_bool()
}

/// The folder (`world0`, `world-1`, ...) a pack's `dimension.properties`
/// gives `dimension` (e.g. `minecraft:the_nether`), else Iris's defaults.
pub fn dimension_folder(dimensions: &BTreeMap<String, String>, dimension: &str) -> String {
    let mut wildcard = None;
    for (key, ids) in dimensions {
        let Some(folder) = key.strip_prefix("dimension.") else {
            continue;
        };
        for id in ids.split_whitespace() {
            if id == dimension {
                return folder.to_owned();
            }
            if id == "*" {
                wildcard = Some(folder.to_owned());
            }
        }
    }
    wildcard.unwrap_or_else(|| {
        match dimension {
            "minecraft:the_nether" => "world-1",
            "minecraft:the_end" => "world1",
            _ => "world0",
        }
        .to_owned()
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parse(text: &str) -> BTreeMap<String, String> {
        parse_ordered(text).into_iter().collect()
    }

    #[test]
    fn continuations_and_comments() {
        let props =
            parse("# comment\nprogram.world0/composite2.enabled = false\nscreen = A B \\\n  C\n");
        assert_eq!(props["program.world0/composite2.enabled"], "false");
        assert_eq!(props["screen"], "A B  C");
    }

    #[test]
    fn preprocessing() {
        let macros = BTreeMap::from([
            ("SHADOW_QUALITY".to_owned(), "-1".to_owned()),
            ("BLOOM".to_owned(), String::new()),
            ("RES".to_owned(), "SIZE".to_owned()),
            ("SIZE".to_owned(), "0.5".to_owned()),
        ]);
        let out = preprocess(
            "# a comment
             #if SHADOW_QUALITY == -1
  a = 1
#elif defined BLOOM
  a = 2
#else
  a = 3
#endif
             #ifdef BLOOM
  #ifndef FOG
    b = RES RES
  #endif
#endif
             #if defined(FOG) || UNKNOWN > 0
  c = 1
#endif
             #define LOCAL 4 // [2 4 8]
d = LOCAL
#if LOCAL == 4 // comment
e = 1
#endif
",
            &macros,
        );
        let props = parse(&out);
        assert_eq!(props["a"], "1");
        assert_eq!(props["b"], "0.5 0.5");
        assert!(!props.contains_key("c"));
        assert_eq!(props["d"], "4");
        assert_eq!(props["e"], "1");
        assert!(condition("BLOOM && !FOG", &macros));
        assert!(!condition("FOG || SHADOW_QUALITY > 0", &macros));
        assert!(condition("true", &macros));
    }

    #[test]
    fn dimension_folders() {
        let props =
            parse("dimension.world0=*\ndimension.world-1=minecraft:the_nether minecraft:nether\n");
        assert_eq!(dimension_folder(&props, "minecraft:the_nether"), "world-1");
        assert_eq!(dimension_folder(&props, "minecraft:overworld"), "world0");
        assert_eq!(dimension_folder(&props, "mod:custom"), "world0");
        assert_eq!(
            dimension_folder(&BTreeMap::new(), "minecraft:the_end"),
            "world1"
        );
    }
}
