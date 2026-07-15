# Historical C++ screenshots

These PNGs are framebuffer readbacks from the complete historical C++
FreeWheeling scene. `scripts/capture_cpp_screenshots.sh` launches the original
application and configuration with SDL's deterministic offscreen `dummy`
driver. A temporary dyld interposer observes the original
`SDL_UpdateWindowSurface` call, then saves the application's window surface;
it does not draw or replace any scene geometry.

The set covers the 640x480 baseline, an 800x600 configuration, a 1024x768
fullscreen logical frame, and the same 640x480 logical scene at 1x and at a
1280x960 High-DPI drawable. Every `.meta` file records the logical and actual
drawable dimensions. `PROVENANCE` records the exact Git revision and hashes of
the executable, renderer sources, configuration, and capture script.

Regenerate from the repository root with:

```sh
scripts/capture_cpp_screenshots.sh
```

The script requires the freshly built historical application at
`MacOSX/build/Release/fweelin.app` (override with `CPP_FWEELIN_APP`) and fails
instead of substituting generated geometry when that bundle is absent.
