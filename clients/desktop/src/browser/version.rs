//! How this app fits its server and the latest release: the version chip above the sync
//! status with its details, the red banner, and the "New version available" toast.

use iced::{
    Alignment, Element, Length, Padding, Task,
    widget::{button, column, container, row, space, text},
};

use api_client::{Client, Release};
use client_core::{
    update,
    versions::{self, Tone},
};
use protocol::{Compatibility, Incompatibility};

use super::{
    Message as BrowserMessage,
    parts::{small_outlined, small_primary},
    syncing::open_url,
    toolbar::tip,
};
use crate::{
    device::{REQUIRES, VERSION},
    icons::{ICON_SIZE, Icon, Tint},
    theme,
    update_prefs::{UpdatePrefs, releases_url},
};

const DOT: f32 = 8.0;
const CHIP_SIZE: f32 = 12.0;
const CHIP_PADDING: Padding = Padding {
    top: 3.0,
    right: 10.0,
    bottom: 3.0,
    left: 8.0,
};
const CHIP_GAP: f32 = 6.0;
const DETAILS_PADDING: f32 = 12.0;
const DETAILS_GAP: f32 = 6.0;
const NOTE_SIZE: f32 = 12.0;
const BANNER_PADDING: [f32; 2] = [10.0, 14.0];
const BANNER_GAP: f32 = 10.0;
const TOAST_WIDTH: f32 = 340.0;
const TOAST_PADDING: f32 = 12.0;
const TOAST_GAP: f32 = 8.0;
const SWITCH_WIDTH: f32 = 34.0;
const SWITCH_HEIGHT: f32 = 20.0;
const SWITCH_INSET: f32 = 2.0;
const THUMB: f32 = 16.0;
const DETAILS: &str = "Details";
const DISMISS: &str = "Dismiss";
const CLOSE: &str = "Close";

#[derive(Debug, Clone)]
pub enum Message {
    /// Opens or closes the details above the chip.
    Toggle,
    /// Hides the red banner for this run.
    DismissBanner,
    /// The latest release, or `None` when it could not be read.
    Checked(Option<Release>),
    /// Hides the toast for this run.
    CloseToast,
    /// Hides the toast until a newer release is out.
    Skip,
    OpenRelease,
    /// Turns the check for new versions on or off.
    SetCheck(bool),
}

/// The versions and what the user chose about them.
#[derive(Debug)]
pub struct Versions {
    compatibility: Option<Compatibility>,
    server: Option<String>,
    open: bool,
    banner_dismissed: bool,
    prefs: UpdatePrefs,
    /// The release the toast offers, while it shows.
    offered: Option<Release>,
    /// Why the choices could not be saved.
    error: Option<String>,
}

impl Versions {
    /// The versions of `client`'s server, and the check for the latest release when the
    /// user has not turned it off.
    pub fn new(client: &Client) -> (Self, Task<Message>) {
        let prefs = UpdatePrefs::load();
        let check = if prefs.check {
            let url = releases_url();
            Task::perform(
                async move { api_client::latest_release(&url).await },
                Message::Checked,
            )
        } else {
            Task::none()
        };
        let versions = Self {
            compatibility: client.compatibility(VERSION, REQUIRES),
            server: client.server_version(),
            open: false,
            banner_dismissed: false,
            prefs,
            offered: None,
            error: None,
        };
        (versions, check)
    }

    pub fn update(&mut self, message: Message) {
        match message {
            Message::Toggle => self.open = !self.open,
            Message::DismissBanner => self.banner_dismissed = true,
            Message::Checked(release) => {
                self.offered = release.filter(|release| {
                    update::should_offer(
                        VERSION,
                        &release.version,
                        self.prefs.skipped_version.as_deref(),
                    )
                });
            }
            Message::CloseToast => self.offered = None,
            Message::Skip => {
                if let Some(release) = self.offered.take() {
                    self.prefs.skipped_version = Some(release.version);
                    self.save();
                }
            }
            Message::OpenRelease => {
                if let Some(release) = &self.offered {
                    self.error = open_url(&release.url).err();
                }
            }
            Message::SetCheck(check) => {
                self.prefs.check = check;
                if !check {
                    self.offered = None;
                }
                self.save();
            }
        }
    }

    fn save(&mut self) {
        self.error = self.prefs.save().err();
    }

