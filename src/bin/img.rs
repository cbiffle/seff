use clap::Parser;

#[derive(Debug, Parser)]
struct Img {
    #[clap(long)]
    first: Option<u8>,
    #[clap(arg_enum, short, long)]
    charset: Option<seff::load::GlyphOrderArg>,
    input: std::path::PathBuf,
}

fn main() {
    let args = Img::parse();

    let input = std::fs::File::open(args.input).unwrap();
    let input = std::io::BufReader::new(input);

    let order = args.charset.unwrap_or(seff::load::GlyphOrderArg::Iso8859_1);

    seff::load::load_font_from_png(input, order.into(), args.first, |font| {
        seff::gen::generate_rust_module(&font, std::io::stdout())?;
        Ok(())
    }).unwrap();
}
