use predicates::prelude::*;
use std::fs;

fn kai() -> assert_cmd::Command {
    assert_cmd::Command::new(assert_cmd::cargo::cargo_bin!("kai"))
}

fn fixture_path(name: &str, file: &str) -> String {
    format!(
        "{}/tests/fixtures/{name}/{file}",
        env!("CARGO_MANIFEST_DIR")
    )
}

/// Default (no --no-interactive) tries interactive mode, which fails without a tty.
#[test]
fn default_is_interactive() {
    kai()
        .args(["tests/fixtures/simple_assignment/input.py", "2", "3"])
        .assert()
        .failure()
        .stderr(predicate::str::contains("unknown").not());
}

/// `--no-interactive` outputs refactored source code.
#[test]
fn no_interactive_outputs_refactored_source() {
    let expected = fs::read_to_string(fixture_path("simple_assignment", "expected.py")).unwrap();
    kai()
        .args([
            "tests/fixtures/simple_assignment/input.py",
            "2",
            "3",
            "--no-interactive",
        ])
        .assert()
        .success()
        .stdout(expected);
}

/// `--diff` outputs unified diff.
#[test]
fn diff_flag_outputs_unified_diff() {
    kai()
        .args([
            "tests/fixtures/simple_assignment/input.py",
            "2",
            "3",
            "--no-interactive",
            "--diff",
        ])
        .assert()
        .success()
        .stdout(predicate::str::contains("-a = 1"))
        .stdout(predicate::str::contains("+extracted_func_0(1, 2)"));
}

/// `--write` writes the file and prints a message to stderr.
#[test]
fn write_flag_modifies_file() {
    let tmp = std::env::temp_dir().join("kai_write_test.py");
    fs::copy(fixture_path("simple_assignment", "input.py"), &tmp).unwrap();

    kai()
        .args([
            tmp.to_str().unwrap(),
            "2",
            "3",
            "--no-interactive",
            "--write",
        ])
        .assert()
        .success()
        .stderr(predicate::str::contains("Wrote refactored code"));

    let written = fs::read_to_string(&tmp).unwrap();
    let expected = fs::read_to_string(fixture_path("simple_assignment", "expected.py")).unwrap();
    assert_eq!(written, expected);

    fs::remove_file(&tmp).ok();
}

/// Error case: nonexistent file.
#[test]
fn nonexistent_file_fails() {
    kai()
        .args(["nonexistent.py", "1", "2", "--no-interactive"])
        .assert()
        .failure();
}
