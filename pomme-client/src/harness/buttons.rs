//! The buttons the last frame drew, by label, so `cursor_to_button` can point
//! at one the way a player would.

use std::cell::RefCell;

#[derive(Default)]
struct Drawn {
    drawing: Vec<(String, [f32; 4])>,
    last_frame: Vec<(String, [f32; 4])>,
}

thread_local! {
    static DRAWN: RefCell<Drawn> = RefCell::default();
}

/// Records a button as it's drawn: its label and framebuffer rect.
pub(crate) fn note(label: &str, rect: [f32; 4]) {
    DRAWN.with(|drawn| drawn.borrow_mut().drawing.push((label.to_owned(), rect)));
}

/// Ends a frame: what it drew becomes what [`find`] sees.
pub(super) fn end_frame() {
    DRAWN.with(|drawn| {
        let mut drawn = drawn.borrow_mut();
        drawn.last_frame = std::mem::take(&mut drawn.drawing);
    });
}

/// The last frame's button labelled `label`.
pub(super) fn find(label: &str) -> Option<[f32; 4]> {
    DRAWN.with(|drawn| {
        drawn
            .borrow()
            .last_frame
            .iter()
            .find(|(drawn_label, _)| drawn_label == label)
            .map(|(_, rect)| *rect)
    })
}
