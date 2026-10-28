@ ARM <-> Thumb interworking in every form: BLX immediate both ways, BX
@ and BLX through registers, a table of function pointers with Thumb bit
@ set, and Thumb pop {pc} returning to an ARM caller.
    .syntax unified
    .text
    .arm
    .global test_main
test_main:
    push {r4-r6, lr}
    ldr r4, =out_start
    movs r0, #3
    blx t_triple                @ ARM -> Thumb (BLX imm)
    str r0, [r4], #4
    adr r5, pointers
    ldr r3, [r5]                @ Thumb pointer (odd)
    movs r0, #7
    blx r3                      @ ARM -> Thumb (BLX reg)
    str r0, [r4], #4
    movs r0, #11
    ldr r3, [r5, #4]            @ ARM pointer
    blx r3
    str r0, [r4], #4
    movs r0, #2
    blx t_calls_arm             @ Thumb calls back into ARM
    str r0, [r4], #4
    movs r0, #9
    adr lr, 1f
    ldr pc, [r5]                @ load PC with a Thumb address
1:  str r0, [r4], #4
    pop {r4-r6, pc}
pointers:
    .word t_square, a_double

a_double:
    add r0, r0, r0
    bx lr

    .thumb
    .thumb_func
t_triple:
    movs r1, #3
    muls r0, r1, r0
    bx lr

    .thumb_func
t_square:
    muls r0, r0, r0
    bx lr

    .thumb_func
t_calls_arm:
    push {r4, lr}
    blx a_double                @ Thumb -> ARM (BLX imm)
    mov r4, r0
    ldr r3, =a_double
    blx r3                      @ Thumb -> ARM (BLX reg)
    adds r0, r0, r4
    pop {r4, pc}                @ returns to ARM (interworking pop)
    .ltorg

    .data
    .balign 4
    .global out_start, out_end
out_start:
    .space 20
out_end:
