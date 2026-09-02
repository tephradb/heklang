//! `hek`, the command-line checker, as executable tests. These run the built binary the
//! way a user does: point it at a directory and read what it prints.

use std::fs;
use std::io::{self, Write as _};
use std::path::{Path, PathBuf};
use std::process::{Command, Output, Stdio};

const EVENTS: &str = "event @order.placed { order_id: Int, total: Money(2) }\n";

const COMMAND: &str = "refusal Duplicate \"already placed\"
command Place(order_id: Int, total: Money(2)) {
  guard @order.placed(order_id)
  state placed: Bool = fold false
    on @order.placed(order_id) => true
  if placed {
    return reject Duplicate
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
    assert!(
        text.contains("1 event, 1 command, 1 refusal, 1 test"),
        "{text}"
    );
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
    assert!(text.contains("\n1 error"), "and it says how many: {text}");
}

/// A run reports every declaration that failed, across every file, and ends by saying
/// how many. One error per declaration: the run steps over the one that failed and
/// parses the next as if nothing happened.
#[test]
fn every_declaration_that_failed_is_reported() {
    let root = project(
        "many-errors",
        &[
            ("events.hk", EVENTS),
            (
                "a.hk",
                "command One(order_id: Int, text: String) {
  emit @order.placed { order_id, total: text }
}

command Two(order_id: Int, total: Money(2), text: String) {
  if text {
    return
  }
  emit @order.placed { order_id, total }
}
",
            ),
            (
                "b.hk",
                "command Three(order_id: Int, a: Money(2), b: Money(3)) {
  if a > b {
    return
  }
  emit @order.placed { order_id, total: a }
}
",
            ),
        ],
    );
    let output = run(&root, &["check"]);

    assert!(!output.status.success());
    let text = stdout(&output);
    assert!(
        text.contains("a.hk:2:41 [type-mismatch] expected Money(2), found String"),
        "the first declaration: {text}"
    );
    assert!(
        text.contains("a.hk:6:6 [type-mismatch] expected Bool, found String"),
        "and the next one in the same file: {text}"
    );
    assert!(
        text.contains("b.hk:2:6 [bad-operands] cannot apply `>` to Money(2) and Money(3)"),
        "and the next file: {text}"
    );
    assert!(text.contains("\n3 errors"), "{text}");
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
fn help_succeeds_and_names_every_command() {
    let output = hek(&["--help"]);

    assert!(output.status.success());
    let text = stdout(&output);
    assert!(
        text.contains("hek [check|test|fmt] [--boundaries] [--check] [path|-]"),
        "{text}"
    );
}

/// The line of carets a run drew, without its trailing newline.
fn caret_line(text: &str) -> &str {
    text.lines()
        .find(|line| line.contains("^"))
        .expect("a diagnostic was drawn")
}

/// `docs/cli.md`: the header says where to jump, and the line under it says what the
/// message is about. The extent is the thing a position alone cannot carry, so drawing it
/// is also what keeps it honest: a wrong span is visible here and nowhere else.
#[test]
fn an_error_is_drawn_under_the_source_it_is_about() {
    let root = project(
        "underline",
        &[
            ("events.hk", EVENTS),
            (
                "a.hk",
                "command One(order_id: Int, total: Money(2), text: String) {\n  emit @order.placed { order_id, total: text }\n}\n",
            ),
        ],
    );
    let text = stdout(&run(&root, &["check"]));

    assert!(
        text.contains("a.hk:2:41 [type-mismatch] expected Money(2), found String"),
        "the header is unchanged: {text}"
    );
    assert!(
        text.contains("2 |   emit @order.placed { order_id, total: text }\n"),
        "the source line, with its number in the gutter: {text}"
    );
    let carets = caret_line(&text);
    assert_eq!(
        carets.find(char::is_alphanumeric),
        None,
        "the caret line holds nothing but the gutter and the carets: {carets:?}"
    );
    assert_eq!(
        carets.matches("^").count(),
        4,
        "`text` is four wide: {text}"
    );
    assert_eq!(
        carets.find("^"),
        Some("  | ".len() + 40),
        "under column 41: {text}"
    );
}

/// `docs/diagnostics.md` rule 5: the end of the file has no extent, so there is nothing
/// to draw and the header stands alone rather than being followed by an empty gutter.
#[test]
fn an_error_with_no_extent_draws_nothing() {
    let root = project(
        "underline-none",
        &[("events.hk", EVENTS), ("a.hk", "command One(id: Int) {\n")],
    );
    let text = stdout(&run(&root, &["check"]));

    assert!(
        text.contains(":0:0 [expected-token] unclosed `{`"),
        "{text}"
    );
    assert!(
        !text.contains('^'),
        "there is no line 0 to draw under: {text}"
    );
}

