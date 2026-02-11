use anyhow::Result;
use clap::Parser;

#[derive(Parser, Debug)]
#[command(name = "pym", about = "Python Extract Method refactoring tool")]
struct Cli {
    /// Path to the Python file
    file: String,

    /// Start line of the target block (1-based)
    start_line: usize,

    /// End line of the target block (1-based, inclusive)
    end_line: usize,
}

fn main() -> Result<()> {
    let cli = Cli::parse();

    let source = std::fs::read_to_string(&cli.file)?;
    let hash = pym::normalize::hash_block(&source, cli.start_line, cli.end_line)?;

    println!("{hash:#018x}");
    Ok(())
}
