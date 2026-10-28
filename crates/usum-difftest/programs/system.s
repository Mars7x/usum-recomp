@ TLS register, barriers, preload hints, and exclusive accesses in all four
@ sizes on a single core. Stores between an LDREX and its STREX are avoided:
@ whether they clear the monitor is IMPLEMENTATION DEFINED, and QEMU's
@ user mode models STREX as a compare-and-swap (RESEARCH.md R-016).
    .syntax unified
    .arm
    .text
    .global test_main
test_main:
    push {r4-r11, lr}
    ldr r11, =out_start
    ldr r0, =0x1234ABCD
    ldr r7, =0x000F0005         @ set_tls(r0)
    svc #0
    mrc p15, 0, r1, c13, c0, 3
    mrc p15, 0, r12, c13, c0, 3
    str r1, [r11], #4
    str r12, [r11], #4
    mov r0, #0
    mcr p15, 0, r0, c7, c10, 5  @ data memory barrier
    mcr p15, 0, r0, c7, c10, 4  @ data synchronization barrier
    mcr p15, 0, r0, c7, c5, 4   @ flush prefetch buffer
    adr r2, cells
    pld [r2, #32]
    pld [r2, r0, lsl #2]
    @ atomic increment, five times
    mov r4, #5
1:  ldrex r1, [r2]
    add r1, r1, #3
    strex r3, r1, [r2]
    cmp r3, #0
    bne 1b
    subs r4, r4, #1
    bne 1b
    ldr r1, [r2]
    str r1, [r11], #4
    @ STREX with the monitor cleared fails and stores nothing
    ldrex r1, [r2]
    clrex
    mov r5, #99
    strex r3, r5, [r2]
    str r3, [r11], #4
    ldr r1, [r2]
    str r1, [r11], #4
    @ a second STREX after a successful one fails
    ldrex r1, [r2]
    strex r3, r5, [r2]
    strex r6, r1, [r2]
    stmia r11!, {r3, r6}
    @ byte, halfword and doubleword forms
    add r8, r2, #8
    ldrexb r1, [r8]
    add r1, r1, #1
    strexb r3, r1, [r8]
    ldrexh r4, [r8]
    add r4, r4, #0x100
    strexh r6, r4, [r8]
    stmia r11!, {r1, r3, r4, r6}
    add r8, r2, #16
    ldrexd r4, r5, [r8]
    adds r4, r4, #1
    adc r5, r5, #0
    strexd r3, r4, r5, [r8]
    ldrd r6, r7, [r8]
    stmia r11!, {r3, r4, r5, r6, r7}
    pop {r4-r11, pc}
    .balign 8
cells:
    .word 10, 0, 0x000000FF, 0, 0xFFFFFFFF, 0x7FFFFFFF, 0, 0

    .data
    .balign 4
    .global out_start, out_end
out_start:
    .space 80
out_end:
