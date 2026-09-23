//! Keyboard and Gamepad Input: bindings, remapping, multi-pad selection.
//!
//! Design notes that matter for behaviour:
//!
//! * **Bindings are a list per action**, not one input per action. The stock
//!   profile binds GBA `A` to both East and North so an 8BitDo's two right-hand
//!   face buttons both work, and it lets a user *add* a paddle without losing
//!   the face button.
//!
//! * **Chords mask their members.** `Select+Start = fullscreen` must not also
//!   push Select and Start into the running game, or every fullscreen toggle
//!   would open the in-game menu. When a chord fires, both of its buttons are
//!   suppressed for every other binding that frame.
//!
//! * **Edge vs level.** Hotkeys (pause, quick save, guide) fire on the press
//!   edge; game buttons, turbo, rewind and guide scrolling are level-triggered,
//!   because holding them is the point.
//!
//! * **Input is read from one pad at a time** (the *active* pad) unless the
//!   user opts into merging every connected pad. Reading all pads by default
//!   means a drifting second controller left on the couch fights the player.

use crate::gba::keypad::Key;
use egui::Key as EKey;
use gilrs::{Axis, Button, Event, EventType, Gilrs, PowerInfo};
use serde::{Deserialize, Deserializer, Serialize, Serializer};
use std::collections::{BTreeMap, HashSet};
use std::fmt;
use std::time::{Duration, Instant};

// ---------------------------------------------------------------------------
// Keyboard
// ---------------------------------------------------------------------------

/// Keyboard bindings. Serialized by egui `Key` *name* ("ArrowLeft", "F5") so
/// the config file stays hand-editable and survives egui renumbering its enum.
#[derive(Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct KeyBindings {
    #[serde(with = "ekey")] pub a: EKey,
    #[serde(with = "ekey")] pub b: EKey,
    #[serde(with = "ekey")] pub select: EKey,
    #[serde(with = "ekey")] pub start: EKey,
    #[serde(with = "ekey")] pub right: EKey,
    #[serde(with = "ekey")] pub left: EKey,
    #[serde(with = "ekey")] pub up: EKey,
    #[serde(with = "ekey")] pub down: EKey,
    #[serde(with = "ekey")] pub r: EKey,
    #[serde(with = "ekey")] pub l: EKey,
    #[serde(with = "ekey")] pub turbo: EKey,
    #[serde(with = "ekey")] pub rewind: EKey,
    #[serde(with = "ekey")] pub pause: EKey,
    #[serde(with = "ekey")] pub frame_step: EKey,
    #[serde(with = "ekey")] pub quick_save: EKey,
    #[serde(with = "ekey")] pub quick_load: EKey,
    #[serde(with = "ekey")] pub reset: EKey,
    #[serde(with = "ekey")] pub fullscreen: EKey,
    #[serde(with = "ekey")] pub screenshot: EKey,
}

impl Default for KeyBindings {
    fn default() -> Self {
        Self {
            a: EKey::Z,
            b: EKey::X,
            select: EKey::Backspace,
            start: EKey::Enter,
            right: EKey::ArrowRight,
            left: EKey::ArrowLeft,
            up: EKey::ArrowUp,
            down: EKey::ArrowDown,
            r: EKey::S,
            l: EKey::A,
            turbo: EKey::Space,
            rewind: EKey::Tab,
            pause: EKey::P,
            frame_step: EKey::F,
            quick_save: EKey::F5,
            quick_load: EKey::F8,
            reset: EKey::R,
            fullscreen: EKey::F11,
            screenshot: EKey::F12,
        }
    }
}

impl KeyBindings {
    /// One-handed keyboard layout for the left hand (ROADMAP M10): the hand
    /// rests on WASD and everything is within reach around it. Avoids the
    /// number row (save-slot keys) and every Ctrl-free hotkey.
    pub fn left_handed_one_hand() -> Self {
        Self {
            up: EKey::W,
            left: EKey::A,
            down: EKey::S,
            right: EKey::D,
            a: EKey::E,
            b: EKey::Q,
            l: EKey::Tab,
            r: EKey::R,
            select: EKey::Z,
            start: EKey::X,
            turbo: EKey::Space,
            rewind: EKey::C,
            pause: EKey::Escape,
            frame_step: EKey::V,
            quick_save: EKey::F5,
            quick_load: EKey::F8,
            // Ctrl+T resets.
            reset: EKey::T,
            fullscreen: EKey::F11,
            screenshot: EKey::F12,
        }
    }

    /// One-handed keyboard layout for the right hand: IJKL D-pad with the
    /// buttons around it.
    pub fn right_handed_one_hand() -> Self {
        Self {
            up: EKey::I,
            left: EKey::J,
            down: EKey::K,
            right: EKey::L,
            a: EKey::O,
            b: EKey::U,
            l: EKey::Y,
            r: EKey::P,
            select: EKey::M,
            start: EKey::Enter,
            turbo: EKey::H,
            rewind: EKey::Backspace,
            pause: EKey::Escape,
            frame_step: EKey::Semicolon,
            quick_save: EKey::F5,
            quick_load: EKey::F8,
            // Ctrl+Delete resets.
            reset: EKey::Delete,
            fullscreen: EKey::F11,
            screenshot: EKey::F12,
        }
    }

    /// The bindings in effect for a one-handed layout choice. `Standard`
    /// is the user's own (possibly remapped) bindings, which the presets
    /// never overwrite.
    pub fn for_layout(&self, layout: crate::gba::accessibility::Handedness) -> Self {
        use crate::gba::accessibility::Handedness;
        match layout {
            Handedness::Standard => self.clone(),
            Handedness::LeftHand => Self::left_handed_one_hand(),
            Handedness::RightHand => Self::right_handed_one_hand(),
        }
    }

    /// (GBA button name, key name) pairs, for showing a layout to the user.
    pub fn game_button_summary(&self) -> Vec<(&'static str, &'static str)> {
        vec![
            ("Up", self.up.name()),
            ("Down", self.down.name()),
            ("Left", self.left.name()),
            ("Right", self.right.name()),
            ("A", self.a.name()),
            ("B", self.b.name()),
            ("L", self.l.name()),
            ("R", self.r.name()),
            ("Select", self.select.name()),
            ("Start", self.start.name()),
            ("Turbo", self.turbo.name()),
            ("Rewind", self.rewind.name()),
        ]
    }
}

/// `serde` adapter for `egui::Key` <-> its name string.
mod ekey {
    use super::EKey;
    use serde::{Deserialize, Deserializer, Serializer};

    pub fn serialize<S: Serializer>(k: &EKey, s: S) -> Result<S::Ok, S::Error> {
        s.serialize_str(k.name())
    }

    pub fn deserialize<'de, D: Deserializer<'de>>(d: D) -> Result<EKey, D::Error> {
        let s = String::deserialize(d)?;
        EKey::from_name(&s).ok_or_else(|| serde::de::Error::custom(format!("unknown key `{}`", s)))
    }
}

// ---------------------------------------------------------------------------
// Pad actions
// ---------------------------------------------------------------------------

/// Everything a controller can be bound to.
///
/// The first ten map 1:1 onto the GBA keypad in hardware bit order, which is
/// the same order the TAS engine and `handle_input` already use; the rest are
/// emulator-level hotkeys.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Debug, Serialize, Deserialize)]
pub enum PadAction {
    A,
    B,
    Select,
    Start,
    Right,
    Left,
    Up,
    Down,
    R,
    L,
    Turbo,
    Rewind,
    Pause,
    QuickSave,
    QuickLoad,
    Fullscreen,
    Screenshot,
    /// Open/close the illustrated strategy guide.
    GuideToggle,
    GuideScrollUp,
    GuideScrollDown,
    GuidePrevPage,
    GuideNextPage,
}

impl PadAction {
    pub const ALL: [PadAction; 22] = [
        PadAction::A,
        PadAction::B,
        PadAction::Select,
        PadAction::Start,
        PadAction::Right,
        PadAction::Left,
        PadAction::Up,
        PadAction::Down,
        PadAction::R,
        PadAction::L,
        PadAction::Turbo,
        PadAction::Rewind,
        PadAction::Pause,
        PadAction::QuickSave,
        PadAction::QuickLoad,
        PadAction::Fullscreen,
        PadAction::Screenshot,
        PadAction::GuideToggle,
        PadAction::GuideScrollUp,
        PadAction::GuideScrollDown,
        PadAction::GuidePrevPage,
        PadAction::GuideNextPage,
    ];

