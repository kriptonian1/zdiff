//! A changed image or SVG, shown as pictures when the terminal has an image protocol.

use std::borrow::Cow;
use std::fmt;
use std::io::{Cursor, Read};
use std::sync::Arc;

use flate2::read::GzDecoder;
use image::{ImageReader, RgbaImage, imageops};
use ratatui_image::picker::Picker;
use ratatui_image::protocol::StatefulProtocol;
use resvg::usvg::fontdb::{Database, Family};
use resvg::{tiny_skia, usvg};
use zdiff_core::FileDiff;

/// Larger files show only their size; decoding them would cost too much memory.
const MAX_DECODE: usize = 20 << 20;
/// Starts every gzip stream, so an `.svgz`.
const GZIP_MAGIC: [u8; 2] = [0x1f, 0x8b];
/// Width of the swipe divider, in image pixels.
const DIVIDER_PX: u32 = 2;

/// How both sides of a changed image are compared, as on GitHub.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Compare {
    /// Side by side, or stacked in unified view.
    #[default]
    TwoUp,
    /// Old left of a divider, new right of it.
    Swipe,
    /// New faded over old.
    Onion,
}

impl Compare {
    pub const ALL: [Self; 3] = [Self::TwoUp, Self::Swipe, Self::Onion];

    pub fn label(self) -> &'static str {
        match self {
            Self::TwoUp => "2-up",
            Self::Swipe => "Swipe",
            Self::Onion => "Onion skin",
        }
    }

    pub fn next(self) -> Self {
        match self {
            Self::TwoUp => Self::Swipe,
            Self::Swipe => Self::Onion,
            Self::Onion => Self::TwoUp,
        }
    }
}

/// Whether an SVG shows as code or as rendered pictures.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum SvgView {
    #[default]
    Code,
    Preview,
}

impl SvgView {
    pub const ALL: [Self; 2] = [Self::Code, Self::Preview];

    pub fn label(self) -> &'static str {
        match self {
            Self::Code => "Code",
            Self::Preview => "Preview",
        }
    }
}

/// What a swipe or onion picture is made for: mode, mix percent, divider color.
type Blend = (Compare, u8, [u8; 3]);

/// Both sides of a changed image, `[old, new]`; `None` where that side has no file.
pub struct Preview {
    pub sides: [Option<Picture>; 2],
    /// `Image` or `SVG`, for the caption.
    kind: &'static str,
    /// The pane size in pixels the pictures were made for, and whether any came out smaller
    /// than it could be; see [`Preview::blurry_at`].
    pub fitted: (u32, u32),
    shrunk: bool,
    /// The last swipe or onion picture and what it was made for: mode, mix, divider color.
    blend: Option<(Blend, StatefulProtocol)>,
}

/// One side's size, plus a drawable copy when the terminal shows images.
pub struct Picture {
    pub width: u32,
    pub height: u32,
    pub bytes: usize,
    pub image: Option<StatefulProtocol>,
    /// The decoded picture, shrunk to the pane, for swipe and onion skin.
    pixels: Option<RgbaImage>,
}

impl fmt::Debug for Preview {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Preview")
            .field("sides", &self.sides)
            .field("kind", &self.kind)
            .field("fitted", &self.fitted)
            .field("shrunk", &self.shrunk)
            .field("blend", &self.blend.as_ref().map(|(key, _)| key))
            .finish()
    }
}

impl fmt::Debug for Picture {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Picture")
            .field("width", &self.width)
            .field("height", &self.height)
            .field("bytes", &self.bytes)
            .field("image", &self.image.is_some())
            .field("pixels", &self.pixels.as_ref().map(RgbaImage::dimensions))
            .finish()
    }
}

impl Preview {
    /// `None` unless `file` is binary and one side is an image, judged by its magic bytes.
    /// With a `picker` each side is decoded and shrunk to fit `max_px`.
    pub fn load(file: &FileDiff, picker: Option<&Picker>, max_px: (u32, u32)) -> Option<Self> {
        if !file.binary {
            return None;
        }
        let sides =
            [file.old.bytes(), file.new.bytes()].map(|bytes| picture(bytes, picker, max_px));
        let shrunk = (sides.iter().flatten())
            .any(|p| p.pixels.as_ref().is_some_and(|px| px.width() < p.width));
        Self::new(sides, "Image", (max_px, shrunk))
    }

    /// Both sides of an SVG or `.svgz` drawn to fit `max_px`, or sizes only without a `picker`;
    /// `<text>` needs `fonts`. `None` for a patch-only diff, or when a side doesn't parse.
    pub fn svg(
        file: &FileDiff,
        picker: Option<&Picker>,
        max_px: (u32, u32),
        fonts: Option<&Arc<Database>>,
    ) -> Option<Self> {
        if file.known.is_some() {
            return None;
        }
        let mut options = usvg::Options {
            // `<image href>` would read any path on disk; linked files are left blank.
            image_href_resolver: usvg::ImageHrefResolver {
                resolve_string: Box::new(|_, _| None),
                ..usvg::ImageHrefResolver::default()
            },
            ..usvg::Options::default()
        };
        if let Some(fonts) = fonts {
            options.fontdb = Arc::clone(fonts);
        }
        let [old, new] = [file.old.bytes(), file.new.bytes()];
        let sides = [
            svg_picture(old, picker, max_px, &options).ok()?,
            svg_picture(new, picker, max_px, &options).ok()?,
        ];
        // A vector picture can always be drawn sharper in a bigger pane.
        let shrunk = picker.is_some();
        Self::new(sides, "SVG", (max_px, shrunk))
    }

    fn new(
        sides: [Option<Picture>; 2],
        kind: &'static str,
        (fitted, shrunk): ((u32, u32), bool),
    ) -> Option<Self> {
        (sides.iter().any(Option::is_some)).then_some(Self {
            sides,
            kind,
            fitted,
            shrunk,
            blend: None,
        })
    }

    /// Whether a pane of `max_px` would show these pictures sharper if they were made again.
    /// Only growth counts: a smaller pane scales them down as well as a remake would.
    pub fn blurry_at(&self, (w, h): (u32, u32)) -> bool {
        self.shrunk && (w > self.fitted.0 || h > self.fitted.1)
    }

    /// Whether any side can be drawn as a picture.
    pub fn drawable(&self) -> bool {
        self.sides.iter().flatten().any(|p| p.image.is_some())
    }

    /// Whether both sides are pictures, so they can be swiped or faded.
    pub fn comparable(&self) -> bool {
        (self.sides.iter()).all(|side| side.as_ref().is_some_and(|p| p.pixels.is_some()))
    }

    /// The swipe or onion-skin picture at `mix` percent; `None` for 2-up or with one side.
    /// Encoded again only when `mode`, `mix`, or the `divider` color changes.
    pub fn blended(
        &mut self,
        picker: &Picker,
        mode: Compare,
        mix: u8,
        divider: [u8; 3],
    ) -> Option<&mut StatefulProtocol> {
        if mode == Compare::TwoUp {
            return None;
        }
        let [Some(old), Some(new)] = &self.sides else {
            return None;
        };
        let (old, new) = (old.pixels.as_ref()?, new.pixels.as_ref()?);
        let key = (mode, mix, divider);
        if self.blend.as_ref().is_none_or(|(made, _)| *made != key) {
            let image = compose(old, new, key);
            self.blend = Some((key, picker.new_resize_protocol(image.into())));
        }
        self.blend.as_mut().map(|(_, image)| image)
    }

    /// ` Image changed · 640×480 → 800×600 · 12 KB → 15 KB`, or `added`/`deleted` with one side.
    pub fn caption(&self) -> String {
        match &self.sides {
            [Some(old), Some(new)] => format!(
                "{} changed · {}×{} → {}×{} · {} → {}",
                self.kind,
                old.width,
                old.height,
                new.width,
                new.height,
                size(old.bytes),
                size(new.bytes)
            ),
            [None, Some(new)] => format!("{} added · {}", self.kind, new.label()),
            [Some(old), None] => format!("{} deleted · {}", self.kind, old.label()),
            [None, None] => String::new(),
        }
    }
}

impl Picture {
    /// `640×480 · 12 KB`.
    pub fn label(&self) -> String {
        format!("{}×{} · {}", self.width, self.height, size(self.bytes))
    }
}

