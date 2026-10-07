//! Review regressions for regex selectors (`cli.md` §3.5): BRE back-references (CLI-3)
//! and linear-time translation of stacked quantifiers (R5).
mod wpc_common;
use mbar_core::command::{bre_to_regex, compile_bre};
use std::time::{Duration, Instant};
use wpc_common::H;

fn matches(pattern: &str, yes: &[&str], no: &[&str]) {
    let re = compile_bre(pattern).unwrap_or_else(|_| panic!("{pattern:?} should compile"));
    for y in yes {
        assert_eq!(re.is_match(y), Ok(true), "{pattern:?} should match {y:?}");
    }
    for n in no {
        assert_eq!(
            re.is_match(n),
            Ok(false),
            "{pattern:?} should not match {n:?}"
        );
    }
}

// --- CLI-3: back-references -------------------------------------------------------------

#[test]
fn backreferences_match_like_regexec() {
    // Expectations checked against glibc regcomp(&re, p, 0) / regexec.
    matches("\\(a\\)\\1", &["aa", "aaa", "xaab"], &["a", "ab", ""]);
    matches(
        "^\\(.*\\)\\1$",
        &["", "aa", "abab", "abcabc"],
        &["aba", "ab"],
    );
    // `\1` followed by a digit is the back-reference and a literal digit, not `\10`.
    matches("\\(a\\)\\10", &["aa0"], &["aa", "a0"]);
    matches("\\(\\(a\\)b\\)\\2", &["aba", "abab"], &["ab", "abb"]);
    matches("^\\(a\\)\\1\\{2\\}$", &["aaa"], &["aa", "aaaa"]);
    matches("^\\(a\\|b\\)\\1$", &["aa", "bb"], &["ab", "ba"]);
    matches("\\(\\(a\\)\\|b\\)\\2", &["aa"], &["ba", "bb"]);
    matches(
        "^\\(ab*\\)\\1*$",
        &["ab", "abbabb", "aaa"],
        &["abab b", "aab"],
    );
    matches(
        "\\(a\\)\\(b\\)\\(c\\)\\(d\\)\\(e\\)\\(f\\)\\(g\\)\\(h\\)\\(i\\)\\9",
        &["abcdefghii"],
        &["abcdefghi"],
    );
    // Other BRE features keep working next to a back-reference.
    matches(
        "^\\([[:alpha:]]\\)[.*+?(){}|&~-]\\1$",
        &["a.a", "x|x", "b-b", "c~c"],
        &["a.b", "1.1"],
    );
    matches("^\\(a\\)\\1\\?$", &["a", "aa"], &["aaa"]);
    matches("^\\(a\\)\\1**$", &["a", "aaaa"], &["b"]);
}

#[test]
fn invalid_backreferences_are_compile_errors() {
    // glibc: REG_ESUBREG unless the group is closed earlier in the same branch.
    for bad in [
        "\\1",
        "\\1\\(a\\)",
        "\\(a\\1\\)",
        "\\(a\\)\\2",
        "\\(a\\)\\|\\1",
        "\\(a\\)\\|b\\1",
    ] {
        assert!(compile_bre(bad).is_err(), "{bad:?} should be rejected");
        assert!(bre_to_regex(bad).is_err(), "{bad:?} should be rejected");
    }
    for good in [
        "\\(a\\)\\|b",
        "\\(\\(a\\)\\|b\\)\\2",
        "\\(a\\)\\(b\\|\\1\\)",
    ] {
        assert!(compile_bre(good).is_ok(), "{good:?} should compile");
    }
}

