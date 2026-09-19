use std::sync::Arc;

use azalea_protocol::packets::game::c_commands::{
    BrigadierNodeStub, BrigadierParser, BrigadierString, ClientboundCommands, NodeType,
};
use parking_lot::Mutex;

/// Shared handle to the server's command tree. The network loop writes it when
/// a `ClientboundCommands` packet arrives and reads it to sign commands.
pub type SharedCommandTree = Arc<Mutex<Option<Arc<CommandTree>>>>;

/// The server's Brigadier command tree as a flat node list plus the root index.
/// Mirrors how the vanilla client keeps a `CommandDispatcher` built from
/// `ClientboundCommandsPacket`; used here to pick signed vs unsigned command
/// packets (and, in future, to drive tab-completion).
pub struct CommandTree {
    nodes: Vec<BrigadierNodeStub>,
    root_index: u32,
}

impl CommandTree {
    pub fn from_packet(packet: &ClientboundCommands) -> Self {
        Self {
            nodes: packet.entries.clone(),
            root_index: packet.root_index,
        }
    }

    fn node(&self, index: u32) -> Option<&BrigadierNodeStub> {
        self.nodes.get(index as usize)
    }

    /// The children to consider when descending from `node`: its own, or the
    /// redirect target's when it has none (e.g. `execute run ...`).
    fn effective_children<'a>(&'a self, node: &'a BrigadierNodeStub) -> &'a [u32] {
        if node.children.is_empty()
            && let Some(target) = node.redirect_node.and_then(|r| self.node(r))
        {
            return &target.children;
        }
        &node.children
    }

    fn is_argument(&self, index: u32) -> bool {
        matches!(
            self.node(index).map(|c| &c.node_type),
            Some(NodeType::Argument { .. })
        )
    }

    /// Follow one command token: a literal child whose name equals `token`,
    /// else the (single) argument child that would consume it.
    fn descend(&self, child_ids: &[u32], token: &str) -> Option<u32> {
        let literal = child_ids.iter().copied().find(|&cid| {
            matches!(
                self.node(cid).map(|c| &c.node_type),
                Some(NodeType::Literal { name }) if name.as_str() == token
            )
        });
        literal.or_else(|| child_ids.iter().copied().find(|&cid| self.is_argument(cid)))
    }

    /// The direct child literals of the root node: the top-level commands the
    /// server offers this player. Logged as a diagnostic, since an op-only
    /// command like `time` is absent from the tree of an unprivileged player.
    pub fn root_child_names(&self) -> Vec<String> {
        self.node(self.root_index)
            .map(|root| {
                root.children
                    .iter()
                    .filter_map(|&i| self.node(i).and_then(BrigadierNodeStub::name))
                    .map(str::to_string)
                    .collect()
            })
            .unwrap_or_default()
    }

    /// The raw values of the command's signable arguments, as vanilla
    /// `SignableCommand.of` collects them: 26.2's only `SignedArgument` is
    /// `MessageArgument`, whose range runs to the end of the command.
    pub fn signable_arguments(&self, command: &str) -> Vec<(String, String)> {
        self.parse_nodes(self.root_index, command, 0)
            .arguments
            .into_iter()
            .filter(|(_, _, signed)| *signed)
            .map(|(name, value, _)| (name, value))
            .collect()
    }

    /// Brigadier's `CommandDispatcher.parseNodes`, skipping arguments by
    /// StringReader rules instead of parsing them. Arguments past a redirect
    /// to the root aren't collected (`visitArguments`' `rejectRootRedirects`).
    fn parse_nodes(&self, node: u32, input: &str, cursor: usize) -> CommandParse {
        let Some(node) = self.node(node) else {
            return CommandParse::stopped(input, cursor, false);
        };
        let word_end = input[cursor..]
            .find(' ')
            .map_or(input.len(), |i| cursor + i);
        let literal = node.children.iter().copied().find(|&child| {
            matches!(
                self.node(child).map(|c| &c.node_type),
                Some(NodeType::Literal { name }) if name.as_str() == &input[cursor..word_end]
            )
        });
        let candidates: Vec<u32> = match literal {
            Some(literal) => vec![literal],
            None => node
                .children
                .iter()
                .copied()
                .filter(|&child| self.is_argument(child))
                .collect(),
        };

        let mut potentials = Vec::new();
        let mut failed = false;
        for child in candidates {
            let Some(child_node) = self.node(child) else {
                continue;
            };
            let (end, argument) = match &child_node.node_type {
                NodeType::Argument { name, parser, .. } => {
                    let Some(end) = argument_end(parser, input, cursor) else {
                        failed = true;
                        continue;
                    };
                    let signed = matches!(parser, BrigadierParser::Message);
                    (
                        end,
                        Some((name.clone(), input[cursor..end].to_owned(), signed)),
                    )
                }
                _ => (word_end, None),
            };
            if end < input.len() && input.as_bytes()[end] != b' ' {
                failed = true;
                continue;
            }
            let redirect = child_node.redirect_node;
            let needed = if redirect.is_some() { 1 } else { 2 };
            let mut parse = if end + needed <= input.len() {
                let mut rest = self.parse_nodes(redirect.unwrap_or(child), input, end + 1);
                if redirect == Some(self.root_index) {
                    rest.arguments.clear();
                }
                rest
            } else {
                CommandParse::stopped(input, end, false)
            };
            parse.arguments.splice(0..0, argument);
            if redirect.is_some() {
                return parse;
            }
            potentials.push(parse);
        }
        potentials.sort_by_key(|parse| (!parse.complete, parse.errors));
        potentials
            .into_iter()
            .next()
            .unwrap_or_else(|| CommandParse::stopped(input, cursor, failed))
    }

    /// Local completions for `command` (the chat input with the leading `/`
    /// removed): literal child names reachable after the completed tokens,
    /// filtered by the partial last token. Mirrors the local half of vanilla
    /// `CommandSuggestions`.
    pub fn suggestions(&self, command: &str) -> Suggestions {
        let tokens: Vec<&str> = command.split_whitespace().collect();
        let (completed, partial): (&[&str], &str) = if command.ends_with(char::is_whitespace) {
            (tokens.as_slice(), "")
        } else {
            match tokens.split_last() {
                Some((last, rest)) => (rest, last),
                None => (&[], ""),
            }
        };

        let mut current = self.root_index;
        for &token in completed {
            let Some(node) = self.node(current) else {
                return Suggestions::empty();
            };
            let child_ids = self.effective_children(node);
            match self.descend(child_ids, token) {
                Some(cid) => current = cid,
                None => return Suggestions::empty(),
            }
        }

        let Some(node) = self.node(current) else {
            return Suggestions::empty();
        };
        let lower = partial.to_ascii_lowercase();
        let child_ids = self.effective_children(node);
        let mut options: Vec<String> = child_ids
            .iter()
            .filter_map(|&cid| match self.node(cid).map(|c| &c.node_type) {
                Some(NodeType::Literal { name })
                    if name.to_ascii_lowercase().starts_with(&lower) =>
                {
                    Some(name.clone())
                }
                _ => None,
            })
            .collect();
        options.sort_by_key(|a| a.to_ascii_lowercase());
        let needs_server = child_ids.iter().any(|&cid| self.is_argument(cid));
        Suggestions {
            options,
            partial_len: partial.len(),
            needs_server,
        }
    }
}

/// One `parse_nodes` outcome: `(name, value, signed)` per parsed argument.
struct CommandParse {
    arguments: Vec<(String, String, bool)>,
    /// Brigadier's reader reached the end of the input.
    complete: bool,
    /// Some child failed to parse where the walk stopped.
    errors: bool,
}

impl CommandParse {
    fn stopped(input: &str, cursor: usize, errors: bool) -> Self {
        Self {
            arguments: Vec::new(),
            complete: cursor == input.len(),
            errors,
        }
    }
}

/// Where an argument starting at `start` ends. Pomme has no argument parsers,
/// so each word is skipped like StringReader would read it: quoted strings
/// and bracketed selector, NBT and block-state parts are opaque, and
/// coordinates take several words.
/// TODO: port the argument parsers whose syntax this can't bound, so a
/// command that only parses as a later sibling still finds its message.
fn argument_end(parser: &BrigadierParser, input: &str, start: usize) -> Option<usize> {
    let words = match parser {
        BrigadierParser::Message | BrigadierParser::String(BrigadierString::GreedyPhrase) => {
            return Some(input.len());
        }
        BrigadierParser::Vec3 | BrigadierParser::BlockPos => 3,
        BrigadierParser::Vec2 | BrigadierParser::ColumnPos | BrigadierParser::Rotation => 2,
        _ => 1,
    };
    let mut end = start;
    for word in 0..words {
        if word > 0 {
            end += input[end..].starts_with(' ').then_some(1)?;
        }
        end = word_end(input.as_bytes(), end)?;
    }
    Some(end)
}

fn word_end(bytes: &[u8], start: usize) -> Option<usize> {
    let mut depth = 0usize;
    let mut i = start;
    while i < bytes.len() {
        match bytes[i] {
            quote @ (b'"' | b'\'') => {
                i += 1;
                loop {
                    match *bytes.get(i)? {
                        b'\\' => i += 2,
                        byte if byte == quote => break,
                        _ => i += 1,
                    }
                }
            }
            b'[' | b'{' => depth += 1,
            b']' | b'}' => depth = depth.checked_sub(1)?,
            b' ' if depth == 0 => break,
            _ => {}
        }
        i += 1;
    }
    (depth == 0 && i > start).then_some(i.min(bytes.len()))
}

/// Local command completions: the matching literal names plus how many bytes of
/// the current partial token they replace.
pub struct Suggestions {
    pub options: Vec<String>,
    pub partial_len: usize,
    /// The token being completed could also be an argument, so the server
    /// should be asked for completions (player names, enum values, ...).
    /// Pomme has no client-side argument parsers, so unlike vanilla it defers
    /// every argument to the server, not just `ask_server` ones.
    pub needs_server: bool,
}

impl Suggestions {
    fn empty() -> Self {
        Self {
            options: Vec::new(),
            partial_len: 0,
            needs_server: false,
        }
    }
}

#[cfg(test)]
mod tests {
    use azalea_protocol::packets::game::c_commands::EntityParser;

    use super::*;

    fn root(children: Vec<u32>) -> BrigadierNodeStub {
        BrigadierNodeStub {
            is_executable: false,
            children,
            redirect_node: None,
            node_type: NodeType::Root,
            is_restricted: false,
        }
    }

    fn literal(name: &str, children: Vec<u32>, executable: bool) -> BrigadierNodeStub {
        BrigadierNodeStub {
            is_executable: executable,
            children,
            redirect_node: None,
            node_type: NodeType::Literal {
                name: name.to_string(),
            },
            is_restricted: false,
        }
    }

    fn argument(
        name: &str,
        parser: BrigadierParser,
        children: Vec<u32>,
        executable: bool,
    ) -> BrigadierNodeStub {
        BrigadierNodeStub {
            is_executable: executable,
            children,
            redirect_node: None,
            node_type: NodeType::Argument {
                name: name.to_string(),
                parser,
                suggestions_type: None,
            },
            is_restricted: false,
        }
    }

    fn tree(nodes: Vec<BrigadierNodeStub>) -> CommandTree {
        CommandTree {
            nodes,
            root_index: 0,
        }
    }

    fn redirect(name: &str, target: u32) -> BrigadierNodeStub {
        BrigadierNodeStub {
            redirect_node: Some(target),
            ..literal(name, vec![], false)
        }
    }

    fn signed(t: &CommandTree, command: &str) -> Vec<(String, String)> {
        t.signable_arguments(command)
    }

    fn message(value: &str) -> Vec<(String, String)> {
        vec![("message".to_owned(), value.to_owned())]
    }

    /// root -> msg <targets> <message>, say <message>,
    /// execute {as <targets> -> execute, run -> root}, tp <pos> <message>
    fn vanilla_like() -> CommandTree {
        tree(vec![
            root(vec![1, 4, 5, 9]),
            literal("msg", vec![2], false),
            argument(
                "targets",
                BrigadierParser::Entity(EntityParser {
                    single: false,
                    players_only: true,
                }),
                vec![3],
                false,
            ),
            argument("message", BrigadierParser::Message, vec![], true),
            literal("say", vec![3], false),
            literal("execute", vec![6, 8], false),
            literal("as", vec![7], false),
            BrigadierNodeStub {
                redirect_node: Some(5),
                ..argument(
                    "targets",
                    BrigadierParser::Entity(EntityParser {
                        single: false,
                        players_only: false,
                    }),
                    vec![],
                    false,
                )
            },
            redirect("run", 0),
            literal("tp", vec![10], false),
            argument("pos", BrigadierParser::Vec3, vec![3], true),
        ])
    }

    #[test]
    fn literal_only_commands_are_unsigned() {
        let t = tree(vec![
            root(vec![1]),
            literal("time", vec![2], false),
            literal("set", vec![3], false),
            literal("day", vec![], true),
        ]);
        assert_eq!(t.root_child_names(), vec!["time".to_string()]);
        assert!(signed(&t, "time set day").is_empty());
        assert!(signed(&t, "nonexistent foo").is_empty());
    }

    #[test]
    fn message_takes_the_rest_of_the_command() {
        let t = vanilla_like();
        assert_eq!(
            signed(&t, "msg Steve hello  there"),
            message("hello  there")
        );
        assert_eq!(signed(&t, "say hi"), message("hi"));
        assert!(signed(&t, "msg Steve").is_empty());
        assert!(signed(&t, "msg Steve ").is_empty());
        assert_eq!(signed(&t, "tp ~ ~1 ~ back soon"), message("back soon"));
    }

    #[test]
    fn selectors_and_quotes_are_skipped_whole() {
        let t = vanilla_like();
        assert_eq!(
            signed(&t, "msg @a[name=\"a ] b\", tag=x] hi"),
            message("hi")
        );
        assert_eq!(signed(&t, "msg @a[tag=x hi"), Vec::new());
    }

    #[test]
    fn root_redirects_stop_collecting_but_other_redirects_continue() {
        let t = vanilla_like();
        assert!(signed(&t, "execute run say hi").is_empty());
        assert!(signed(&t, "execute as @a run say hi").is_empty());
        let t = tree(vec![
            root(vec![1, 3]),
            literal("tell", vec![2], false),
            argument("message", BrigadierParser::Message, vec![], true),
            literal("w", vec![4], false),
            redirect("whisper", 1),
        ]);
        assert_eq!(signed(&t, "w whisper hi"), message("hi"));
    }

    #[test]
    fn suggestions_list_subcommand_literals() {
        // root -> "time" -> "set" -> {day, night, noon, midnight, <amount>}
        let t = tree(vec![
            root(vec![1]),
            literal("time", vec![2], false),
            literal("set", vec![3, 4, 5, 6, 7], false),
            literal("day", vec![], true),
            literal("night", vec![], true),
            literal("noon", vec![], true),
            literal("midnight", vec![], true),
            argument("amount", BrigadierParser::Bool, vec![], true),
        ]);

        let all = t.suggestions("time set ");
        assert_eq!(all.options, vec!["day", "midnight", "night", "noon"]);
        // <amount> is an argument sibling: the server should be asked too.
        assert!(all.needs_server);

        let d = t.suggestions("time set d");
        assert_eq!(d.options, vec!["day"]);
        assert_eq!(d.partial_len, 1);
        assert!(d.needs_server);

        let se = t.suggestions("time se");
        assert_eq!(se.options, vec!["set"]);
        assert_eq!(se.partial_len, 2);
        assert!(!se.needs_server);

        let bogus = t.suggestions("bogus foo");
        assert!(bogus.options.is_empty());
        assert!(!bogus.needs_server);
    }

    #[test]
    fn suggestions_argument_only_position_asks_server() {
        // root -> "gamemode" -> <gamemode>
        let t = tree(vec![
            root(vec![1]),
            literal("gamemode", vec![2], false),
            argument("gamemode", BrigadierParser::Bool, vec![], true),
        ]);

        let sug = t.suggestions("gamemode ");
        assert!(sug.options.is_empty());
        assert!(sug.needs_server);

        let sug = t.suggestions("gamemode c");
        assert!(sug.options.is_empty());
        assert_eq!(sug.partial_len, 1);
        assert!(sug.needs_server);

        assert!(!t.suggestions("gam").needs_server);
    }
}
