# Real-hardware macOS workflow capture

This capture is an operator-assisted native-app acceptance run. It is not a
smoke test and it does not turn a claimed action into evidence: each step is
captured from the running signed application using macOS Accessibility output
and a native `screencapture` image.

## Exact operator procedure

1. Use an Apple Silicon Mac with the physical audio/MIDI devices used for the
   claim connected. Grant the terminal (or CI host) Accessibility and Screen
   Recording permissions in System Settings > Privacy & Security.
2. Build/package the candidate and complete `scripts/verify_macos_bundle.py`.
3. Choose the workflow steps, in order, and write them down before running the
   command. Example labels must describe real actions, such as `launch`,
   `load the supplied session`, `trigger a loop`, and `mute the loop`. Close
   the app after attestation is published because an exited process has no
   native state to observe.
4. Run from `freewheeling-plus` (replace the bundle and steps with the exact
   candidate workflow):

   ```sh
   rm -rf /tmp/fwp-macos-capture
   python3 scripts/run_macos_workflow_capture.py \
     /Applications/FreeWheeling.app \
     --output /tmp/fwp-macos-capture \
     --step 'launch' \
     --step 'load the supplied session' \
     --step 'trigger a loop' \
     --step 'mute the loop'
   ```

5. For each prompt, perform exactly that action in the visible native app;
   press Return only after it visibly completes. Do not press ahead, use
   `--smoke-test`, substitute a virtual device, or edit the output directory.
6. Review every `step-*-accessibility.txt`, screenshot, and
   `hardware.txt`. Preserve the complete output directory and its generated
   `attestation.json`; the JSON is valid only when its status is `passed` and
   every listed SHA-256 matches. Then close the app normally.

The runner fails closed on non-macOS/non-arm64 hosts, unsigned or malformed
bundles, missing CoreAudio inventory, missing observations/captures, aborted
steps, and existing attestations. It never writes an attestation on failure.