    /// The chip above the sync status, and its details above it while open. Nothing when the
    /// server's capabilities could not be read.
    pub fn sidebar(&self, compact: bool) -> Option<Element<'_, BrowserMessage>> {
        let compatibility = self.compatibility.as_ref()?;
        let tone = versions::tone(compatibility);
        let label = versions::chip(VERSION, self.server.as_deref());
        let mut content = row![dot(tone)].spacing(CHIP_GAP).align_y(Alignment::Center);
        if !compact {
            content = content.push(text(label.clone()).size(CHIP_SIZE).font(theme::MEDIUM));
        }
        let chip: Element<'_, BrowserMessage> = button(content)
            .padding(CHIP_PADDING)
            .style(theme::version_chip(tone))
            .on_press(wrap(Message::Toggle))
            .into();
        let chip = if compact {
            tip(chip, versions::title(compatibility), "")
        } else {
            chip
        };
        let mut part = column![].spacing(DETAILS_GAP);
        if self.open && !compact {
            part = part.push(self.details(compatibility, tone));
        }
        Some(part.push(chip).into())
    }

    fn details<'a>(
        &'a self,
        compatibility: &'a Compatibility,
        tone: Tone,
    ) -> Element<'a, BrowserMessage> {
        let head = row![
            dot(tone),
            text(versions::title(compatibility))
                .font(theme::SEMIBOLD)
                .style(theme::version_text(tone))
        ]
        .spacing(CHIP_GAP)
        .align_y(Alignment::Center);
        let facts = column![
            fact("This app", VERSION),
            fact(
                "Server",
                self.server.as_deref().unwrap_or(versions::UNKNOWN_VERSION)
            ),
        ]
        .spacing(2.0);
        let hint = text(versions::hint(
            compatibility,
            VERSION,
            self.server.as_deref(),
        ))
        .size(NOTE_SIZE)
        .style(theme::muted_text);
        let check = row![
            text(update::CHECK_SETTING)
                .size(NOTE_SIZE)
                .width(Length::Fill),
            switch(self.prefs.check, wrap(Message::SetCheck(!self.prefs.check))),
        ]
        .spacing(CHIP_GAP)
        .align_y(Alignment::Center);
        let mut body = column![head, facts, hint, check].spacing(DETAILS_GAP);
        if let Some(error) = &self.error {
            body = body.push(text(error).size(NOTE_SIZE).style(theme::danger_text));
        }
        container(body)
            .padding(DETAILS_PADDING)
            .width(Length::Fill)
            .style(theme::panel)
            .into()
    }

    /// The one-line red banner over the page, until dismissed.
    pub fn banner(&self) -> Option<Element<'_, BrowserMessage>> {
        let compatibility = self.compatibility.as_ref()?;
        let Compatibility::Error { reason } = compatibility else {
            return None;
        };
        if self.banner_dismissed {
            return None;
        }
        let detail = match reason {
            Incompatibility::MissingFeatures(missing) => format!(
                "{} won't work until the server is updated.",
                versions::missing_features(missing)
            ),
            Incompatibility::MajorVersion => {
                versions::hint(compatibility, VERSION, self.server.as_deref())
            }
        };
        let line = row![
            Icon::Error.view(ICON_SIZE, Tint::Danger),
            column![
                text(versions::title(compatibility)).font(theme::SEMIBOLD),
                text(detail).size(NOTE_SIZE),
            ]
            .width(Length::Fill),
            button(text(DETAILS).font(theme::MEDIUM))
                .style(theme::tool)
                .on_press(wrap(Message::Toggle)),
            tip(
                button(Icon::Close.view(ICON_SIZE, Tint::Muted))
                    .style(theme::tool)
                    .on_press(wrap(Message::DismissBanner))
                    .into(),
                DISMISS,
                ""
            ),
        ]
        .spacing(BANNER_GAP)
        .align_y(Alignment::Center);
        Some(
            container(line)
                .padding(BANNER_PADDING)
                .width(Length::Fill)
                .style(theme::alert_error)
                .into(),
        )
    }

    /// The "New version available" toast, while a release is offered.
    pub fn toast(&self) -> Option<Element<'_, BrowserMessage>> {
        let release = self.offered.as_ref()?;
        let head = row![
            Icon::Info.view(ICON_SIZE, Tint::Accent),
            text(update::TITLE)
                .font(theme::SEMIBOLD)
                .width(Length::Fill),
            tip(
                button(Icon::Close.view(ICON_SIZE, Tint::Muted))
                    .padding(0)
                    .style(theme::tool)
                    .on_press(wrap(Message::CloseToast))
                    .into(),
                CLOSE,
                ""
            ),
        ]
        .spacing(TOAST_GAP)
        .align_y(Alignment::Center);
        let detail = text(update::app_message(&release.version, VERSION))
            .size(NOTE_SIZE)
            .style(theme::muted_text);
        let buttons = row![
            space::horizontal(),
            small_outlined(update::SKIP, Some(wrap(Message::Skip))),
            small_primary(
                Icon::Web,
                update::OPEN_WEB,
                Some(wrap(Message::OpenRelease))
            ),
        ]
        .spacing(TOAST_GAP)
        .align_y(Alignment::Center);
        let mut body = column![head, detail, buttons].spacing(TOAST_GAP);
        if let Some(error) = &self.error {
            body = body.push(text(error).size(NOTE_SIZE).style(theme::danger_text));
        }
        Some(
            container(body)
                .padding(TOAST_PADDING)
                .width(TOAST_WIDTH)
                .style(theme::panel)
                .into(),
        )
    }
}

