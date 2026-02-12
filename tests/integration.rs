use std::fs;
use std::path::Path;

/// Parse the `# kai: START-END` marker from the first line of a fixture input file.
fn parse_marker(content: &str) -> (usize, usize) {
    let first_line = content.lines().next().expect("empty fixture file");
    let marker = first_line
        .strip_prefix("# kai: ")
        .unwrap_or_else(|| panic!("missing '# kai: START-END' marker in: {first_line}"));
    let (start, end) = marker
        .split_once('-')
        .unwrap_or_else(|| panic!("invalid marker format: {marker}"));
    (
        start.parse().expect("invalid start line"),
        end.parse().expect("invalid end line"),
    )
}

/// Parse an optional `options.toml` file from a fixture directory.
///
/// Supports simple key-value format:
///   func_name = "compute"
fn parse_options(dir: &Path) -> kai::ExtractOptions {
    let path = dir.join("options.toml");
    let content = match fs::read_to_string(&path) {
        Ok(c) => c,
        Err(_) => return kai::ExtractOptions::default(),
    };

    let mut opts = kai::ExtractOptions::default();
    for line in content.lines() {
        let line = line.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        let Some((key, value)) = line.split_once('=') else {
            continue;
        };
        let key = key.trim();
        let value = value.trim();
        if key == "func_name" {
            opts.func_name = Some(value.trim_matches('"').to_string());
        }
    }
    opts
}

/// Check if a fixture directory is a multi-file fixture (has extra_*.py files).
fn is_multi_file_fixture(dir: &Path) -> bool {
    fs::read_dir(dir).unwrap().filter_map(|e| e.ok()).any(|e| {
        let name = e.file_name().to_string_lossy().to_string();
        name.starts_with("extra_") && name.ends_with(".py")
    })
}

/// Collect extra files in order: extra_1.py, extra_2.py, etc.
fn collect_extra_files(dir: &Path) -> Vec<String> {
    let mut extras: Vec<String> = fs::read_dir(dir)
        .unwrap()
        .filter_map(|e| e.ok())
        .filter_map(|e| {
            let name = e.file_name().to_string_lossy().to_string();
            if name.starts_with("extra_") && name.ends_with(".py") {
                Some(name)
            } else {
                None
            }
        })
        .collect();
    extras.sort();
    extras
}

/// Run a single-file success-case fixture: input.py + expected.py (+ optional options.toml).
fn run_fixture(dir: &Path) -> Result<(), String> {
    let input = fs::read_to_string(dir.join("input.py"))
        .unwrap_or_else(|e| panic!("failed to read {}/input.py: {e}", dir.display()));
    let expected = fs::read_to_string(dir.join("expected.py"))
        .unwrap_or_else(|e| panic!("failed to read {}/expected.py: {e}", dir.display()));

    let (start, end) = parse_marker(&input);
    let options = parse_options(dir);
    let result = match kai::extract_method_with_options(&input, start, end, &options) {
        Ok(r) => r,
        Err(e) => return Err(format!("pipeline failed: {e}")),
    };

    if ruff_python_parser::parse_module(&result).is_err() {
        return Err(format!("output is not valid Python:\n{result}"));
    }

    if result != expected {
        return Err(format!(
            "output mismatch:\n--- expected ---\n{expected}\n--- got ---\n{result}"
        ));
    }

    Ok(())
}

