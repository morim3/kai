use predicates::prelude::*;
use std::fs;

fn pym() -> assert_cmd::Command {
    assert_cmd::Command::new(assert_cmd::cargo::cargo_bin!("pym"))
}

fn fixture_path(name: &str, file: &str) -> String {
    format!(
        "{}/tests/fixtures/{name}/{file}",
        env!("CARGO_MANIFEST_DIR")
    )
}

/// Default output is refactored source code.
#[test]
fn default_output_is_refactored_source() {
    let expected = fs::read_to_string(fixture_path("simple_assignment", "expected.py")).unwrap();
    pym()
        .args(["tests/fixtures/simple_assignment/input.py", "2", "3"])
        .assert()
        .success()
        .stdout(expected);
}

/// `--diff` outputs unified diff.
#[test]
fn diff_flag_outputs_unified_diff() {
    pym()
        .args([
            "tests/fixtures/simple_assignment/input.py",
            "2",
            "3",
            "--diff",
        ])
        .assert()
        .success()
        .stdout(predicate::str::contains("-a = 1"))
        .stdout(predicate::str::contains("+extracted_func_0(1, 2)"));
}

/// `--name` customizes the generated function name.
#[test]
fn custom_name() {
    let expected = fs::read_to_string(fixture_path("custom_names", "expected.py")).unwrap();
    pym()
        .args([
            "tests/fixtures/custom_names/input.py",
            "2",
            "3",
            "--name",
            "compute",
        ])
        .assert()
        .success()
        .stdout(expected);
}

/// `--write` writes the file and prints a message to stderr.
#[test]
fn write_flag_modifies_file() {
    let tmp = std::env::temp_dir().join("pym_write_test.py");
    fs::copy(fixture_path("simple_assignment", "input.py"), &tmp).unwrap();

    pym()
        .args([tmp.to_str().unwrap(), "2", "3", "--write"])
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
    pym().args(["nonexistent.py", "1", "2"]).assert().failure();
}