/// The picture in `bytes`; `None` when empty or not an image.
fn picture(bytes: &[u8], picker: Option<&Picker>, (w, h): (u32, u32)) -> Option<Picture> {
    let reader = || {
        ImageReader::new(Cursor::new(bytes))
            .with_guessed_format()
            .ok()
    };
    let (width, height) = reader()?.into_dimensions().ok()?;
    let decoded = picker
        .filter(|_| bytes.len() <= MAX_DECODE)
        .and_then(|picker| {
            let image = reader()?.decode().ok()?;
            // ponytail: each side shrinks to the pane on its own, so a much larger side loses
            // scale against the other; shrink both by one factor if anyone compares such pairs.
            let image = if width > w || height > h {
                image.thumbnail(w, h)
            } else {
                image
            };
            Some(encode(picker, image.to_rgba8()))
        });
    let (image, pixels) = decoded.unzip();
    Some(Picture {
        width,
        height,
        bytes: bytes.len(),
        image,
        pixels,
    })
}

/// The SVG in `bytes`, gzipped or not; `None` when empty.
fn svg_picture(
    bytes: &[u8],
    picker: Option<&Picker>,
    max_px: (u32, u32),
    options: &usvg::Options,
) -> Result<Option<Picture>, usvg::Error> {
    if bytes.is_empty() {
        return Ok(None);
    }
    let text = if bytes.starts_with(&GZIP_MAGIC) {
        Cow::Owned(gunzip(bytes)?)
    } else {
        Cow::Borrowed(bytes)
    };
    let text = std::str::from_utf8(&text).map_err(|_| usvg::Error::NotAnUtf8Str)?;
    let tree = usvg::Tree::from_str(text, options)?;
    let size = tree.size().to_int_size();
    let decoded = picker
        .filter(|_| bytes.len() <= MAX_DECODE)
        .and_then(|picker| Some(encode(picker, rasterize(&tree, max_px)?)));
    let (image, pixels) = decoded.unzip();
    Ok(Some(Picture {
        width: size.width(),
        height: size.height(),
        bytes: bytes.len(),
        image,
        pixels,
    }))
}

/// The installed fonts, with `serif`, `sans-serif` and `monospace` on installed families.
pub fn system_fonts() -> Database {
    let mut fonts = Database::new();
    fonts.load_system_fonts();
    point_generics(&mut fonts);
    fonts
}

/// Points each generic family whose default is missing at an installed one.
fn point_generics(fonts: &mut Database) {
    type Set = fn(&mut Database, String);
    // fontdb defaults to Windows and macOS names, which Linux rarely has.
    let generics: [(Family, &[&str], Set); 3] = [
        (
            Family::Serif,
            &["DejaVu Serif", "Liberation Serif", "Noto Serif"],
            Database::set_serif_family::<String>,
        ),
        (
            Family::SansSerif,
            &["DejaVu Sans", "Liberation Sans", "Noto Sans"],
            Database::set_sans_serif_family::<String>,
        ),
        (
            Family::Monospace,
            &["DejaVu Sans Mono", "Liberation Mono", "Noto Sans Mono"],
            Database::set_monospace_family::<String>,
        ),
    ];
    for (generic, candidates, set) in generics {
        let installed =
            |name: &str| (fonts.faces()).any(|f| f.families.iter().any(|(n, _)| n == name));
        if installed(fonts.family_name(&generic)) {
            continue;
        }
        let pick = (candidates
            .iter()
            .find(|c| installed(c))
            .map(|c| (*c).to_owned()))
        .or_else(|| {
            fonts
                .faces()
                .find_map(|f| Some(f.families.first()?.0.clone()))
        });
        if let Some(name) = pick {
            set(fonts, name);
        }
    }
}

/// Whether drawing `file` needs fonts: it has `<text>`, or is an `.svgz` that can't be read.
pub fn needs_fonts(file: &FileDiff) -> bool {
    file.binary
        || [file.old.bytes(), file.new.bytes()]
            .iter()
            .any(|bytes| memchr::memmem::find(bytes, b"<text").is_some())
}

/// The unzipped `.svgz`; an error past [`MAX_DECODE`] bytes, so a gzip bomb stays small.
fn gunzip(bytes: &[u8]) -> Result<Vec<u8>, usvg::Error> {
    let mut out = Vec::new();
    let limit = u64::try_from(MAX_DECODE).expect("20 MiB fits u64") + 1;
    GzDecoder::new(bytes)
        .take(limit)
        .read_to_end(&mut out)
        .map_err(|_| usvg::Error::MalformedGZip)?;
    if out.len() > MAX_DECODE {
        return Err(usvg::Error::MalformedGZip);
    }
    Ok(out)
}

