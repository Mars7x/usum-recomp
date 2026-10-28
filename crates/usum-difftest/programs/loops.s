@ Counted loops: sum 1..100, a countdown with subs/bne, nested loops.
    .syntax unified
    .arm
    .text
    .global test_main
test_main:
    ldr r12, =out_start
    mov r0, #0
    mov r1, #1
1:  add r0, r0, r1
    add r1, r1, #1
    cmp r1, #101
    blt 1b
    str r0, [r12], #4          @ 5050
    mov r2, #37
    mov r3, #0
2:  add r3, r3, r2, lsl #1
    subs r2, r2, #1
    bne 2b
    str r3, [r12], #4
    mov r4, #0                 @ nested 7 x 9
    mov r5, #7
3:  mov r6, #9
4:  add r4, r4, r5
    subs r6, r6, #1
    bgt 4b
    subs r5, r5, #1
    bne 3b
    str r4, [r12], #4
    bx lr

    .data
    .balign 4
    .global out_start, out_end
out_start:
    .space 16
out_end:
