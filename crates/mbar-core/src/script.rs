//! Script environments (`docs/spec/events.md` §4, `docs/spec/cli.md` §12,
//! `docs/spec/item.md` §8.3–8.5).
//!
//! mbar follows D1/D16: every script run gets a **fresh** env = the daemon's startup
//! environment (added by the platform when spawning) + the item's persistent vars + the
//! event's vars + `NAME` + `SENDER`, in SketchyBar's key order (`events.md` §4.4). The
//! persistent-`SENDER` leak (Quirk Q3) is kept: env-less deliveries write `SENDER` into the
//! item's persistent env, so a later `click_script` sees it.
//!
//! [`EnvVars`] (complete) is the ordered map used for item persistent envs and event envs.
//! The builders below produce the per-run env; the runtime decides *whether* a run happens
//! (gating, `script`/`mach_helper` present) and only then calls [`build_update_env`], so the
//! Q3 `SENDER` leak happens exactly when SketchyBar would write it.

/// `struct env_vars`: ordered map where `set` removes an existing key and appends the new
/// pair at the end (`env_vars_set`).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct EnvVars {
    vars: Vec<(String, String)>,
}

impl EnvVars {
    pub fn new() -> Self {
        Self::default()
    }

    /// `env_vars_set`: unset then append.
    pub fn set(&mut self, key: impl Into<String>, value: impl Into<String>) {
        let key = key.into();
        self.vars.retain(|(k, _)| *k != key);
        self.vars.push((key, value.into()));
    }

    pub fn get(&self, key: &str) -> Option<&str> {
        self.vars
            .iter()
            .find(|(k, _)| k == key)
            .map(|(_, v)| v.as_str())
    }

    pub fn remove(&mut self, key: &str) {
        self.vars.retain(|(k, _)| k != key);
    }

    pub fn iter(&self) -> impl Iterator<Item = (&str, &str)> {
        self.vars.iter().map(|(k, v)| (k.as_str(), v.as_str()))
    }

    pub fn len(&self) -> usize {
        self.vars.len()
    }

    pub fn is_empty(&self) -> bool {
        self.vars.is_empty()
    }

    pub fn clear(&mut self) {
        self.vars.clear();
    }

    /// Copies every pair of `other` into `self` with `set` semantics.
    pub fn extend_from(&mut self, other: &EnvVars) {
        for (k, v) in other.iter() {
            self.set(k, v);
        }
    }

    pub fn into_vec(self) -> Vec<(String, String)> {
        self.vars
    }

    pub fn to_vec(&self) -> Vec<(String, String)> {
        self.vars.clone()
    }
}

/// Who asked for an item update (`bar_item_update(item, sender, forced, env)`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Sender {
    /// 1 s clock tick (`SENDER=routine`).
    Routine,
    /// `--update` (`SENDER=forced`).
    Forced,
    /// An event (`SENDER=<name>`), e.g. `mouse.clicked`, `space_change`, a custom event.
    Event(String),
    /// Native provider sample (`SENDER=provider`, extension).
    Provider,
}

impl Sender {
    pub fn as_str(&self) -> &str {
        match self {
            Sender::Routine => "routine",
            Sender::Forced => "forced",
            Sender::Event(e) => e,
            Sender::Provider => "provider",
        }
    }
}

/// Builds the env of one script run (`bar_item_update`, `events.md` §4.2/§4.4) under D1:
///
/// * `event_env == None` (routine, forced, `mouse.entered/exited`, `*.global` enter/exit,
///   `system_woke`, `system_will_sleep`): `SENDER` is written into `persistent` (Q3 leak) and
///   the result is a copy of `persistent`.
/// * otherwise: a copy of `event_env`, then every persistent pair (`set` semantics: item vars
///   override event vars), then `NAME=<name>`, then `SENDER`.
///
/// `name` is the item name (`(null)`-safe: `None` → key omitted).
pub fn build_update_env(
    persistent: &mut EnvVars,
    event_env: Option<&EnvVars>,
    name: Option<&str>,
    sender: &Sender,
) -> EnvVars {
    match event_env {
        None => {
            persistent.set("SENDER", sender.as_str());
            persistent.clone()
        }
        Some(ev) => {
            let mut env = ev.clone();
            env.extend_from(persistent);
            if let Some(n) = name {
                env.set("NAME", n);
            }
            env.set("SENDER", sender.as_str());
            env
        }
    }
}

/// Env of a `click_script` run (`bar_item_on_click`, `events.md` §6.2): the click env
/// (`INFO`, `BUTTON`, `MODIFIER`) with the item's persistent vars copied in (no explicit
/// `NAME`/`SENDER`; `SENDER` may be stale from the persistent env).
pub fn build_click_script_env(click_env: &EnvVars, persistent: &EnvVars) -> EnvVars {
    let mut env = click_env.clone();
    env.extend_from(persistent);
    env
}

/// `env_vars_copy_serialized_representation`: `k\0v\0…` plus one extra `\0` (mach helper
/// payload, `events.md` §4.6).
pub fn serialize_for_mach(env: &EnvVars) -> Vec<u8> {
    let mut out = Vec::new();
    for (k, v) in env.iter() {
        out.extend_from_slice(k.as_bytes());
        out.push(0);
        out.extend_from_slice(v.as_bytes());
        out.push(0);
    }
    out.push(0);
    out
}

/// The 2-byte `"k\0"` message every mach helper gets on exit/reload.
pub const MACH_HELPER_DESTROY: &[u8] = b"k\0";

/// The default script of space items (`bar_item_set_type`; literally `sketchybar`).
pub const DEFAULT_SPACE_SCRIPT: &str = "sketchybar -m --set $NAME icon.highlight=$SELECTED";

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn env_vars_order() {
        let mut e = EnvVars::new();
        e.set("A", "1");
        e.set("B", "2");
        e.set("A", "3");
        assert_eq!(
            e.to_vec(),
            vec![("B".into(), "2".into()), ("A".into(), "3".into())]
        );
        assert_eq!(e.get("A"), Some("3"));
        assert_eq!(
            Sender::Event("mouse.clicked".into()).as_str(),
            "mouse.clicked"
        );
    }
}
