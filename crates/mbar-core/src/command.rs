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
//! regular expressions, unanchored: [`bre_to_regex`] translates them for the `regex` crate and
//! [`compile_bre`] compiles them (with back-reference support).

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
    /// Extension `--query stats` (an existing item named `stats` still wins, see
    /// `query::query`).
    Stats,
    /// Extension `--query menus` (an existing item named `menus` still wins).
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
    let rest =
        |from: usize| -> Vec<String> { line.iter().skip(from).map(|s| s.to_string()).collect() };
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

/// Backtracking budget per item name for patterns with back-references; exceeding it is a
/// match error (`[!] Regex: Regex match failed …`, see `docs/DEVIATIONS.md` D20).
const BACKTRACK_LIMIT: usize = 1_000_000;

/// A repetition count range `{min, max}` (`None` = unbounded).
type Quant = (u32, Option<u32>);

/// `regex` syntax of a repetition.
fn quant_str(q: Quant) -> String {
    match q {
        (0, None) => "*".into(),
        (1, None) => "+".into(),
        (0, Some(1)) => "?".into(),
        (m, None) => format!("{{{m},}}"),
        (m, Some(n)) if m == n => format!("{{{m}}}"),
        (m, Some(n)) => format!("{{{m},{n}}}"),
    }
}

/// `(X{inner}){outer}` as a single `X{…}` when the repetition counts it allows form one
/// contiguous range (`a**` = `a*`, `a*\?` = `a*`, `a\{2\}\{3\}` = `a\{6\}`); `None`
/// otherwise (`a\{3\}\?` allows 0 or 3).
fn compose(inner: Quant, outer: Quant) -> Option<Quant> {
    let ((a, b), (c, d)) = (inner, outer);
    // k repetitions of X{a,b} match X exactly ka..=kb times; the ranges for consecutive
    // k ∈ c..=d touch iff (k+1)a ≤ kb + 1, which is tightest at k = c.
    if d != Some(c) {
        let contiguous = match b {
            None => c >= 1 || a <= 1,
            Some(b) => u64::from(c + 1) * u64::from(a) <= u64::from(c) * u64::from(b) + 1,
        };
        if !contiguous {
            return None;
        }
    }
    let max = match (b, d) {
        (Some(0), _) => Some(0),
        (_, Some(0)) => Some(0),
        (Some(b), Some(d)) => Some(b.saturating_mul(d)),
        _ => None,
    };
    Some((c.saturating_mul(a), max))
}

/// Alternation bookkeeping of one RE level (pattern or `\(…\)`) for back-reference
/// validation, as glibc `regcomp` does it: a group is referable once it is closed, but
/// only within the branch it was closed in (`\(a\)\|\1` is `REG_ESUBREG`).
struct AltFrame {
    /// Groups closed before this level started.
    initial: u16,
    /// Groups closed in the finished branches of this level.
    acc: u16,
}

/// Output builder of [`bre_to_regex`]: tracks the last atom so quantifiers can be applied
/// (and stacked quantifiers merged or wrapped, since `a*\?` must not become the lazy
/// `a*?`).
struct BreOut {
    out: String,
    /// Byte offset of the `(` and the number of every open group.
    groups: Vec<(usize, usize)>,
    /// Start of the last atom in `out`; `None` at the start of an RE (pattern start, after
    /// `\(`, `\|` or a `^` anchor), where `*` is literal.
    atom: Option<usize>,
    /// End of the last atom's text in `out` (where its quantifier begins).
    atom_end: usize,
    /// The combined quantifier of the last atom, if it has one.
    quant: Option<Quant>,
    /// `(?:` still to be inserted at `atom` for stacked quantifiers that could not be
    /// merged. They are inserted once when the atom is finished: inserting each one
    /// immediately shifts the growing atom every time (quadratic in the pattern length).
    wraps: usize,
    /// Number of `\(` so far.
    nsub: usize,
    /// One frame per open RE level (the pattern itself and every open group).
    alts: Vec<AltFrame>,
    /// Bit `n` set: group `n` (1–9) may be back-referenced here.
    completed: u16,
    /// The pattern uses a back-reference.
    backref: bool,
}

