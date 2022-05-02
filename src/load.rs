use crate::*;
use std::io::{BufRead, Seek};
use image::Rgb;

#[derive(Copy, Clone, Eq, PartialEq, Debug, clap::ArgEnum)]
pub enum GlyphOrder {
    Iso8859_1,
    Cp437,
}

pub fn load_font_from_png<R>(
    png: impl BufRead + Seek,
    order: GlyphOrder,
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

    // Try to detect offset based on blanks.
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

    // Build sorted table of glyphs if required. Gotta do this out of the match
    // below because it winds up being borrowed.
    let sorted_glyphs = {
        let mut table = vec![];
        match order {
            GlyphOrder::Iso8859_1 => (),

            GlyphOrder::Cp437 => {
                for (&g, &c) in out_glyphs.iter().zip(&CP437_CODEPOINTS[first as usize..]) {
                    table.push((c, g));
                }
            }
        }
        table.sort_unstable_by_key(|&(c, _)| c);
        table
    };

    let glyph_storage = match order {
        GlyphOrder::Iso8859_1 => {
            GlyphStorage::Dense {
                first,
                glyphs: &out_glyphs,
            }
        }
        _ => GlyphStorage::Sparse { sorted_glyphs: &sorted_glyphs },
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

static CP437_CODEPOINTS: [char; 256] = {
    const CP437_CODEPOINTS_LOW_32: [char; 32] = [
        '\0',
        '\u{263A}',
        '\u{263B}',
        '\u{2665}',
        '\u{2666}',
        '\u{2663}',
        '\u{2660}',
        '\u{2022}',
        '\u{25D8}',
        '\u{25CB}',
        '\u{25D9}',
        '\u{2642}',
        '\u{2640}',
        '\u{266A}',
        '\u{266B}',
        '\u{263C}',
        '\u{25BA}',
        '\u{25C4}',
        '\u{2195}',
        '\u{203C}',
        '\u{00B6}',
        '\u{00A7}',
        '\u{25AC}',
        '\u{21A8}',
        '\u{2191}',
        '\u{2193}',
        '\u{2192}',
        '\u{2190}',
        '\u{221F}',
        '\u{2194}',
        '\u{25B2}',
        '\u{25BC}',
    ];

    let mut table = ['\0'; 256];
    let mut i = 0;
    while i < 32 {
        table[i] = CP437_CODEPOINTS_LOW_32[i];
        i += 1;
    }
    while i < 127 {
        table[i] = i as u8 as char;
        i += 1;
    }

    const CP437_CODEPOINTS_HIGH: [char; 129] = [
        '\u{2302}',
        '\u{00C7}',
        '\u{00FC}',
        '\u{00E9}',
        '\u{00E2}',
        '\u{00E4}',
        '\u{00E0}',
        '\u{00E5}',
        '\u{00E7}',
        '\u{00EA}',
        '\u{00EB}',
        '\u{00E8}',
        '\u{00EF}',
        '\u{00EE}',
        '\u{00EC}',
        '\u{00C4}',
        '\u{00C5}',
        '\u{00C9}',
        '\u{00E6}',
        '\u{00C6}',
        '\u{00F4}',
        '\u{00F6}',
        '\u{00F2}',
        '\u{00FB}',
        '\u{00F9}',
        '\u{00FF}',
        '\u{00D6}',
        '\u{00DC}',
        '\u{00A2}',
        '\u{00A3}',
        '\u{00A5}',
        '\u{20A7}',
        '\u{0192}',
        '\u{00E1}',
        '\u{00ED}',
        '\u{00F3}',
        '\u{00FA}',
        '\u{00F1}',
        '\u{00D1}',
        '\u{00AA}',
        '\u{00BA}',
        '\u{00BF}',
        '\u{2310}',
        '\u{00AC}',
        '\u{00BD}',
        '\u{00BC}',
        '\u{00A1}',
        '\u{00AB}',
        '\u{00BB}',
        '\u{2591}',
        '\u{2592}',
        '\u{2593}',
        '\u{2502}',
        '\u{2524}',
        '\u{2561}',
        '\u{2562}',
        '\u{2556}',
        '\u{2555}',
        '\u{2563}',
        '\u{2551}',
        '\u{2557}',
        '\u{255D}',
        '\u{255C}',
        '\u{255B}',
        '\u{2510}',
        '\u{2514}',
        '\u{2534}',
        '\u{252C}',
        '\u{251C}',
        '\u{2500}',
        '\u{253C}',
        '\u{255E}',
        '\u{255F}',
        '\u{255A}',
        '\u{2554}',
        '\u{2569}',
        '\u{2566}',
        '\u{2560}',
        '\u{2550}',
        '\u{256C}',
        '\u{2567}',
        '\u{2568}',
        '\u{2564}',
        '\u{2565}',
        '\u{2559}',
        '\u{2558}',
        '\u{2552}',
        '\u{2553}',
        '\u{256B}',
        '\u{256A}',
        '\u{2518}',
        '\u{250C}',
        '\u{2588}',
        '\u{2584}',
        '\u{258C}',
        '\u{2590}',
        '\u{2580}',
        '\u{03B1}',
        '\u{00DF}',
        '\u{0393}',
        '\u{03C0}',
        '\u{03A3}',
        '\u{03C3}',
        '\u{00B5}',
        '\u{03C4}',
        '\u{03A6}',
        '\u{0398}',
        '\u{03A9}',
        '\u{03B4}',
        '\u{221E}',
        '\u{03C6}',
        '\u{03B5}',
        '\u{2229}',
        '\u{2261}',
        '\u{00B1}',
        '\u{2265}',
        '\u{2264}',
        '\u{2320}',
        '\u{2321}',
        '\u{00F7}',
        '\u{2248}',
        '\u{00B0}',
        '\u{2219}',
        '\u{00B7}',
        '\u{221A}',
        '\u{207F}',
        '\u{00B2}',
        '\u{25A0}',
        '\u{00A0}',
    ];
    while i < 256{
        table[i] = CP437_CODEPOINTS_HIGH[i - 127];
        i += 1;
    }

    table
};

