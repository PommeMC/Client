//! Shared setup-time symlink policy for discovery and explicit registrations.

use std::fs::FileType;
use std::io;
use std::path::{Path, PathBuf};
use std::rc::Rc;

const SYMLINK_RULE_PREFIX: &str = "prefix";
const SYMLINK_RULE_GLOB: &str = "glob";
const SYMLINK_RULE_REGEX: &str = "regex";

/// Vanilla `PathAllowList`: a prefix test on the target's string, or a
/// `FileSystem.getPathMatcher` glob or regex. Both matcher kinds compile to one
/// whole-string regex, case-insensitive on Windows as the JDK's are.
enum SymlinkRule {
    Prefix(String),
    Matcher(regex::Regex),
}

/// Immutable setup snapshot shared by discovery and explicit registrations.
/// The default denies all symlinks; cloning never reparses or copies rules.
#[derive(Clone, Default)]
pub(crate) struct SymlinkPolicy {
    rules: Rc<[SymlinkRule]>,
}

impl SymlinkPolicy {
    /// Vanilla `LevelStorageSource.parseValidator`: no file denies every
    /// symlink quietly, and any read or parse error discards the whole list.
    pub(crate) fn load(path: &Path) -> Self {
        if !path.exists() {
            return Self::default();
        }
        match std::fs::read_to_string(path).and_then(|text| parse_symlinks(&text)) {
            Ok(rules) => Self {
                rules: rules.into(),
            },
            Err(error) => {
                tracing::warn!(path = %path.display(), %error, "Cannot load allowed symlinks; denying all symlinks");
                Self::default()
            }
        }
    }
}

fn parse_symlinks(text: &str) -> io::Result<Vec<SymlinkRule>> {
    let mut rules = Vec::new();
    for (index, line) in text.lines().enumerate() {
        if line.trim().is_empty() || line.starts_with('#') {
            continue;
        }
        let invalid = |message: String| {
            io::Error::new(
                io::ErrorKind::InvalidData,
                format!("line {}: {message}", index + 1),
            )
        };
        let (tag, pattern) = if let Some(tagged) = line.strip_prefix('[') {
            tagged
                .split_once(']')
                .ok_or_else(|| invalid("unterminated rule type".to_owned()))?
        } else {
            (SYMLINK_RULE_PREFIX, line)
        };
        let rule = match tag {
            SYMLINK_RULE_PREFIX => SymlinkRule::Prefix(pattern.to_owned()),
            SYMLINK_RULE_GLOB | SYMLINK_RULE_REGEX => {
                let regex = if tag == SYMLINK_RULE_GLOB {
                    glob_to_regex(pattern, cfg!(windows)).map_err(invalid)?
                } else {
                    // TODO: Java `Pattern` syntax, not Rust `regex`: lookaround
                    // and backreferences fail to parse, which denies all.
                    pattern.to_owned()
                };
                let flags = if cfg!(windows) { "(?i)" } else { "" };
                let compiled = regex::Regex::new(&format!(r"{flags}\A(?:{regex})\z"))
                    .map_err(|error| invalid(error.to_string()))?;
                SymlinkRule::Matcher(compiled)
            }
            _ => return Err(invalid(format!("unknown rule type {tag:?}"))),
        };
        rules.push(rule);
    }
    Ok(rules)
}

/// The JDK's `sun.nio.fs.Globs.toRegexPattern`: `*` and `?` stay within one
/// name, `**` crosses separators, `[...]` never matches a separator, and
/// `{a,b}` alternates. `dos` is the Windows variant, where `\` separates.
fn glob_to_regex(glob: &str, dos: bool) -> Result<String, String> {
    const REGEX_META: &str = ".^$+{[]|()";
    const GLOB_META: &str = "\\*?[{";
    let one_char = if dos { r"[^\\]" } else { "[^/]" };
    let chars: Vec<char> = glob.chars().collect();
    let next = |i: usize| chars.get(i).copied();
    let mut regex = String::new();
    let mut in_group = false;
    let mut i = 0;
    while i < chars.len() {
        let c = chars[i];
        i += 1;
        match c {
            '\\' => {
                let escaped = next(i).ok_or("no character to escape")?;
                if GLOB_META.contains(escaped) || REGEX_META.contains(escaped) {
                    regex.push('\\');
                }
                regex.push(escaped);
                i += 1;
            }
            '/' if dos => regex.push_str(r"\\"),
            '[' => {
                regex.push('[');
                regex.push_str(one_char);
                regex.push_str("&&[");
                if next(i) == Some('^') {
                    regex.push_str(r"\^");
                    i += 1;
                } else {
                    if next(i) == Some('!') {
                        regex.push('^');
                        i += 1;
                    }
                    if next(i) == Some('-') {
                        regex.push('-');
                        i += 1;
                    }
                }
                let mut has_range_start = false;
                let mut last = '\0';
                let mut closed = false;
                while i < chars.len() {
                    let c = chars[i];
                    i += 1;
                    if c == ']' {
                        closed = true;
                        break;
                    }
                    if c == '/' || (dos && c == '\\') {
                        return Err("explicit name separator in class".to_owned());
                    }
                    if c == '\\' || c == '[' || (c == '&' && next(i) == Some('&')) {
                        regex.push('\\');
                    }
                    regex.push(c);
                    if c == '-' {
                        if !has_range_start {
                            return Err("invalid range".to_owned());
                        }
                        let end = next(i);
                        i += 1;
                        match end {
                            None => break,
                            Some(']') => {
                                closed = true;
                                break;
                            }
                            Some(end) if end < last => return Err("invalid range".to_owned()),
                            Some(end) => regex.push(end),
                        }
                        has_range_start = false;
                    } else {
                        has_range_start = true;
                        last = c;
                    }
                }
                if !closed {
                    return Err("missing ']'".to_owned());
                }
                regex.push_str("]]");
            }
            '{' => {
                if in_group {
                    return Err("cannot nest groups".to_owned());
                }
                regex.push_str("(?:(?:");
                in_group = true;
            }
            '}' if in_group => {
                regex.push_str("))");
                in_group = false;
            }
            ',' if in_group => regex.push_str(")|(?:"),
            '*' if next(i) == Some('*') => {
                regex.push_str(".*");
                i += 1;
            }
            '*' => {
                regex.push_str(one_char);
                regex.push('*');
            }
            '?' => regex.push_str(one_char),
            c => {
                if REGEX_META.contains(c) {
                    regex.push('\\');
                }
                regex.push(c);
            }
        }
    }
    if in_group {
        return Err("missing '}'".to_owned());
    }
    Ok(regex)
}

