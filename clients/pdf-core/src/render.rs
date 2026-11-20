//! Rendering pages with hayro.

use hayro::{
    PixmapSettings, RenderCache, RenderSettings,
    hayro_interpret::InterpreterSettings,
    hayro_syntax::{LoadPdfError, Pdf},
    render,
    vello_cpu::color::palette::css::WHITE,
};

use crate::PageSize;

const ENCRYPTED: &str = "This PDF is protected with a password, which the viewer cannot open.";
const INVALID: &str = "This file is not a PDF the viewer can read.";

/// An open PDF document.
pub struct Document {
    pdf: Pdf,
    settings: InterpreterSettings,
}

/// A rendered page: `width` × `height` pixels, RGBA, row by row.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Rendered {
    pub width: u32,
    pub height: u32,
    pub rgba: Vec<u8>,
}

impl std::fmt::Debug for Document {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Document")
            .field("pages", &self.page_count())
            .finish()
    }
}

impl Document {
    /// Opens the PDF in `bytes`; the error is a message for the user.
    pub fn open(bytes: Vec<u8>) -> Result<Self, String> {
        let pdf = Pdf::new(bytes).map_err(|err| match err {
            LoadPdfError::Decryption(_) => ENCRYPTED.to_owned(),
            LoadPdfError::Invalid => INVALID.to_owned(),
        })?;
        if pdf.pages().is_empty() {
            return Err(INVALID.to_owned());
        }
        Ok(Self {
            pdf,
            settings: InterpreterSettings::default(),
        })
    }

    pub fn page_count(&self) -> usize {
        self.pdf.pages().len()
    }

    /// The size of every page, in points.
    pub fn page_sizes(&self) -> Vec<PageSize> {
        self.pdf
            .pages()
            .iter()
            .map(|page| {
                let (width, height) = page.render_dimensions();
                PageSize { width, height }
            })
            .collect()
    }

    /// Page `index` (from 0) at `scale` pixels per point, on white; `None` past the last page.
    /// The scale is lowered to keep the page within [`crate::MAX_SIDE_PIXELS`].
    pub fn render(&self, index: usize, scale: f32) -> Option<Rendered> {
        let page = self.pdf.pages().get(index)?;
        let (width, height) = page.render_dimensions();
        let scale = PageSize { width, height }.clamp_scale(scale);
        // A cache borrows the document, so it lives for one page; the pages the user looks at
        // are rendered once per zoom level, which keeps this cheap enough.
        let cache = RenderCache::new();
        let pixmap = render(
            page,
            &cache,
            &self.settings,
            &RenderSettings::default(),
            &PixmapSettings {
                x_scale: scale,
                y_scale: scale,
                bg_color: WHITE,
            },
        );
        Some(Rendered {
            width: u32::from(pixmap.width()),
            height: u32::from(pixmap.height()),
            // The page is painted on opaque white, so premultiplied and straight alpha agree.
            rgba: pixmap.data_as_u8_slice().to_vec(),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_pdf_lists_its_pages() {
        let document = Document::open(crate::sample_pdf(1)).expect("pdf");
        assert_eq!(
            document.page_sizes(),
            [PageSize {
                width: 200.0,
                height: 100.0
            }]
        );
    }

    #[test]
    fn a_page_renders_at_its_scale() {
        let document = Document::open(crate::sample_pdf(1)).expect("pdf");
        let page = document.render(0, 2.0).expect("page");
        assert_eq!((page.width, page.height), (400, 200));
        assert_eq!(page.rgba.len(), 400 * 200 * 4);
        // The top-left corner is the white page; the square is painted near the bottom left.
        assert_eq!(&page.rgba[..4], &[255, 255, 255, 255]);
        let row = 200 - 40;
        let at = (row * 400 + 40) * 4;
        assert_eq!(&page.rgba[at..at + 3], &[0, 0, 0]);
    }

    #[test]
    fn there_is_no_page_past_the_last() {
        let document = Document::open(crate::sample_pdf(1)).expect("pdf");
        assert!(document.render(1, 1.0).is_none());
    }

    #[test]
    fn other_files_are_not_pdfs() {
        assert_eq!(
            Document::open(b"hello".to_vec()).map(|_| ()),
            Err(INVALID.to_owned())
        );
    }
}
