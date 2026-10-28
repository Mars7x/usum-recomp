@ Euclid's GCD with conditional execution only (no branches in the loop body).
    .syntax unified
    .arm
    .text
    .global test_main
test_main:
    ldr r12, =out_start
    adr r4, pairs
    mov r5, #4
1:  ldmia r4!, {r0, r1}
2:  cmp r0, r1
    subgt r0, r0, r1
    sublt r1, r1, r0
    bne 2b
    str r0, [r12], #4
    subs r5, r5, #1
    bne 1b
    bx lr
pairs:
    .word 1071, 462, 270, 192, 17, 5, 123456, 7890

    .data
    .balign 4
    .global out_start, out_end
out_start:
    .space 16
out_end:
