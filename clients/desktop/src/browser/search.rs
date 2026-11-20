use std::{ops::Range, time::Duration};

use iced::{
    Alignment, Element, Length, Task,
    widget::{button, column, container, responsive, row, scrollable, text},
};

use api_client::{ApiError, Client};
use client_core::{format::human_size, labels::location_label, search::destination};
use protocol::{NodeId, NodeKind, SearchHit};

use super::{
    Message as BrowserMessage,
    parts::{
        COLUMN_GAP, DATE_WIDTH, LIST_PADDING_WIDE, ROW_HEIGHT, ROW_SIZE, SECOND_LINE_SIZE,
        SIZE_WIDTH, alert_error, divider, empty, header_bar, list_box, muted_cell, title,
    },
};
use crate::{
    ellipsis::fit,
    icons::{ICON_SIZE, Icon, Tint, node_icon},
    locale::short_date,
    theme,
};

/// How long typing must pause before the query is sent.
pub const DEBOUNCE: Duration = Duration::from_millis(250);

const SPACING: f32 = 12.0;
const NAME_GAP: f32 = 12.0;
const NAME_PORTION: u16 = 3;
const LOCATION_PORTION: u16 = 2;

#[derive(Debug, Clone)]
pub enum Message {
    Changed(String),
    Debounced(u64),
    Loaded(u64, Result<Vec<SearchHit>, ApiError>),
    Open(usize),
    Clear,
}

/// What the browser has to do after a search update.
pub enum Outcome {
    Task(Task<Message>),
    /// Show the folder with this id (`None` for Home).
    OpenFolder(Option<NodeId>),
}

/// The search box and the results of the last query that finished.
#[derive(Debug, Default)]
pub struct Search {
    query: String,
    pub(super) seq: u64,
    hits: Vec<SearchHit>,
    loading: bool,
    error: Option<String>,
}

impl Search {
    /// Whether results replace the folder listing.
    pub fn is_active(&self) -> bool {
        !self.query.trim().is_empty()
    }

    pub fn query(&self) -> &str {
        &self.query
    }

    /// Takes the text typed so far; returns the sequence number to debounce when there is
    /// something to search for.
    pub fn change(&mut self, query: String) -> Option<u64> {
        self.query = query;
        self.seq += 1;
        self.hits.clear();
        self.error = None;
        self.loading = false;
        self.is_active().then_some(self.seq)
    }

    pub fn clear(&mut self) {
        self.change(String::new());
    }

    /// Whether `seq` still belongs to the latest typed query.
    pub fn accepts(&self, seq: u64) -> bool {
        seq == self.seq
    }

    pub fn start(&mut self) {
        self.loading = true;
        self.error = None;
    }

    pub fn finish(&mut self, result: Result<Vec<SearchHit>, ApiError>) {
        self.loading = false;
        match result {
            Ok(hits) => self.hits = hits,
            Err(err) => self.error = Some(err.to_string()),
        }
    }

    pub fn hit(&self, index: usize) -> Option<&SearchHit> {
        self.hits.get(index)
    }

    pub fn update(&mut self, client: &Client, message: Message) -> Outcome {
        match message {
            Message::Changed(query) => match self.change(query) {
                Some(seq) => Outcome::Task(Task::perform(
                    async move {
                        tokio::time::sleep(DEBOUNCE).await;
                        seq
                    },
                    Message::Debounced,
                )),
                None => Outcome::Task(Task::none()),
            },
            Message::Debounced(seq) if self.accepts(seq) => {
                self.start();
                let client = client.clone();
                let query = self.query.trim().to_owned();
                Outcome::Task(Task::perform(
                    async move { client.search(&query).await },
                    move |result| Message::Loaded(seq, result),
                ))
            }
            Message::Debounced(_) => Outcome::Task(Task::none()),
            Message::Loaded(seq, result) => {
                if self.accepts(seq) {
                    self.finish(result);
                }
                Outcome::Task(Task::none())
            }
            Message::Open(index) => match self.hit(index).map(destination) {
                Some(folder) => Outcome::OpenFolder(folder),
                None => Outcome::Task(Task::none()),
            },
            Message::Clear => {
                self.clear();
                Outcome::Task(Task::none())
            }
        }
    }

    /// How many results there are over the box with their rows, or a hint when there is
    /// nothing to list.
    pub fn view(&self) -> Element<'_, BrowserMessage> {
        if let Some(error) = &self.error {
            alert_error(Icon::Error, error.as_str(), None)
        } else if self.hits.is_empty() {
            list_box(empty(if self.loading {
                "Searching\u{2026}"
            } else {
                "Nothing matches your search."
            }))
        } else {
            let query = self.query.trim();
            let rows = self
                .hits
                .iter()
                .enumerate()
                .flat_map(|(index, hit)| [hit_row(index, hit, query), divider()]);
            column![
                text(count_label(self.hits.len()))
                    .size(SECOND_LINE_SIZE)
                    .style(theme::muted_text),
                list_box(column![
                    header(),
                    scrollable(column(rows)).height(Length::Fill)
                ]),
            ]
            .spacing(SPACING)
            .into()
        }
    }
}

fn count_label(count: usize) -> String {
    match count {
        1 => "1 result".to_owned(),
        count => format!("{count} results"),
    }
}

fn header<'a>() -> Element<'a, BrowserMessage> {
    header_bar(
        row![
            title("Name", Length::FillPortion(NAME_PORTION), Alignment::Start),
            title(
                "Location",
                Length::FillPortion(LOCATION_PORTION),
                Alignment::Start
            ),
            title("Size", SIZE_WIDTH, Alignment::End),
            title("Modified", DATE_WIDTH, Alignment::End),
        ],
        LIST_PADDING_WIDE,
    )
}

fn hit_row<'a>(index: usize, hit: &'a SearchHit, query: &str) -> Element<'a, BrowserMessage> {
    let node = &hit.node;
    let icon = node_icon(node);
    let tint = match icon {
        Icon::Folder => Tint::Accent,
        _ => Tint::Text,
    };
    let size = match node.kind {
        NodeKind::Folder => String::new(),
        NodeKind::File => human_size(node.size),
    };
    let cells = row![
        row![
            icon.view(ICON_SIZE, tint),
            highlighted(node.name.as_str(), query.to_owned())
        ]
        .spacing(NAME_GAP)
        .align_y(Alignment::Center)
        .width(Length::FillPortion(NAME_PORTION)),
        muted_cell(
            location_label(&hit.location),
            Length::FillPortion(LOCATION_PORTION),
            Alignment::Start
        ),
        muted_cell(size, SIZE_WIDTH, Alignment::End),
        muted_cell(short_date(node.updated_at), DATE_WIDTH, Alignment::End),
    ]
    .spacing(COLUMN_GAP)
    .height(Length::Fill)
    .align_y(Alignment::Center);
    button(cells)
        .width(Length::Fill)
        .height(ROW_HEIGHT)
        .padding(LIST_PADDING_WIDE)
        .style(theme::row(false))
        .on_press(BrowserMessage::Search(Message::Open(index)))
        .into()
}

/// `name` on one line with the part matching `query` in bold, cut short when it does not fit.
/// Three texts in a row, as rich text keeps a span's weight up to the end of the word.
fn highlighted(name: &str, query: String) -> Element<'_, BrowserMessage> {
    responsive(move |bounds| {
        let shown = fit(name, ROW_SIZE, bounds.width);
        let part = |part: &str, font| {
            text(part.to_owned())
                .size(ROW_SIZE)
                .font(font)
                .wrapping(text::Wrapping::None)
        };
        let line = match find_ignoring_case(&shown, &query) {
            Some(found) => row![
                part(&shown[..found.start], theme::FONT),
                part(&shown[found.clone()], theme::BOLD),
                part(&shown[found.end..], theme::FONT),
            ],
            None => row![part(&shown, theme::FONT)],
        };
        container(line).clip(true).into()
    })
    .height(Length::Shrink)
    .into()
}

/// Where `query` first appears in `name`, whatever the case of either.
fn find_ignoring_case(name: &str, query: &str) -> Option<Range<usize>> {
    let query: Vec<char> = query.chars().flat_map(char::to_lowercase).collect();
    if query.is_empty() {
        return None;
    }
    name.char_indices().find_map(|(start, _)| {
        let mut left = query.as_slice();
        for (at, found) in name[start..].char_indices() {
            for lower in found.to_lowercase() {
                match left.split_first() {
                    Some((first, rest)) if *first == lower => left = rest,
                    _ => return None,
                }
            }
            if left.is_empty() {
                return Some(start..start + at + found.len_utf8());
            }
        }
        None
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn blank_text_is_not_a_search() {
        let mut search = Search::default();
        assert_eq!(search.change("  ".to_owned()), None);
        assert!(!search.is_active());
    }

    #[test]
    fn typing_invalidates_earlier_queries() {
        let mut search = Search::default();
        let first = search.change("a".to_owned()).expect("active");
        let second = search.change("ab".to_owned()).expect("active");
        assert!(!search.accepts(first));
        assert!(search.accepts(second));
    }

    #[test]
    fn a_match_is_found_whatever_the_case() {
        assert_eq!(
            find_ignoring_case("electricity-Invoice.jpg", "invoice"),
            Some(12..19)
        );
        assert_eq!(find_ignoring_case("invoice", "INV"), Some(0..3));
    }

    #[test]
    fn a_match_counts_bytes_after_wider_characters() {
        assert_eq!(find_ignoring_case("Ünï-file", "FILE"), Some(6..10));
        assert_eq!(find_ignoring_case("ÜNÏ", "ï"), Some(3..5));
    }

    #[test]
    fn no_match_or_query_highlights_nothing() {
        assert_eq!(find_ignoring_case("report.pdf", "invoice"), None);
        assert_eq!(find_ignoring_case("report.pdf", ""), None);
    }

    #[test]
    fn the_count_reads_naturally() {
        assert_eq!(count_label(1), "1 result");
        assert_eq!(count_label(5), "5 results");
    }

    #[test]
    fn clearing_ends_the_search() {
        let mut search = Search::default();
        search.change("a".to_owned());
        search.clear();
        assert!(!search.is_active());
        assert_eq!(search.query(), "");
    }
}
