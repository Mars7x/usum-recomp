@ Byte loops: strlen and a copy with post-indexed ldrb/strb.
    .syntax unified
    .arm
    .text
    .global test_main
test_main:
    adr r0, text
    mov r1, r0
1:  ldrb r2, [r1], #1
    cmp r2, #0
    bne 1b
    sub r3, r1, r0
    sub r3, r3, #1             @ length
    ldr r12, =out_start
    str r3, [r12], #4
2:  ldrb r2, [r0], #1          @ copy, uppercasing a-z
    sub r1, r2, #'a'
    cmp r1, #25
    subls r2, r2, #32
    strb r2, [r12], #1
    cmp r2, #0
    bne 2b
    bx lr
text:
    .asciz "static recompilation, checked against qemu"
    .balign 4

    .data
    .balign 4
    .global out_start, out_end
out_start:
    .space 64
out_end:
