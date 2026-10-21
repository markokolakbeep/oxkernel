//! `cargo xtask <command>`: build, run, debug and test oxkernel.
//!
//! ```text
//! build     build user programs, the disk image and the kernel
//! run       boot to the shell (QEMU, -nographic; quit with Ctrl-A X)
//! debug     like run, but QEMU waits for gdb on :1234
//! test      host unit tests, in-kernel tests, then usertests on 1 and 4 harts
//! lint      rustfmt --check and clippy (warnings are errors) on every crate
//! bench     boot and run /bin/bench, then power off
//! addr2line resolve kernel addresses from a panic backtrace
//!
//! options:  --release   --smp N   --mem SIZE
//! ```

use std::io::{BufRead, BufReader, Write};
use std::path::{Path, PathBuf};
use std::process::{Command, ExitCode, Stdio};
use std::sync::mpsc;
use std::time::{Duration, Instant};

const TARGET: &str = "riscv64imac-unknown-none-elf";

#[derive(Clone)]
struct Opts {
    release: bool,
    smp: u32,
    mem: String,
    rest: Vec<String>,
}

fn root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).parent().unwrap().to_path_buf()
}

fn target_dir() -> PathBuf {
    root().join("target")
}

fn cargo() -> Command {
    let mut c = Command::new(std::env::var("CARGO").unwrap_or_else(|_| "cargo".into()));
    c.current_dir(root());
    c
}

fn run_cmd(mut c: Command) -> Result<(), String> {
    let desc = format!("{c:?}");
    let status = c.status().map_err(|e| format!("{desc}: {e}"))?;
    if status.success() { Ok(()) } else { Err(format!("{desc} failed with {status}")) }
}

fn user_bins() -> Vec<String> {
    let mut bins: Vec<String> = std::fs::read_dir(root().join("user/src/bin"))
        .unwrap()
        .filter_map(|e| {
            let p = e.ok()?.path();
            (p.extension()? == "rs").then(|| p.file_stem().unwrap().to_string_lossy().into_owned())
        })
        .collect();
    bins.sort();
    bins
}

fn build_user() -> Result<(), String> {
    let mut c = cargo();
    c.args(["build", "-p", "user", "--target", TARGET, "--profile", "user"]);
    run_cmd(c)
}

fn fs_image() -> PathBuf {
    target_dir().join("fs.img")
}

/// A fresh disk image every time, so tests always start from the same state.
fn build_image() -> Result<(), String> {
    let bin_dir = target_dir().join(TARGET).join("user");
    let mut c = cargo();
    c.args(["run", "--quiet", "-p", "mkfs", "--"]).arg(fs_image()).args(["--size-mb", "32"]);
    for b in user_bins() {
        c.arg("--file").arg(format!("/bin/{b}={}", bin_dir.join(&b).display()));
    }
    c.arg("--file").arg(format!("/README={}", root().join("README.md").display()));
    run_cmd(c)
}

fn kernel_path(release: bool, ktest: bool) -> PathBuf {
    let profile = if release { "release" } else { "debug" };
    let p = target_dir().join(TARGET).join(profile).join("kernel");
    if ktest { p.with_file_name("kernel-ktest") } else { p }
}

fn build_kernel(release: bool, ktest: bool) -> Result<PathBuf, String> {
    let mut c = cargo();
    c.args(["build", "-p", "kernel", "--target", TARGET]);
    if release {
        c.arg("--release");
    }
    if ktest {
        c.args(["--features", "ktest"]);
    }
    run_cmd(c)?;
    let built = kernel_path(release, false);
    if ktest {
        // Keep the test kernel separate so it never gets booted by `run`.
        let dst = kernel_path(release, true);
        std::fs::copy(&built, &dst).map_err(|e| e.to_string())?;
        return Ok(dst);
    }
    Ok(built)
}

fn build_all(o: &Opts) -> Result<PathBuf, String> {
    build_user()?;
    build_image()?;
    build_kernel(o.release, false)
}