/// `tree` drawn as large as fits in `w`×`h` pixels; `None` when that's no pixels at all.
#[expect(
    clippy::cast_possible_truncation,
    clippy::cast_sign_loss,
    clippy::cast_precision_loss,
    reason = "pixel sizes, capped at the pane's"
)]
fn rasterize(tree: &usvg::Tree, (w, h): (u32, u32)) -> Option<RgbaImage> {
    let size = tree.size();
    let scale = (w as f32 / size.width()).min(h as f32 / size.height());
    let width = ((size.width() * scale).round() as u32).min(w);
    let height = ((size.height() * scale).round() as u32).min(h);
    let mut pixmap = tiny_skia::Pixmap::new(width, height)?;
    let transform = tiny_skia::Transform::from_scale(scale, scale);
    resvg::render(tree, transform, &mut pixmap.as_mut());
    let rgba = (pixmap.pixels().iter())
        .flat_map(|pixel| {
            let c = pixel.demultiply();
            [c.red(), c.green(), c.blue(), c.alpha()]
        })
        .collect();
    RgbaImage::from_raw(width, height, rgba)
}

/// A drawable copy of `pixels`, kept next to them for swipe and onion skin.
fn encode(picker: &Picker, pixels: RgbaImage) -> (StatefulProtocol, RgbaImage) {
    (picker.new_resize_protocol(pixels.clone().into()), pixels)
}

/// `old` and `new` centred on one canvas, then swiped or faded at `mix` percent.
fn compose(old: &RgbaImage, new: &RgbaImage, (mode, mix, divider): Blend) -> RgbaImage {
    let (width, height) = (old.width().max(new.width()), old.height().max(new.height()));
    let centred = |image: &RgbaImage| {
        let mut canvas = RgbaImage::new(width, height);
        let x = i64::from((width - image.width()) / 2);
        let y = i64::from((height - image.height()) / 2);
        imageops::replace(&mut canvas, image, x, y);
        canvas
    };
    let (mut out, new) = (centred(old), centred(new));
    let mix = u32::from(mix);
    match mode {
        Compare::TwoUp => {}
        Compare::Swipe => {
            let split = width * mix / 100;
            for (x, y, pixel) in out.enumerate_pixels_mut() {
                if (split..split + DIVIDER_PX).contains(&x) {
                    *pixel = image::Rgba([divider[0], divider[1], divider[2], 255]);
                } else if x >= split {
                    *pixel = *new.get_pixel(x, y);
                }
            }
        }
        Compare::Onion => {
            for (pixel, top) in out.pixels_mut().zip(new.pixels()) {
                for (a, b) in pixel.0.iter_mut().zip(top.0) {
                    let (a32, b32) = (u32::from(*a), u32::from(b));
                    let mixed = (a32 * (100 - mix) + b32 * mix) / 100;
                    *a = u8::try_from(mixed).expect("a weighted mean of two bytes is a byte");
                }
            }
        }
    }
    out
}

/// `512 B`, `1.5 KB`, `2.0 MB`.
#[expect(
    clippy::cast_precision_loss,
    reason = "one decimal place of a file size"
)]
fn size(bytes: usize) -> String {
    match bytes {
        0..1024 => format!("{bytes} B"),
        1024..0x10_0000 => format!("{:.1} KB", bytes as f64 / 1024.0),
        _ => format!("{:.1} MB", bytes as f64 / f64::from(0x10_0000)),
    }
}

/// A 4×2 red and an 8×8 blue SVG.
#[cfg(test)]
pub(crate) const SVGS: [&str; 2] = [
    r##"<svg xmlns="http://www.w3.org/2000/svg" width="4" height="2"><rect width="4" height="2" fill="#f00"/></svg>"##,
    r##"<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 8 8"><rect width="8" height="8" fill="#00f"/></svg>"##,
];

/// `bytes` gzipped, as in an `.svgz`.
#[cfg(test)]
pub(crate) fn gzip(bytes: &[u8]) -> Vec<u8> {
    use std::io::Write as _;
    let mut out = flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::fast());
    out.write_all(bytes).expect("in memory");
    out.finish().expect("in memory")
}

