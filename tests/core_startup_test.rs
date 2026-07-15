#[path = "../src/core_startup.rs"]
mod core_startup;

use core_startup::*;

#[derive(Default)]
struct Config {
    vars: Vec<String>,
    calls: Vec<&'static str>,
    fail: bool,
    refreshed: bool,
}
impl StartupConfig for Config {
    fn add_int_constant(&mut self, n: &str, _: i32) {
        self.vars.push(n.into());
    }
    fn add_empty_variable(&mut self, n: &str) {
        self.vars.push(n.into());
    }
    fn parse(&mut self) -> Result<(), String> {
        self.calls.push("parse");
        if self.fail {
            Err("bad config".into())
        } else {
            Ok(())
        }
    }
    fn start(&mut self) -> Result<(), String> {
        self.calls.push("start");
        Ok(())
    }
    fn refresh_system_variables(&mut self) -> Result<(), String> {
        self.refreshed = true;
        Ok(())
    }
}

#[derive(Default)]
struct Services {
    calls: Vec<&'static str>,
    rollback: usize,
    fail_at: Option<&'static str>,
}
macro_rules! service_impl { ($($name:ident),+ $(,)?) => { $(fn $name(&mut self) -> Result<(), String> { self.calls.push(stringify!($name)); if self.fail_at == Some(stringify!($name)) { Err("service failed".into()) } else { Ok(()) } })+ }; }
impl StartupServices for Services {
    service_impl!(
        lock_memory,
        init_rt_threads,
        register_main_thread,
        init_platform_threads,
        init_sdl,
        init_memory_manager,
        init_event_manager,
        activate_video,
        wait_for_video,
        init_audio,
        init_core_graph,
        init_synth_and_buffers,
        init_loop_and_scene_browsers,
        init_input_and_midi,
        init_osc_and_mixer,
        link_system_variables,
        activate_signal_processing,
        init_streamers_and_finalize_rings,
        add_processing_elements
    );
    fn rollback_setup(&mut self) {
        self.rollback += 1;
        self.calls.push("rollback_setup");
    }
}

#[test]
fn variables_match_startup_shape() {
    let mut c = Config::default();
    install_startup_variables(&mut c, 2, 3);
    assert_eq!(
        &c.vars[..5],
        &[
            "BROWSE_loop",
            "BROWSE_scene",
            "BROWSE_loop_tray",
            "BROWSE_scene_tray",
            "BROWSE_patch"
        ]
    );
    assert!(c.vars.contains(&"SYSTEM_in_2_record".into()));
    assert!(c.vars.contains(&"SYSTEM_loopid_lastrecord_2".into()));
    for name in LIVE_SYSTEM_VARIABLES {
        assert!(STARTUP_SYSTEM_VARIABLES.contains(name), "missing {name}");
    }
}

#[test]
fn setup_preserves_order_and_rolls_back_once() {
    let mut c = Config::default();
    let mut s = Services {
        fail_at: Some("init_audio"),
        ..Default::default()
    };
    let e = setup(&mut c, &mut s, 1, 1).unwrap_err();
    assert_eq!(e.phase, "init_audio");
    assert_eq!(s.rollback, 1);
    assert_eq!(s.calls.last(), Some(&"rollback_setup"));
    assert_eq!(c.calls, vec!["parse"]);
}

#[test]
fn successful_setup_starts_config_before_signal_processing() {
    let mut c = Config::default();
    let mut s = Services::default();
    setup(&mut c, &mut s, 0, 0).unwrap();
    assert_eq!(c.calls, vec!["parse", "start"]);
    assert!(c.refreshed);
    assert_eq!(s.rollback, 0);
    assert_eq!(s.calls.last(), Some(&"add_processing_elements"));
}
