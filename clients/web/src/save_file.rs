//! Writing a download into a file the user picks, where the browser has the File System
//! Access API (Chromium, on a secure origin). Unlike a download handed to the browser, the
//! page then knows when the file is complete.

use js_sys::{Array, Function, Object, Promise, Reflect, Uint8Array};
use wasm_bindgen::{JsCast, JsValue};
use wasm_bindgen_futures::JsFuture;
use web_sys::{ReadableStreamDefaultReader, Response};

const PICKER: &str = "showSaveFilePicker";
const SUGGESTED_NAME: &str = "suggestedName";
/// What the picker fails with when the user closes it.
const ABORT_ERROR: &str = "AbortError";
const FAILED: &str = "The download could not be saved";

/// Whether this browser can write a download itself.
pub fn supported() -> bool {
    web_sys::window().is_some_and(|window| {
        Reflect::get(&window, &PICKER.into()).is_ok_and(|picker| picker.is_function())
    })
}

/// A file the user picked to save into.
pub struct Target(JsValue);

/// How asking for a file to save into ended.
pub enum Pick {
    Picked(Target),
    /// The user closed the picker.
    Cancelled,
    /// The browser has no picker, or refused to open it.
    Unavailable,
}

/// Calls `target.method(...args)`, waiting for the result when it is a promise.
async fn call(target: &JsValue, method: &str, args: &[JsValue]) -> Result<JsValue, JsValue> {
    let function: Function = Reflect::get(target, &method.into())?.dyn_into()?;
    let result = function.apply(target, &args.iter().collect::<Array>())?;
    match result.dyn_into::<Promise>() {
        Ok(promise) => JsFuture::from(promise).await,
        Err(value) => Ok(value),
    }
}

fn text(error: JsValue, key: &str) -> Option<String> {
    Reflect::get(&error, &key.into()).ok()?.as_string()
}

fn message(error: JsValue) -> String {
    match text(error, "message") {
        Some(message) if !message.is_empty() => format!("{FAILED}: {message}"),
        _ => FAILED.to_owned(),
    }
}

/// Asks where to save a file, suggesting `name`. Must follow a click closely: browsers only
/// open the picker for a user's action.
pub async fn pick(name: &str) -> Pick {
    let Some(window) = web_sys::window().filter(|_| supported()) else {
        return Pick::Unavailable;
    };
    let options = Object::new();
    let _ = Reflect::set(&options, &SUGGESTED_NAME.into(), &name.into());
    match call(&window, PICKER, &[options.into()]).await {
        Ok(handle) => Pick::Picked(Target(handle)),
        Err(error) if text(error.clone(), "name").as_deref() == Some(ABORT_ERROR) => {
            Pick::Cancelled
        }
        Err(_) => Pick::Unavailable,
    }
}

impl Target {
    /// Downloads `url` into the file, telling `on_progress` how many bytes are written so
    /// far. A download that fails leaves no file behind.
    pub async fn write_from(&self, url: &str, on_progress: impl Fn(f64)) -> Result<(), String> {
        let writable = call(&self.0, "createWritable", &[])
            .await
            .map_err(message)?;
        let written = match copy(url, &writable, on_progress).await {
            Ok(()) => call(&writable, "close", &[]).await.map(|_| ()),
            Err(error) => Err(error),
        };
        if written.is_err() {
            let _ = call(&writable, "abort", &[]).await;
            // The picker made an empty file; not every browser can take it away again.
            let _ = call(&self.0, "remove", &[]).await;
        }
        written.map_err(message)
    }
}

async fn copy(url: &str, writable: &JsValue, on_progress: impl Fn(f64)) -> Result<(), JsValue> {
    let window = web_sys::window().ok_or(JsValue::NULL)?;
    let response: Response = JsFuture::from(window.fetch_with_str(url))
        .await?
        .dyn_into()?;
    if !response.ok() {
        let error = Object::new();
        let text = format!("the server answered {}", response.status());
        Reflect::set(&error, &"message".into(), &text.into())?;
        return Err(error.into());
    }
    let reader: ReadableStreamDefaultReader = response
        .body()
        .ok_or(JsValue::NULL)?
        .get_reader()
        .dyn_into()?;
    let mut done = 0.0;
    loop {
        let chunk = JsFuture::from(reader.read()).await?;
        if Reflect::get(&chunk, &"done".into())?.is_truthy() {
            return Ok(());
        }
        let value = Reflect::get(&chunk, &"value".into())?;
        done += value
            .dyn_ref::<Uint8Array>()
            .map_or(0.0, |bytes| f64::from(bytes.length()));
        call(writable, "write", &[value]).await?;
        on_progress(done);
    }
}
