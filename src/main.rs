use clap::Parser;

#[derive(Parser)]
#[command(
    name = "lore",
    version,
    about = "Local semantic search for your software patterns"
)]
struct Cli {}

fn main() {
    let _cli = Cli::parse();
}
