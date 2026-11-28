//! The card over the folder while its content is encrypted or decrypted: which file, which
//! step, how much is done and about how long is left; or why it stopped, with Finish.

use std::time::{Duration, Instant};

use iced::{
    Alignment, Element, Length,
    widget::{column, progress_bar, row, text},
};

use client_core::storage;
use protocol::NodeId;

use super::{Message, parts};
use crate::{
    icons::{ICON_SIZE, Icon, Tint},
    protected::convert::{Direction, Progress, Step},
    theme,
};

pub(super) const LEAVING_SYNC: &str =
    "Leaving this folder out of the sync on this computer before encrypting it…";
const PROTECT_STOPPED: &str =
    "Encrypting the files of this folder stopped; choose Finish protecting to go on.";
const UNPROTECT_STOPPED: &str =
    "Decrypting the files of this folder stopped; choose Finish unprotecting to go on.";
const OPENS_AFTER: &str = "The folder opens when this is done.";
/// How long the bytes are counted before the time left is guessed from them.
const SETTLING: Duration = Duration::from_secs(3);
const BAR: f32 = 6.0;
const GAP: f32 = 8.0;

/// A folder whose content is being encrypted or decrypted, or whose conversion stopped.
pub(super) struct Conversion {
    pub(super) folder: NodeId,
    pub(super) direction: Direction,
    /// The folder's name, for the title.
    name: Option<String>,
    /// Protecting waits for the sync to stop following the folder, which would otherwise
    /// bring its sealed names to this computer and delete the plain files as they go.
    pub(super) leaving_sync: bool,
    /// Why it stopped, once it has.
    pub(super) stopped: Option<String>,
    planned: Option<Planned>,
    /// The file it works on: its plain name, which of the planned ones, and the step.
    file: Option<(String, usize, Step)>,
    /// The bytes downloaded and uploaded so far.
    moved: u64,
}

/// What the conversion counted before the first file, and when.
struct Planned {
    files: usize,
    bytes: u64,
    at: Instant,
}

impl Conversion {
    pub(super) fn new(
        folder: NodeId,
        direction: Direction,
        name: Option<String>,
        leaving_sync: bool,
    ) -> Self {
        Self {
            folder,
            direction,
            name,
            leaving_sync,
            stopped: None,
            planned: None,
            file: None,
            moved: 0,
        }
    }

    /// Takes in how it goes; `Done` is the caller's.
    pub(super) fn advance(&mut self, progress: Progress) {
        match progress {
            Progress::Planned { files, bytes } => {
                self.planned = Some(Planned {
                    files,
                    bytes,
                    at: Instant::now(),
                });
            }
            Progress::File { name, index, step } => self.file = Some((name, index, step)),
            Progress::Moved(moved) => self.moved = moved,
            Progress::Done(_) => {}
        }
    }

    /// Notes that it stopped, and why.
    pub(super) fn stop(&mut self, why: impl std::fmt::Display) {
        let stopped = match self.direction {
            Direction::Protect => PROTECT_STOPPED,
            Direction::Unprotect => UNPROTECT_STOPPED,
        };
        self.stopped = Some(format!("{stopped} {why}"));
    }

    /// How much is done, between 0 and 1: by the bytes, each counted down and up, or by the
    /// files when they are all empty.
    fn fraction(&self) -> f32 {
        let Some(planned) = &self.planned else {
            return 0.0;
        };
        let done = if planned.bytes > 0 {
            storage::fraction(self.moved, planned.bytes.saturating_mul(2))
        } else {
            let started = self.file.as_ref().map_or(0, |(_, index, _)| index - 1);
            storage::fraction(started as u64, planned.files as u64)
        };
        done as f32
    }

    /// About how long is left, at the rate so far; `None` until it has run a little.
    fn time_left(&self, now: Instant) -> Option<Duration> {
        let planned = self.planned.as_ref()?;
        let elapsed = now.checked_duration_since(planned.at)?;
        if elapsed < SETTLING || self.moved == 0 {
            return None;
        }
        let left = planned.bytes.saturating_mul(2).saturating_sub(self.moved);
        let rate = self.moved as f64 / elapsed.as_secs_f64();
        Some(Duration::from_secs_f64(left as f64 / rate))
    }

    fn title(&self) -> String {
        let doing = match self.direction {
            Direction::Protect => "Encrypting",
            Direction::Unprotect => "Decrypting",
        };
        match &self.name {
            Some(name) => format!("{doing} “{name}”"),
            None => format!("{doing} this folder"),
        }
    }

    /// The line under the bar: what it does now, with the bytes and the time left.
    fn detail(&self, now: Instant) -> String {
        if self.leaving_sync {
            return LEAVING_SYNC.to_owned();
        }
        let Some(planned) = &self.planned else {
            return match self.direction {
                Direction::Protect => "Counting the files to encrypt…",
                Direction::Unprotect => "Counting the files to decrypt…",
            }
            .to_owned();
        };
        let Some((name, _, step)) = &self.file else {
            return match self.direction {
                Direction::Protect => "Sealing the names…",
                Direction::Unprotect => "Opening the names…",
            }
            .to_owned();
        };
        let doing = match (step, self.direction) {
            (Step::Downloading, _) => "Downloading",
            (Step::Converting, Direction::Protect) => "Encrypting",
            (Step::Converting, Direction::Unprotect) => "Decrypting",
            (Step::Uploading, _) => "Uploading",
        };
        let mut parts = vec![format!("{doing} “{name}”")];
        if planned.bytes > 0 {
            let done = (self.moved / 2).min(planned.bytes);
            parts.push(format!(
                "{} of {}",
                storage::bytes(done),
                storage::bytes(planned.bytes)
            ));
        }
        parts.extend(self.time_left(now).map(left));
        parts.join(" · ")
    }

