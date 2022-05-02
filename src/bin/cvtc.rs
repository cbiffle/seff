use std::collections::BTreeMap;

use clap::Parser;
use image::Rgb;

#[derive(Debug, Parser)]
struct Cvtc {
    #[clap(short)]
    width: usize,
    #[clap(short)]
    height: usize,
    #[clap(short)]
    ascent: Option<usize>,
    #[clap(long, default_value = "16")]
    per_band: usize,
    #[clap(long)]
    flip_y: bool,
    #[clap(long)]
    flip_x: bool,
    #[clap(long)]
    add_advance: Option<usize>,

    input: std::path::PathBuf,
    output: std::path::PathBuf,
}

fn main() {
    let args = Cvtc::parse();

    let mut font_data: Vec<u32> = ron::de::from_reader(
        std::fs::File::open(args.input).unwrap()
    ).unwrap();
    let bytes_per_row = (args.width + 7) / 8;
    for row in &mut font_data {
        if args.flip_x {
            *row = row.reverse_bits();
        } else {
            *row <<= 8 * (4 - bytes_per_row);
        }
    }

    let glyph_data: Vec<Vec<_>> = font_data.chunks_exact(args.height)
        .map(|chunk| {
            if args.flip_y {
                chunk.iter().rev().cloned().collect()
            } else {
                chunk.iter().cloned().collect()
            }
        })
        .collect();

    println!("Loaded {} glyphs / {} bytes.", glyph_data.len(), font_data.len() * bytes_per_row);

    let ascent = if let Some(a) = args.ascent {
        a
    } else {
        // Guess the font's ascent by scanning glyphs to count bottom-padding, and
        // then taking the mode of that.
        let mut bottom_pads: BTreeMap<usize, usize> = BTreeMap::new();
        for glyph in &glyph_data {
            let p = glyph.iter().rev().take_while(|&&row| row == 0).count();
            if p != 0 {
                *bottom_pads.entry(p).or_default() += 1;
            }
        }
        if bottom_pads.is_empty() {
            0
        } else {
            let mut bottom_pads: Vec<_> = bottom_pads.into_iter().collect();
            bottom_pads.sort_by_key(|&(_pad, count)| count);
            let descender_guess = bottom_pads.last().unwrap().0;
            args.height - descender_guess
        }
    };

    if ascent > args.height {
        panic!("ascent must be <= height");
    }

    println!("Line height: {}; ascent = {}, descent = {}", args.height, ascent, args.height - ascent);

    let cell_width = args.width + args.add_advance.unwrap_or(0);

    let img_width = u32::try_from((cell_width + 1) * args.per_band).unwrap();
    let n_bands = (glyph_data.len() + (args.per_band - 1)) / args.per_band;
    let band_height = args.height + 1;
    let img_height = u32::try_from(band_height * n_bands).unwrap();

    let mut img = image::ImageBuffer::<Rgb<u8>, _>::new(img_width, img_height);

    // Fill the canvas with white, except for band boundaries and baselines.
    for (y, row) in img.enumerate_rows_mut() {
        let band_y = y as usize % band_height;
        let color = if band_y == band_height - 1 {
            Rgb([0xFF, 0, 0])
        } else if band_y == ascent - 1 {
            Rgb([0, 0, 0xFF])
        } else {
            Rgb([0xFF, 0xFF, 0xFF])
        };
        for (_, _, p) in row {
            *p = color;
        }
    }

    for (i, data) in glyph_data.iter().enumerate() {
        let band = i / args.per_band;
        let gy = u32::try_from(band_height * band).unwrap();
        let gx = u32::try_from((i % args.per_band) * (cell_width + 1)).unwrap();

        // Baseline glyph separator
        img.put_pixel(gx + cell_width as u32, gy + ascent as u32 - 1, Rgb([0xFF, 0, 0]));
        img.put_pixel(gx + cell_width as u32, gy + ascent as u32 - 2, Rgb([0xFF, 0, 0]));

        // Draw glyph
        for (row_i, row) in data.iter().enumerate() {
            let mut row = *row;
            let py = gy + u32::try_from(row_i).unwrap();
            for px in gx..gx + args.width as u32 {
                if row & 1 << 31 != 0 {
                    img.put_pixel(px, py, Rgb([0, 0, 0]));
                }
                row <<= 1;
            }
        }
    }

    img.save(args.output).unwrap();
    
}
