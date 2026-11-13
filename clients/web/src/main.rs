mod app;
mod components;
mod events;
mod icon;
mod incoming;
mod leave_guard;
mod load;
mod local_files;
mod locale;
mod pages;
mod route;
mod save_file;
mod session;
mod upload;
mod uploads;

fn main() {
    console_error_panic_hook::set_once();
    leptos::mount::mount_to_body(app::App);
}