/// Run a multi-file success-case fixture.
fn run_multi_fixture(dir: &Path) -> Result<(), String> {
    let input = fs::read_to_string(dir.join("input.py"))
        .unwrap_or_else(|e| panic!("failed to read {}/input.py: {e}", dir.display()));
    let expected = fs::read_to_string(dir.join("expected.py"))
        .unwrap_or_else(|e| panic!("failed to read {}/expected.py: {e}", dir.display()));

    let (start, end) = parse_marker(&input);
    let options = parse_options(dir);
    let func_name = options.func_name.as_deref().unwrap_or("extracted_func_0");

    // Collect extra files.
    let extra_names = collect_extra_files(dir);
    let extra_sources: Vec<String> = extra_names
        .iter()
        .map(|name| {
            fs::read_to_string(dir.join(name))
                .unwrap_or_else(|e| panic!("failed to read {}/{name}: {e}", dir.display()))
        })
        .collect();

    // Build sources array: [target, extra_1, extra_2, ...]
    let mut sources: Vec<&str> = vec![input.as_str()];
    sources.extend(extra_sources.iter().map(|s| s.as_str()));

    // Stage 1: Scan all files.
    let all_blocks = kai::scan_all_sources(&sources, start, end)
        .map_err(|e| format!("scan failed: {e}"))?;

    // Stage 2: Plan.
    let plan = kai::plan_extraction_multi(&sources, &all_blocks, start, end)
        .map_err(|e| format!("plan failed: {e}"))?;

    // Stage 3: Apply.
    let results = kai::rewrite::apply_refactoring_multi(
        &sources,
        &all_blocks,
        &plan.ref_node_positions,
        &plan.sig,
        func_name,
        &plan.scope_ctx,
        "input", // target file stem
    );

    // Check target file output.
    if ruff_python_parser::parse_module(&results[0]).is_err() {
        return Err(format!(
            "target output is not valid Python:\n{}",
            results[0]
        ));
    }
    if results[0] != expected {
        return Err(format!(
            "target output mismatch:\n--- expected ---\n{expected}\n--- got ---\n{}",
            results[0]
        ));
    }

    // Check each extra file output.
    for (i, extra_name) in extra_names.iter().enumerate() {
        let expected_name = extra_name.replace("extra_", "expected_extra_");
        let expected_path = dir.join(&expected_name);

        if expected_path.exists() {
            let expected_extra = fs::read_to_string(&expected_path)
                .unwrap_or_else(|e| panic!("failed to read {}: {e}", expected_path.display()));

            let result = &results[i + 1];

            if ruff_python_parser::parse_module(result).is_err() {
                return Err(format!(
                    "{extra_name} output is not valid Python:\n{result}"
                ));
            }
            if result.as_str() != expected_extra {
                return Err(format!(
                    "{extra_name} output mismatch:\n--- expected ---\n{expected_extra}\n--- got ---\n{result}"
                ));
            }
        }
    }

    Ok(())
}

/// Run an error-case fixture: input.py + expected_error.txt.
fn run_error_fixture(dir: &Path) {
    let input = fs::read_to_string(dir.join("input.py"))
        .unwrap_or_else(|e| panic!("failed to read {}/input.py: {e}", dir.display()));
    let expected_error = fs::read_to_string(dir.join("expected_error.txt"))
        .unwrap_or_else(|e| panic!("failed to read {}/expected_error.txt: {e}", dir.display()));

    let (start, end) = parse_marker(&input);
    let err = kai::extract_method(&input, start, end)
        .expect_err(&format!("expected error for {}, but got Ok", dir.display()));

    assert!(
        err.to_string().contains(expected_error.trim()),
        "error mismatch for {}:\n  got:      {err}\n  expected: {}",
        dir.display(),
        expected_error.trim()
    );
}

/// Data-driven test runner: discovers all fixture directories and runs them.
#[test]
fn fixture_tests() {
    let fixtures_dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures");
    let mut entries: Vec<_> = fs::read_dir(&fixtures_dir)
        .unwrap_or_else(|e| panic!("failed to read fixtures dir: {e}"))
        .filter_map(|e| e.ok())
        .filter(|e| e.file_type().map(|t| t.is_dir()).unwrap_or(false))
        .collect();
    entries.sort_by_key(|e| e.file_name());

    assert!(!entries.is_empty(), "no fixture directories found");

    let mut passed = 0;
    let mut known_bugs = 0;

    for entry in &entries {
        let dir = entry.path();
        let name = dir.file_name().unwrap().to_string_lossy();
        let is_known_bug = dir.join("known_bug.txt").exists();

        if dir.join("expected.py").exists() {
            let result = if is_multi_file_fixture(&dir) {
                run_multi_fixture(&dir)
            } else {
                run_fixture(&dir)
            };

            match (result, is_known_bug) {
                (Ok(()), false) => {
                    eprintln!("  PASS:      {name}");
                    passed += 1;
                }
                (Ok(()), true) => {
                    panic!(
                        "fixture {name} is marked as known_bug but now passes! \
                         Remove known_bug.txt."
                    );
                }
                (Err(_msg), true) => {
                    let bug_desc = fs::read_to_string(dir.join("known_bug.txt")).unwrap();
                    eprintln!("  KNOWN_BUG: {name} — {}", bug_desc.lines().next().unwrap());
                    known_bugs += 1;
                }
                (Err(msg), false) => {
                    panic!("fixture {name} failed:\n{msg}");
                }
            }
        } else if dir.join("expected_error.txt").exists() {
            eprintln!("  PASS:      {name} (error case)");
            run_error_fixture(&dir);
            passed += 1;
        } else {
            panic!(
                "fixture {} has neither expected.py nor expected_error.txt",
                dir.display()
            );
        }
    }

    eprintln!("  --- {passed} passed, {known_bugs} known bug(s) ---");
}