#[cfg(test)]
pub(crate) fn png(width: u32, height: u32) -> Vec<u8> {
    let mut bytes = Vec::new();
    image::DynamicImage::ImageRgb8(image::RgbImage::new(width, height))
        .write_to(&mut Cursor::new(&mut bytes), image::ImageFormat::Png)
        .expect("encodes");
    bytes
}

#[cfg(test)]
mod tests {
    use super::*;

    const ANY: (u32, u32) = (1024, 1024);

    #[test]
    fn sizes_come_from_the_header_and_decoding_needs_a_picker() {
        let (old, new) = (png(3, 2), png(4, 4));
        let len = new.len();
        let file = FileDiff::new(old, new);
        let preview = Preview::load(&file, None, ANY).expect("an image");
        let [Some(a), Some(b)] = &preview.sides else {
            panic!("both sides: {preview:?}")
        };
        assert_eq!((a.width, a.height, b.width, b.height), (3, 2, 4, 4));
        assert_eq!(b.bytes, len);
        assert!(!preview.drawable());

        let preview = Preview::load(&file, Some(&Picker::halfblocks()), ANY).expect("an image");
        assert!(preview.drawable(), "decoded with a picker");
    }

    #[test]
    fn only_binary_images_have_a_preview() {
        let junk = FileDiff::new(b"\0\0\0junk".to_vec(), b"\0more".to_vec());
        assert!(Preview::load(&junk, None, ANY).is_none());
        let text = FileDiff::new(b"a\n".to_vec(), b"b\n".to_vec());
        assert!(Preview::load(&text, None, ANY).is_none());
    }

    #[test]
    fn captions_name_the_change() {
        let caption = |old: Vec<u8>, new: Vec<u8>| {
            Preview::load(&FileDiff::new(old, new), None, ANY)
                .expect("an image")
                .caption()
        };
        let (a, b) = (png(3, 2), png(4, 4));
        let (sa, sb) = (size(a.len()), size(b.len()));
        assert_eq!(
            caption(a.clone(), b.clone()),
            format!("Image changed · 3×2 → 4×4 · {sa} → {sb}")
        );
        assert_eq!(caption(Vec::new(), b), format!("Image added · 4×4 · {sb}"));
        assert_eq!(
            caption(a, Vec::new()),
            format!("Image deleted · 3×2 · {sa}")
        );
        assert_eq!(
            (size(512), size(1536), size(2 << 20)),
            ("512 B".into(), "1.5 KB".into(), "2.0 MB".into())
        );
    }

    fn solid(width: u32, height: u32, rgba: [u8; 4]) -> RgbaImage {
        RgbaImage::from_pixel(width, height, image::Rgba(rgba))
    }

    const RED: [u8; 4] = [255, 0, 0, 255];
    const BLUE: [u8; 4] = [0, 0, 255, 255];
    const LINE: [u8; 3] = [9, 9, 9];

    #[test]
    fn swipe_shows_old_left_of_the_divider_and_new_right_of_it() {
        let row = |mix| {
            let out = compose(
                &solid(10, 1, RED),
                &solid(10, 1, BLUE),
                (Compare::Swipe, mix, LINE),
            );
            (0..10).map(|x| out.get_pixel(x, 0).0).collect::<Vec<_>>()
        };
        let divider = [9, 9, 9, 255];
        let run = |parts: &[([u8; 4], usize)]| -> Vec<[u8; 4]> {
            (parts.iter())
                .flat_map(|&(pixel, n)| [pixel].repeat(n))
                .collect()
        };
        assert_eq!(row(50), run(&[(RED, 5), (divider, 2), (BLUE, 3)]));
        assert_eq!(row(0), run(&[(divider, 2), (BLUE, 8)]));
        assert_eq!(row(100), run(&[(RED, 10)]), "the divider is past the edge");
    }

