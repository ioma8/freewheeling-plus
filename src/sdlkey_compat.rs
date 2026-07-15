//! SDL2 keycode translation to Freewheeling's SDL1-compatible key values.

pub type SdlKey = i32;

pub const FWL_SDLK_FIRST: SdlKey = 0;
pub const FWL_SDLK_UNKNOWN: SdlKey = 0;
pub const FWL_SDLK_BACKSPACE: SdlKey = 8;
pub const FWL_SDLK_TAB: SdlKey = 9;
pub const FWL_SDLK_CLEAR: SdlKey = 12;
pub const FWL_SDLK_RETURN: SdlKey = 13;
pub const FWL_SDLK_PAUSE: SdlKey = 19;
pub const FWL_SDLK_ESCAPE: SdlKey = 27;
pub const FWL_SDLK_SPACE: SdlKey = 32;
pub const FWL_SDLK_DELETE: SdlKey = 127;

pub const FWL_SDLK_KP0: SdlKey = 256;
pub const FWL_SDLK_KP1: SdlKey = 257;
pub const FWL_SDLK_KP2: SdlKey = 258;
pub const FWL_SDLK_KP3: SdlKey = 259;
pub const FWL_SDLK_KP4: SdlKey = 260;
pub const FWL_SDLK_KP5: SdlKey = 261;
pub const FWL_SDLK_KP6: SdlKey = 262;
pub const FWL_SDLK_KP7: SdlKey = 263;
pub const FWL_SDLK_KP8: SdlKey = 264;
pub const FWL_SDLK_KP9: SdlKey = 265;
pub const FWL_SDLK_KP_PERIOD: SdlKey = 266;
pub const FWL_SDLK_KP_DIVIDE: SdlKey = 267;
pub const FWL_SDLK_KP_MULTIPLY: SdlKey = 268;
pub const FWL_SDLK_KP_MINUS: SdlKey = 269;
pub const FWL_SDLK_KP_PLUS: SdlKey = 270;
pub const FWL_SDLK_KP_ENTER: SdlKey = 271;
pub const FWL_SDLK_KP_EQUALS: SdlKey = 272;
pub const FWL_SDLK_UP: SdlKey = 273;
pub const FWL_SDLK_DOWN: SdlKey = 274;
pub const FWL_SDLK_RIGHT: SdlKey = 275;
pub const FWL_SDLK_LEFT: SdlKey = 276;
pub const FWL_SDLK_INSERT: SdlKey = 277;
pub const FWL_SDLK_HOME: SdlKey = 278;
pub const FWL_SDLK_END: SdlKey = 279;
pub const FWL_SDLK_PAGEUP: SdlKey = 280;
pub const FWL_SDLK_PAGEDOWN: SdlKey = 281;
pub const FWL_SDLK_F1: SdlKey = 282;
pub const FWL_SDLK_F2: SdlKey = 283;
pub const FWL_SDLK_F3: SdlKey = 284;
pub const FWL_SDLK_F4: SdlKey = 285;
pub const FWL_SDLK_F5: SdlKey = 286;
pub const FWL_SDLK_F6: SdlKey = 287;
pub const FWL_SDLK_F7: SdlKey = 288;
pub const FWL_SDLK_F8: SdlKey = 289;
pub const FWL_SDLK_F9: SdlKey = 290;
pub const FWL_SDLK_F10: SdlKey = 291;
pub const FWL_SDLK_F11: SdlKey = 292;
pub const FWL_SDLK_F12: SdlKey = 293;
pub const FWL_SDLK_F13: SdlKey = 294;
pub const FWL_SDLK_F14: SdlKey = 295;
pub const FWL_SDLK_F15: SdlKey = 296;
pub const FWL_SDLK_NUMLOCK: SdlKey = 300;
pub const FWL_SDLK_CAPSLOCK: SdlKey = 301;
pub const FWL_SDLK_SCROLLOCK: SdlKey = 302;
pub const FWL_SDLK_RSHIFT: SdlKey = 303;
pub const FWL_SDLK_LSHIFT: SdlKey = 304;
pub const FWL_SDLK_RCTRL: SdlKey = 305;
pub const FWL_SDLK_LCTRL: SdlKey = 306;
pub const FWL_SDLK_RALT: SdlKey = 307;
pub const FWL_SDLK_LALT: SdlKey = 308;
pub const FWL_SDLK_RMETA: SdlKey = 309;
pub const FWL_SDLK_LMETA: SdlKey = 310;
pub const FWL_SDLK_LSUPER: SdlKey = 311;
pub const FWL_SDLK_RSUPER: SdlKey = 312;
pub const FWL_SDLK_MODE: SdlKey = 313;
pub const FWL_SDLK_COMPOSE: SdlKey = 314;
pub const FWL_SDLK_HELP: SdlKey = 315;
pub const FWL_SDLK_PRINT: SdlKey = 316;
pub const FWL_SDLK_SYSREQ: SdlKey = 317;
pub const FWL_SDLK_BREAK: SdlKey = 318;
pub const FWL_SDLK_MENU: SdlKey = 319;
pub const FWL_SDLK_POWER: SdlKey = 320;
pub const FWL_SDLK_EURO: SdlKey = 321;
pub const FWL_SDLK_UNDO: SdlKey = 322;
pub const FWL_SDLK_LAST: SdlKey = 323;

