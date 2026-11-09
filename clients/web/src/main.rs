mod app;
mod components;
mod events;
mod icon;
mod load;
mod local_files;
mod locale;
mod pages;
mod route;
mod session;
mod upload;
mod uploads;

fn main() {
    console_error_panic_hook::set_once();
    leptos::mount::mount_to_body(app::App);
}