fn wrap(message: Message) -> BrowserMessage {
    BrowserMessage::Version(message)
}

fn dot<'a>(tone: Tone) -> Element<'a, BrowserMessage> {
    container(space())
        .width(DOT)
        .height(DOT)
        .style(theme::version_dot(tone))
        .into()
}

fn fact<'a>(label: &'a str, value: &'a str) -> Element<'a, BrowserMessage> {
    row![
        text(label)
            .size(NOTE_SIZE)
            .style(theme::muted_text)
            .width(Length::Fill),
        text(value).size(NOTE_SIZE).font(theme::MEDIUM),
    ]
    .into()
}

/// An on/off switch.
fn switch<'a>(on: bool, message: BrowserMessage) -> Element<'a, BrowserMessage> {
    let thumb = container(space())
        .width(THUMB)
        .height(THUMB)
        .style(theme::switch_thumb);
    let track = if on {
        row![space::horizontal(), thumb]
    } else {
        row![thumb, space::horizontal()]
    };
    button(track.width(Length::Fill))
        .padding(SWITCH_INSET)
        .width(SWITCH_WIDTH)
        .height(SWITCH_HEIGHT)
        .style(theme::switch(on))
        .on_press(message)
        .into()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn versions(compatibility: Option<Compatibility>, skipped: Option<&str>) -> Versions {
        Versions {
            compatibility,
            server: Some(VERSION.to_owned()),
            open: false,
            banner_dismissed: false,
            prefs: UpdatePrefs {
                check: true,
                skipped_version: skipped.map(str::to_owned),
            },
            offered: None,
            error: None,
        }
    }

    fn release(version: &str) -> Release {
        Release {
            version: version.to_owned(),
            url: format!("https://github.com/home-space-app/home-space/releases/tag/v{version}"),
        }
    }

    #[test]
    fn a_newer_release_is_offered() {
        let mut versions = versions(None, None);
        versions.update(Message::Checked(Some(release("999.0.0"))));
        assert!(versions.toast().is_some());
    }

    #[test]
    fn the_installed_release_is_not_offered() {
        let mut versions = versions(None, None);
        versions.update(Message::Checked(Some(release(VERSION))));
        assert!(versions.toast().is_none());
    }

    #[test]
    fn a_skipped_release_is_not_offered() {
        let mut versions = versions(None, Some("999.0.0"));
        versions.update(Message::Checked(Some(release("999.0.0"))));
        assert!(versions.toast().is_none());
    }

    #[test]
    fn a_failed_check_shows_nothing() {
        let mut versions = versions(None, None);
        versions.update(Message::Checked(None));
        assert!(versions.toast().is_none());
    }

    #[test]
    fn closing_the_toast_hides_it() {
        let mut versions = versions(None, None);
        versions.update(Message::Checked(Some(release("999.0.0"))));
        versions.update(Message::CloseToast);
        assert!(versions.toast().is_none());
    }

    #[test]
    fn unknown_capabilities_show_no_chip() {
        assert!(versions(None, None).sidebar(false).is_none());
    }

    #[test]
    fn only_an_error_shows_the_banner_until_dismissed() {
        let missing = Compatibility::Error {
            reason: Incompatibility::MissingFeatures(vec!["device_remove".to_owned()]),
        };
        assert!(versions(Some(Compatibility::Same), None).banner().is_none());
        let mut versions = versions(Some(missing), None);
        assert!(versions.banner().is_some());
        versions.update(Message::DismissBanner);
        assert!(versions.banner().is_none());
    }
}
