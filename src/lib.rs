#![cfg_attr(not(feature = "std"), no_std)]

#[cfg(feature = "std")]
pub mod gen;
#[cfg(feature = "std")]
pub mod load;

/// In-memory representation of a font, which is a typeface realized at a
/// particular size, weight, and other parameters.
#[derive(Copy, Clone, Debug)]
pub struct Font<'g, 'i, 'k> {
    /// Displacement from the top of the bounding box to the baseline, in
    /// pixels.
    pub ascent: u8,
    /// Displacement from the baseline to the bottom of the bounding box, in
    /// pixels.
    pub descent: u8,
    /// Displacement from the bounding box of one line to the bounding box of
    /// the next, in pixels. Ordinarily this should be at least `ascent +
    /// descent`.
    pub line_spacing: u8,
    /// Glyphs in the font.
    pub glyph_storage: GlyphStorage<'g>,
    /// Index of glyph to use as a replacement for rendering characters whose
    /// glyphs are missing in this font.
    ///
    /// Note that glyph indexes are glyph-storage specific.
    pub replacement: u8,
    /// Bitmap storage for all glyphs. Individual glyphs reference ranges in
    /// this slice.
    pub bitmaps: &'i [u8],
    /// Kerning table for adjusting glyph-to-glyph spacing.
    pub kerning: KerningTable<'k>,
}

impl<'k> Font<'_, '_, 'k> {
    /// Given the Y coordinate of the desired text baseline, this computes the Y
    /// of the top of its bounding box, for use with `render`.
    ///
    /// This returns an `Option` because we can't currently represent negative Y
    /// coordinates, so if `baseline` is less than `font.ascent`, there's no
    /// result.
    pub fn baseline_to_y(&self, baseline: usize) -> Option<usize> {
        baseline.checked_sub(usize::from(self.ascent))
    }

    /// Returns the spacing between lines of text in this font, as a `usize` for
    /// convenience.
    pub fn line_spacing_usize(&self) -> usize {
        usize::from(self.line_spacing)
    }

    /// Looks up the glyph for `c`, or the replacement glyph if `c` is not
    /// present in this font.
    pub fn get_glyph_or_replacement(&self, c: char) -> &Glyph {
        self.glyph_storage.get(c).unwrap_or_else(|| {
            self.glyph_storage.get_by_index(usize::from(self.replacement))
                .unwrap()
        })
    }

    /// Computes the width, in pixels, of the char `c` rendered in this font.
    /// Note that this ignores kerning, so looping over the chars in a string
    /// will not get you the correct result (see `width`).
    pub fn char_width(&self, c: char) -> usize {
        usize::from(self.get_glyph_or_replacement(c).advance)
    }

    /// Computes the width, in pixels, of the string `s` rendered in this font.
    /// This handles kerning but not line breaks; newlines will be rendered
    /// using whatever glyph is given in the font for `'\n'`.
    ///
    /// This happens to be exactly the same logic used by `render`, so you can
    /// use `width` to work out the dimensions needed for `render`.
    pub fn width(&self, s: &str) -> usize {
        let mut x = 0_usize;
        let mut kerning = self.start_kerning();

        for c in s.chars() {
            kerning.adjust_usize_for_char(c, &mut x);

            // Add the default advance; if kerning applies we'll handle it next
            // iteration.
            x = x.saturating_add(self.char_width(c));
        }
        x
    }

