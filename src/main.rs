use std::path::Path;

use anyhow::{Result, bail};
use clap::Parser;

#[derive(Parser, Debug)]
#[command(name = "pym", about = "Python Extract Method refactoring tool")]
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

    /// Custom name for the extracted function (default: extracted_func_0)
    #[arg(long)]
    name: Option<String>,

    /// Interactive mode: review and customize each step
    #[arg(long, short)]
    interactive: bool,
}

/// Parse positional args into (file_paths, start_line, end_line).
///
/// The last two numeric arguments are start_line and end_line.
/// Everything before them is a file path.
fn parse_positional(args: &[String]) -> Result<(Vec<String>, usize, usize)> {
    if args.len() < 3 {
        bail!("Usage: pym FILE [FILE...] START END");
    }

    let end: usize = args[args.len() - 1]
        .parse()
        .map_err(|_| anyhow::anyhow!("Last argument must be a line number (END)"))?;
    let start: usize = args[args.len() - 2]
        .parse()
        .map_err(|_| anyhow::anyhow!("Second-to-last argument must be a line number (START)"))?;

    let files: Vec<String> = args[..args.len() - 2].to_vec();
    if files.is_empty() {
        bail!("At least one file path is required");
    }

    Ok((files, start, end))
}

/// Extract the module stem from a file path (e.g., "src/utils.py" -> "utils").
fn file_stem(path: &str) -> String {
    Path::new(path)
        .file_stem()
        .map(|s| s.to_string_lossy().to_string())
        .unwrap_or_else(|| path.to_string())
}

fn main() -> Result<()> {
    let cli = Cli::parse();
    let (files, start_line, end_line) = parse_positional(&cli.args)?;

    let func_name = cli.name.as_deref().unwrap_or("extracted_func_0");

    if files.len() == 1 {
        // Single-file mode: fully compatible with existing behavior.
        let source = std::fs::read_to_string(&files[0])?;

        if cli.interactive {
            let file_path = if cli.write || cli.diff {
                Some(files[0].as_str())
            } else {
                None
            };
            return pym::interactive::run_interactive(
                &source, start_line, end_line, file_path, cli.diff,
            );
        }

        let options = pym::ExtractOptions {
            func_name: cli.name,
        };
        let result = pym::extract_method_with_options(&source, start_line, end_line, &options)?;

        if cli.write {
            std::fs::write(&files[0], &result)?;
            eprintln!("Wrote refactored code to {}", files[0]);
        } else if cli.diff {
            let diff = pym::rewrite::unified_diff(&source, &result, &files[0]);
            print!("{diff}");
        } else {
            print!("{result}");
        }
    } else {
        // Multi-file mode.
        let sources: Vec<String> = files
            .iter()
            .map(std::fs::read_to_string)
            .collect::<Result<Vec<_>, _>>()?;
        let source_refs: Vec<&str> = sources.iter().map(|s| s.as_str()).collect();
        let file_refs: Vec<&str> = files.iter().map(|s| s.as_str()).collect();
        let target_stem = file_stem(&files[0]);

        if cli.interactive {
            return pym::interactive::run_interactive_multi(
                &source_refs,
                &file_refs,
                start_line,
                end_line,
                cli.write || cli.diff,
                cli.diff,
                &target_stem,
            );
        }

        // Stage 1: Scan target file for hash + matches.
        let (target_hash, window_size, target_matches) =
            pym::scan::find_matches_with_hash(source_refs[0], start_line, end_line)?;

        // Scan extra files for matches.
        let mut all_blocks: Vec<pym::SourcedBlock> = target_matches
            .into_iter()
            .map(|b| pym::SourcedBlock {
                block: b,
                source_index: 0,
            })
            .collect();

        for (i, src) in source_refs.iter().enumerate().skip(1) {
            let extra_matches = pym::scan::find_matches_in_file(src, target_hash, window_size);
            all_blocks.extend(extra_matches.into_iter().map(|b| pym::SourcedBlock {
                block: b,
                source_index: i,
            }));
        }

        if all_blocks.len() < 2 {
            bail!(
                "Only {} block(s) found across all files. Need at least 2 matching blocks.",
                all_blocks.len()
            );
        }

        // Stage 2: Plan extraction.
        let plan = pym::plan_extraction_multi(&source_refs, &all_blocks, start_line, end_line)?;

        // Stage 3: Apply refactoring.
        let results = pym::rewrite::apply_refactoring_multi(
            &source_refs,
            &all_blocks,
            &plan.ref_node_positions,
            &plan.sig,
            func_name,
            &plan.scope_ctx,
            &target_stem,
        );

        if cli.write {
            for (i, result) in results.iter().enumerate() {
                std::fs::write(&files[i], result)?;
                eprintln!("Wrote refactored code to {}", files[i]);
            }
        } else if cli.diff {
            for (i, result) in results.iter().enumerate() {
                if result != sources[i].as_str() {
                    let diff = pym::rewrite::unified_diff(&sources[i], result, &files[i]);
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
    }

    Ok(())
}
