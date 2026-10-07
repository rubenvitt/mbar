//! What the daemon does after reading the appcast: offer the update (open the app),
//! restart itself (a newer bundle is installed on disk), or nothing. Pure; the caller
//! persists [`UpdateState`] in `~/Library/Application Support/mbar/update-state.json`.

use crate::appcast::AppcastItem;

/// At most one offer per version in this many seconds.
pub const OFFER_INTERVAL_SECS: u64 = 86_400;

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct UpdateState {
    pub offered_build: Option<u64>,
    /// Unix seconds.
    pub offered_at: Option<u64>,
    pub restarted_for_build: Option<u64>,
}

impl UpdateState {
    pub fn to_json(&self) -> String {
        serde_json::json!({
            "offered_build": self.offered_build,
            "offered_at": self.offered_at,
            "restarted_for_build": self.restarted_for_build,
        })
        .to_string()
    }

    /// Unreadable or invalid state is the default state (worst case: one extra offer).
    pub fn from_json(s: &str) -> UpdateState {
        let Ok(v) = serde_json::from_str::<serde_json::Value>(s) else {
            return UpdateState::default();
        };
        UpdateState {
            offered_build: v["offered_build"].as_u64(),
            offered_at: v["offered_at"].as_u64(),
            restarted_for_build: v["restarted_for_build"].as_u64(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Action {
    Nothing,
    Offer { build: u64, short_version: String },
    RestartSelf { build: u64 },
}

/// `own_build` is the running daemon's build, `bundle_build` the `CFBundleVersion`
/// currently on disk. A newer bundle on disk means an update was installed but the
/// daemon still runs the old binary: restart, but only once per build so a failed
/// restart never loops. Otherwise offer the newest appcast item newer than both,
/// rate-limited per build.
pub fn decide(
    own_build: u64,
    bundle_build: u64,
    latest: Option<&AppcastItem>,
    state: &UpdateState,
    now: u64,
) -> Action {
    if bundle_build > own_build && state.restarted_for_build != Some(bundle_build) {
        return Action::RestartSelf {
            build: bundle_build,
        };
    }
    // After a failed restart the installed bundle is the baseline: never offer what is
    // already on disk, but still offer anything newer.
    let installed = own_build.max(bundle_build);
    let Some(item) = latest.filter(|i| i.build > installed) else {
        return Action::Nothing;
    };
    // An offer time in the future means the clock went backwards: don't let it
    // suppress offers until the clock catches up.
    let recently = state.offered_build == Some(item.build)
        && state
            .offered_at
            .is_some_and(|t| t <= now && now - t < OFFER_INTERVAL_SECS);
    if recently {
        return Action::Nothing;
    }
    Action::Offer {
        build: item.build,
        short_version: item.short_version.clone(),
    }
}

/// Persist the effect of `action` (taken at `now`) into `state`.
pub fn record(state: &mut UpdateState, action: &Action, now: u64) {
    match action {
        Action::Offer { build, .. } => {
            state.offered_build = Some(*build);
            state.offered_at = Some(now);
        }
        Action::RestartSelf { build } => state.restarted_for_build = Some(*build),
        Action::Nothing => {}
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn item(build: u64) -> AppcastItem {
        AppcastItem {
            build,
            short_version: format!("{build}"),
            minimum_system: None,
        }
    }

    #[test]
    fn offers_newer_version() {
        let s = UpdateState::default();
        assert_eq!(
            decide(1000, 1000, Some(&item(2000)), &s, 100),
            Action::Offer {
                build: 2000,
                short_version: "2000".into()
            }
        );
        assert_eq!(
            decide(2000, 2000, Some(&item(2000)), &s, 100),
            Action::Nothing
        );
        assert_eq!(decide(1000, 1000, None, &s, 100), Action::Nothing);
    }

    #[test]
    fn offer_rate_limited_per_version() {
        let mut s = UpdateState::default();
        let a = decide(1000, 1000, Some(&item(2000)), &s, 100);
        record(&mut s, &a, 100);
        assert_eq!(
            decide(1000, 1000, Some(&item(2000)), &s, 100 + 3600),
            Action::Nothing
        );
        assert!(matches!(
            decide(1000, 1000, Some(&item(2000)), &s, 100 + OFFER_INTERVAL_SECS),
            Action::Offer { .. }
        ));
        // A newer version is offered right away.
        assert!(matches!(
            decide(1000, 1000, Some(&item(3000)), &s, 200),
            Action::Offer { build: 3000, .. }
        ));
    }

    #[test]
    fn restart_once_per_version() {
        let mut s = UpdateState::default();
        let a = decide(1000, 2000, Some(&item(2000)), &s, 100);
        assert_eq!(a, Action::RestartSelf { build: 2000 });
        record(&mut s, &a, 100);
        // Still the old binary (restart failed): never loop.
        assert_eq!(
            decide(1000, 2000, Some(&item(2000)), &s, 200),
            Action::Nothing
        );
        // The state survives the restart via the state file.
        let s = UpdateState::from_json(&s.to_json());
        assert_eq!(
            decide(1000, 2000, Some(&item(2000)), &s, 300),
            Action::Nothing
        );
    }

    #[test]
    fn failed_restart_still_offers_newer_version_only() {
        let s = UpdateState {
            restarted_for_build: Some(2000),
            ..UpdateState::default()
        };
        assert_eq!(
            decide(1000, 2000, Some(&item(2000)), &s, 100),
            Action::Nothing
        );
        assert!(matches!(
            decide(1000, 2000, Some(&item(3000)), &s, 100),
            Action::Offer { build: 3000, .. }
        ));
    }

    #[test]
    fn clock_going_backwards_does_not_block_offers() {
        let mut s = UpdateState::default();
        let a = decide(1000, 1000, Some(&item(2000)), &s, 1_000_000);
        record(&mut s, &a, 1_000_000);
        // Clock set back by a day: the stale future timestamp must not suppress offers.
        let a = decide(1000, 1000, Some(&item(2000)), &s, 1_000_000 - 86_400);
        assert!(matches!(a, Action::Offer { build: 2000, .. }));
        record(&mut s, &a, 1_000_000 - 86_400);
        // ...and the rate limit applies again from the new time.
        assert_eq!(
            decide(1000, 1000, Some(&item(2000)), &s, 1_000_000 - 86_000),
            Action::Nothing
        );
    }

    #[test]
    fn corrupted_state_is_default() {
        for bad in [
            "",
            "{\"offered_build\": 20",
            "[1,2]",
            "null",
            "{\"offered_build\": \"2000\", \"offered_at\": -5, \"restarted_for_build\": 1.5}",
        ] {
            assert_eq!(UpdateState::from_json(bad), UpdateState::default(), "{bad}");
        }
    }

    #[test]
    fn restart_wins_over_offer() {
        let s = UpdateState::default();
        assert_eq!(
            decide(1000, 2000, Some(&item(3000)), &s, 100),
            Action::RestartSelf { build: 2000 }
        );
    }

    #[test]
    fn state_json_roundtrip_and_garbage() {
        let s = UpdateState {
            offered_build: Some(2000),
            offered_at: Some(5),
            restarted_for_build: None,
        };
        assert_eq!(UpdateState::from_json(&s.to_json()), s);
        assert_eq!(UpdateState::from_json("not json"), UpdateState::default());
    }
}
