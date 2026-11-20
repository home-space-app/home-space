//! The messages between the web client and its PDF worker.
//!
//! Each message travels as a two-item array: the message as JSON, then an `ArrayBuffer` for
//! the bytes that go with it (the PDF on [`Request::Open`], the pixels on
//! [`Response::Rendered`]) or `undefined`. The buffer is transferred, not copied.

use serde::{Deserialize, Serialize};

use crate::PageSize;

/// What the web client asks the worker.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum Request {
    /// Opens the PDF in the buffer, in place of any document open before.
    Open,
    /// Renders `page` (from 0) of the open document at `scale` pixels per point, in place of a
    /// render of that page still waiting for its turn.
    Render {
        request_id: u32,
        page: u32,
        scale: f32,
    },
    /// Drops the render of `page` that is still waiting for its turn, if any: the page has left
    /// the view.
    Cancel { page: u32 },
}

/// What the worker answers.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum Response {
    /// The worker has started and listens; messages sent before this are lost.
    Ready,
    /// The document is open; it has one size per page.
    Opened { pages: Vec<PageSize> },
    /// The document could not be opened.
    Failed { message: String },
    /// The page of `request_id`, as `width` × `height` RGBA pixels in the buffer.
    Rendered {
        request_id: u32,
        page: u32,
        width: u32,
        height: u32,
    },
    /// The page of `request_id` could not be rendered.
    RenderFailed { request_id: u32, message: String },
}

impl Request {
    pub fn to_json(&self) -> String {
        serde_json::to_string(self).expect("a request serializes")
    }

    pub fn from_json(json: &str) -> Result<Self, String> {
        serde_json::from_str(json).map_err(|err| err.to_string())
    }
}

impl Response {
    pub fn to_json(&self) -> String {
        serde_json::to_string(self).expect("a response serializes")
    }

    pub fn from_json(json: &str) -> Result<Self, String> {
        serde_json::from_str(json).map_err(|err| err.to_string())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn requests_round_trip() {
        let render = Request::Render {
            request_id: 7,
            page: 2,
            scale: 1.5,
        };
        assert_eq!(Request::from_json(&render.to_json()), Ok(render));
        let cancel = Request::Cancel { page: 2 };
        assert_eq!(Request::from_json(&cancel.to_json()), Ok(cancel));
    }

    #[test]
    fn responses_round_trip() {
        let opened = Response::Opened {
            pages: vec![PageSize {
                width: 612.0,
                height: 792.0,
            }],
        };
        assert_eq!(Response::from_json(&opened.to_json()), Ok(opened));
    }
}