    /// Renders text on a single line.
    ///
    /// The text in `string` will be drawn with its _upper left_ coordinate at
    /// position `(x, y)` in `target`. Pixels that are set in the font will be
    /// filled in with color `fg`. Other pixels will be left undisturbed, so the
    /// text's background will appear transparent.
    ///
    /// This handles kerning but not line breaks. Newlines in `string` will be
    /// drawn with whatever glyph the font specifies for `'\n'`.
    ///
    /// Note that the `y` coordinate given to this function is the top of the
    /// bounding box, _not_ the baseline. Use `baseline_to_y` to compute the
    /// bounding box coordinate corresponding to a given baseline coordinate.
    ///
    /// This implementation tends to be a little more expensive than
    /// `render_direct`, so if your render target can be made to implement
    /// `DirectRenderTarget`, consider using that instead.
    pub fn render<T>(
        &self,
        string: &str,
        x: usize,
        y: usize,
        target: &mut T,
        fg: T::Pixel,
    )
        where T: RenderTarget,
    {
        self.render_core(string, x, y, |gx, gy, glyph, slice| {
            let height = usize::from(glyph.image_height);
            let row_bytes = glyph.row_bytes_usize();

            for (y, data) in (gy..gy + height).zip(slice.chunks(row_bytes)) {
                let mut x = gx;
                for byte in data {
                    let mut byte = *byte;
                    for _ in 0..8 {
                        if byte & 0x80 != 0 {
                            target.put_pixel_slow(x, y, fg);
                        }
                        byte <<= 1;
                        x += 1;
                    }
                }
            }
        });
    }

    /// Renders text on a single line, slightly faster.
    ///
    /// This behaves just like `render` but uses the `DirectRenderTarget` API,
    /// which lets us make more assumptions about memory layout and winds up
    /// being slightly cheaper.
    ///
    /// See `render` for more details.
    pub fn render_direct<T>(
        &self,
        string: &str,
        x: usize,
        y: usize,
        target: &mut T,
        fg: T::Pixel,
    )
        where T: DirectRenderTarget,
    {
        self.render_core(string, x, y, |gx, gy, glyph, slice| {
            let height = usize::from(glyph.image_height);
            let row_bytes = glyph.row_bytes_usize();

            for (y, data) in (gy..gy + height).zip(slice.chunks(row_bytes)) {
                let dest =
                    target.subrow_mut(y, gx..gx + row_bytes * 8);
                let mut data = data.iter().cloned();
                let mut byte = 0;
                let mut bits_left = 0_usize;
                for pel in dest {
                    if let Some(n) = bits_left.checked_sub(1) {
                        bits_left = n;
                    } else if let Some(b) = data.next() {
                        byte = b;
                        bits_left = 7;
                    } else {
                        break;
                    }

                    if byte & 0x80 != 0 {
                        *pel = fg;
                    }
                    byte <<= 1;
                }
            }
        });
    }

    /// Implementation factor of both `render` and `render_direct`, exposed here
    /// in case you're doing something unexpected.
    ///
    /// This will call `action` with the X, Y coordinates for each non-empty
    /// glyph rendered from `string`, starting at the given location, as well as
    /// the font's `Glyph` for the character and the actual slice of bitmap
    /// data.
    pub fn render_core(
        &self,
        string: &str,
        x: usize,
        y: usize,
        mut action: impl FnMut(usize, usize, &Glyph, &[u8]),
    ) {
        let mut pen_x = x;
        let mut kerning = self.start_kerning();
        for c in string.chars() {
            kerning.adjust_usize_for_char(c, &mut pen_x);

            let glyph = self.get_glyph_or_replacement(c);

            if glyph.has_image() {
                let (gx, gy) = glyph.displace_usize(pen_x, y);
                action(
                    gx,
                    gy,
                    glyph,
                    glyph.slice_bitmap(&self.bitmaps),
                );
            }

            pen_x += glyph.default_advance_usize();
        }
    }

    /// Returns a `KerningState` ready to being kerning characters. This is
    /// appropriate for use at the beginning of a line.
    pub fn start_kerning(&self) -> KerningState<'k> {
        KerningState {
            table: self.kerning,
            last_char: None,
        }
    }
}

/// Storage for the set of glyphs that make up a font.
#[derive(Copy, Clone, Debug)]
pub enum GlyphStorage<'g> {
    /// The font provides a set of glyphs for a contiguous range of characters
    /// in ISO8859-1.
    Dense {
        /// Code point for `glyphs[0]`. Having a `first` value greater than zero
        /// allows fonts to avoid encoding empty glyphs for the ASCII control
        /// characters.
        first: u8,
        /// Glyph data for a consecutive sequence of codepoints starting at
        /// `first`.
        ///
        /// In practice, this should be no longer than `256 - first` entries.
        glyphs: &'g [Glyph],
    },
}

impl GlyphStorage<'_> {
    /// Looks up a `char` in glyph storage. Returns `None` if the `char` is not
    /// explicitly represented in storage.
    pub fn get(&self, c: char) -> Option<&Glyph> {
        match self {
            Self::Dense { first, glyphs } => {
                let i = u32::from(c).wrapping_sub(u32::from(*first)) as usize;
                glyphs.get(i)
            },
        }
    }

    /// Looks up a glyph by glyph _index,_ which is mostly only used during
    /// replacement glyph processing, but maybe you've got ideas.
    pub fn get_by_index(&self, index: usize) -> Option<&Glyph> {
        match self {
            Self::Dense { glyphs, .. } => {
                glyphs.get(index)
            },
        }
    }
}

/// Data for a single glyph in a font.
#[derive(Copy, Clone, Debug, Eq, PartialEq)]
pub struct Glyph {
    /// Width of each pixel row in `image`, measured in bytes, or units of 8
    /// pixels.
    pub row_bytes: u8,
    /// Image data slice position in the font's image data array.
    pub image_offset: u16,
    /// Number of rows of pixels in this glyph's image.
    pub image_height: u8,
    /// Displacement in (X, Y) from the top-left of the glyph's bounding box to
    /// the top-left pixel in `image`. This allows an image to be smaller than
    /// the bounding box, e.g. omitting data above a lowercase letter.
    pub origin: (u8, u8),
    /// Default X advance from the left side of this glyph's bounding box to the
    /// left side of the next glyph. This can be overridden by kerning
    /// information.
    pub advance: u8,
}

impl Glyph {
    /// Checks whether this glyph has an image, i.e. is not blank.
    pub fn has_image(&self) -> bool {
        self.row_bytes != 0
    }

    /// Adds this glyph's `origin` to the given X/Y coordinates to produce the
    /// coordinates of the top left of the glyph's rendered area.
    pub fn displace_usize(&self, x: usize, y: usize) -> (usize, usize) {
        (x + usize::from(self.origin.0), y + usize::from(self.origin.1))
    }

    /// Computes the width in pixels of this glyph's rendered area.
    pub fn width_in_pixels(&self) -> usize {
        self.row_bytes_usize() * 8
    }

    /// Slices this glyph's bitmap out of a shared bitmap slice.
    ///
    /// # Panics
    ///
    /// If this glyph's offset and size wind up being out of range for `bitmap`,
    /// which probably means you're attempting to use a `Glyph` from one font
    /// with a bitmap array from another, or something.
    pub fn slice_bitmap<'b>(&self, bitmap: &'b [u8]) -> &'b [u8] {
        let data_off = usize::from(self.image_offset);
        let height = usize::from(self.image_height);
        let data_len = self.row_bytes_usize() * height;
        &bitmap[data_off..data_off + data_len]
    }

    /// Returns the default horizontal advance for glyphs in this font as a
    /// `usize`.
    pub fn default_advance_usize(&self) -> usize {
        usize::from(self.advance)
    }

    /// Returns the number of bitmap bytes in a row of this glyph's image, as a
    /// `usize`.
    pub fn row_bytes_usize(&self) -> usize {
        usize::from(self.row_bytes)
    }
}

/// A kerning table.
#[derive(Copy, Clone, Debug, Default, Ord, PartialOrd, Eq, PartialEq)]
pub struct KerningTable<'k> {
    pub entries: &'k [KerningEntry],
}

impl KerningTable<'_> {
    pub fn get(&self, before: char, after: char) -> Option<&KerningEntry> {
        // Due to the limited size of the entry, we definitely don't have any
        // entries for chars outside of ISO8859-1.
        let before = u8::try_from(before).ok()?;
        let after = u8::try_from(after).ok()?;

        self.entries.binary_search_by_key(&(before, after), |e| e.pair)
            .ok()
            .map(|i| &self.entries[i])
    }
}

/// An entry in the kerning table.
#[derive(Copy, Clone, Debug, Default, Ord, PartialOrd, Eq, PartialEq)]
pub struct KerningEntry {
    /// Sequence of characters that cause this entry to apply. Characters here
    /// are given by the bottom 8 bits of their codepoint, limiting this to
    /// ISO8859-1.
    pub pair: (u8, u8),
    /// Adjustment to the advance between the two characters given in `pair`.
    /// Negative values bring the glyphs closer together, positive values move
    /// them farther apart.
    pub adjust: i8,
}

impl KerningEntry {
    /// Apply the tracking adjustment from this kerning table entry to a
    /// position represented as a `usize` using saturating arithmetic.
    #[must_use = "this doesn't adjust in-place"]
    pub fn adjust_usize(&self, val: usize) -> usize {
        if self.adjust < 0 {
            val.saturating_sub(usize::from((-self.adjust) as u8))
        } else {
            val.saturating_add(usize::from(self.adjust as u8))
        }
    }
}

pub struct KerningState<'k> {
    table: KerningTable<'k>,
    last_char: Option<char>,
}

impl KerningState<'_> {
    pub fn adjust_usize_for_char(&mut self, c: char, x: &mut usize) {
        if let Some(prev) = self.last_char.replace(c) {
            if let Some(entry) = self.table.get(prev, c) {
                *x = entry.adjust_usize(*x);
            }
        }
    }
}

pub trait RenderTarget {
    type Pixel: Copy + 'static;

    fn put_pixel_slow(&mut self, x: usize, y: usize, pixel: Self::Pixel);
}

#[cfg(feature = "std")]
impl<P, C> RenderTarget for image::ImageBuffer<P, C>
    where P: Copy + image::Pixel + 'static,
          C: core::ops::Deref<Target = [P::Subpixel]> + core::ops::DerefMut,
{
    type Pixel = P;
    fn put_pixel_slow(&mut self, x: usize, y: usize, pixel: P) {
        let x = u32::try_from(x).unwrap();
        let y = u32::try_from(y).unwrap();
        if x < self.width() && y < self.height() {
            self.put_pixel(x, y, pixel);
        }
    }
}

pub trait DirectRenderTarget {
    type Pixel: Copy + 'static;

    fn subrow_mut(&mut self, y: usize, x: core::ops::Range<usize>) -> &mut [Self::Pixel];
}

#[cfg(feature = "std")]
impl<P, C> DirectRenderTarget for image::ImageBuffer<image::Luma<P>, C>
    where P: Copy + image::Primitive + 'static,
          C: core::ops::Deref<Target = [P]> + core::ops::DerefMut + AsMut<[P]>,
{
    type Pixel = image::Luma<P>;

    fn subrow_mut(&mut self, y: usize, x: core::ops::Range<usize>) -> &mut [Self::Pixel] {
        let flat = self.as_flat_samples_mut();
        let row_i = y as usize * flat.layout.width as usize;
        let row = &mut flat.samples[row_i..row_i + flat.layout.width as usize];

        let x_start = usize::min(x.start as usize, flat.layout.width as usize);
        let x_end = usize::min(x.end as usize, flat.layout.width as usize);
        let subpixels = &mut row[x_start..x_end];
        // The reason this is only defined for Luma is so that I know it's a
        // single-channel image, and I can do this:
        unsafe {
            &mut *(subpixels as *mut [P] as *mut [image::Luma<P>])
        }
    }
}
