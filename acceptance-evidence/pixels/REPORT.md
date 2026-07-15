# Native UI pixel parity report

Captured 2026-07-14 from the production XML scene (`../data/graphics.xml` and
the interfaces it names) through `SoftwareRgbaRenderer`. Reference FWRGBA1
files are lossless RGBA decodes of the genuine C++ PNG readbacks in
`fixtures/cpp-golden/screenshots`; their `PROVENANCE` identifies the historical
binary and capture mechanism. They are not synthesized references.

The required gate is 99.5% of pixels within delta 2 in every RGBA channel,
both for the complete image and each structural region. Current results:

| Mode | Drawable | Complete image |
| --- | ---: | ---: |
| 640x480 | 640x480 | 1.462891% |
| configured | 800x600 | 1.157708% |
| fullscreen | 1024x768 | 0.933202% |
| High-DPI | 1280x960 | 0.794596% |

All regions fail. At 640x480: keyboard/logo 1.953664%, primary browser
0.572447%, status/controls 0.545281%. At configured size: 1.738260%,
0.557961%, 0.241803%. At fullscreen: 1.495993%, 0.580670%, 0.190930%.
At High-DPI: 0.978718%, 0.427224%, 0.356744%.

## Source changes indicated by the evidence

1. `SoftwareRgbaRenderer::begin_frame` must clear a newly allocated
   `SoftwareSurface` to opaque black just as it clears a reused surface. The
   current first frame leaves 96.9% to 98.8% of candidate pixels at alpha 0;
   C++ readbacks are opaque. This single issue makes the RGBA gate fail even
   where RGB is black in both images.
2. After the alpha fix, widget rendering still needs C++ color/compositing and
   geometry reconciliation. RGB-only parity is 72.270508% (640x480),
   81.779375% (configured), 88.848750% (fullscreen), and 79.850911% (High-DPI),
   far below the gate. Compare `native_ui_scene` display ordering/state,
   `surface_primitives::blend`, and text rasterization/alignment against the
   C++ SDL_gfx/SDL_ttf path.
3. High-DPI needs separate attention after 1x parity: its RGB-only result
   regresses to 79.850911% despite the larger surface, pointing to font
   rasterization and integer coordinate/extent scaling rather than only the
   transparent-frame defect.

No product source was changed in this lane.
