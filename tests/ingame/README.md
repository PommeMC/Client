# In-game tests

Scripted tests that run a real client against a local vanilla 26.2 server, for
behaviour unit tests can't reach: items arriving with their components,
inventory clicks, entity data, advancements, and what ends up on screen.

Unit tests check pomme's pieces in isolation, such as a decoder against bytes
built by hand. They can't show that a real server's packets end up as the right
item in the right slot, drawn correctly. That used to need someone to check in
game. These scripts do it against vanilla's own server, so the whole path is
tested, and the screenshots catch what the checks miss.

A full run takes under two minutes, server start included, and needs no one
at the keyboard, so it's cheap to rerun on every change.

```sh
just ingame-test                          # every script in this directory
just ingame-test inventory_armor_equip    # one script
```

The runner (`tools/ingame-test`):

1. Downloads the vanilla 26.2 server jar into `target/ingame/server/` once,
   checking its sha1.
2. Starts it on a fresh flat world in offline mode, with the test player
   `Tester` opped. This needs Java 25 (`JAVA_HOME` or `java` on `PATH`).
3. Runs a debug client per script, then resets the player for the next one.

Each script writes `report.json` and its screenshots to
`target/ingame/<script>/`. The run fails if any script does.

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
| A menu slot | `slot`, `item`, optional `count` and `components` (component types present) |
| A living entity | `entity` (kind), optional `variant` (asset name, e.g. `cold`) |
| A dropped item | `item_entity`, optional `count` |
| An advancement | `advancement` (id) |
| A chat line | `chat` (text it contains) |

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
`click` (`slot`, `op`), `inventory`, `entities`, `advancements`, `chat_log`
and `screenshot` (`path`).
