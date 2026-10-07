//! Command language: argv → [`Command`]s — **WP-B** (`docs/spec/cli.md` §3–§9, §11).
//!
//! The parser is state-independent: it reproduces `get_token` and the three argument modes
//! (Mode A key/value lists, Mode B batch lines, Mode C fixed tokens) including their quirks
//! (an empty argv element ends the message; the first token after a domain is never
//! checked for `-`; `--bar`/`--default` stop at a malformed pair and the next token becomes
//! a command; `--set` reports malformed tokens and continues). Messages that depend on
//! state (`Expected <key>=<value> pair` naming the first matched item, unknown items,
//! regex errors) are produced by the runtime when executing the command.
//!
//! Regex selectors (`/…/` for `--set`, `--remove` and bracket members) are POSIX **basic**
//! regular expressions, unanchored: [`bre_to_regex`] translates them for the `regex` crate.

use crate::animation::Curve;

/// An item selector of `--set`/`--remove`/bracket members (`cli.md` §3.5).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Selector {
    Name(String),
    /// The full token **including** the slashes (used verbatim in error messages);
    /// the pattern is `token[1..len-1]`.
    Regex(String),
}

impl Selector {
    /// A token is a regex iff `len > 1 && starts with '/' && ends with '/'`.
    pub fn parse(token: &str) -> Selector {
        if token.len() > 1 && token.starts_with('/') && token.ends_with('/') {
            Selector::Regex(token.to_string())
        } else {
            Selector::Name(token.to_string())
        }
    }

    /// The pattern between the slashes (regex selectors only).
    pub fn pattern(&self) -> Option<&str> {
        match self {
            Selector::Regex(t) => Some(&t[1..t.len() - 1]),
            Selector::Name(_) => None,
        }
    }
}

/// One token of a `--set` list.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SetToken {
    /// Split at the first `=`; the value may be empty or contain `=`.
    Pair { key: String, value: String },
    /// No `=`: `[!] Set (<first item>): Expected <key>=<value> pair, but got: '<tok>'\n`,
    /// then continue.
    Malformed(String),
}

/// Position argument of `--clone` (exactly `before`/`after`, else appended at the end).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Placement {
    Before,
    After,
}

/// `--query <what> [<name>]` (`cli.md` §9).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum QueryTarget {
    Bar,
    Defaults,
    Events,
    Displays,
    DefaultMenuItems,
    /// `--query item <name>`: `[!] Query: Item '<name>' not found\n`.
    Item(String),
    /// Fallback: `[!] Query: Invalid query, or item '<name>' not found \n`.
    Name(String),
    /// Extension `--query stats`.
    Stats,
    /// Extension `--query menus`.
    Menus,
}

/// `--monitor [events|stats|all]` (extension).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MonitorMode {
    Events,
    Stats,
    All,
}

/// `--menubar hide|show|toggle` (extension).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MenuBarAction {
    Hide,
    Show,
    Toggle,
}

/// `--add <type> <name> <position> [args…]` (Mode B). Missing tokens are `""`. For
/// brackets the members are `position` followed by `args`; for graph/slider `args[0]` is the
/// width (missing → 0).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AddCommand {
    pub item_type: String,
    pub name: String,
    pub position: String,
    pub args: Vec<String>,
}

/// One parsed command (`cli.md` §3.4 domain table).
#[derive(Debug, Clone, PartialEq)]
pub enum Command {
    /// `--set <name|/regex/> k=v …` (Mode A after the target). When the selection is
    /// empty, C discards the rest of the batch line instead; that range is exactly
    /// `tokens` (both scans take the first token unconditionally and stop before the next
    /// `-` token), so the runtime just ignores them.
    Set {
        target: Selector,
        tokens: Vec<SetToken>,
    },
    /// `--default k=v …`; `malformed` = the token that ended the list
    /// (`[!] Set (default): Expected <key>=<value> pair, but got: '<tok>'\n`).
    Default {
        pairs: Vec<(String, String)>,
        malformed: Option<String>,
    },
    /// `--bar k=v …`; `malformed` → `[!] Bar: Expected <key>=<value> pair, but got: '<tok>'\n`.
    Bar {
        pairs: Vec<(String, String)>,
        malformed: Option<String>,
    },
    /// `--animate <curve> <duration>` (Mode C, 2 tokens, no `-` check). Applies to the rest
    /// of this message only; duration via `strtoul(…, 0)` (Q9).
    Animate {
        curve: Curve,
        duration: u32,
    },
    /// `--add <type> <name> <position> [args…]`.
    Add(AddCommand),
    /// `--add event <name> [<notification>]` (no response, duplicates ignored).
    AddEvent {
        name: String,
        notification: Option<String>,
    },
    /// `--clone <new name> <parent> [before|after]`.
    Clone {
        name: String,
        parent: String,
        placement: Option<Placement>,
    },
    /// `--subscribe <item> <event>…` (literal name).
    Subscribe {
        item: String,
        events: Vec<String>,
    },
    /// `--push <graph> <v>…` (values via `strtof`).
    Push {
        item: String,
        values: Vec<f32>,
    },
    /// `--update` (Mode C, 0 tokens).
    Update,
    /// `--trigger <event> [K=V…]`; `args` are the raw tokens (see `event::trigger_env`).
    Trigger {
        event: String,
        args: Vec<String>,
    },
    Query(QueryTarget),
    /// `--reorder <name>…`.
    Reorder(Vec<String>),
    /// `--move <name> before|after <reference>`: anything but exactly `before` is after.
    Move {
        item: String,
        before: bool,
        reference: String,
    },
    /// `--remove <name|/regex/>`.
    Remove(Selector),
    /// `--rename <old> <new>`.
    Rename {
        old: String,
        new: String,
    },
    /// `--exit` (no reply). Always the last parsed command: nothing after it can run.
    Exit,
    /// `--hotload <bool token>` (Mode C, 1 token).
    Hotload(String),
    /// `--load-font <path>` (Mode C, 1 token).
    LoadFont(String),
    /// `--reload [<path>]` (Mode B).
    Reload(Option<String>),
    /// Unknown command token: `[!] Unknown domain '<token>'\n`; the rest of its batch line
    /// was skipped.
    UnknownDomain(String),
    /// Extension `--monitor [events|stats|all]` (Mode B; a missing or unknown argument
    /// means `all`).
    Monitor(MonitorMode),
    /// Extension `--menu <index|title>` (Mode B; missing → `""`).
    Menu(String),
    /// Extension `--menubar hide|show|toggle` (`None` = invalid argument).
    MenuBar(Option<MenuBarAction>),
}

/// Cursor over the argv with SketchyBar's `get_token` semantics (`cli.md` §3.1): once the
/// list ends — or an empty argv element is reached — every further token is empty.
#[derive(Debug, Clone)]
pub struct Tokens<'a> {
    args: &'a [String],
    pos: usize,
}

impl<'a> Tokens<'a> {
    pub fn new(args: &'a [String]) -> Self {
        Tokens { args, pos: 0 }
    }

    /// True once the cursor sits on a NUL followed by NUL (end of message, or an empty
    /// argv element was reached): every further token is empty.
    fn ended(&self) -> bool {
        self.pos >= self.args.len() || self.args[self.pos].is_empty()
    }

    /// Moves the cursor to the end of the message.
    fn finish(&mut self) {
        self.pos = self.args.len();
    }

