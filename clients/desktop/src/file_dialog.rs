//! The system's file dialogs, opened over the app's window. A dialog without a parent window can
//! open behind the app on some desktops, where it cannot be reached until the app is moved.

use iced::{Task, window};
use rfd::AsyncFileDialog;

/// A file dialog over the app's window, set up by `build`; one without a parent when the window
/// cannot be named, such as before it opens.
pub fn over_window(
    build: impl FnOnce(AsyncFileDialog) -> AsyncFileDialog + Send + 'static,
) -> Task<AsyncFileDialog> {
    let mut build = Some(build);
    window::latest().then(move |id| match (id, build.take()) {
        (Some(id), Some(build)) => window::run(id, move |window| {
            build(AsyncFileDialog::new().set_parent(window))
        }),
        (None, Some(build)) => Task::done(build(AsyncFileDialog::new())),
        // `then` calls this once; the dialog is set up then.
        (_, None) => Task::none(),
    })
}