#[test]
fn backreference_selectors_in_set_remove_and_brackets() {
    let mut h = H::new();
    h.msg(&[
        "--add", "item", "aa", "left", "--add", "item", "ab", "left", "--add", "item", "bb", "left",
    ]);
    assert_eq!(h.msg(&["--set", "/\\(a\\)\\1/", "label=x"]), "");
    assert_eq!(h.query(&["aa"])["label"]["value"], "x");
    assert_eq!(h.query(&["ab"])["label"]["value"], "");
    assert_eq!(h.query(&["bb"])["label"]["value"], "");

    assert_eq!(h.msg(&["--add", "bracket", "pair", "/^\\(.\\)\\1$/"]), "");
    let members = h.query(&["pair"])["bracket"].clone();
    assert_eq!(members, serde_json::json!(["aa", "bb"]));

    assert_eq!(
        h.msg(&["--set", "/\\(a\\)\\2/", "label=y"]),
        "[!] Regex: Could not compile regex '/\\(a\\)\\2/'\n"
    );
    assert_eq!(h.msg(&["--remove", "/^\\(.\\)\\1$/"]), "");
    assert!(h.msg(&["--query", "aa"]).contains("not found"));
    assert_eq!(h.query(&["ab"])["name"], "ab");
}

#[test]
fn backtracking_blowup_is_a_match_error() {
    let mut h = H::new();
    h.msg(&["--add", "item", "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaab", "left"]);
    let rsp = h.msg(&["--set", "/^\\(\\(a*\\)*\\)*\\1\\2c$/", "label=x"]);
    assert_eq!(rsp, "[!] Regex: Regex match failed 'out of memory'\n");
}

// --- R5: stacked quantifiers --------------------------------------------------------------

#[test]
fn stacked_quantifiers_are_merged() {
    for (pattern, expected) in [
        ("a**", "(?s)a*"),
        ("a*\\?", "(?s)a*"),
        ("a\\+*", "(?s)a*"),
        ("a\\+\\+", "(?s)a+"),
        ("a\\?\\+", "(?s)a*"),
        ("a\\?\\?", "(?s)a?"),
        ("a\\{2\\}\\{3\\}", "(?s)a{6}"),
        ("a\\{2,3\\}\\{2\\}", "(?s)a{4,6}"),
        ("a\\{1,2\\}\\{2,\\}", "(?s)a{2,}"),
        // 0 or 3 repetitions is not a range: wrapped.
        ("a\\{3\\}\\?", "(?s)(?:a{3})?"),
        ("a\\{3\\}\\?b", "(?s)(?:a{3})?b"),
        ("\\(ab\\)\\{2\\}\\{1,2\\}", "(?s)(?:(ab){2}){1,2}"),
        ("a\\{2\\}\\{1,2\\}\\{3\\}\\?", "(?s)(?:(?:a{2}){3,6})?"),
    ] {
        assert_eq!(
            bre_to_regex(pattern).as_deref(),
            Ok(expected),
            "{pattern:?}"
        );
    }
    let re = compile_bre("^a\\{3\\}\\?$").unwrap();
    assert_eq!(re.is_match(""), Ok(true));
    assert_eq!(re.is_match("aaa"), Ok(true));
    assert_eq!(re.is_match("a"), Ok(false));
}

#[test]
fn many_stacked_quantifiers_translate_in_linear_time() {
    // Was O(n²): 400k stars took ~11 s in a release build.
    let pattern = format!("a{}", "*".repeat(1_000_000));
    let t = Instant::now();
    assert_eq!(bre_to_regex(&pattern).as_deref(), Ok("(?s)a*"));
    let mut h = H::new();
    h.msg(&["--add", "item", "aaa", "left"]);
    assert_eq!(h.msg(&["--set", &format!("/{pattern}/"), "label=x"]), "");
    assert_eq!(h.query(&["aaa"])["label"]["value"], "x");

    // Stacks that cannot be merged are wrapped once per atom, not once per quantifier.
    let pattern = "b\\{3\\}\\?".repeat(200_000);
    assert!(bre_to_regex(&pattern).is_ok());
    let pattern = format!("a\\{{2\\}}{}", "\\{1,2\\}\\{2\\}".repeat(200_000));
    assert!(bre_to_regex(&pattern).is_ok());
    assert!(
        t.elapsed() < Duration::from_secs(20),
        "took {:?}",
        t.elapsed()
    );
}
