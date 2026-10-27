# In-game tests

Scripted tests that run a real client against a local vanilla server (26.2,
the native version, unless you pick others), for
behaviour unit tests can't reach: items arriving with their components,
inventory clicks, entity data, advancements, and what ends up on screen.

Unit tests check pomme's pieces in isolation, such as a decoder against bytes
built by hand. They can't show that a real server's packets end up as the right
item in the right slot, drawn correctly. That used to need someone to check in
game. These scripts do it against vanilla's own server, so the whole path is
tested, and the screenshots catch what the checks miss.

A full run takes about half a minute, server start included, and needs no one
at the keyboard, so it's cheap to rerun on every change.

```sh
just ingame-test                          # every script
just ingame-test entities                 # every script in a folder
just ingame-test player/armor_equip       # one script
just ingame-test --jobs 2                 # two at once (default 6)
just ingame-test --version 1.21.1         # against another version (repeatable)
just ingame-test --versions all world     # every tested version, one folder
```

`--versions all` is one version per joinable protocol, its newest patch
(`pomme_protocol::version::tested_versions`, 26.3 down to 1.20.1). A full
sweep takes a while: each version downloads its server jar once, and the client
installs that version's assets on its first join. Not every version passes yet:
versions below 1.21.5 don't load chunks, and no script is gated by version so
far, so a sweep shows what's still to fix rather than a green run.

Scripts live in a folder per area (`blocks`, `entities`, `items`, `player`,
`ui`, `world`); a new one goes in the folder it tests. A script's name is its
path without `.toml`.

The runner (`tools/ingame-test`):

1. Looks each version up in Mojang's version manifest and downloads its
   server jar into `target/ingame/servers/<version>/` once, checking its sha1.
2. Starts it on a fresh flat world in offline mode, with the test player
   `Tester` opped. Each version needs the Java its `version.json` names
   (17 for 1.20.x, 21 for 1.21.x, 25 for 26.x): `JAVA_HOME_<major>` if set,
   else `JAVA_HOME`, else `java` on `PATH`. A newer Java runs older servers.
3. Runs a debug client per script, joining as that version, then resets the
   player for the next one, and the world to noon in clear weather with time
   ticking.

With `--jobs N`, N workers per version each start their own server and world
under `target/ingame/workers/<version>/<n>/` and take scripts from a shared
queue. Their clients use a game dir there too, so a run never touches your own
options.

Each script writes `report.json`, its screenshots and the client's output
(`client.log`) to `target/ingame/<version>/<script>/`, e.g.
`target/ingame/26.2/entities/data/`. The run fails if any script does.

A script whose commands don't exist on older servers (item component syntax
like `stone[custom_name=...]` is 1.20.5+) declares the versions it's valid on,
above its first step, with a comment saying why:

```toml
# Item components in commands arrived in 1.20.5.
min_version = "1.20.5"
```

`max_version` works the same way. Scripts outside the range print `SKIP` for
that version. Gate only for what the server can't do; a script failing because
of a client bug stays a failure.

## Wire fixtures

`just wire-fixtures <version>...` runs `world/join` on each version with
`--capture`: the client records every frame the server sends, before
translation, and the runner keeps the login and config frames plus the first
few of each game packet in `pomme-client/src/net/wire-fixtures/<version>/`.
`net::wire_corpus` replays them through the translation and the client's
readers in `cargo test`, so each version is checked against what its real
server sends. Recapture a version when its join traffic changes, such as after
a protogen or reference update for it. Packets a version is known to misread
are listed in `KNOWN_GAPS` there; once one reads, the test asks for its entry
to go.

## The harness

The client side is the `test-harness` cargo feature: debug tooling that's off
by default and never in release builds. It adds two launch flags:

- `--harness-script <file>` runs a script, then exits 0 on pass and 1 on fail.
- `--harness-port <port>` serves the same commands on `127.0.0.1` only, for
  driving a client by hand.

Commands go through the same paths as the UI. Chat goes through the chat
sender, and clicks go through the inventory's click prediction and packets, so
a test exercises the real code.

## Writing a script

A script is a list of `[[step]]` tables, run in order once the client is in
game. Each step has exactly one action:

