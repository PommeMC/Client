use std::io;

use regex::Regex;
use serde::Deserialize;
use serde_json::Value;

impl ResourceFilter {
    /// Compilation failure rejects the entire section, never leaving partially
    /// compiled rules. Matching searches, without anchoring, as
    /// `Pattern.asPredicate` does.
    // TODO: Java `Pattern` syntax, not Rust `regex`: a pattern using lookaround
    // or backreferences fails to compile, so that pack filters nothing.
    pub(super) fn compile(
        patterns: Vec<(Option<String>, Option<String>)>,
    ) -> Result<Self, regex::Error> {
        let filters = patterns
            .into_iter()
            .map(|(namespace, path)| {
                Ok(Filter {
                    namespace: namespace.map(|value| Regex::new(&value)).transpose()?,
                    path: path.map(|value| Regex::new(&value)).transpose()?,
                })
            })
            .collect::<Result<_, _>>()?;
        Ok(ResourceFilter { filters })
    }
}

#[derive(Default)]
pub(in crate::resources) struct ResourceFilter {
    filters: Vec<Filter>,
}

struct Filter {
    namespace: Option<Regex>,
    path: Option<Regex>,
}

impl ResourceFilter {
    pub(in crate::resources) fn namespace_blocked(&self, namespace: &str) -> bool {
        self.filters.iter().any(|rule| {
            rule.namespace
                .as_ref()
                .is_none_or(|pattern| pattern.is_match(namespace))
        })
    }

    pub(in crate::resources) fn path_blocked(&self, path: &str) -> bool {
        self.filters.iter().any(|rule| {
            rule.path
                .as_ref()
                .is_none_or(|pattern| pattern.is_match(path))
        })
    }
}

/// A present filter section requires `block`; unknown fields are ignored.
#[derive(Deserialize)]
struct PackFilterSpec {
    block: Vec<FilterSpec>,
}

#[derive(Deserialize)]
struct FilterSpec {
    // Omission and JSON null are wildcards; other values must be strings.
    namespace: Option<String>,
    path: Option<String>,
}

pub(in crate::resources) fn parse_filter(section: Option<&Value>) -> io::Result<ResourceFilter> {
    let Some(section) = section else {
        return Ok(ResourceFilter::default());
    };
    let spec: PackFilterSpec = serde_json::from_value(section.clone())?;
    ResourceFilter::compile(
        spec.block
            .into_iter()
            .map(|rule| (rule.namespace, rule.path))
            .collect(),
    )
    .map_err(|error| io::Error::new(io::ErrorKind::InvalidData, error))
}
