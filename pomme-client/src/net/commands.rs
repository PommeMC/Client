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

/// A Brigadier `CommandSyntaxException`: its message's translation key and
/// arguments, and the reader position `createWithContext` captured (`None`
/// for a context-free `create()`).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SyntaxError {
    pub key: &'static str,
    pub args: Vec<String>,
    pub cursor: Option<usize>,
}

/// One line of `CommandSuggestions.commandUsage` from the parse.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum UsageLine {
    Usage(String),
    Error(SyntaxError),
}

/// `CommandSuggestions.updateUsageInfo`'s lines and the start of the
/// suggestion context they're drawn at.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CommandUsage {
    pub lines: Vec<UsageLine>,
    pub start: usize,
}

/// Vanilla `CommandSuggestions.currentParse`: the chat input's command,
/// parsed once per edit.
pub struct CommandParse {
    input: String,
    parse: TreeParse,
    tokens: Vec<CommandTokenRange>,
    is_message: bool,
}

impl CommandParse {
    /// The parsed command, without its `/`.
    pub fn input(&self) -> &str {
        &self.input
    }

    /// `CommandSuggestions.formatText`'s ranges: the last context's
    /// arguments, then any unparsed rest.
    pub fn tokens(&self) -> &[CommandTokenRange] {
        &self.tokens
    }

    /// `CommandSuggestions.currentParseIsMessage`.
    pub fn is_message(&self) -> bool {
        self.is_message
    }
}

/// How far one argument's vanilla parser reads from its start.
#[derive(Clone, Debug, PartialEq, Eq)]
enum ArgCheck {
    Valid(usize),
    Invalid(SyntaxError),
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
    /// Why each child failed where the reader stopped, in child order.
    errors: Vec<SyntaxError>,
    /// Nodes below which a vanilla parse may have taken another branch,
    /// because some argument's parser isn't ported.
    uncertain: Vec<u32>,
}

impl TreeParse {
    /// `ClientPacketListener.isValidCommand`.
    fn is_valid(&self, input: &str) -> bool {
        self.cursor == input.len()
            && self.errors.is_empty()
            && self.contexts.last().is_some_and(|c| c.executable)
    }

    /// `Commands.getParseException`.
    fn parse_exception(&self, input: &str) -> Option<SyntaxError> {
        if self.cursor == input.len() {
            return None;
        }
        if let [error] = self.errors.as_slice() {
            return Some(error.clone());
        }
        let key = if self.contexts.first().is_some_and(|c| c.range.is_empty()) {
            "command.unknown.command"
        } else {
            "command.unknown.argument"
        };
        Some(SyntaxError {
            key,
            args: Vec::new(),
            cursor: Some(self.cursor),
        })
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
    /// word, else every argument child. A literal is only ever tried when it
    /// matches, so Brigadier's `literalIncorrect` can't arise.
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
        let mut errors = Vec::new();
        let mut uncertain = Vec::new();
        for &(child, child_node) in &candidates {
            let end = match &child_node.node_type {
                NodeType::Argument { parser, .. } => match check_argument(parser, input, cursor) {
                    ArgCheck::Valid(end) => end,
                    ArgCheck::Unknown(Some(end)) => {
                        uncertain.push(node);
                        end
                    }
                    ArgCheck::Invalid(error) => {
                        errors.push(error);
                        continue;
                    }
                    ArgCheck::Unknown(None) => {
                        uncertain.push(node);
                        // TODO: the unported parser's own message.
                        errors.push(SyntaxError {
                            key: "command.unknown.argument",
                            args: Vec::new(),
                            cursor: Some(cursor),
                        });
                        continue;
                    }
                },
                NodeType::Literal { name } => cursor + name.len(),
                NodeType::Root => continue,
            };
            if input.as_bytes().get(end).is_some_and(|&b| b != b' ') {
                errors.push(SyntaxError {
                    key: "command.expected.separator",
                    args: Vec::new(),
                    cursor: Some(end),
                });
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
                    errors: Vec::new(),
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
        potentials.sort_by_key(|p| (p.cursor < input.len(), !p.errors.is_empty()));
        let mut parse = potentials.into_iter().next().unwrap_or(TreeParse {
            contexts: vec![context],
            cursor,
            errors,
            uncertain: Vec::new(),
        });
        parse.uncertain = uncertain;
        parse
    }

    /// Parse the chat input's `command` (without its `/`) as ChatScreen does.
    pub fn parse_command(&self, command: &str) -> CommandParse {
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
        if parse.cursor < command.len() {
            tokens.push(CommandTokenRange {
                range: parse.cursor..command.len(),
                kind: CommandTokenKind::Unparsed,
            });
        }
        let is_message = parse.message_arguments(self, false).next().is_some();
        CommandParse {
            input: command.to_owned(),
            parse,
            tokens,
            is_message,
        }
    }

    /// `CommandDispatcher.getCompletionSuggestions` with the caret at
    /// `cursor`: the parent's literal children that complete the typed text,
    /// as `LiteralCommandNode.listSuggestions` offers them.
    pub fn completions(&self, parse: &CommandParse, cursor: usize) -> Suggestions {
        let Some((parent, start)) = parse
            .parse
            .find_suggestion_context(cursor)
            .and_then(|(parent, start)| Some((self.node(parent)?, start.min(cursor))))
        else {
            return Suggestions::empty(cursor);
        };
        let typed = &parse.input[start..cursor];
        let typed_lower = typed.to_lowercase();
        let mut options = Vec::new();
        let mut needs_server = false;
        for node in parent.children.iter().filter_map(|&child| self.node(child)) {
            match &node.node_type {
                NodeType::Literal { name } => {
                    if name.to_lowercase().starts_with(&typed_lower) && name != typed {
                        options.push(name.clone());
                    }
                }
                NodeType::Argument { .. } => needs_server = true,
                NodeType::Root => {}
            }
        }
        options.sort_by_key(|a| a.to_lowercase());
        Suggestions {
            options,
            start,
            needs_server,
        }
    }

    /// The parse-derived half of `CommandSuggestions.updateUsageInfo`, with
    /// the caret at `cursor`: the parse errors when nothing completes the
    /// input, else the parent's argument usage, falling back to
    /// `Commands.getParseException` for unparsed trailing input.
    pub fn usage(&self, parse: &CommandParse, cursor: usize, no_completions: bool) -> CommandUsage {
        let input = parse.input.as_str();
        let parse = &parse.parse;
        let mut lines = Vec::new();
        let mut trailing = false;
        if cursor == input.len() {
            if no_completions && !parse.errors.is_empty() {
                lines.extend(parse.errors.iter().cloned().map(UsageLine::Error));
            } else if parse.cursor < input.len() {
                trailing = true;
            }
        }
        let (parent, start) = parse
            .find_suggestion_context(cursor)
            .unwrap_or((self.root_index, 0));
        if lines.is_empty() {
            let usage: Vec<String> = self
                .node(parent)
                .map(|parent| {
                    parent
                        .children
                        .iter()
                        .filter(|&&child| self.is_argument(child))
                        .filter_map(|&child| self.smart_usage(child, parent.is_executable, false))
                        .collect()
                })
                .unwrap_or_default();
            if usage.is_empty() && trailing {
                lines.extend(parse.parse_exception(input).map(UsageLine::Error));
            }
            lines.extend(usage.into_iter().map(UsageLine::Usage));
        }
        CommandUsage { lines, start }
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

    /// `createWithContext(reader)`.
    fn error(&self, key: &'static str, args: Vec<String>) -> SyntaxError {
        SyntaxError {
            key,
            args,
            cursor: Some(self.cursor),
        }
    }

    fn read_while(&mut self, allowed: impl Fn(char) -> bool) -> &'a str {
        let start = self.cursor;
        while let Some(c) = self.peek().filter(|&c| allowed(c)) {
            self.cursor += c.len_utf8();
        }
        &self.input[start..self.cursor]
    }

    /// `readInt`/`readLong`/`readFloat`/`readDouble`.
    fn read_number<T: JavaNumber>(&mut self) -> Result<T, SyntaxError> {
        let start = self.cursor;
        let number = self.read_while(|c| c.is_ascii_digit() || matches!(c, '.' | '-'));
        if number.is_empty() {
            return Err(self.error(T::EXPECTED, Vec::new()));
        }
        number.parse().map_err(|_| {
            self.cursor = start;
            self.error(T::INVALID, vec![number.to_owned()])
        })
    }

    fn read_unquoted(&mut self) -> &'a str {
        self.read_while(|c| c.is_ascii_alphanumeric() || matches!(c, '_' | '-' | '.' | '+'))
    }

    fn read_string(&mut self) -> Result<String, SyntaxError> {
        let Some(quote @ ('"' | '\'')) = self.peek() else {
            return Ok(self.read_unquoted().to_owned());
        };
        self.cursor += 1;
        let mut out = String::new();
        let mut escaped = false;
        while let Some(c) = self.peek() {
            self.cursor += c.len_utf8();
            if escaped {
                if c != quote && c != '\\' {
                    self.cursor -= c.len_utf8();
                    return Err(self.error("parsing.quote.escape", vec![c.to_string()]));
                }
                out.push(c);
                escaped = false;
            } else if c == '\\' {
                escaped = true;
            } else if c == quote {
                return Ok(out);
            } else {
                out.push(c);
            }
        }
        Err(self.error("parsing.quote.expected.end", Vec::new()))
    }

    /// `readBoolean`.
    fn read_bool(&mut self) -> Result<(), SyntaxError> {
        let start = self.cursor;
        let value = self.read_string()?;
        if value.is_empty() {
            return Err(self.error("parsing.bool.expected", Vec::new()));
        }
        if value != "true" && value != "false" {
            self.cursor = start;
            return Err(self.error("parsing.bool.invalid", vec![value]));
        }
        Ok(())
    }
}

/// A Brigadier number type: its reader and range-check messages, extremes,
/// and Java `toString`.
trait JavaNumber: FromStr + PartialOrd + Copy {
    const EXPECTED: &'static str;
    const INVALID: &'static str;
    const LOW: &'static str;
    const BIG: &'static str;
    const LOWEST: Self;
    const HIGHEST: Self;
    fn java_string(self) -> String;
}

macro_rules! java_number {
    ($ty:ty, $reader:literal, $argument:literal, $to_string:expr) => {
        impl JavaNumber for $ty {
            const EXPECTED: &'static str = concat!("parsing.", $reader, ".expected");
            const INVALID: &'static str = concat!("parsing.", $reader, ".invalid");
            const LOW: &'static str = concat!("argument.", $argument, ".low");
            const BIG: &'static str = concat!("argument.", $argument, ".big");
            const LOWEST: Self = <$ty>::MIN;
            const HIGHEST: Self = <$ty>::MAX;
            fn java_string(self) -> String {
                ($to_string)(self)
            }
        }
    };
}

java_number!(i32, "int", "integer", |v: i32| v.to_string());
java_number!(i64, "long", "long", |v: i64| v.to_string());
java_number!(f32, "float", "float", |v: f32| java_float_string(&format!(
    "{v:e}"
)));
java_number!(f64, "double", "double", |v: f64| java_float_string(
    &format!("{v:e}")
));

/// Java's `Float.toString`/`Double.toString` from Rust's shortest `{:e}`
/// form: plain digits from 10^-3 up to 10^7, else `d.dddE<n>`.
fn java_float_string(sci: &str) -> String {
    let (sign, sci) = match sci.strip_prefix('-') {
        Some(rest) => ("-", rest),
        None => ("", sci),
    };
    let Some((mantissa, exponent)) = sci.split_once('e') else {
        return format!("{sign}{}", if sci == "inf" { "Infinity" } else { sci });
    };
    let exponent: i32 = exponent.parse().unwrap_or(0);
    let digits = mantissa.replace('.', "");
    let or_zero = |s: &str| {
        if s.is_empty() {
            "0".to_owned()
        } else {
            s.to_owned()
        }
    };
    let body = if digits == "0" {
        "0.0".to_owned()
    } else if !(-3..7).contains(&exponent) {
        let (first, rest) = digits.split_at(1);
        format!("{first}.{}E{exponent}", or_zero(rest))
    } else if exponent < 0 {
        format!("0.{}{digits}", "0".repeat((-exponent - 1) as usize))
    } else {
        let point = exponent as usize + 1;
        let padded = format!("{digits:0<point$}");
        let (int, frac) = padded.split_at(point);
        format!("{int}.{}", or_zero(frac))
    };
    format!("{sign}{body}")
}

/// The `parse` of the argument type behind `parser`, starting at `start`.
/// Parsers whose outcome depends on registries, permissions or syntax Pomme
/// doesn't port (selectors, NBT, components, ...) are `Unknown`.
fn check_argument(parser: &BrigadierParser, input: &str, start: usize) -> ArgCheck {
    const POS3D_INCOMPLETE: &str = "argument.pos3d.incomplete";
    const POS2D_INCOMPLETE: &str = "argument.pos2d.incomplete";
    let mut r = Reader {
        input,
        cursor: start,
    };
    let parsed = match parser {
        BrigadierParser::Bool => r.read_bool(),
        BrigadierParser::Integer(bounds) => read_bounded(&mut r, bounds),
        BrigadierParser::Long(bounds) => read_bounded(&mut r, bounds),
        BrigadierParser::Float(bounds) => read_bounded(&mut r, bounds),
        BrigadierParser::Double(bounds) => read_bounded(&mut r, bounds),
        BrigadierParser::String(BrigadierString::SingleWord)
        | BrigadierParser::Objective
        | BrigadierParser::Team => {
            r.read_unquoted();
            Ok(())
        }
        BrigadierParser::String(BrigadierString::QuotablePhrase) => r.read_string().map(drop),
        BrigadierParser::String(BrigadierString::GreedyPhrase) => {
            r.cursor = input.len();
            Ok(())
        }
        BrigadierParser::Message => {
            let length = input[start..].encode_utf16().count();
            if length > 256 {
                Err(SyntaxError {
                    key: "argument.message.too_long",
                    args: vec![length.to_string(), "256".to_owned()],
                    cursor: None,
                })
            } else if input[start..].contains('@') {
                return ArgCheck::Unknown(Some(input.len()));
            } else {
                r.cursor = input.len();
                Ok(())
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
            Ok(())
        }
        BrigadierParser::Entity(_) => match r.read_string() {
            Ok(name) if name.len() <= 36 && name.matches('-').count() == 4 => {
                return ArgCheck::Unknown(Some(r.cursor));
            }
            Ok(name) if (1..=16).contains(&name.encode_utf16().count()) => Ok(()),
            Ok(_) => {
                r.cursor = start;
                Err(r.error("argument.entity.invalid", Vec::new()))
            }
            Err(error) => Err(error),
        },
        BrigadierParser::Vec3 | BrigadierParser::BlockPos if r.peek() == Some('^') => {
            coordinates(&mut r, 3, POS3D_INCOMPLETE, |r| local_coordinate(r, start))
        }
        BrigadierParser::Vec3 => {
            coordinates(&mut r, 3, POS3D_INCOMPLETE, |r| world_coordinate(r, false))
        }
        BrigadierParser::BlockPos => {
            coordinates(&mut r, 3, POS3D_INCOMPLETE, |r| world_coordinate(r, true))
        }
        BrigadierParser::Vec2 => {
            coordinates(&mut r, 2, POS2D_INCOMPLETE, |r| world_coordinate(r, false))
        }
        BrigadierParser::Rotation => coordinates(&mut r, 2, "argument.rotation.incomplete", |r| {
            world_coordinate(r, false)
        }),
        BrigadierParser::ColumnPos => {
            coordinates(&mut r, 2, POS2D_INCOMPLETE, |r| world_coordinate(r, true))
        }
        BrigadierParser::Angle => angle(&mut r),
        BrigadierParser::Time { min } => time(&mut r, *min),
        BrigadierParser::GameMode => {
            let name = r.read_unquoted();
            if matches!(name, "survival" | "creative" | "adventure" | "spectator") {
                Ok(())
            } else {
                Err(r.error("argument.gamemode.invalid", vec![name.to_owned()]))
            }
        }
        BrigadierParser::EntityAnchor => {
            let name = r.read_unquoted();
            if matches!(name, "feet" | "eyes") {
                Ok(())
            } else {
                r.cursor = start;
                Err(r.error("argument.anchor.invalid", vec![name.to_owned()]))
            }
        }
        _ => return ArgCheck::Unknown(word_end(input.as_bytes(), start)),
    };
    match parsed {
        Ok(()) => ArgCheck::Valid(r.cursor),
        Err(error) => ArgCheck::Invalid(error),
    }
}

/// `IntegerArgumentType.parse` and its long/float/double siblings; an absent
/// bound is the type's extreme.
fn read_bounded<T: JavaNumber>(
    r: &mut Reader,
    bounds: &BrigadierNumber<T>,
) -> Result<(), SyntaxError> {
    let start = r.cursor;
    let value: T = r.read_number()?;
    let min = bounds.min.unwrap_or(T::LOWEST);
    let max = bounds.max.unwrap_or(T::HIGHEST);
    let (key, limit) = if value < min {
        (T::LOW, min)
    } else if value > max {
        (T::BIG, max)
    } else {
        return Ok(());
    };
    r.cursor = start;
    Err(r.error(key, vec![limit.java_string(), value.java_string()]))
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
fn identifier(r: &mut Reader) -> Result<(), SyntaxError> {
    let start = r.cursor;
    let raw = r.read_while(|c| matches!(c, '0'..='9' | 'a'..='z' | '_' | ':' | '/' | '.' | '-'));
    let (namespace, path) = raw.split_once(':').unwrap_or(("", raw));
    if namespace != ".." && !namespace.contains('/') && !path.contains(':') {
        return Ok(());
    }
    r.cursor = start;
    Err(r.error("argument.id.invalid", Vec::new()))
}

/// `count` coordinates separated by single spaces, as `WorldCoordinates`,
/// `LocalCoordinates` and the two-axis arguments read them.
fn coordinates(
    r: &mut Reader,
    count: usize,
    incomplete: &'static str,
    coordinate: impl Fn(&mut Reader) -> Result<(), SyntaxError>,
) -> Result<(), SyntaxError> {
    let start = r.cursor;
    for i in 0..count {
        if i > 0 && !r.eat(' ') {
            r.cursor = start;
            return Err(r.error(incomplete, Vec::new()));
        }
        coordinate(r)?;
    }
    Ok(())
}

/// `WorldCoordinate.parseInt`/`parseDouble`; an empty absolute number reads
/// as zero.
fn world_coordinate(r: &mut Reader, int: bool) -> Result<(), SyntaxError> {
    if r.peek() == Some('^') {
        return Err(r.error("argument.pos.mixed", Vec::new()));
    }
    if !r.can_read() {
        let key = if int {
            "argument.pos.missing.int"
        } else {
            "argument.pos.missing.double"
        };
        return Err(r.error(key, Vec::new()));
    }
    let relative = r.eat('~');
    if r.at_separator() {
        Ok(())
    } else if int && !relative {
        r.read_number::<i32>().map(drop)
    } else {
        r.read_number::<f64>().map(drop)
    }
}

/// `LocalCoordinates.readDouble`; `start` is the whole argument's.
fn local_coordinate(r: &mut Reader, start: usize) -> Result<(), SyntaxError> {
    if !r.can_read() {
        return Err(r.error("argument.pos.missing.double", Vec::new()));
    }
    if !r.eat('^') {
        r.cursor = start;
        return Err(r.error("argument.pos.mixed", Vec::new()));
    }
    if r.at_separator() {
        Ok(())
    } else {
        r.read_number::<f64>().map(drop)
    }
}

/// `AngleArgument.parse`.
fn angle(r: &mut Reader) -> Result<(), SyntaxError> {
    if !r.can_read() {
        return Err(r.error("argument.angle.incomplete", Vec::new()));
    }
    r.eat('~');
    if !r.at_separator() && !r.read_number::<f32>()?.is_finite() {
        return Err(r.error("argument.angle.invalid", Vec::new()));
    }
    Ok(())
}

/// `TimeArgument.parse`.
fn time(r: &mut Reader, min: i32) -> Result<(), SyntaxError> {
    let value: f32 = r.read_number()?;
    let factor = match r.read_unquoted() {
        "d" => 24000,
        "s" => 20,
        "t" | "" => 1,
        _ => return Err(r.error("argument.time.invalid_unit", Vec::new())),
    };
    let ticks = java_round(value * factor as f32);
    if ticks < min {
        return Err(r.error(
            "argument.time.tick_count_too_low",
            vec![min.to_string(), ticks.to_string()],
        ));
    }
    Ok(())
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

/// Local command completions: the matching literal names and where the text
/// they replace starts.
pub struct Suggestions {
    pub options: Vec<String>,
    pub start: usize,
    /// An argument could follow, so the server should be asked for
    /// completions (player names, enum values, ...). Pomme has no
    /// client-side argument suggestions, so unlike vanilla it defers every
    /// argument to the server, not just `ask_server` ones.
    pub needs_server: bool,
}

impl Suggestions {
    fn empty(start: usize) -> Self {
        Self {
            options: Vec::new(),
            start,
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

    fn parsed(t: &CommandTree, command: &str) -> CommandParse {
        t.parse_command(command)
    }

    fn complete(t: &CommandTree, command: &str, cursor: usize) -> Suggestions {
        t.completions(&parsed(t, command), cursor)
    }

    fn usage(t: &CommandTree, command: &str, no_completions: bool) -> CommandUsage {
        t.usage(&parsed(t, command), command.len(), no_completions)
    }

    fn error(key: &'static str, args: &[&str], cursor: usize) -> SyntaxError {
        SyntaxError {
            key,
            args: args.iter().map(|a| a.to_string()).collect(),
            cursor: Some(cursor),
        }
    }

    #[test]
    fn trigger_usage_lists_each_child() {
        let t = trigger_tree();
        assert_eq!(
            t.verify_unattended("trigger vote set 1"),
            UnattendedCommandCheck::NoIssues
        );
        assert_eq!(
            usage(&t, "trigger ", false),
            CommandUsage {
                lines: vec![UsageLine::Usage("<objective> [add|set]".to_owned())],
                start: 8,
            }
        );
        assert_eq!(
            parsed(&t, "trigger ").tokens(),
            [CommandTokenRange {
                range: 7..8,
                kind: CommandTokenKind::Unparsed,
            }]
        );
    }

    #[test]
    fn parse_takes_the_best_branch() {
        let t = teleport_tree();
        assert_eq!(
            parsed(&t, "tp Steve 1 2 3").tokens(),
            arguments(&[(3, 8), (9, 14)])
        );
        // Two player names, as vanilla reads it: not a partial Vec3.
        assert_eq!(parsed(&t, "tp 1 2").tokens(), arguments(&[(3, 4), (5, 6)]));
    }

    #[test]
    fn parse_colours_only_the_last_context() {
        let t = teleport_tree();
        assert_eq!(
            parsed(&t, "execute as @a run tp ~ ~ ~").tokens(),
            arguments(&[(21, 26)])
        );
    }

    #[test]
    fn parse_marks_message_commands() {
        let t = vanilla_like();
        assert!(parsed(&t, "execute run say hi").is_message());
        assert!(!parsed(&t, "tp ~ ~ ~").is_message());
    }

    #[test]
    fn usage_shows_parse_errors_only_without_completions() {
        let t = tree(vec![
            root(vec![1, 3]),
            literal("number", vec![2], false),
            argument("value", int(None, None), vec![], true),
            literal("time", vec![4], false),
            literal("set", vec![5], false),
            literal("day", vec![], true),
        ]);
        assert_eq!(
            usage(&t, "number x", true).lines,
            vec![UsageLine::Error(error("parsing.int.expected", &[], 7))]
        );
        assert_eq!(
            usage(&t, "number x", false).lines,
            vec![UsageLine::Usage("<value>".to_owned())]
        );
        // Trailing input with no usage falls back to getParseException.
        assert_eq!(
            usage(&t, "time set x", false).lines,
            vec![UsageLine::Error(error("command.unknown.argument", &[], 9))]
        );
        assert_eq!(
            usage(&t, "bogus", false).lines,
            vec![UsageLine::Error(error("command.unknown.command", &[], 0))]
        );
        // A lone exception is reported as itself.
        assert_eq!(
            t.parse("number 5x", true).parse_exception("number 5x"),
            Some(error("command.expected.separator", &[], 8))
        );
        // Away from the end, errors wait and the caret's context gives the
        // usage: the root's, which has no arguments.
        let mid = t.usage(&parsed(&t, "number x"), 3, true);
        assert!(mid.lines.is_empty());
        assert_eq!(mid.start, 0);
    }

    #[test]
    fn completions_follow_the_parse() {
        // root -> execute {positioned <pos> -> execute, run -> root}
        let t = tree(vec![
            root(vec![1]),
            literal("execute", vec![2, 4], false),
            literal("positioned", vec![3], false),
            BrigadierNodeStub {
                redirect_node: Some(1),
                ..argument("pos", BrigadierParser::Vec3, vec![], false)
            },
            redirect("run", 0),
        ]);
        let command = "execute positioned ~ ~ ~ ";
        let s = complete(&t, command, command.len());
        assert_eq!(s.options, vec!["positioned", "run"]);
        assert_eq!(s.start, command.len());
        assert!(!s.needs_server);
    }

    #[test]
    fn completions_skip_quoted_strings_whole() {
        let t = tree(vec![
            root(vec![1]),
            literal("say2", vec![2], false),
            argument(
                "text",
                BrigadierParser::String(BrigadierString::QuotablePhrase),
                vec![3],
                false,
            ),
            literal("now", vec![], true),
        ]);
        let command = "say2 \"a b\" n";
        let s = complete(&t, command, command.len());
        assert_eq!(s.options, vec!["now"]);
        assert_eq!(s.start, 11);
    }

    #[test]
    fn completions_list_subcommand_literals() {
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

        let all = complete(&t, "time set ", 9);
        assert_eq!(all.options, vec!["day", "midnight", "night", "noon"]);
        // <amount> is an argument sibling: the server should be asked too.
        assert!(all.needs_server);

        let d = complete(&t, "time set d", 10);
        assert_eq!(d.options, vec!["day"]);
        assert_eq!(d.start, 9);
        assert!(d.needs_server);

        let se = complete(&t, "time se", 7);
        assert_eq!(se.options, vec!["set"]);
        assert_eq!(se.start, 5);
        assert!(!se.needs_server);

        // A literal already typed in full isn't offered again.
        assert!(complete(&t, "time set", 8).options.is_empty());

        // Mid-text, only the text up to the caret counts.
        let mid = complete(&t, "time s day", 6);
        assert_eq!(mid.options, vec!["set"]);
        assert_eq!(mid.start, 5);

        let bogus = complete(&t, "bogus foo", 9);
        assert!(bogus.options.is_empty());
        assert!(!bogus.needs_server);
    }

    #[test]
    fn completions_ask_the_server_for_arguments() {
        // root -> "gamemode" -> <gamemode>
        let t = tree(vec![
            root(vec![1]),
            literal("gamemode", vec![2], false),
            argument("gamemode", BrigadierParser::Bool, vec![], true),
        ]);

        let s = complete(&t, "gamemode ", 9);
        assert!(s.options.is_empty());
        assert!(s.needs_server);

        let s = complete(&t, "gamemode c", 10);
        assert!(s.options.is_empty());
        assert_eq!(s.start, 9);
        assert!(s.needs_server);

        assert!(!complete(&t, "gam", 3).needs_server);
    }

    fn check(parser: BrigadierParser, input: &str) -> ArgCheck {
        check_argument(&parser, input, 0)
    }

    fn invalid(key: &'static str, args: &[&str], cursor: usize) -> ArgCheck {
        ArgCheck::Invalid(error(key, args, cursor))
    }

    #[test]
    fn numbers_follow_string_reader_rules() {
        let bounded = || int(Some(0), Some(10));
        assert_eq!(check(bounded(), "5 x"), ArgCheck::Valid(1));
        assert_eq!(
            check(bounded(), "11"),
            invalid("argument.integer.big", &["10", "11"], 0)
        );
        assert_eq!(
            check(bounded(), "-1"),
            invalid("argument.integer.low", &["0", "-1"], 0)
        );
        assert_eq!(
            check(int(None, None), "1.0"),
            invalid("parsing.int.invalid", &["1.0"], 0)
        );
        assert_eq!(
            check(int(None, None), "x"),
            invalid("parsing.int.expected", &[], 0)
        );
        assert_eq!(
            check(
                BrigadierParser::Float(BrigadierNumber::new(Some(0.0), None)),
                "-1.5"
            ),
            invalid("argument.float.low", &["0.0", "-1.5"], 0)
        );
        assert_eq!(check(BrigadierParser::Bool, "true"), ArgCheck::Valid(4));
        assert_eq!(
            check(BrigadierParser::Bool, "yes"),
            invalid("parsing.bool.invalid", &["yes"], 0)
        );
        assert_eq!(
            check(BrigadierParser::Bool, "\"\""),
            invalid("parsing.bool.expected", &[], 2)
        );
        assert_eq!(
            check(BrigadierParser::Time { min: 0 }, "1d"),
            ArgCheck::Valid(2)
        );
        assert_eq!(
            check(BrigadierParser::Time { min: 0 }, "1x"),
            invalid("argument.time.invalid_unit", &[], 2)
        );
        assert_eq!(
            check(BrigadierParser::Time { min: 1 }, "0.4t"),
            invalid("argument.time.tick_count_too_low", &["1", "0"], 4)
        );
    }

    #[test]
    fn java_float_strings() {
        let f = |v: f64| java_float_string(&format!("{v:e}"));
        assert_eq!(f(1234.5), "1234.5");
        assert_eq!(f(1.0), "1.0");
        assert_eq!(f(100.0), "100.0");
        assert_eq!(f(0.001), "0.001");
        assert_eq!(f(0.0001), "1.0E-4");
        assert_eq!(f(1e7), "1.0E7");
        assert_eq!(f(-0.0), "-0.0");
        assert_eq!(f32::MIN.java_string(), "-3.4028235E38");
        assert_eq!(f64::INFINITY.java_string(), "Infinity");
    }

    #[test]
    fn coordinates_follow_world_and_local_rules() {
        assert_eq!(
            check(BrigadierParser::Vec3, "~ ~1 ^"),
            invalid("argument.pos.mixed", &[], 5)
        );
        assert_eq!(check(BrigadierParser::Vec3, "^ ^ ^1"), ArgCheck::Valid(6));
        // An empty absolute coordinate before a space reads as zero.
        assert_eq!(check(BrigadierParser::Vec3, "1  2 3"), ArgCheck::Valid(4));
        assert_eq!(
            check(BrigadierParser::Vec3, "1 2"),
            invalid("argument.pos3d.incomplete", &[], 0)
        );
        assert_eq!(
            check(BrigadierParser::ColumnPos, "~1 2"),
            ArgCheck::Valid(4)
        );
        assert_eq!(
            check(BrigadierParser::BlockPos, "1.5 2 3"),
            invalid("parsing.int.invalid", &["1.5"], 0)
        );
    }

    #[test]
    fn identifiers_strings_and_entities() {
        // Reads nothing; the node then fails on the missing separator.
        assert_eq!(
            check(BrigadierParser::Identifier, "Minecraft:x"),
            ArgCheck::Valid(0)
        );
        assert_eq!(
            check(BrigadierParser::Identifier, "a:b:c"),
            invalid("argument.id.invalid", &[], 0)
        );
        assert_eq!(
            check(BrigadierParser::Identifier, "minecraft:stone"),
            ArgCheck::Valid(15)
        );
        assert_eq!(check(entity(), "@a[tag=x"), ArgCheck::Unknown(None));
        assert_eq!(check(entity(), "@a[tag=x] hi"), ArgCheck::Unknown(Some(9)));
        assert_eq!(check(entity(), "Steve"), ArgCheck::Valid(5));
        assert_eq!(check(entity(), "\"a b\""), ArgCheck::Valid(5));
        assert_eq!(
            check(entity(), "abcdefghijklmnopq"),
            invalid("argument.entity.invalid", &[], 0)
        );
        assert_eq!(
            check(entity(), "\"a"),
            invalid("parsing.quote.expected.end", &[], 2)
        );
        assert_eq!(
            check(entity(), "\"a\\b\""),
            invalid("parsing.quote.escape", &["b"], 3)
        );
        assert_eq!(
            check(BrigadierParser::Message, &"a".repeat(257)),
            ArgCheck::Invalid(SyntaxError {
                key: "argument.message.too_long",
                args: vec!["257".to_owned(), "256".to_owned()],
                cursor: None,
            })
        );
    }
}
