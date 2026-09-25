# Skins for the Android controls

The **Menu → Skin & controls…** screen changes how the on-screen buttons look and where they sit.

## Settings

- **Skin.** Six built-in skins: Classic, Indigo, Glacier, Famicom, Outline and OLED Black. Imported skins are listed below them. You can remove an imported skin with the **Remove** button next to it.
- **Size.** Scales all buttons, from 50% to 200%.
- **Opacity.** Sets how see-through the buttons are, from 20% to 100%.
- **Touch controls.**
  - **Auto** hides the buttons while a controller is in use.
  - **Always** keeps the buttons on screen all the time.
  - **Hidden** shows only the menu button, for controller or TV play.
- **Move & resize buttons…**
  - Drag any button to move it.
  - Tap a button, then use **−** and **+** to resize it.
  - **Reset** puts the selected button back where it started.
  - Portrait and landscape layouts are saved separately.
  - One-handed mode (Accessibility) keeps its own stacked layout.

All of this is saved in `files/skin.json`.

## Included examples

- `docs/skins/make_sample_skin.py` builds **Gold Rush**, a still skin.
- `docs/skins/make_synthwave_skin.py` builds **Synthwave**, an animated skin:
  - A striped retro sun and a neon grid rolling towards you, with twinkling stars.
  - Pulsing neon buttons that flare when pressed.

  Run `python3 docs/skins/make_synthwave_skin.py Synthwave.zip`; it needs Pillow.

## Making a skin

A skin is a `.zip` file with a `skin.json`, plus PNG images if you want them. The files can sit at the top level of the zip or inside one folder.

```json
{
  "name": "Gold Rush",
  "author": "you",
  "colors": {
    "fill": "#28200FC0",
    "pressed": "#F0C85AE0",
    "edge": "#F0C85AFF",
    "label": "#FFF0C8FF",
    "background": "#1E140A"
  },
  "images": {
    "a": "a.png",
    "a_pressed": "a_down.png",
    "b": "b.png",
    "dpad": "dpad.png",
    "background_portrait": "bg_portrait.png",
    "background_landscape": "bg_landscape.png"
  },
  "layout": {
    "portrait": { "a": { "dx": 0.0, "dy": -0.02, "scale": 1.15 } },
    "landscape": {}
  }
}
```

Every field is optional.

- **`colors`**
  - Written as `#RRGGBB` or `#RRGGBBAA`. The `AA` part is transparency.
  - Colours you leave out are taken from the Classic skin.
  - `fill` is the button colour and `pressed` is its colour while held.
  - `edge` is the outline colour and `label` is the text and arrow colour.
  - `background` is the colour behind the game.
- **`images`**
  - Image slots are named `dpad`, `a`, `b`, `l`, `r`, `start`, `select`, `menu` and `fast`.
  - Add `_pressed` to a name for the image shown while the button is held, for example `a_pressed`. Without one, the normal image is darkened while held.
  - `background_portrait` and `background_landscape` fill the whole screen behind the game.
  - Images must be PNG files, 2048×2048 pixels or smaller. They're stretched to fit the button, so square images work best for round buttons and the D-pad.
- **`animations`**
  - Makes an image move. Each entry is keyed by an image slot, for example `"a"` or `"background_portrait"`.
  - `frames` is the number of animation frames in the image.
  - `columns` sets how many frames sit side by side. The default is 1, meaning the frames are stacked top to bottom. Frames are read left to right, then top to bottom.
  - `fps` is the playback speed, from 0 to 60 frames per second.
  - `scroll_x` and `scroll_y` make an image scroll and wrap around, in image widths or heights per second (-10 to 10).
  - The image has to divide evenly into the frames.
  - Example:

    ```json
    "animations": {
      "a": { "frames": 8, "fps": 6 },
      "background_portrait": { "frames": 8, "columns": 4, "fps": 8 }
    }
    ```
- **`layout`**
  - Moves and resizes buttons relative to where they normally sit.
  - `dx` and `dy` are fractions of the screen size; `-0.05` means 5% towards the top or left.
  - `scale` changes the size, from 0.5 to 2.0.
  - Moves the player makes in the editor are added on top of the skin's own layout.

The whole skin can be at most 16 MB. Importing checks it, and it's turned down with a message if something is wrong: a missing image, a file that isn't a PNG, an unknown image slot, a badly written colour, or a damaged zip.

Importing a skin with the same name again replaces the earlier version.