    /// Index into the state arrays. Matches `ALL`'s order.
    pub fn index(self) -> usize {
        PadAction::ALL.iter().position(|a| *a == self).unwrap_or(0)
    }

    /// The first ten actions are GBA keypad buttons.
    pub fn is_game_button(self) -> bool {
        self.index() < 10
    }

    pub fn label(self) -> &'static str {
        match self {
            PadAction::A => "A",
            PadAction::B => "B",
            PadAction::Select => "Select",
            PadAction::Start => "Start",
            PadAction::Right => "D-Pad Right",
            PadAction::Left => "D-Pad Left",
            PadAction::Up => "D-Pad Up",
            PadAction::Down => "D-Pad Down",
            PadAction::R => "R Shoulder",
            PadAction::L => "L Shoulder",
            PadAction::Turbo => "Turbo (hold)",
            PadAction::Rewind => "Rewind (hold)",
            PadAction::Pause => "Pause / Resume",
            PadAction::QuickSave => "Quick Save",
            PadAction::QuickLoad => "Quick Load",
            PadAction::Fullscreen => "Fullscreen",
            PadAction::Screenshot => "Screenshot",
            PadAction::GuideToggle => "Open / Close Guide",
            PadAction::GuideScrollUp => "Guide: Scroll Up",
            PadAction::GuideScrollDown => "Guide: Scroll Down",
            PadAction::GuidePrevPage => "Guide: Previous Page",
            PadAction::GuideNextPage => "Guide: Next Page",
        }
    }

    /// Grouping for the remap UI, so the dialog does not present 22 flat rows.
    pub fn group(self) -> &'static str {
        match self {
            a if a.is_game_button() => "Game Buttons",
            PadAction::GuideToggle
            | PadAction::GuideScrollUp
            | PadAction::GuideScrollDown
            | PadAction::GuidePrevPage
            | PadAction::GuideNextPage => "Strategy Guide",
            _ => "Emulator Hotkeys",
        }
    }

    /// Hotkeys fire once per press; game buttons and held modifiers are level
    /// triggered.
    pub fn is_edge_triggered(self) -> bool {
        matches!(
            self,
            PadAction::Pause
                | PadAction::QuickSave
                | PadAction::QuickLoad
                | PadAction::Fullscreen
                | PadAction::Screenshot
                | PadAction::GuideToggle
                | PadAction::GuidePrevPage
                | PadAction::GuideNextPage
        )
    }
}

// ---------------------------------------------------------------------------
// Pad inputs
// ---------------------------------------------------------------------------

#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Debug)]
pub enum AxisDir {
    Positive,
    Negative,
}

/// A single physical thing on a controller that can be bound.
///
/// Deliberately not `Ord`: gilrs's `Button`/`Axis` are not ordered, and an
/// artificial ordering here would only invite someone to key a BTreeMap on it.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub enum PadInput {
    Button(Button),
    /// A stick or analog trigger pushed past the deadzone in one direction.
    Axis(Axis, AxisDir),
    /// Two buttons held together. Firing a chord suppresses both members for
    /// every other binding in the same frame.
    Chord(Button, Button),
}

impl PadInput {
    /// Normalizes chord member order so `Select+Start` and `Start+Select`
    /// compare and serialize identically.
    pub fn chord(a: Button, b: Button) -> Self {
        if (a as u16) <= (b as u16) {
            PadInput::Chord(a, b)
        } else {
            PadInput::Chord(b, a)
        }
    }

    pub fn is_chord(self) -> bool {
        matches!(self, PadInput::Chord(_, _))
    }
}

fn button_name(b: Button) -> &'static str {
    match b {
        Button::South => "South",
        Button::East => "East",
        Button::North => "North",
        Button::West => "West",
        Button::C => "C",
        Button::Z => "Z",
        Button::LeftTrigger => "LeftTrigger",
        Button::LeftTrigger2 => "LeftTrigger2",
        Button::RightTrigger => "RightTrigger",
        Button::RightTrigger2 => "RightTrigger2",
        Button::Select => "Select",
        Button::Start => "Start",
        Button::Mode => "Mode",
        Button::LeftThumb => "LeftThumb",
        Button::RightThumb => "RightThumb",
        Button::DPadUp => "DPadUp",
        Button::DPadDown => "DPadDown",
        Button::DPadLeft => "DPadLeft",
        Button::DPadRight => "DPadRight",
        Button::Unknown => "Unknown",
    }
}

fn button_from_name(s: &str) -> Option<Button> {
    Some(match s {
        "South" => Button::South,
        "East" => Button::East,
        "North" => Button::North,
        "West" => Button::West,
        "C" => Button::C,
        "Z" => Button::Z,
        "LeftTrigger" => Button::LeftTrigger,
        "LeftTrigger2" => Button::LeftTrigger2,
        "RightTrigger" => Button::RightTrigger,
        "RightTrigger2" => Button::RightTrigger2,
        "Select" => Button::Select,
        "Start" => Button::Start,
        "Mode" => Button::Mode,
        "LeftThumb" => Button::LeftThumb,
        "RightThumb" => Button::RightThumb,
        "DPadUp" => Button::DPadUp,
        "DPadDown" => Button::DPadDown,
        "DPadLeft" => Button::DPadLeft,
        "DPadRight" => Button::DPadRight,
        _ => return None,
    })
}

fn axis_name(a: Axis) -> &'static str {
    match a {
        Axis::LeftStickX => "LeftStickX",
        Axis::LeftStickY => "LeftStickY",
        Axis::LeftZ => "LeftZ",
        Axis::RightStickX => "RightStickX",
        Axis::RightStickY => "RightStickY",
        Axis::RightZ => "RightZ",
        Axis::DPadX => "DPadX",
        Axis::DPadY => "DPadY",
        Axis::Unknown => "Unknown",
    }
}

fn axis_from_name(s: &str) -> Option<Axis> {
    Some(match s {
        "LeftStickX" => Axis::LeftStickX,
        "LeftStickY" => Axis::LeftStickY,
        "LeftZ" => Axis::LeftZ,
        "RightStickX" => Axis::RightStickX,
        "RightStickY" => Axis::RightStickY,
        "RightZ" => Axis::RightZ,
        "DPadX" => Axis::DPadX,
        "DPadY" => Axis::DPadY,
        _ => return None,
    })
}

/// Wire format: `South`, `LeftStickY+`, `Select+Start`. Chosen over a tagged
/// enum so a user editing config.json by hand can actually read it.
impl fmt::Display for PadInput {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            PadInput::Button(b) => write!(f, "{}", button_name(*b)),
            PadInput::Axis(a, AxisDir::Positive) => write!(f, "{}+", axis_name(*a)),
            PadInput::Axis(a, AxisDir::Negative) => write!(f, "{}-", axis_name(*a)),
            PadInput::Chord(a, b) => write!(f, "{}+{}", button_name(*a), button_name(*b)),
        }
    }
}

impl std::str::FromStr for PadInput {
    type Err = String;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        let s = s.trim();
        // Chord: two button names joined by '+', e.g. "Select+Start".
        if let Some((l, r)) = s.split_once('+') {
            if !r.is_empty() {
                let (lb, rb) = (button_from_name(l), button_from_name(r));
                if let (Some(lb), Some(rb)) = (lb, rb) {
                    return Ok(PadInput::chord(lb, rb));
                }
            }
        }
        // Axis with a trailing sign, e.g. "LeftStickY+" / "LeftStickY-".
        if let Some(stripped) = s.strip_suffix('+') {
            if let Some(ax) = axis_from_name(stripped) {
                return Ok(PadInput::Axis(ax, AxisDir::Positive));
            }
        }
        if let Some(stripped) = s.strip_suffix('-') {
            if let Some(ax) = axis_from_name(stripped) {
                return Ok(PadInput::Axis(ax, AxisDir::Negative));
            }
        }
        button_from_name(s)
            .map(PadInput::Button)
            .ok_or_else(|| format!("unknown pad input `{}`", s))
    }
}

