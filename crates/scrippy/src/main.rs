mod codegen;

use std::env;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, ExitCode, Stdio};
use std::time::{SystemTime, UNIX_EPOCH};

const HELP: &str = "\
scrippy - compile and run ScrippyScript (.scp) programs

usage:
  scrippy run <file.scp>            compile and run
  scrippy build <file.scp> [-o path] [--keep-c]
                                   compile to a native executable
  scrippy check <file.scp>          parse and check the core language only
  scrippy emit-c <file.scp> [-o path]
                                   write the generated C

options:
  -o, --output <path>   output file for build or emit-c
  --keep-c              keep the generated C next to the script or output
  -h, --help            show this help
  -V, --version         show the version

build needs a C compiler. It uses $CC when set, otherwise `cc`.
";

struct Options {
    file: PathBuf,
    output: Option<PathBuf>,
    keep_c: bool,
}

fn main() -> ExitCode {
    match run() {
        Ok(code) => ExitCode::from(code),
        Err(message) => {
            eprintln!("scrippy: {message}");
            ExitCode::FAILURE
        }
    }
}

fn run() -> Result<u8, String> {
    let mut args = env::args().skip(1);
    let Some(command) = args.next() else {
        print!("{HELP}");
        return Ok(0);
    };

    match command.as_str() {
        "-h" | "--help" | "help" => {
            print!("{HELP}");
            Ok(0)
        }
        "-V" | "--version" | "version" => {
            println!("scrippy {}", env!("CARGO_PKG_VERSION"));
            Ok(0)
        }
        "run" | "build" | "check" | "emit-c" => {
            let options = parse_options(args.collect())?;
            match command.as_str() {
                "run" => run_file(&options),
                "build" => build_file(&options),
                "check" => check_file(&options),
                "emit-c" => emit_file(&options),
                _ => unreachable!(),
            }
        }
        other => Err(format!("unknown command `{other}`; try `scrippy --help`")),
    }
}

fn parse_options(args: Vec<String>) -> Result<Options, String> {
    let mut file = None;
    let mut output = None;
    let mut keep_c = false;
    let mut index = 0;

    while index < args.len() {
        match args[index].as_str() {
            "-o" | "--output" => {
                index += 1;
                let Some(path) = args.get(index) else {
                    return Err("`-o` needs a path".to_string());
                };
                output = Some(PathBuf::from(path));
            }
            "--keep-c" => keep_c = true,
            "-h" | "--help" => {
                print!("{HELP}");
                std::process::exit(0);
            }
            value if value.starts_with('-') => {
                return Err(format!("unknown option `{value}`"));
            }
            value => {
                if file.is_some() {
                    return Err(format!("unexpected argument `{value}`"));
                }
                file = Some(PathBuf::from(value));
            }
        }
        index += 1;
    }

    let Some(file) = file else {
        return Err("no input file; try `scrippy --help`".to_string());
    };
    Ok(Options {
        file,
        output,
        keep_c,
    })
}

fn load(options: &Options) -> Result<(String, String), String> {
    let source = fs::read_to_string(&options.file)
        .map_err(|err| format!("cannot read {}: {err}", options.file.display()))?;
    let script = scrippy_script_core::parse(&source).map_err(|errors| {
        let mut text = String::new();
        for error in errors {
            text.push_str(&format!(
                "{}:{}\n",
                options.file.display(),
                error.render(&source)
            ));
        }
        text.trim_end().to_string()
    })?;
    let generated = codegen::generate(&script, &source, &display_path(&options.file))
        .map_err(|error| format!("{}:{}", options.file.display(), error.render(&source)))?;
    Ok((source, generated))
}

fn check_file(options: &Options) -> Result<u8, String> {
    if options.output.is_some() || options.keep_c {
        return Err("check does not write files".to_string());
    }
    let (_, _) = load(options)?;
    println!("{}: ok", options.file.display());
    Ok(0)
}

fn emit_file(options: &Options) -> Result<u8, String> {
    if options.keep_c {
        return Err("`--keep-c` is only for run and build".to_string());
    }
    let (_, generated) = load(options)?;
    if let Some(path) = &options.output {
        fs::write(path, generated)
            .map_err(|err| format!("cannot write {}: {err}", path.display()))?;
    } else {
        print!("{generated}");
    }
    Ok(0)
}

fn run_file(options: &Options) -> Result<u8, String> {
    if options.output.is_some() {
        return Err("`-o` is only for build and emit-c".to_string());
    }
    let (_, generated) = load(options)?;
    let directory = temp_dir("run")?;
    fs::create_dir_all(&directory)
        .map_err(|err| format!("cannot create {}: {err}", directory.display()))?;
    let source = directory.join("program.c");
    let binary = directory.join(if cfg!(windows) {
        "program.exe"
    } else {
        "program"
    });

    let result = (|| {
        fs::write(&source, generated)
            .map_err(|err| format!("cannot write {}: {err}", source.display()))?;
        compile(&source, &binary)?;
        let status = Command::new(&binary)
            .stdin(Stdio::inherit())
            .stdout(Stdio::inherit())
            .stderr(Stdio::inherit())
            .status()
            .map_err(|err| format!("cannot run {}: {err}", binary.display()))?;
        Ok(status.code().unwrap_or(1).clamp(0, 255) as u8)
    })();

    if options.keep_c {
        let kept = Path::new(&display_path(&options.file)).with_extension("c");
        if let Err(err) = fs::copy(&source, &kept) {
            eprintln!("could not keep the generated C: {err}");
        } else {
            eprintln!("kept generated C at {}", kept.display());
        }
    }
    let _ = fs::remove_dir_all(&directory);
    result
}

fn build_file(options: &Options) -> Result<u8, String> {
    let (_, generated) = load(options)?;
    let output = match &options.output {
        Some(path) => path.clone(),
        None => {
            let stem = options
                .file
                .file_stem()
                .ok_or_else(|| "input file has no name".to_string())?;
            let mut path = PathBuf::from(stem);
            if cfg!(windows) {
                path.set_extension("exe");
            }
            path
        }
    };

    let source = if options.keep_c {
        output.with_extension("c")
    } else {
        let directory = temp_dir("build")?;
        fs::create_dir_all(&directory)
            .map_err(|err| format!("cannot create {}: {err}", directory.display()))?;
        directory.join("program.c")
    };

    fs::write(&source, generated)
        .map_err(|err| format!("cannot write {}: {err}", source.display()))?;
    let result = compile(&source, &output);

    if !options.keep_c {
        if let Some(parent) = source.parent() {
            let _ = fs::remove_dir_all(parent);
        }
    }
    result?;
    Ok(0)
}

fn compile(source: &Path, output: &Path) -> Result<(), String> {
    let compiler = env::var_os("CC").unwrap_or_else(|| "cc".into());
    let status = Command::new(&compiler)
        .arg("-std=c11")
        .arg("-O2")
        .arg("-o")
        .arg(output)
        .arg(source)
        .arg("-lm")
        .status()
        .map_err(|err| {
            format!(
                "cannot run the C compiler `{}`: {err}\ninstall a C compiler or set CC",
                compiler.to_string_lossy()
            )
        })?;
    if !status.success() {
        return Err("the C compiler failed".to_string());
    }
    Ok(())
}

fn temp_dir(kind: &str) -> Result<PathBuf, String> {
    let stamp = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos();
    Ok(env::temp_dir().join(format!("scrippy-{kind}-{}-{stamp}", std::process::id())))
}

fn display_path(path: &Path) -> String {
    path.display().to_string()
}