    #[test]
    fn onion_fades_new_over_old_and_centres_smaller_sides() {
        let pixel = |old: &RgbaImage, new: &RgbaImage, mix, (x, y)| {
            compose(old, new, (Compare::Onion, mix, LINE))
                .get_pixel(x, y)
                .0
        };
        let (old, new) = (solid(1, 1, RED), solid(1, 1, BLUE));
        assert_eq!(pixel(&old, &new, 0, (0, 0)), RED);
        assert_eq!(pixel(&old, &new, 50, (0, 0)), [127, 0, 127, 255]);
        assert_eq!(pixel(&old, &new, 100, (0, 0)), BLUE);

        let big = solid(3, 3, RED);
        assert_eq!(pixel(&big, &new, 100, (1, 1)), BLUE, "centred");
        assert_eq!(
            pixel(&big, &new, 100, (0, 0)),
            [0; 4],
            "transparent around it"
        );
        assert_eq!(pixel(&big, &new, 0, (0, 0)), RED);
    }

    #[test]
    fn blending_needs_two_pictures_and_is_kept_until_the_mix_changes() {
        let picker = Picker::halfblocks();
        let file = FileDiff::new(png(3, 2), png(4, 4));
        let mut preview = Preview::load(&file, Some(&picker), ANY).expect("an image");
        assert!(preview.comparable());
        assert!(preview.blended(&picker, Compare::TwoUp, 50, LINE).is_none());
        assert!(preview.blended(&picker, Compare::Swipe, 50, LINE).is_some());
        let key = |p: &Preview| p.blend.as_ref().map(|(key, _)| *key);
        assert_eq!(key(&preview), Some((Compare::Swipe, 50, LINE)));
        preview.blended(&picker, Compare::Onion, 70, LINE);
        assert_eq!(key(&preview), Some((Compare::Onion, 70, LINE)));

        let added = FileDiff::new(Vec::new(), png(4, 4));
        let mut preview = Preview::load(&added, Some(&picker), ANY).expect("an image");
        assert!(!preview.comparable());
        assert!(preview.blended(&picker, Compare::Swipe, 50, LINE).is_none());
        assert!(
            !Preview::load(&file, None, ANY)
                .expect("an image")
                .comparable(),
            "sizes only"
        );
    }

    #[test]
    fn svgs_report_their_size_and_render_scaled_to_fit() {
        let [red, blue] = SVGS.map(|svg| svg.as_bytes().to_vec());
        let file = FileDiff::new(red.clone(), blue);
        let preview = Preview::svg(&file, None, ANY, None).expect("an SVG");
        assert!(preview.caption().starts_with("SVG changed · 4×2 → 8×8 · "));
        assert!(!preview.drawable(), "sizes only without a picker");

        let preview =
            Preview::svg(&file, Some(&Picker::halfblocks()), (8, 8), None).expect("an SVG");
        let pixels = (preview.sides[0].as_ref()).and_then(|p| p.pixels.as_ref());
        let pixels = pixels.expect("rendered");
        assert_eq!(pixels.dimensions(), (8, 4), "scaled up to fit 8×8");
        assert_eq!(pixels.get_pixel(0, 0).0, RED);

        let added = FileDiff::new(Vec::new(), red);
        let caption = Preview::svg(&added, None, ANY, None)
            .expect("an SVG")
            .caption();
        assert!(caption.starts_with("SVG added · 4×2 · "), "{caption}");
    }

    #[test]
    fn broken_binary_and_patch_only_svgs_have_no_preview() {
        let broken = FileDiff::new(SVGS[0].into(), b"<svg width=".to_vec());
        assert!(
            Preview::svg(&broken, None, ANY, None).is_none(),
            "a side that doesn't parse"
        );
        let binary = FileDiff::new(png(1, 1), png(2, 2));
        assert!(Preview::svg(&binary, None, ANY, None).is_none());

        let patch = format!(
            "diff --git a/i.svg b/i.svg\n--- a/i.svg\n+++ b/i.svg\n@@ -1 +1 @@\n-{}\n+{}\n",
            SVGS[0], SVGS[1]
        );
        let patch = zdiff_core::Patch::parse(patch.into_bytes()).expect("parses");
        let partial = patch.full_diff(0, None);
        assert!(partial.known.is_some());
        assert!(
            Preview::svg(&partial, None, ANY, None).is_none(),
            "no whole file to draw"
        );
    }

    #[test]
    fn linked_images_are_left_blank_not_read() {
        let svg = r#"<svg xmlns="http://www.w3.org/2000/svg" width="2" height="2"><image href="/etc/hosts" width="2" height="2"/></svg>"#;
        let file = FileDiff::new(Vec::new(), svg.into());
        let preview = Preview::svg(&file, Some(&Picker::halfblocks()), ANY, None).expect("parses");
        let pixels = (preview.sides[1].as_ref()).and_then(|p| p.pixels.as_ref());
        assert_eq!(pixels.expect("rendered").get_pixel(0, 0).0, [0; 4], "blank");
    }

    #[test]
    fn text_renders_once_fonts_are_given() {
        let svg = r#"<svg xmlns="http://www.w3.org/2000/svg" width="40" height="20"><text x="0" y="16" font-family="sans-serif" font-size="16">Hi</text></svg>"#;
        let file = FileDiff::new(Vec::new(), svg.into());
        assert!(needs_fonts(&file));
        assert!(!needs_fonts(&FileDiff::new(Vec::new(), SVGS[0].into())));
        let picker = Picker::halfblocks();
        let inked = |fonts: Option<&Arc<Database>>| {
            let preview = Preview::svg(&file, Some(&picker), (40, 20), fonts).expect("parses");
            let pixels = (preview.sides[1].as_ref()).and_then(|p| p.pixels.as_ref());
            pixels.expect("rendered").pixels().any(|px| px.0[3] > 0)
        };
        assert!(!inked(None), "no fonts, no text");
        let mut fonts = Database::new();
        fonts.load_system_fonts();
        if fonts.is_empty() {
            eprintln!("no system fonts here; text rendering not checked");
            return;
        }
        // Without fontdb's default families, as on most Linux systems.
        let defaults = ["Arial", "Times New Roman", "Courier New"];
        let ids: Vec<_> = (fonts.faces())
            .filter(|f| {
                f.families
                    .iter()
                    .any(|(n, _)| defaults.contains(&n.as_str()))
            })
            .map(|f| f.id)
            .collect();
        for id in ids {
            fonts.remove_face(id);
        }
        point_generics(&mut fonts);
        assert!(inked(Some(&Arc::new(fonts))));
    }

    #[test]
    fn only_shrunk_pictures_are_blurry_and_only_in_a_bigger_pane() {
        let picker = Picker::halfblocks();
        let svg = FileDiff::new(SVGS[0].into(), SVGS[1].into());
        let svg = Preview::svg(&svg, Some(&picker), (8, 8), None).expect("an SVG");
        assert!(svg.blurry_at((16, 8)));
        assert!(!svg.blurry_at((4, 4)), "a smaller pane scales down fine");

        let small = FileDiff::new(png(3, 2), png(3, 2));
        let small = Preview::load(&small, Some(&picker), (100, 100)).expect("an image");
        assert!(!small.blurry_at((4096, 4096)), "shown at full size already");
        let big = FileDiff::new(png(400, 300), png(400, 300));
        let big = Preview::load(&big, Some(&picker), (100, 100)).expect("an image");
        assert!(big.blurry_at((200, 200)) && !big.blurry_at((50, 50)));
        let sizes = Preview::svg(
            &FileDiff::new(SVGS[0].into(), SVGS[1].into()),
            None,
            ANY,
            None,
        );
        assert!(
            !sizes.expect("an SVG").blurry_at((4096, 4096)),
            "no pictures to sharpen"
        );
    }

    #[test]
    fn svgz_is_unzipped_up_to_the_decode_limit() {
        let zipped = gzip(SVGS[0].as_bytes());
        let file = FileDiff::new(zipped.clone(), zipped);
        assert!(file.binary, "git sees an .svgz as binary");
        let preview = Preview::svg(&file, Some(&Picker::halfblocks()), (8, 8), None);
        let preview = preview.expect("an .svgz");
        assert!(preview.caption().starts_with("SVG changed · 4×2 → 4×2"));
        let pixels = (preview.sides[0].as_ref()).and_then(|p| p.pixels.as_ref());
        assert_eq!(pixels.expect("rendered").get_pixel(0, 0).0, RED);

        let bomb = gzip(&vec![b' '; MAX_DECODE + 1]);
        let bomb = FileDiff::new(Vec::new(), bomb);
        assert!(
            Preview::svg(&bomb, None, ANY, None).is_none(),
            "past the limit"
        );
    }
}
