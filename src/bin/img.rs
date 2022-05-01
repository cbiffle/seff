use clap::Parser;

#[derive(Debug, Parser)]
struct Img {
    #[clap(long)]
    first: Option<u8>,
    input: std::path::PathBuf,
}

fn main() {
    let args = Img::parse();

    let input = std::fs::File::open(args.input).unwrap();
    let input = std::io::BufReader::new(input);

    seff::load::load_font_from_png(input, args.first, |font| {
        seff::gen::generate_rust_module(&font, std::io::stdout())?;
        Ok(())
    }).unwrap();
}
