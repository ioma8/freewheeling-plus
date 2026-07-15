use freewheeling_plus::application_services::Components;
use freewheeling_plus::core::{CoreEvent, LoopSnapshot, Snapshot, StreamState};
use freewheeling_plus::core_startup::{StartupConfig, StartupServices};
use freewheeling_plus::fweelin_app::Fweelin;

#[derive(Default)]
struct Config;
impl StartupConfig for Config {
    fn add_int_constant(&mut self, _: &str, _: i32) {}
    fn add_empty_variable(&mut self, _: &str) {}
    fn parse(&mut self) -> Result<(), String> {
        Ok(())
    }
    fn start(&mut self) -> Result<(), String> {
        Ok(())
    }
}
struct Startup;
impl StartupServices for Startup {
    fn rollback_setup(&mut self) {}
    fn lock_memory(&mut self) -> Result<(), String> {
        Ok(())
    }
    fn init_rt_threads(&mut self) -> Result<(), String> {
        Ok(())
    }
    fn register_main_thread(&mut self) -> Result<(), String> {
        Ok(())
    }
    fn init_platform_threads(&mut self) -> Result<(), String> {
        Ok(())
    }
    fn init_sdl(&mut self) -> Result<(), String> {
        Ok(())
    }
    fn init_memory_manager(&mut self) -> Result<(), String> {
        Ok(())
    }
    fn init_event_manager(&mut self) -> Result<(), String> {
        Ok(())
    }
    fn activate_video(&mut self) -> Result<(), String> {
        Ok(())
    }
    fn wait_for_video(&mut self) -> Result<(), String> {
        Ok(())
    }
    fn init_audio(&mut self) -> Result<(), String> {
        Ok(())
    }
    fn init_core_graph(&mut self) -> Result<(), String> {
        Ok(())
    }
    fn init_synth_and_buffers(&mut self) -> Result<(), String> {
        Ok(())
    }
    fn init_loop_and_scene_browsers(&mut self) -> Result<(), String> {
        Ok(())
    }
    fn init_input_and_midi(&mut self) -> Result<(), String> {
        Ok(())
    }
    fn init_osc_and_mixer(&mut self) -> Result<(), String> {
        Ok(())
    }
    fn link_system_variables(&mut self) -> Result<(), String> {
        Ok(())
    }
    fn activate_signal_processing(&mut self) -> Result<(), String> {
        Ok(())
    }
    fn init_streamers_and_finalize_rings(&mut self) -> Result<(), String> {
        Ok(())
    }
    fn add_processing_elements(&mut self) -> Result<(), String> {
        Ok(())
    }
}
struct Services {
    events: Vec<Option<CoreEvent>>,
    state: StreamState,
}
impl Components for Services {
    fn start_session(&mut self) -> Result<(), String> {
        Ok(())
    }
    fn start_interfaces(&mut self) -> Result<(), String> {
        Ok(())
    }
    fn next_event(&mut self) -> Result<Option<CoreEvent>, String> {
        Ok(self.events.pop().unwrap_or(None))
    }
    fn set_streaming(&mut self, e: bool, _: u64) -> Result<(), String> {
        self.state = if e {
            StreamState::Writing
        } else {
            StreamState::Stopped
        };
        Ok(())
    }
    fn stream_state(&self) -> StreamState {
        self.state
    }
    fn stream_bytes(&self) -> u64 {
        11
    }
    fn close_video(&mut self) {}
    fn close_sdl(&mut self) {}
    fn close_midi(&mut self) {}
    fn close_audio(&mut self) {}
    fn shutdown(&mut self) {}
    fn snapshot_loops(&self) -> Vec<LoopSnapshot> {
        vec![]
    }
    fn restore_snapshot(&mut self, _: &Snapshot) -> Result<(), String> {
        Ok(())
    }
}

#[test]
fn façade_runs_ordered_lifecycle_and_controls_streaming() {
    let mut app = Fweelin::new(
        Config,
        Startup,
        Services {
            events: vec![None],
            state: StreamState::Stopped,
        },
        0,
        0,
    );
    app.setup().unwrap();
    app.toggle_disk_output().unwrap();
    assert_eq!(app.stream_stats(), (StreamState::Writing, 11));
    app.flush_stream_out_name().unwrap();
    assert_eq!(app.stream_stats().0, StreamState::Stopped);
    app.go().unwrap();
    assert!(!app.is_running());
}