impl Serialize for PadInput {
    fn serialize<S: Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        s.serialize_str(&self.to_string())
    }
}

impl<'de> Deserialize<'de> for PadInput {
    fn deserialize<D: Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        let s = String::deserialize(d)?;
        s.parse().map_err(serde::de::Error::custom)
    }
}

/// Human-facing name, using the labels printed on an 8BitDo shell rather than
/// gilrs's compass names ("South" means nothing to a player looking at a pad).
pub fn pretty_input(input: PadInput, layout: FaceLayout) -> String {
    match input {
        PadInput::Button(b) => pretty_button(b, layout).to_string(),
        PadInput::Axis(a, d) => pretty_axis(a, d).to_string(),
        PadInput::Chord(a, b) => {
            format!("{} + {}", pretty_button(a, layout), pretty_button(b, layout))
        }
    }
}

fn pretty_button(b: Button, layout: FaceLayout) -> &'static str {
    match b {
        // Face-button glyphs depend on how the pad is labelled.
        Button::South => match layout {
            FaceLayout::Nintendo => "B (bottom)",
            FaceLayout::Xbox => "A (bottom)",
        },
        Button::East => match layout {
            FaceLayout::Nintendo => "A (right)",
            FaceLayout::Xbox => "B (right)",
        },
        Button::North => match layout {
            FaceLayout::Nintendo => "X (top)",
            FaceLayout::Xbox => "Y (top)",
        },
        Button::West => match layout {
            FaceLayout::Nintendo => "Y (left)",
            FaceLayout::Xbox => "X (left)",
        },
        Button::C => "Extra button (C)",
        Button::Z => "Extra button (Z)",
        Button::LeftTrigger => "L1 bumper",
        Button::LeftTrigger2 => "L2 trigger",
        Button::RightTrigger => "R1 bumper",
        Button::RightTrigger2 => "R2 trigger",
        Button::Select => "Select / Minus",
        Button::Start => "Start / Plus",
        Button::Mode => "Home",
        Button::LeftThumb => "L3 (left stick click)",
        Button::RightThumb => "R3 (right stick click)",
        Button::DPadUp => "D-Pad Up",
        Button::DPadDown => "D-Pad Down",
        Button::DPadLeft => "D-Pad Left",
        Button::DPadRight => "D-Pad Right",
        Button::Unknown => "Unrecognised button",
    }
}

fn pretty_axis(a: Axis, d: AxisDir) -> &'static str {
    let pos = matches!(d, AxisDir::Positive);
    match a {
        Axis::LeftStickX => if pos { "Left Stick Right" } else { "Left Stick Left" },
        Axis::LeftStickY => if pos { "Left Stick Up" } else { "Left Stick Down" },
        Axis::RightStickX => if pos { "Right Stick Right" } else { "Right Stick Left" },
        Axis::RightStickY => if pos { "Right Stick Up" } else { "Right Stick Down" },
        Axis::LeftZ => if pos { "L2 analog" } else { "L2 analog (neg)" },
        Axis::RightZ => if pos { "R2 analog" } else { "R2 analog (neg)" },
        Axis::DPadX => if pos { "D-Pad Right (axis)" } else { "D-Pad Left (axis)" },
        Axis::DPadY => if pos { "D-Pad Up (axis)" } else { "D-Pad Down (axis)" },
        Axis::Unknown => "Unrecognised axis",
    }
}

/// How the pad's face buttons are *labelled*, which is independent of the
/// compass positions gilrs reports.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Serialize, Deserialize)]
pub enum FaceLayout {
    /// 8BitDo / Nintendo: physical "A" sits on the right (East).
    Nintendo,
    /// Xbox: physical "A" sits at the bottom (South).
    Xbox,
}

// ---------------------------------------------------------------------------
// Profiles
// ---------------------------------------------------------------------------

/// A named, fully remappable binding set.
#[derive(Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct PadProfile {
    pub name: String,
    /// Built-ins ship with the binary and cannot be deleted; editing one
    /// silently forks it into a user copy so an update can still fix the stock
    /// mapping without stomping the user's edits.
    pub builtin: bool,
    pub layout: FaceLayout,
    pub deadzone: f32,
    /// Analog guide-scrolling speed, in points per second at full deflection.
    pub guide_scroll_speed: f32,
    /// Rate a held D-Pad repeats guide page turns / scroll steps.
    pub bindings: BTreeMap<PadAction, Vec<PadInput>>,
}

impl Default for PadProfile {
    fn default() -> Self {
        Self::ultimate2_pro()
    }
}

impl PadProfile {
    /// Bindings shared by every stock profile: d-pad + left stick movement,
    /// shoulders, and the emulator hotkeys.
    fn common(layout: FaceLayout) -> BTreeMap<PadAction, Vec<PadInput>> {
        use Button::*;
        use PadAction as P;
        let mut m: BTreeMap<PadAction, Vec<PadInput>> = BTreeMap::new();

        // Face buttons. GBA A/B are bound to *two* physical buttons each so
        // the "wrong" thumb position still works -- a long-standing comfort
        // feature of this emulator that predates remapping.
        let (a_main, a_alt, b_main, b_alt) = match layout {
            FaceLayout::Nintendo => (East, North, South, West),
            FaceLayout::Xbox => (South, West, East, North),
        };
        m.insert(P::A, vec![PadInput::Button(a_main), PadInput::Button(a_alt)]);
        m.insert(P::B, vec![PadInput::Button(b_main), PadInput::Button(b_alt)]);

        m.insert(P::Select, vec![PadInput::Button(Select)]);
        m.insert(P::Start, vec![PadInput::Button(Start)]);

        // D-Pad plus left stick. DPadX/DPadY cover pads whose hat reports as
        // an axis instead of four buttons (common on 2.4GHz dongles).
        m.insert(
            P::Right,
            vec![
                PadInput::Button(DPadRight),
                PadInput::Axis(Axis::LeftStickX, AxisDir::Positive),
                PadInput::Axis(Axis::DPadX, AxisDir::Positive),
            ],
        );
        m.insert(
            P::Left,
            vec![
                PadInput::Button(DPadLeft),
                PadInput::Axis(Axis::LeftStickX, AxisDir::Negative),
                PadInput::Axis(Axis::DPadX, AxisDir::Negative),
            ],
        );
        m.insert(
            P::Up,
            vec![
                PadInput::Button(DPadUp),
                PadInput::Axis(Axis::LeftStickY, AxisDir::Positive),
                PadInput::Axis(Axis::DPadY, AxisDir::Positive),
            ],
        );
        m.insert(
            P::Down,
            vec![
                PadInput::Button(DPadDown),
                PadInput::Axis(Axis::LeftStickY, AxisDir::Negative),
                PadInput::Axis(Axis::DPadY, AxisDir::Negative),
            ],
        );

        m.insert(P::L, vec![PadInput::Button(LeftTrigger)]);
        m.insert(P::R, vec![PadInput::Button(RightTrigger)]);

        // Emulator hotkeys.
        m.insert(P::Turbo, vec![PadInput::Button(RightTrigger2)]);
        m.insert(P::Rewind, vec![PadInput::Button(LeftTrigger2)]);
        m.insert(P::Pause, vec![PadInput::Button(Mode)]);
        m.insert(P::QuickSave, vec![PadInput::Button(LeftThumb)]);
        m.insert(P::QuickLoad, vec![PadInput::Button(RightThumb)]);
        m.insert(P::Fullscreen, vec![PadInput::chord(Select, Start)]);
        m.insert(P::Screenshot, vec![]);

        // Guide. The toggle is a chord so it cannot be hit mid-battle, and the
        // navigation bindings deliberately reuse the d-pad/shoulders: while the
        // guide has pad capture those inputs do not reach the game, so there is
        // nothing to collide with.
        m.insert(
            P::GuideToggle,
            vec![PadInput::chord(Select, RightTrigger), PadInput::Button(C)],
        );
        m.insert(
            P::GuideScrollUp,
            vec![
                PadInput::Button(DPadUp),
                PadInput::Axis(Axis::LeftStickY, AxisDir::Positive),
                PadInput::Axis(Axis::RightStickY, AxisDir::Positive),
            ],
        );
        m.insert(
            P::GuideScrollDown,
            vec![
                PadInput::Button(DPadDown),
                PadInput::Axis(Axis::LeftStickY, AxisDir::Negative),
                PadInput::Axis(Axis::RightStickY, AxisDir::Negative),
            ],
        );
        m.insert(
            P::GuidePrevPage,
            vec![PadInput::Button(LeftTrigger), PadInput::Button(DPadLeft)],
        );
        m.insert(
            P::GuideNextPage,
            vec![PadInput::Button(RightTrigger), PadInput::Button(DPadRight)],
        );
        m
    }

    /// 8BitDo Ultimate 2 (Pro) — Hall-effect sticks, 2.4GHz dongle.
    ///
    /// Tuned for this pad specifically: the Hall sensors have essentially no
    /// mechanical slop, so the stock 0.35 deadzone (sized for worn potentiometer
    /// sticks) throws away a third of the usable travel and makes the stick feel
    /// mushy. 0.18 is comfortably outside this pad's measured centre noise.
    pub fn ultimate2_pro() -> Self {
        Self {
            name: "8BitDo Ultimate 2 (Pro)".to_string(),
            builtin: true,
            layout: FaceLayout::Nintendo,
            deadzone: 0.18,
            guide_scroll_speed: 900.0,
            bindings: Self::common(FaceLayout::Nintendo),
        }
    }

    pub fn nintendo_generic() -> Self {
        Self {
            name: "8BitDo / Nintendo Layout".to_string(),
            builtin: true,
            layout: FaceLayout::Nintendo,
            deadzone: 0.35,
            guide_scroll_speed: 800.0,
            bindings: Self::common(FaceLayout::Nintendo),
        }
    }

    pub fn xbox_generic() -> Self {
        Self {
            name: "Xbox / XInput Layout".to_string(),
            builtin: true,
            layout: FaceLayout::Xbox,
            deadzone: 0.35,
            guide_scroll_speed: 800.0,
            bindings: Self::common(FaceLayout::Xbox),
        }
    }

    pub fn builtins() -> Vec<PadProfile> {
        vec![
            Self::ultimate2_pro(),
            Self::nintendo_generic(),
            Self::xbox_generic(),
        ]
    }

    pub fn binds(&self, action: PadAction) -> &[PadInput] {
        self.bindings.get(&action).map(|v| v.as_slice()).unwrap_or(&[])
    }

    /// Every action already using `input`, excluding `except`. Drives the
    /// remap dialog's conflict warning.
    pub fn conflicts(&self, input: PadInput, except: PadAction) -> Vec<PadAction> {
        self.bindings
            .iter()
            .filter(|(action, inputs)| **action != except && inputs.contains(&input))
            .map(|(action, _)| *action)
            .collect()
    }

    pub fn bind(&mut self, action: PadAction, input: PadInput) {
        let entry = self.bindings.entry(action).or_default();
        if !entry.contains(&input) {
            entry.push(input);
        }
    }

    pub fn unbind(&mut self, action: PadAction, input: PadInput) {
        if let Some(entry) = self.bindings.get_mut(&action) {
            entry.retain(|i| *i != input);
        }
    }

    /// Replaces every binding for one action (the "set" path in the UI).
    pub fn rebind_single(&mut self, action: PadAction, input: PadInput) {
        self.bindings.insert(action, vec![input]);
    }
}

// ---------------------------------------------------------------------------
// Settings (persisted)
// ---------------------------------------------------------------------------

#[derive(Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct ControllerSettings {
    /// User profiles. Built-ins are re-seeded on load if missing, so a new
    /// stock profile added by an update shows up without wiping user copies.
    pub profiles: Vec<PadProfile>,
    /// Profile assigned per physical pad, keyed by its UUID. Lets a player keep
    /// an Xbox pad and an Ultimate 2 plugged in with correct mappings on both.
    pub pad_profiles: BTreeMap<String, String>,
    /// UUID of the pad the player last chose as Player 1.
    pub preferred_pad: Option<String>,
    /// Merge input from every connected pad instead of only the active one.
    /// Off by default: a drifting spare pad otherwise fights the player.
    pub merge_all_pads: bool,
    /// While the guide is open, controller input drives the guide and is kept
    /// out of the game. Disable for players who want to walk and read.
    pub guide_captures_pad: bool,
    /// Fallback profile for a pad with no explicit assignment and no name match.
    pub default_profile: String,
}

impl Default for ControllerSettings {
    fn default() -> Self {
        Self {
            profiles: PadProfile::builtins(),
            pad_profiles: BTreeMap::new(),
            preferred_pad: None,
            merge_all_pads: false,
            guide_captures_pad: true,
            default_profile: PadProfile::ultimate2_pro().name,
        }
    }
}

impl ControllerSettings {
    /// Re-adds any built-in profile missing from a loaded config, and repairs a
    /// `default_profile` pointing at something that no longer exists.
    pub fn reseed_builtins(&mut self) {
        for b in PadProfile::builtins() {
            if !self.profiles.iter().any(|p| p.name == b.name) {
                self.profiles.push(b);
            }
        }
        if !self.profiles.iter().any(|p| p.name == self.default_profile) {
            self.default_profile = PadProfile::ultimate2_pro().name;
        }
    }

    pub fn profile(&self, name: &str) -> Option<&PadProfile> {
        self.profiles.iter().find(|p| p.name == name)
    }

    pub fn profile_mut(&mut self, name: &str) -> Option<&mut PadProfile> {
        self.profiles.iter_mut().find(|p| p.name == name)
    }

    /// Picks a profile for a freshly seen pad: explicit assignment first, then
    /// a name heuristic, then the configured default.
    pub fn profile_for(&self, uuid: &str, pad_name: &str) -> String {
        if let Some(assigned) = self.pad_profiles.get(uuid) {
            if self.profile(assigned).is_some() {
                return assigned.clone();
            }
        }
        if let Some(guess) = guess_profile_name(pad_name) {
            if self.profile(&guess).is_some() {
                return guess;
            }
        }
        self.default_profile.clone()
    }
}

/// Maps a pad's reported product name onto a stock profile.
///
/// The Ultimate 2 reports several different strings depending on how it is
/// attached (2.4GHz dongle vs Bluetooth vs USB) and whether it is in XInput or
/// DInput mode, so the match is on substrings rather than an exact name.
pub fn guess_profile_name(pad_name: &str) -> Option<String> {
    let n = pad_name.to_ascii_lowercase();
    let is_8bitdo = n.contains("8bitdo") || n.contains("8bit do");
    let is_ultimate2 = n.contains("ultimate 2")
        || n.contains("ultimate2")
        || n.contains("ultimate v2")
        // The dongle enumerates as a generic Xbox 360 pad in XInput mode; the
        // 8BitDo vendor string is the only reliable tell there.
        || (is_8bitdo && n.contains("ultimate"));

    if is_ultimate2 {
        return Some(PadProfile::ultimate2_pro().name);
    }
    if is_8bitdo || n.contains("nintendo") || n.contains("switch") || n.contains("pro controller") {
        return Some(PadProfile::nintendo_generic().name);
    }
    if n.contains("xbox") || n.contains("xinput") || n.contains("microsoft") {
        return Some(PadProfile::xbox_generic().name);
    }
    None
}

// ---------------------------------------------------------------------------
// Runtime pad state
// ---------------------------------------------------------------------------

/// A connected pad as shown in the selection UI.
#[derive(Clone)]
pub struct PadInfo {
    pub id: gilrs::GamepadId,
    pub name: String,
    pub uuid: String,
    pub profile: String,
    pub power: String,
    /// Set for a short window after the pad last produced input, so the
    /// selection list can show "this one is the pad in your hands".
    pub recently_active: bool,
}

/// One frame of resolved controller state.
#[derive(Clone)]
pub struct PadState {
    /// GBA keypad order: A, B, Select, Start, Right, Left, Up, Down, R, L.
    pub game_keys: [bool; 10],
    held: [bool; PadAction::ALL.len()],
    just: [bool; PadAction::ALL.len()],
    /// Signed analog magnitude for smooth guide scrolling, -1.0..=1.0.
    pub guide_scroll_axis: f32,
}

