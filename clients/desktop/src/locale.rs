use std::sync::OnceLock;

use time::{OffsetDateTime, UtcOffset};

static LOCAL_OFFSET: OnceLock<UtcOffset> = OnceLock::new();

/// Reads the system's offset from UTC. Call it before any other thread starts: on Unix the
/// offset can only be read safely while the process has one thread.
pub fn init_local_offset() {
    let _ = LOCAL_OFFSET.set(UtcOffset::current_local_offset().unwrap_or(UtcOffset::UTC));
}

/// The system's offset from UTC as read at start; UTC when it could not be read.
pub fn local_offset() -> UtcOffset {
    LOCAL_OFFSET.get().copied().unwrap_or(UtcOffset::UTC)
}

/// Formats `time` per the system's regional settings, falling back to the fixed
/// `YYYY-MM-DD HH:MM` form when the locale or timestamp can't be handled.
pub fn short_date(time: OffsetDateTime) -> String {
    platform::short_date(time).unwrap_or_else(|| client_core::format::short_date(time))
}

/// macOS keeps the region (and any custom date format) apart from the language, e.g.
/// `en_US@rg=atzzzz`, so let Foundation format it the way the rest of the system does.
#[cfg(target_os = "macos")]
mod platform {
    use objc2::rc::Retained;
    use objc2_foundation::{NSDate, NSDateFormatter, NSDateFormatterStyle};
    use time::OffsetDateTime;

    pub fn short_date(time: OffsetDateTime) -> Option<String> {
        thread_local! {
            static FORMATTER: Retained<NSDateFormatter> = formatter();
        }
        let seconds = time.unix_timestamp_nanos() as f64 / 1e9;
        let date = NSDate::dateWithTimeIntervalSince1970(seconds);
        Some(FORMATTER.with(|formatter| formatter.stringFromDate(&date).to_string()))
    }

    fn formatter() -> Retained<NSDateFormatter> {
        let formatter = NSDateFormatter::new();
        formatter.setDateStyle(NSDateFormatterStyle::ShortStyle);
        formatter.setTimeStyle(NSDateFormatterStyle::ShortStyle);
        formatter
    }
}

#[cfg(not(target_os = "macos"))]
mod platform {
    use icu_datetime::{
        DateTimeFormatter,
        fieldsets::{self, YMD},
        input::{Date, DateTime, Time},
    };
    use icu_locale::Locale;
    use time::OffsetDateTime;

    type Formatter = DateTimeFormatter<fieldsets::YMDT>;

    pub fn short_date(time: OffsetDateTime) -> Option<String> {
        thread_local! {
            static FORMATTER: Option<Formatter> = formatter();
        }
        FORMATTER.with(|formatter| {
            formatter
                .as_ref()
                .and_then(|formatter| localized(formatter, time))
        })
    }

    fn formatter() -> Option<Formatter> {
        let locale = sys_locale::get_locale()?.parse::<Locale>().ok()?;
        DateTimeFormatter::try_new(locale.into(), YMD::short().with_time_hm()).ok()
    }

    fn localized(formatter: &Formatter, time: OffsetDateTime) -> Option<String> {
        let date = Date::try_new_iso(time.year(), u8::from(time.month()), time.day()).ok()?;
        let time = Time::try_new(time.hour(), time.minute(), time.second(), 0).ok()?;
        Some(formatter.format(&DateTime { date, time }).to_string())
    }
}
