use crate::*;
use std::io::{BufRead, Seek};
use image::Rgb;

pub fn load_font_from_png<R>(
    png: impl BufRead + Seek,
    first: Option<u8>,
    body: impl FnOnce(&Font<'_, '_, '_>) -> Result<R, Box<dyn std::error::Error>>,
) -> Result<R, Box<dyn std::error::Error>> {
    let img = image::io::Reader::new(png)
        .with_guessed_format()?
        .decode()?
        .to_rgb8();

    // Scan the left margin to find band boundaries.
    let mut last_y = 0;
    let mut bands = vec![];
    for y in 0..img.height() {
        if *img.get_pixel(0, y) == Rgb([0xFF, 0, 0]) {
            // See how wide the red strip is.
            let band_width = (1..img.width()).map(|x| *img.get_pixel(x, y) == Rgb([0xFF, 0, 0])).count() + 1;

            let line_height = y - last_y;

            // Scan to find all the blues.
            let mut blues = vec![];
            for by in last_y..y {
                if (0..band_width).any(|bx| *img.get_pixel(bx as u32, by) == Rgb([0, 0, 0xFF])) {
                    blues.push(by);
                }
            }
            if blues.is_empty() {
                panic!("missing baseline");
            } else if blues.len() > 1 {
                panic!("ambiguous baseline");
            }

            let baseline = blues.iter().cloned().next().unwrap();

            let ascent = baseline + 1 - last_y;
            let descent = line_height - ascent;

            let mut glyph_widths = vec![];
            let mut glyph_data = vec![];
            let mut last_glyph_edge = 0;
            for bx in 0..band_width {
                if *img.get_pixel(bx as u32, baseline) == Rgb([0xFF, 0, 0]) {
                    let w = bx - last_glyph_edge;
                    assert!(w < 65);
                    if w != 0 {
                        let mut bits = vec![];
                        for gy in last_y..y {
                            let mut row = 0u64;
                            let mut mask = 1 << 63;
                            for gx in last_glyph_edge..bx {
                                if *img.get_pixel(gx as u32, gy) == Rgb([0, 0, 0]) {
                                    row |= mask;
                                }
                                mask >>= 1;
                            }
                            bits.push(row);
                        }
                        glyph_data.push(bits);
                        glyph_widths.push(w);
                    }
                    last_glyph_edge = bx + 1;
                }
            }


            bands.push((
                ascent,
                descent,
                glyph_data,
                glyph_widths,
            ));

            last_y = y + 1;
        }
    }
    let max_ascent = bands.iter().map(|&(ascent, _, _, _)| ascent).max().unwrap();
    let max_descent = bands.iter().map(|&(_, descent, _, _)| descent).max().unwrap();

    for (ascent, descent, glyphs, _) in &mut bands {
        let ascent_pad = max_ascent - *ascent;
        let descent_pad = max_descent - *descent;
        if ascent_pad != 0 || descent_pad != 0 {
            for glyph in glyphs {
                for _ in 0..ascent_pad {
                    glyph.insert(0, 0);
                }
                for _ in 0..descent_pad {
                    glyph.push(0);
                }
            }
            *ascent = max_ascent;
            *descent = max_descent;
        }
    }

    let mut out_glyphs = vec![];
    let mut out_bitmap = vec![];

    for (_, _, data, widths) in &bands {
        for (glyph, &width) in data.iter().zip(widths) {
            let pad_top = glyph.iter().take_while(|&&row| row == 0).count();
            let g = if pad_top == glyph.len() {
                Glyph {
                    row_bytes: 0,
                    image_height: 0,
                    image_offset: 0,
                    origin: (0, 0),
                    advance: u8::try_from(width).unwrap(),
                }
            } else {
                let pad_bottom = glyph.iter().rev().take_while(|&&row| row == 0).count();
                let pad_left = glyph.iter().map(|row| row.leading_zeros()).min().unwrap();
                let pad_right = glyph.iter().map(|row| row.trailing_zeros()).min().unwrap();

                let x_bits = 64 - pad_right - pad_left;
                let height = glyph.len() - pad_bottom - pad_top;
                let row_bytes = u8::try_from((x_bits + 7) / 8).unwrap();

                let mut bytes = vec![];

                for row in glyph[pad_top..glyph.len() - pad_bottom].iter().cloned() {
                    let mut row = row << pad_left;
                    for _ in 0..row_bytes {
                        bytes.push(row.to_be_bytes()[0]);
                        row <<= 8;
                    }
                }

                // Search for any _existing_ copy of the bitmap data in our
                // array. This finds actual hits for actual fonts, believe it or
                // not.
                //
                // The windows + `==` + position approach being used here relies
                // on the slice `==` implementation early exiting, which of
                // course it does. So it's n^2 worst-case but in practice much
                // closer to n.
                let image_offset = if let Some(prev) = out_bitmap.windows(bytes.len()).position(|w| w == bytes) {
                    u16::try_from(prev).unwrap()
                } else {
                    let image_offset = u16::try_from(out_bitmap.len()).unwrap();
                    out_bitmap.extend(bytes);
                    image_offset
                };

                Glyph {
                    row_bytes,
                    image_height: u8::try_from(height).unwrap(),
                    origin: (
                        u8::try_from(pad_left).unwrap(),
                        u8::try_from(pad_top).unwrap(),
                    ),
                    advance: u8::try_from(width).unwrap(),

                    image_offset,
                }
            };

            out_glyphs.push(g);
        }
    }

    // Double-check the byte reuse logic above.
    //
    // Yeah, using Aho-Corasick for this is arguably massive overkill, but it's
    // also _really easy._
    {
        let patterns = out_glyphs.iter()
            .map(|g| {
                let s = usize::from(g.image_offset);
                let e = s + usize::from(g.image_height) * usize::from(g.row_bytes);
                &out_bitmap[s..e]
            })
            .collect::<Vec<_>>();
        let fsm = aho_corasick::AhoCorasick::new_auto_configured(&patterns);
        for mat in fsm.find_overlapping_iter(&out_bitmap) {
            let g = &out_glyphs[mat.pattern()];
            let io = usize::from(g.image_offset);
            if mat.start() < io && mat.end() <= io {
                eprintln!("WARNING: data for glyph {} can be found earlier at {}",
                    mat.pattern(), mat.start());
                let orig = &out_bitmap[io..io + usize::from(g.row_bytes) * usize::from(g.image_height)];
                let alt = &out_bitmap[mat.start()..mat.end()];
                assert_eq!(orig, alt);
                eprintln!("original at {}: {:x?}", io, orig);
                eprintln!("alt at {}:      {:x?}", mat.start(), alt);
            }
        }
    }

    let first = if let Some(f) = first {
        f
    } else {
        let blanks: Vec<usize> = out_glyphs.iter().enumerate()
            .filter_map(|(i, g)| if g.image_height == 0 { Some(i) } else { None })
            .collect();
        match &blanks[..] {
            [x] => b' ' - u8::try_from(*x).unwrap(),
            [0, 32] => 0,
            [x, y] if *y == *x + 223 => 32,
            [0, 32, 255] => 0,
            [0, 95] => 32,
            _ => {
                panic!("can't detect font offset due to ambiguous blank pattern: {:?}", blanks);
            }
        }
    };


    let glyph_storage = GlyphStorage::Dense {
        first,
        glyphs: &out_glyphs,
    };
    let kerning = KerningTable { entries: &[] };
    let font = Font {
        ascent: u8::try_from(max_ascent).unwrap(),
        descent: u8::try_from(max_descent).unwrap(),
        line_spacing: u8::try_from(max_ascent + max_descent).unwrap(),
        glyph_storage,
        replacement: 0,
        bitmaps: &out_bitmap,
        kerning,
    };

    body(&font)
}
