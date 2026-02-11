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

    /// Write the refactored code back to the file instead of showing a diff
    #[arg(long)]
    write: bool,
}

fn main() -> Result<()> {
    let cli = Cli::parse();
    let source = std::fs::read_to_string(&cli.file)?;

    let result = pym::extract_method(&source, cli.start_line, cli.end_line)?;

    if cli.write {
        std::fs::write(&cli.file, &result)?;
        eprintln!("Wrote refactored code to {}", cli.file);
    } else {
        let diff = pym::rewrite::unified_diff(&source, &result, &cli.file);
        print!("{diff}");
    }

    Ok(())
}
