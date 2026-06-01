//! Minimal, real (compiled, in-process) reproduction of the framehop aarch64
//! SP re-anchoring fix. It unwinds the SAME live stack with TWO framehop revisions
//! -- one without the fix, one with it -- and prints both results side by side.
//!
//! Call chain (caller -> callee):
//!
//!     main (Rust)  ->  dwarf_fn (C, has .eh_frame, SP-relative CFA = sp+32)
//!                  ->  nofp_fn  (C, NO .eh_frame, 80-byte frame -> fp-fallback)
//!                  ->  capture  (Rust): takes the "sample" and unwinds.
//!
//! Unwinding from `capture` outward, framehop reaches `nofp_fn`, which has no unwind
//! info, so it uses the frame-pointer fallback `new_sp = fp + 16`. nofp_fn's frame
//! record sits at the bottom of an 80-byte frame, so that SP is 0x40 too low. The
//! next frame, `dwarf_fn`, has a real SP-relative DWARF rule and reads a garbage
//! return address from the wrong slot -- losing `main` -- unless framehop re-anchors
//! the unreliable SP onto the frame pointer.

use object::{Object, ObjectSection, ObjectSegment, ObjectSymbol};

extern "C" {
    fn dwarf_fn(cb: extern "C" fn());
}

fn main() {
    println!("== framehop aarch64 SP re-anchor reproduction (two framehop revisions) ==\n");
    unsafe { dwarf_fn(capture) };
}

/// The innermost "sampled" frame. Reads the live (pc, sp, fp, lr) and unwinds.
extern "C" fn capture() {
    let (sp, fp, lr, pc): (u64, u64, u64, u64);
    unsafe {
        core::arch::asm!(
            "mov {sp}, sp",
            "mov {fp}, x29",
            "mov {lr}, x30",
            "adr {pc}, 1f",
            "1:",
            sp = out(reg) sp, fp = out(reg) fp, lr = out(reg) lr, pc = out(reg) pc,
        );
    }

    let exe = std::fs::read("/proc/self/exe").expect("read /proc/self/exe");
    let obj = object::File::parse(&*exe).expect("parse exe");

    // Load bias: runtime address of dwarf_fn minus its static address (SVMA).
    let dwarf_fn_avma = dwarf_fn as *const () as u64;
    let dwarf_fn_svma = obj
        .symbols()
        .find(|s| s.name() == Ok("dwarf_fn"))
        .expect("dwarf_fn symbol")
        .address();
    let bias = dwarf_fn_avma.wrapping_sub(dwarf_fn_svma);

    // Symbol table for symbolication (crate-independent).
    let mut syms: Vec<(u64, u64, String)> = obj
        .symbols()
        .filter(|s| s.kind() == object::SymbolKind::Text && s.size() > 0)
        .filter_map(|s| {
            s.name()
                .ok()
                .map(|n| (s.address(), s.address() + s.size(), n.to_string()))
        })
        .collect();
    syms.sort_by_key(|t| t.0);
    let symbolicate = |avma: u64| -> String {
        let svma = avma.wrapping_sub(bias);
        match syms.iter().find(|(a, b, _)| svma >= *a && svma < *b) {
            Some((_, _, n)) => clean(n),
            None => "<unknown>".to_string(),
        }
    };

    // Confine stack reads to a window around the captured SP, so a bogus frame
    // pointer (in the un-fixed case) makes framehop stop cleanly, not segfault.
    let lo = sp.saturating_sub(0x1000);
    let hi = sp.saturating_add(0x20_0000);

    // We are reproducing an unwinding bug, not building a profiler: stop the
    // unwind at `main` rather than walking out through std::rt into libc.
    let main_sym = obj
        .symbols()
        .find(|s| s.name() == Ok("main"))
        .expect("main symbol");
    let main_lo = main_sym.address().wrapping_add(bias);
    let main_hi = main_lo + main_sym.size();

    println!("sample:  pc={pc:#x} sp={sp:#x} fp={fp:#x} lr={lr:#x}  bias={bias:#x}\n");

    let (f_nofix, stop_nofix) = unwind_nofix(pc, sp, fp, lr, lo, hi, main_lo, main_hi);
    let (f_fixed, stop_fixed) = unwind_fixed(pc, sp, fp, lr, lo, hi, main_lo, main_hi);

    report(
        "WITHOUT the fix  (framehop @ 3a47fc0)",
        &f_nofix,
        &stop_nofix,
        &symbolicate,
    );
    report(
        "WITH the fix     (framehop @ cc3f786)",
        &f_fixed,
        &stop_fixed,
        &symbolicate,
    );
}

fn report(title: &str, frames: &[u64], stop: &Option<String>, sym: &dyn Fn(u64) -> String) {
    println!("--- {title} ---");
    for (i, &a) in frames.iter().enumerate() {
        println!("  {i:<2} {a:#018x}  {}", sym(a));
    }
    if let Some(s) = stop {
        println!("     (unwinding stopped: {s})");
    }
    let reached_main = frames.iter().any(|&a| sym(a) == "main");
    println!(
        "  => {} frames; reached `main` = {}\n",
        frames.len(),
        reached_main
    );
}

fn clean(n: &str) -> String {
    rustc_demangle::demangle(n).to_string()
}

