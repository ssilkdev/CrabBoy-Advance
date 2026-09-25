package io.github.ssilkdev.crabboyadvance;

import android.app.NativeActivity;
import android.content.Intent;
import android.hardware.Sensor;
import android.hardware.SensorEvent;
import android.hardware.SensorEventListener;
import android.hardware.SensorManager;
import android.database.Cursor;
import android.net.Uri;
import android.os.Build;
import android.os.Bundle;
import android.os.Handler;
import android.os.HandlerThread;
import android.os.VibrationEffect;
import android.os.Vibrator;
import android.provider.DocumentsContract;
import android.provider.OpenableColumns;
import android.view.DisplayCutout;
import android.view.View;
import android.view.WindowInsets;
import android.view.WindowManager;

import java.io.File;
import java.io.FileOutputStream;
import java.io.InputStream;
import java.io.OutputStream;
import java.util.ArrayDeque;
import java.util.Locale;
import java.util.Map;
import java.util.Queue;
import java.util.concurrent.ConcurrentHashMap;

/**
 * Hosts the Rust front-end (libcrabboy_android.so) and provides the pieces
 * that need the Android framework: the system file picker, SAF folder auto-scan,
 * haptic rumble, motion sensors, immersive fullscreen, and safe-area insets.
 */
public class MainActivity extends NativeActivity {
    private static final int PICK_ROM = 1001;
    private static final int PICK_SKIN = 1002;
    private static final int PICK_FOLDER = 1003;
    private static final String PREFS_NAME = "crabboy_prefs";
    private static final String KEY_ROM_TREE_URI = "rom_tree_uri";
    private static final String KEY_ROM_FOLDER_NAME = "rom_folder_name";

    /** Largest skin pack accepted (checked again, more strictly, in Rust). */
    private static final long MAX_SKIN_BYTES = 16L * 1024 * 1024;
    /** Largest GBA cartridge is 32 MiB (saves are far smaller). */
    private static final long MAX_ROM_BYTES = 32L * 1024 * 1024;

    private volatile String importedRom;
    private volatile String importedSkin;
    private volatile String importError;
    private volatile int[] safeInsets = new int[4];
    private Vibrator vibrator;
    /**
     * Vibrator calls are binder IPC and sensor events need a looper; both
     * run here, off the emulation thread.
     */
    private Handler background;
    private volatile boolean scanning;
    private volatile String scanNotice;
    private final Map<String, String> saveDocMap = new ConcurrentHashMap<>();
    private final Map<String, String> parentDocMap = new ConcurrentHashMap<>();

    private SensorManager sensors;
    /** Whether Rust wants tilt readings (kept across pause/resume). */
    private volatile boolean tiltWanted;
    private boolean tiltRegistered;
    /** Latest gravity reading (m/s^2, device axes) and display rotation; null until the first one. */
    private volatile float[] tilt;
    private final float[] lowPass = new float[3];
    private boolean lowPassSeeded;
    private boolean rawAccelerometer;
    private final SensorEventListener tiltListener = new SensorEventListener() {
        @Override
        public void onSensorChanged(SensorEvent e) {
            float[] g = e.values;
            if (rawAccelerometer) {
                // No fused gravity sensor: smooth out hand shake.
                float k = lowPassSeeded ? 0.2f : 1f;
                lowPassSeeded = true;
                for (int i = 0; i < 3; i++) lowPass[i] += k * (e.values[i] - lowPass[i]);
                g = lowPass;
            }
            @SuppressWarnings("deprecation")
            int rotation = getWindowManager().getDefaultDisplay().getRotation();
            tilt = new float[]{g[0], g[1], g[2], rotation};
        }

        @Override
        public void onAccuracyChanged(Sensor sensor, int accuracy) {
        }
    };

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
        vibrator = (Vibrator) getSystemService(VIBRATOR_SERVICE);
        sensors = (SensorManager) getSystemService(SENSOR_SERVICE);
        HandlerThread thread = new HandlerThread("haptics-sensors");
        thread.start();
        background = new Handler(thread.getLooper());

