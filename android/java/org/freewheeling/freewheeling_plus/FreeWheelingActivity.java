// FreeWheeling+ Android entry activity.
//
// SDL is statically linked into libfreewheeling_plus.so (the sdl2 crate's
// "bundled" feature), so there is no separate libSDL2.so. Override the
// default library list so the Java glue loads exactly our one shared object,
// whose SDL_main export is the application entry point.
//
// Two Android-specific problems are solved here:
//
// 1. The bundled data/ tree (fweelin.xml, basic.sf2, fonts, ...) is packaged
//    into the APK as assets by scripts/package-android-apk.sh, but Android
//    never mounts APK assets on the filesystem. The native side expects them
//    at /data/data/<package>/files/data, so we extract them there before the
//    SDL thread (and thus SDL_main) starts.
//
//    The copy runs on a background thread and publishes sDataExtracted, which
//    SDL_main waits for before reading files/data (and reports if it failed).
//
// 2. RECORD_AUDIO is a dangerous permission on Android 6+ and must be
//    requested at runtime; declaring it in the manifest is not enough. The
//    request is issued here, and the result is published in
//    sRecordAudioResult so the native SDL_main can wait for the user's
//    decision before opening the capture stream (a pending or denied
//    permission makes AAudio's input open fail and used to kill the app).

package org.freewheeling.freewheeling_plus;

import android.Manifest;
import android.content.Intent;
import android.content.pm.PackageManager;
import android.content.res.AssetManager;
import android.net.Uri;
import android.os.Bundle;
import android.os.Environment;
import android.provider.Settings;
import android.util.Log;
import org.libsdl.app.SDLActivity;

import java.io.File;
import java.io.FileOutputStream;
import java.io.IOException;
import java.io.InputStream;
import java.io.OutputStream;

public class FreeWheelingActivity extends SDLActivity {
    private static final String TAG = "FreeWheeling";
    private static final int REQUEST_RECORD_AUDIO = 1;

    /** 0 = decision pending, 1 = granted, 2 = denied. Read from native code. */
    public static volatile int sRecordAudioResult = 0;

    /** 1 when "all files access" is granted, else 0. Read from native code. */
    public static volatile int sExternalStorageGranted = 0;

    /** 0 = extraction pending, 1 = data/ is complete, 2 = extraction failed.
     *  Read from native code (SDL_main waits for it before loading the config). */
    public static volatile int sDataExtracted = 0;

    /** Supplies the delayed "all files access" prompt; kept so it can be
     *  cancelled when the activity goes away. */
    private android.os.Handler promptHandler;
    /** Whether the prompt was already shown for this activity instance. */
    private boolean promptedExternalStorageAccess = false;

    @Override
    protected void onCreate(Bundle savedInstanceState) {
        super.onCreate(savedInstanceState);

        // The data/ tree holds several megabytes (soundfont, fonts, XML);
        // copying it on the UI thread blocks the first launch of every APK
        // update and can trip the ANR watchdog. It runs on a worker instead,
        // and SDL_main waits for sDataExtracted before reading files/data.
        Thread extractor = new Thread(() -> {
            try {
                extractDataAssets();
                sDataExtracted = 1;
            } catch (IOException error) {
                Log.e(TAG, "extracting bundled data/ assets failed: " + error);
                sDataExtracted = 2;
            }
        }, "fweelin-assets");
        extractor.start();

        if (checkSelfPermission(Manifest.permission.RECORD_AUDIO)
                == PackageManager.PERMISSION_GRANTED) {
            sRecordAudioResult = 1;
        } else {
            sRecordAudioResult = 0;
            requestPermissions(
                    new String[] { Manifest.permission.RECORD_AUDIO },
                    REQUEST_RECORD_AUDIO);
        }

        refreshExternalStorageAccess();

        // Prompting immediately in onCreate would background the app before
        // SDL has finished starting (the settings activity takes focus and
        // SDL pauses the main thread). Ask a few seconds later instead, from a
        // handler that is cancelled with the activity and only fires once per
        // instance.
        promptHandler = new android.os.Handler(android.os.Looper.getMainLooper());
        promptHandler.postDelayed(() -> {
            if (isFinishing() || isDestroyed()) {
                return;
            }
            if (!promptedExternalStorageAccess && sExternalStorageGranted == 0) {
                promptedExternalStorageAccess = true;
                promptExternalStorageAccess();
            }
        }, 3000);
    }

    @Override
    protected void onDestroy() {
        // The delayed prompt must not outlive the activity: starting an
        // activity from a destroyed one throws (and leaks the intent).
        if (promptHandler != null) {
            promptHandler.removeCallbacksAndMessages(null);
            promptHandler = null;
        }
        super.onDestroy();
    }

    @Override
    protected void onPause() {
        super.onPause();
        // Leaving the foreground cancels the pending prompt; the next resume
        // of *this* instance can still show it if access was not granted.
        if (promptHandler != null) {
            promptHandler.removeCallbacksAndMessages(null);
        }
    }

    @Override
    protected void onResume() {
        super.onResume();
        // The user may have granted "all files access" in Settings and
        // returned to the app.
        refreshExternalStorageAccess();
    }

    /** Update sExternalStorageGranted from the current system state. */
    private void refreshExternalStorageAccess() {
        // minSdkVersion is 30+ (see AndroidManifest.xml), so
        // `Environment.isExternalStorageManager()` is always available: no
        // version check is needed.
        sExternalStorageGranted = Environment.isExternalStorageManager() ? 1 : 0;
    }

    /** Open Settings so the user can grant "all files access" for saving
     *  stream recordings to the shared Documents folder. */
    private void promptExternalStorageAccess() {
        try {
            Intent intent = new Intent(
                    Settings.ACTION_MANAGE_APP_ALL_FILES_ACCESS_PERMISSION,
                    Uri.parse("package:" + getPackageName()));
            startActivity(intent);
        } catch (Exception error) {
            Log.w(TAG, "cannot open all-files-access settings: " + error);
        }
    }

    @Override
    public void onRequestPermissionsResult(
            int requestCode, String[] permissions, int[] grantResults) {
        super.onRequestPermissionsResult(requestCode, permissions, grantResults);
        if (requestCode == REQUEST_RECORD_AUDIO) {
            sRecordAudioResult = (grantResults.length > 0
                    && grantResults[0] == PackageManager.PERMISSION_GRANTED) ? 1 : 2;
        }
    }

    /** Copy the packaged assets/data tree to files/data. Extracted once per
     *  APK update: the marker file records the APK's lastUpdateTime, so a
     *  reinstall (which keeps app data) re-extracts the new bundled files. */
    private void extractDataAssets() throws IOException {
        File target = new File(getFilesDir(), "data");
        File marker = new File(target, ".extracted-apk-mtime");
        // -1 is a sentinel that no marker can contain: falling back to 0 made
        // "lookup failed" look like a valid, matching timestamp, so a stale
        // tree was accepted as up to date.
        long apkTime = -1;
        try {
            apkTime = getPackageManager()
                    .getPackageInfo(getPackageName(), 0).lastUpdateTime;
        } catch (PackageManager.NameNotFoundException error) {
            Log.w(TAG, "cannot read APK update time: " + error);
        }
        long extractedTime = -1;
        if (marker.isFile()) {
            try {
                extractedTime = Long.parseLong(
                        new String(java.nio.file.Files.readAllBytes(
                                marker.toPath()), "UTF-8").trim());
            } catch (Exception error) {
                extractedTime = -1;
            }
        }
        if (apkTime >= 0 && new File(target, "fweelin.xml").isFile()
                && extractedTime == apkTime) {
            return; // already extracted for this APK
        }
        copyAssetTree(getAssets(), "data", target);
        java.nio.file.Files.write(marker.toPath(),
                Long.toString(apkTime).getBytes("UTF-8"));
        Log.i(TAG, "extracted bundled data/ assets to " + target);
    }

    private static void copyAssetTree(AssetManager assets, String path, File out)
            throws IOException {
        // `AssetManager.list` returns an empty array for a file *and* for an
        // empty directory, so the two cannot be told apart from the listing.
        // Trying to open the path answers the question: a file opens, a
        // directory fails with FileNotFoundException.
        try (InputStream in = assets.open(path)) {
            try (OutputStream outStream = new FileOutputStream(out)) {
                byte[] buffer = new byte[16 * 1024];
                int read;
                while ((read = in.read(buffer)) > 0) {
                    outStream.write(buffer, 0, read);
                }
            }
            return;
        } catch (java.io.FileNotFoundException notAFile) {
            // A directory, handled below.
        }
        String[] entries = assets.list(path);
        if (!out.exists() && !out.mkdirs()) {
            throw new IOException("cannot create directory " + out);
        }
        if (entries == null) {
            return; // an empty directory: created above, nothing to copy
        }
        for (String entry : entries) {
            try {
                copyAssetTree(assets, path + "/" + entry, new File(out, entry));
            } catch (IOException error) {
                // One unreadable entry must not abort the whole extraction:
                // the remaining siblings still need to be written, and the
                // caller records the failure.
                Log.e(TAG, "cannot extract asset " + path + "/" + entry + ": " + error);
            }
        }
    }

    @Override
    protected String[] getLibraries() {
        return new String[] { "freewheeling_plus" };
    }

    @Override
    protected String getMainFunction() {
        return "SDL_main";
    }
}
