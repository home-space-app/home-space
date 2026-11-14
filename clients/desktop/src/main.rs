#![cfg_attr(all(windows, not(debug_assertions)), windows_subsystem = "windows")]

mod app;
mod browser;
mod device;
mod ellipsis;
mod events;
mod icons;
mod locale;
mod login;
mod session;
mod sso;
mod sync;
mod theme;
mod transfers;
mod viewers;

const APP_TITLE: &str = "Home Space";
/// Window class (X11) and app id (Wayland) that desktop environments match against the `.desktop`
/// file. Inside Flatpak the desktop file is named after the Flatpak app id, so that wins.
const APP_ID: &str = "home-space";
const FLATPAK_ID_VAR: &str = "FLATPAK_ID";
const WINDOW: iced::Size = iced::Size::new(1280.0, 800.0);
const MIN_WINDOW: iced::Size = iced::Size::new(480.0, 360.0);
const APP_ICON: &[u8] = include_bytes!("../packaging/home-space.png");
#[cfg(target_os = "macos")]
const APP_ICNS: &[u8] = include_bytes!("../packaging/home-space.icns");

fn main() -> iced::Result {
    #[cfg(target_os = "macos")]
    set_dock_icon();
    let app_id = std::env::var(FLATPAK_ID_VAR).unwrap_or_else(|_| APP_ID.to_owned());
    iced::application(app::App::new, app::App::update, app::App::view)
        .title(APP_TITLE)
        .subscription(app::App::subscription)
        .theme(app::App::theme)
        .settings(iced::Settings {
            id: Some(app_id.clone()),
            default_font: theme::FONT,
            default_text_size: iced::Pixels(theme::TEXT_SIZE),
            ..iced::Settings::default()
        })
        .window(iced::window::Settings {
            size: WINDOW,
            icon: window_icon(),
            min_size: Some(MIN_WINDOW),
            platform_specific: platform_specific(app_id),
            ..iced::window::Settings::default()
        })
        .run()
}

/// iced reads the Linux window class and Wayland app id from here, not from `Settings::id`.
#[cfg(target_os = "linux")]
fn platform_specific(app_id: String) -> iced::window::settings::PlatformSpecific {
    iced::window::settings::PlatformSpecific {
        application_id: app_id,
        ..iced::window::settings::PlatformSpecific::default()
    }
}

#[cfg(not(target_os = "linux"))]
fn platform_specific(_app_id: String) -> iced::window::settings::PlatformSpecific {
    iced::window::settings::PlatformSpecific::default()
}

/// Taskbar/title-bar icon for Windows and Linux. macOS ignores it and takes
/// the Dock icon from the `.app` bundle (see `[package.metadata.bundle]`) or
/// from [`set_dock_icon`].
fn window_icon() -> Option<iced::window::Icon> {
    let mut reader = png::Decoder::new(std::io::Cursor::new(APP_ICON))
        .read_info()
        .ok()?;
    let mut rgba = vec![0; reader.output_buffer_size()?];
    let info = reader.next_frame(&mut rgba).ok()?;
    rgba.truncate(info.buffer_size());
    iced::window::icon::from_rgba(rgba, info.width, info.height).ok()
}

/// Dock icon for the unbundled binary (`cargo run`), which macOS would
/// otherwise show as a generic "exec" icon. The icon is set once launching is
/// done, when the app gets its Dock tile. A `.app` bundle's icon wins anyway.
#[cfg(target_os = "macos")]
fn set_dock_icon() {
    use std::ptr::NonNull;

    use block2::RcBlock;
    use objc2::{AllocAnyThread, MainThreadMarker};
    use objc2_app_kit::{
        NSApplication, NSApplicationActivationPolicy, NSApplicationDidFinishLaunchingNotification,
        NSImage,
    };
    use objc2_foundation::{NSData, NSNotification, NSNotificationCenter};

    let block = RcBlock::new(|_: NonNull<NSNotification>| {
        let Some(mtm) = MainThreadMarker::new() else {
            return;
        };
        let app = NSApplication::sharedApplication(mtm);
        // The Dock tile only exists once the app is a regular app; winit makes
        // it one in this same notification, possibly after this observer runs.
        app.setActivationPolicy(NSApplicationActivationPolicy::Regular);
        let data = NSData::with_bytes(APP_ICNS);
        if let Some(image) = NSImage::initWithData(NSImage::alloc(), &data) {
            unsafe { app.setApplicationIconImage(Some(&image)) };
        }
    });
    // The observer lives for the whole process, so its token is leaked.
    std::mem::forget(unsafe {
        NSNotificationCenter::defaultCenter().addObserverForName_object_queue_usingBlock(
            Some(NSApplicationDidFinishLaunchingNotification),
            None,
            None,
            &block,
        )
    });
}

#[cfg(test)]
mod tests {
    #[test]
    fn window_icon_decodes() {
        assert!(super::window_icon().is_some());
    }
}