impl Default for PadState {
    fn default() -> Self {
        Self {
            game_keys: [false; 10],
            held: [false; PadAction::ALL.len()],
            just: [false; PadAction::ALL.len()],
            guide_scroll_axis: 0.0,
        }
    }
}

impl PadState {
    pub fn held(&self, a: PadAction) -> bool {
        self.held[a.index()]
    }
    pub fn pressed(&self, a: PadAction) -> bool {
        self.just[a.index()]
    }
}

/// What the remap UI is currently waiting for.
#[derive(Clone, Copy, PartialEq)]
pub enum CaptureState {
    Idle,
    /// Listening for the next input to bind to this action.
    Listening(PadAction),
}

const CHORD_WINDOW: Duration = Duration::from_millis(220);
const PAGE_REPEAT_DELAY: Duration = Duration::from_millis(420);
const PAGE_REPEAT_RATE: Duration = Duration::from_millis(160);

pub struct GamepadManager {
    gilrs: Option<Gilrs>,
    pub settings: ControllerSettings,
    /// Pads seen this frame, in stable enumeration order.
    pub pads: Vec<PadInfo>,
    /// The pad driving the emulator, or None when nothing is connected.
    pub active_pad: Option<gilrs::GamepadId>,
    pub state: PadState,

    // Remapping
    pub capture: CaptureState,
    /// Input captured by the remap listener, consumed by the dialog.
    pub captured_input: Option<PadInput>,
    capture_started: Option<Instant>,
    /// Buttons currently held, used to promote a second press into a chord.
    held_buttons: HashSet<Button>,
    chord_candidate: Option<(Button, Instant)>,

    // Guide navigation repeat
    page_held_since: [Option<Instant>; 2],
    page_last_fire: [Option<Instant>; 2],

    /// Set when the pad set changed, so the UI can toast and re-resolve.
    pub hotplug_message: Option<String>,
    last_input_at: BTreeMap<String, Instant>,
    /// Guide has pad capture this frame (set by the app before polling).
    pub guide_open: bool,
}

/// Resolves a profile's bindings against a snapshot of pad hardware state.
///
/// Pure and hardware-free on purpose: this is the logic that decides what the
/// player's fingers actually did, and it must be testable without a physical
/// controller plugged into CI. `is_btn` / `axis_val` are the only coupling to
/// gilrs, and the caller supplies them.
///
/// Returns `(held_actions, guide_scroll_axis)`.
pub fn resolve_bindings(
    profile: &PadProfile,
    is_btn: &dyn Fn(Button) -> bool,
    axis_val: &dyn Fn(Axis) -> f32,
) -> ([bool; PadAction::ALL.len()], f32) {
    let mut held = [false; PadAction::ALL.len()];
    let dz = profile.deadzone.clamp(0.02, 0.95);

    // Pass 1: which chords are active? Their member buttons are then invisible
    // to every plain-button binding, so `Select+Start` does not leak a Select
    // and a Start into the game.
    let mut masked: HashSet<Button> = HashSet::new();
    let mut active_chords: HashSet<PadInput> = HashSet::new();
    for action in PadAction::ALL {
        for input in profile.binds(action) {
            if let PadInput::Chord(a, b) = *input {
                if is_btn(a) && is_btn(b) {
                    active_chords.insert(*input);
                    masked.insert(a);
                    masked.insert(b);
                }
            }
        }
    }

    // Pass 2: evaluate every binding, with chord members masked out.
    for action in PadAction::ALL {
        let mut on = false;
        for input in profile.binds(action) {
            match *input {
                PadInput::Button(b) => {
                    if !masked.contains(&b) && is_btn(b) {
                        on = true;
                    }
                }
                PadInput::Axis(a, dir) => {
                    let v = axis_val(a);
                    let past = match dir {
                        AxisDir::Positive => v > dz,
                        AxisDir::Negative => v < -dz,
                    };
                    if past {
                        on = true;
                    }
                }
                PadInput::Chord(_, _) => {
                    if active_chords.contains(input) {
                        on = true;
                    }
                }
            }
            if on {
                break;
            }
        }
        held[action.index()] = on;
    }

    // Analog guide scroll: take the largest deflection among the axes bound to
    // scroll up/down so the stick gives proportional speed rather than the
    // on/off a digital binding would.
    let mut scroll = 0.0f32;
    for (action, sign) in [
        (PadAction::GuideScrollUp, 1.0f32),
        (PadAction::GuideScrollDown, -1.0f32),
    ] {
        for input in profile.binds(action) {
            match *input {
                PadInput::Axis(a, dir) => {
                    let v = axis_val(a);
                    let mag = match dir {
                        AxisDir::Positive if v > dz => (v - dz) / (1.0 - dz),
                        AxisDir::Negative if v < -dz => (-v - dz) / (1.0 - dz),
                        _ => 0.0,
                    };
                    if mag.abs() > scroll.abs() {
                        scroll = mag * sign;
                    }
                }
                // A digital binding scrolls at full speed -- but only when
                // THAT button is actually down. Testing the action's held
                // state here instead would let any stick nudge past the
                // deadzone get promoted to full speed, destroying the
                // proportional control the analog branch just computed.
                PadInput::Button(b) => {
                    if is_btn(b) && scroll.abs() < 1.0 {
                        scroll = sign;
                    }
                }
                PadInput::Chord(a, b) => {
                    if is_btn(a) && is_btn(b) && scroll.abs() < 1.0 {
                        scroll = sign;
                    }
                }
            }
        }
    }

    (held, scroll)
}

impl Default for GamepadManager {
    fn default() -> Self {
        Self::new(ControllerSettings::default())
    }
}

impl GamepadManager {
    pub fn new(mut settings: ControllerSettings) -> Self {
        settings.reseed_builtins();

        let gilrs = match Gilrs::new() {
            Ok(g) => {
                log::info!("Initialized Gilrs gamepad subsystem");
                Some(g)
            }
            Err(e) => {
                log::warn!("Failed to initialize gamepad subsystem: {}", e);
                None
            }
        };

        let mut mgr = Self {
            gilrs,
            settings,
            pads: Vec::new(),
            active_pad: None,
            state: PadState::default(),
            capture: CaptureState::Idle,
            captured_input: None,
            capture_started: None,
            held_buttons: HashSet::new(),
            chord_candidate: None,
            page_held_since: [None, None],
            page_last_fire: [None, None],
            hotplug_message: None,
            last_input_at: BTreeMap::new(),
            guide_open: false,
        };
        mgr.refresh_pads();
        mgr.ensure_active_pad();
        mgr
    }

    /// Name of the active pad, for status bars.
    pub fn active_pad_name(&self) -> Option<&str> {
        self.active_pad
            .and_then(|id| self.pads.iter().find(|p| p.id == id))
            .map(|p| p.name.as_str())
    }

    pub fn active_pad_info(&self) -> Option<&PadInfo> {
        self.active_pad
            .and_then(|id| self.pads.iter().find(|p| p.id == id))
    }

    /// Profile currently driving the active pad.
    pub fn active_profile_name(&self) -> String {
        self.active_pad_info()
            .map(|p| p.profile.clone())
            .unwrap_or_else(|| self.settings.default_profile.clone())
    }

    pub fn active_profile(&self) -> PadProfile {
        let name = self.active_profile_name();
        self.settings
            .profile(&name)
            .cloned()
            .unwrap_or_else(PadProfile::ultimate2_pro)
    }

    /// Makes `id` Player 1 and remembers the choice across launches.
    pub fn select_pad(&mut self, id: gilrs::GamepadId) {
        self.active_pad = Some(id);
        if let Some(info) = self.pads.iter().find(|p| p.id == id) {
            self.settings.preferred_pad = Some(info.uuid.clone());
        }
    }

    /// Assigns a profile to the active pad and persists the association.
    pub fn assign_profile(&mut self, profile_name: &str) {
        if let Some(id) = self.active_pad {
            if let Some(info) = self.pads.iter_mut().find(|p| p.id == id) {
                info.profile = profile_name.to_string();
                self.settings
                    .pad_profiles
                    .insert(info.uuid.clone(), profile_name.to_string());
            }
        } else {
            self.settings.default_profile = profile_name.to_string();
        }
    }

