@ Both GCC switch forms: ldrls pc table and addls pc branch table.
    .syntax unified
    .arm
    .text
    .global test_main
test_main:
    push {r4, r5, lr}
    ldr r4, =out_start
    mov r5, #0
1:  mov r0, r5
    bl words
    str r0, [r4], #4
    mov r0, r5
    bl branches
    str r0, [r4], #4
    add r5, r5, #1
    cmp r5, #8
    blt 1b
    pop {r4, r5, pc}
words:
    cmp r0, #4
    ldrls pc, [pc, r0, lsl #2]
    b 9f
    .word 10f, 11f, 12f, 13f, 14f
10: mov r0, #100
    bx lr
11: mov r0, #101
    bx lr
12: mov r0, #102
    bx lr
13: mov r0, #103
    bx lr
14: mov r0, #104
    bx lr
9:  mvn r0, #0
    bx lr
branches:
    cmp r0, #3
    addls pc, pc, r0, lsl #2
    b 9f
    b 20f
    b 21f
    b 22f
    b 23f
20: mov r0, #200
    bx lr
21: mov r0, #201
    bx lr
22: mov r0, #202
    bx lr
23: mov r0, #203
    bx lr
9:  mov r0, #255
    bx lr

    .data
    .balign 4
    .global out_start, out_end
out_start:
    .space 64
out_end:
