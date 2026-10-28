@ LDM/STM in all four modes, with and without writeback.
    .syntax unified
    .arm
    .text
    .global test_main
test_main:
    push {r4-r11, lr}
    ldr r12, =out_start
    mov r0, #1
    mov r1, #2
    mov r2, #3
    mov r3, #4
    stmia r12!, {r0-r3}
    add r12, r12, #16
    stmdb r12!, {r1, r3}       @ into the gap
    add r12, r12, #8
    stmib r12, {r0, r2}
    add r12, r12, #16
    stmda r12!, {r0-r2}
    ldr r4, =out_start
    ldmia r4, {r5-r8}
    ldmib r4!, {r9, r10}
    ldmdb r4, {r11}
    ldmda r4!, {r0}
    ldr r12, =out_start
    add r12, r12, #48
    stmia r12, {r4-r11}
    pop {r4-r11, pc}

    .data
    .balign 4
    .global out_start, out_end
out_start:
    .space 96
out_end:
