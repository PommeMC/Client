//! Shared setup-time symlink policy for discovery and explicit registrations.

use std::fs::FileType;
use std::io;
use std::path::{Path, PathBuf};
use std::rc::Rc;

const SYMLINK_RULE_PREFIX: &str = "prefix";
const SYMLINK_RULE_GLOB: &str = "glob";
const SYMLINK_RULE_REGEX: &str = "regex";

/// COMPAT: Native Rust matching, NOT Java PathMatcher/Pattern parity. Accepted
/// syntax, Unicode classes, case and platform separators can differ.
enum SymlinkRule {
    Prefix(String),
    Glob(glob::Pattern),
    Regex(regex::Regex),
}

/// Immutable setup snapshot shared by discovery and explicit registrations.
/// The default denies all symlinks; cloning never reparses or copies rules.
#[derive(Clone, Default)]
pub(crate) struct SymlinkPolicy {
    rules: Rc<[SymlinkRule]>,
}

impl SymlinkPolicy {
    /// Read once now; any load/parse error discards the entire rule list.
    pub(crate) fn load(path: Option<&Path>) -> Self {
        let Some(path) = path else {
            return Self::default();
        };
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
        let (tag, pattern) = if let Some(tagged) = line.strip_prefix('[') {
            tagged.split_once(']').ok_or_else(|| {
                io::Error::new(
                    io::ErrorKind::InvalidData,
                    format!("line {}: unterminated rule type", index + 1),
                )
            })?
        } else {
            (SYMLINK_RULE_PREFIX, line)
        };
        let rule = match tag {
            SYMLINK_RULE_PREFIX => SymlinkRule::Prefix(pattern.to_owned()),
            SYMLINK_RULE_GLOB => {
                SymlinkRule::Glob(glob::Pattern::new(pattern).map_err(|error| {
                    io::Error::new(
                        io::ErrorKind::InvalidData,
                        format!("line {}: {error}", index + 1),
                    )
                })?)
            }
            SYMLINK_RULE_REGEX => {
                // Validate raw syntax first: it must not escape the whole-match
                // wrapper. Group alternatives and flags inside absolute anchors.
                regex::Regex::new(pattern).map_err(|error| {
                    io::Error::new(
                        io::ErrorKind::InvalidData,
                        format!("line {}: {error}", index + 1),
                    )
                })?;
                SymlinkRule::Regex(regex::Regex::new(&format!(r"\A(?:{pattern})\z")).map_err(
                    |error| {
                        io::Error::new(
                            io::ErrorKind::InvalidData,
                            format!("line {}: {error}", index + 1),
                        )
                    },
                )?)
            }
            _ => {
                return Err(io::Error::new(
                    io::ErrorKind::InvalidData,
                    format!("line {}: unknown rule type {tag:?}", index + 1),
                ));
            }
        };
        rules.push(rule);
    }
    Ok(rules)
}

fn validate_symlink(path: &Path, rules: &[SymlinkRule]) -> io::Result<PathBuf> {
    let target = std::fs::read_link(path)?;
    // Match the stored target; non-UTF8 fails closed, never lossy matching.
    let allowed = target.to_str().is_some_and(|target| {
        rules.iter().any(|rule| match rule {
            SymlinkRule::Prefix(prefix) => target.starts_with(prefix),
            SymlinkRule::Glob(pattern) => pattern.matches_with(
                target,
                glob::MatchOptions {
                    case_sensitive: true,
                    require_literal_separator: true,
                    require_literal_leading_dot: false,
                },
            ),
            SymlinkRule::Regex(pattern) => pattern.is_match(target),
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
