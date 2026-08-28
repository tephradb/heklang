//! `hek`, the command-line checker, as executable tests. These run the built binary the
//! way a user does: point it at a directory and read what it prints.

use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};

const EVENTS: &str = "event @order.placed { order_id: Int, total: Money(2) }\n";

const COMMAND: &str = "command Place(order_id: Int, total: Money(2)) {
  guard @order.placed(order_id)
  state placed: Bool = fold false
    on @order.placed(order_id) => true
  if placed {
    return reject(\"duplicate\", \"already placed\")
  }
  emit @order.placed { order_id, total }
}
";

const PASSING: &str = "test \"a first order is appended\" {
  run Place { order_id: 1, total: 10.00 }
  expect @order.placed { order_id: 1, total: 10.00 }
}
";

const FAILING: &str = "test \"this one is wrong\" {
  run Place { order_id: 1, total: 10.00 }
  expect @order.placed { order_id: 2, total: 10.00 }
}
";

/// A throwaway directory of `.hk` files, named after the case so two tests running at
/// once cannot collide.
fn project(name: &str, files: &[(&str, &str)]) -> PathBuf {
    let root = std::env::temp_dir().join(format!("hek-cli-{name}"));
    let _ = fs::remove_dir_all(&root);
    for (path, body) in files {
        let path = root.join(path);
        fs::create_dir_all(path.parent().expect("a parent")).expect("create the directory");
        fs::write(&path, body).expect("write the file");
    }
    root
}

fn hek(args: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_hek"))
        .args(args)
        .output()
        .expect("run hek")
}

fn run(root: &Path, args: &[&str]) -> Output {
    let mut all: Vec<&str> = args.to_vec();
    let root = root.to_str().expect("a utf-8 path");
    all.push(root);
    hek(&all)
}

fn stdout(output: &Output) -> String {
    String::from_utf8(output.stdout.clone()).expect("utf-8 output")
}

fn stderr(output: &Output) -> String {
    String::from_utf8(output.stderr.clone()).expect("utf-8 output")
}

/// Every `.hk` file under the directory is one module of one program, so a command in
/// one file and the event it emits in another compose without anything linking them.
#[test]
fn files_across_directories_are_one_program() {
    let root = project(
        "one-program",
        &[
            ("events/order.hk", EVENTS),
            ("commands/place.hk", COMMAND),
            ("tests/place.hk", PASSING),
        ],
    );
    let output = run(&root, &[]);

    assert!(output.status.success(), "{}", stdout(&output));
    let text = stdout(&output);
    assert!(text.contains("checked 3 files"), "{text}");
    assert!(text.contains("1 event, 1 command, 1 test"), "{text}");
    assert!(text.contains("pass   a first order is appended"), "{text}");
    assert!(text.contains("1 passed, 0 failed"), "{text}");
}

#[test]
fn a_failing_test_fails_the_run() {
    let root = project(
        "failing",
        &[
            ("events.hk", EVENTS),
            ("place.hk", COMMAND),
            ("tests.hk", FAILING),
        ],
    );
    let output = run(&root, &[]);

    assert!(!output.status.success());
    let text = stdout(&output);
    assert!(text.contains("FAIL   this one is wrong"), "{text}");
    assert!(
        text.contains("@order.placed.order_id: expected 2, got 1"),
        "{text}"
    );
    assert!(text.contains("0 passed, 1 failed"), "{text}");
}

/// A syntax error is reported at `file:line:col`, with the file named relative to the
/// directory the run was pointed at.
#[test]
fn a_syntax_error_names_the_file_and_position() {
    let root = project(
        "syntax",
        &[
            ("events.hk", EVENTS),
            (
                "commands/broken.hk",
                "command Broken(x: Int) { emit @nope.here { x } }\n",
            ),
        ],
    );
    let output = run(&root, &[]);

    assert!(!output.status.success());
    let text = stdout(&output);
    assert!(
        text.starts_with("commands/broken.hk:1:"),
        "expected a relative path and a position: {text}"
    );
    assert!(text.contains("event @nope.here is not declared"), "{text}");
}

/// `check` stops after parsing, which is what a pre-commit hook wants.
#[test]
fn check_parses_without_running_the_tests() {
    let root = project(
        "check-only",
        &[
            ("events.hk", EVENTS),
            ("place.hk", COMMAND),
            ("tests.hk", FAILING),
        ],
    );
    let output = run(&root, &["check"]);

    assert!(
        output.status.success(),
        "a failing test does not fail `check`: {}",
        stdout(&output)
    );
    let text = stdout(&output);
    assert!(text.contains("checked 3 files"), "{text}");
    assert!(!text.contains("FAIL"), "{text}");
}

/// A module is not a namespace, so a single file is a whole program too.
#[test]
fn a_single_file_is_a_whole_program() {
    let root = project("single", &[("events.hk", EVENTS)]);
    let output = hek(&[root.join("events.hk").to_str().expect("a utf-8 path")]);

    assert!(output.status.success(), "{}", stderr(&output));
    let text = stdout(&output);
    assert!(text.contains("checked 1 file"), "{text}");
    assert!(text.contains("1 event"), "{text}");
    assert!(text.contains("no tests"), "{text}");
}

/// A build directory holding a copied `.hk` would otherwise join the program and
/// collide with the source it was copied from.
#[test]
fn target_and_hidden_directories_are_skipped() {
    let root = project(
        "skipped",
        &[
            ("events.hk", EVENTS),
            ("target/events.hk", EVENTS),
            (".git/events.hk", EVENTS),
        ],
    );
    let output = run(&root, &[]);

    assert!(
        output.status.success(),
        "a copy under target would be a duplicate declaration: {}",
        stdout(&output)
    );
    assert!(
        stdout(&output).contains("checked 1 file"),
        "{}",
        stdout(&output)
    );
}

#[test]
fn a_directory_with_no_sources_says_so() {
    let root = project("empty", &[("README.md", "not heklang\n")]);
    let output = run(&root, &[]);

    assert!(!output.status.success());
    assert!(
        stderr(&output).contains("no `.hk` files under"),
        "{}",
        stderr(&output)
    );
}

#[test]
fn an_unknown_option_points_at_help() {
    let output = hek(&["--nope"]);

    assert!(!output.status.success());
    assert!(
        stderr(&output).contains("unknown option `--nope`; try `hek --help`"),
        "{}",
        stderr(&output)
    );
}

#[test]
fn help_succeeds_and_names_both_commands() {
    let output = hek(&["--help"]);

    assert!(output.status.success());
    let text = stdout(&output);
    assert!(text.contains("hek [check|test] [path]"), "{text}");
}
