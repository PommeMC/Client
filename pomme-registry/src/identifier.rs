//! Vanilla `Identifier`: a namespace and a path, written `namespace:path`.

use std::fmt;
use std::hash::{Hash, Hasher};
use std::str::FromStr;

/// `Identifier.DEFAULT_NAMESPACE`.
pub const DEFAULT_NAMESPACE: &str = "minecraft";

/// A namespaced name, like `minecraft:stone`.
///
/// Unlike vanilla's, any character is accepted: what a server names is kept
/// as sent rather than refused.
#[derive(Clone, Default, Eq)]
pub struct Identifier {
    /// As written, without the default namespace when it was left out.
    text: Box<str>,
    /// Where the path starts in `text`: past the colon when it names a
    /// namespace.
    path_at: usize,
}

impl Identifier {
    /// `Identifier.parse`: the namespace is what precedes the first colon,
    /// and the default one when there is no colon or nothing before it.
    pub fn new(text: impl Into<String>) -> Self {
        let mut text = text.into();
        let path_at = match text.find(':') {
            Some(0) => {
                text.remove(0);
                0
            }
            Some(colon) => colon + 1,
            None => 0,
        };
        Self {
            text: text.into(),
            path_at,
        }
    }

    pub fn namespace(&self) -> &str {
        match self.path_at {
            0 => DEFAULT_NAMESPACE,
            path_at => &self.text[..path_at - 1],
        }
    }

    pub fn path(&self) -> &str {
        &self.text[self.path_at..]
    }
}

impl PartialEq for Identifier {
    fn eq(&self, other: &Self) -> bool {
        self.path() == other.path() && self.namespace() == other.namespace()
    }
}

impl Hash for Identifier {
    fn hash<H: Hasher>(&self, state: &mut H) {
        self.namespace().hash(state);
        self.path().hash(state);
    }
}

impl fmt::Display for Identifier {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}:{}", self.namespace(), self.path())
    }
}

impl fmt::Debug for Identifier {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        fmt::Display::fmt(self, f)
    }
}

impl FromStr for Identifier {
    type Err = std::convert::Infallible;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        Ok(Self::new(s))
    }
}

impl From<&str> for Identifier {
    fn from(s: &str) -> Self {
        Self::new(s)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parts(text: &str) -> (String, String) {
        let ident = Identifier::new(text);
        (ident.namespace().to_owned(), ident.path().to_owned())
    }

    #[test]
    fn the_namespace_is_what_precedes_the_first_colon() {
        assert_eq!(parts("mod:thing"), ("mod".into(), "thing".into()));
        assert_eq!(parts("mod:a:b"), ("mod".into(), "a:b".into()));
        assert_eq!(parts("mod:"), ("mod".into(), "".into()));
    }

    #[test]
    fn the_namespace_defaults_when_left_out_or_empty() {
        assert_eq!(parts("stone"), ("minecraft".into(), "stone".into()));
        assert_eq!(parts(":stone"), ("minecraft".into(), "stone".into()));
    }

    #[test]
    fn it_prints_with_its_namespace_and_compares_by_both_parts() {
        assert_eq!(Identifier::new("stone").to_string(), "minecraft:stone");
        assert_eq!(Identifier::new("mod:stone").to_string(), "mod:stone");
        assert_eq!(Identifier::new("stone"), Identifier::new("minecraft:stone"));
        assert_ne!(Identifier::new("stone"), Identifier::new("mod:stone"));
    }
}
