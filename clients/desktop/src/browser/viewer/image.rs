//! The image viewer: the picture, scaled down to fit the page.

use iced::{
    ContentFit, Element, Length,
    widget::{container, image},
};

pub struct State {
    handle: image::Handle,
}

impl State {
    pub fn new(bytes: &[u8]) -> Self {
        Self {
            handle: image::Handle::from_bytes(bytes.to_vec()),
        }
    }

    pub fn view<'a, M: 'a>(&'a self) -> Element<'a, M> {
        container(
            image(&self.handle)
                .content_fit(ContentFit::ScaleDown)
                .width(Length::Fill)
                .height(Length::Fill),
        )
        .center(Length::Fill)
        .into()
    }
}

#[cfg(test)]
mod tests {
    use std::io::Cursor;

    use ::image::{ImageFormat, RgbaImage, load_from_memory};

    fn encoded(format: ImageFormat) -> Vec<u8> {
        let mut bytes = Cursor::new(Vec::new());
        RgbaImage::new(2, 2)
            .write_to(&mut bytes, format)
            .expect("the image is encoded");
        bytes.into_inner()
    }

    #[test]
    fn the_registered_formats_decode() {
        let formats = [
            ImageFormat::Png,
            ImageFormat::Bmp,
            ImageFormat::Gif,
            ImageFormat::Tiff,
            ImageFormat::Ico,
        ];
        let decoded: Vec<_> = formats
            .into_iter()
            .map(|format| load_from_memory(&encoded(format)).is_ok())
            .collect();
        assert_eq!(decoded, [true; 5]);
    }
}
