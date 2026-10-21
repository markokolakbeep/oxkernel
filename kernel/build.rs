fn main() {
    let dir = std::env::var("CARGO_MANIFEST_DIR").unwrap();
    println!("cargo:rustc-link-arg-bins=-T{dir}/linker.ld");
    println!("cargo:rerun-if-changed=linker.ld");
    for f in ["entry.S", "trap.S", "switch.S"] {
        println!("cargo:rerun-if-changed=src/arch/riscv/{f}");
    }
}
