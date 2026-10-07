//! Review regression (F1): `mbar.delay` with a finite delay too large for a `Duration`
//! must mean "never", not "now".

use std::time::Duration;

use mbar_lua::{Host, LuaEngine};

#[derive(Default)]
struct Mock {
    scheduled: Vec<(Duration, u64)>,
}

impl Host for Mock {
    fn command(&mut self, _args: Vec<String>) -> String {
        String::new()
    }

    fn spawn_shell(&mut self, _cmd: String, _callback: Option<u64>) {}

    fn schedule(&mut self, delay: Duration, callback: u64) {
        self.scheduled.push((delay, callback));
    }
}

#[test]
fn huge_delays_never_fire_immediately() {
    let mut engine = LuaEngine::new().unwrap();
    let mut host = Mock::default();
    engine
        .load_string(
            r#"
            mbar.delay(1e300, function() end)
            mbar.delay(1e19, function() end)
            mbar.delay(-5, function() end)
            mbar.delay(0.25, function() end)
            "#,
            "=test",
            &mut host,
        )
        .unwrap();
    let delays: Vec<Duration> = host.scheduled.iter().map(|s| s.0).collect();
    assert_eq!(delays.len(), 4);
    assert_eq!(delays[0], Duration::MAX);
    assert_eq!(delays[1], Duration::from_secs_f64(1e19));
    assert_eq!(delays[2], Duration::ZERO);
    assert_eq!(delays[3], Duration::from_millis(250));
}
