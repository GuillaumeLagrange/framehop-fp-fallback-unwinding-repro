/*
 * Compiled WITH unwind tables (has .eh_frame) and a frame pointer.
 *
 * This mirrors glib's g_main_context_dispatch: a real DWARF CFI rule whose CFA is
 * SP-relative (CFA = sp + N). gcc emits:
 *     stp x29,x30,[sp,#-32]! ; mov x29,sp
 *     CFI: CFA = sp+32, x29 @ CFA-32 (= sp+0), ra @ CFA-24 (= sp+8)
 * i.e. exactly glib's shape (x29 = sp, fp stored at sp+0), just with N=32 instead of 144.
 *
 * When its callee (nofp_fn) hands it an SP that is 0x40 too low, this SP-relative rule
 * reads the wrong stack slots and produces a garbage return address.
 */
void nofp_fn(void (*cb)(void));

__attribute__((noinline, noipa))
void dwarf_fn(void (*cb)(void)) {
    volatile long guard = 0xCAFE;
    nofp_fn(cb);
    __asm__ volatile("" :: "r"(guard));
}