```toml
[[step]]
chat = "/give @s minecraft:iron_chestplate"   # chat, or a command with `/`

[[step]]
open_inventory = true                         # or close_menu = true

[[step]]
click = { slot = 36, op = "quick_move" }      # pickup_left, pickup_right,
                                              # quick_move, { swap = { hotbar = 0 } }
[[step]]
creative_set_slot = { slot = 37, from = 36 }  # creative mode: copy a stack, as
                                              # the creative screen places one
[[step]]
look_at = [0.5, -58.5, 4.0]                   # turn to face a point

[[step]]
mouse_down = "left"                           # or mouse_up; left, right, middle

[[step]]
hover = { slot = 36 }                         # creative item tab: hotbar slot
                                              # 36-44, or { catalog = 0 };
                                              # offset = [x, y] GUI px from
                                              # the cell's centre
[[step]]
cursor = [2, 2]                               # cursor to a point, GUI px

[[step]]
scroll = -2                                   # mouse wheel over the open
                                              # screen, lines (up is positive)
[[step]]
text = "stone"                                # types into the focused field

[[step]]
press = { action = "key.drop", ctrl = true }  # a key binding's default key, or
                                              # key = "key.keyboard.j" for a
                                              # physical key; ctrl is optional
[[step]]
bind = { action = "key.drop", keys = ["key.keyboard.j"] }  # this session only;
                                              # no keys resets the defaults
[[step]]
option = { main_hand = "left" }               # sets client options, as the
                                              # options screen does

[[step]]
wait_ms = 1000

[[step]]
screenshot = "equipped"                       # target/ingame/<script>/equipped.png

[[step]]
expect = { slot = 6, item = "iron_chestplate" }
timeout_ms = 5000                             # how long to retry (default 5 s)
```

An `expect` is retried every frame until it holds or times out. It can check:

| Check | Fields |
|---|---|
| A menu slot | `slot`, `item` (`air` for empty), optional `count`, `components` (types present) and `absent` (types not present) |
| A living entity | `entity` (kind), optional `variant` (asset name, e.g. `cold`), `name`, `baby`, `glowing`, `villager_type` and `profession` |
| A dropped item | `item_entity`, optional `count` |
| An advancement | `advancement` (id) |
| A chat line | `chat` (text it contains) |
| A block | `block` (state name, e.g. `air`), `at`, optional `block_entity` (type) |
| The local player | `player = { ... }` with any of `health`, `food`, `level`, `game_mode`, `effect`, `no_effect`, `max_health`, `raining`, `hand`, `dimension`, `day_time`, `position` (within 0.01), `y_rot` (within 0.01), `spectating` (the kind of entity the camera follows, empty for none) and `post_effect` (the spectator view effect drawn: `creeper`, `spider`, `invert`, or empty) |
| The open menu | `menu` (a container's menu type, e.g. `generic_9x3`, or `inventory`, `creative`, `pause`, `none`), optional `title`; on the creative screen also `tab` (its vanilla id, e.g. `building_blocks`), `row` (the first catalog row shown) and `search` |
| The HUD text | `hud = { ... }` with any of `title`, `subtitle` and `sidebar_title` (`""` when none is shown), `boss_bars` (every name, top to bottom), `sidebar` (every row as `"name number"`, top to bottom) and `tab_list` (every listed player as `{ name, game_mode? }`, in tab order) |
| A played sound | `sound` (event name, e.g. `block.note_block.harp`), among the last 64 played |

Slots are menu slots. In the player inventory, 5-8 are armor (head to feet),
9-35 the main inventory, 36-44 the hotbar and 45 the offhand. `/give` fills
the first free hotbar slot, 36.

## Driving a client live

```sh
just ingame-server    # prints the port
just client-dev --features test-harness -- --version 26.2 --username Tester \
    --quick-access-multiplayer 127.0.0.1:<server port> --harness-port 25599
```

Then send one JSON command per line, and read one reply per line:

```sh
echo '{"cmd":"inventory"}' | nc 127.0.0.1 25599
# {"ok":true,"data":{"menu":"none","slots":[...],"cursor":null}}
```

The commands are `status`, `chat` (`text`), `open_inventory`, `close_menu`,
`click` (`slot`, `op`), `creative_set_slot` (`slot`, `from`), `inventory`,
`entities`, `advancements`, `player`, `hud`, `sounds`, `block` (`at`), `look_at` (`at`),
`mouse_down` and `mouse_up` (`button`), `press` (`action`, a key binding id like `key.drop`, or `key`, an input id like `key.keyboard.j`; optional `ctrl`), `bind` (`action`, optional `keys`), `option` (`main_hand`: `left` or `right`; saved as closing its options screen would, so the server is told), `hover` (`slot` 36-44 or `catalog` cell, in a creative item tab; optional `offset`), `cursor` (`at`, GUI px), `scroll` (`y`, wheel lines), `text` (`text`), `chat_log` and `screenshot` (`path`).
