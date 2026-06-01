use std::env;
use std::path::PathBuf;
use std::process::Command;

// Compile the two C translation units with DIFFERENT unwind-table flags, so that one
// (nofp_tu) has no .eh_frame and the other (dwarf_tu) does -- reproducing the
// heterogeneous-binary situation that triggers the bug. We invoke gcc directly for
// full control over the flags (the `cc` crate applies one flag set to all files).
fn main() {
    let out = PathBuf::from(env::var("OUT_DIR").unwrap());
    let common = [
        "-O2",
        "-fno-omit-frame-pointer",
        "-mbranch-protection=none",
        "-fno-stack-protector",
    ];

    // nofp_tu: NO unwind tables -> no .eh_frame -> framehop uses the fp-fallback rule.
    compile(
        "csrc/nofp_tu.c",
        &out.join("nofp_tu.o"),
        &common,
        &["-fno-asynchronous-unwind-tables", "-fno-unwind-tables"],
    );
    // dwarf_tu: WITH unwind tables -> real DWARF CFI (SP-relative CFA).
    compile(
        "csrc/dwarf_tu.c",
        &out.join("dwarf_tu.o"),
        &common,
        &["-fasynchronous-unwind-tables"],
    );

    let lib = out.join("libcframes.a");
    let _ = std::fs::remove_file(&lib);
    run(Command::new("ar")
        .arg("rcs")
        .arg(&lib)
        .arg(out.join("nofp_tu.o"))
        .arg(out.join("dwarf_tu.o")));

    println!("cargo:rustc-link-search=native={}", out.display());
    println!("cargo:rustc-link-lib=static=cframes");
    println!("cargo:rerun-if-changed=csrc/nofp_tu.c");
    println!("cargo:rerun-if-changed=csrc/dwarf_tu.c");
}

fn compile(src: &str, obj: &std::path::Path, common: &[&str], extra: &[&str]) {
    run(Command::new("gcc")
        .args(common)
        .args(extra)
        .arg("-c")
        .arg(src)
        .arg("-o")
        .arg(obj));
}

fn run(cmd: &mut Command) {
    let status = cmd.status().expect("failed to spawn compiler/ar");
    assert!(status.success(), "command failed: {:?}", cmd);
}
