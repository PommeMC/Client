# pomme-item

Item stacks as the client holds them: an item, a count and a component patch.

A patch keeps each component it sets as the exact bytes its `STREAM_CODEC`
wrote, so every component survives the trip back to the wire whether or not
anything reads it. `components` holds the values pomme reads out of those
bytes, each ported from the game's codec.

`defaults` holds every item's default components in the same form.
`src/data/default-components.json` is written by the game itself:
`tools/codec-oracle/DefaultComponents.java` runs on the reference server jar
(`just typegen <version>`), builds each item's components the way the game
does and encodes them with its own codecs.
