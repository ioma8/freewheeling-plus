// FreeWheeling+ Android entry activity.
//
// SDL is statically linked into libfreewheeling_plus.so (the sdl2 crate's
// "bundled" feature), so there is no separate libSDL2.so. Override the
// default library list so the Java glue loads exactly our one shared object,
// whose SDL_main export is the application entry point.

package org.freewheeling.freewheeling_plus;

import org.libsdl.app.SDLActivity;

public class FreeWheelingActivity extends SDLActivity {
    @Override
    protected String[] getLibraries() {
        return new String[] { "freewheeling_plus" };
    }

    @Override
    protected String getMainFunction() {
        return "SDL_main";
    }
}
