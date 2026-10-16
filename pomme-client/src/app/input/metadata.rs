use std::borrow::Cow;

/// A language-table lookup key, not an action or physical-input identifier.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct StringId<'a>(&'a str);

impl<'a> StringId<'a> {
    pub const fn new(value: &'a str) -> Self {
        Self(value)
    }

    pub const fn as_str(self) -> &'a str {
        self.0
    }
}

/// Stable identity of a catalog action, used for persistence and lookup.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct ActionId(&'static str);

impl ActionId {
    pub const fn new(value: &'static str) -> Self {
        Self(value)
    }

    pub const fn as_str(self) -> &'static str {
        self.0
    }
}

/// Display text explicitly separates translation identity from its fallback.
#[derive(Clone, Debug)]
pub struct DisplayText<'a> {
    pub translation_id: Option<StringId<'a>>,
    pub fallback: Cow<'a, str>,
}

/// Vanilla `InputConstants.UNKNOWN`'s label.
pub const NOT_BOUND: DisplayText<'static> = DisplayText {
    translation_id: Some(StringId::new("key.keyboard.unknown")),
    fallback: Cow::Borrowed("Not Bound"),
};

impl<'a> DisplayText<'a> {
    pub fn resolve(self) -> Cow<'a, str> {
        if let Some(id) = self.translation_id
            && let Some(text) = crate::lang::translate(id.as_str())
        {
            return Cow::Borrowed(text);
        }
        self.fallback
    }
}

/// A serialized input token and its independent human-facing text metadata.
/// This is used by the bindings macros to construct the mapping functions
#[derive(Clone, Debug)]
pub struct BindingMetadata<'a> {
    pub serialized_id: Cow<'a, str>,
    pub display: DisplayText<'a>,
}