    fn refresh_pads(&mut self) {
        let Some(ref gilrs) = self.gilrs else {
            self.pads.clear();
            return;
        };

        let now = Instant::now();
        let mut pads = Vec::new();
        for (id, gamepad) in gilrs.gamepads() {
            if !gamepad.is_connected() {
                continue;
            }
            let uuid = uuid_hex(&gamepad.uuid());
            let name = gamepad.name().to_string();
            let profile = self.settings.profile_for(&uuid, &name);
            let recently_active = self
                .last_input_at
                .get(&uuid)
                .map(|t| now.duration_since(*t) < Duration::from_secs(3))
                .unwrap_or(false);
            pads.push(PadInfo {
                id,
                name,
                uuid,
                profile,
                power: power_label(gamepad.power_info()),
                recently_active,
            });
        }
        self.pads = pads;
    }

    /// Picks an active pad when there is none, preferring the remembered one.
    fn ensure_active_pad(&mut self) {
        if let Some(id) = self.active_pad {
            if self.pads.iter().any(|p| p.id == id) {
                return;
            }
            self.active_pad = None;
        }
        // Remembered pad wins if it is plugged in.
        if let Some(ref want) = self.settings.preferred_pad {
            if let Some(p) = self.pads.iter().find(|p| &p.uuid == want) {
                self.active_pad = Some(p.id);
                return;
            }
        }
        self.active_pad = self.pads.first().map(|p| p.id);
    }

    /// Drains events, resolves bindings and returns this frame's state.
    pub fn poll(&mut self) -> PadState {
        let prev = self.state.clone();
        let mut state = PadState::default();

        if self.gilrs.is_none() {
            self.state = state.clone();
            return state;
        }

        let mut topology_changed = false;
        let mut messages: Vec<String> = Vec::new();

        // --- Drain the event queue ------------------------------------------
        // Held-button tracking and remap capture both live here, because
        // is_pressed() alone cannot distinguish "still held" from "just hit",
        // which is exactly what chord detection needs.
        let mut events = Vec::new();
        if let Some(ref mut gilrs) = self.gilrs {
            while let Some(Event { id, event, .. }) = gilrs.next_event() {
                events.push((id, event));
            }
        }

        for (id, event) in events {
            match event {
                EventType::Connected => {
                    topology_changed = true;
                    if let Some(ref gilrs) = self.gilrs {
                        let g = gilrs.gamepad(id);
                        messages.push(format!("🎮 {} connected", g.name()));
                    }
                }
                EventType::Disconnected => {
                    topology_changed = true;
                    messages.push("🎮 Controller disconnected".to_string());
                }
                EventType::ButtonPressed(btn, _) => {
                    self.note_pad_activity(id);
                    self.held_buttons.insert(btn);
                    self.on_capture_button(btn);
                }
                EventType::ButtonReleased(btn, _) => {
                    self.held_buttons.remove(&btn);
                }
                EventType::AxisChanged(axis, value, _) => {
                    if value.abs() > 0.5 {
                        self.note_pad_activity(id);
                        self.on_capture_axis(axis, value);
                    }
                }
                _ => {}
            }
        }

        self.refresh_pads();
        if topology_changed {
            let had = self.active_pad;
            self.ensure_active_pad();
            if had != self.active_pad {
                if let Some(name) = self.active_pad_name() {
                    messages.push(format!("🎮 Active controller: {}", name));
                }
            }
        }
        if let Some(msg) = messages.into_iter().next_back() {
            self.hotplug_message = Some(msg);
        }

        // Chord candidates go stale so a slow double-tap is not read as a chord.
        if let Some((_, at)) = self.chord_candidate {
            if at.elapsed() > CHORD_WINDOW {
                self.chord_candidate = None;
            }
        }

        // While the remap dialog is listening, nothing reaches the game: the
        // player is pressing buttons *at the dialog*, not at the emulator.
        if self.is_capturing() {
            self.expire_capture();
            self.state = state.clone();
            return state;
        }

        // --- Resolve bindings ------------------------------------------------
        let pad_ids: Vec<gilrs::GamepadId> = if self.settings.merge_all_pads {
            self.pads.iter().map(|p| p.id).collect()
        } else {
            self.active_pad.into_iter().collect()
        };

        for id in pad_ids {
            let profile_name = self
                .pads
                .iter()
                .find(|p| p.id == id)
                .map(|p| p.profile.clone())
                .unwrap_or_else(|| self.settings.default_profile.clone());
            let Some(profile) = self.settings.profile(&profile_name).cloned() else {
                continue;
            };
            self.accumulate_pad(id, &profile, &mut state);
        }

        // --- Derive press edges ---------------------------------------------
        for (i, held) in state.held.iter().enumerate() {
            state.just[i] = *held && !prev.held[i];
        }

        // Held page turns auto-repeat, so a player can flip through the guide
        // without lifting a thumb -- but only after a delay, so a single press
        // never turns two pages.
        self.apply_page_repeat(&mut state);

        // Game buttons mirror the first ten actions, except while the guide
        // has capture.
        if !(self.guide_open && self.settings.guide_captures_pad) {
            for (i, key) in state.game_keys.iter_mut().enumerate() {
                *key = state.held[i];
            }
        }

        self.state = state.clone();
        state
    }

    fn note_pad_activity(&mut self, id: gilrs::GamepadId) {
        if let Some(info) = self.pads.iter().find(|p| p.id == id) {
            let uuid = info.uuid.clone();
            self.last_input_at.insert(uuid, Instant::now());
        }
    }

    /// Evaluates one pad's bindings into `state`, OR-ing into what is there.
    ///
    /// The actual resolution lives in the free function `resolve_bindings` so
    /// it can be tested without hardware; this wrapper only supplies the gilrs
    /// accessors and merges the result (merging matters when `merge_all_pads`
    /// is on and several pads contribute to one frame).
    fn accumulate_pad(&self, id: gilrs::GamepadId, profile: &PadProfile, state: &mut PadState) {
        let Some(ref gilrs) = self.gilrs else { return };
        let gamepad = gilrs.gamepad(id);
        if !gamepad.is_connected() {
            return;
        }

        let (held, scroll) = resolve_bindings(
            profile,
            &|b: Button| gamepad.is_pressed(b),
            &|a: Axis| gamepad.axis_data(a).map(|d| d.value()).unwrap_or(0.0),
        );

        for (slot, on) in state.held.iter_mut().zip(held.iter()) {
            *slot |= *on;
        }
        if scroll.abs() > state.guide_scroll_axis.abs() {
            state.guide_scroll_axis = scroll;
        }
    }

    /// Turns a held page-turn binding into repeated press edges.
    fn apply_page_repeat(&mut self, state: &mut PadState) {
        let now = Instant::now();
        for (slot, action) in [(0usize, PadAction::GuidePrevPage), (1, PadAction::GuideNextPage)] {
            let idx = action.index();
            if !state.held[idx] {
                self.page_held_since[slot] = None;
                self.page_last_fire[slot] = None;
                continue;
            }
            let since = *self.page_held_since[slot].get_or_insert(now);
            if now.duration_since(since) < PAGE_REPEAT_DELAY {
                continue;
            }
            let ready = self.page_last_fire[slot]
                .map(|t| now.duration_since(t) >= PAGE_REPEAT_RATE)
                .unwrap_or(true);
            if ready {
                state.just[idx] = true;
                self.page_last_fire[slot] = Some(now);
            }
        }
    }

    // --- Remap capture ---------------------------------------------------

    pub fn is_capturing(&self) -> bool {
        matches!(self.capture, CaptureState::Listening(_))
    }

    pub fn begin_capture(&mut self, action: PadAction) {
        self.capture = CaptureState::Listening(action);
        self.captured_input = None;
        self.capture_started = Some(Instant::now());
        self.chord_candidate = None;
    }

    pub fn cancel_capture(&mut self) {
        self.capture = CaptureState::Idle;
        self.captured_input = None;
        self.capture_started = None;
        self.chord_candidate = None;
    }

    /// Seconds left before the listener gives up, for the countdown in the UI.
    pub fn capture_seconds_left(&self) -> f32 {
        const LIMIT: f32 = 8.0;
        self.capture_started
            .map(|t| (LIMIT - t.elapsed().as_secs_f32()).max(0.0))
            .unwrap_or(0.0)
    }

