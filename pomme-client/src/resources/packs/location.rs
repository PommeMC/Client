use std::rc::Rc;

use pomme_protocol::KnownPack;

use crate::chat_component::{Argument, Component, HoverEvent};

/// Vanilla's textual pack identity, distinct from the existing opaque PackId.
#[derive(Clone)]
pub(crate) struct PackLocationInfo {
    pub(crate) id: String,
    pub(crate) title: Component,
    pub(crate) source: PackSource,
    pub(crate) known_pack_info: Option<KnownPack>,
}

impl PackLocationInfo {
    pub(crate) fn create_chat_link(&self, enabled: bool, description: &Component) -> Component {
        let decorated = self.source.decorate(Component::text(self.id.clone()));
        let mut link = Component::translate(
            "chat.square_brackets",
            vec![Argument::Component(Box::new(decorated))],
        );
        let mut hover = Component::text("");
        hover.siblings = vec![
            self.title.clone(),
            Component::text("\n"),
            description.clone(),
        ];
        link.style.color = Some(if enabled { 0x55ff55 } else { 0xff5555 });
        link.style.insertion = Some(escape_pack_id_for_insertion(&self.id));
        link.style.hover_event = Some(HoverEvent::Text(Box::new(hover)));
        link
    }
}

/// Brigadier StringArgumentType.escapeIfRequired, not JSON string escaping.
fn escape_pack_id_for_insertion(id: &str) -> String {
    if id
        .bytes()
        .all(|byte| byte.is_ascii_alphanumeric() || b"_.+-".contains(&byte))
    {
        return id.to_owned();
    }
    let mut escaped = String::with_capacity(id.len() + 2);
    escaped.push('"');
    for character in id.chars() {
        if matches!(character, '\\' | '"') {
            escaped.push('\\');
        }
        escaped.push(character);
    }
    escaped.push('"');
    escaped
}

/// Rich-text decoration and automatic-selection policy. Factories replace
/// Vanilla's shared constants because Rc decorators are application-thread
/// local.
#[derive(Clone)]
pub(crate) struct PackSource {
    decorator: Rc<dyn Fn(Component) -> Component>,
    add_automatically: bool,
}

impl PackSource {
    pub(crate) fn new(
        decorator: impl Fn(Component) -> Component + 'static,
        add_automatically: bool,
    ) -> Self {
        Self {
            decorator: Rc::new(decorator),
            add_automatically,
        }
    }

    pub(crate) fn decorate(&self, description: Component) -> Component {
        (self.decorator)(description)
    }

    pub(crate) fn should_add_automatically(&self) -> bool {
        self.add_automatically
    }

    pub(crate) fn built_in() -> Self {
        Self::with_source("pack.source.builtin", true)
    }

    pub(crate) fn feature() -> Self {
        Self::with_source("pack.source.feature", false)
    }

    pub(crate) fn world() -> Self {
        Self::with_source("pack.source.world", true)
    }

    pub(crate) fn server() -> Self {
        Self::with_source("pack.source.server", true)
    }

    fn with_source(key: &'static str, add_automatically: bool) -> Self {
        let source = Component::translate(key, Vec::new());
        Self::new(
            move |description| {
                let mut decorated = Component::translate(
                    "pack.nameAndSource",
                    vec![
                        Argument::Component(Box::new(description)),
                        Argument::Component(Box::new(source.clone())),
                    ],
                );
                decorated.style.color = Some(0xaaaaaa);
                decorated
            },
            add_automatically,
        )
    }
}

impl Default for PackSource {
    fn default() -> Self {
        Self::new(std::convert::identity, true)
    }
}
