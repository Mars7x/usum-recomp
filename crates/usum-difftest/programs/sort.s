@ Bubble sort of 16 words using register-offset addressing.
    .syntax unified
    .arm
    .text
    .global test_main
test_main:
    push {r4-r6, lr}
    ldr r0, =out_start
    adr r1, input
    mov r2, #0
1:  ldr r3, [r1, r2, lsl #2]
    str r3, [r0, r2, lsl #2]
    add r2, r2, #1
    cmp r2, #16
    bne 1b
    mov r4, #15
2:  mov r2, #0
    mov r6, #0
3:  add r5, r2, #1
    ldr r3, [r0, r2, lsl #2]
    ldr r1, [r0, r5, lsl #2]
    cmp r3, r1                  @ signed order
    strgt r1, [r0, r2, lsl #2]
    strgt r3, [r0, r5, lsl #2]
    movgt r6, #1
    mov r2, r5
    cmp r2, r4
    blt 3b
    cmp r6, #0
    subne r4, r4, #1
    bne 2b
    pop {r4-r6, pc}
input:
    .word 5, -3, 99, 0, 7, 0x7FFFFFFF, -100, 42, 1, 1, 0x80000000, 13, -1, 8, 64, 2

    .data
    .balign 4
    .global out_start, out_end
out_start:
    .space 64
out_end:
