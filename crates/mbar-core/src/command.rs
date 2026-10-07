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
    /// `--set <name|/regex/> k=v …` (Mode A after the target).
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
    /// `--exit` (no reply).
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
    /// Extension `--monitor`.
    Monitor(MonitorMode),
    /// Extension `--menu <index|title>`.
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

    /// `get_token`: the current token (empty at the end / at an empty element), advancing
    /// only past non-empty tokens.
    pub fn next_token(&mut self) -> &'a str {
        let _ = &self.args;
        todo!("WP-B: cli.md §3.1")
    }

    /// True if the next token starts with `-` (Mode A stop condition).
    pub fn next_starts_with_dash(&self) -> bool {
        todo!("WP-B: cli.md §3.2")
    }

    /// `get_batch_line` (Mode B): every token from the cursor up to (excluding) the next
    /// token starting with `-` — the first token is taken even if it starts with `-`.
    pub fn batch_line(&mut self) -> Vec<&'a str> {
        todo!("WP-B: cli.md §3.2")
    }
}

/// Parses a whole message (`handle_message_mach` dispatch loop, `cli.md` §3.2–§3.4).
pub fn parse(args: &[String]) -> Vec<Command> {
    let _ = args;
    todo!("WP-B: cli.md §3")
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

/// Translates a POSIX **basic** regular expression (as compiled by `regcomp(&re, p, 0)`)
/// into `regex` crate syntax, unanchored: `\(` `\)` `\{` `\}` `\|` `\+` `\?` are the
/// operators while `(` `)` `{` `}` `|` `+` `?` are literals; `*` at the start of the pattern
/// (or after `\(`/`^`) is literal; `^`/`$` anchor only at the pattern/group edges; bracket
/// expressions incl. `[[:alpha:]]` classes are passed through; back-references `\1`–`\9`
/// are unsupported (`Err`). An empty pattern is an error (macOS `REG_EMPTY`), as are
/// malformed patterns: the caller responds `[!] Regex: Could not compile regex '<token>'\n`.
pub fn bre_to_regex(pattern: &str) -> Result<String, RegexError> {
    let _ = pattern;
    todo!("WP-B: cli.md §3.5")
}
