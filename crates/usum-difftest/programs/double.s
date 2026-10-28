@ 64-bit arithmetic with LDRD/STRD and adds/adc, subs/sbc.
    .syntax unified
    .arm
    .text
    .global test_main
test_main:
    push {r4-r7, lr}
    adr r0, pairs
    ldr r12, =out_start
    mov r7, #3
1:  ldrd r2, r3, [r0], #8
    ldrd r4, r5, [r0], #8
    adds r6, r2, r4
    adc r1, r3, r5
    strd r6, r7, [r12]
    str r1, [r12, #4]
    subs r6, r2, r4
    sbc r1, r3, r5
    str r6, [r12, #8]
    str r1, [r12, #12]
    add r12, r12, #16
    subs r7, r7, #1
    bne 1b
    pop {r4-r7, pc}
    .balign 8
pairs:
    .word 0xFFFFFFFF, 0x00000000, 0x00000001, 0x00000000
    .word 0x89ABCDEF, 0x01234567, 0x76543211, 0xFEDCBA98
    .word 0x00000000, 0x80000000, 0x00000001, 0x00000000

    .data
    .balign 4
    .global out_start, out_end
out_start:
    .space 48
out_end:
