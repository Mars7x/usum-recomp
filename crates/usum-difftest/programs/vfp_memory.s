@ VFP loads, stores and transfers: VLDR/VSTR (including a PC-relative
@ literal), VLDM/VSTM increment-after and decrement-before with writeback,
@ VPUSH/VPOP, every VMOV form, VMRS/VMSR. Cumulative exception flags in
@ FPSCR are masked out (RESEARCH.md R-017).
    .syntax unified
    .arm
    .fpu vfpv2
    .text
    .global test_main
test_main:
    push {r4-r11, lr}
    ldr r11, =out_start
    adr r0, values
    vldr s0, [r0]
    vldr s1, [r0, #4]
    vldr d1, [r0, #8]
    vldr d2, literal
    add r1, r0, #24
    vldmia r1!, {s6-s9}
    sub r2, r1, r0
    vldmdb r1!, {d5-d6}
    sub r3, r1, r0
    stmia r11!, {r2, r3}
    vpush {d0-d2}
    vpop {d8-d10}
    vmov r4, s16
    vmov s20, r4
    vmov r5, r6, d9
    vmov d11, r6, r5
    vmov s24, s25, r5, r4
    vmov r7, r8, s24, s25
    vmov.32 d13[1], r7
    vmov.32 r9, d13[1]
    stmia r11!, {r4-r9}
    ldr r4, =0x03C00000      @ DN, FZ, round toward zero
    vmsr fpscr, r4
    vmrs r5, fpscr
    mov r4, #0
    vmsr fpscr, r4
    vmrs r6, fpscr
    stmia r11!, {r5, r6}
    vstr d9, [r11]
    vstr s20, [r11, #8]
    add r11, r11, #12
    vstmia r11!, {s0-s31}
    vstmdb r11!, {d14-d15}
    pop {r4-r11, pc}
    .balign 8
literal:
    .word 0x54442D18, 0x400921FB  @ pi
values:
    .word 0x3F800000, 0xC0200000, 0x00000000, 0x3FF00000
    .word 0x7FC00001, 0x007FFFFF, 0x12345678, 0x9ABCDEF0
    .word 0x80000000, 0x7F800000, 0xDEADBEEF, 0xCAFEF00D
    .word 0x00000001, 0x7F7FFFFF, 0x00800000, 0xFF800000

    .data
    .balign 4
    .global out_start, out_end
out_start:
    .space 256
out_end:
