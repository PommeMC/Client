use std::sync::Arc;

use azalea_protocol::packets::game::c_commands::{
    BrigadierNodeStub, BrigadierParser, ClientboundCommands, NodeType,
};
use parking_lot::Mutex;

/// Shared handle to the server's command tree. The network read task writes it
/// when a `ClientboundCommands` packet arrives; the chat-send task reads it to
/// decide whether a command needs to be signed.
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

    /// Whether parsing `command` walks through an argument whose parser is
    /// `Message` — the only signable argument type. Mirrors vanilla
    /// `SignableCommand.of`: such commands must be sent signed once a chat
    /// session exists. Returns `false` on any parse miss, leaving validation to
    /// the server (matching vanilla, which sends unsigned when unsure).
    pub fn has_signable_args(&self, command: &str) -> bool {
        let mut current = self.root_index;
        for token in command.split_whitespace() {
            let Some(node) = self.node(current) else {
                return false;
            };
            let child_ids = self.effective_children(node);
            let Some(cid) = self.descend(child_ids, token) else {
                return false;
            };
            if matches!(
                self.node(cid).map(|c| &c.node_type),
                Some(NodeType::Argument {
                    parser: BrigadierParser::Message,
                    ..
                })
            ) {
                return true;
            }
            current = cid;
        }
        false
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

    #[test]
    fn time_set_day_is_unsigned() {
        // root -> "time" -> "set" -> "day"
        let t = tree(vec![
            root(vec![1]),
            literal("time", vec![2], false),
            literal("set", vec![3], false),
            literal("day", vec![], true),
        ]);
        assert_eq!(t.root_child_names(), vec!["time".to_string()]);
        assert!(!t.has_signable_args("time set day"));
    }

    #[test]
    fn message_argument_is_signable() {
        // root -> "msg" -> <target:GameProfile> -> <message:Message>
        let t = tree(vec![
            root(vec![1]),
            literal("msg", vec![2], false),
            argument("target", BrigadierParser::GameProfile, vec![3], false),
            argument("message", BrigadierParser::Message, vec![], true),
        ]);
        assert!(t.has_signable_args("msg Steve hello there"));
        // The message token has not been supplied yet, so nothing to sign.
        assert!(!t.has_signable_args("msg Steve"));
        assert!(!t.has_signable_args("msg"));
    }

    #[test]
    fn unknown_command_is_not_signable() {
        let t = tree(vec![root(vec![1]), literal("time", vec![], true)]);
        assert!(!t.has_signable_args("nonexistent foo"));
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
