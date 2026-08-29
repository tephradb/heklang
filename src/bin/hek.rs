//! `hek`, the command-line checker. Parses every `.hk` file under a directory as one
//! program and runs the `test` declarations in it.
//!
//! Every check heklang has lives in the parser today (`docs/projectors.md` and
//! `docs/effects.md` both record which ones are still deferred to a checker that does
//! not exist yet), so "parses" and "checks" are the same pass. When the checker splits
//! out, this is where it gets called.
//!
//! `check_files` rather than `parse_files`, because a run reports every declaration
//! that failed rather than only the first. `docs/cli.md` has the granularity.

use std::env;
use std::fs;
use std::io::{self, Write};
use std::path::{Path, PathBuf};
use std::process::ExitCode;

use heklang::{Program, TestOutcome, check_files, run_tests};

const USAGE: &str = "\
hek: check heklang sources and run their tests

usage:
  hek [check|test] [path]

  check   parse every `.hk` file under `path` as one program
  test    the same, then run every `test` declaration in it

`path` is a directory or a single `.hk` file, and defaults to the current
directory. With no command `hek` does both. Every file under `path` is one
module of one program, and declaration order across them does not matter.
";

fn main() -> ExitCode {
    match run() {
        Ok(true) => ExitCode::SUCCESS,
        Ok(false) => ExitCode::FAILURE,
        Err(message) => {
            let _ = writeln!(io::stderr(), "hek: {message}");
            ExitCode::FAILURE
        }
    }
}

/// `Ok(false)` is a program that was read fine and did not pass; `Err` is not being
/// able to look at all. Same cut `docs/testing.md` rule 9 makes between a failed test
/// and an errored one, for the same reason.
fn run() -> Result<bool, String> {
    let mut args = env::args().skip(1);
    let mut command = Command::Both;
    let mut root: Option<PathBuf> = None;

    for arg in args.by_ref() {
        match arg.as_str() {
            "-h" | "--help" => {
                print!("{USAGE}");
                return Ok(true);
            }
            "check" if root.is_none() => command = Command::Check,
            "test" if root.is_none() => command = Command::Test,
            other if other.starts_with('-') => {
                return Err(format!("unknown option `{other}`; try `hek --help`"));
            }
            other => root = Some(PathBuf::from(other)),
        }
    }

    let root = root.unwrap_or_else(|| PathBuf::from("."));
    // A single file is a whole program too (`docs/modules.md`: there is no header item),
    // so pointing at one is worth allowing rather than making the caller name its
    // directory and pick up its neighbours.
    let (root, paths) = if root.is_dir() {
        let paths = sources(&root)?;
        if paths.is_empty() {
            return Err(format!("no `.hk` files under `{}`", root.display()));
        }
        (root, paths)
    } else if root.extension().is_some_and(|ext| ext == "hk") {
        let parent = root.parent().unwrap_or(Path::new(".")).to_path_buf();
        (parent, vec![root])
    } else {
        return Err(format!(
            "`{}` is not a directory or a `.hk` file",
            root.display()
        ));
    };

    // Read every file before parsing any: `check_files` borrows all of them at once,
    // because one program is assembled from all the modules together.
    let mut sources = Vec::new();
    for path in &paths {
        let body = fs::read_to_string(path).map_err(|err| format!("{}: {err}", path.display()))?;
        sources.push((label(&root, path), body));
    }
    let files: Vec<(&str, &str)> = sources
        .iter()
        .map(|(name, body)| (name.as_str(), body.as_str()))
        .collect();

    let program = match check_files(files) {
        Ok(program) => program,
        Err(errors) => {
            // Each is already `file:line:col: message`, which is what an editor jumps
            // to. The count goes last so a long list ends by saying how long it was.
            for err in &errors {
                println!("{err}");
            }
            let s = if errors.len() == 1 { "" } else { "s" };
            println!("\n{} error{s}", errors.len());
            return Ok(false);
        }
    };

    let files = paths.len();
    let s = if files == 1 { "" } else { "s" };
    println!("checked {files} file{s}");
    println!("  {}", counts(&program));

    if command == Command::Check {
        return Ok(true);
    }
    Ok(suite(&program))
}

#[derive(PartialEq, Eq)]
enum Command {
    Check,
    Test,
    Both,
}

fn suite(program: &Program) -> bool {
    if program.tests.is_empty() {
        println!("\nno tests");
        return true;
    }

    println!();
    let results = run_tests(program);
    for result in &results {
        match &result.outcome {
            TestOutcome::Passed => println!("pass   {}", result.name),
            TestOutcome::Failed(why) => println!("FAIL   {}\n         {why}", result.name),
            TestOutcome::Errored(why) => println!("ERROR  {}\n         {why}", result.name),
        }
    }

    let passed = results.iter().filter(|result| result.passed()).count();
    let failed = results.len() - passed;
    println!("\n{passed} passed, {failed} failed");
    failed == 0
}

/// What the program declares, so a run says something even when there are no tests.
fn counts(program: &Program) -> String {
    let mut parts = Vec::new();
    let mut count = |n: usize, one: &str, many: &str| {
        if n > 0 {
            parts.push(format!("{n} {}", if n == 1 { one } else { many }));
        }
    };
    count(program.events.len(), "event", "events");
    count(program.commands.len(), "command", "commands");
    count(program.projectors.len(), "projector", "projectors");
    count(program.effects.len(), "effect", "effects");
    count(program.functions.len(), "fn", "fns");
    count(program.records.len(), "record", "records");
    count(program.enums.len(), "enum", "enums");
    count(program.consts.len(), "const", "consts");
    count(program.tests.len(), "test", "tests");
    if parts.is_empty() {
        return "nothing declared".to_string();
    }
    parts.join(", ")
}

/// Every `.hk` file under `root`, sorted so a run reports the same way twice. Hidden
/// directories and `target` are skipped: a build directory holding a vendored `.hk`
/// would otherwise join the program and collide with the source it was copied from.
fn sources(root: &Path) -> Result<Vec<PathBuf>, String> {
    let mut found = Vec::new();
    walk(root, &mut found)?;
    found.sort();
    Ok(found)
}

fn walk(dir: &Path, found: &mut Vec<PathBuf>) -> Result<(), String> {
    let entries = fs::read_dir(dir).map_err(|err| format!("{}: {err}", dir.display()))?;
    for entry in entries {
        let entry = entry.map_err(|err| format!("{}: {err}", dir.display()))?;
        let path = entry.path();
        let name = entry.file_name();
        let name = name.to_string_lossy();
        if name.starts_with('.') || name == "target" {
            continue;
        }
        if path.is_dir() {
            walk(&path, found)?;
        } else if path.extension().is_some_and(|ext| ext == "hk") {
            found.push(path);
        }
    }
    Ok(())
}

/// The name an error is reported under: relative to the root, so a diagnostic reads
/// `commands/place_order.hk:12:3` rather than carrying the whole absolute path.
fn label(root: &Path, path: &Path) -> String {
    path.strip_prefix(root)
        .unwrap_or(path)
        .to_string_lossy()
        .into_owned()
}
