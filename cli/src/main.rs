//! `hek`, the command-line checker. Parses every `.hk` file under a directory as one
//! program and runs the `test` declarations in it.
//!
//! Every check heklang has lives in the parser today (`docs/projectors.md` and
//! `docs/effects.md` both record which ones are still deferred to a checker that does
//! not exist yet), so "parses" and "checks" are the same pass. When the checker splits
//! out, this is where it gets called.
//!
//! `check_files` rather than `parse_files`, because a run reports every mistake it found
//! rather than only the first. `docs/cli.md` has the granularity.

use std::env;
use std::fs;
use std::io::{self, Write};
use std::mem;
use std::path::{Path, PathBuf};
use std::process::ExitCode;

use heklang::{Diagnostic, Program, Severity, Span, TestOutcome, check_files, run_tests};

/// Where a `= ` line wraps. Wide enough for a long sentence to stay one or two lines,
/// narrow enough to read beside the source it is about.
const WIDTH: usize = 84;

const USAGE: &str = "\
hek: check heklang sources, run their tests, and format them

usage:
  hek [check|test|fmt] [--boundaries] [--check] [path]

  check   parse every `.hk` file under `path` as one program
  test    the same, then run every `test` declaration in it
  fmt     rewrite every `.hk` file under `path` canonically

  --boundaries  print what each command guards, transitively
  --check       with `fmt`, name what would change and write nothing

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
    let mut asked_for_boundaries = false;
    let mut checking = false;

    for arg in args.by_ref() {
        match arg.as_str() {
            "-h" | "--help" => {
                print!("{USAGE}");
                return Ok(true);
            }
            "--boundaries" => asked_for_boundaries = true,
            "--check" => checking = true,
            "check" if root.is_none() => command = Command::Check,
            "test" if root.is_none() => command = Command::Test,
            "fmt" if root.is_none() => command = Command::Fmt,
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

    if command == Command::Fmt {
        return format(&root, &paths, checking);
    }
    if checking {
        return Err("`--check` belongs to `fmt`; `check` is already a command".to_string());
    }

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
            // The count goes last so a long list ends by saying how long it was.
            for err in &errors {
                print!("{}", report(&sources, err));
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
    if asked_for_boundaries {
        boundaries(&program);
    }

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
    Fmt,
}

/// Rewrite every file under `path`, or with `--check` name the ones that would change and
/// write nothing.
///
/// Per file rather than per program, which is the one place `fmt` parts company with
/// `check`. A program is every `.hk` file together, but layout is a property of one file at
/// a time, and a file whose neighbours are missing still formats.
fn format(root: &Path, paths: &[PathBuf], checking: bool) -> Result<bool, String> {
    let mut changed = Vec::new();
    let mut unparsed = Vec::new();
    for path in paths {
        let source =
            fs::read_to_string(path).map_err(|err| format!("{}: {err}", path.display()))?;
        // The grammar accepts more than the language does, so this is a syntax error and
        // nothing else: a program `check` would reject still has a shape to print.
        let Some(formatted) = hek::fmt::format(&source) else {
            unparsed.push(label(root, path));
            continue;
        };
        if formatted == source {
            continue;
        }
        changed.push(label(root, path));
        if !checking {
            fs::write(path, formatted).map_err(|err| format!("{}: {err}", path.display()))?;
        }
    }

    for name in &unparsed {
        println!("{name}: does not parse, so it was left alone");
    }
    let verb = if checking { "would be" } else { "was" };
    for name in &changed {
        println!("{name} {verb} reformatted");
    }
    let s = if changed.len() == 1 { "" } else { "s" };
    match (checking, changed.is_empty()) {
        (_, true) => println!(
            "{} file{} already formatted",
            paths.len(),
            if paths.len() == 1 { "" } else { "s" }
        ),
        (true, false) => println!("\n{} file{s} would change", changed.len()),
        (false, false) => println!("\n{} file{s} reformatted", changed.len()),
    }

    // `--check` is a gate, so a file that would change fails it. Rewriting is not a gate,
    // so it only fails on a file it could not read at all, which is what a syntax error is
    // here: `check` is where a program is judged.
    Ok(unparsed.is_empty() && (!checking || changed.is_empty()))
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

/// What each command guards, transitively. A guard's slices join the boundary of what
/// names it, so once guards compose the append condition is a closure rather than
/// something a reader can take off the page. This is how to see it: two commands meant to
/// conflict on the same events can be compared here, which is the one question
/// `docs/testing.md` §8 deliberately keeps out of a test.
///
/// **Asked for, not printed by default.** `check` is a pass/fail gate, and this is one
/// line per command rather than a summary, so a 26-command program pays 26 lines on every
/// run to restate what its own `guard` lines already say. Only the transitive part is
/// information, and only when someone is asking.
fn boundaries(program: &Program) {
    let named: Vec<&heklang::Command> = program
        .commands
        .iter()
        .filter(|command| !command.calls.is_empty())
        .collect();
    if named.is_empty() {
        return;
    }
    println!();
    for command in named {
        let mut reached: Vec<String> = Vec::new();
        for call in &command.calls {
            through(program, &call.guard, &mut reached);
        }
        println!("  {} guards {}", command.name, reached.join(", "));
    }
}

/// A guard and everything it guards, first named first, each once however many paths
/// reach it.
fn through(program: &Program, name: &str, out: &mut Vec<String>) {
    if out.iter().any(|seen| seen == name) {
        return;
    }
    out.push(name.to_string());
    let Some(guard) = program.guard(name) else {
        return;
    };
    for call in &guard.calls {
        through(program, &call.guard, out);
    }
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
    count(program.guards.len(), "guard", "guards");
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

/// One diagnostic, whole. The header is what an editor jumps to and everything under it
/// is for a person: the source line with the extent drawn, then the hint, then every
/// related location. `docs/cli.md` has the shape.
fn report(sources: &[(String, String)], err: &Diagnostic) -> String {
    let mut out = match err.severity {
        Severity::Error => format!(
            "{} [{}] {}\n",
            place(err.file.as_deref(), err.span),
            err.code,
            err.message
        ),
        Severity::Warning => format!(
            "{} [warning: {}] {}\n",
            place(err.file.as_deref(), err.span),
            err.code,
            err.message
        ),
    };
    let gutter = " ".repeat(err.span.start.line.to_string().len());
    if let Some(drawn) = underline(sources, err) {
        out.push_str(&drawn);
    }
    if let Some(hint) = &err.hint {
        out.push_str(&note(&gutter, hint));
    }
    for related in &err.related {
        let line = format!(
            "{}: {}",
            place(related.file.as_deref(), related.span),
            related.message
        );
        out.push_str(&note(&gutter, &line));
    }
    out
}

/// `file:line:col`, or `line:col` for a source with no name. The same text in the header
/// and in a related location, so both read as somewhere to go.
fn place(file: Option<&str>, span: Span) -> String {
    match file {
        Some(file) => format!("{file}:{span}"),
        None => format!("{span}"),
    }
}

/// A `= ` line under the drawing, wrapped so a long hint does not run off the screen.
/// Continuations line up under the first word rather than under the `=`, which is what
/// makes one note read as one thing.
fn note(gutter: &str, text: &str) -> String {
    let mut out = String::new();
    for (index, line) in wrap(text, WIDTH).into_iter().enumerate() {
        let lead = if index == 0 { "=" } else { " " };
        out.push_str(&format!("{gutter} {lead} {line}\n"));
    }
    out
}

/// Greedy wrapping on spaces. A word longer than the width is left whole rather than
/// broken: it is a path or an identifier, and half of one is not readable.
fn wrap(text: &str, width: usize) -> Vec<String> {
    let mut lines = Vec::new();
    let mut line = String::new();
    for word in text.split(' ') {
        if !line.is_empty() && line.chars().count() + 1 + word.chars().count() > width {
            lines.push(mem::take(&mut line));
        }
        if !line.is_empty() {
            line.push(' ');
        }
        line.push_str(word);
    }
    if !line.is_empty() {
        lines.push(line);
    }
    lines
}

/// The source line a diagnostic is about, with its extent drawn underneath. The header
/// line above it already says where to jump to; this says what the message is about, which
/// is the thing a position alone cannot.
///
/// `None` where there is nothing to draw: an error with no module, one whose span is the
/// end-of-file sentinel, or a line that is not in the file. `docs/diagnostics.md` rule 5
/// has why each of those has no extent.
fn underline(sources: &[(String, String)], err: &Diagnostic) -> Option<String> {
    let file = err.file.as_deref()?;
    let body = sources
        .iter()
        .find(|(name, _)| name == file)
        .map(|(_, body)| body.as_str())?;

    let number = err.span.start.line;
    let text = body.lines().nth(number.checked_sub(1)? as usize)?;

    // Columns count `char`s (`docs/diagnostics.md` rule 1), so the gap in front of the
    // carets is measured in `char`s too. A span ending on a later line is drawn to the end
    // of this one and no further: a raw string would otherwise take the screen.
    let start = err.span.start.col.max(1) as usize - 1;
    let end = if err.span.end.line == number {
        err.span.end.col.max(1) as usize - 1
    } else {
        text.chars().count()
    };
    let width = end.saturating_sub(start).max(1);

    let gutter = " ".repeat(number.to_string().len());
    Some(format!(
        "{gutter} |\n{number} | {text}\n{gutter} | {}{}\n",
        " ".repeat(start),
        "^".repeat(width)
    ))
}

/// The name an error is reported under: relative to the root, so a diagnostic reads
/// `commands/place_order.hk:12:3` rather than carrying the whole absolute path.
fn label(root: &Path, path: &Path) -> String {
    path.strip_prefix(root)
        .unwrap_or(path)
        .to_string_lossy()
        .into_owned()
}
