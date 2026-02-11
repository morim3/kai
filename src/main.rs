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

    /// Write the refactored code back to the file
    #[arg(long)]
    write: bool,

    /// Show a unified diff instead of the full refactored source
    #[arg(long)]
    diff: bool,

    /// Custom name for the extracted function (default: extracted_func_0)
    #[arg(long)]
    name: Option<String>,

    /// Custom parameter names (comma-separated, e.g. "a, b, c")
    #[arg(long)]
    args: Option<String>,

    /// Select which matched blocks to replace (comma-separated 1-based indices, e.g. "1,3")
    #[arg(long)]
    select: Option<String>,
}

fn main() -> Result<()> {
    let cli = Cli::parse();
    let source = std::fs::read_to_string(&cli.file)?;

    let options = pym::ExtractOptions {
        func_name: cli.name,
        param_names: cli
            .args
            .map(|s| s.split(',').map(|p| p.trim().to_string()).collect()),
        select: cli.select.map(|s| {
            s.split(',')
                .map(|p| p.trim().parse::<usize>().expect("invalid block index"))
                .collect()
        }),
    };

    let result = pym::extract_method_with_options(&source, cli.start_line, cli.end_line, &options)?;

    if cli.write {
        std::fs::write(&cli.file, &result)?;
        eprintln!("Wrote refactored code to {}", cli.file);
    } else if cli.diff {
        let diff = pym::rewrite::unified_diff(&source, &result, &cli.file);
        print!("{diff}");
    } else {
        print!("{result}");
    }

    Ok(())
}
