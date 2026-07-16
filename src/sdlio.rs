//! SDL input translation and state management.

use std::cell::RefCell;

/// The SDL library handle shared by the window and input adapters.
///
/// `Sdl` is deliberately kept on the thread that created it: SDL2's event
/// pump (and Cocoa's window integration) must be used from that thread.  The
/// per-thread slot also makes the old constructors safe for native assembly,
/// which constructs video and input independently.
#[derive(Clone)]
pub struct Sdl2Context {
    sdl: sdl2::Sdl,
}

thread_local! {
    static CONTEXT: RefCell<Option<Sdl2Context>> = const { RefCell::new(None) };
}

impl Sdl2Context {
    pub fn new() -> Result<Self, String> {
        let context = Self {
            sdl: sdl2::init().map_err(|e| format!("SDL initialization failed: {e}"))?,
        };
        CONTEXT.with(|slot| *slot.borrow_mut() = Some(context.clone()));
        Ok(context)
    }

    pub fn shared() -> Result<Self, String> {
        CONTEXT.with(|slot| {
            slot.borrow()
                .clone()
                .ok_or_else(|| "SDL context has not been initialized on this thread".into())
        })
    }

    pub(crate) fn sdl(&self) -> &sdl2::Sdl {
        &self.sdl
    }
}

pub const SDLK_LAST: usize = 323;
pub const SDLK_UNKNOWN: i32 = 0;
pub const FWL_SDLK_UNKNOWN: i32 = SDLK_UNKNOWN;

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum SdlEvent {
    Quit,
    JoystickButton {
        joystick: i32,
        button: i32,
        down: bool,
    },
    MouseMotion {
        x: i32,
        y: i32,
    },
    MouseButton {
        button: i32,
        x: i32,
        y: i32,
        down: bool,
    },
    Key {
        keycode: i32,
        down: bool,
        repeat: bool,
    },
    Text(String),
}

pub trait SdlBackend {
    fn poll_event(&mut self, timeout_ms: u32) -> Option<SdlEvent>;
    fn start_text_input(&mut self) {}
    fn stop_text_input(&mut self) {}
    fn shutdown(&mut self) {}
}

/// Production SDL2 event backend. Joystick handles remain alive for the
/// backend lifetime so button events continue to be delivered.
pub struct Sdl2InputBackend {
    _sdl: Sdl2Context,
    video: sdl2::VideoSubsystem,
    event_pump: sdl2::EventPump,
    joysticks: Vec<sdl2::joystick::Joystick>,
}

impl Sdl2InputBackend {
    pub fn new() -> Result<Self, String> {
        let context = Sdl2Context::shared().or_else(|_| Sdl2Context::new())?;
        Self::new_with_context(context)
    }

    pub fn new_with_context(context: Sdl2Context) -> Result<Self, String> {
        let sdl = context.sdl();
        let video = sdl
            .video()
            .map_err(|e| format!("SDL video initialization failed: {e}"))?;
        let joystick = sdl
            .joystick()
            .map_err(|e| format!("SDL joystick initialization failed: {e}"))?;
        joystick.set_event_state(true);
        let mut joysticks = Vec::new();
        for index in 0..joystick
            .num_joysticks()
            .map_err(|e| format!("SDL joystick discovery failed: {e}"))?
        {
            match joystick.open(index) {
                Ok(handle) => joysticks.push(handle),
                Err(error) => eprintln!("SDL: cannot open joystick {index}: {error}"),
            }
        }
        let event_pump = sdl
            .event_pump()
            .map_err(|e| format!("SDL event pump initialization failed: {e}"))?;
        Ok(Self {
            _sdl: context,
            video,
            event_pump,
            joysticks,
        })
    }
    pub fn joystick_count(&self) -> usize {
        self.joysticks.len()
    }
}

