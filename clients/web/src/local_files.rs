//! Files and folders picked on this computer, from a file input or dropped on the page.

use std::{future::Future, pin::Pin};

use js_sys::{Array, Function, Promise};
use wasm_bindgen::{JsCast, JsValue};
use wasm_bindgen_futures::JsFuture;
use web_sys::{
    DataTransfer, File, FileList, FileSystemDirectoryEntry, FileSystemDirectoryReader,
    FileSystemEntry, FileSystemFileEntry,
};

const RELATIVE_PATH: &str = "webkitRelativePath";

/// One file or folder picked at the top of a send, with everything a folder holds.
#[derive(Clone)]
pub struct Picked {
    /// Its name, which is also the first segment of every path below.
    pub name: String,
    pub folder: bool,
    /// Each file, by its path in the transfer: `a.pdf`, or `Trip/day 1/a.jpg`.
    pub files: Vec<(String, File)>,
    /// Folders inside a picked folder that hold no files, by their path.
    pub empty_folders: Vec<String>,
}

impl Picked {
    fn file(file: File) -> Self {
        let name = file.name();
        Self {
            name: name.clone(),
            folder: false,
            files: vec![(name, file)],
            empty_folders: Vec::new(),
        }
    }

    fn folder(name: String) -> Self {
        Self {
            name,
            folder: true,
            files: Vec::new(),
            empty_folders: Vec::new(),
        }
    }

    pub fn bytes(&self) -> u64 {
        self.files.iter().map(|(_, file)| file.size() as u64).sum()
    }

    /// Gives it the name `name`, moving every path under it along.
    pub fn rename(&mut self, name: String) {
        let rename = |path: &str| client_core::send::with_top(path, &name);
        for (path, _) in &mut self.files {
            *path = rename(path);
        }
        for path in &mut self.empty_folders {
            *path = rename(path);
        }
        self.name = name;
    }
}

/// The path a folder input gave `file`, such as `Trip/day 1/a.jpg`; empty for a lone file.
fn relative_path(file: &File) -> String {
    js_sys::Reflect::get(file, &JsValue::from_str(RELATIVE_PATH))
        .ok()
        .and_then(|path| path.as_string())
        .unwrap_or_default()
}

/// What a file input picked: each file on its own, or one folder with what is inside it.
pub fn from_list(list: &FileList) -> Vec<Picked> {
    let mut picked: Vec<Picked> = Vec::new();
    for file in (0..list.length()).filter_map(|index| list.get(index)) {
        let path = relative_path(&file);
        if path.is_empty() {
            picked.push(Picked::file(file));
            continue;
        }
        let top = client_core::send::top_name(&path).to_owned();
        let at = match picked
            .iter()
            .position(|item| item.folder && item.name == top)
        {
            Some(at) => at,
            None => {
                picked.push(Picked::folder(top));
                picked.len() - 1
            }
        };
        picked[at].files.push((path, file));
    }
    picked
}

/// The entries dropped with `transfer`. They must be taken before the drop handler returns;
/// the browser empties the drop after that.
pub fn dropped_entries(transfer: &DataTransfer) -> Vec<FileSystemEntry> {
    let items = transfer.items();
    (0..items.length())
        .filter_map(|index| items.get(index))
        .filter(|item| item.kind() == "file")
        .filter_map(|item| item.webkit_get_as_entry().ok().flatten())
        .collect()
}

/// Reads the dropped `entries`, going through folders.
pub async fn read_dropped(entries: Vec<FileSystemEntry>) -> Result<Vec<Picked>, JsValue> {
    let mut picked = Vec::new();
    for entry in entries {
        if entry.is_directory() {
            let mut folder = Picked::folder(entry.name());
            walk(entry.unchecked_into(), folder.name.clone(), &mut folder).await?;
            picked.push(folder);
        } else if entry.is_file() {
            picked.push(Picked::file(file_of(entry.unchecked_ref()).await?));
        }
    }
    Ok(picked)
}

/// Adds what `directory`, at `path`, holds to `into`.
fn walk<'a>(
    directory: FileSystemDirectoryEntry,
    path: String,
    into: &'a mut Picked,
) -> Pin<Box<dyn Future<Output = Result<(), JsValue>> + 'a>> {
    Box::pin(async move {
        let children = read_all(&directory.create_reader()).await?;
        if children.is_empty() {
            into.empty_folders.push(path);
            return Ok(());
        }
        for child in children {
            let child_path = format!("{path}/{}", child.name());
            if child.is_directory() {
                walk(child.unchecked_into(), child_path, into).await?;
            } else if child.is_file() {
                let file = file_of(child.unchecked_ref()).await?;
                into.files.push((child_path, file));
            }
        }
        Ok(())
    })
}

/// Every entry of a folder; the browser hands them out in batches until one comes back empty.
async fn read_all(reader: &FileSystemDirectoryReader) -> Result<Vec<FileSystemEntry>, JsValue> {
    let mut entries = Vec::new();
    loop {
        let batch = callback_promise(|resolve, reject| {
            let _ = reader.read_entries_with_callback_and_callback(resolve, reject);
        })
        .await?;
        let batch: Array = batch.unchecked_into();
        if batch.length() == 0 {
            return Ok(entries);
        }
        entries.extend(batch.iter().map(JsCast::unchecked_into::<FileSystemEntry>));
    }
}

async fn file_of(entry: &FileSystemFileEntry) -> Result<File, JsValue> {
    let file = callback_promise(|resolve, reject| {
        entry.file_with_callback_and_callback(resolve, reject);
    })
    .await?;
    Ok(file.unchecked_into())
}

/// Awaits an API that reports through a success and an error callback.
async fn callback_promise(start: impl FnOnce(&Function, &Function)) -> Result<JsValue, JsValue> {
    let mut start = Some(start);
    let promise = Promise::new(&mut |resolve, reject| {
        if let Some(start) = start.take() {
            start(&resolve, &reject);
        }
    });
    JsFuture::from(promise).await
}
