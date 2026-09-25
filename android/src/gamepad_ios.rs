//! iOS has no controller hook yet: touch (and tilt) only.

use crate::touch::Buttons;

pub fn install() {}

pub fn buttons() -> Buttons {
    Buttons(0)
}

pub fn take_menu_request() -> bool {
    false
}

pub fn take_back_request() -> bool {
    false
}

pub fn recently_used() -> bool {
    false
}