/// Generate an unwinding entry point bound to one framehop crate. The two framehop
/// revisions have distinct `Module`/`ModuleSectionInfo`/`Unwinder` types, so the
/// harness is instantiated once per crate.
macro_rules! gen_unwind {
    ($name:ident, $fh:ident) => {
        fn $name(
            pc: u64,
            sp: u64,
            fp: u64,
            lr: u64,
            lo: u64,
            hi: u64,
            main_lo: u64,
            main_hi: u64,
        ) -> (Vec<u64>, Option<String>) {
            use $fh::aarch64::{CacheAarch64, UnwindRegsAarch64, UnwinderAarch64};
            use $fh::{FrameAddress, Module, ModuleSectionInfo, Unwinder};

            type Item = (Vec<u8>, core::ops::Range<u64>, Vec<u8>);
            struct Info {
                sections: Vec<Item>,
                segments: Vec<Item>,
                base_svma: u64,
            }
            impl ModuleSectionInfo<Vec<u8>> for Info {
                fn base_svma(&self) -> u64 {
                    self.base_svma
                }
                fn section_svma_range(&mut self, name: &[u8]) -> Option<core::ops::Range<u64>> {
                    self.sections
                        .iter()
                        .find(|(n, _, _)| n == name)
                        .map(|(_, r, _)| r.clone())
                }
                fn section_data(&mut self, name: &[u8]) -> Option<Vec<u8>> {
                    self.sections
                        .iter()
                        .find(|(n, _, _)| n == name)
                        .map(|(_, _, d)| d.clone())
                }
                fn segment_svma_range(&mut self, name: &[u8]) -> Option<core::ops::Range<u64>> {
                    self.segments
                        .iter()
                        .find(|(n, _, _)| n == name)
                        .map(|(_, r, _)| r.clone())
                }
                fn segment_data(&mut self, name: &[u8]) -> Option<Vec<u8>> {
                    self.segments
                        .iter()
                        .find(|(n, _, _)| n == name)
                        .map(|(_, _, d)| d.clone())
                }
            }

            let mut unwinder: UnwinderAarch64<Vec<u8>> = UnwinderAarch64::new();

            // Load every mapped file into the unwinder so glibc / ld frames unwind cleanly.
            let maps = std::fs::read_to_string("/proc/self/maps").unwrap_or_default();
            let mut loaded: std::collections::HashSet<String> = std::collections::HashSet::new();
            for line in maps.lines() {
                // Format: <start>-<end> <perms> <offset> <dev> <inode> <path>
                let mut fields = line.splitn(6, ' ');
                let range_field = fields.next().unwrap_or("");
                let perms = fields.next().unwrap_or("");
                let _offset = fields.next().unwrap_or("");
                let _dev = fields.next().unwrap_or("");
                let _inode = fields.next().unwrap_or("");
                let path = fields.next().unwrap_or("").trim();

                // Only executable mappings backed by a real file.
                if !perms.contains('x') || !path.starts_with('/') {
                    continue;
                }
                if !loaded.insert(path.to_string()) {
                    continue;
                }

                let avma_start =
                    u64::from_str_radix(range_field.split('-').next().unwrap_or("0"), 16)
                        .unwrap_or(0);

                let bytes = match std::fs::read(path) {
                    Ok(b) => b,
                    Err(_) => continue,
                };
                let obj = match object::File::parse(bytes.as_slice()) {
                    Ok(o) => o,
                    Err(_) => continue,
                };

                // bias = first executable mapping start - ELF load address of that segment
                let base_svma = obj.relative_address_base();
                let bias = avma_start.wrapping_sub(base_svma);
                let file_avma_start = base_svma.wrapping_add(bias);
                let file_avma_end = file_avma_start + bytes.len() as u64;

                let sections = obj
                    .sections()
                    .filter_map(|s| {
                        Some((
                            s.name_bytes().ok()?.to_vec(),
                            s.address()..s.address() + s.size(),
                            s.data().ok()?.to_vec(),
                        ))
                    })
                    .collect();
                let segments = obj
                    .segments()
                    .filter_map(|s| {
                        Some((
                            s.name_bytes().ok()??.to_vec(),
                            s.address()..s.address() + s.size(),
                            s.data().ok()?.to_vec(),
                        ))
                    })
                    .collect();
                let info = Info {
                    sections,
                    segments,
                    base_svma,
                };

                unwinder.add_module(Module::new(
                    path.to_string(),
                    file_avma_start..file_avma_end,
                    file_avma_start,
                    info,
                ));
            }
            let mut cache = CacheAarch64::<_>::new();
            let mut read_stack = |addr: u64| -> Result<u64, ()> {
                if addr % 8 != 0 || addr < lo || addr + 8 > hi {
                    return Err(());
                }
                Ok(unsafe { (addr as *const u64).read() })
            };

            let mut regs = UnwindRegsAarch64::new(lr, sp, fp);
            let mut frames = vec![pc];
            let mut addr = FrameAddress::from_instruction_pointer(pc);
            let mut stop = None;
            loop {
                match unwinder.unwind_frame(addr, &mut regs, &mut cache, &mut read_stack) {
                    Ok(Some(ra)) => {
                        frames.push(ra);
                        // Stop once we reach `main` -- we are reproducing an
                        // unwinding bug, not profiling the whole stack.
                        if ra >= main_lo && ra < main_hi {
                            break;
                        }
                        match FrameAddress::from_return_address(ra) {
                            Some(a) => addr = a,
                            None => break,
                        }
                    }
                    Ok(None) => break,
                    Err(e) => {
                        stop = Some(format!("{:?}", e));
                        break;
                    }
                }
            }
            (frames, stop)
        }
    };
}

gen_unwind!(unwind_fixed, framehop_fixed);
gen_unwind!(unwind_nofix, framehop_nofix);
