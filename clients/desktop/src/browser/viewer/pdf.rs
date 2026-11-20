//! The PDF viewer: the pages one under another, zoomable, rendered with hayro.
//!
//! A page is rendered off the UI thread when it scrolls near the view, at the current zoom, and
//! dropped again when it is far away, so a long document does not hold every page's pixels.

use std::{collections::HashMap, sync::Arc};

use iced::{
    Alignment, Color, Element, Length, Task,
    widget::{
        Space, column, container, image, row,
        scrollable::{self, Scrollbar, Viewport},
        text,
    },
};

use pdf_core::{Document, PageSize, Rendered};

use crate::{browser::parts, theme};

/// Logical pixels per PDF point at 100%: a point is 1/72 inch, a logical pixel 1/96.
const PIXELS_PER_POINT: f32 = 96.0 / 72.0;
/// Pages are rendered at this many pixels per logical pixel, sharp on high-density screens.
const RENDER_DENSITY: f32 = 2.0;
const ZOOMS: &[f32] = &[0.5, 0.75, 1.0, 1.25, 1.5, 2.0, 3.0];
const DEFAULT_ZOOM: f32 = 1.0;
const PAGE_GAP: f32 = 16.0;
const PADDING: f32 = 16.0;
/// How many pages beyond the view are rendered ahead, and kept, on each side.
const AHEAD: usize = 2;
/// The view's height until the first scroll reports it.
const FIRST_VIEW_HEIGHT: f32 = 1200.0;
const ZOOM_OUT: &str = "Zoom out";
const ZOOM_IN: &str = "Zoom in";

#[derive(Debug, Clone)]
pub enum Message {
    Scrolled(Viewport),
    Rendered {
        page: usize,
        zoom: Zoom,
        result: Result<Arc<Rendered>, String>,
    },
    ZoomOut,
    ZoomIn,
    ActualSize,
}

/// A zoom level, compared exactly: it is always one of [`ZOOMS`].
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Zoom(f32);

pub struct State {
    document: Arc<Document>,
    sizes: Vec<PageSize>,
    zoom: Zoom,
    /// Each page shown, with the zoom it was rendered at.
    rendered: HashMap<usize, (Zoom, image::Handle)>,
    /// Each page being rendered, at the zoom asked for.
    asked: HashMap<usize, Zoom>,
    /// The top of the view and its height, in logical pixels.
    view: (f32, f32),
    error: Option<String>,
}

impl State {
    /// Opens the PDF in `bytes` and renders its first pages; the error is for the user.
    pub fn open(bytes: &[u8]) -> Result<(Self, Task<Message>), String> {
        let document = Document::open(bytes.to_vec())?;
        let mut state = Self {
            sizes: document.page_sizes(),
            document: Arc::new(document),
            zoom: Zoom(DEFAULT_ZOOM),
            rendered: HashMap::new(),
            asked: HashMap::new(),
            view: (0.0, FIRST_VIEW_HEIGHT),
            error: None,
        };
        let task = state.render_near_view();
        Ok((state, task))
    }

    pub fn update(&mut self, message: Message) -> Task<Message> {
        match message {
            Message::Scrolled(viewport) => {
                self.view = (viewport.absolute_offset().y, viewport.bounds().height);
                self.render_near_view()
            }
            Message::Rendered { page, zoom, result } => {
                if self.asked.get(&page) == Some(&zoom) {
                    self.asked.remove(&page);
                }
                match result {
                    Ok(rendered) if zoom == self.zoom && self.near_view().contains(&page) => {
                        let handle = image::Handle::from_rgba(
                            rendered.width,
                            rendered.height,
                            rendered.rgba.clone(),
                        );
                        self.rendered.insert(page, (zoom, handle));
                    }
                    Ok(_) => {}
                    Err(err) => self.error = Some(err),
                }
                Task::none()
            }
            Message::ZoomOut => self.zoom_by(-1),
            Message::ZoomIn => self.zoom_by(1),
            Message::ActualSize => self.zoom_to(DEFAULT_ZOOM),
        }
    }

    fn zoom_by(&mut self, by: isize) -> Task<Message> {
        let at = ZOOMS
            .iter()
            .position(|level| *level >= self.zoom.0)
            .unwrap_or(ZOOMS.len() - 1);
        self.zoom_to(ZOOMS[at.saturating_add_signed(by).min(ZOOMS.len() - 1)])
    }

    fn zoom_to(&mut self, level: f32) -> Task<Message> {
        if self.zoom.0 == level {
            return Task::none();
        }
        // Keep the same part of the document in view.
        let ratio = level / self.zoom.0;
        self.view.0 *= ratio;
        self.zoom = Zoom(level);
        self.render_near_view()
    }

    /// The pages in view and [`AHEAD`] on each side.
    fn near_view(&self) -> std::ops::Range<usize> {
        let (top, height) = self.view;
        let mut y = PADDING;
        let (mut first, mut last) = (None, 0);
        for (index, size) in self.sizes.iter().enumerate() {
            let bottom = y + self.shown_size(*size).1;
            if bottom >= top && y <= top + height {
                first.get_or_insert(index);
                last = index;
            }
            y = bottom + PAGE_GAP;
        }
        let first = first.unwrap_or(0);
        first.saturating_sub(AHEAD)..(last + AHEAD + 1).min(self.sizes.len())
    }

    /// Renders the pages near the view that are not shown at this zoom, and drops the others.
    fn render_near_view(&mut self) -> Task<Message> {
        let near = self.near_view();
        self.rendered.retain(|page, _| near.contains(page));
        let zoom = self.zoom;
        let scale = zoom.0 * PIXELS_PER_POINT * RENDER_DENSITY;
        let wanted: Vec<_> = near
            .filter(|page| {
                self.rendered.get(page).is_none_or(|(at, _)| *at != zoom)
                    && self.asked.get(page) != Some(&zoom)
            })
            .collect();
        Task::batch(wanted.into_iter().map(|page| {
            self.asked.insert(page, zoom);
            let document = Arc::clone(&self.document);
            Task::perform(render(document, page, scale), move |result| {
                Message::Rendered { page, zoom, result }
            })
        }))
    }

    /// The size a page is shown at, in logical pixels.
    fn shown_size(&self, size: PageSize) -> (f32, f32) {
        let scale = self.zoom.0 * PIXELS_PER_POINT;
        (size.width * scale, size.height * scale)
    }

    /// The page at the top of the view, from 1.
    fn current_page(&self) -> usize {
        let mut y = PADDING;
        for (index, size) in self.sizes.iter().enumerate() {
            y += self.shown_size(*size).1 + PAGE_GAP;
            if y > self.view.0 {
                return index + 1;
            }
        }
        self.sizes.len()
    }

    /// The page counter and the zoom buttons.
    pub fn action(&self) -> Element<'_, Message> {
        let level = self.zoom.0;
        row![
            text(format!("{} / {}", self.current_page(), self.sizes.len()))
                .style(theme::muted_text),
            parts::ghost(
                None,
                ZOOM_OUT,
                (level > ZOOMS[0]).then_some(Message::ZoomOut),
            ),
            parts::ghost(
                None,
                format!("{:.0}%", level * 100.0),
                (level != DEFAULT_ZOOM).then_some(Message::ActualSize),
            ),
            parts::ghost(
                None,
                ZOOM_IN,
                (level < ZOOMS[ZOOMS.len() - 1]).then_some(Message::ZoomIn),
            ),
        ]
        .spacing(super::SPACING)
        .align_y(Alignment::Center)
        .into()
    }

    pub fn view(&self) -> Element<'_, Message> {
        let pages = self.sizes.iter().enumerate().map(|(index, size)| {
            let (width, height) = self.shown_size(*size);
            let page: Element<'_, Message> = match self.rendered.get(&index) {
                Some((_, handle)) => image(handle)
                    .width(Length::Fixed(width))
                    .height(Length::Fixed(height))
                    .into(),
                None => Space::new()
                    .width(Length::Fixed(width))
                    .height(Length::Fixed(height))
                    .into(),
            };
            container(page)
                .style(|_| container::Style::default().background(Color::WHITE))
                .into()
        });
        let pages = column(pages)
            .spacing(PAGE_GAP)
            .padding(PADDING)
            .align_x(Alignment::Center)
            .width(Length::Fill);
        let error = self
            .error
            .as_deref()
            .map(|err| parts::alert_error(crate::icons::Icon::Error, err, None));
        column![]
            .push(error)
            .push(
                scrollable::Scrollable::new(pages)
                    .direction(scrollable::Direction::Both {
                        vertical: Scrollbar::default(),
                        horizontal: Scrollbar::default(),
                    })
                    .on_scroll(Message::Scrolled)
                    .width(Length::Fill)
                    .height(Length::Fill),
            )
            .spacing(super::SPACING)
            .into()
    }
}

async fn render(document: Arc<Document>, page: usize, scale: f32) -> Result<Arc<Rendered>, String> {
    tokio::task::spawn_blocking(move || document.render(page, scale))
        .await
        .map_err(|err| format!("the page could not be rendered: {err}"))?
        .map(Arc::new)
        .ok_or_else(|| "the PDF has no such page".to_owned())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample() -> State {
        let (state, _task) = State::open(&pdf_core::sample_pdf(3)).expect("pdf");
        state
    }

    #[test]
    fn opening_a_pdf_asks_for_its_first_pages() {
        let state = sample();
        assert_eq!(state.sizes.len(), 3);
        assert_eq!(state.asked.len(), 3);
    }

    #[test]
    fn a_file_that_is_not_a_pdf_is_refused() {
        assert!(State::open(b"hello").is_err());
    }

    #[test]
    fn zooming_in_asks_for_the_pages_again() {
        let mut state = sample();
        state.asked.clear();
        let _ = state.update(Message::ZoomIn);
        assert_eq!(state.zoom, Zoom(1.25));
        assert!(state.asked.values().all(|zoom| *zoom == Zoom(1.25)));
        assert!(!state.asked.is_empty());
    }

    #[test]
    fn a_render_for_an_old_zoom_is_dropped() {
        let mut state = sample();
        let _ = state.update(Message::ZoomIn);
        let page = Rendered {
            width: 1,
            height: 1,
            rgba: vec![255; 4],
        };
        let _ = state.update(Message::Rendered {
            page: 0,
            zoom: Zoom(1.0),
            result: Ok(Arc::new(page)),
        });
        assert!(state.rendered.is_empty());
    }

    #[test]
    fn zoom_stops_at_its_limits() {
        let mut state = sample();
        for _ in 0..10 {
            let _ = state.update(Message::ZoomIn);
        }
        assert_eq!(state.zoom, Zoom(3.0));
        for _ in 0..10 {
            let _ = state.update(Message::ZoomOut);
        }
        assert_eq!(state.zoom, Zoom(0.5));
    }
}
