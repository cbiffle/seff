use std::io::{self, Write};

use crate::{Font, GlyphStorage, Glyph};

pub fn generate_rust_module(
    font: &Font<'_, '_, '_>,
    mut out: impl Write,
) -> io::Result<()> {
    writeln!(out, "use seff::*;")?;
    writeln!(out, "pub static FONT: Font = Font {{")?;
    writeln!(out, "    ascent: {},", font.ascent)?;
    writeln!(out, "    descent: {},", font.descent)?;
    writeln!(out, "    line_spacing: {},", font.line_spacing)?;
    write!(out, "    glyph_storage: ")?;
    match font.glyph_storage {
        GlyphStorage::Dense { first, .. } => {
            writeln!(out, "GlyphStorage::Dense {{")?;
            writeln!(out, "        first: {first},")?;
            writeln!(out, "        glyphs: &GLYPHS,")?;
            writeln!(out, "    }},")?;
        }
    }
    writeln!(out, "    replacement: {},", font.replacement)?;
    writeln!(out, "    bitmaps: &BITMAPS,")?;
    writeln!(out, "    kerning: KerningTable {{ entries: &KERNING_ENTRIES }},")?;
    writeln!(out, "}};")?;

    match font.glyph_storage {
        GlyphStorage::Dense { first, glyphs } => {
            writeln!(out, "pub static GLYPHS: [Glyph; {}] = [", glyphs.len())?;
            for (i, g) in glyphs.iter().enumerate() {
                let Glyph {
                    row_bytes,
                    image_offset,
                    image_height,
                    origin,
                    advance,
                } = g;
                writeln!(out, "    // index {}: '{}'", i, char::from_u32(u32::from(first) + i as u32).unwrap_or('?'))?;
                if *row_bytes != 0 {
                    let chunk = &font.bitmaps[usize::from(*image_offset)..usize::from(*image_offset) + usize::from(*row_bytes) * usize::from(*image_height)];
                    for row in chunk.chunks(usize::from(*row_bytes)) {
                        write!(out, "    // |")?;
                        for byte in row {
                            let mut byte = *byte;
                            for _ in 0..8 {
                                write!(out, "{}", if byte & 0x80 != 0 { '*' } else { ' ' })?;
                                byte <<= 1;
                            }
                        }
                        writeln!(out, "|")?;
                    }
                }
                writeln!(out, "    Glyph {{")?;
                writeln!(out, "        row_bytes: {row_bytes},")?;
                writeln!(out, "        image_offset: {image_offset},")?;
                writeln!(out, "        image_height: {image_height},")?;
                writeln!(out, "        origin: {origin:?},")?;
                writeln!(out, "        advance: {advance},")?;
                writeln!(out, "    }},")?;
            }
            writeln!(out, "];")?;
        }
    }

    writeln!(out, "pub static KERNING_ENTRIES: [KerningEntry; {}] = [",
        font.kerning.entries.len())?;
    for e in font.kerning.entries {
        writeln!(out, "    KerningEntry {{")?;
        writeln!(out, "        pair: {:?},", e.pair)?;
        writeln!(out, "        adjust: {},", e.adjust)?;
        writeln!(out, "    }},")?;
    }
    writeln!(out, "];")?;

    writeln!(out, "pub static BITMAPS: [u8; {}] = [", font.bitmaps.len())?;
    for line in font.bitmaps.chunks(8) {
        for (i, byte) in line.iter().enumerate() {
            write!(out, "{}0x{:02x},",
                if i == 0 { "    " } else { " " },
                byte
            )?;
        }

        writeln!(out)?;
    }
    writeln!(out, "];")?;

    Ok(())
}
