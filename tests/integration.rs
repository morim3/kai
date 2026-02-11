use std::fs;
use std::path::Path;

/// Parse the `# pym: START-END` marker from the first line of a fixture input file.
fn parse_marker(content: &str) -> (usize, usize) {
    let first_line = content.lines().next().expect("empty fixture file");
    let marker = first_line
        .strip_prefix("# pym: ")
        .unwrap_or_else(|| panic!("missing '# pym: START-END' marker in: {first_line}"));
    let (start, end) = marker
        .split_once('-')
        .unwrap_or_else(|| panic!("invalid marker format: {marker}"));
    (
        start.parse().expect("invalid start line"),
        end.parse().expect("invalid end line"),
    )
}

/// Run a success-case fixture: input.py + expected.py.
/// Returns Ok(()) on match, Err(message) on mismatch.
fn run_fixture(dir: &Path) -> Result<(), String> {
    let input = fs::read_to_string(dir.join("input.py"))
        .unwrap_or_else(|e| panic!("failed to read {}/input.py: {e}", dir.display()));
    let expected = fs::read_to_string(dir.join("expected.py"))
        .unwrap_or_else(|e| panic!("failed to read {}/expected.py: {e}", dir.display()));

    let (start, end) = parse_marker(&input);
    let result = match pym::extract_method(&input, start, end) {
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

/// Run an error-case fixture: input.py + expected_error.txt.
fn run_error_fixture(dir: &Path) {
    let input = fs::read_to_string(dir.join("input.py"))
        .unwrap_or_else(|e| panic!("failed to read {}/input.py: {e}", dir.display()));
    let expected_error = fs::read_to_string(dir.join("expected_error.txt"))
        .unwrap_or_else(|e| panic!("failed to read {}/expected_error.txt: {e}", dir.display()));

    let (start, end) = parse_marker(&input);
    let err = pym::extract_method(&input, start, end)
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
            match (run_fixture(&dir), is_known_bug) {
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