    /// "File 3 of 12 · 42%", once it is counted.
    fn count(&self) -> Option<String> {
        let planned = self.planned.as_ref()?;
        let percent = (self.fraction() * 100.0).floor();
        Some(match &self.file {
            Some((_, index, _)) => format!("File {index} of {} · {percent}%", planned.files),
            None => format!("{percent}%"),
        })
    }

    pub(super) fn view(&self) -> Element<'_, Message> {
        if let Some(stopped) = &self.stopped {
            let label = match self.direction {
                Direction::Protect => "Finish protecting",
                Direction::Unprotect => "Finish unprotecting",
            };
            let finish = parts::small_outlined(label, Some(Message::FinishConversion(self.folder)));
            return parts::alert_error(Icon::Error, stopped.as_str(), Some(finish));
        }
        let icon = match self.direction {
            Direction::Protect => Icon::Lock,
            Direction::Unprotect => Icon::LockOpen,
        };
        let head = row![
            icon.view(ICON_SIZE, Tint::Accent),
            text(self.title()).font(theme::MEDIUM).width(Length::Fill),
        ]
        .push(self.count().map(|count| {
            text(count)
                .size(parts::SECOND_LINE_SIZE)
                .style(theme::muted_text)
        }))
        .spacing(GAP)
        .align_y(Alignment::Center);
        let bar = (self.planned.is_some() && !self.leaving_sync).then(|| {
            progress_bar(0.0..=1.0, self.fraction())
                .girth(BAR)
                .style(theme::progress)
        });
        let detail = text(self.detail(Instant::now()))
            .size(parts::SECOND_LINE_SIZE)
            .style(theme::muted_text);
        let opens = match &self.name {
            Some(name) => format!("“{name}” opens when this is done."),
            None => OPENS_AFTER.to_owned(),
        };
        let opens = text(opens)
            .size(parts::SECOND_LINE_SIZE)
            .style(theme::muted_text);
        parts::notice_info(
            column![head]
                .push(bar)
                .push(detail)
                .push(opens)
                .spacing(GAP),
        )
    }
}

/// "about 3 min left", rounded the way a person would say it.
fn left(time: Duration) -> String {
    let seconds = time.as_secs();
    match seconds {
        0..45 => "less than a minute left".to_owned(),
        45..3600 => format!("about {} min left", seconds.div_ceil(60)),
        _ => {
            let (hours, minutes) = (seconds / 3600, (seconds % 3600) / 60);
            if minutes == 0 {
                format!("about {hours} h left")
            } else {
                format!("about {hours} h {minutes} min left")
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use iced_test::simulator;
    use uuid::Uuid;

    use super::*;

    fn protecting() -> Conversion {
        Conversion::new(
            NodeId(Uuid::from_u128(1)),
            Direction::Protect,
            Some("Folder A".to_owned()),
            false,
        )
    }

    #[test]
    fn it_counts_first_then_names_the_file_the_step_and_the_bytes() {
        let mut conversion = protecting();
        let ui = simulator(conversion.view());
        let mut ui = ui;
        assert!(ui.find("Encrypting “Folder A”").is_ok());
        assert!(ui.find("Counting the files to encrypt…").is_ok());
        drop(ui);

        conversion.advance(Progress::Planned {
            files: 4,
            bytes: 1000,
        });
        conversion.advance(Progress::File {
            name: "tax.pdf".to_owned(),
            index: 2,
            step: Step::Uploading,
        });
        conversion.advance(Progress::Moved(1000));
        assert!((conversion.fraction() - 0.5).abs() < f32::EPSILON);
        let mut ui = simulator(conversion.view());
        assert!(ui.find("File 2 of 4 · 50%").is_ok());
        assert!(ui.find("Uploading “tax.pdf” · 500 B of 1000 B").is_ok());
    }

    #[test]
    fn empty_files_count_by_file() {
        let mut conversion = protecting();
        conversion.advance(Progress::Planned { files: 4, bytes: 0 });
        conversion.advance(Progress::File {
            name: "empty.txt".to_owned(),
            index: 3,
            step: Step::Downloading,
        });
        assert!((conversion.fraction() - 0.5).abs() < f32::EPSILON);
    }

    #[test]
    fn a_stopped_conversion_offers_to_finish() {
        let mut conversion = protecting();
        conversion.stop("The server is unreachable.");
        let mut ui = simulator(conversion.view());
        ui.click("Finish protecting").expect("offered");
        assert!(
            ui.into_messages()
                .any(|message| matches!(message, Message::FinishConversion(_)))
        );
    }

    #[test]
    fn the_time_left_reads_like_speech() {
        assert_eq!(left(Duration::from_secs(20)), "less than a minute left");
        assert_eq!(left(Duration::from_secs(150)), "about 3 min left");
        assert_eq!(left(Duration::from_secs(3600)), "about 1 h left");
        assert_eq!(left(Duration::from_secs(5400)), "about 1 h 30 min left");
    }

    #[test]
    fn the_time_left_waits_for_the_rate_to_settle() {
        let mut conversion = protecting();
        conversion.advance(Progress::Planned {
            files: 1,
            bytes: 1000,
        });
        conversion.advance(Progress::Moved(500));
        let at = conversion.planned.as_ref().expect("planned").at;
        assert_eq!(conversion.time_left(at + Duration::from_secs(1)), None);
        assert_eq!(
            conversion.time_left(at + Duration::from_secs(10)),
            Some(Duration::from_secs(30))
        );
    }
}
