@ Thumb loads and stores: register and immediate offsets, signed forms,
@ SP-relative, literal loads, ADR, LDMIA/STMIA.
    .syntax unified
    .text
    .arm
    .global test_main
test_main:
    push {r4-r7, lr}
    ldr r0, =out_start
    blx memory
    pop {r4-r7, pc}

    .thumb
    .thumb_func
memory:
    push {r4-r7, lr}
    sub sp, #16
    adr r1, table
    ldrb r2, [r1, #1]
    ldrh r3, [r1, #2]
    movs r4, #4
    ldrsb r5, [r1, r4]
    ldrsh r6, [r1, r4]
    ldr r7, [r1, #8]
    stmia r0!, {r2-r7}
    str r7, [sp, #4]
    ldr r2, [sp, #4]
    strh r6, [r0, #0]
    strb r5, [r0, #2]
    movs r4, #3
    strb r4, [r0, r4]
    adds r0, #4
    ldr r3, value
    ldmia r1!, {r4, r5}
    mov r6, r1
    stmia r0!, {r2-r6}
    add r4, sp, #8
    str r4, [r0]
    add sp, #16
    pop {r4-r7, pc}
    .balign 4
table:
    .byte 0x11, 0xF2, 0x33, 0x84, 0x95, 0xA6, 0x07, 0x18
    .word 0xCAFEF00D
value:
    .word 0x12345678

    .data
    .balign 4
    .global out_start, out_end
out_start:
    .space 64
out_end:
