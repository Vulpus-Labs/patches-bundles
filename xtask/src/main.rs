use std::{env, fs, path::PathBuf, process::Command};

fn main() {
    let cmd = env::args().nth(1).unwrap_or_else(|| "package".into());
    match cmd.as_str() {
        "package" | "release" => package(),
        "help" | "--help" | "-h" => usage(),
        other => {
            eprintln!("xtask: unknown subcommand `{other}`");
            usage();
            std::process::exit(2);
        }
    }
}

fn usage() {
    eprintln!("usage: cargo xtask <package|release>");
    eprintln!("  builds the workspace in release mode and stages cdylibs into");
    eprintln!("  release/plugins/ with a .pxm extension");
}

fn package() {
    let root = workspace_root();
    let cargo = env::var("CARGO").unwrap_or_else(|_| "cargo".into());

    let status = Command::new(&cargo)
        .args([
            "build",
            "--release",
            "--workspace",
            "--exclude",
            "xtask",
        ])
        .current_dir(&root)
        .status()
        .expect("failed to invoke cargo");
    if !status.success() {
        std::process::exit(status.code().unwrap_or(1));
    }

    let out_dir = root.join("release").join("plugins");
    fs::create_dir_all(&out_dir).expect("create release/plugins");

    let target_dir = root.join("target").join("release");
    let (strip_prefix, dylib_ext): (&str, &str) = if cfg!(target_os = "windows") {
        ("", "dll")
    } else if cfg!(target_os = "macos") {
        ("lib", "dylib")
    } else {
        ("lib", "so")
    };

    let mut copied = 0usize;
    for entry in fs::read_dir(&target_dir).expect("read target/release") {
        let path = entry.expect("dir entry").path();
        if !path.is_file() {
            continue;
        }
        let Some(ext) = path.extension().and_then(|s| s.to_str()) else {
            continue;
        };
        if ext != dylib_ext {
            continue;
        }
        let Some(stem) = path.file_stem().and_then(|s| s.to_str()) else {
            continue;
        };
        let base = stem.strip_prefix(strip_prefix).unwrap_or(stem);
        if !base.starts_with("patches_") {
            continue;
        }
        let dest = out_dir.join(format!("{base}.pxm"));
        fs::copy(&path, &dest).expect("copy dylib");
        println!("packaged: {} -> {}", path.display(), dest.display());
        copied += 1;
    }

    if copied == 0 {
        eprintln!("warning: no plugin artefacts found in {}", target_dir.display());
        std::process::exit(1);
    }
    println!("{copied} plugin(s) staged in {}", out_dir.display());
}

fn workspace_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .expect("xtask must live one level below workspace root")
        .to_path_buf()
}
