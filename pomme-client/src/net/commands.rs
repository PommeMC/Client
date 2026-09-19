use std::collections::HashSet;
use std::ops::Range;
use std::str::FromStr;
use std::sync::Arc;

use azalea_protocol::packets::game::c_commands::{
    BrigadierNodeStub, BrigadierNumber, BrigadierParser, BrigadierString, ClientboundCommands,
    NodeType,
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
    /// Where parsing stopped short of the end of the input.
    pub error_at: Option<usize>,
    /// `Commands.getParseException`'s unknown-command case: nothing matched
    /// the first word.
    pub unknown_command: bool,
    /// `CommandSuggestions.currentParseIsMessage`.
    pub is_message: bool,
}

/// How far one argument's vanilla parser reads from its start.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum ArgCheck {
    Valid(usize),
    Invalid,
    /// A parser Pomme doesn't port; the end is a best guess where there is one.
    Unknown(Option<usize>),
}

#[derive(Clone)]
struct ParsedNode {
    node: u32,
    range: Range<usize>,
}

/// Brigadier's `CommandContextBuilder`: the nodes parsed from `root` up to
/// the next redirect.
#[derive(Clone)]
struct ParseContext {
    root: u32,
    range: Range<usize>,
    nodes: Vec<ParsedNode>,
    executable: bool,
}

impl ParseContext {
    fn new(root: u32, start: usize) -> Self {
        Self {
            root,
            range: start..start,
            nodes: Vec::new(),
            executable: false,
        }
    }
}

/// Brigadier's `ParseResults`, with the context chain flattened.
struct TreeParse {
    contexts: Vec<ParseContext>,
    /// Where the reader stopped.
    cursor: usize,
    /// Children that failed to parse where the reader stopped.
    errors: usize,
    /// Nodes below which a vanilla parse may have taken another branch,
    /// because some argument's parser isn't ported.
    uncertain: Vec<u32>,
}

impl TreeParse {
    /// `ClientPacketListener.isValidCommand`.
    fn is_valid(&self, input: &str) -> bool {
        self.cursor == input.len()
            && self.errors == 0
            && self.contexts.last().is_some_and(|c| c.executable)
    }