impl BreOut {
    /// Emits the `(?:` owed by the last atom; called before anything else is appended.
    fn finish(&mut self) {
        if self.wraps > 0 {
            if let Some(start) = self.atom {
                self.out.insert_str(start, &"(?:".repeat(self.wraps));
            }
            self.wraps = 0;
        }
    }

    fn atom(&mut self, s: &str) {
        self.finish();
        self.atom = Some(self.out.len());
        self.quant = None;
        self.out.push_str(s);
        self.atom_end = self.out.len();
    }

    fn literal(&mut self, c: char) {
        self.atom(&regex::escape(c.encode_utf8(&mut [0; 4])));
    }

    fn quantify(&mut self, q: Quant) {
        if self.atom.is_none() {
            return;
        }
        match self.quant {
            None => {}
            Some(inner) => match compose(inner, q) {
                Some(merged) => {
                    self.out.truncate(self.atom_end);
                    self.out.push_str(&quant_str(merged));
                    self.quant = Some(merged);
                    return;
                }
                None => {
                    self.out.push(')');
                    self.wraps += 1;
                    self.atom_end = self.out.len();
                }
            },
        }
        self.out.push_str(&quant_str(q));
        self.quant = Some(q);
    }

    /// Start of an RE: no atom to quantify.
    fn reset(&mut self) {
        self.finish();
        self.atom = None;
        self.quant = None;
    }

    fn open_group(&mut self) {
        self.reset();
        self.nsub += 1;
        self.groups.push((self.out.len(), self.nsub));
        self.alts.push(AltFrame {
            initial: self.completed,
            acc: 0,
        });
        self.out.push('(');
    }

    fn close_group(&mut self) -> Result<(), RegexError> {
        self.finish();
        let (start, num) = self.groups.pop().ok_or(RegexError)?;
        let frame = self.alts.pop().ok_or(RegexError)?;
        self.completed |= frame.acc;
        if num <= 9 {
            self.completed |= 1 << num;
        }
        self.out.push(')');
        self.atom = Some(start);
        self.atom_end = self.out.len();
        self.quant = None;
        Ok(())
    }

    fn alternation(&mut self) {
        self.reset();
        if let Some(frame) = self.alts.last_mut() {
            frame.acc |= self.completed;
            self.completed = frame.initial;
        }
        self.out.push('|');
    }

    fn backref(&mut self, n: u32) -> Result<(), RegexError> {
        if self.completed & (1 << n) == 0 {
            return Err(RegexError);
        }
        self.backref = true;
        // Wrapped so that a following digit is not read as part of the group number.
        self.atom(&format!("(?:\\{n})"));
        Ok(())
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

/// Parses the body of `\{m,n\}` starting after `\{`; returns the repetition range and the
/// index after `\}`.
fn interval(chars: &[char], mut i: usize) -> Result<(Quant, usize), RegexError> {
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
    let mut max = Some(min);
    if chars.get(i) == Some(&',') {
        i += 1;
        max = number(&mut i);
        if max.is_some_and(|max| max < min || max > RE_DUP_MAX) {
            return Err(RegexError);
        }
    }
    if min > RE_DUP_MAX || chars.get(i) != Some(&'\\') || chars.get(i + 1) != Some(&'}') {
        return Err(RegexError);
    }
    Ok(((min, max), i + 2))
}

/// Translates a POSIX **basic** regular expression (as compiled by `regcomp(&re, p, 0)`)
/// into `regex` crate syntax, unanchored: `\(` `\)` `\{` `\}` `\|` `\+` `\?` are the
/// operators while `(` `)` `{` `}` `|` `+` `?` are literals; `*` at the start of the pattern
/// (or after `\(`/`^`) is literal; `^`/`$` anchor only at the pattern/group edges; bracket
/// expressions incl. `[[:alpha:]]` classes are passed through. Back-references `\1`–`\9`
/// become `\1`–`\9` (only [`compile_bre`] can match those, the `regex` crate cannot); a
/// reference to a group that is not closed yet in the current branch is an error
/// (glibc `REG_ESUBREG`). An empty pattern is an error (macOS `REG_EMPTY`), as are
/// malformed patterns: the caller responds `[!] Regex: Could not compile regex
/// '<token>'\n`.
///
/// Further details: the result starts with `(?s)` (POSIX `.` matches a newline without
/// `REG_NEWLINE`); `\+`/`\?` at the start of an RE are literal like `*`; `\{` there is an
/// error; stacked quantifiers (`a**`, `a*\?`) apply to the quantified atom and are merged
/// into one where possible (`a**` → `a*`); any other escaped character is that literal
/// character (`\.`, `\*`, `\[`, `\a` → `a`). Runs in linear time.
pub fn bre_to_regex(pattern: &str) -> Result<String, RegexError> {
    translate_bre(pattern).map(|(re, _)| re)
}

/// A compiled selector regex (see [`compile_bre`]).
#[derive(Debug)]
pub struct BreRegex(BreEngine);

#[derive(Debug)]
enum BreEngine {
    Plain(regex::Regex),
    /// Patterns with back-references need a backtracking engine.
    Backref(fancy_regex::Regex),
}

/// `regexec` failed for a reason other than no match (`[!] Regex: Regex match failed
/// '<text>'\n`): the backtracking budget of a back-reference pattern ran out (the
/// equivalent of `REG_ESPACE`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RegexMatchError;

impl std::fmt::Display for RegexMatchError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("out of memory")
    }
}

impl std::error::Error for RegexMatchError {}

impl BreRegex {
    /// Unanchored search, like `regexec`.
    pub fn is_match(&self, haystack: &str) -> Result<bool, RegexMatchError> {
        match &self.0 {
            BreEngine::Plain(re) => Ok(re.is_match(haystack)),
            BreEngine::Backref(re) => re.is_match(haystack).map_err(|_| RegexMatchError),
        }
    }
}

/// Compiles a POSIX basic regular expression (`regcomp(&re, p, 0)`) via [`bre_to_regex`].
pub fn compile_bre(pattern: &str) -> Result<BreRegex, RegexError> {
    let (re, backref) = translate_bre(pattern)?;
    let engine = if backref {
        fancy_regex::RegexBuilder::new(&re)
            .backtrack_limit(BACKTRACK_LIMIT)
            .build()
            .map(BreEngine::Backref)
            .map_err(|_| RegexError)?
    } else {
        regex::Regex::new(&re)
            .map(BreEngine::Plain)
            .map_err(|_| RegexError)?
    };
    Ok(BreRegex(engine))
}

/// [`bre_to_regex`], plus whether the pattern uses a back-reference.
fn translate_bre(pattern: &str) -> Result<(String, bool), RegexError> {
    if pattern.is_empty() {
        return Err(RegexError);
    }
    let chars: Vec<char> = pattern.chars().collect();
    let mut b = BreOut {
        out: String::from("(?s)"),
        groups: Vec::new(),
        atom: None,
        atom_end: 0,
        quant: None,
        wraps: 0,
        nsub: 0,
        alts: vec![AltFrame { initial: 0, acc: 0 }],
        completed: 0,
        backref: false,
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
                        b.open_group();
                        re_start = true;
                    }
                    ')' => b.close_group()?,
                    '|' => {
                        b.alternation();
                        re_start = true;
                    }
                    '{' => {
                        if b.atom.is_none() {
                            return Err(RegexError);
                        }
                        let (q, next) = interval(&chars, i)?;
                        b.quantify(q);
                        i = next;
                    }
                    '+' | '?' => {
                        if b.atom.is_some() {
                            b.quantify(if e == '+' { (1, None) } else { (0, Some(1)) });
                        } else {
                            b.literal(e);
                        }
                    }
                    '1'..='9' => b.backref(e.to_digit(10).unwrap_or(0))?,
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
                    b.quantify((0, None));
                } else {
                    b.literal('*');
                }
            }
            '^' if at_start => {
                b.reset();
                b.out.push('^');
            }
            '$' if i + 1 == chars.len()
                || (chars[i + 1] == '\\' && matches!(chars.get(i + 2), Some(')' | '|'))) =>
            {
                b.reset();
                b.out.push('$');
            }
            other => b.literal(other),
        }
        i += 1;
    }
    if !b.groups.is_empty() {
        return Err(RegexError);
    }
    b.finish();
    Ok((b.out, b.backref))
}
