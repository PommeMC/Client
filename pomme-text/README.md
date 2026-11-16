# pomme-text

The game's text components: the model (`Component`, `Content`, `Style`,
click and hover events), reading one from NBT or JSON, and the traversal that
resolves it to styled runs of text.

What a component's text depends on outside itself comes in through
`Resolver`: the language's translations and the names of bound keys. The
client supplies both (`pomme-client/src/chat_component.rs`).
