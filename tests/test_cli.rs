use assert_cmd::Command;
use predicates::prelude::*;
use std::io::Write;
use tempfile::NamedTempFile;

fn cmd() -> Command {
    Command::new(assert_cmd::cargo::cargo_bin!("anytomd"))
}

/// Single file to stdout produces expected content.
#[test]
fn test_cli_single_file_stdout() {
    cmd()
        .arg("tests/fixtures/sample.csv")
        .assert()
        .success()
        .stdout(predicate::str::contains("Alice"))
        .stdout(predicate::str::contains("서울"));
}

/// Multiple files produce source separators.
#[test]
fn test_cli_multiple_files_with_separators() {
    cmd()
        .args(["tests/fixtures/sample.csv", "tests/fixtures/sample.json"])
        .assert()
        .success()
        .stdout(predicate::str::contains(
            "<!-- source: tests/fixtures/sample.csv -->",
        ))
        .stdout(predicate::str::contains(
            "<!-- source: tests/fixtures/sample.json -->",
        ))
        .stdout(predicate::str::contains("Alice"));
}

/// Output to file with -o flag.
#[test]
fn test_cli_output_to_file() {
    let out = NamedTempFile::new().unwrap();
    let out_path = out.path().to_str().unwrap().to_string();

    cmd()
        .args(["tests/fixtures/sample.csv", "-o", &out_path])
        .assert()
        .success()
        .stdout(predicate::str::is_empty());

    let content = std::fs::read_to_string(&out_path).unwrap();
    assert!(content.contains("Alice"));
}

/// Stdin with --format flag.
#[test]
fn test_cli_stdin_with_format() {
    cmd()
        .args(["--format", "txt"])
        .write_stdin("hello world")
        .assert()
        .success()
        .stdout(predicate::str::contains("hello world"));
}

/// Stdin with CSV format.
#[test]
fn test_cli_stdin_csv_format() {
    cmd()
        .args(["--format", "csv"])
        .write_stdin("Name,Age\nAlice,30\n")
        .assert()
        .success()
        .stdout(predicate::str::contains("Alice"))
        .stdout(predicate::str::contains("| Name | Age |"));
}

/// --format overrides file extension detection.
#[test]
fn test_cli_format_override_on_file() {
    // Create a file with .dat extension containing CSV data
    let mut tmp = NamedTempFile::with_suffix(".dat").unwrap();
    write!(tmp, "X,Y\n1,2\n").unwrap();

    cmd()
        .args(["--format", "csv", tmp.path().to_str().unwrap()])
        .assert()
        .success()
        .stdout(predicate::str::contains("| X | Y |"));
}

/// Missing file produces exit code 1.
#[test]
fn test_cli_missing_file_exit_1() {
    cmd()
        .arg("nonexistent_file.csv")
        .assert()
        .failure()
        .code(1)
        .stderr(predicate::str::contains("error: nonexistent_file.csv"));
}

/// Stdin without --format produces exit code 2.
#[test]
fn test_cli_stdin_without_format_exit_2() {
    cmd()
        .write_stdin("hello")
        .assert()
        .failure()
        .code(2)
        .stderr(predicate::str::contains("--format is required"));
}

/// Unsupported format produces exit code 1.
#[test]
fn test_cli_unsupported_format_exit_1() {
    cmd()
        .args(["--format", "zzz"])
        .write_stdin("data")
        .assert()
        .failure()
        .code(1)
        .stderr(predicate::str::contains("error:"));
}

/// --version flag shows version.
#[test]
fn test_cli_version_flag() {
    cmd()
        .arg("--version")
        .assert()
        .success()
        .stdout(predicate::str::contains(env!("CARGO_PKG_VERSION")));
}

/// --help flag shows usage.
#[test]
fn test_cli_help_flag() {
    cmd()
        .arg("--help")
        .assert()
        .success()
        .stdout(predicate::str::contains("Usage"))
        .stdout(predicate::str::contains("--format"))
        .stdout(predicate::str::contains("--output"));
}

/// --strict flag is accepted.
#[test]
fn test_cli_strict_flag() {
    cmd()
        .args(["--strict", "tests/fixtures/sample.csv"])
        .assert()
        .success()
        .stdout(predicate::str::contains("Alice"));
}

/// Multiple files where one is missing: partial success with exit code 1.
#[test]
fn test_cli_partial_failure_multiple_files() {
    cmd()
        .args(["tests/fixtures/sample.csv", "nonexistent.csv"])
        .assert()
        .failure()
        .code(1)
        .stdout(predicate::str::contains("Alice"))
        .stderr(predicate::str::contains("error: nonexistent.csv"));
}