    /// `ArgumentVisitor.visitArguments` filtered to `MessageArgument`, 26.2's
    /// only `SignedArgument`: `(name, range)` per parsed message.
    fn message_arguments<'a>(
        &'a self,
        tree: &'a CommandTree,
        reject_root_redirects: bool,
    ) -> impl Iterator<Item = (&'a str, Range<usize>)> + 'a {
        let root = self.contexts.first().map(|c| c.root);
        self.contexts
            .iter()
            .enumerate()
            .take_while(move |(i, c)| *i == 0 || !reject_root_redirects || Some(c.root) != root)
            .flat_map(|(_, c)| &c.nodes)
            .filter_map(
                |parsed| match tree.node(parsed.node).map(|n| &n.node_type) {
                    Some(NodeType::Argument {
                        name,
                        parser: BrigadierParser::Message,
                        ..
                    }) => Some((name.as_str(), parsed.range.clone())),
                    _ => None,
                },
            )
    }

    /// `CommandContextBuilder.findSuggestionContext`: the node whose children
    /// complete the input at `cursor`, and where that completion starts.
    fn find_suggestion_context(&self, cursor: usize) -> Option<(u32, usize)> {
        let mut contexts = self.contexts.iter().peekable();
        while let Some(context) = contexts.next() {
            if context.range.end < cursor {
                if contexts.peek().is_some() {
                    continue;
                }
                return Some(
                    context
                        .nodes
                        .last()
                        .map_or((context.root, context.range.start), |last| {
                            (last.node, last.range.end + 1)
                        }),
                );
            }
            let mut prev = context.root;
            for parsed in &context.nodes {
                if parsed.range.start <= cursor && cursor <= parsed.range.end {
                    return Some((prev, parsed.range.start));
                }
                prev = parsed.node;
            }
            return Some((prev, context.range.start));
        }
        None
    }
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

    fn subtree_has_message(&self, start: u32) -> bool {
        let mut stack = vec![start];
        let mut visited = HashSet::new();
        while let Some(index) = stack.pop() {
            if !visited.insert(index) {
                continue;
            }
            let Some(node) = self.node(index) else {
                continue;
            };
            if matches!(
                &node.node_type,
                NodeType::Argument {
                    parser: BrigadierParser::Message,
                    ..
                }
            ) {
                return true;
            }
            stack.extend(node.children.iter().copied());
            stack.extend(node.redirect_node);
        }
        false
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

    /// `ClientPacketListener.verifyCommand` for a server-provided click
    /// command. A parse that went through an unported argument parser never
    /// reads as safe: it asks for a signature if a message could be involved,
    /// else reports parse errors.
    pub fn verify_unattended(&self, command: &str) -> UnattendedCommandCheck {
        let parse = self.parse(command, true);
        let signed = parse.message_arguments(self, true).next().is_some();
        if !parse.uncertain.is_empty() {
            return if signed
                || parse
                    .uncertain
                    .iter()
                    .any(|&node| self.subtree_has_message(node))
            {
                UnattendedCommandCheck::SignatureRequired
            } else if parse.contexts.iter().flat_map(|c| &c.nodes).any(|parsed| {
                self.node(parsed.node)
                    .is_some_and(|node| node.is_restricted)
            }) {
                UnattendedCommandCheck::PermissionsRequired
            } else {
                UnattendedCommandCheck::ParseErrors
            };
        }
        if !parse.is_valid(command) {
            return UnattendedCommandCheck::ParseErrors;
        }
        if signed {
            return UnattendedCommandCheck::SignatureRequired;
        }
        let restricted = self.parse(command, false);
        if !restricted.is_valid(command) || !restricted.uncertain.is_empty() {
            return UnattendedCommandCheck::PermissionsRequired;
        }
        UnattendedCommandCheck::NoIssues
    }

    /// The raw values of the command's signable arguments, as vanilla
    /// `SignableCommand.of` collects them.
    pub fn signable_arguments(&self, command: &str) -> Vec<(String, String)> {
        self.parse(command, true)
            .message_arguments(self, true)
            .map(|(name, range)| (name.to_owned(), command[range].to_owned()))
            .collect()
    }

    /// Brigadier's `CommandDispatcher.parse`. Restricted nodes are only
    /// usable with `allow_restricted`, like the client's two suggestion
    /// providers.
    fn parse(&self, input: &str, allow_restricted: bool) -> TreeParse {
        let context = ParseContext::new(self.root_index, 0);
        self.parse_nodes(self.root_index, input, 0, context, allow_restricted)
    }

    /// `CommandNode.getRelevantNodes`: the literal child named by the next
    /// word, else every argument child.
    fn relevant_nodes(&self, node: &BrigadierNodeStub, input: &str, cursor: usize) -> Vec<u32> {
        let word_end = input[cursor..]
            .find(' ')
            .map_or(input.len(), |i| cursor + i);
        let word = &input[cursor..word_end];
        let literal = node.children.iter().copied().find(|&child| {
            matches!(
                self.node(child).map(|c| &c.node_type),
                Some(NodeType::Literal { name }) if name == word
            )
        });
        match literal {
            Some(literal) => vec![literal],
            None => node
                .children
                .iter()
                .copied()
                .filter(|&child| self.is_argument(child))
                .collect(),
        }
    }

    /// Brigadier's `CommandDispatcher.parseNodes`.
    fn parse_nodes(
        &self,
        node: u32,
        input: &str,
        cursor: usize,
        context: ParseContext,
        allow_restricted: bool,
    ) -> TreeParse {
        let candidates: Vec<(u32, &BrigadierNodeStub)> = self
            .node(node)
            .map(|n| self.relevant_nodes(n, input, cursor))
            .unwrap_or_default()
            .into_iter()
            .filter_map(|child| Some((child, self.node(child)?)))
            .filter(|(_, child)| allow_restricted || !child.is_restricted)
            .collect();
        let mut potentials = Vec::new();
        let mut errors = 0;
        let mut uncertain = Vec::new();
        for &(child, child_node) in &candidates {
            let end = match &child_node.node_type {
                NodeType::Argument { parser, .. } => match check_argument(parser, input, cursor) {
                    ArgCheck::Valid(end) => end,
                    ArgCheck::Unknown(Some(end)) => {
                        uncertain.push(node);
                        end
                    }
                    check => {
                        if check == ArgCheck::Unknown(None) {
                            uncertain.push(node);
                        }
                        errors += 1;
                        continue;
                    }
                },
                NodeType::Literal { name } => cursor + name.len(),
                NodeType::Root => continue,
            };
            if input.as_bytes().get(end).is_some_and(|&b| b != b' ') {
                errors += 1;
                continue;
            }
            let mut context = context.clone();
            context.nodes.push(ParsedNode {
                node: child,
                range: cursor..end,
            });
            context.range.end = context.range.end.max(end);
            context.executable = child_node.is_executable;
            let redirect = child_node.redirect_node;
            if end + if redirect.is_some() { 1 } else { 2 } > input.len() {
                potentials.push(TreeParse {
                    contexts: vec![context],
                    cursor: end,
                    errors: 0,
                    uncertain: Vec::new(),
                });
                continue;
            }
            let Some(target) = redirect else {
                potentials.push(self.parse_nodes(child, input, end + 1, context, allow_restricted));
                continue;
            };
            let child_context = ParseContext::new(target, end + 1);
            let mut parse =
                self.parse_nodes(target, input, end + 1, child_context, allow_restricted);
            parse.contexts.insert(0, context);
            parse.uncertain.extend(uncertain);
            return parse;
        }

        uncertain.extend(potentials.iter().flat_map(|p| p.uncertain.iter().copied()));
        // A sibling may have won had an uncertain branch parsed differently.
        if !uncertain.is_empty() && candidates.len() > 1 {
            uncertain.push(node);
        }
        potentials.sort_by_key(|p| (p.cursor < input.len(), p.errors > 0));
        let mut parse = potentials.into_iter().next().unwrap_or(TreeParse {
            contexts: vec![context],
            cursor,
            errors,
            uncertain: Vec::new(),
        });
        parse.uncertain = uncertain;
        parse
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

    /// ChatScreen's command feedback for `command` (the input without its
    /// `/`) with the caret at `cursor`: `CommandSuggestions.formatText`'s
    /// argument colours and `updateUsageInfo`'s usage lines.
    pub fn presentation(&self, command: &str, cursor: usize) -> CommandPresentation {
        let parse = self.parse(command, true);
        let mut tokens: Vec<CommandTokenRange> = parse
            .contexts
            .last()
            .into_iter()
            .flat_map(|c| &c.nodes)
            .filter(|parsed| self.is_argument(parsed.node))
            .enumerate()
            .map(|(i, parsed)| CommandTokenRange {
                range: parsed.range.clone(),
                kind: CommandTokenKind::Argument(i),
            })
            .collect();
        let error_at = (parse.cursor < command.len()).then_some(parse.cursor);
        if let Some(start) = error_at {
            tokens.push(CommandTokenRange {
                range: start..command.len(),
                kind: CommandTokenKind::Unparsed,
            });
        }
        let (usage, usage_start) = parse
            .find_suggestion_context(cursor)
            .and_then(|(parent, start)| Some((self.node(parent)?, start)))
            .map(|(parent, start)| {
                let usage = parent
                    .children
                    .iter()
                    .filter(|&&child| self.is_argument(child))
                    .filter_map(|&child| self.smart_usage(child, parent.is_executable, false))
                    .collect();
                (usage, start)
            })
            .unwrap_or_default();
        CommandPresentation {
            tokens,
            usage,
            usage_start,
            error_at,
            unknown_command: error_at.is_some()
                && parse.errors != 1
                && parse.contexts.first().is_some_and(|c| c.range.is_empty()),
            is_message: parse.message_arguments(self, false).next().is_some(),
        }
    }

    /// Brigadier's private `CommandDispatcher.getSmartUsage`.
    fn smart_usage(&self, node_id: u32, optional: bool, deep: bool) -> Option<String> {
        let node = self.node(node_id)?;
        let usage = usage_text(node);
        let this = if optional {
            format!("[{usage}]")
        } else {
            usage
        };
        if deep {
            return Some(this);
        }
        if let Some(redirect) = node.redirect_node {
            let redirect = if redirect == self.root_index {
                "...".to_owned()
            } else {
                format!("-> {}", usage_text(self.node(redirect)?))
            };
            return Some(format!("{this} {redirect}"));
        }

        let child_optional = node.is_executable;
        match node.children.as_slice() {
            &[child] => {
                if let Some(usage) = self.smart_usage(child, child_optional, child_optional) {
                    return Some(format!("{this} {usage}"));
                }
            }
            children @ [_, _, ..] => {
                let mut child_usage: Vec<String> = Vec::new();
                for usage in children
                    .iter()
                    .filter_map(|&child| self.smart_usage(child, child_optional, true))
                {
                    if !child_usage.contains(&usage) {
                        child_usage.push(usage);
                    }
                }
                match child_usage.as_slice() {
                    [usage] => {
                        let usage = if child_optional {
                            format!("[{usage}]")
                        } else {
                            usage.clone()
                        };
                        return Some(format!("{this} {usage}"));
                    }
                    [_, _, ..] => {
                        let group = children
                            .iter()
                            .filter_map(|&child| self.node(child))
                            .map(usage_text)
                            .collect::<Vec<_>>()
                            .join("|");
                        let (open, close) = if child_optional {
                            ('[', ']')
                        } else {
                            ('(', ')')
                        };
                        return Some(format!("{this} {open}{group}{close}"));
                    }
                    [] => {}
                }
            }
            [] => {}
        }
        Some(this)
    }
}

/// `CommandNode.getUsageText`.
fn usage_text(node: &BrigadierNodeStub) -> String {
    match &node.node_type {
        NodeType::Root => String::new(),
        NodeType::Literal { name } => name.clone(),
        NodeType::Argument { name, .. } => format!("<{name}>"),
    }
}

/// Brigadier's `StringReader` over byte offsets into `input`.
struct Reader<'a> {
    input: &'a str,
    cursor: usize,
}

impl<'a> Reader<'a> {
    fn can_read(&self) -> bool {
        self.cursor < self.input.len()
    }

    fn peek(&self) -> Option<char> {
        self.input[self.cursor..].chars().next()
    }

    fn eat(&mut self, c: char) -> bool {
        let matched = self.peek() == Some(c);
        if matched {
            self.cursor += c.len_utf8();
        }
        matched
    }

    fn at_separator(&self) -> bool {
        matches!(self.peek(), None | Some(' '))
    }

    fn read_while(&mut self, allowed: impl Fn(char) -> bool) -> &'a str {
        let start = self.cursor;
        while let Some(c) = self.peek().filter(|&c| allowed(c)) {
            self.cursor += c.len_utf8();
        }
        &self.input[start..self.cursor]
    }

    /// `readInt`/`readLong`/`readFloat`/`readDouble`.
    fn read_number<T: FromStr>(&mut self) -> Option<T> {
        self.read_while(|c| c.is_ascii_digit() || matches!(c, '.' | '-'))
            .parse()
            .ok()
    }

    fn read_unquoted(&mut self) -> &'a str {
        self.read_while(|c| c.is_ascii_alphanumeric() || matches!(c, '_' | '-' | '.' | '+'))
    }

    fn read_string(&mut self) -> Option<String> {
        let Some(quote @ ('"' | '\'')) = self.peek() else {
            return Some(self.read_unquoted().to_owned());
        };
        self.cursor += 1;
        let mut out = String::new();
        let mut escaped = false;
        while let Some(c) = self.peek() {
            self.cursor += c.len_utf8();
            if escaped {
                if c != quote && c != '\\' {
                    return None;
                }
                out.push(c);
                escaped = false;
            } else if c == '\\' {
                escaped = true;
            } else if c == quote {
                return Some(out);
            } else {
                out.push(c);
            }
        }
        None
    }
}

/// The `parse` of the argument type behind `parser`, starting at `start`.
/// Parsers whose outcome depends on registries, permissions or syntax Pomme
/// doesn't port (selectors, NBT, components, ...) are `Unknown`.
fn check_argument(parser: &BrigadierParser, input: &str, start: usize) -> ArgCheck {
    let mut r = Reader {
        input,
        cursor: start,
    };
    let parsed = match parser {
        BrigadierParser::Bool => r.read_string().is_some_and(|v| v == "true" || v == "false"),
        BrigadierParser::Integer(bounds) => r
            .read_number()
            .is_some_and(|v| in_bounds(v, bounds, i32::MIN, i32::MAX)),
        BrigadierParser::Long(bounds) => r
            .read_number()
            .is_some_and(|v| in_bounds(v, bounds, i64::MIN, i64::MAX)),
        BrigadierParser::Float(bounds) => r
            .read_number()
            .is_some_and(|v| in_bounds(v, bounds, f32::MIN, f32::MAX)),
        BrigadierParser::Double(bounds) => r
            .read_number()
            .is_some_and(|v| in_bounds(v, bounds, f64::MIN, f64::MAX)),
        BrigadierParser::String(BrigadierString::SingleWord)
        | BrigadierParser::Objective
        | BrigadierParser::Team => {
            r.read_unquoted();
            true
        }
        BrigadierParser::String(BrigadierString::QuotablePhrase) => r.read_string().is_some(),
        BrigadierParser::String(BrigadierString::GreedyPhrase) => {
            r.cursor = input.len();
            true
        }
        BrigadierParser::Message => {
            let text = &input[start..];
            if text.encode_utf16().count() > 256 {
                false
            } else if text.contains('@') {
                return ArgCheck::Unknown(Some(input.len()));
            } else {
                r.cursor = input.len();
                true
            }
        }
        BrigadierParser::Identifier
        | BrigadierParser::Dimension
        | BrigadierParser::ResourceKey { .. } => identifier(&mut r),
        BrigadierParser::Entity(_)
        | BrigadierParser::GameProfile
        | BrigadierParser::ScoreHolder { .. }
            if r.peek() == Some('@') =>
        {
            return ArgCheck::Unknown(word_end(input.as_bytes(), start));
        }
        BrigadierParser::GameProfile | BrigadierParser::ScoreHolder { .. } => {
            r.read_while(|c| c != ' ');
            true
        }
        BrigadierParser::Entity(_) => match r.read_string() {
            Some(name) if name.len() <= 36 && name.matches('-').count() == 4 => {
                return ArgCheck::Unknown(Some(r.cursor));
            }
            Some(name) => (1..=16).contains(&name.encode_utf16().count()),
            None => false,
        },
        BrigadierParser::Vec3 | BrigadierParser::BlockPos if r.peek() == Some('^') => {
            coordinates(&mut r, 3, local_coordinate)
        }
        BrigadierParser::Vec3 => coordinates(&mut r, 3, |r| world_coordinate(r, false)),
        BrigadierParser::BlockPos => coordinates(&mut r, 3, |r| world_coordinate(r, true)),
        BrigadierParser::Vec2 | BrigadierParser::Rotation => {
            coordinates(&mut r, 2, |r| world_coordinate(r, false))
        }
        BrigadierParser::ColumnPos => coordinates(&mut r, 2, |r| world_coordinate(r, true)),
        BrigadierParser::Angle => {
            r.can_read() && {
                r.eat('~');
                r.at_separator() || r.read_number().is_some_and(f32::is_finite)
            }
        }
        BrigadierParser::Time { min } => r.read_number::<f32>().is_some_and(|value| {
            let factor = match r.read_unquoted() {
                "d" => 24000,
                "s" => 20,
                "t" | "" => 1,
                _ => return false,
            };
            java_round(value * factor as f32) >= *min
        }),
        BrigadierParser::GameMode => matches!(
            r.read_unquoted(),
            "survival" | "creative" | "adventure" | "spectator"
        ),
        BrigadierParser::EntityAnchor => matches!(r.read_unquoted(), "feet" | "eyes"),
        _ => return ArgCheck::Unknown(word_end(input.as_bytes(), start)),
    };
    if parsed {
        ArgCheck::Valid(r.cursor)
    } else {
        ArgCheck::Invalid
    }
}

/// Brigadier's numeric range check; an absent bound is the type's extreme.
fn in_bounds<T: PartialOrd + Copy>(
    value: T,
    bounds: &BrigadierNumber<T>,
    lowest: T,
    highest: T,
) -> bool {
    (bounds.min.unwrap_or(lowest)..=bounds.max.unwrap_or(highest)).contains(&value)
}

/// Java's `Math.round(float)`: halves round up, out-of-range saturates.
fn java_round(value: f32) -> i32 {
    let floor = value.floor();
    (if value - floor >= 0.5 {
        floor + 1.0
    } else {
        floor
    }) as i32
}

/// `Identifier.read`.
fn identifier(r: &mut Reader) -> bool {
    let raw = r.read_while(|c| matches!(c, '0'..='9' | 'a'..='z' | '_' | ':' | '/' | '.' | '-'));
    let (namespace, path) = raw.split_once(':').unwrap_or(("", raw));
    namespace != ".." && !namespace.contains('/') && !path.contains(':')
}

/// `count` coordinates separated by single spaces, as `WorldCoordinates`,
/// `LocalCoordinates` and the two-axis arguments read them.
fn coordinates(r: &mut Reader, count: usize, coordinate: impl Fn(&mut Reader) -> bool) -> bool {
    (0..count).all(|i| (i == 0 || r.eat(' ')) && coordinate(r))
}

/// `WorldCoordinate.parseInt`/`parseDouble`; an empty absolute number reads
/// as zero.
fn world_coordinate(r: &mut Reader, int: bool) -> bool {
    if !r.can_read() || r.peek() == Some('^') {
        return false;
    }
    let relative = r.eat('~');
    if r.at_separator() {
        return true;
    }
    if int && !relative {
        r.read_number::<i32>().is_some()
    } else {
        r.read_number::<f64>().is_some()
    }
}

/// `LocalCoordinates.readDouble`.
fn local_coordinate(r: &mut Reader) -> bool {
    r.eat('^') && (r.at_separator() || r.read_number::<f64>().is_some())
}

/// Where a word that an unported parser would read ends: quoted strings and
/// bracketed selector, NBT and block-state parts are skipped whole.
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
    /// Pomme has no client-side argument suggestions, so unlike vanilla it
    /// defers every argument to the server, not just `ask_server` ones.
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
        // Not executable without the message, so the parse isn't valid.
        assert_eq!(
            t.verify_unattended("msg Steve"),
            UnattendedCommandCheck::ParseErrors
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
            argument("value", int(None, None), vec![], true),
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
            argument("number", int(None, None), vec![], true),
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

        // An unported selector below a restricted literal still reports the
        // permission rather than a parse error.
        let mut kill = literal("kill", vec![2], false);
        kill.is_restricted = true;
        let targets = argument(
            "targets",
            BrigadierParser::Entity(EntityParser {
                single: false,
                players_only: false,
            }),
            vec![],
            true,
        );
        let t = tree(vec![root(vec![1]), kill, targets]);
        assert_eq!(
            t.verify_unattended("kill @e[type=zombie]"),
            UnattendedCommandCheck::PermissionsRequired
        );
    }

    #[test]
    fn unattended_verifier_rejects_stray_whitespace() {
        let t = tree(vec![
            root(vec![1]),
            literal("time", vec![2], false),
            literal("set", vec![3], false),
            literal("day", vec![], true),
        ]);
        for command in [
            " time set day",
            "time set day ",
            "time  set day",
            "time\tset day",
        ] {
            assert_eq!(
                t.verify_unattended(command),
                UnattendedCommandCheck::ParseErrors,
                "{command:?}"
            );
        }
    }

    #[test]
    fn unattended_verifier_checks_signatures_before_permissions() {
        let mut t = vanilla_like();
        t.nodes[4].is_restricted = true;
        t.nodes[5].is_restricted = true;
        assert_eq!(
            t.verify_unattended("say hi"),
            UnattendedCommandCheck::SignatureRequired
        );
        assert_eq!(
            t.verify_unattended("execute run say hi"),
            UnattendedCommandCheck::PermissionsRequired
        );
        // The selector isn't parsed locally, so the message below it counts.
        assert_eq!(
            t.verify_unattended("execute as @a run say hi"),
            UnattendedCommandCheck::SignatureRequired
        );
    }

    fn int(min: Option<i32>, max: Option<i32>) -> BrigadierParser {
        BrigadierParser::Integer(BrigadierNumber::new(min, max))
    }

    fn entity() -> BrigadierParser {
        BrigadierParser::Entity(EntityParser {
            single: false,
            players_only: false,
        })
    }

    /// root -> trigger <objective> [add|set <value>]
    fn trigger_tree() -> CommandTree {
        tree(vec![
            root(vec![1]),
            literal("trigger", vec![2], false),
            argument("objective", BrigadierParser::Objective, vec![3, 4], true),
            literal("add", vec![5], false),
            literal("set", vec![5], false),
            argument("value", int(None, None), vec![], true),
        ])
    }

    /// root -> tp {<location>, <destination>, <targets> {<location>,
    /// <destination>}}, execute {as <targets> -> execute, run -> root}
    fn teleport_tree() -> CommandTree {
        tree(vec![
            root(vec![1, 7]),
            literal("tp", vec![2, 3, 4], false),
            argument("location", BrigadierParser::Vec3, vec![], true),
            argument("destination", entity(), vec![], true),
            argument("targets", entity(), vec![5, 6], false),
            argument("location", BrigadierParser::Vec3, vec![], true),
            argument("destination", entity(), vec![], true),
            literal("execute", vec![8, 10], false),
            literal("as", vec![9], false),
            BrigadierNodeStub {
                redirect_node: Some(7),
                ..argument("targets", entity(), vec![], false)
            },
            redirect("run", 0),
        ])
    }

    fn arguments(ranges: &[(usize, usize)]) -> Vec<CommandTokenRange> {
        ranges
            .iter()
            .enumerate()
            .map(|(i, &(start, end))| CommandTokenRange {
                range: start..end,
                kind: CommandTokenKind::Argument(i),
            })
            .collect()
    }

    #[test]
    fn trigger_usage_lists_each_child() {
        let t = trigger_tree();
        assert_eq!(
            t.verify_unattended("trigger vote set 1"),
            UnattendedCommandCheck::NoIssues
        );
        let p = t.presentation("trigger ", 8);
        assert_eq!(p.usage, vec!["<objective> [add|set]"]);
        assert_eq!(p.usage_start, 8);
        assert_eq!(
            p.tokens,
            vec![CommandTokenRange {
                range: 7..8,
                kind: CommandTokenKind::Unparsed,
            }]
        );
        assert!(!p.unknown_command);
    }

    #[test]
    fn presentation_takes_the_best_branch() {
        let t = teleport_tree();
        let p = t.presentation("tp Steve 1 2 3", 14);
        assert_eq!(p.tokens, arguments(&[(3, 8), (9, 14)]));
        assert_eq!(p.error_at, None);
        // Two player names, as vanilla reads it: not a partial Vec3.
        assert_eq!(
            t.presentation("tp 1 2", 6).tokens,
            arguments(&[(3, 4), (5, 6)])
        );
    }

    #[test]
    fn presentation_colours_only_the_last_context() {
        let t = teleport_tree();
        let p = t.presentation("execute as @a run tp ~ ~ ~", 26);
        assert_eq!(p.tokens, arguments(&[(21, 26)]));
    }

    #[test]
    fn presentation_flags_unknown_commands() {
        let t = teleport_tree();
        let p = t.presentation("bogus", 5);
        assert!(p.unknown_command);
        assert_eq!(p.error_at, Some(0));
        let p = t.presentation("tp ~ ~ ~ extra", 14);
        assert!(!p.unknown_command);
        assert_eq!(p.error_at, Some(9));
    }

    #[test]
    fn presentation_marks_message_commands() {
        let t = vanilla_like();
        assert!(t.presentation("execute run say hi", 18).is_message);
        assert!(!t.presentation("tp ~ ~ ~", 8).is_message);
    }

    fn check(parser: BrigadierParser, input: &str) -> ArgCheck {
        check_argument(&parser, input, 0)
    }

    #[test]
    fn numbers_follow_string_reader_rules() {
        let bounded = || int(Some(0), Some(10));
        assert_eq!(check(bounded(), "5 x"), ArgCheck::Valid(1));
        assert_eq!(check(bounded(), "11"), ArgCheck::Invalid);
        assert_eq!(check(bounded(), "-1"), ArgCheck::Invalid);
        assert_eq!(check(int(None, None), "1.0"), ArgCheck::Invalid);
        assert_eq!(check(BrigadierParser::Bool, "true"), ArgCheck::Valid(4));
        assert_eq!(check(BrigadierParser::Bool, "yes"), ArgCheck::Invalid);
        assert_eq!(
            check(BrigadierParser::Time { min: 0 }, "1d"),
            ArgCheck::Valid(2)
        );
        assert_eq!(
            check(BrigadierParser::Time { min: 0 }, "1x"),
            ArgCheck::Invalid
        );
        assert_eq!(
            check(BrigadierParser::Time { min: 1 }, "0.4t"),
            ArgCheck::Invalid
        );
    }

    #[test]
    fn coordinates_follow_world_and_local_rules() {
        assert_eq!(check(BrigadierParser::Vec3, "~ ~1 ^"), ArgCheck::Invalid);
        assert_eq!(check(BrigadierParser::Vec3, "^ ^ ^1"), ArgCheck::Valid(6));
        // An empty absolute coordinate before a space reads as zero.
        assert_eq!(check(BrigadierParser::Vec3, "1  2 3"), ArgCheck::Valid(4));
        assert_eq!(check(BrigadierParser::Vec3, "1 2"), ArgCheck::Invalid);
        assert_eq!(
            check(BrigadierParser::ColumnPos, "~1 2"),
            ArgCheck::Valid(4)
        );
        assert_eq!(
            check(BrigadierParser::BlockPos, "1.5 2 3"),
            ArgCheck::Invalid
        );
    }

    #[test]
    fn identifiers_and_entities() {
        // Reads nothing; the node then fails on the missing separator.
        assert_eq!(
            check(BrigadierParser::Identifier, "Minecraft:x"),
            ArgCheck::Valid(0)
        );
        assert_eq!(
            check(BrigadierParser::Identifier, "a:b:c"),
            ArgCheck::Invalid
        );
        assert_eq!(
            check(BrigadierParser::Identifier, "minecraft:stone"),
            ArgCheck::Valid(15)
        );
        assert_eq!(check(entity(), "@a[tag=x"), ArgCheck::Unknown(None));
        assert_eq!(check(entity(), "@a[tag=x] hi"), ArgCheck::Unknown(Some(9)));
        assert_eq!(check(entity(), "Steve"), ArgCheck::Valid(5));
        assert_eq!(check(entity(), "\"a b\""), ArgCheck::Valid(5));
        assert_eq!(check(entity(), "abcdefghijklmnopq"), ArgCheck::Invalid);
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
