//! Behavioural checks for guide navigation and chord masking.
//!
//! These cover the two pieces of controller logic that are easy to get subtly
//! wrong and impossible to notice in a unit test of the wire format:
//!
//! 1. A chord's member buttons must not also reach the game.
//! 2. Analog guide scrolling must be direction-correct and frame-rate
//!    independent, or the guide crawls on a 144Hz display and flies on 30Hz.

use gba_simulator::ui::controls::{PadAction, PadInput, PadProfile};
use gba_simulator::ui::guide_dialog::{GuideDialog, GuidePadInput};

#[test]
fn the_stock_guide_toggle_is_a_chord_so_it_cannot_be_hit_by_accident() {
    let p = PadProfile::ultimate2_pro();
    let binds = p.binds(PadAction::GuideToggle);
    assert!(!binds.is_empty(), "guide toggle must ship bound");
    assert!(
        binds.iter().any(|b| b.is_chord()),
        "at least one guide-toggle binding should be a two-button combo, got {:?}",
        binds
    );
}

#[test]
fn guide_toggle_chord_members_are_also_game_buttons_which_is_why_masking_matters() {
    // This asserts the *precondition* for the masking logic: the stock combo
    // deliberately reuses buttons the game also uses, so if masking regressed
    // the player would get a stray Select/R in-game every time they opened the
    // guide.
    let p = PadProfile::ultimate2_pro();
    let chord = p
        .binds(PadAction::GuideToggle)
        .iter()
        .find(|b| b.is_chord())
        .copied()
        .expect("a chord binding");

    let PadInput::Chord(a, b) = chord else {
        unreachable!()
    };

    let game_bound = |btn| {
        PadAction::ALL
            .into_iter()
            .filter(|act| act.is_game_button())
            .any(|act| p.binds(act).contains(&PadInput::Button(btn)))
    };
    assert!(
        game_bound(a) || game_bound(b),
        "expected the guide chord to overlap game buttons"
    );
}

#[test]
fn every_action_ships_bound_except_the_ones_intentionally_left_free() {
    let p = PadProfile::ultimate2_pro();
    for action in PadAction::ALL {
        let bound = !p.binds(action).is_empty();
        if action == PadAction::Screenshot {
            // Left unbound on purpose: there is no spare button on an Ultimate
            // 2 that a player would not hit mid-game, and F12 covers it.
            assert!(!bound, "Screenshot should ship unbound on the pad");
        } else {
            assert!(bound, "{:?} ships with no binding", action);
        }
    }
}

#[test]
fn guide_page_turns_respect_the_page_bounds() {
    let mut g = GuideDialog::new();
    g.is_open = true;

    let next = GuidePadInput { next_page: true, ..Default::default() };
    let prev = GuidePadInput { prev_page: true, ..Default::default() };

    // Cannot page before the cover.
    g.apply_pad_input(prev, 0.016);
    assert_eq!(g.current_page, 0);

    for expected in 1..=7 {
        g.apply_pad_input(next, 0.016);
        assert_eq!(g.current_page, expected);
    }
    // Cannot page past the last page.
    g.apply_pad_input(next, 0.016);
    assert_eq!(g.current_page, 7);

    g.apply_pad_input(prev, 0.016);
    assert_eq!(g.current_page, 6);
}

#[test]
fn guide_scroll_is_frame_rate_independent_and_direction_correct() {
    // Full deflection "up" for one simulated second, delivered at 60Hz vs
    // 144Hz, must move the page by the same amount.
    let run = |steps: u32, dt: f32| {
        let mut g = GuideDialog::new();
        g.is_open = true;
        let pad = GuidePadInput {
            scroll: 1.0,
            scroll_speed: 1000.0,
            ..Default::default()
        };
        for _ in 0..steps {
            g.apply_pad_input(pad, dt);
        }
        g.take_pad_scroll_delta_for_test()
    };

    let at_60 = run(60, 1.0 / 60.0);
    let at_144 = run(144, 1.0 / 144.0);

    // Scrolling "up" decreases the scroll offset.
    assert!(at_60 < 0.0, "up-scroll should decrease offset, got {}", at_60);
    assert!(
        (at_60 - at_144).abs() < 1.0,
        "frame rate changed scroll distance: 60Hz={} 144Hz={}",
        at_60,
        at_144
    );
    assert!(
        (at_60.abs() - 1000.0).abs() < 5.0,
        "one second at full deflection should travel ~scroll_speed, got {}",
        at_60.abs()
    );
}

#[test]
fn small_stick_deflections_scroll_far_slower_than_large_ones() {
    // The cubic response curve is what makes careful reading possible; a
    // linear ramp would make a light touch already fast.
    let travel = |deflection: f32| {
        let mut g = GuideDialog::new();
        g.is_open = true;
        g.apply_pad_input(
            GuidePadInput {
                scroll: deflection,
                scroll_speed: 1000.0,
                ..Default::default()
            },
            0.016,
        );
        g.take_pad_scroll_delta_for_test().abs()
    };

    let light = travel(0.25);
    let full = travel(1.0);
    assert!(
        full > light * 20.0,
        "expected a strongly non-linear response: light={} full={}",
        light,
        full
    );
}

#[test]
fn a_closed_guide_ignores_controller_input() {
    let mut g = GuideDialog::new();
    // is_open defaults to false.
    g.apply_pad_input(
        GuidePadInput {
            scroll: 1.0,
            scroll_speed: 1000.0,
            next_page: true,
            ..Default::default()
        },
        0.016,
    );
    assert_eq!(g.current_page, 0, "closed guide should not turn pages");
    assert_eq!(g.take_pad_scroll_delta_for_test(), 0.0);
}