    fn expire_capture(&mut self) {
        if self.capture_seconds_left() <= 0.0 {
            self.cancel_capture();
        }
    }

    fn on_capture_button(&mut self, btn: Button) {
        if !self.is_capturing() || btn == Button::Unknown {
            return;
        }
        // A second button pressed while the first is still down means the user
        // wants a chord (Select+Start), not two separate rebinds.
        if let Some((first, at)) = self.chord_candidate {
            if first != btn && at.elapsed() <= CHORD_WINDOW && self.held_buttons.contains(&first) {
                self.captured_input = Some(PadInput::chord(first, btn));
                self.chord_candidate = None;
                return;
            }
        }
        self.chord_candidate = Some((btn, Instant::now()));
        self.captured_input = Some(PadInput::Button(btn));
    }

    fn on_capture_axis(&mut self, axis: Axis, value: f32) {
        if !self.is_capturing() || axis == Axis::Unknown {
            return;
        }
        let dir = if value > 0.0 { AxisDir::Positive } else { AxisDir::Negative };
        self.captured_input = Some(PadInput::Axis(axis, dir));
        self.chord_candidate = None;
    }

    /// Consumes a captured input once the user stops adding to it (i.e. all
    /// buttons released), returning the action it belongs to.
    pub fn take_capture(&mut self) -> Option<(PadAction, PadInput)> {
        let CaptureState::Listening(action) = self.capture else {
            return None;
        };
        let input = self.captured_input?;
        // Wait for release so a chord has a chance to form before we commit.
        if !self.held_buttons.is_empty() {
            return None;
        }
        self.cancel_capture();
        Some((action, input))
    }
}

impl GamepadManager {
    /// Raw left/right stick deflection on the active pad, for the live stick
    /// readout in the config dialog. Showing the untouched value is the whole
    /// point -- it is how a user picks a deadzone that actually clears their
    /// stick's centre noise instead of guessing.
    pub fn active_stick_raw(&self) -> Option<(f32, f32, f32, f32)> {
        let gilrs = self.gilrs.as_ref()?;
        let id = self.active_pad?;
        let g = gilrs.gamepad(id);
        if !g.is_connected() {
            return None;
        }
        let v = |a: Axis| g.axis_data(a).map(|d| d.value()).unwrap_or(0.0);
        Some((
            v(Axis::LeftStickX),
            v(Axis::LeftStickY),
            v(Axis::RightStickX),
            v(Axis::RightStickY),
        ))
    }

    /// True when the gamepad subsystem could not start at all (no udev access,
    /// no driver). Distinct from "started fine but nothing is plugged in".
    pub fn subsystem_available(&self) -> bool {
        self.gilrs.is_some()
    }
}

fn uuid_hex(uuid: &[u8; 16]) -> String {
    uuid.iter().map(|b| format!("{:02x}", b)).collect()
}

fn power_label(info: PowerInfo) -> String {
    match info {
        PowerInfo::Unknown => "—".to_string(),
        PowerInfo::Wired => "Wired".to_string(),
        PowerInfo::Discharging(p) => format!("Battery {}%", p),
        PowerInfo::Charging(p) => format!("Charging {}%", p),
        PowerInfo::Charged => "Charged".to_string(),
    }
}

// ---------------------------------------------------------------------------
// Frontend entry point
// ---------------------------------------------------------------------------

