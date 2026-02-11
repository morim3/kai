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
    let matches = pym::scan::find_matches(&source, cli.start_line, cli.end_line)?;

    println!("Found {} matching block(s):", matches.len());
    for m in &matches {
        println!(
            "  lines {}-{} (bytes {}..{})",
            m.start_line, m.end_line, m.start_offset, m.end_offset
        );
    }

    Ok(())
}