    /// `get_token`: the current token (empty at the end / at an empty element), advancing
    /// only past non-empty tokens.
    ///
    /// C moves the cursor past the token's NUL only if the next byte is not NUL; an empty
    /// argv element (or the terminating double NUL) therefore pins the cursor for good.
    pub fn next_token(&mut self) -> &'a str {
        if self.ended() {
            self.finish();
            return "";
        }
        let args: &'a [String] = self.args;
        let token = args[self.pos].as_str();
        if args.get(self.pos + 1).is_some_and(|next| !next.is_empty()) {
            self.pos += 1;
        } else {
            self.finish();
        }
        token
    }

    /// True if the next token starts with `-` (Mode A stop condition: `message[0] == '-'`).
    pub fn next_starts_with_dash(&self) -> bool {
        !self.ended() && self.args[self.pos].starts_with('-')
    }

    /// `get_batch_line` (Mode B): every token from the cursor up to (excluding) the next
    /// token starting with `-` — the first token is taken even if it starts with `-`.
    /// The batch also ends at an empty argv element (double NUL). The cursor moves to the
    /// `-` token, or to the end.
    pub fn batch_line(&mut self) -> Vec<&'a str> {
        let mut line = Vec::new();
        if self.ended() {
            self.finish();
            return line;
        }
        let args: &'a [String] = self.args;
        line.push(args[self.pos].as_str());
        let mut i = self.pos + 1;
        while let Some(token) = args.get(i) {
            if token.is_empty() || token.starts_with('-') {
                break;
            }
            line.push(token.as_str());
            i += 1;
        }
        match args.get(i) {
            Some(token) if token.starts_with('-') => self.pos = i,
            _ => self.finish(),
        }
        line
    }
}

/// Mode A list of `--bar`/`--default`: stops (and reports) at the first token without `=`.
fn pair_list(t: &mut Tokens) -> (Vec<(String, String)>, Option<String>) {
    let mut pairs = Vec::new();
    let mut token = t.next_token();
    while !token.is_empty() {
        match crate::value::split_key_value(token) {
            Some((k, v)) => pairs.push((k.to_string(), v.to_string())),
            None => return (pairs, Some(token.to_string())),
        }
        if t.next_starts_with_dash() {
            break;
        }
        token = t.next_token();
    }
    (pairs, None)
}

/// Mode A list of `--set` (after the target): malformed tokens are kept and the list goes on.
fn set_tokens(t: &mut Tokens) -> Vec<SetToken> {
    let mut tokens = Vec::new();
    let mut token = t.next_token();
    while !token.is_empty() {
        tokens.push(match crate::value::split_key_value(token) {
            Some((key, value)) => SetToken::Pair {
                key: key.to_string(),
                value: value.to_string(),
            },
            None => SetToken::Malformed(token.to_string()),
        });
        if t.next_starts_with_dash() {
            break;
        }
        token = t.next_token();
    }
    tokens
}

/// Parses a whole message (`handle_message_mach` dispatch loop, `cli.md` §3.2–§3.4).
///
/// Parsing stops after `--exit` (nothing after it can run).
pub fn parse(args: &[String]) -> Vec<Command> {
    let mut t = Tokens::new(args);
    let mut out = Vec::new();
    loop {
        let command = t.next_token();
        if command.is_empty() {
            break;
        }
        let cmd = match command {
            "--set" => {
                let target = Selector::parse(t.next_token());
                let tokens = set_tokens(&mut t);
                Command::Set { target, tokens }
            }
            "--default" => {
                let (pairs, malformed) = pair_list(&mut t);
                Command::Default { pairs, malformed }
            }
            "--bar" => {
                let (pairs, malformed) = pair_list(&mut t);
                Command::Bar { pairs, malformed }
            }
            "--animate" => {
                let curve = Curve::from_token(t.next_token());
                let duration = crate::value::parse_u32(t.next_token());
                Command::Animate { curve, duration }
            }
            "--update" => Command::Update,
            "--exit" => {
                out.push(Command::Exit);
                break;
            }
            "--hotload" => Command::Hotload(t.next_token().to_string()),
            "--load-font" => Command::LoadFont(t.next_token().to_string()),
            _ => {
                let line = t.batch_line();
                batch_command(command, &line)
            }
        };
        out.push(cmd);
    }
    out
}

/// Mode B domains: `line` is the batch line after the command token.
fn batch_command(command: &str, line: &[&str]) -> Command {
    let arg = |i: usize| line.get(i).copied().unwrap_or("").to_string();
    let rest = |from: usize| -> Vec<String> {
        line.iter().skip(from).map(|s| s.to_string()).collect()
    };
    match command {
        "--add" => {
            if line.first() == Some(&"event") {
                // `strlen(message) > 0` after consuming the name: inside a batch line every
                // token is non-empty, so a notification exists iff a third token exists.
                Command::AddEvent {
                    name: arg(1),
                    notification: line.get(2).map(|s| s.to_string()),
                }
            } else {
                Command::Add(AddCommand {
                    item_type: arg(0),
                    name: arg(1),
                    position: arg(2),
                    args: rest(3),
                })
            }
        }
        "--clone" => Command::Clone {
            name: arg(0),
            parent: arg(1),
            placement: match line.get(2).copied() {
                Some("before") => Some(Placement::Before),
                Some("after") => Some(Placement::After),
                _ => None,
            },
        },
        "--subscribe" => Command::Subscribe {
            item: arg(0),
            events: rest(1),
        },
        "--push" => Command::Push {
            item: arg(0),
            values: line
                .iter()
                .skip(1)
                .map(|v| crate::value::parse_float(v))
                .collect(),
        },
        "--trigger" => Command::Trigger {
            event: arg(0),
            args: rest(1),
        },
        "--query" => Command::Query(match line.first().copied().unwrap_or("") {
            "default_menu_items" => QueryTarget::DefaultMenuItems,
            "item" => QueryTarget::Item(arg(1)),
            "bar" => QueryTarget::Bar,
            "defaults" => QueryTarget::Defaults,
            "events" => QueryTarget::Events,
            "displays" => QueryTarget::Displays,
            "stats" => QueryTarget::Stats,
            "menus" => QueryTarget::Menus,
            other => QueryTarget::Name(other.to_string()),
        }),
        "--reorder" => Command::Reorder(rest(0)),
        "--move" => Command::Move {
            item: arg(0),
            before: line.get(1) == Some(&"before"),
            reference: arg(2),
        },
        "--remove" => Command::Remove(Selector::parse(&arg(0))),
        "--rename" => Command::Rename {
            old: arg(0),
            new: arg(1),
        },
        "--reload" => Command::Reload(line.first().map(|s| s.to_string())),
        "--monitor" => Command::Monitor(match line.first().copied() {
            Some("events") => MonitorMode::Events,
            Some("stats") => MonitorMode::Stats,
            _ => MonitorMode::All,
        }),
        "--menu" => Command::Menu(arg(0)),
        "--menubar" => Command::MenuBar(match line.first().copied() {
            Some("hide") => Some(MenuBarAction::Hide),
            Some("show") => Some(MenuBarAction::Show),
            Some("toggle") => Some(MenuBarAction::Toggle),
            _ => None,
        }),
        other => Command::UnknownDomain(other.to_string()),
    }
}

/// A pattern [`bre_to_regex`] cannot translate (`[!] Regex: Could not compile regex
/// '<token>'\n`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RegexError;

impl std::fmt::Display for RegexError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("invalid basic regular expression")
    }
}

impl std::error::Error for RegexError {}

/// `RE_DUP_MAX`: largest bound accepted in `\{m,n\}`.
const RE_DUP_MAX: u32 = 255;

/// Output builder of [`bre_to_regex`]: tracks the last atom so quantifiers can be applied
/// (and stacked quantifiers wrapped, since `a*\?` must not become the lazy `a*?`).
struct BreOut {
    out: String,
    /// Byte offsets of the `(` of every open group.
    groups: Vec<usize>,
    /// Start of the last atom in `out`; `None` at the start of an RE (pattern start, after
    /// `\(`, `\|` or a `^` anchor), where `*` is literal.
    atom: Option<usize>,
    /// The last atom already carries a quantifier.
    quantified: bool,
}

impl BreOut {
    fn literal(&mut self, c: char) {
        self.atom = Some(self.out.len());
        self.quantified = false;
        self.out.push_str(&regex::escape(c.encode_utf8(&mut [0; 4])));
    }

    fn atom(&mut self, s: &str) {
        self.atom = Some(self.out.len());
        self.quantified = false;
        self.out.push_str(s);
    }

    fn quantify(&mut self, q: &str) {
        let Some(start) = self.atom else {
            return;
        };
        if self.quantified {
            self.out.insert_str(start, "(?:");
            self.out.push(')');
        }
        self.out.push_str(q);
        self.quantified = true;
    }

    fn reset(&mut self) {
        self.atom = None;
        self.quantified = false;
    }
}

/// One element of a bracket expression.
enum BracketElem {
    Char(char),
    Class(String),
}

const POSIX_CLASSES: [&str; 12] = [
    "alpha", "digit", "alnum", "upper", "lower", "space", "blank", "punct", "print", "graph",
    "cntrl", "xdigit",
];

/// Reads one bracket element at `i` (`[:class:]`, `[=c=]`, `[.c.]` or a plain char).
fn bracket_elem(chars: &[char], i: usize) -> Result<(BracketElem, usize), RegexError> {
    let c = *chars.get(i).ok_or(RegexError)?;
    if c == '[' {
        if let Some(&kind @ (':' | '=' | '.')) = chars.get(i + 1) {
            let mut j = i + 2;
            while j + 1 < chars.len() && !(chars[j] == kind && chars[j + 1] == ']') {
                j += 1;
            }
            if j + 1 >= chars.len() {
                return Err(RegexError);
            }
            let name: String = chars[i + 2..j].iter().collect();
            let elem = if kind == ':' {
                if !POSIX_CLASSES.contains(&name.as_str()) {
                    return Err(RegexError);
                }
                BracketElem::Class(name)
            } else {
                // Equivalence classes and collating symbols: single characters only.
                let mut it = name.chars();
                match (it.next(), it.next()) {
                    (Some(ch), None) => BracketElem::Char(ch),
                    _ => return Err(RegexError),
                }
            };
            return Ok((elem, j + 2));
        }
    }
    Ok((BracketElem::Char(c), i + 1))
}

/// A literal inside a `regex` character class.
fn class_char(out: &mut String, c: char) {
    if matches!(c, '\\' | '[' | ']' | '-' | '^' | '&' | '~') {
        out.push('\\');
    }
    out.push(c);
}

/// Translates the bracket expression whose `[` is at `chars[i - 1]`; returns the class and
/// the index after the closing `]`.
fn bracket(chars: &[char], mut i: usize) -> Result<(String, usize), RegexError> {
    let mut out = String::from("[");
    if chars.get(i) == Some(&'^') {
        out.push('^');
        i += 1;
    }
    let mut first = true;
    loop {
        let c = *chars.get(i).ok_or(RegexError)?;
        if c == ']' && !first {
            out.push(']');
            return Ok((out, i + 1));
        }
        first = false;
        let (elem, next) = bracket_elem(chars, i)?;
        i = next;
        match elem {
            BracketElem::Class(name) => {
                out.push_str("[:");
                out.push_str(&name);
                out.push_str(":]");
            }
            BracketElem::Char(a) => {
                let is_range =
                    chars.get(i) == Some(&'-') && chars.get(i + 1).is_some_and(|&n| n != ']');
                if is_range {
                    let (end, next) = bracket_elem(chars, i + 1)?;
                    let BracketElem::Char(b) = end else {
                        return Err(RegexError);
                    };
                    if b < a {
                        return Err(RegexError);
                    }
                    class_char(&mut out, a);
                    out.push('-');
                    class_char(&mut out, b);
                    i = next;
                } else {
                    class_char(&mut out, a);
                }
            }
        }
    }
}

/// Parses the body of `\{m,n\}` starting after `\{`; returns the `regex` quantifier and
/// the index after `\}`.
fn interval(chars: &[char], mut i: usize) -> Result<(String, usize), RegexError> {
    let number = |i: &mut usize| -> Option<u32> {
        let start = *i;
        let mut v: u32 = 0;
        while let Some(d) = chars.get(*i).and_then(|c| c.to_digit(10)) {
            v = v.saturating_mul(10).saturating_add(d);
            *i += 1;
        }
        (*i > start).then_some(v)
    };
    let min = number(&mut i).ok_or(RegexError)?;
    let mut q = format!("{{{min}");
    if chars.get(i) == Some(&',') {
        i += 1;
        q.push(',');
        if let Some(max) = number(&mut i) {
            if max < min || max > RE_DUP_MAX {
                return Err(RegexError);
            }
            q.push_str(&max.to_string());
        }
    }
    if min > RE_DUP_MAX || chars.get(i) != Some(&'\\') || chars.get(i + 1) != Some(&'}') {
        return Err(RegexError);
    }
    q.push('}');
    Ok((q, i + 2))
}

/// Translates a POSIX **basic** regular expression (as compiled by `regcomp(&re, p, 0)`)
/// into `regex` crate syntax, unanchored: `\(` `\)` `\{` `\}` `\|` `\+` `\?` are the
/// operators while `(` `)` `{` `}` `|` `+` `?` are literals; `*` at the start of the pattern
/// (or after `\(`/`^`) is literal; `^`/`$` anchor only at the pattern/group edges; bracket
/// expressions incl. `[[:alpha:]]` classes are passed through; back-references `\1`–`\9`
/// are unsupported (`Err`). An empty pattern is an error (macOS `REG_EMPTY`), as are
/// malformed patterns: the caller responds `[!] Regex: Could not compile regex '<token>'\n`.
///
/// Further details: the result starts with `(?s)` (POSIX `.` matches a newline without
/// `REG_NEWLINE`); `\+`/`\?` at the start of an RE are literal like `*`; `\{` there is an
/// error; stacked quantifiers (`a**`, `a*\?`) apply to the quantified atom; any other
/// escaped character is that literal character (`\.`, `\*`, `\[`, `\a` → `a`).
pub fn bre_to_regex(pattern: &str) -> Result<String, RegexError> {
    if pattern.is_empty() {
        return Err(RegexError);
    }
    let chars: Vec<char> = pattern.chars().collect();
    let mut b = BreOut {
        out: String::from("(?s)"),
        groups: Vec::new(),
        atom: None,
        quantified: false,
    };
    // `^` is an anchor only as the first character of an RE.
    let mut re_start = true;
    let mut i = 0;
    while i < chars.len() {
        let c = chars[i];
        let at_start = re_start;
        re_start = false;
        match c {
            '\\' => {
                let e = *chars.get(i + 1).ok_or(RegexError)?;
                i += 2;
                match e {
                    '(' => {
                        b.groups.push(b.out.len());
                        b.out.push('(');
                        b.reset();
                        re_start = true;
                    }
                    ')' => {
                        let start = b.groups.pop().ok_or(RegexError)?;
                        b.out.push(')');
                        b.atom = Some(start);
                        b.quantified = false;
                    }
                    '|' => {
                        b.out.push('|');
                        b.reset();
                        re_start = true;
                    }
                    '{' => {
                        if b.atom.is_none() {
                            return Err(RegexError);
                        }
                        let (q, next) = interval(&chars, i)?;
                        b.quantify(&q);
                        i = next;
                    }
                    '+' | '?' => {
                        if b.atom.is_some() {
                            b.quantify(if e == '+' { "+" } else { "?" });
                        } else {
                            b.literal(e);
                        }
                    }
                    '1'..='9' => return Err(RegexError),
                    other => b.literal(other),
                }
                continue;
            }
            '[' => {
                let (class, next) = bracket(&chars, i + 1)?;
                b.atom(&class);
                i = next;
                continue;
            }
            '.' => b.atom("."),
            '*' => {
                if b.atom.is_some() {
                    b.quantify("*");
                } else {
                    b.literal('*');
                }
            }
            '^' if at_start => {
                b.out.push('^');
                b.reset();
            }
            '$' if i + 1 == chars.len()
                || (chars[i + 1] == '\\' && matches!(chars.get(i + 2), Some(')' | '|'))) =>
            {
                b.out.push('$');
                b.reset();
            }
            other => b.literal(other),
        }
        i += 1;
    }
    if !b.groups.is_empty() {
        return Err(RegexError);
    }
    Ok(b.out)
}