/// Polls the pad, merges with the keyboard and pushes the result at the core.
///
/// Returns this frame's pad state so the caller can act on hotkeys without
/// polling gilrs a second time (double-polling would eat the event queue and
/// silently break press-edge detection).
pub fn handle_input(
    ctx: &egui::Context,
    bindings: &KeyBindings,
    gamepad: &mut GamepadManager,
    set_key: &mut impl FnMut(Key, bool),
) -> PadState {
    let pad = gamepad.poll();
    let gp = pad.game_keys;

    ctx.input(|i| {
        set_key(Key::A, i.key_down(bindings.a) || gp[0]);
        set_key(Key::B, i.key_down(bindings.b) || gp[1]);
        set_key(Key::Select, i.key_down(bindings.select) || gp[2]);
        set_key(Key::Start, i.key_down(bindings.start) || gp[3]);
        set_key(Key::Right, i.key_down(bindings.right) || gp[4]);
        set_key(Key::Left, i.key_down(bindings.left) || gp[5]);
        set_key(Key::Up, i.key_down(bindings.up) || gp[6]);
        set_key(Key::Down, i.key_down(bindings.down) || gp[7]);
        set_key(Key::R, i.key_down(bindings.r) || gp[8]);
        set_key(Key::L, i.key_down(bindings.l) || gp[9]);
    });

    pad
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_chord_masks_its_member_buttons_from_the_game() {
        // Stock Ultimate 2 profile: Select+Start is fullscreen, and Select /
        // Start are also GBA buttons. Holding the chord must fire fullscreen
        // and NOT push Select/Start into the game.
        let p = PadProfile::ultimate2_pro();
        let down = [Button::Select, Button::Start];
        let (held, _) = resolve_bindings(
            &p,
            &|b| down.contains(&b),
            &|_| 0.0,
        );

        assert!(held[PadAction::Fullscreen.index()], "chord did not fire");
        assert!(
            !held[PadAction::Select.index()],
            "Select leaked into the game while the chord was held"
        );
        assert!(
            !held[PadAction::Start.index()],
            "Start leaked into the game while the chord was held"
        );
    }

    #[test]
    fn one_handed_layouts_have_no_duplicate_or_hotkey_clashes() {
        // Keys the app handles without Ctrl regardless of bindings.
        let reserved = [
            EKey::Num0, EKey::Num1, EKey::Num2, EKey::Num3, EKey::Num4,
            EKey::Num5, EKey::Num6, EKey::Num7, EKey::Num8, EKey::Num9,
            EKey::F1, EKey::F3, EKey::F6, EKey::F7, EKey::N, EKey::Period,
        ];
        for kb in [KeyBindings::left_handed_one_hand(), KeyBindings::right_handed_one_hand()] {
            let game = [kb.a, kb.b, kb.select, kb.start, kb.right, kb.left, kb.up, kb.down, kb.r, kb.l, kb.turbo, kb.rewind];
            let all: Vec<EKey> = game.iter().copied().chain([kb.pause, kb.frame_step, kb.reset]).collect();
            for (i, k) in all.iter().enumerate() {
                assert!(!all[i + 1..].contains(k), "{k:?} bound twice");
                assert!(!reserved.contains(k), "{k:?} clashes with a hotkey");
            }
        }
        // The left-hand layout only uses the left half of the keyboard.
        let left = KeyBindings::left_handed_one_hand();
        for (_, name) in left.game_button_summary() {
            assert!(["W", "A", "S", "D", "E", "Q", "Tab", "R", "Z", "X", "Space", "C"].contains(&name), "{name}");
        }
    }

    #[test]
    fn a_lone_chord_member_still_works_as_a_normal_button() {
        let p = PadProfile::ultimate2_pro();
        let (held, _) = resolve_bindings(&p, &|b| b == Button::Select, &|_| 0.0);
        assert!(held[PadAction::Select.index()], "Select alone should work");
        assert!(!held[PadAction::Fullscreen.index()], "half a chord fired");
    }

    #[test]
    fn the_guide_toggle_chord_does_not_leak_its_members_either() {
        let p = PadProfile::ultimate2_pro();
        let chord = p
            .binds(PadAction::GuideToggle)
            .iter()
            .find_map(|b| match b {
                PadInput::Chord(x, y) => Some((*x, *y)),
                _ => None,
            })
            .expect("stock guide toggle should be a chord");

        let (held, _) = resolve_bindings(&p, &|b| b == chord.0 || b == chord.1, &|_| 0.0);
        assert!(held[PadAction::GuideToggle.index()]);
        // Whichever game actions those buttons also serve must stay quiet.
        for action in PadAction::ALL.into_iter().filter(|a| a.is_game_button()) {
            let bound_to_member = p.binds(action).iter().any(|i| {
                matches!(i, PadInput::Button(b) if *b == chord.0 || *b == chord.1)
            });
            if bound_to_member {
                assert!(
                    !held[action.index()],
                    "{:?} leaked while the guide chord was held",
                    action
                );
            }
        }
    }

    #[test]
    fn stick_input_respects_the_profile_deadzone() {
        let mut p = PadProfile::ultimate2_pro();
        p.deadzone = 0.30;

        // Inside the deadzone: ignored.
        let (held, _) = resolve_bindings(
            &p,
            &|_| false,
            &|a| if a == Axis::LeftStickX { 0.25 } else { 0.0 },
        );
        assert!(!held[PadAction::Right.index()], "drift inside deadzone moved the player");

        // Outside: registers.
        let (held, _) = resolve_bindings(
            &p,
            &|_| false,
            &|a| if a == Axis::LeftStickX { 0.55 } else { 0.0 },
        );
        assert!(held[PadAction::Right.index()], "stick past deadzone did nothing");
        assert!(!held[PadAction::Left.index()], "opposite direction also fired");
    }

    #[test]
    fn guide_scroll_is_proportional_and_deadzone_compensated() {
        let mut p = PadProfile::ultimate2_pro();
        p.deadzone = 0.20;

        // Just past the deadzone => near zero scroll, not an abrupt jump to
        // full speed. This is what makes a stick usable for reading.
        let (_, scroll) = resolve_bindings(
            &p,
            &|_| false,
            &|a| if a == Axis::LeftStickY { 0.22 } else { 0.0 },
        );
        assert!(scroll > 0.0 && scroll < 0.1, "expected a gentle ramp, got {}", scroll);

        // Full deflection => full speed.
        let (_, scroll) = resolve_bindings(
            &p,
            &|_| false,
            &|a| if a == Axis::LeftStickY { 1.0 } else { 0.0 },
        );
        assert!((scroll - 1.0).abs() < 1e-5, "expected 1.0, got {}", scroll);

        // Down is negative.
        let (_, scroll) = resolve_bindings(
            &p,
            &|_| false,
            &|a| if a == Axis::LeftStickY { -1.0 } else { 0.0 },
        );
        assert!((scroll + 1.0).abs() < 1e-5, "expected -1.0, got {}", scroll);
    }

    #[test]
    fn a_digital_dpad_press_scrolls_the_guide_at_full_speed() {
        let p = PadProfile::ultimate2_pro();
        let (_, scroll) = resolve_bindings(&p, &|b| b == Button::DPadDown, &|_| 0.0);
        assert!((scroll + 1.0).abs() < 1e-5, "d-pad scroll: {}", scroll);
    }

    #[test]
    fn a_remapped_binding_actually_changes_what_the_hardware_does() {
        let mut p = PadProfile::ultimate2_pro();
        // Stock: A is East. Remap it onto the left bumper.
        p.rebind_single(PadAction::A, PadInput::Button(Button::LeftTrigger));

        let (held, _) = resolve_bindings(&p, &|b| b == Button::East, &|_| 0.0);
        assert!(!held[PadAction::A.index()], "old binding still fires A");

        let (held, _) = resolve_bindings(&p, &|b| b == Button::LeftTrigger, &|_| 0.0);
        assert!(held[PadAction::A.index()], "new binding does not fire A");
    }

    #[test]
    fn both_stock_face_buttons_map_to_the_same_gba_button() {
        // A long-standing comfort feature: A works from either right-hand
        // face button, so a player's thumb position does not matter.
        let p = PadProfile::ultimate2_pro();
        for b in [Button::East, Button::North] {
            let (held, _) = resolve_bindings(&p, &|x| x == b, &|_| 0.0);
            assert!(held[PadAction::A.index()], "{:?} should press A", b);
        }
        for b in [Button::South, Button::West] {
            let (held, _) = resolve_bindings(&p, &|x| x == b, &|_| 0.0);
            assert!(held[PadAction::B.index()], "{:?} should press B", b);
        }
    }

    #[test]
    fn the_xbox_profile_swaps_the_face_buttons() {
        let p = PadProfile::xbox_generic();
        let (held, _) = resolve_bindings(&p, &|x| x == Button::South, &|_| 0.0);
        assert!(held[PadAction::A.index()], "Xbox layout: bottom button is A");
        let (held, _) = resolve_bindings(&p, &|x| x == Button::East, &|_| 0.0);
        assert!(held[PadAction::B.index()], "Xbox layout: right button is B");
    }

    #[test]
    fn nothing_held_produces_no_input() {
        let p = PadProfile::ultimate2_pro();
        let (held, scroll) = resolve_bindings(&p, &|_| false, &|_| 0.0);
        assert!(held.iter().all(|h| !*h), "phantom input with nothing pressed");
        assert_eq!(scroll, 0.0);
    }

    #[test]
    fn pad_input_round_trips_through_its_wire_format() {
        let cases = [
            PadInput::Button(Button::South),
            PadInput::Button(Button::DPadLeft),
            PadInput::Axis(Axis::LeftStickY, AxisDir::Positive),
            PadInput::Axis(Axis::RightStickX, AxisDir::Negative),
            PadInput::chord(Button::Select, Button::Start),
        ];
        for c in cases {
            let s = c.to_string();
            let back: PadInput = s.parse().unwrap_or_else(|e| panic!("{}: {}", s, e));
            assert_eq!(c, back, "round trip failed for {}", s);
        }
    }

    #[test]
    fn chord_member_order_is_normalized() {
        assert_eq!(
            PadInput::chord(Button::Start, Button::Select),
            PadInput::chord(Button::Select, Button::Start)
        );
    }

    #[test]
    fn action_indices_match_gba_keypad_order() {
        assert_eq!(PadAction::A.index(), 0);
        assert_eq!(PadAction::L.index(), 9);
        assert!(PadAction::L.is_game_button());
        assert!(!PadAction::Turbo.is_game_button());
    }

    #[test]
    fn ultimate2_is_matched_across_its_connection_modes() {
        let want = PadProfile::ultimate2_pro().name;
        for n in [
            "8BitDo Ultimate 2 Wireless Controller",
            "8BitDo Ultimate 2C Wireless",
            "8BitDo Ultimate Wireless Controller",
        ] {
            assert_eq!(guess_profile_name(n).as_deref(), Some(want.as_str()), "{}", n);
        }
        assert_eq!(
            guess_profile_name("Microsoft X-Box 360 pad").as_deref(),
            Some(PadProfile::xbox_generic().name.as_str())
        );
        assert_eq!(guess_profile_name("Some Random Pad"), None);
    }

    #[test]
    fn hall_effect_profile_uses_a_tighter_deadzone_than_generic() {
        assert!(PadProfile::ultimate2_pro().deadzone < PadProfile::nintendo_generic().deadzone);
    }

    #[test]
    fn conflicts_reports_other_actions_sharing_an_input() {
        let mut p = PadProfile::ultimate2_pro();
        p.rebind_single(PadAction::Turbo, PadInput::Button(Button::South));
        let c = p.conflicts(PadInput::Button(Button::South), PadAction::Turbo);
        assert!(c.contains(&PadAction::B), "expected B conflict, got {:?}", c);
    }

    #[test]
    fn settings_reseed_restores_missing_builtins() {
        let mut s = ControllerSettings {
            profiles: vec![],
            default_profile: "gone".to_string(),
            ..Default::default()
        };
        s.reseed_builtins();
        assert_eq!(s.profiles.len(), PadProfile::builtins().len());
        assert!(s.profile(&s.default_profile).is_some());
    }

    #[test]
    fn keybindings_survive_a_json_round_trip() {
        let k = KeyBindings::default();
        let json = serde_json::to_string(&k).unwrap();
        let back: KeyBindings = serde_json::from_str(&json).unwrap();
        assert_eq!(back.a, k.a);
        assert_eq!(back.quick_save, k.quick_save);
        assert_eq!(back.fullscreen, k.fullscreen);
    }

    #[test]
    fn profile_assignment_beats_the_name_heuristic() {
        let mut s = ControllerSettings::default();
        s.pad_profiles
            .insert("abc".to_string(), PadProfile::xbox_generic().name);
        assert_eq!(
            s.profile_for("abc", "8BitDo Ultimate 2 Wireless"),
            PadProfile::xbox_generic().name
        );
        assert_eq!(
            s.profile_for("other", "8BitDo Ultimate 2 Wireless"),
            PadProfile::ultimate2_pro().name
        );
    }
}
