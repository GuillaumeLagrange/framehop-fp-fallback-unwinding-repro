# framehop aarch64 SP re-anchor — minimal reproduction

A small reproduction of the framehop aarch64 stack-unwinding bug fixed by the SP re-anchoring change, and a demonstration that the fix repairs it.

## The setup

Three functions form the call chain (caller → callee):

```
main (Rust)  →  dwarf_fn (C)  →  nofp_fn (C)  →  capture (Rust)
```

The two C functions are compiled in **separate translation units with different
unwind-table flags** (see `build.rs`), reproducing the heterogeneous-binary situation
that triggers the bug:

| function   | `.eh_frame`?                                                  | prologue (gcc -O2 -fno-omit-frame-pointer)                            | frame record              | consequence                                                                  |
| ---------- | ------------------------------------------------------------- | --------------------------------------------------------------------- | ------------------------- | ---------------------------------------------------------------------------- |
| `nofp_fn`  | **no** (`-fno-asynchronous-unwind-tables -fno-unwind-tables`) | `stp x29,x30,[sp,#-80]!; mov x29,sp`                                  | at `sp+0`, `x29 = CFA-80` | framehop falls back to `new_sp = fp + 16`, which is **0x40 too low**         |
| `dwarf_fn` | **yes** (`-fasynchronous-unwind-tables`)                      | `stp x29,x30,[sp,#-32]!; mov x29,sp` → `CFA=sp+32, x29@sp+0, ra@sp+8` | at `sp+0`, `x29 = sp`     | a real **SP-relative** DWARF rule — mirrors glib's `g_main_context_dispatch` |

`capture` reads the live `(pc, sp, fp, lr)` and unwinds its own process with framehop.
Unwinding `nofp_fn` (no CFI) yields an SP that is 0x40 too low; `dwarf_fn`'s
SP-relative rule then reads a garbage return address from the wrong slot — unless
framehop re-anchors the unreliable SP onto the frame pointer (`sp = fp - fp_offset`).

## What it shows

The harness unwinds the **same live stack** with **two framehop revisions**, imported
as renamed git dependencies (see `Cargo.toml`):

- `framehop_nofix` = `3a47fc0` (before the re-anchor)
- `framehop_fixed` = `cc3f786` (with the re-anchor)

```
cargo run
```

```
--- WITHOUT the fix  (framehop @ 3a47fc0) ---
  0  ...  framehop_reanchor_repro::capture
  1  ...  nofp_fn
  2  ...  dwarf_fn
  => 3 frames; reached `main` = false        # garbage SP → main is LOST

--- WITH the fix     (framehop @ cc3f786) ---
  0  ...  framehop_reanchor_repro::capture
  1  ...  nofp_fn
  2  ...  dwarf_fn
  3  ...  framehop_reanchor_repro::main
  ...                                         # std::rt frames
  9  ...  main
  => 10 frames; reached `main` = true         # SP re-anchored → correct
```

The unwind stops at `main` (this is a bug reproduction, not a profiler), so the
libc frames below `main` are not walked.

## Requirements

- aarch64 Linux, `gcc` (for the C TUs), a Rust toolchain, and network access (to fetch
  the two framehop git revisions on first build).
- Frame pointers are forced on the Rust frames via `.cargo/config.toml`.
