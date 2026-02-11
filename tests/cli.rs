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

/// Phase 5 Exit Criterion 1: Default output is refactored source code.
#[test]
fn default_output_is_refactored_source() {
    let expected = fs::read_to_string(fixture_path("simple_assignment", "expected.py")).unwrap();
    pym()
        .args(["tests/fixtures/simple_assignment/input.py", "2", "3"])
        .assert()
        .success()
        .stdout(expected);
}

/// Phase 5 Exit Criterion 1: `--diff` outputs unified diff.
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

/// Phase 5 Exit Criterion 2: `--select` replaces only chosen blocks.
#[test]
fn select_replaces_chosen_blocks_only() {
    let expected = fs::read_to_string(fixture_path("select_blocks", "expected.py")).unwrap();
    pym()
        .args([
            "tests/fixtures/select_blocks/input.py",
            "2",
            "3",
            "--select",
            "1,3",
        ])
        .assert()
        .success()
        .stdout(expected);
}

/// Phase 5 Exit Criterion 3: `--name` and `--args` customize the generated function.
#[test]
fn custom_name_and_args() {
    let expected = fs::read_to_string(fixture_path("custom_names", "expected.py")).unwrap();
    pym()
        .args([
            "tests/fixtures/custom_names/input.py",
            "2",
            "3",
            "--name",
            "compute",
            "--args",
            "x, y",
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

/// Error case: invalid block index in --select.
#[test]
fn select_invalid_index_fails() {
    pym()
        .args([
            "tests/fixtures/simple_assignment/input.py",
            "2",
            "3",
            "--select",
            "abc",
        ])
        .assert()
        .failure()
        .stderr(predicate::str::contains("invalid block index"));
}

/// Error case: nonexistent file.
#[test]
fn nonexistent_file_fails() {
    pym().args(["nonexistent.py", "1", "2"]).assert().failure();
}