fn qemu(kernel: &Path, o: &Opts, append: Option<&str>, gdb: bool) -> Command {
    let mut c = Command::new("qemu-system-riscv64");
    c.args(["-machine", "virt", "-bios", "default", "-nographic"]);
    c.args(["-smp", &o.smp.to_string(), "-m", &o.mem]);
    c.arg("-kernel").arg(kernel);
    c.arg("-drive").arg(format!("file={},if=none,format=raw,id=disk0", fs_image().display()));
    c.args(["-device", "virtio-blk-device,drive=disk0,bus=virtio-mmio-bus.0"]);
    c.args(["-global", "virtio-mmio.force-legacy=false"]);
    if let Some(a) = append {
        c.args(["-append", a]);
    }
    if gdb {
        c.args(["-s", "-S"]);
    }
    c
}

fn check_qemu() -> Result<(), String> {
    match Command::new("qemu-system-riscv64").arg("--version").output() {
        Ok(_) => Ok(()),
        Err(_) => Err("qemu-system-riscv64 not found (Arch: pacman -S qemu-system-riscv; \
                       Debian/Ubuntu: apt install qemu-system-misc)"
            .into()),
    }
}

struct Outcome {
    code: Option<i32>,
    output: String,
    timed_out: bool,
}

/// Boots QEMU non-interactively, echoing its console, and kills it after
/// `timeout`.
fn boot_headless(mut c: Command, timeout: Duration) -> Result<Outcome, String> {
    c.stdin(Stdio::null()).stdout(Stdio::piped()).stderr(Stdio::inherit());
    let mut child = c.spawn().map_err(|e| format!("qemu: {e}"))?;
    let stdout = child.stdout.take().unwrap();
    let (tx, rx) = mpsc::channel();
    let reader = std::thread::spawn(move || {
        let mut out = String::new();
        for line in BufReader::new(stdout).lines().map_while(Result::ok) {
            println!("  | {}", line.trim_end_matches('\r'));
            out.push_str(&line);
            out.push('\n');
        }
        let _ = tx.send(out);
    });
    let start = Instant::now();
    let (code, timed_out) = loop {
        if let Some(status) = child.try_wait().map_err(|e| e.to_string())? {
            break (status.code(), false);
        }
        if start.elapsed() > timeout {
            let _ = child.kill();
            let _ = child.wait();
            break (None, true);
        }
        std::thread::sleep(Duration::from_millis(50));
    };
    let _ = reader.join();
    let output = rx.recv().unwrap_or_default();
    std::io::stdout().flush().ok();
    Ok(Outcome { code, output, timed_out })
}

fn cmd_run(o: &Opts, gdb: bool) -> Result<(), String> {
    check_qemu()?;
    let kernel = build_all(o)?;
    if gdb {
        println!("QEMU waits for gdb. In another terminal:");
        println!("  gdb {} -ex 'target remote :1234'", kernel.display());
    }
    println!("Booting oxkernel on {} hart(s). Quit QEMU with Ctrl-A X.", o.smp);
    let append = (!o.rest.is_empty()).then(|| o.rest.join(" "));
    run_cmd(qemu(&kernel, o, append.as_deref(), gdb))
}

