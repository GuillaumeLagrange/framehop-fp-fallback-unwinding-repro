/*
 * Compiled WITHOUT unwind tables (-fno-asynchronous-unwind-tables -fno-unwind-tables)
 * but WITH a frame pointer (-fno-omit-frame-pointer).
 *
 * This mirrors Chromium/Electron: the function ships NO .eh_frame, so framehop has to
 * fall back to the frame-pointer rule `new_sp = fp + 16` for it. We give it a frame
 * larger than 16 bytes, so the frame record (where x29 points) is NOT at CFA-16 --
 * which is exactly what makes `fp + 16` an *underestimate* of the caller's SP.
 *
 * gcc emits: stp x29,x30,[sp,#-80]! ; mov x29,sp   => record at sp+0, x29 = CFA-80.
 * So `fp + 16` = CFA-64, i.e. 0x40 too low.
 */
#include <stdint.h>

__attribute__((noinline, noipa))
void nofp_fn(void (*cb)(void)) {
    volatile uint64_t a[6];
    for (int i = 0; i < 6; i++) a[i] = (uint64_t)(i * 0x1111);
    cb();                       /* the "sample" is taken inside here */
    uint64_t s = 0;
    for (int i = 0; i < 6; i++) s += a[i];
    __asm__ volatile("" :: "r"(s));   /* keep `a` live across the call */
}