/// `docs/diagnostics.md` rule 6: a span may end on a later line. It is drawn to the end
/// of the first one and stops, because a raw string would otherwise take the screen.
#[test]
fn a_span_over_several_lines_is_drawn_to_the_end_of_the_first() {
    let root = project(
        "underline-multiline",
        &[
            ("events.hk", EVENTS),
            (
                "a.hk",
                "command One(total: Money(2)) {\n  emit @order.placed { order_id: \"\"\"one\ntwo\"\"\", total }\n}\n",
            ),
        ],
    );
    let text = stdout(&run(&root, &["check"]));

    assert!(
        text.contains("a.hk:2:34 [type-mismatch] expected Int, found String"),
        "{text}"
    );
    let carets = caret_line(&text);
    assert_eq!(
        carets.matches("^").count(),
        6,
        "six characters, to the end of line 2 and no further: {text}"
    );
    assert!(!text.contains("3 | "), "and line 3 is not drawn: {text}");
}

/// `docs/cli.md`: the hint goes on a `= ` line under the drawing rather than inside the
/// header, so the header stays the one line an editor reads.
#[test]
fn a_hint_is_a_line_of_its_own() {
    let root = project(
        "hint-line",
        &[
            ("events.hk", "event @a.b { id: Int, name: String }\n"),
            (
                "a.hk",
                "command C(id: Int, text: String?) {\n  emit @a.b { id, name: text }\n}\n",
            ),
        ],
    );
    let text = stdout(&run(&root, &["check"]));

    assert!(
        text.contains("a.hk:2:25 [type-mismatch] expected String, found String?\n"),
        "the header is the message alone: {text}"
    );
    assert!(
        text.contains("  = `unwrap_or` gives it a fallback"),
        "and the hint is under the drawing: {text}"
    );
}

/// A related location reads like the header does, because it is somewhere to go too.
#[test]
fn a_related_location_is_a_line_of_its_own() {
    let root = project(
        "related-line",
        &[
            ("a.hk", "command C(id: Int) { return }\n"),
            (
                "b.hk",
                "event @x.y { id: Int }\ncommand C(id: Int) { return }\n",
            ),
        ],
    );
    let text = stdout(&run(&root, &["check"]));

    assert!(
        text.contains("b.hk:2:9 [declared-twice] command `C` is declared twice\n"),
        "{text}"
    );
    assert!(
        text.contains("  = a.hk:1:9: first declared here"),
        "the first declaration, in the module it is in: {text}"
    );
}

/// A cycle gets one `= ` line per link, so the loop can be walked from the report.
#[test]
fn a_cycle_draws_every_link() {
    let root = project(
        "cycle-links",
        &[(
            "a.hk",
            "fn a(n: Int) -> Int { return b(n) }\nfn b(n: Int) -> Int { return c(n) }\nfn c(n: Int) -> Int { return a(n) }\n",
        )],
    );
    let text = stdout(&run(&root, &["check"]));

    assert!(
        text.contains("  = a.hk:2:1: `b` is declared here"),
        "{text}"
    );
    assert!(
        text.contains("  = a.hk:3:1: `c` is declared here"),
        "{text}"
    );
}

/// A long hint wraps rather than running off the screen, and the continuation lines up
/// under the first word so one note reads as one thing.
#[test]
fn a_long_hint_wraps_under_itself() {
    let root = project(
        "hint-wrap",
        &[
            ("events.hk", "event @a.b { id: Int, name: String }\n"),
            (
                "a.hk",
                "command C(id: Int, text: String?) {\n  emit @a.b { id, name: text }\n}\n",
            ),
        ],
    );
    let text = stdout(&run(&root, &["check"]));

    let note: Vec<&str> = text
        .lines()
        .skip_while(|line| !line.starts_with("  = "))
        .take(2)
        .collect();
    assert_eq!(note.len(), 2, "the hint took two lines: {text}");
    assert!(
        note[1].starts_with("    ") && !note[1].starts_with("    ="),
        "the continuation lines up under the text, not under the `=`: {text}"
    );
}

