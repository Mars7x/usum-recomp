@ Shared harness for differential test programs (original code, written
@ for these tests). It is linked in front of each program and run both
@ under qemu-arm and as recompiled Rust. It clears r0-r12 and the flags,
@ calls test_main, then writes to stdout (Linux syscalls via svc #0):
@   1. r0-r12 and the APSR flags (NZCVQ and GE only) after test_main;
@   2. the bytes from out_start to out_end, which the program defines.
@ Linked with link.ld (one RWX segment), so .text is writable and the register area can live in
@ it, within reach of PC-relative stores.

    .syntax unified
    .arm
    .text
    .global _start
_start:
    ldr sp, =stack_top
    mov r0, #0
    mov r1, #0
    mov r2, #0
    mov r3, #0
    mov r4, #0
    mov r5, #0
    mov r6, #0
    mov r7, #0
    mov r8, #0
    mov r9, #0
    mov r10, #0
    mov r11, #0
    mov r12, #0
    msr cpsr_fs, #0
    bl test_main
    str r0, saved + 0
    str r1, saved + 4
    str r2, saved + 8
    str r3, saved + 12
    str r4, saved + 16
    str r5, saved + 20
    str r6, saved + 24
    str r7, saved + 28
    str r8, saved + 32
    str r9, saved + 36
    str r10, saved + 40
    str r11, saved + 44
    str r12, saved + 48
    mrs r0, cpsr
    and r1, r0, #0x000F0000
    and r0, r0, #0xF8000000
    orr r0, r0, r1
    str r0, saved + 52
    mov r0, #1
    adr r1, saved
    mov r2, #56
    mov r7, #4
    svc #0
    mov r0, #1
    ldr r1, =out_start
    ldr r2, =out_end
    sub r2, r2, r1
    mov r7, #4
    svc #0
    mov r0, #0
    mov r7, #1
    svc #0
    .ltorg
saved:
    .space 56

    .data
    .balign 8
stack:
    .space 0x4000
stack_top:
