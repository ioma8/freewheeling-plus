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
import android.os.Build;
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

    @Override
    protected void onCreate(Bundle savedInstanceState) {
        super.onCreate(savedInstanceState);

        try {
            extractDataAssets();
        } catch (IOException error) {
            Log.e(TAG, "extracting bundled data/ assets failed: " + error);
        }

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
        // SDL pauses the main thread). Ask a few seconds later instead.
        new android.os.Handler(android.os.Looper.getMainLooper()).postDelayed(() -> {
            if (sExternalStorageGranted == 0) {
                promptExternalStorageAccess();
            }
        }, 3000);
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
        if (Build.VERSION.SDK_INT >= 30) {
            sExternalStorageGranted = Environment.isExternalStorageManager() ? 1 : 0;
        } else {
            sExternalStorageGranted = 1;
        }
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
        long apkTime = 0;
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
        if (new File(target, "fweelin.xml").isFile() && extractedTime == apkTime) {
            return; // already extracted for this APK
        }
        copyAssetTree(getAssets(), "data", target);
        java.nio.file.Files.write(marker.toPath(),
                Long.toString(apkTime).getBytes("UTF-8"));
        Log.i(TAG, "extracted bundled data/ assets to " + target);
    }

    private static void copyAssetTree(AssetManager assets, String path, File out)
            throws IOException {
        String[] entries = assets.list(path);
        if (entries != null && entries.length > 0) {
            // A directory.
            if (!out.exists() && !out.mkdirs()) {
                throw new IOException("cannot create directory " + out);
            }
            for (String entry : entries) {
                copyAssetTree(assets, path + "/" + entry, new File(out, entry));
            }
        } else {
            // A file (AssetManager.list on a file returns an empty array).
            try (InputStream in = assets.open(path);
                    OutputStream outStream = new FileOutputStream(out)) {
                byte[] buffer = new byte[16 * 1024];
                int read;
                while ((read = in.read(buffer)) > 0) {
                    outStream.write(buffer, 0, read);
                }
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
