//! What Iris adds to a program's text before compiling: its standard macros
//! after `#version`, and the player's option overrides on the pack's own
//! `#define` lines.

use std::collections::BTreeMap;
use std::sync::LazyLock;

use regex::Regex;

/// An option's value from `shaderpacks/<pack>.txt`.
#[derive(Clone, Debug, PartialEq)]
pub enum OptionValue {
    Toggle(bool),
    Value(String),
}

pub type Overrides = BTreeMap<String, OptionValue>;

/// Iris's `StandardMacros`, as a client on the Vulkan path reports them.
fn standard_macros() -> Vec<(&'static str, &'static str)> {
    let os = if cfg!(windows) {
        "MC_OS_WINDOWS"
    } else if cfg!(target_os = "macos") {
        "MC_OS_MAC"
    } else if cfg!(target_os = "linux") || cfg!(target_os = "android") {
        "MC_OS_LINUX"
    } else {
        "MC_OS_UNKNOWN"
    };
    vec![
        ("MC_VERSION", "260300"),
        ("MC_GL_VERSION", "460"),
        ("MC_GLSL_VERSION", "460"),
        (os, ""),
        ("MC_GL_VENDOR_OTHER", ""),
        ("MC_GL_RENDERER_OTHER", ""),
        ("MC_RENDER_QUALITY", "1.0"),
        ("MC_SHADOW_QUALITY", "1.0"),
        ("MC_HAND_DEPTH", "0.125"),
        ("IS_IRIS", ""),
    ]
}

/// The program's text with the standard macros after its `#version` line and
/// `overrides` applied.
pub fn prepare(expanded: &str, overrides: &Overrides) -> String {
    let expanded = &super::directives::mark_draw_buffers(expanded);
    let mut out = String::with_capacity(expanded.len() + 512);
    let mut lines = expanded.lines();
    let mut version_seen = false;
    for line in lines.by_ref() {
        out.push_str(&override_line(line, overrides));
        out.push('\n');
        if line.trim_start().starts_with("#version") {
            version_seen = true;
            break;
        }
    }
    let mut macros = String::new();
    for (name, value) in standard_macros() {
        macros.push_str(&format!("#define {name} {value}\n"));
    }
    if version_seen {
        out.push_str(&macros);
    } else {
        out.insert_str(0, &macros);
    }
    for line in lines {
        out.push_str(&override_line(line, overrides));
        out.push('\n');
    }
    out
}

/// `line` with an overridden option's `#define` switched or re-valued.
fn override_line(line: &str, overrides: &Overrides) -> String {
    static DEFINE: LazyLock<Regex> = LazyLock::new(|| {
        Regex::new(r"^(\s*)(//\s*)?#define\s+(\w+)(\s+[^/\s]+)?(.*)$").expect("static regex")
    });
    if overrides.is_empty() {
        return line.to_owned();
    }
    let Some(caps) = DEFINE.captures(line) else {
        return line.to_owned();
    };
    let Some(value) = overrides.get(&caps[3]) else {
        return line.to_owned();
    };
    let indent = &caps[1];
    let name = &caps[3];
    let rest = &caps[5];
    match value {
        OptionValue::Toggle(true) => format!("{indent}#define {name}{rest}"),
        OptionValue::Toggle(false) => format!("{indent}//#define {name}{rest}"),
        OptionValue::Value(v) => format!("{indent}#define {name} {v}{rest}"),
    }
}

/// Parses `shaderpacks/<pack>.txt`: `NAME=value` lines, `true`/`false` for
/// toggles.
pub fn parse_overrides(text: &str) -> Overrides {
    text.lines()
        .filter_map(|line| {
            let (name, value) = line.split_once('=')?;
            let value = value.trim();
            let value = match value {
                "true" => OptionValue::Toggle(true),
                "false" => OptionValue::Toggle(false),
                _ => OptionValue::Value(value.to_owned()),
            };
            Some((name.trim().to_owned(), value))
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn macros_follow_the_version_line() {
        let out = prepare(
            "// header\n#version 130\nvoid main() {}\n",
            &Overrides::new(),
        );
        let lines: Vec<&str> = out.lines().collect();
        assert_eq!(lines[1], "#version 130");
        assert!(lines[2].starts_with("#define MC_VERSION"));
        assert_eq!(*lines.last().unwrap(), "void main() {}");
    }

    #[test]
    fn overrides_switch_and_revalue_defines() {
        let overrides = parse_overrides("SHADOWS=false\nBLOOM=true\nSTRENGTH=3.0\n");
        let out = prepare(
            "#version 120\n#define SHADOWS // [x]\n//#define BLOOM\n  #define STRENGTH 1.0 // [1.0 2.0 3.0]\n#define OTHER 2\n",
            &overrides,
        );
        assert!(out.contains("\n//#define SHADOWS // [x]\n"));
        assert!(out.contains("\n#define BLOOM\n"));
        assert!(out.contains("\n  #define STRENGTH 3.0 // [1.0 2.0 3.0]\n"));
        assert!(out.contains("\n#define OTHER 2\n"));
    }
}
