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

    /// Interactive mode: review and customize each step
    #[arg(long, short)]
    interactive: bool,
}

fn main() -> Result<()> {
    let cli = Cli::parse();
    let source = std::fs::read_to_string(&cli.file)?;

    if cli.interactive {
        let file_path = if cli.write || cli.diff {
            Some(cli.file.as_str())
        } else {
            None
        };
        return pym::interactive::run_interactive(
            &source,
            cli.start_line,
            cli.end_line,
            file_path,
            cli.diff,
        );
    }

    let options = pym::ExtractOptions {
        func_name: cli.name,
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