/// A guard's slices join the boundary of whatever names it, transitively, so the append
/// condition stops being something a reader can take off the page. `check` prints the
/// closure, which is what makes two commands' boundaries comparable at all: a test
/// cannot ask (`docs/testing.md` §8).
#[test]
fn check_names_what_a_command_guards() {
    let source = "\
event @shop.connected { shop_id: Int }
event @plan.created { plan_id: Int, shop_id: Int }
event @plan.archived { plan_id: Int, shop_id: Int }
refusal ShopNotFound \"shop does not exist\"
refusal PlanNotFound \"no such plan\"

guard ShopIsConnected(shop_id: Int) {
  state connected: Bool = fold false
    on @shop.connected(shop_id) => true
  if !connected {
    return reject ShopNotFound
  }
}

guard PlanExists(plan_id: Int, shop_id: Int) {
  guard ShopIsConnected { shop_id }
  state exists: Bool = fold false
    on @plan.created(plan_id, shop_id) => true
  if !exists {
    return reject PlanNotFound
  }
}

command ArchivePlan(plan_id: Int, shop_id: Int) {
  guard PlanExists { plan_id, shop_id }
  emit @plan.archived { plan_id, shop_id }
}
";
    let root = project("guarded", &[("app.hk", source)]);
    let path = root.to_str().expect("a utf-8 path").to_string();
    let output = hek(&["check", "--boundaries", &path]);

    assert!(output.status.success(), "{}", stderr(&output));
    let text = stdout(&output);
    assert!(text.contains("2 guards"), "{text}");
    // `ShopIsConnected` is two levels down and still in the boundary.
    assert!(
        text.contains("ArchivePlan guards PlanExists, ShopIsConnected"),
        "{text}"
    );
}

/// `check` is a pass/fail gate, so the listing is asked for. Without the flag it prints
/// the counts and nothing per command, however many guards the program has.
#[test]
fn check_says_nothing_about_boundaries_unless_asked() {
    let source = "\
event @shop.connected { shop_id: Int }
event @shop.renamed { shop_id: Int }
refusal ShopNotFound \"shop does not exist\"

guard ShopIsConnected(shop_id: Int) {
  state connected: Bool = fold false
    on @shop.connected(shop_id) => true
  if !connected {
    return reject ShopNotFound
  }
}

command RenameShop(shop_id: Int) {
  guard ShopIsConnected { shop_id }
  emit @shop.renamed { shop_id }
}
";
    let root = project("quiet", &[("app.hk", source)]);
    let output = hek(&["check", root.to_str().expect("a utf-8 path")]);

    assert!(output.status.success(), "{}", stderr(&output));
    let text = stdout(&output);
    assert!(text.contains("1 guard"), "{text}");
    assert!(!text.contains("RenameShop guards"), "{text}");
}

/// `hek fmt` writes the file back, and says which ones it touched.
#[test]
fn fmt_rewrites_a_file_and_names_it() {
    let root = project(
        "fmt-writes",
        &[("messy.hk", "record  Item{sku:String,price:Int}\n")],
    );
    let output = run(&root, &["fmt"]);
    assert!(output.status.success(), "{}", stderr(&output));
    let text = stdout(&output);
    assert!(text.contains("messy.hk was reformatted"), "{text}");
    let written = fs::read_to_string(root.join("messy.hk")).expect("read it back");
    assert_eq!(
        written, "record Item {\n  sku: String,\n  price: Int,\n}\n",
        "a record always breaks, and every field takes a comma"
    );
}

/// `--check` is the gate: it names what would change, writes nothing, and fails.
#[test]
fn fmt_check_reports_without_writing() {
    let before = "record  Item{sku:String}\n";
    let root = project("fmt-check", &[("messy.hk", before)]);
    let output = run(&root, &["fmt", "--check"]);
    assert!(!output.status.success(), "a file that would change fails");
    let text = stdout(&output);
    assert!(text.contains("messy.hk would be reformatted"), "{text}");
    assert_eq!(
        fs::read_to_string(root.join("messy.hk")).expect("read it back"),
        before,
        "`--check` wrote to the file"
    );
}

/// Nothing to do is success and says so, which is what a pre-commit hook sees.
#[test]
fn fmt_check_is_quiet_when_everything_is_formatted() {
    let root = project(
        "fmt-clean",
        &[("tidy.hk", "record Item {\n  sku: String,\n}\n")],
    );
    let output = run(&root, &["fmt", "--check"]);
    assert!(output.status.success(), "{}", stdout(&output));
    assert!(
        stdout(&output).contains("already formatted"),
        "{}",
        stdout(&output)
    );
}

/// A file that does not parse is named and left alone rather than aborting the run, so one
/// broken file does not stop the rest of a tree being formatted.
#[test]
fn fmt_leaves_a_file_it_cannot_parse_alone() {
    let broken = "record Item {\n";
    let root = project(
        "fmt-broken",
        &[
            ("broken.hk", broken),
            ("messy.hk", "record  Other{a:Int}\n"),
        ],
    );
    let output = run(&root, &["fmt"]);
    assert!(
        !output.status.success(),
        "an unparseable file fails the run"
    );
    let text = stdout(&output);
    assert!(text.contains("broken.hk: does not parse"), "{text}");
    assert_eq!(
        fs::read_to_string(root.join("broken.hk")).expect("read it back"),
        broken,
        "the file that did not parse was written to"
    );
    assert!(
        fs::read_to_string(root.join("messy.hk"))
            .expect("read it back")
            .contains("record Other {"),
        "the file beside it should still have been formatted"
    );
}

/// `fmt` formats what `check` would reject: the grammar is a superset of the language, and
/// judging a program is a different command's job.
#[test]
fn fmt_formats_a_program_that_does_not_check() {
    let root = project(
        "fmt-uncheckable",
        &[("bad.hk", "command  C(){emit @never.declared{x}}\n")],
    );
    let formatted = run(&root, &["fmt"]);
    assert!(formatted.status.success(), "{}", stderr(&formatted));
    let checked = run(&root, &["check"]);
    assert!(!checked.status.success(), "the event really is undeclared");
}

/// `--check` names `fmt`'s flag, and `check` is already a command, so the pair is worth a
/// sentence rather than a silent no-op.
#[test]
fn check_with_the_fmt_flag_says_so() {
    let root = project(
        "fmt-misuse",
        &[("a.hk", "record Item {\n  sku: String,\n}\n")],
    );
    let output = run(&root, &["check", "--check"]);
    assert!(!output.status.success());
    assert!(
        stderr(&output).contains("`--check` belongs to `fmt`"),
        "{}",
        stderr(&output)
    );
}

/// Run `hek` with something on stdin, which is how an editor calls a formatter.
fn hek_stdin(args: &[&str], input: &str) -> Output {
    let mut child = Command::new(env!("CARGO_BIN_EXE_hek"))
        .args(args)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("spawn hek");
    let mut stdin = child.stdin.take().expect("a piped stdin");
    // A subcommand that rejects `-` outright answers and exits without reading, which closes
    // the pipe: losing this write is that answer arriving first, not a failure. Anything else
    // still panics.
    match stdin.write_all(input.as_bytes()) {
        Ok(()) => {}
        Err(err) if err.kind() == io::ErrorKind::BrokenPipe => {}
        Err(err) => panic!("write to stdin: {err}"),
    }
    drop(stdin);
    child.wait_with_output().expect("wait for hek")
}

/// `hek fmt -` is the editor shape: one module in, the formatted module out.
#[test]
fn fmt_reads_a_module_from_stdin() {
    let output = hek_stdin(&["fmt", "-"], "record  Item{sku:String}\n");
    assert!(output.status.success(), "{}", stderr(&output));
    assert_eq!(stdout(&output), "record Item {\n  sku: String,\n}\n");
}

/// **The one that matters.** An editor replaces the buffer with what the formatter writes,
/// so printing nothing and succeeding would empty the file the moment it stopped parsing
/// mid-edit. It has to fail instead, with stdout untouched.
#[test]
fn fmt_from_stdin_fails_rather_than_emptying_an_unparseable_module() {
    let output = hek_stdin(&["fmt", "-"], "record Item {\n");
    assert!(
        !output.status.success(),
        "a module that does not parse must fail"
    );
    assert_eq!(
        stdout(&output),
        "",
        "stdout must stay empty, or the editor writes it"
    );
    assert!(
        stderr(&output).contains("not a `.hk` module that parses"),
        "{}",
        stderr(&output)
    );
}

/// `--check` still answers with its status alone, so it composes in a hook.
#[test]
fn fmt_check_from_stdin_answers_with_its_status() {
    let tidy = hek_stdin(
        &["fmt", "--check", "-"],
        "record Item {\n  sku: String,\n}\n",
    );
    assert!(tidy.status.success());
    assert_eq!(stdout(&tidy), "");

    let messy = hek_stdin(&["fmt", "--check", "-"], "record  Item{sku:String}\n");
    assert!(
        !messy.status.success(),
        "a module that would change fails the gate"
    );
}

/// A program is a directory, so the other commands say what to hand them instead.
#[test]
fn check_from_stdin_says_a_program_is_a_directory() {
    let output = hek_stdin(&["check", "-"], "record Item {\n  sku: String,\n}\n");
    assert!(!output.status.success());
    assert!(
        stderr(&output).contains("`-` formats one module"),
        "{}",
        stderr(&output)
    );
}
