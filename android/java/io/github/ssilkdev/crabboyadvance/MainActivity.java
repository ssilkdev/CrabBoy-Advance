package io.github.ssilkdev.crabboyadvance;

import android.app.NativeActivity;
import android.content.Intent;
import android.database.Cursor;
import android.net.Uri;
import android.os.Build;
import android.os.Bundle;
import android.provider.OpenableColumns;
import android.view.DisplayCutout;
import android.view.View;
import android.view.WindowInsets;
import android.view.WindowManager;

import java.io.File;
import java.io.FileOutputStream;
import java.io.InputStream;
import java.io.OutputStream;
import java.util.Locale;

/**
 * Hosts the Rust front-end (libcrabboy_android.so) and provides the pieces
 * that need the Android framework: the system file picker, copying the chosen
 * ROM into app storage, immersive fullscreen, and safe-area insets.
 *
 * Rust calls pickRom(), takeImportedRom(), takeImportError(),
 * getSafeInsets() and setOrientation() over JNI.
 */
public class MainActivity extends NativeActivity {
    private static final int PICK_ROM = 1001;
    /** Largest GBA cartridge is 32 MiB (saves are far smaller). */
    private static final long MAX_ROM_BYTES = 32L * 1024 * 1024;

    private volatile String importedRom;
    private volatile String importError;
    private volatile int[] safeInsets = new int[4];

    @Override
    protected void onCreate(Bundle savedInstanceState) {
        super.onCreate(savedInstanceState);
        getWindow().addFlags(WindowManager.LayoutParams.FLAG_KEEP_SCREEN_ON);
        if (Build.VERSION.SDK_INT >= 28) {
            WindowManager.LayoutParams lp = getWindow().getAttributes();
            lp.layoutInDisplayCutoutMode =
                    WindowManager.LayoutParams.LAYOUT_IN_DISPLAY_CUTOUT_MODE_SHORT_EDGES;
            getWindow().setAttributes(lp);
        }
        View decor = getWindow().getDecorView();
        decor.setOnApplyWindowInsetsListener((v, insets) -> {
            updateInsets(insets);
            return v.onApplyWindowInsets(insets);
        });
        hideSystemBars();
    }

    @Override
    public void onWindowFocusChanged(boolean hasFocus) {
        super.onWindowFocusChanged(hasFocus);
        if (hasFocus) hideSystemBars();
    }

    @SuppressWarnings("deprecation")
    private void hideSystemBars() {
        getWindow().getDecorView().setSystemUiVisibility(
                View.SYSTEM_UI_FLAG_IMMERSIVE_STICKY
                        | View.SYSTEM_UI_FLAG_FULLSCREEN
                        | View.SYSTEM_UI_FLAG_HIDE_NAVIGATION
                        | View.SYSTEM_UI_FLAG_LAYOUT_STABLE
                        | View.SYSTEM_UI_FLAG_LAYOUT_HIDE_NAVIGATION
                        | View.SYSTEM_UI_FLAG_LAYOUT_FULLSCREEN);
    }

    /** With the system bars hidden, only a display cutout can cover content. */
    private void updateInsets(WindowInsets insets) {
        int[] out = new int[4];
        if (Build.VERSION.SDK_INT >= 28) {
            DisplayCutout cutout = insets.getDisplayCutout();
            if (cutout != null) {
                out[0] = cutout.getSafeInsetLeft();
                out[1] = cutout.getSafeInsetTop();
                out[2] = cutout.getSafeInsetRight();
                out[3] = cutout.getSafeInsetBottom();
            }
        }
        safeInsets = out;
    }

    // ---- Called from Rust ------------------------------------------------

    public int[] getSafeInsets() {
        return safeInsets;
    }

    /** ActivityInfo.SCREEN_ORIENTATION_* value chosen in the in-game menu. */
    public void setOrientation(final int value) {
        runOnUiThread(() -> {
            if (getRequestedOrientation() != value) setRequestedOrientation(value);
        });
    }

    public void pickRom() {
        runOnUiThread(() -> {
            Intent intent = new Intent(Intent.ACTION_OPEN_DOCUMENT);
            intent.addCategory(Intent.CATEGORY_OPENABLE);
            // ROM files have no registered MIME type; filter by name on return.
            intent.setType("*/*");
            try {
                startActivityForResult(intent, PICK_ROM);
            } catch (Exception e) {
                importError = "No file picker available: " + e.getMessage();
            }
        });
    }

    public String takeImportedRom() {
        String r = importedRom;
        importedRom = null;
        return r;
    }

    public String takeImportError() {
        String r = importError;
        importError = null;
        return r;
    }

    // ---- Picker result -----------------------------------------------------

    @Override
    protected void onActivityResult(int requestCode, int resultCode, Intent data) {
        super.onActivityResult(requestCode, resultCode, data);
        if (requestCode != PICK_ROM || resultCode != RESULT_OK || data == null || data.getData() == null) {
            return;
        }
        final Uri uri = data.getData();
        // Copy off the UI thread: a 32 MiB ROM from cloud storage can be slow.
        new Thread(() -> importRom(uri), "rom-import").start();
    }

    private void importRom(Uri uri) {
        String name = queryName(uri);
        if (name == null) name = "game.gba";
        name = name.replaceAll("[\\\\/:*?\"<>|]", "_");
        String lower = name.toLowerCase(Locale.ROOT);
        boolean isSave = lower.endsWith(".sav");
        if (!(isSave || lower.endsWith(".gba") || lower.endsWith(".gb") || lower.endsWith(".gbc"))) {
            importError = "\"" + name + "\" is not a .gba, .gb or .gbc ROM or a .sav file"
                    + (lower.endsWith(".zip") || lower.endsWith(".7z") ? " (unzip it first)" : "");
            return;
        }
        File dir = new File(getFilesDir(), "roms");
        if (!dir.isDirectory() && !dir.mkdirs()) {
            importError = "Cannot create ROM folder";
            return;
        }
        File dest = new File(dir, name);
        File tmp = new File(dir, name + ".part");
        long total = 0;
        try (InputStream in = getContentResolver().openInputStream(uri);
             OutputStream out = new FileOutputStream(tmp)) {
            if (in == null) throw new java.io.IOException("cannot open file");
            byte[] buf = new byte[1 << 16];
            int n;
            while ((n = in.read(buf)) > 0) {
                total += n;
                if (total > MAX_ROM_BYTES) throw new java.io.IOException("file is larger than 32 MB");
                out.write(buf, 0, n);
            }
        } catch (Exception e) {
            //noinspection ResultOfMethodCallIgnored
            tmp.delete();
            importError = "Import failed: " + e.getMessage();
            return;
        }
        if (total == 0 || !tmp.renameTo(dest)) {
            //noinspection ResultOfMethodCallIgnored
            tmp.delete();
            importError = total == 0 ? "That file is empty" : "Import failed: could not save the ROM";
            return;
        }
        if (isSave) {
            // Battery saves live next to the ROM with the same base name;
            // the next launch of that game picks it up.
            importError = "Imported save \"" + name + "\". It is used by the game with the same name.";
            return;
        }
        importedRom = dest.getAbsolutePath();
    }

    private String queryName(Uri uri) {
        try (Cursor c = getContentResolver().query(uri,
                new String[]{OpenableColumns.DISPLAY_NAME}, null, null, null)) {
            if (c != null && c.moveToFirst()) return c.getString(0);
        } catch (Exception ignored) {
        }
        String last = uri.getLastPathSegment();
        return last == null ? null : last.substring(last.lastIndexOf('/') + 1);
    }
}
