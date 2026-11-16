# pomme-item

Item stacks as the client holds them: an item, a count and a component patch.

A patch keeps each component it sets as the exact bytes its `STREAM_CODEC`
wrote, so every component survives the trip back to the wire whether or not
anything reads it. `components` holds the values pomme reads out of those
bytes, each ported from the game's codec.

Most component values, each item's defaults and the component hashes still
come from the `pomme-inventory` fork, through
`pomme-client/src/net/item_codec.rs`. They move here as they are ported.
