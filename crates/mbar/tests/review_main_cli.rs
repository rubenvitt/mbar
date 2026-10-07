//! Regression tests for `main.rs` argument handling, run against the real binary:
//!
//! * CLI-7: `sketchybar -h` prints `misc/help.h` verbatim (`cli.md` §1.3).
//! * R7: an argument that is not valid UTF-8 no longer panics the client or the daemon.

use std::ffi::OsString;
use std::os::unix::ffi::OsStringExt;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};
use std::sync::atomic::{AtomicUsize, Ordering};

const EXE: &str = env!("CARGO_BIN_EXE_mbar");

static COUNTER: AtomicUsize = AtomicUsize::new(0);

struct Sandbox {
    root: PathBuf,
}

impl Sandbox {
    fn new() -> Sandbox {
        let root = std::env::temp_dir().join(format!(
            "mbar-review-main-{}-{}",
            std::process::id(),
            COUNTER.fetch_add(1, Ordering::SeqCst)
        ));
        let _ = std::fs::remove_dir_all(&root);
        for d in ["home", "tmp", "bin"] {
            std::fs::create_dir_all(root.join(d)).unwrap();
        }
        std::os::unix::fs::symlink(EXE, root.join("bin/sketchybar")).unwrap();
        Sandbox { root }
    }

    fn run(&self, program: &Path, args: &[OsString]) -> Output {
        Command::new(program)
            .args(args)
            .env_clear()
            .env("HOME", self.root.join("home"))
            .env("TMPDIR", self.root.join("tmp"))
            .env("MBAR_LOCK_DIR", self.root.join("tmp"))
            .env("USER", "mbarreviewmain")
            .env("MBAR_ALLOW_ROOT", "1")
            .output()
            .unwrap()
    }
}

impl Drop for Sandbox {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.root);
    }
}

fn spec_help() -> String {
    let spec = include_str!("../../../docs/spec/cli.md");
    let body = &spec[spec.find("### 1.3 Help text").unwrap()..];
    let open = body.find("```\n").unwrap() + 4;
    let len = body[open..].find("\n```\n").unwrap();
    format!("{}\n", &body[open..open + len])
}

fn assert_no_panic(out: &Output) {
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(!stderr.contains("panicked"), "stderr: {stderr}");
    assert_ne!(out.status.code(), Some(101), "stderr: {stderr}");
}

#[test]
fn sketchybar_help_is_verbatim() {
    let sb = Sandbox::new();
    let program = sb.root.join("bin/sketchybar");
    for flag in ["-h", "--help"] {
        let out = sb.run(&program, &[flag.into()]);
        assert_eq!(out.status.code(), Some(0));
        let expected = spec_help().replacen("%s", &program.to_string_lossy(), 1);
        assert_eq!(String::from_utf8(out.stdout).unwrap(), expected);
    }
}

#[test]
fn mbar_help_keeps_extensions() {
    let sb = Sandbox::new();
    let out = sb.run(Path::new(EXE), &["-h".into()]);
    assert_eq!(out.status.code(), Some(0));
    let text = String::from_utf8(out.stdout).unwrap();
    assert!(text.starts_with(&format!("Usage: {EXE} [options]\n")));
    assert!(text.contains("--headless"));
}

#[test]
fn non_utf8_client_argument_does_not_panic() {
    let sb = Sandbox::new();
    // No daemon is running: the client gives up quietly (exit 0), as SketchyBar does.
    let args = [
        OsString::from("--set"),
        OsString::from("x"),
        OsString::from_vec(b"label=\xff".to_vec()),
    ];
    let out = sb.run(Path::new(EXE), &args);
    assert_no_panic(&out);
    assert_eq!(out.status.code(), Some(0));
}

#[test]
fn non_utf8_config_path_does_not_panic() {
    let sb = Sandbox::new();
    let mut missing = sb.root.clone().into_os_string().into_vec();
    missing.extend_from_slice(b"/missing\xff");
    let out = sb.run(
        Path::new(EXE),
        &[OsString::from("-c"), OsString::from_vec(missing)],
    );
    assert_no_panic(&out);
    assert_eq!(out.status.code(), Some(1));
    assert_eq!(
        String::from_utf8_lossy(&out.stdout),
        "[!] Error: Specified config file path invalid.\n"
    );
}
