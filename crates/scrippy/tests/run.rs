use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};
use std::time::{SystemTime, UNIX_EPOCH};

fn temp_path(name: &str) -> PathBuf {
    let stamp = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos();
    std::env::temp_dir().join(format!(
        "scrippy-test-{}-{}-{name}",
        std::process::id(),
        stamp
    ))
}

fn scrippy() -> &'static str {
    env!("CARGO_BIN_EXE_scrippy")
}

fn run(source: &str) -> Output {
    let dir = temp_path("run");
    fs::create_dir_all(&dir).expect("create temp dir");
    let file = dir.join("program.scp");
    fs::write(&file, source).expect("write script");
    let output = Command::new(scrippy())
        .arg("run")
        .arg(&file)
        .output()
        .expect("run scrippy");
    let _ = fs::remove_dir_all(&dir);
    output
}

fn stdout(output: &Output) -> String {
    String::from_utf8_lossy(&output.stdout).replace("\r\n", "\n")
}

fn stderr(output: &Output) -> String {
    String::from_utf8_lossy(&output.stderr).replace("\r\n", "\n")
}

#[test]
fn runs_a_program() {
    let output = run(r#"
fun greet(name, greeting = "Hello") {
    return "${greeting}, ${name}!"
}

let total = 0
for n in 1..3 {
    total += n
}
print(greet("world"))
print("total=${total}")
"#);
    assert!(output.status.success(), "{}", stderr(&output));
    assert_eq!(stdout(&output), "Hello, world!\ntotal=6\n");
}

#[test]
fn closures_keep_their_frames() {
    let output = run(r#"
fun counter(start) {
    let value = start
    return fun(step = 1) {
        value += step
        return value
    }
}

let c = counter(10)
print(c())
print(c(5))
"#);
    assert!(output.status.success(), "{}", stderr(&output));
    assert_eq!(stdout(&output), "11\n16\n");
}

#[test]
fn collections_and_strings() {
    let output = run(r#"
let scores = { alice: 3, bob: 7 }
scores["carol"] = 9
print(scores.keys().join(","))
print(scores.get("bob"))
print(scores.missing ?? 0)
print("  Hello  ".trim().lower())
let items = [3, 1, 2]
items.sort()
print(items.join("-"))
print("a,b,,c".split(",").size())
"#);
    assert!(output.status.success(), "{}", stderr(&output));
    assert_eq!(stdout(&output), "alice,bob,carol\n7\n0\nhello\n1-2-3\n4\n");
}

#[test]
fn continue_keeps_the_iterated_value_alive() {
    // Lots of iterations with one allocation each, so the collector runs
    // while the loop is still walking an expression result.
    let output = run(r#"
let odd = 0
for i in 0..<60000 {
    let tag = "n${i}"
    if i % 2 == 0 {
        continue
    }
    odd += 1
}
print(odd)
"#);
    assert!(output.status.success(), "{}", stderr(&output));
    assert_eq!(stdout(&output), "30000\n");
}

#[test]
fn integer_comparisons_are_exact() {
    let output = run(r#"
print(9223372036854775806 == 9223372036854775807)
print(9223372036854775806 < 9223372036854775807)
print(9223372036854775807 > 9223372036854775806)
"#);
    assert!(output.status.success(), "{}", stderr(&output));
    assert_eq!(stdout(&output), "false\ntrue\ntrue\n");
}

#[test]
fn substring_end_matches_the_builtin() {
    let output = run(r#"
let s = "hello"
print("[${substring(s, 0, -1)}]")
print("[${s.substring(0, -1)}]")
print("[${s.substring(1, -3)}]")
"#);
    assert!(output.status.success(), "{}", stderr(&output));
    assert_eq!(stdout(&output), "[]\n[]\n[]\n");
}

#[test]
fn map_calls_fall_back_to_the_key() {
    let output = run(r#"
let m = { f: fun() { return "called" } }
let g = m.f()
print(typeof(g))
print(g())
"#);
    assert!(output.status.success(), "{}", stderr(&output));
    assert_eq!(stdout(&output), "function\ncalled\n");
}

#[test]
fn a_function_named_main_shows_in_the_trace() {
    let output = run(r#"
fun main() {
    return 1 / 0
}
print(main())
"#);
    assert!(!output.status.success());
    let err = stderr(&output);
    assert!(err.contains("program.scp:3:12"), "{err}");
    assert!(err.contains("at main ("), "{err}");
}

#[test]
fn runtime_errors_have_a_location() {
    let output = run(r#"
fun divide(a, b) {
    return a / b
}
print(divide(1, 0))
"#);
    assert!(!output.status.success());
    let err = stderr(&output);
    assert!(err.contains("division by zero"), "{err}");
    assert!(err.contains("program.scp:3:12"), "{err}");
    assert!(err.contains("at divide"), "{err}");
}

#[test]
fn compile_errors_happen_before_the_c_compiler() {
    let output = run("print(missing)\n");
    assert!(!output.status.success());
    let err = stderr(&output);
    assert!(
        err.contains("unknown variable or function `missing`"),
        "{err}"
    );
    assert!(err.contains("program.scp:1:7"), "{err}");
}

#[test]
fn host_objects_are_rejected() {
    let output = run("let stack = new ItemStack(1)\n");
    assert!(!output.status.success());
    assert!(stderr(&output).contains("needs a host"));
}

#[test]
fn check_and_emit_c() {
    let dir = temp_path("check");
    fs::create_dir_all(&dir).expect("create temp dir");
    let file = dir.join("program.scp");
    fs::write(&file, "print(\"hi\")\n").expect("write script");

    let check = Command::new(scrippy())
        .arg("check")
        .arg(&file)
        .output()
        .expect("check");
    assert!(check.status.success(), "{}", stderr(&check));
    assert!(stdout(&check).contains("ok"));

    let emitted = Command::new(scrippy())
        .arg("emit-c")
        .arg(&file)
        .output()
        .expect("emit");
    assert!(emitted.status.success(), "{}", stderr(&emitted));
    let c = String::from_utf8_lossy(&emitted.stdout);
    assert!(c.contains("int main(void)"));
    assert!(c.contains("rt_init("));

    let _ = fs::remove_dir_all(&dir);
}

#[test]
fn builds_a_native_binary() {
    let dir = temp_path("build");
    fs::create_dir_all(&dir).expect("create temp dir");
    let file = dir.join("program.scp");
    let binary = dir.join(if cfg!(windows) { "hello.exe" } else { "hello" });
    fs::write(&file, "print(\"built\")\n").expect("write script");

    let build = Command::new(scrippy())
        .arg("build")
        .arg(&file)
        .arg("-o")
        .arg(&binary)
        .output()
        .expect("build");
    assert!(build.status.success(), "{}", stderr(&build));
    assert!(Path::new(&binary).exists());

    let run = Command::new(&binary).output().expect("run binary");
    assert!(run.status.success());
    assert_eq!(stdout(&run), "built\n");

    let _ = fs::remove_dir_all(&dir);
}

#[test]
fn defaults_see_earlier_parameters() {
    let output = run(r#"
fun label(name, prefix = name.upper(), suffix = "!") {
    return "${prefix}: ${name}${suffix} (${args.size()})"
}

print(label("bob"))
print(label("bob", "hi", "?"))
"#);
    assert!(output.status.success(), "{}", stderr(&output));
    assert_eq!(stdout(&output), "BOB: bob! (1)\nhi: bob? (3)\n");
}

#[test]
fn constants_and_parameters_are_read_only() {
    let output = run(r#"
fun bump(x) {
    x = x + 1
    return x
}
print(bump(1))
"#);
    assert!(!output.status.success());
    let err = stderr(&output);
    assert!(err.contains("read-only `x`"), "{err}");
    assert!(err.contains("program.scp:3:5"), "{err}");
}
