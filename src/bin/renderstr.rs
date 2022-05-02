use image::Luma;
use clap::Parser;

#[derive(Debug, Parser)]
struct RenderStr {
    #[clap(long)]
    first: Option<u8>,
    #[clap(short)]
    invert: bool,

    font: std::path::PathBuf,
    output: std::path::PathBuf,
    text: String,
}

fn main() {
    let args = RenderStr::parse();

    let font = std::fs::File::open(args.font).unwrap();
    let font = std::io::BufReader::new(font);

    seff::load::load_font_from_png(
        font,
        args.first,
        |font| {
            let line_count = args.text.lines().count();
            let img_width = args.text.lines()
                .map(|line| font.width(line))
                .max()
                .unwrap();

            let (bg, fg) = if args.invert {
                (0, Luma([0xFF]))
            } else {
                (0xFF, Luma([0]))
            };
            let mut outimg = image::ImageBuffer::<Luma<u8>, _>::new(
                img_width as u32,
                (font.line_spacing_usize() * line_count) as u32,
            );
            outimg.fill(bg);

            for (i, line) in args.text.lines().enumerate() {
                font.render_direct(line, 0, i * font.line_spacing_usize(), &mut outimg, fg);
            }

            outimg.save(args.output)?;
            Ok(())
        }
    ).unwrap();

}