fn validate_symlink(path: &Path, rules: &[SymlinkRule]) -> io::Result<PathBuf> {
    let target = std::fs::read_link(path)?;
    // Match the stored target; non-UTF8 fails closed, never lossy matching.
    let allowed = target.to_str().is_some_and(|target| {
        rules.iter().any(|rule| match rule {
            SymlinkRule::Prefix(prefix) => target.starts_with(prefix),
            SymlinkRule::Matcher(pattern) => pattern.is_match(target),
        })
    });
    if !allowed {
        return Err(io::Error::new(
            io::ErrorKind::PermissionDenied,
            format!(
                "forbidden symlink {} -> {}",
                path.display(),
                target.display()
            ),
        ));
    }
    Ok(target)
}

/// Setup boundary, not a sandbox: no ancestor checks, canonicalization, or
/// descendant-link traversal. Installation/index/theme roots bypass this gate.
/// Reuse the caller's no-follow classification; classify only a distinct
/// target.
pub(super) fn prepare_pack_path(
    path: &Path,
    mut kind: FileType,
    policy: &SymlinkPolicy,
) -> io::Result<(PathBuf, FileType)> {
    let rules = &policy.rules;
    let mut effective = path.to_owned();
    if kind.is_symlink() {
        // Java PackDetector follows one stored target, relative to CWD, not
        // the link parent. Open this effective path, never the original alias.
        effective = validate_symlink(path, rules)?;
        kind = std::fs::symlink_metadata(&effective)?.file_type();
    }
    if kind.is_dir() {
        validate_pack_tree(&effective, rules)?;
    } else if !kind.is_file() {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            format!(
                "pack root is not a directory or regular file: {}",
                effective.display()
            ),
        ));
    }
    Ok((effective, kind))
}

fn validate_pack_tree(root: &Path, rules: &[SymlinkRule]) -> io::Result<()> {
    for entry in std::fs::read_dir(root)? {
        let entry = entry?;
        let kind = entry.file_type()?;
        if kind.is_symlink() {
            validate_symlink(&entry.path(), rules)?;
        } else if kind.is_dir() {
            validate_pack_tree(&entry.path(), rules)?;
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::{SymlinkRule, glob_to_regex, parse_symlinks};

    fn glob_matches(glob: &str, dos: bool, path: &str) -> bool {
        let regex = glob_to_regex(glob, dos).unwrap();
        regex::Regex::new(&format!(r"\A(?:{regex})\z"))
            .unwrap()
            .is_match(path)
    }

    #[test]
    fn globs_match_like_path_matcher() {
        assert!(glob_matches("/srv/packs/*", false, "/srv/packs/a"));
        assert!(!glob_matches("/srv/packs/*", false, "/srv/packs/a/b"));
        assert!(glob_matches("/srv/packs/**", false, "/srv/packs/a/b"));
        assert!(glob_matches("/home/me/packs**", false, "/home/me/packs2/x"));
        assert!(glob_matches("/srv/{packs,maps}/*", false, "/srv/maps/x"));
        assert!(!glob_matches("/srv/{packs,maps}/*", false, "/srv/other/x"));
        assert!(glob_matches("/srv/pack?", false, "/srv/pack1"));
        assert!(glob_matches("/srv/[a-c]x", false, "/srv/bx"));
        assert!(!glob_matches("/srv/[!a-c]x", false, "/srv/bx"));
        assert!(glob_matches("/srv/a.b+c", false, "/srv/a.b+c"));
        assert!(!glob_matches("/srv/a.b", false, "/srv/axb"));
        assert!(glob_to_regex("/srv/{a,{b}}", false).is_err());
        assert!(glob_to_regex("/srv/[a/b]", false).is_err());
        assert!(glob_to_regex("/srv/[ab", false).is_err());
    }

    #[test]
    fn dos_globs_use_backslash_separators() {
        assert!(glob_matches("C:/packs/*", true, r"C:\packs\a"));
        assert!(!glob_matches("C:/packs/*", true, r"C:\packs\a\b"));
    }

    #[test]
    fn parses_rule_lines() {
        let rules =
            parse_symlinks("# comment\n\n/srv\n[glob]/a/*\n[regex]/b/.*\n[prefix]/c\n").unwrap();
        assert!(matches!(
            rules.as_slice(),
            [
                SymlinkRule::Prefix(_),
                SymlinkRule::Matcher(_),
                SymlinkRule::Matcher(_),
                SymlinkRule::Prefix(_)
            ]
        ));
        assert!(parse_symlinks("[glob/a").is_err());
        assert!(parse_symlinks("[other]/a").is_err());
    }
}