/// Translate an SDL2 `SDL_Keycode` exactly as the legacy C++ adapter does.
pub fn translate_sdl_keycode(keycode: SdlKey) -> SdlKey {
    if (32..=126).contains(&keycode) || (160..=255).contains(&keycode) {
        return keycode;
    }
    use sdl2::keyboard::Keycode as K;
    match K::from_i32(keycode) {
        Some(K::BACKSPACE) => FWL_SDLK_BACKSPACE,
        Some(K::TAB) => FWL_SDLK_TAB,
        Some(K::RETURN) => FWL_SDLK_RETURN,
        Some(K::PAUSE) => FWL_SDLK_PAUSE,
        Some(K::ESCAPE) => FWL_SDLK_ESCAPE,
        Some(K::DELETE) => FWL_SDLK_DELETE,
        Some(K::KP_0) => FWL_SDLK_KP0,
        Some(K::KP_1) => FWL_SDLK_KP1,
        Some(K::KP_2) => FWL_SDLK_KP2,
        Some(K::KP_3) => FWL_SDLK_KP3,
        Some(K::KP_4) => FWL_SDLK_KP4,
        Some(K::KP_5) => FWL_SDLK_KP5,
        Some(K::KP_6) => FWL_SDLK_KP6,
        Some(K::KP_7) => FWL_SDLK_KP7,
        Some(K::KP_8) => FWL_SDLK_KP8,
        Some(K::KP_9) => FWL_SDLK_KP9,
        Some(K::KP_PERIOD) => FWL_SDLK_KP_PERIOD,
        Some(K::KP_DIVIDE) => FWL_SDLK_KP_DIVIDE,
        Some(K::KP_MULTIPLY) => FWL_SDLK_KP_MULTIPLY,
        Some(K::KP_MINUS) => FWL_SDLK_KP_MINUS,
        Some(K::KP_PLUS) => FWL_SDLK_KP_PLUS,
        Some(K::KP_ENTER) => FWL_SDLK_KP_ENTER,
        Some(K::KP_EQUALS) => FWL_SDLK_KP_EQUALS,
        Some(K::UP) => FWL_SDLK_UP,
        Some(K::DOWN) => FWL_SDLK_DOWN,
        Some(K::RIGHT) => FWL_SDLK_RIGHT,
        Some(K::LEFT) => FWL_SDLK_LEFT,
        Some(K::INSERT) => FWL_SDLK_INSERT,
        Some(K::HOME) => FWL_SDLK_HOME,
        Some(K::END) => FWL_SDLK_END,
        Some(K::PAGEUP) => FWL_SDLK_PAGEUP,
        Some(K::PAGEDOWN) => FWL_SDLK_PAGEDOWN,
        Some(K::F1) => FWL_SDLK_F1,
        Some(K::F2) => FWL_SDLK_F2,
        Some(K::F3) => FWL_SDLK_F3,
        Some(K::F4) => FWL_SDLK_F4,
        Some(K::F5) => FWL_SDLK_F5,
        Some(K::F6) => FWL_SDLK_F6,
        Some(K::F7) => FWL_SDLK_F7,
        Some(K::F8) => FWL_SDLK_F8,
        Some(K::F9) => FWL_SDLK_F9,
        Some(K::F10) => FWL_SDLK_F10,
        Some(K::F11) => FWL_SDLK_F11,
        Some(K::F12) => FWL_SDLK_F12,
        Some(K::F13) => FWL_SDLK_F13,
        Some(K::F14) => FWL_SDLK_F14,
        Some(K::F15) => FWL_SDLK_F15,
        Some(K::NUMLOCKCLEAR) => FWL_SDLK_NUMLOCK,
        Some(K::CAPSLOCK) => FWL_SDLK_CAPSLOCK,
        Some(K::SCROLLLOCK) => FWL_SDLK_SCROLLOCK,
        Some(K::RSHIFT) => FWL_SDLK_RSHIFT,
        Some(K::LSHIFT) => FWL_SDLK_LSHIFT,
        Some(K::RCTRL) => FWL_SDLK_RCTRL,
        Some(K::LCTRL) => FWL_SDLK_LCTRL,
        Some(K::RALT) => FWL_SDLK_RALT,
        Some(K::LALT) => FWL_SDLK_LALT,
        Some(K::RGUI) => FWL_SDLK_RMETA,
        Some(K::LGUI) => FWL_SDLK_LMETA,
        Some(K::MODE) => FWL_SDLK_MODE,
        Some(K::HELP) => FWL_SDLK_HELP,
        Some(K::PRINTSCREEN) => FWL_SDLK_PRINT,
        Some(K::SYSREQ) => FWL_SDLK_SYSREQ,
        Some(K::MENU) => FWL_SDLK_MENU,
        Some(K::POWER) => FWL_SDLK_POWER,
        Some(K::UNDO) => FWL_SDLK_UNDO,
        _ => FWL_SDLK_UNKNOWN,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn range_invariants() {
        assert_eq!(FWL_SDLK_FIRST, FWL_SDLK_UNKNOWN);
        assert_eq!(FWL_SDLK_LAST, FWL_SDLK_UNDO + 1);
    }
    #[test]
    fn printable_keys_pass_through_and_others_are_unknown() {
        for key in 32..=126 {
            assert_eq!(translate_sdl_keycode(key), key);
        }
        for key in 160..=255 {
            assert_eq!(translate_sdl_keycode(key), key);
        }
        assert_eq!(translate_sdl_keycode(31), FWL_SDLK_UNKNOWN);
        assert_eq!(translate_sdl_keycode(127), FWL_SDLK_DELETE);
        assert_eq!(translate_sdl_keycode(8), FWL_SDLK_BACKSPACE);
        assert_eq!(translate_sdl_keycode(13), FWL_SDLK_RETURN);
    }
    #[test]
    fn keypad_and_function_ranges_translate() {
        use sdl2::keyboard::Keycode as K;
        let keypad = [
            K::KP_0,
            K::KP_1,
            K::KP_2,
            K::KP_3,
            K::KP_4,
            K::KP_5,
            K::KP_6,
            K::KP_7,
            K::KP_8,
            K::KP_9,
        ];
        for (n, keycode) in keypad.iter().enumerate() {
            assert_eq!(
                translate_sdl_keycode(i32::from(*keycode)),
                FWL_SDLK_KP0 + n as i32
            );
        }
        let functions = [
            K::F1,
            K::F2,
            K::F3,
            K::F4,
            K::F5,
            K::F6,
            K::F7,
            K::F8,
            K::F9,
            K::F10,
            K::F11,
            K::F12,
            K::F13,
            K::F14,
            K::F15,
        ];
        for (n, keycode) in functions.iter().enumerate() {
            assert_eq!(
                translate_sdl_keycode(i32::from(*keycode)),
                FWL_SDLK_F1 + n as i32
            );
        }
    }

    #[test]
    fn special_keys_use_sdl_enum_values_not_assumed_numeric_ranges() {
        use sdl2::keyboard::Keycode as K;
        assert_eq!(
            translate_sdl_keycode(i32::from(K::SCROLLLOCK)),
            FWL_SDLK_SCROLLOCK
        );
        assert_eq!(
            translate_sdl_keycode(i32::from(K::PRINTSCREEN)),
            FWL_SDLK_PRINT
        );
        assert_eq!(translate_sdl_keycode(i32::from(K::LGUI)), FWL_SDLK_LMETA);
        assert_eq!(translate_sdl_keycode(i32::from(K::RGUI)), FWL_SDLK_RMETA);
    }
}
