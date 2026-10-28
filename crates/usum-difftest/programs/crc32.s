@ Bitwise CRC-32 (reflected, polynomial 0xEDB88320) of a buffer.
    .syntax unified
    .arm
    .text
    .global test_main
test_main:
    adr r0, data
    mov r1, #43                 @ length
    mvn r2, #0
    ldr r3, =0xEDB88320
1:  ldrb r12, [r0], #1
    eor r2, r2, r12
    mov r12, #8
2:  movs r2, r2, lsr #1
    eorcs r2, r2, r3
    subs r12, r12, #1
    bne 2b
    subs r1, r1, #1
    bne 1b
    mvn r2, r2
    ldr r12, =out_start
    str r2, [r12]
    bx lr
data:
    .ascii "The quick brown fox jumps over the lazy dog"
    .balign 4

    .data
    .balign 4
    .global out_start, out_end
out_start:
    .space 4
out_end:
