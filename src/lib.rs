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
    /// Glyph to be used to render any codepoint missing from the glyph storage.
    ///
    /// This may alias glyph storage.
    pub replacement: &'g Glyph,
    /// Bitmap storage for all glyphs. Individual glyphs reference ranges in
    /// this slice.
    pub bitmaps: &'i [u8],
    /// Kerning table for adjusting glyph-to-glyph spacing.
    pub kerning: KerningTable<'k>,
}

impl Font<'_, '_, '_> {
    /// Computes the width, in pixels, of the char `c` rendered in this font.
    /// Note that this ignores kerning, so looping over the chars in a string
    /// will not get you the correct result (see `width`).
    pub fn char_width(&self, c: char) -> usize {
        let glyph = self.glyph_storage.get(c).unwrap_or(&self.replacement);
        usize::from(glyph.advance)
    }

    pub fn width(&self, s: &str) -> usize {
        // To lookup codepoint pairs in the kerning table, we'll keep track of
        // the previous character here:
        let mut prev_c = None;

        let mut x = 0_usize;

        for c in s.chars() {
            // Record this character as the new previous character. If there was
            // already a previous character, look for a kerning table entry.
            if let Some(pc) = prev_c.replace(c) {
                if let Some(kte) = self.kerning.get(pc, c) {
                    // We have an entry. Add its adjustment gingerly to avoid
                    // overflow.
                    x = if kte.adjust < 0 {
                        x.saturating_sub(usize::from((-kte.adjust) as u8))
                    } else {
                        x.saturating_add(usize::from(kte.adjust as u8))
                    };
                }
            }

            // Add the default advance of either the specific glyph for this
            // character, or the replacement glyph.
            let glyph = self.glyph_storage.get(c).unwrap_or(&self.replacement);
            x = x.saturating_add(usize::from(glyph.advance));
        }
        x
    }

    pub fn render<T>(
        &self,
        string: &str,
        x: u32,
        y: u32,
        target: &mut T,
        fg: T::Pixel,
    )
        where T: RenderTarget,
    {
        let mut pen_x = x;
        let mut last_c = None;
        for c in string.chars() {
            if let Some(prev) = last_c.replace(c) {
                if let Some(entry) = self.kerning.get(prev, c) {
                    if entry.adjust < 0 {
                        pen_x = pen_x.saturating_sub((-entry.adjust) as u32);
                    } else {
                        pen_x = pen_x.saturating_add(entry.adjust as u32);
                    }
                }
            }

            let glyph = self.glyph_storage.get(c).unwrap_or(self.replacement);
            let gx = pen_x + u32::from(glyph.origin.0);
            let gy = y + u32::from(glyph.origin.1);

            let data_off = usize::from(glyph.image_offset);
            let height = usize::from(glyph.image_height);
            let row_bytes = usize::from(glyph.row_bytes);
            let data_len = row_bytes * height;

            if data_len != 0 {
                let slice = &self.bitmaps[data_off..data_off + data_len];
                for (y, data) in (gy..gy + height as u32).zip(slice.chunks(row_bytes)) {
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
            }

            pen_x += u32::from(glyph.advance);
        }
    }
    pub fn render_direct<T>(
        &self,
        string: &str,
        x: u32,
        y: u32,
        target: &mut T,
        fg: T::Pixel,
    )
        where T: DirectRenderTarget,
    {
        let mut pen_x = x;
        let mut last_c = None;
        for c in string.chars() {
            if let Some(prev) = last_c.replace(c) {
                if let Some(entry) = self.kerning.get(prev, c) {
                    if entry.adjust < 0 {
                        pen_x = pen_x.saturating_sub((-entry.adjust) as u32);
                    } else {
                        pen_x = pen_x.saturating_add(entry.adjust as u32);
                    }
                }
            }

            let glyph = self.glyph_storage.get(c).unwrap_or(self.replacement);
            let gx = pen_x + u32::from(glyph.origin.0);
            let gy = y + u32::from(glyph.origin.1);

            let data_off = usize::from(glyph.image_offset);
            let height = usize::from(glyph.image_height);
            let row_bytes = usize::from(glyph.row_bytes);
            let data_len = row_bytes * height;

            if data_len != 0 {
                let slice = &self.bitmaps[data_off..data_off + data_len];
                for (y, data) in (gy..gy + height as u32).zip(slice.chunks(row_bytes)) {
                    let dest =
                        target.subrow_mut(y, gx..gx + row_bytes as u32 * 8);
                    let mut data = data.iter().cloned();
                    let mut byte = 0;
                    let mut bits_left = 0_u32;
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
            }

            pen_x += u32::from(glyph.advance);
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
    pub fn get(&self, c: char) -> Option<&Glyph> {
        match self {
            Self::Dense { first, glyphs } => {
                let i = u32::from(c).wrapping_sub(u32::from(*first)) as usize;
                glyphs.get(i)
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

pub trait RenderTarget {
    type Pixel: Copy + 'static;

    fn put_pixel_slow(&mut self, x: u32, y: u32, pixel: Self::Pixel);
}

#[cfg(feature = "std")]
impl<P, C> RenderTarget for image::ImageBuffer<P, C>
    where P: Copy + image::Pixel + 'static,
          C: core::ops::Deref<Target = [P::Subpixel]> + core::ops::DerefMut,
{
    type Pixel = P;
    fn put_pixel_slow(&mut self, x: u32, y: u32, pixel: P) {
        if x < self.width() && y < self.height() {
            self.put_pixel(x, y, pixel);
        }
    }
}

pub trait DirectRenderTarget {
    type Pixel: Copy + 'static;

    fn subrow_mut(&mut self, y: u32, x: core::ops::Range<u32>) -> &mut [Self::Pixel];
}

#[cfg(feature = "std")]
impl<P, C> DirectRenderTarget for image::ImageBuffer<image::Luma<P>, C>
    where P: Copy + image::Primitive + 'static,
          C: core::ops::Deref<Target = [P]> + core::ops::DerefMut + AsMut<[P]>,
{
    type Pixel = image::Luma<P>;

    fn subrow_mut(&mut self, y: u32, x: core::ops::Range<u32>) -> &mut [Self::Pixel] {
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

#[cfg(test)]
mod tests {
    #[test]
    fn it_works() {
        let result = 2 + 2;
        assert_eq!(result, 4);
    }
}
