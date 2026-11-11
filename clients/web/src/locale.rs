use js_sys::{Array, Date, Function, Intl::DateTimeFormat, Object, Reflect};
use time::OffsetDateTime;
use wasm_bindgen::JsValue;

/// Formats `time` in the browser's regional settings, in the local time zone.
pub fn short_date(time: OffsetDateTime) -> String {
    format(time, &[("dateStyle", "short"), ("timeStyle", "short")])
        .unwrap_or_else(|| client_core::format::short_date(time))
}

/// The local time of day of `time`, e.g. `14:32`.
pub fn short_time(time: OffsetDateTime) -> String {
    format(time, &[("timeStyle", "short")])
        .unwrap_or_else(|| format!("{:02}:{:02}", time.hour(), time.minute()))
}

/// The local date of `time` with the month in words, e.g. `20 Mar 2026`.
pub fn medium_date(time: OffsetDateTime) -> String {
    format(time, &[("dateStyle", "medium")])
        .unwrap_or_else(|| client_core::format::short_date(time))
}

fn format(time: OffsetDateTime, style: &[(&str, &str)]) -> Option<String> {
    let millis = (time.unix_timestamp_nanos() / 1_000_000) as f64;
    let date = Date::new(&JsValue::from_f64(millis));
    let options = Object::new();
    for (key, value) in style {
        let _ = Reflect::set(&options, &(*key).into(), &(*value).into());
    }
    let format: Function = DateTimeFormat::new(&Array::new(), &options).format();
    format
        .call1(&JsValue::NULL, &date)
        .ok()
        .and_then(|text| text.as_string())
}

/// The current time, from the browser's clock: the standard library has none in wasm.
pub fn now() -> OffsetDateTime {
    let millis = Date::now() as i128;
    OffsetDateTime::from_unix_timestamp_nanos(millis * 1_000_000)
        .unwrap_or(OffsetDateTime::UNIX_EPOCH)
}