fn cmd_test(o: &Opts) -> Result<(), String> {
    println!("== host unit tests (oxcore, mkfs)");
    let mut c = cargo();
    c.args(["test", "-p", "oxcore", "-p", "mkfs"]);
    run_cmd(c)?;

    check_qemu()?;
    build_user()?;
    let mut failures = Vec::new();

    println!("== in-kernel tests (ktest)");
    build_image()?;
    let ktest = build_kernel(o.release, true)?;
    let r = boot_headless(qemu(&ktest, &Opts { smp: 2, ..o.clone() }, None, false), Duration::from_secs(120))?;
    if r.timed_out || r.code != Some(0) || !r.output.contains("ktest: all passed") {
        failures.push(format!("ktest (exit {:?}, timed out: {})", r.code, r.timed_out));
    }

    let kernel = build_kernel(o.release, false)?;
    for smp in [1, 4] {
        println!("== usertests on {smp} hart(s)");
        build_image()?;
        let opts = Opts { smp, ..o.clone() };
        let r = boot_headless(qemu(&kernel, &opts, Some("test"), false), Duration::from_secs(600))?;
        if r.timed_out || r.code != Some(0) || !r.output.contains("ALL TESTS PASSED") {
            failures.push(format!("usertests -smp {smp} (exit {:?}, timed out: {})", r.code, r.timed_out));
        }
    }

    if failures.is_empty() {
        println!("== all tests passed");
        Ok(())
    } else {
        Err(format!("failed: {}", failures.join(", ")))
    }
}

fn cmd_lint() -> Result<(), String> {
    let mut c = cargo();
    c.args(["fmt", "--all", "--check"]);
    run_cmd(c)?;
    let host = ["-p", "oxcore", "-p", "mkfs", "-p", "xtask", "--all-targets"];
    let riscv = ["-p", "kernel", "-p", "user", "--target", TARGET];
    let ktest = ["-p", "kernel", "--target", TARGET, "--features", "ktest"];
    for args in [&host[..], &riscv[..], &ktest[..]] {
        let mut c = cargo();
        c.arg("clippy").args(args).args(["--", "-D", "warnings"]);
        run_cmd(c)?;
    }
    Ok(())
}

fn cmd_bench(o: &Opts) -> Result<(), String> {
    check_qemu()?;
    let kernel = build_all(&Opts { release: true, ..o.clone() })?;
    let r = boot_headless(qemu(&kernel, o, Some("bench"), false), Duration::from_secs(900))?;
    if r.code == Some(0) { Ok(()) } else { Err(format!("bench exited with {:?}", r.code)) }
}

fn cmd_addr2line(o: &Opts) -> Result<(), String> {
    let sysroot = Command::new("rustc").args(["--print", "sysroot"]).output().map_err(|e| e.to_string())?;
    let sysroot = String::from_utf8_lossy(&sysroot.stdout).trim().to_string();
    let tool = Path::new(&sysroot).join("lib/rustlib/x86_64-unknown-linux-gnu/bin/llvm-addr2line");
    let tool = if tool.exists() { tool } else { PathBuf::from("addr2line") };
    let mut c = Command::new(tool);
    c.args(["-f", "-C", "-p", "-e"]).arg(kernel_path(o.release, false)).args(&o.rest);
    run_cmd(c)
}

fn parse(args: &[String]) -> Result<(String, Opts), String> {
    let mut o = Opts { release: false, smp: 4, mem: "256M".into(), rest: Vec::new() };
    let mut it = args.iter();
    let cmd = it.next().cloned().unwrap_or_else(|| "help".into());
    while let Some(a) = it.next() {
        match a.as_str() {
            "--release" => o.release = true,
            "--smp" => o.smp = it.next().and_then(|s| s.parse().ok()).ok_or("--smp needs a number")?,
            "--mem" => o.mem = it.next().cloned().ok_or("--mem needs a size")?,
            _ => o.rest.push(a.clone()),
        }
    }
    Ok((cmd, o))
}

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let result = parse(&args).and_then(|(cmd, o)| match cmd.as_str() {
        "build" => build_all(&o).map(|k| println!("kernel: {}", k.display())),
        "run" => cmd_run(&o, false),
        "debug" => cmd_run(&o, true),
        "test" => cmd_test(&o),
        "lint" => cmd_lint(),
        "bench" => cmd_bench(&o),
        "addr2line" => cmd_addr2line(&o),
        _ => {
            println!(
                "usage: cargo xtask <build|run|debug|test|lint|bench|addr2line> [--release] [--smp N] [--mem SIZE]"
            );
            Ok(())
        }
    });
    match result {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("xtask: {e}");
            ExitCode::FAILURE
        }
    }
}
