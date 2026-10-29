use regex::Regex;

use crate::resources::ResourceId;

impl ResourceFilter {
    /// Rust regex is not fully compatible with Java Pattern: lookaround and
    /// backreferences are unsupported; other syntax and semantics may differ.
    /// Compilation failure rejects the entire section, never
    /// leaving partially compiled rules. Matching searches, without anchoring.
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
    fn namespace_blocked(&self, namespace: &str) -> bool {
        self.filters.iter().any(|rule| {
            rule.namespace
                .as_ref()
                .is_none_or(|pattern| pattern.is_match(namespace))
        })
    }

    fn path_blocked(&self, path: &str) -> bool {
        self.filters.iter().any(|rule| {
            rule.path
                .as_ref()
                .is_none_or(|pattern| pattern.is_match(path))
        })
    }

    pub(in crate::resources) fn blocks(&self, id: &ResourceId) -> bool {
        // Vanilla 26.3 ResourceFilterSection + MultiPackResourceManager:
        // ANY namespace AND ANY path across the filters, NOT paired rules.
        self.namespace_blocked(id.namespace()) && self.path_blocked(id.path())
    }
}
