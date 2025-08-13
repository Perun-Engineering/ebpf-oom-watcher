use std::env;
use std::fs;
use std::path::PathBuf;
use std::process::Command;

fn main() {
    // Allow skipping the nested eBPF build to avoid deadlocking on Cargo's global build lock
    if std::env::var("OOM_WATCHER_SKIP_EBPF").ok().as_deref() == Some("1") {
        println!("cargo:warning=OOM_WATCHER_SKIP_EBPF=1 set — skipping eBPF build in build.rs");
        return;
    }
    let out_dir = PathBuf::from(env::var("OUT_DIR").unwrap());
    println!("cargo:rerun-if-changed=oom-watcher-ebpf/src");

    // Get the workspace root directory
    let workspace_root = env::var("CARGO_MANIFEST_DIR")
        .map(PathBuf::from)
        .unwrap()
        .parent()
        .unwrap()
        .to_path_buf();

    println!("cargo:warning=Building eBPF program...");

    // Build the eBPF program
    let mut cmd = Command::new("cargo");
    cmd.arg("+nightly")
        .arg("build")
        .arg("--release")
        .arg("--package")
        .arg("oom-watcher-ebpf")
        .arg("--target")
        .arg("bpfel-unknown-none")
        .arg("-Z")
        .arg("build-std=core")
        .current_dir(&workspace_root);

    println!("cargo:warning=Running command: {cmd:?}");

    let output = cmd.output().expect("Failed to execute eBPF build command");

    if !output.status.success() {
        eprintln!("eBPF build failed!");
        eprintln!("stdout: {}", String::from_utf8_lossy(&output.stdout));
        eprintln!("stderr: {}", String::from_utf8_lossy(&output.stderr));
        panic!("eBPF build failed");
    }

    println!("cargo:warning=eBPF build completed successfully");

    // Look for the eBPF binary in multiple possible locations
    let possible_paths = vec![
        workspace_root.join("target/bpfel-unknown-none/release/oom-watcher-ebpf"),
        PathBuf::from("../target/bpfel-unknown-none/release/oom-watcher-ebpf"),
        PathBuf::from("target/bpfel-unknown-none/release/oom-watcher-ebpf"),
    ];

    let mut found_path = None;
    for path in &possible_paths {
        if path.exists() {
            found_path = Some(path.clone());
            break;
        }
    }

    if let Some(ebpf_path) = found_path {
        println!(
            "cargo:warning=Found eBPF binary at: {}",
            ebpf_path.display()
        );

        // Copy it to OUT_DIR
        let target_path = out_dir.join("oom-watcher-ebpf-object");
        fs::copy(&ebpf_path, &target_path).unwrap_or_else(|_| {
            panic!(
                "Failed to copy {} to {}",
                ebpf_path.display(),
                target_path.display()
            )
        });

        println!(
            "cargo:warning=Copied eBPF object to: {}",
            target_path.display()
        );
    } else {
        // List what we have in the target directories for debugging
        println!("cargo:warning=Could not find eBPF binary in any of these locations:");
        for path in &possible_paths {
            println!("cargo:warning=  {}", path.display());
        }

        // List contents of workspace target directory
        let workspace_target = workspace_root.join("target/bpfel-unknown-none/release/");
        println!("cargo:warning=Contents of {}:", workspace_target.display());
        if let Ok(entries) = fs::read_dir(&workspace_target) {
            for entry in entries.flatten() {
                println!("cargo:warning=  {}", entry.file_name().to_string_lossy());
            }
        }

        panic!("eBPF binary not found");
    }
}
