mod format;
mod inline;
mod node;
mod style;
mod text_view;
mod utils;
mod word_selection;

use gpui::App;
pub use style::*;
pub use text_view::*;

pub(crate) fn init(cx: &mut App) {
    text_view::init(cx);
}