        final String savedTree = getFolderUri();
        if (savedTree != null && !savedTree.isEmpty()) {
            boolean valid = false;
            try {
                for (android.content.UriPermission p : getContentResolver().getPersistedUriPermissions()) {
                    if (p.getUri().toString().equals(savedTree) && p.isReadPermission()) {
                        valid = true;
                        break;
                    }
                }
            } catch (Exception ignored) {
            }
            if (valid) {
                background.post(() -> scanRomTree(Uri.parse(savedTree)));
            }
        }
    }

    @Override
    protected void onResume() {
        super.onResume();
        updateTiltSensor();
    }

    @Override
    protected void onPause() {
        unregisterTilt();
        super.onPause();
    }

    @Override
    protected void onDestroy() {
        unregisterTilt();
        if (background != null) background.getLooper().quitSafely();
        super.onDestroy();
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

    /** A short click for an on-screen button press. */
    public void vibrate() {
        final Vibrator v = vibrator;
        final Handler h = background;
        if (v == null || h == null || !v.hasVibrator()) return;
        h.post(() -> {
            try {
                if (Build.VERSION.SDK_INT >= 29) {
                    v.vibrate(VibrationEffect.createPredefined(VibrationEffect.EFFECT_CLICK));
                } else {
                    v.vibrate(VibrationEffect.createOneShot(15, VibrationEffect.DEFAULT_AMPLITUDE));
                }
            } catch (Exception ignored) {
            }
        });
    }

    /** Rumble motor for cartridge rumble or audio bass impact. */
    public void rumble(final int durationMs, final float intensity) {
        final Vibrator v = vibrator;
        final Handler h = background;
        if (v == null || h == null || !v.hasVibrator()) return;
        h.post(() -> {
            try {
                int amp = Math.round(Math.max(0.0f, Math.min(1.0f, intensity)) * 255.0f);
                if (amp <= 0) {
                    v.cancel();
                    return;
                }
                if (Build.VERSION.SDK_INT >= 26) {
                    if (v.hasAmplitudeControl()) {
                        v.vibrate(VibrationEffect.createOneShot(durationMs, amp));
                    } else {
                        v.vibrate(VibrationEffect.createOneShot(durationMs, VibrationEffect.DEFAULT_AMPLITUDE));
                    }
                } else {
                    v.vibrate(durationMs);
                }
            } catch (Exception ignored) {
            }
        });
    }

    public void stopRumble() {
        final Vibrator v = vibrator;
        final Handler h = background;
        if (v == null || h == null || !v.hasVibrator()) return;
        h.post(() -> {
            try {
                v.cancel();
            } catch (Exception ignored) {
            }
        });
    }

    /** Experimental tilt controls: start or stop gravity readings. */
    public void setTiltSensor(final boolean on) {
        tiltWanted = on;
        runOnUiThread(this::updateTiltSensor);
    }

    /** {x, y, z, displayRotation}, or null when there is no reading yet. */
    public float[] getTilt() {
        return tiltWanted ? tilt : null;
    }

    private void updateTiltSensor() {
        if (!tiltWanted) {
            unregisterTilt();
            return;
        }
        if (tiltRegistered || sensors == null || background == null) return;
        Sensor s = sensors.getDefaultSensor(Sensor.TYPE_GRAVITY);
        rawAccelerometer = s == null;
        if (s == null) s = sensors.getDefaultSensor(Sensor.TYPE_ACCELEROMETER);
        if (s == null) return;
        tilt = null;
        lowPassSeeded = false;
        tiltRegistered = sensors.registerListener(tiltListener, s, SensorManager.SENSOR_DELAY_GAME, background);
    }

    private void unregisterTilt() {
        if (tiltRegistered && sensors != null) sensors.unregisterListener(tiltListener);
        tiltRegistered = false;
        tilt = null;
    }

    /** ActivityInfo.SCREEN_ORIENTATION_* value chosen in the in-game menu. */
    public void setOrientation(final int value) {
        runOnUiThread(() -> {
            if (getRequestedOrientation() != value) setRequestedOrientation(value);
        });
    }

    /**
     * Battery: ask for a ~60 Hz display mode while a game runs (the GBA is
     * 59.73 Hz, so on a 90/120 Hz phone every extra refresh is a redraw of
     * the same picture). {@code on == false} returns to the system default.
     * Only modes with the current resolution are considered.
     */
    public void setGameRefreshRate(final boolean on) {
        runOnUiThread(() -> {
            WindowManager.LayoutParams lp = getWindow().getAttributes();
            int want = 0;
            if (on) {
                android.view.Display d = getWindowManager().getDefaultDisplay();
                android.view.Display.Mode cur = d.getMode();
                float best = Float.MAX_VALUE;
                for (android.view.Display.Mode m : d.getSupportedModes()) {
                    if (m.getPhysicalWidth() != cur.getPhysicalWidth()
                            || m.getPhysicalHeight() != cur.getPhysicalHeight()) continue;
                    float r = m.getRefreshRate();
                    if (r < 59f) continue; // never below the game's rate
                    if (r < best) { best = r; want = m.getModeId(); }
                }
            }
            if (lp.preferredDisplayModeId != want) {
                lp.preferredDisplayModeId = want;
                getWindow().setAttributes(lp);
            }
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

    public void pickSkin() {
        runOnUiThread(() -> {
            Intent intent = new Intent(Intent.ACTION_OPEN_DOCUMENT);
            intent.addCategory(Intent.CATEGORY_OPENABLE);
            intent.setType("*/*");
            try {
                startActivityForResult(intent, PICK_SKIN);
            } catch (Exception e) {
                importError = "No file picker available: " + e.getMessage();
            }
        });
    }

    public String takeImportedSkin() {
        String r = importedSkin;
        importedSkin = null;
        return r;
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

    public void pickFolder() {
        runOnUiThread(() -> {
            Intent intent = new Intent(Intent.ACTION_OPEN_DOCUMENT_TREE);
            intent.addFlags(Intent.FLAG_GRANT_READ_URI_PERMISSION
                    | Intent.FLAG_GRANT_WRITE_URI_PERMISSION
                    | Intent.FLAG_GRANT_PERSISTABLE_URI_PERMISSION
                    | Intent.FLAG_GRANT_PREFIX_URI_PERMISSION);
            try {
                startActivityForResult(intent, PICK_FOLDER);
            } catch (Exception e) {
                importError = "No folder picker available: " + e.getMessage();
            }
        });
    }

    public void rescanFolder() {
        String uriStr = getFolderUri();
        if (uriStr == null || uriStr.isEmpty()) {
            pickFolder();
            return;
        }
        if (scanning) return;
        final Handler h = background;
        if (h != null) {
            h.post(() -> scanRomTree(Uri.parse(uriStr)));
        } else {
            new Thread(() -> scanRomTree(Uri.parse(uriStr)), "rom-tree-scan").start();
        }
    }

    public String getFolderUri() {
        return getSharedPreferences(PREFS_NAME, MODE_PRIVATE).getString(KEY_ROM_TREE_URI, "");
    }

    public String getFolderName() {
        return getSharedPreferences(PREFS_NAME, MODE_PRIVATE).getString(KEY_ROM_FOLDER_NAME, "");
    }

    public void clearFolder() {
        String uriStr = getFolderUri();
        if (uriStr != null && !uriStr.isEmpty()) {
            try {
                getContentResolver().releasePersistableUriPermission(
                        Uri.parse(uriStr),
                        Intent.FLAG_GRANT_READ_URI_PERMISSION | Intent.FLAG_GRANT_WRITE_URI_PERMISSION
                );
            } catch (Exception ignored) {
            }
        }
        getSharedPreferences(PREFS_NAME, MODE_PRIVATE)
                .edit()
                .remove(KEY_ROM_TREE_URI)
                .remove(KEY_ROM_FOLDER_NAME)
                .apply();
        saveDocMap.clear();
        parentDocMap.clear();
        scanNotice = "ROM folder unlinked";
    }

    public boolean isScanning() {
        return scanning;
    }

    public String takeScanNotice() {
        String notice = scanNotice;
        scanNotice = null;
        return notice;
    }

    public void syncSaveToFolder(final String romStem) {
        if (romStem == null || romStem.isEmpty()) return;
        final Handler h = background;
        if (h == null) return;
        h.post(() -> {
            try {
                File romDir = new File(getFilesDir(), "roms");
                File localSav = new File(romDir, romStem + ".sav");
                if (!localSav.exists() || localSav.length() == 0) return;

                String savUriStr = saveDocMap.get(romStem);
                Uri savDocUri = (savUriStr != null) ? Uri.parse(savUriStr) : null;

                if (savDocUri == null) {
                    String parentUriStr = parentDocMap.get(romStem);
                    if (parentUriStr != null) {
                        Uri parentUri = Uri.parse(parentUriStr);
                        savDocUri = DocumentsContract.createDocument(
                                getContentResolver(),
                                parentUri,
                                "application/octet-stream",
                                romStem + ".sav"
                        );
                        if (savDocUri != null) {
                            saveDocMap.put(romStem, savDocUri.toString());
                        }
                    }
                }

                if (savDocUri != null) {
                    try (InputStream in = new java.io.FileInputStream(localSav);
                         OutputStream out = getContentResolver().openOutputStream(savDocUri, "wt")) {
                        if (out != null) {
                            byte[] buf = new byte[1 << 16];
                            int n;
                            while ((n = in.read(buf)) > 0) {
                                out.write(buf, 0, n);
                            }
                        }
                    }
                }
            } catch (Exception e) {
                android.util.Log.w("CrabBoy", "Could not sync save to folder: " + e);
            }
        });
    }

    // ---- Picker result -----------------------------------------------------

    @Override
    protected void onActivityResult(int requestCode, int resultCode, Intent data) {
        super.onActivityResult(requestCode, resultCode, data);
        if (resultCode != RESULT_OK || data == null || data.getData() == null) {
            return;
        }
        final Uri uri = data.getData();
        if (requestCode == PICK_FOLDER) {
            final int takeFlags = data.getFlags()
                    & (Intent.FLAG_GRANT_READ_URI_PERMISSION | Intent.FLAG_GRANT_WRITE_URI_PERMISSION);
            try {
                getContentResolver().takePersistableUriPermission(uri, takeFlags);
            } catch (Exception e) {
                android.util.Log.w("CrabBoy", "Failed to take persistable URI permission: " + e);
            }

            String folderName = queryFolderName(uri);
            getSharedPreferences(PREFS_NAME, MODE_PRIVATE)
                    .edit()
                    .putString(KEY_ROM_TREE_URI, uri.toString())
                    .putString(KEY_ROM_FOLDER_NAME, folderName)
                    .apply();

            rescanFolder();
            return;
        }
        if (requestCode == PICK_SKIN) {
            new Thread(() -> importSkin(uri), "skin-import").start();
            return;
        }
        if (requestCode != PICK_ROM) {
            return;
        }
        // Copy off the UI thread: a 32 MiB ROM from cloud storage can be slow.
        new Thread(() -> importRom(uri), "rom-import").start();
    }

    private void importRom(Uri uri) {
        String name = queryName(uri);
        if (name == null) name = "game.gba";
        name = name.replaceAll("[\\\\/:*?\"<>|]", "_");
        String lower = name.toLowerCase(Locale.ROOT);
        boolean isSave = lower.endsWith(".sav");
        if (!(isSave || lower.endsWith(".gba") || lower.endsWith(".gb") || lower.endsWith(".gbc") || lower.endsWith(".nds"))) {
            importError = "\"" + name + "\" is not a supported ROM (.gba, .gb, .gbc, .nds) or .sav file"
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

    /** Copy a picked skin pack to a temporary file; Rust checks and installs it. */
    private void importSkin(Uri uri) {
        String name = queryName(uri);
        if (name == null) name = "skin.zip";
        if (!name.toLowerCase(Locale.ROOT).endsWith(".zip")) {
            importError = "\"" + name + "\" is not a .zip skin";
            return;
        }
        name = name.replaceAll("[\\\\/:*?\"<>|]", "_");
        File dest = new File(getCacheDir(), name);
        long total = 0;
        try (InputStream in = getContentResolver().openInputStream(uri);
             OutputStream out = new FileOutputStream(dest)) {
            if (in == null) throw new java.io.IOException("cannot open file");
            byte[] buf = new byte[1 << 16];
            int n;
            while ((n = in.read(buf)) > 0) {
                total += n;
                if (total > MAX_SKIN_BYTES) throw new java.io.IOException("file is larger than 16 MB");
                out.write(buf, 0, n);
            }
        } catch (Exception e) {
            //noinspection ResultOfMethodCallIgnored
            dest.delete();
            importError = "Skin import failed: " + e.getMessage();
            return;
        }
        importedSkin = dest.getAbsolutePath();
    }

    private static class DirEntry {
        final String docId;
        final int depth;
        DirEntry(String docId, int depth) {
            this.docId = docId;
            this.depth = depth;
        }
    }

    private void scanRomTree(Uri treeUri) {
        scanning = true;
        try {
            File romDir = new File(getFilesDir(), "roms");
            if (!romDir.isDirectory() && !romDir.mkdirs()) {
                scanNotice = "Cannot create ROM folder";
                scanning = false;
                return;
            }

            String rootDocId = DocumentsContract.getTreeDocumentId(treeUri);
            Queue<DirEntry> queue = new ArrayDeque<>();
            queue.add(new DirEntry(rootDocId, 0));

            int newRoms = 0;
            int totalRoms = 0;
            int savesSynced = 0;

            while (!queue.isEmpty()) {
                DirEntry current = queue.poll();
                Uri childrenUri = DocumentsContract.buildChildDocumentsUriUsingTree(treeUri, current.docId);

                try (Cursor c = getContentResolver().query(
                        childrenUri,
                        new String[]{
                                DocumentsContract.Document.COLUMN_DOCUMENT_ID,
                                DocumentsContract.Document.COLUMN_DISPLAY_NAME,
                                DocumentsContract.Document.COLUMN_MIME_TYPE,
                                DocumentsContract.Document.COLUMN_SIZE,
                                DocumentsContract.Document.COLUMN_LAST_MODIFIED
                        },
                        null, null, null)) {

                    if (c == null) continue;

                    while (c.moveToNext()) {
                        String docId = c.getString(0);
                        String name = c.getString(1);
                        String mimeType = c.getString(2);
                        long size = c.isNull(3) ? 0 : c.getLong(3);
                        long lastModified = c.isNull(4) ? 0 : c.getLong(4);

                        if (DocumentsContract.Document.MIME_TYPE_DIR.equals(mimeType)) {
                            if (current.depth < 3) {
                                queue.add(new DirEntry(docId, current.depth + 1));
                            }
                            continue;
                        }

                        if (name == null) continue;
                        String lower = name.toLowerCase(Locale.ROOT);
                        String safeName = name.replaceAll("[\\\\/:*?\"<>|]", "_");

                        boolean isRom = lower.endsWith(".gba") || lower.endsWith(".gb")
                                || lower.endsWith(".gbc") || lower.endsWith(".nds");
                        boolean isSave = lower.endsWith(".sav");

                        if (isRom) {
                            totalRoms++;
                            int dotIdx = safeName.lastIndexOf('.');
                            String stem = dotIdx > 0 ? safeName.substring(0, dotIdx) : safeName;
                            Uri parentDocUri = DocumentsContract.buildDocumentUriUsingTree(treeUri, current.docId);
                            parentDocMap.put(stem, parentDocUri.toString());

                            File dest = new File(romDir, safeName);
                            if (!dest.exists() || (size > 0 && dest.length() != size)) {
                                File tmp = new File(romDir, safeName + ".part");
                                Uri docUri = DocumentsContract.buildDocumentUriUsingTree(treeUri, docId);
                                try (InputStream in = getContentResolver().openInputStream(docUri);
                                     OutputStream out = new FileOutputStream(tmp)) {
                                    if (in != null) {
                                        byte[] buf = new byte[1 << 16];
                                        int n;
                                        long total = 0;
                                        while ((n = in.read(buf)) > 0) {
                                            total += n;
                                            out.write(buf, 0, n);
                                        }
                                        if (total > 0 && tmp.renameTo(dest)) {
                                            newRoms++;
                                        } else {
                                            //noinspection ResultOfMethodCallIgnored
                                            tmp.delete();
                                        }
                                    }
                                } catch (Exception e) {
                                    //noinspection ResultOfMethodCallIgnored
                                    tmp.delete();
                                }
                            }
                        } else if (isSave) {
                            int dotIdx = safeName.lastIndexOf('.');
                            String stem = dotIdx > 0 ? safeName.substring(0, dotIdx) : safeName;
                            Uri docUri = DocumentsContract.buildDocumentUriUsingTree(treeUri, docId);
                            saveDocMap.put(stem, docUri.toString());

                            File localSav = new File(romDir, safeName);
                            if (!localSav.exists() || (lastModified > 0 && lastModified > localSav.lastModified() + 2000)) {
                                File tmp = new File(romDir, safeName + ".part");
                                try (InputStream in = getContentResolver().openInputStream(docUri);
                                     OutputStream out = new FileOutputStream(tmp)) {
                                    if (in != null) {
                                        byte[] buf = new byte[1 << 16];
                                        int n;
                                        while ((n = in.read(buf)) > 0) {
                                            out.write(buf, 0, n);
                                        }
                                        if (tmp.renameTo(localSav)) {
                                            if (lastModified > 0) {
                                                //noinspection ResultOfMethodCallIgnored
                                                localSav.setLastModified(lastModified);
                                            }
                                            savesSynced++;
                                        } else {
                                            //noinspection ResultOfMethodCallIgnored
                                            tmp.delete();
                                        }
                                    }
                                } catch (Exception e) {
                                    //noinspection ResultOfMethodCallIgnored
                                    tmp.delete();
                                }
                            }
                        }
                    }
                } catch (Exception e) {
                    android.util.Log.w("CrabBoy", "Error querying tree directory: " + e);
                }
            }

            scanNotice = "Scan complete: " + totalRoms + " games (" + newRoms + " newly imported"
                    + (savesSynced > 0 ? ", " + savesSynced + " saves updated" : "") + ")";
        } catch (Exception e) {
            scanNotice = "Scan failed: " + e.getMessage();
        } finally {
            scanning = false;
        }
    }

    private String queryFolderName(Uri uri) {
        try {
            String docId = DocumentsContract.getTreeDocumentId(uri);
            Uri docUri = DocumentsContract.buildDocumentUriUsingTree(uri, docId);
            try (Cursor c = getContentResolver().query(docUri,
                    new String[]{DocumentsContract.Document.COLUMN_DISPLAY_NAME}, null, null, null)) {
                if (c != null && c.moveToFirst()) {
                    String name = c.getString(0);
                    if (name != null && !name.isEmpty()) return name;
                }
            }
            int idx = docId.lastIndexOf(':');
            if (idx >= 0 && idx + 1 < docId.length()) {
                return docId.substring(idx + 1);
            }
            return docId;
        } catch (Exception ignored) {
        }
        String last = uri.getLastPathSegment();
        return last != null ? last : "ROMs";
    }

    private String queryName(Uri uri) {
        try (Cursor c = getContentResolver().query(uri,
                new String[]{OpenableColumns.DISPLAY_NAME}, null, null, null)) {
            if (c != null && c.moveToFirst()) return c.getString(0);
        } catch (Exception ignored) {
        }
        String last = uri.getLastPathSegment();
        return last != null ? last : null;
    }
}
