use anyhow::{Result, bail};
use clap::Parser;
use ruff_text_size::Ranged;

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

    // Phase 2: find matching blocks.
    let blocks = pym::scan::find_matches(&source, cli.start_line, cli.end_line)?;
    if blocks.len() < 2 {
        bail!(
            "Only {} block(s) found. Need at least 2 matching blocks to extract a function.",
            blocks.len()
        );
    }

    eprintln!("Found {} matching block(s):", blocks.len());
    for m in &blocks {
        eprintln!(
            "  lines {}-{} (bytes {}..{})",
            m.start_line, m.end_line, m.start_offset, m.end_offset
        );
    }

    // Phase 3: scope analysis.
    let parsed = ruff_python_parser::parse_module(&source)
        .map_err(|e| anyhow::anyhow!("Parse error: {e}"))?;
    let body = &parsed.into_syntax().body;
    let window_size = {
        let target_stmts =
            pym::normalize::select_stmts(&source, body, cli.start_line, cli.end_line);
        target_stmts.len()
    };

    let mut sig_inputs: Vec<(&[ruff_python_ast::Stmt], &[ruff_python_ast::Stmt])> = Vec::new();
    for block in &blocks {
        let idx = body
            .iter()
            .position(|s| s.range().start().to_usize() == block.start_offset)
            .expect("block offset should match a statement");
        let after_start = idx + window_size;
        let block_slice = &body[idx..idx + window_size];
        let after_slice = if after_start < body.len() {
            &body[after_start..]
        } else {
            &[]
        };
        sig_inputs.push((block_slice, after_slice));
    }

    // Extract divergences between blocks.
    let mut all_divs = Vec::new();
    if sig_inputs.len() >= 2 {
        let (ref_block, _) = &sig_inputs[0];
        for (other_block, _) in sig_inputs.iter().skip(1) {
            let divs = pym::diff_extract::extract_divergences(ref_block, other_block, &source, &source);
            all_divs.push(divs);
        }
    }

    let sig = pym::scope::unify_signatures(&sig_inputs, &all_divs);

    // Phase 4: rewrite.
    let result = pym::rewrite::apply_refactoring(&source, &blocks, &sig);

    if cli.write {
        std::fs::write(&cli.file, &result)?;
        eprintln!("Wrote refactored code to {}", cli.file);
    } else {
        let diff = pym::rewrite::unified_diff(&source, &result, &cli.file);
        print!("{diff}");
    }

    Ok(())
}
