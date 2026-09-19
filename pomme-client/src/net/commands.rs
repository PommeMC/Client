use std::collections::HashSet;
use std::ops::Range;
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
/// `ClientboundCommandsPacket`; used for command signing, local parse/usage
/// feedback, and Vanilla-style chat completion/suggestion presentation.
pub struct CommandTree {
    nodes: Vec<BrigadierNodeStub>,
    root_index: u32,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum UnattendedCommandCheck {
    NoIssues,
    SignatureRequired,
    PermissionsRequired,
    ParseErrors,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CommandTokenKind {
    Literal,
    Argument(usize),
    Unparsed,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CommandTokenRange {
    pub range: Range<usize>,
    pub kind: CommandTokenKind,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct CommandPresentation {
    pub tokens: Vec<CommandTokenRange>,
    pub usage: Vec<String>,
    pub usage_start: usize,
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

    fn conservative_subtree_flags(&self, start: u32) -> (bool, bool) {
        let mut stack = vec![start];
        let mut visited = HashSet::new();
        let mut has_message = false;
        let mut has_restricted = false;
        while let Some(index) = stack.pop() {
            if !visited.insert(index) {
                continue;
            }
            let Some(node) = self.node(index) else {
                continue;
            };
            has_restricted |= node.is_restricted;
            if matches!(
                &node.node_type,
                NodeType::Argument {
                    parser: BrigadierParser::Message,
                    ..
                }
            ) {
                has_message = true;
            }
            stack.extend(node.children.iter().copied());
            if let Some(redirect) = node.redirect_node {
                stack.push(redirect);
            }
        }
        (has_message, has_restricted)
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

    /// Classify a server-provided click command before unattended execution.
    ///
    /// Direct execution is deliberately fail-closed: only an all-literal,
    /// executable, unrestricted path is considered parser-exact here. Network
    /// argument stubs do not contain enough client-side parser machinery for
    /// Pomme to reproduce Brigadier's best-branch/exceptions semantics safely.
    /// Any non-message argument therefore requires confirmation rather than
    /// risking a false-safe click. A message argument still gets the stronger
    /// signature-required confirmation, matching Vanilla's consent boundary.
    pub fn verify_unattended(&self, command: &str) -> UnattendedCommandCheck {
        let lexical = command_token_ranges(command);
        if lexical.is_empty() {
            return UnattendedCommandCheck::ParseErrors;
        }
        let mut current = self.root_index;
        let mut token_index = 0usize;

        while token_index < lexical.len() {
            let Some(node) = self.node(current) else {
                return UnattendedCommandCheck::ParseErrors;
            };
            if node.is_restricted {
                return UnattendedCommandCheck::PermissionsRequired;
            }
            if let Some(redirect) = node.redirect_node {
                let (has_message, has_restricted) = self.conservative_subtree_flags(redirect);
                if has_message {
                    return UnattendedCommandCheck::SignatureRequired;
                }
                if has_restricted {
                    return UnattendedCommandCheck::PermissionsRequired;
                }
                return UnattendedCommandCheck::ParseErrors;
            }
            let token = &command[lexical[token_index].clone()];
            let child_ids = &node.children;
            if let Some(cid) = child_ids.iter().copied().find(|&cid| {
                matches!(
                    self.node(cid).map(|child| &child.node_type),
                    Some(NodeType::Literal { name }) if name == token
                )
            }) {
                let Some(child) = self.node(cid) else {
                    return UnattendedCommandCheck::ParseErrors;
                };
                if child.is_restricted {
                    return UnattendedCommandCheck::PermissionsRequired;
                }
                current = cid;
                token_index += 1;
                continue;
            }

            let mut saw_argument = false;
            let mut saw_restricted = false;
            let mut saw_message = false;
            for &cid in child_ids {
                let Some(child) = self.node(cid) else {
                    continue;
                };
                let NodeType::Argument { .. } = &child.node_type else {
                    continue;
                };
                saw_argument = true;
                let (subtree_message, subtree_restricted) = self.conservative_subtree_flags(cid);
                saw_message |= subtree_message;
                saw_restricted |= subtree_restricted;
            }
            if saw_message {
                return UnattendedCommandCheck::SignatureRequired;
            }
            if saw_restricted {
                return UnattendedCommandCheck::PermissionsRequired;
            }
            if saw_argument {
                return UnattendedCommandCheck::ParseErrors;
            }
            return UnattendedCommandCheck::ParseErrors;
        }

        let Some(node) = self.node(current) else {
            return UnattendedCommandCheck::ParseErrors;
        };
        if node.is_restricted {
            return UnattendedCommandCheck::PermissionsRequired;
        }
        if let Some(redirect) = node.redirect_node {
            let (has_message, has_restricted) = self.conservative_subtree_flags(redirect);
            if has_message {
                return UnattendedCommandCheck::SignatureRequired;
            }
            if has_restricted {
                return UnattendedCommandCheck::PermissionsRequired;
            }
            return UnattendedCommandCheck::ParseErrors;
        }
        if !node.is_executable {
            return UnattendedCommandCheck::ParseErrors;
        }
        UnattendedCommandCheck::NoIssues
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

    /// Parsed ranges and smart-usage text for ChatScreen's command renderer.
    /// This mirrors Vanilla's presentation model using the server-supplied
    /// Brigadier tree. Minecraft-specific argument values are still validated
    /// by the server, but their consumed ranges and node transitions come from
    /// the packet metadata rather than a whitespace-only coloring heuristic.
    pub fn presentation(&self, command: &str) -> CommandPresentation {
        let lexical = command_token_ranges(command);
        let mut out = CommandPresentation::default();
        let mut current = self.root_index;
        let mut token_index = 0usize;
        let mut argument_index = 0usize;

        while token_index < lexical.len() {
            let Some(parent) = self.node(current) else {
                break;
            };
            let children = self.effective_children(parent);
            let token_text = &command[lexical[token_index].clone()];

            if let Some(child) = children.iter().copied().find(|&id| {
                matches!(
                    self.node(id).map(|node| &node.node_type),
                    Some(NodeType::Literal { name }) if name == token_text
                )
            }) {
                out.tokens.push(CommandTokenRange {
                    range: lexical[token_index].clone(),
                    kind: CommandTokenKind::Literal,
                });
                current = child;
                token_index += 1;
                continue;
            }

            let Some(child) = children.iter().copied().find(|&id| self.is_argument(id)) else {
                out.tokens.push(CommandTokenRange {
                    range: lexical[token_index].start..command.len(),
                    kind: CommandTokenKind::Unparsed,
                });
                return out;
            };
            let Some(NodeType::Argument { parser, .. }) =
                self.node(child).map(|node| &node.node_type)
            else {
                unreachable!();
            };
            let consumed = argument_token_count(parser, lexical.len() - token_index);
            if consumed == 0 || token_index + consumed > lexical.len() {
                out.tokens.push(CommandTokenRange {
                    range: lexical[token_index].start..command.len(),
                    kind: CommandTokenKind::Unparsed,
                });
                return out;
            }
            let end = lexical[token_index + consumed - 1].end;
            out.tokens.push(CommandTokenRange {
                range: lexical[token_index].start..end,
                kind: CommandTokenKind::Argument(argument_index % 5),
            });
            argument_index += 1;
            current = child;
            token_index += consumed;
        }

        out.usage_start = if command.ends_with(char::is_whitespace) {
            command.len()
        } else {
            lexical.last().map_or(0, |range| range.start)
        };
        if let Some(node) = self.node(current) {
            out.usage = self
                .effective_children(node)
                .iter()
                .filter_map(|&child| {
                    let child_node = self.node(child)?;
                    if matches!(child_node.node_type, NodeType::Literal { .. }) {
                        return None;
                    }
                    self.smart_usage(child, node.is_executable, false)
                })
                .collect();
        }
        out
    }

    fn smart_usage(&self, node_id: u32, optional: bool, deep: bool) -> Option<String> {
        let node = self.node(node_id)?;
        let usage_text = match &node.node_type {
            NodeType::Root => return None,
            NodeType::Literal { name } => name.clone(),
            NodeType::Argument { name, .. } => format!("<{name}>"),
        };
        let this = if optional {
            format!("[{usage_text}]")
        } else {
            usage_text
        };
        if deep {
            return Some(this);
        }
        if let Some(redirect) = node.redirect_node {
            let redirect = if redirect == self.root_index {
                "...".to_owned()
            } else {
                let target = self.node(redirect)?.name()?;
                format!("-> {target}")
            };
            return Some(format!("{this} {redirect}"));
        }

        let children = self.effective_children(node);
        if children.is_empty() {
            return Some(this);
        }
        let child_optional = node.is_executable;
        if children.len() == 1 {
            let child = self.smart_usage(children[0], child_optional, child_optional)?;
            return Some(format!("{this} {child}"));
        }

        let mut usages = Vec::new();
        for &child in children {
            if let Some(usage) = self.smart_usage(child, child_optional, true)
                && !usages.contains(&usage)
            {
                usages.push(usage);
            }
        }
        if usages.is_empty() {
            return Some(this);
        }
        if usages.len() == 1 {
            let usage = if child_optional {
                format!("[{}]", usages[0])
            } else {
                usages.remove(0)
            };
            return Some(format!("{this} {usage}"));
        }

        let separator = usages.join("|");
        let group = if child_optional {
            format!("[{separator}]")
        } else {
            format!("({separator})")
        };
        Some(format!("{this} {group}"))
    }
}

fn command_token_ranges(command: &str) -> Vec<Range<usize>> {
    let bytes = command.as_bytes();
    let mut ranges = Vec::new();
    let mut index = 0usize;
    while index < bytes.len() {
        while index < bytes.len() && bytes[index].is_ascii_whitespace() {
            index += 1;
        }
        if index >= bytes.len() {
            break;
        }
        let start = index;
        if bytes[index] == b'"' {
            index += 1;
            let mut escaped = false;
            while index < bytes.len() {
                let byte = bytes[index];
                index += 1;
                if escaped {
                    escaped = false;
                } else if byte == b'\\' {
                    escaped = true;
                } else if byte == b'"' {
                    break;
                }
            }
        } else {
            while index < bytes.len() && !bytes[index].is_ascii_whitespace() {
                index += 1;
            }
        }
        ranges.push(start..index);
    }
    ranges
}

fn argument_token_count(parser: &BrigadierParser, remaining: usize) -> usize {
    match parser {
        BrigadierParser::Vec3 | BrigadierParser::BlockPos => remaining.min(3),
        BrigadierParser::Vec2 | BrigadierParser::ColumnPos | BrigadierParser::Rotation => {
            remaining.min(2)
        }
        BrigadierParser::Message
        | BrigadierParser::FormattedText
        | BrigadierParser::String(BrigadierString::GreedyPhrase) => remaining,
        _ => remaining.min(1),
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
        assert_eq!(
            t.verify_unattended("time set day"),
            UnattendedCommandCheck::NoIssues
        );
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
    fn unattended_verifier_requires_signatures_for_messages() {
        let t = vanilla_like();
        assert_eq!(
            t.verify_unattended("msg Steve hello there"),
            UnattendedCommandCheck::SignatureRequired
        );
        // No message yet, but the command isn't executable, so it still needs
        // confirmation rather than unattended execution.
        assert_eq!(
            t.verify_unattended("msg Steve"),
            UnattendedCommandCheck::SignatureRequired,
            "an unparsed target argument must not hide a downstream MessageArgument"
        );
        assert_eq!(
            t.verify_unattended("msg"),
            UnattendedCommandCheck::ParseErrors
        );
        assert_eq!(
            t.verify_unattended("nonexistent foo"),
            UnattendedCommandCheck::ParseErrors
        );
    }

    #[test]
    fn unattended_verifier_fails_closed_for_unparsed_arguments() {
        let t = tree(vec![
            root(vec![1]),
            literal("number", vec![2], false),
            argument(
                "value",
                BrigadierParser::Integer(
                    azalea_protocol::packets::game::c_commands::BrigadierNumber::new(None, None),
                ),
                vec![],
                true,
            ),
        ]);
        assert_eq!(
            t.verify_unattended("number not-an-int"),
            UnattendedCommandCheck::ParseErrors
        );
    }

    #[test]
    fn unattended_verifier_prefers_signature_confirmation_for_ambiguous_message_branch() {
        let t = tree(vec![
            root(vec![1]),
            literal("mixed", vec![2, 3], false),
            argument(
                "number",
                BrigadierParser::Integer(
                    azalea_protocol::packets::game::c_commands::BrigadierNumber::new(None, None),
                ),
                vec![],
                true,
            ),
            argument("message", BrigadierParser::Message, vec![], true),
        ]);
        assert_eq!(
            t.verify_unattended("mixed hello"),
            UnattendedCommandCheck::SignatureRequired
        );
    }

    #[test]
    fn unattended_verifier_fails_closed_through_redirects_and_trailing_input() {
        let mut alias = literal("alias", vec![], false);
        alias.redirect_node = Some(2);
        let t = tree(vec![
            root(vec![1, 4]),
            alias,
            literal("target", vec![3], false),
            argument("message", BrigadierParser::Message, vec![], true),
            literal("plain", vec![], true),
        ]);
        assert_eq!(
            t.verify_unattended("alias hello"),
            UnattendedCommandCheck::SignatureRequired
        );
        assert_eq!(
            t.verify_unattended("plain extra"),
            UnattendedCommandCheck::ParseErrors
        );
    }

    #[test]
    fn unattended_verifier_reports_restricted_nodes() {
        let mut restricted = literal("admin", vec![], true);
        restricted.is_restricted = true;
        let t = tree(vec![root(vec![1]), restricted]);
        assert_eq!(
            t.verify_unattended("admin"),
            UnattendedCommandCheck::PermissionsRequired
        );
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