impl SdlBackend for Sdl2InputBackend {
    fn poll_event(&mut self, timeout_ms: u32) -> Option<SdlEvent> {
        use sdl2::event::Event;
        loop {
            let event = self.event_pump.wait_event_timeout(timeout_ms)?;
            if std::env::var_os("FWEELIN_DIAGNOSTICS").is_some() {
                eprintln!("FreeWheeling SDL event: {event:?}");
            }
            let translated = match event {
                Event::Quit { .. } => Some(SdlEvent::Quit),
                Event::JoyButtonDown {
                    which, button_idx, ..
                } => Some(SdlEvent::JoystickButton {
                    joystick: which as i32,
                    button: button_idx as i32,
                    down: true,
                }),
                Event::JoyButtonUp {
                    which, button_idx, ..
                } => Some(SdlEvent::JoystickButton {
                    joystick: which as i32,
                    button: button_idx as i32,
                    down: false,
                }),
                Event::MouseMotion { x, y, .. } => Some(SdlEvent::MouseMotion { x, y }),
                Event::MouseButtonDown {
                    mouse_btn, x, y, ..
                } => Some(SdlEvent::MouseButton {
                    button: mouse_btn as u8 as i32,
                    x,
                    y,
                    down: true,
                }),
                Event::MouseButtonUp {
                    mouse_btn, x, y, ..
                } => Some(SdlEvent::MouseButton {
                    button: mouse_btn as u8 as i32,
                    x,
                    y,
                    down: false,
                }),
                Event::MouseWheel {
                    y: scroll_y,
                    precise_y,
                    mouse_x,
                    mouse_y,
                    ..
                } if scroll_y != 0 || precise_y != 0.0 => Some(SdlEvent::MouseButton {
                    // SDL1 exposed wheel motion to the original C++ event
                    // path as button 4/5. Preserve that contract so the
                    // XML `loop-clicked` bindings adjust loop gain.
                    button: if scroll_y > 0 || (scroll_y == 0 && precise_y > 0.0) {
                        4
                    } else {
                        5
                    },
                    x: mouse_x,
                    y: mouse_y,
                    down: true,
                }),
                Event::KeyDown {
                    keycode: Some(keycode),
                    keymod,
                    repeat,
                    ..
                } => {
                    // The C++ SDL adapter explicitly turns Command-Q into a
                    // Quit event on macOS, before emitting the key event.
                    // SDL does not guarantee this conversion across Cocoa
                    // versions, so keep the legacy behavior explicit.
                    #[cfg(target_os = "macos")]
                    if keycode == sdl2::keyboard::Keycode::Q
                        && keymod
                            .intersects(sdl2::keyboard::Mod::LGUIMOD | sdl2::keyboard::Mod::RGUIMOD)
                    {
                        Some(SdlEvent::Quit)
                    } else {
                        Some(SdlEvent::Key {
                            keycode: i32::from(keycode),
                            down: true,
                            repeat,
                        })
                    }
                    #[cfg(not(target_os = "macos"))]
                    {
                        let _ = keymod;
                        Some(SdlEvent::Key {
                            keycode: i32::from(keycode),
                            down: true,
                            repeat,
                        })
                    }
                }
                Event::KeyUp {
                    keycode: Some(keycode),
                    repeat,
                    ..
                } => Some(SdlEvent::Key {
                    keycode: i32::from(keycode),
                    down: false,
                    repeat,
                }),
                Event::TextInput { text, .. } => Some(SdlEvent::Text(text)),
                _ => None,
            };
            if translated.is_some() {
                return translated;
            }
        }
    }
    fn start_text_input(&mut self) {
        self.video.text_input().start();
    }
    fn stop_text_input(&mut self) {
        self.video.text_input().stop();
    }
    fn shutdown(&mut self) {
        self.video.text_input().stop();
        self.joysticks.clear();
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum InputEvent {
    Quit,
    JoystickButton {
        joystick: i32,
        button: i32,
        down: bool,
    },
    MouseMotion {
        x: i32,
        y: i32,
    },
    MouseButton {
        button: i32,
        x: i32,
        y: i32,
        down: bool,
    },
    Key {
        down: bool,
        keysym: i32,
        unicode: i32,
    },
    /// Text committed by SDL's text input system.  Production SDL input is
    /// normalized to the C++ adapter's one Latin-1 `Key` event, but retaining
    /// this variant keeps the public, backend-neutral adapter usable by
    /// callers that inject text directly.
    Text(String),
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct KeySettings {
    pub leftshift: bool,
    pub rightshift: bool,
    pub leftctrl: bool,
    pub rightctrl: bool,
    pub leftalt: bool,
    pub rightalt: bool,
    pub upkey: bool,
    pub downkey: bool,
    pub spacekey: bool,
}

pub struct SdlIo<B: SdlBackend> {
    backend: B,
    active: bool,
    unicode_input_enabled: bool,
    key_repeat_enabled: bool,
    held: [bool; SDLK_LAST],
    pub settings: KeySettings,
    previous_key: i32,
    repeat_count: i32,
    pending_pulse_subdivide: Option<u32>,
}

impl<B: SdlBackend> SdlIo<B> {
    pub fn new(backend: B) -> Self {
        Self {
            backend,
            active: false,
            unicode_input_enabled: false,
            key_repeat_enabled: false,
            held: [false; SDLK_LAST],
            settings: KeySettings::default(),
            previous_key: -1,
            repeat_count: 0,
            pending_pulse_subdivide: None,
        }
    }
    pub fn activate(&mut self) -> i32 {
        self.active = true;
        0
    }
    pub fn close(&mut self) {
        if self.unicode_input_enabled {
            self.backend.stop_text_input();
        }
        self.backend.shutdown();
        self.held.fill(false);
        self.settings = KeySettings::default();
        self.active = false;
    }
    pub fn is_active(&self) -> bool {
        self.active
    }
    pub fn keys_held(&self) -> &[bool; SDLK_LAST] {
        &self.held
    }
    pub fn enable_unicode(&mut self, enable: bool) {
        self.unicode_input_enabled = enable;
        if enable {
            self.backend.start_text_input()
        } else {
            self.backend.stop_text_input()
        }
    }
    pub fn enable_key_repeat(&mut self, enable: bool) {
        self.key_repeat_enabled = enable;
    }
    /// Consume C++ SDLIO's direct Shift+F1..F10 pulse-subdivision side
    /// effect.  It is kept separate from the configuration key event because
    /// the original invokes `LoopManager::SetSubdivide` directly.
    pub fn take_pulse_subdivide(&mut self) -> Option<u32> {
        self.pending_pulse_subdivide.take()
    }
    pub fn poll(&mut self) -> Option<InputEvent> {
        if !self.active {
            return None;
        }
        // C++ waits for 100 ms on a dedicated SDL thread.  This adapter runs
        // on the Cocoa/UI thread, where the same wait stalls rendering and
        // event dispatch, so only wait long enough to yield the CPU briefly.
        let event = self.backend.poll_event(1)?;
        match event {
            SdlEvent::Quit => {
                self.active = false;
                Some(InputEvent::Quit)
            }
            SdlEvent::JoystickButton {
                joystick,
                button,
                down,
            } => Some(InputEvent::JoystickButton {
                joystick,
                button,
                down,
            }),
            SdlEvent::MouseMotion { x, y } => Some(InputEvent::MouseMotion { x, y }),
            SdlEvent::MouseButton { button, x, y, down } => {
                Some(InputEvent::MouseButton { button, x, y, down })
            }
            SdlEvent::Key {
                keycode,
                down,
                repeat,
            } => {
                if down && repeat && !self.key_repeat_enabled {
                    return None;
                }
                let key = translate_keycode(keycode);
                if !(0..SDLK_LAST as i32).contains(&key) {
                    return None;
                }
                self.held[key as usize] = down;
                self.update_settings(key, down);
                if down {
                    if key == self.previous_key {
                        self.repeat_count += 1
                    } else {
                        self.repeat_count = 0;
                        self.previous_key = key;
                    }
                    if (282..=291).contains(&key)
                        && (self.settings.leftshift || self.settings.rightshift)
                    {
                        self.pending_pulse_subdivide = Some(
                            (self.repeat_count.max(0) as u32 + 1)
                                .saturating_mul((key - 282 + 1) as u32),
                        );
                    }
                }
                Some(InputEvent::Key {
                    down,
                    keysym: key,
                    unicode: 0,
                })
            }
            // `DecodeTextInputUnicode` in fweelin_sdlio.cc deliberately
            // consumes exactly one UTF-8 scalar and only accepts the legacy
            // SDL1/Latin-1 range.  It posts that scalar as a pressed key,
            // rather than a separate text event.  Keep this at the native
            // boundary so the runtime receives precisely the C++ event
            // sequence (including ignoring supplementary Unicode text).
            SdlEvent::Text(text) if self.unicode_input_enabled => {
                let unicode = decode_legacy_text_input(&text)?;
                Some(InputEvent::Key {
                    down: true,
                    keysym: unicode,
                    unicode,
                })
            }
            SdlEvent::Text(_) => None,
        }
    }
    fn update_settings(&mut self, key: i32, down: bool) {
        match key {
            32 => self.settings.spacekey = down,
            273 => self.settings.upkey = down,
            274 => self.settings.downkey = down,
            303 => self.settings.rightshift = down,
            304 => self.settings.leftshift = down,
            305 => self.settings.rightctrl = down,
            306 => self.settings.leftctrl = down,
            307 => self.settings.rightalt = down,
            308 => self.settings.leftalt = down,
            _ => {}
        }
    }
}

/// C++ `DecodeTextInputUnicode` semantics for SDL's valid UTF-8 text payload.
///
/// The old event API only has an `int unicode` field and the original port
/// intentionally supports ASCII plus the two-byte Unicode values that fit in
/// an SDL1-compatible byte key range (0..=255).  It discards all remaining
/// scalars and values above Latin-1.
fn decode_legacy_text_input(text: &str) -> Option<i32> {
    let first = text.chars().next()? as u32;
    (first <= u32::from(u8::MAX)).then_some(first as i32)
}

impl<B: SdlBackend> Drop for SdlIo<B> {
    fn drop(&mut self) {
        self.close();
    }
}

/// Verbatim transcription of the original `SDL_names[]` table
/// (`fweelin_sdlio.cc`), indexed by SDL1-compatible keysym. This table, not
/// ASCII intuition, is the source of truth: e.g. indices 65-90 (uppercase
/// letters) are unnamed in the original -- only 97-122 (lowercase) name
/// `"a"`..`"z"` -- and index 39 is named `"backquote"` despite being the
/// apostrophe position. `key_from_name` must reproduce the original's
/// linear, first-match `GetSDLKey` scan exactly: several XML-bound key
/// names (punctuation, keypad symbols) exist only past index 90, and a
/// spurious match anywhere ahead of the intended one would silently bind
/// the wrong keysym to an XML `key=` condition.
const SDL_NAMES: [&str; 323] = [
    "",
    "",
    "",
    "",
    "",
    "",
    "",
    "",
    "backspace",
    "tab",
    "",
    "",
    "clear",
    "return",
    "",
    "",
    "",
    "",
    "",
    "pause",
    "",
    "",
    "",
    "",
    "",
    "",
    "",
    "escape",
    "",
    "",
    "",
    "",
    "space",
    "exclamation",
    "dblquote",
    "numbersign",
    "dollarsign",
    "",
    "ampersand",
    "backquote",
    "openparen",
    "closeparen",
    "asterisk",
    "plus",
    "comma",
    "minus",
    "period",
    "slash",
    "zero",
    "one",
    "two",
    "three",
    "four",
    "five",
    "six",
    "seven",
    "eight",
    "nine",
    "colon",
    "semicolon",
    "lessthan",
    "equal",
    "greaterthan",
    "questionmark",
    "at",
    "",
    "",
    "",
    "",
    "",
    "",
    "",
    "",
    "",
    "",
    "",
    "",
    "",
    "",
    "",
    "",
    "",
    "",
    "",
    "",
    "",
    "",
    "",
    "",
    "",
    "",
    "squarebracketopen",
    "backslash",
    "squarebracketclose",
    "caret",
    "underscore",
    "tilde",
    "a",
    "b",
    "c",
    "d",
    "e",
    "f",
    "g",
    "h",
    "i",
    "j",
    "k",
    "l",
    "m",
    "n",
    "o",
    "p",
    "q",
    "r",
    "s",
    "t",
    "u",
    "v",
    "w",
    "x",
    "y",
    "z",
    "",
    "",
    "",
    "",
    "delete",
    "",
    "",
    "",
    "",
    "",
    "",
    "",
    "",
    "",
    "",
    "",
    "",
    "",
    "",
    "",
    "",
    "",
    "",
    "",
    "",
    "",
    "",
    "",
    "",
    "",
    "",
    "",
    "",
    "",
    "",
    "",
    "",
    "world0",
    "world1",
    "world2",
    "world3",
    "world4",
    "world5",
    "world6",
    "world7",
    "world8",
    "world9",
    "world10",
    "world11",
    "world12",
    "world13",
    "world14",
    "world15",
    "world16",
    "world17",
    "world18",
    "world19",
    "world20",
    "world21",
    "world22",
    "world23",
    "world24",
    "world25",
    "world26",
    "world27",
    "world28",
    "world29",
    "world30",
    "world31",
    "world32",
    "world33",
    "world34",
    "world35",
    "world36",
    "world37",
    "world38",
    "world39",
    "world40",
    "world41",
    "world42",
    "world43",
    "world44",
    "world45",
    "world46",
    "world47",
    "world48",
    "world49",
    "world50",
    "world51",
    "world52",
    "world53",
    "world54",
    "world55",
    "world56",
    "world57",
    "world58",
    "world59",
    "world60",
    "world61",
    "world62",
    "world63",
    "world64",
    "world65",
    "world66",
    "world67",
    "world68",
    "world69",
    "world70",
    "world71",
    "world72",
    "world73",
    "world74",
    "world75",
    "world76",
    "world77",
    "world78",
    "world79",
    "world80",
    "world81",
    "world82",
    "world83",
    "world84",
    "world85",
    "world86",
    "world87",
    "world88",
    "world89",
    "world90",
    "world91",
    "world92",
    "world93",
    "world94",
    "world95",
    "KP0",
    "KP1",
    "KP2",
    "KP3",
    "KP4",
    "KP5",
    "KP6",
    "KP7",
    "KP8",
    "KP9",
    "KPperiod",
    "KPslash",
    "KPasterisk",
    "KPminus",
    "KPplus",
    "enter",
    "equals",
    "up",
    "down",
    "right",
    "left",
    "insert",
    "home",
    "end",
    "pageup",
    "pagedown",
    "f1",
    "f2",
    "f3",
    "f4",
    "f5",
    "f6",
    "f7",
    "f8",
    "f9",
    "f10",
    "f11",
    "f12",
    "f13",
    "f14",
    "f15",
    "",
    "",
    "",
    "numlock",
    "capslock",
    "scrolllock",
    "rightshift",
    "leftshift",
    "rightctrl",
    "leftctrl",
    "rightalt",
    "leftalt",
    "rightmeta",
    "leftmeta",
    "leftsuper",
    "rightsuper",
    "altgr",
    "compose",
    "help",
    "printscreen",
    "sysreq",
    "break",
    "menu",
    "power",
    "euro",
    "undo",
];
pub fn key_name(key: i32) -> &'static str {
    usize::try_from(key)
        .ok()
        .and_then(|key| SDL_NAMES.get(key))
        .copied()
        .unwrap_or("")
}
pub fn key_from_name(name: &str) -> i32 {
    SDL_NAMES
        .iter()
        .position(|candidate| *candidate == name)
        .map(|index| index as i32)
        .unwrap_or(0)
}

/// Original compatibility-header spelling used by configuration parsing.
pub fn get_sdl_key(name: &str) -> i32 {
    key_from_name(name)
}

pub fn translate_keycode(k: i32) -> i32 {
    crate::sdlkey_compat::translate_sdl_keycode(k)
}

#[cfg(test)]
mod tests {
    use super::*;
    struct Fake(Vec<SdlEvent>);
    impl SdlBackend for Fake {
        fn poll_event(&mut self, _: u32) -> Option<SdlEvent> {
            if self.0.is_empty() {
                None
            } else {
                Some(self.0.remove(0))
            }
        }
    }

    #[test]
    fn translates_events_and_tracks_state_without_sdl() {
        let mut io = SdlIo::new(Fake(vec![
            SdlEvent::Key {
                keycode: 32,
                down: true,
                repeat: false,
            },
            SdlEvent::MouseMotion { x: 4, y: 9 },
        ]));
        io.activate();
        assert_eq!(
            io.poll(),
            Some(InputEvent::Key {
                down: true,
                keysym: 32,
                unicode: 0
            })
        );
        assert!(io.keys_held()[32] && io.settings.spacekey);
        assert_eq!(io.poll(), Some(InputEvent::MouseMotion { x: 4, y: 9 }));
    }

    #[test]
    fn wheel_compatibility_events_are_loop_click_buttons() {
        let mut io = SdlIo::new(Fake(vec![
            SdlEvent::MouseButton {
                button: 4,
                x: 12,
                y: 34,
                down: true,
            },
            SdlEvent::MouseButton {
                button: 5,
                x: 12,
                y: 34,
                down: true,
            },
        ]));
        io.activate();
        assert_eq!(
            io.poll(),
            Some(InputEvent::MouseButton {
                button: 4,
                x: 12,
                y: 34,
                down: true,
            })
        );
        assert_eq!(
            io.poll(),
            Some(InputEvent::MouseButton {
                button: 5,
                x: 12,
                y: 34,
                down: true,
            })
        );
    }

    #[test]
    fn names_and_repeat_policy_match_compatibility_behavior() {
        assert_eq!(key_from_name("leftshift"), 304);
        assert_eq!(key_name(282), "f1");
        // Letter names only exist at the C++ `SDL_names` lowercase range
        // (97-122); the uppercase range (65-90) is unnamed there, matching
        // the SDL2 keycodes real key presses actually deliver. A prior bug
        // resolved "a" to 65 by matching an incorrectly-added uppercase
        // alias before reaching the real entry at 97, which silently
        // desynced every letter-keyed XML binding (browse, undo, trigger
        // selected, ...) from the keysyms live key events produce.
        assert_eq!(key_from_name("a"), 97);
        assert_eq!(key_from_name("b"), 98);
        assert_eq!(key_from_name("z"), 122);
        assert_eq!(key_name(65), "");
        assert_eq!(key_name(97), "a");
        // These names are used by the shipped default keybindings
        // (coreinterface.xml: SLASH for help, BACKSLASH to reset master out
        // volume, KP +/- to change the browser item) and were previously
        // entirely absent from the lookup table, so `key_from_name` silently
        // returned 0 (unknown) for all of them.
        assert_eq!(key_from_name("slash"), 47);
        assert_eq!(key_from_name("backslash"), 92);
        assert_eq!(key_from_name("tilde"), 96);
        assert_eq!(key_from_name("minus"), 45);
        assert_eq!(key_from_name("equal"), 61);
        assert_eq!(key_from_name("KPplus"), 270);
        assert_eq!(key_from_name("KPminus"), 269);
        let mut io = SdlIo::new(Fake(vec![SdlEvent::Key {
            keycode: 65,
            down: true,
            repeat: true,
        }]));
        io.activate();
        assert_eq!(io.poll(), None);
        io.enable_key_repeat(true);
        let mut io = SdlIo::new(Fake(vec![SdlEvent::Text("é".into())]));
        io.activate();
        io.enable_unicode(true);
        assert_eq!(
            io.poll(),
            Some(InputEvent::Key {
                down: true,
                keysym: 'é' as i32,
                unicode: 'é' as i32,
            })
        );
    }

    #[test]
    fn text_input_matches_cpp_first_latin1_scalar_rule() {
        let mut io = SdlIo::new(Fake(vec![SdlEvent::Text("ésecond".into())]));
        io.activate();
        io.enable_unicode(true);
        assert_eq!(
            io.poll(),
            Some(InputEvent::Key {
                down: true,
                keysym: 'é' as i32,
                unicode: 'é' as i32,
            })
        );

        let mut io = SdlIo::new(Fake(vec![SdlEvent::Text("こんにちは🙂".into())]));
        io.activate();
        io.enable_unicode(true);
        assert_eq!(io.poll(), None);
    }

    #[test]
    fn shift_function_keys_emit_cpp_direct_subdivide_values() {
        let mut io = SdlIo::new(Fake(vec![
            SdlEvent::Key {
                keycode: i32::from(sdl2::keyboard::Keycode::LSHIFT),
                down: true,
                repeat: false,
            },
            SdlEvent::Key {
                keycode: i32::from(sdl2::keyboard::Keycode::F3),
                down: true,
                repeat: false,
            },
            SdlEvent::Key {
                keycode: i32::from(sdl2::keyboard::Keycode::F3),
                down: true,
                repeat: true,
            },
        ]));
        io.activate();
        assert!(matches!(
            io.poll(),
            Some(InputEvent::Key { keysym: 304, .. })
        ));
        assert!(io.poll().is_some());
        assert_eq!(io.take_pulse_subdivide(), Some(3));
        io.enable_key_repeat(true);
        assert!(io.poll().is_some());
        assert_eq!(io.take_pulse_subdivide(), Some(6));
    }
}
