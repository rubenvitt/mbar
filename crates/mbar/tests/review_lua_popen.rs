//! Review regression F8: `io.popen("mbar --query bar")` in `init.lua` froze the
//! daemon for the full client timeout (Lua runs on the main thread, the only one
//! that answers IPC) and returned nothing. The client started from a blocking
//! `io.popen` / `os.execute` of the daemon's own Lua now fails at once with an
//! error pointing to `mbar.query` / `mbar.exec`; `mbar.exec` keeps working.

use serde_json::Value;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Output, Stdio};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::{Duration, Instant};

const EXE: &str = env!("CARGO_BIN_EXE_mbar");
const USER: &str = "mbarpopen";

static COUNTER: AtomicUsize = AtomicUsize::new(0);

struct Sandbox {
    root: PathBuf,
}

impl Sandbox {
    fn new() -> Sandbox {
        let root = PathBuf::from("/tmp").join(format!(
            "mbpo-{}-{}",
            std::process::id(),
            COUNTER.fetch_add(1, Ordering::SeqCst)
        ));
        let _ = std::fs::remove_dir_all(&root);
        for d in ["home/.config/mbar", "tmp", "bin"] {
            std::fs::create_dir_all(root.join(d)).unwrap();
        }
        std::os::unix::fs::symlink(EXE, root.join("bin/mbar")).unwrap();
        std::os::unix::fs::symlink(EXE, root.join("bin/sketchybar")).unwrap();
        Sandbox { root }
    }

    fn command(&self, program: &Path) -> Command {
        let mut c = Command::new(program);
        let path = format!(
            "{}:{}",
            self.root.join("bin").display(),
            std::env::var("PATH").unwrap_or_default()
        );
        c.env_clear()
            .env("PATH", path)
            .env("HOME", self.root.join("home"))
            .env("TMPDIR", self.root.join("tmp"))
            .env("MBAR_LOCK_DIR", self.root.join("tmp"))
            .env("USER", USER)
            .env("MBAR_ALLOW_ROOT", "1");
        c
    }

    fn mbar(&self, args: &[&str]) -> Output {
        self.command(Path::new(EXE))
            .args(args)
            .stdin(Stdio::null())
            .output()
            .unwrap()
    }
}

impl Drop for Sandbox {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.root);
    }
}

struct Daemon(Child);

impl Drop for Daemon {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}

fn label(sb: &Sandbox, item: &str) -> Option<String> {
    let out = sb.mbar(&["--query", item]);
    let v: Value = serde_json::from_slice(&out.stdout).ok()?;
    Some(v["label"]["value"].as_str()?.to_owned())
}

#[test]
fn popen_of_own_client_fails_fast_and_exec_still_works() {
    let sb = Sandbox::new();
    std::fs::write(
        sb.root.join("home/.config/mbar/init.lua"),
        r#"
mbar.add("item", "p", {})
mbar.add("item", "s", {})
mbar.add("item", "e", {})
local p = io.popen("mbar --query bar 2>&1; echo rc=$?")
local out = p:read("a"); p:close()
mbar.set("p", { label = out })
local q = io.popen("sketchybar --set p icon=x 2>&1; echo rc=$?")
mbar.set("s", { label = q:read("a") }); q:close()
mbar.exec("mbar --query bar | head -c 1; echo ${MBAR_LUA_SYNC-unset}", function(_, raw)
  mbar.set("e", { label = raw })
end)
"#,
    )
    .unwrap();
    let start = Instant::now();
    let _d = Daemon(
        sb.command(Path::new(EXE))
            .arg("--headless")
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .unwrap(),
    );
    let (p, s, e) = loop {
        if let (Some(p), Some(s), Some(e)) = (label(&sb, "p"), label(&sb, "s"), label(&sb, "e")) {
            if !e.is_empty() {
                break (p, s, e);
            }
        }
        assert!(
            start.elapsed() < Duration::from_secs(15),
            "daemon not ready"
        );
        std::thread::sleep(Duration::from_millis(20));
    };
    // Before the fix each popen waited the 5 s client timeout.
    assert!(
        start.elapsed() < Duration::from_secs(4),
        "config load took {:?}",
        start.elapsed()
    );
    assert!(p.contains("mbar.query"), "{p}");
    assert!(p.ends_with("rc=1\n"), "{p}");
    assert!(s.contains("mbar.query") && s.ends_with("rc=1\n"), "{s}");
    // `mbar.exec` runs asynchronously: not marked, the daemon answers it.
    assert_eq!(e, "{unset\n");
}
