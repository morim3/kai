use std::path::Path;

use anyhow::{Result, bail};
use clap::Parser;

#[derive(Parser, Debug)]
#[command(name = "kai", about = "Python Extract Method refactoring tool")]
struct Cli {
    /// Python files and line range: FILE [FILE...] START END
    #[arg(required = true)]
    args: Vec<String>,

    /// Write the refactored code back to the file(s)
    #[arg(long)]
    write: bool,

    /// Show a unified diff instead of the full refactored source
    #[arg(long)]
    diff: bool,

    /// Disable interactive mode (for scripting and testing)
    #[arg(long)]
    no_interactive: bool,
}

/// Parse positional args into (file_paths, start_line, end_line).
///
/// The last two numeric arguments are start_line and end_line.
/// Everything before them is a file path.
fn parse_positional(args: &[String]) -> Result<(Vec<String>, usize, usize)> {
    if args.len() < 3 {
        bail!("Usage: kai FILE [FILE...] START END");
    }

    let last_idx = args.len() - 1;
    let second_last_idx = args.len() - 2;

    let end: usize = args[last_idx]
        .parse()
        .map_err(|_| anyhow::anyhow!("Last argument must be a line number (END)"))?;
    let start: usize = args[second_last_idx]
        .parse()
        .map_err(|_| anyhow::anyhow!("Second-to-last argument must be a line number (START)"))?;

    let files: Vec<String> = args[..second_last_idx].to_vec();
    if files.is_empty() {
        bail!("At least one file path is required");
    }

    Ok((files, start, end))
}

/// Extract the module stem from a file path (e.g., "src/utils.py" -> "utils").
fn file_stem(path: &str) -> String {
    Path::new(path)
        .file_stem()
        .map_or_else(|| path.to_string(), |s| s.to_string_lossy().to_string())
}

fn main() -> Result<()> {
    let cli = Cli::parse();
    let (files, start_line, end_line) = parse_positional(&cli.args)?;

    let sources: Vec<String> = files
        .iter()
        .map(std::fs::read_to_string)
        .collect::<Result<Vec<_>, _>>()?;
    let source_refs: Vec<&str> = sources.iter().map(std::string::String::as_str).collect();
    let file_refs: Vec<&str> = files.iter().map(std::string::String::as_str).collect();
    let target_stem = file_stem(&files[0]);

    if !cli.no_interactive {
        return kai::interactive::run_interactive_multi(
            &source_refs,
            &file_refs,
            start_line,
            end_line,
            cli.write || cli.diff,
            cli.diff,
            &target_stem,
        );
    }

    let options = kai::ExtractOptions::default();
    let results =
        kai::extract_method_multi(&source_refs, start_line, end_line, &options, &target_stem)?;

    output_results(&sources, &results, &files, cli.write, cli.diff)?;
    Ok(())
}

fn output_results(
    sources: &[String],
    results: &[String],
    files: &[String],
    write: bool,
    show_diff: bool,
) -> Result<()> {
    if write {
        for (i, result) in results.iter().enumerate() {
            std::fs::write(&files[i], result)?;
            eprintln!("Wrote refactored code to {}", files[i]);
        }
    } else if show_diff {
        for (i, result) in results.iter().enumerate() {
            if result != sources[i].as_str() {
                let diff = kai::rewrite::unified_diff(&sources[i], result, &files[i]);
                print!("{diff}");
            }
        }
    } else {
        for (i, result) in results.iter().enumerate() {
            if files.len() > 1 {
                println!("=== {} ===", files[i]);
            }
            print!("{result}");
        }
    }
    Ok(())
}
