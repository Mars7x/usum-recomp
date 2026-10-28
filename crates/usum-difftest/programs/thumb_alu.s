@ Thumb arithmetic, shifts, extends and byte reversal, called from ARM.
    .syntax unified
    .text
    .arm
    .global test_main
test_main:
    push {r4-r7, lr}
    ldr r0, =out_start
    blx compute
    pop {r4-r7, pc}

    .thumb
    .thumb_func
compute:
    ldr r1, =0x80FF7F01
    movs r2, #200
    adds r3, r1, r2
    subs r4, r2, #7
    lsls r5, r1, #4
    lsrs r6, r1, #31
    asrs r7, r1, #3
    stmia r0!, {r3-r7}
    movs r3, #5
    muls r3, r2, r3
    negs r4, r2
    mvns r5, r2
    rors r6, r2
    sxtb r7, r1
    stmia r0!, {r3-r7}
    uxth r3, r1
    sxth r4, r1
    rev r5, r1
    rev16 r6, r1
    revsh r7, r1
    stmia r0!, {r3-r7}
    movs r3, #0
    movs r4, #10
1:  adds r3, r3, r4
    subs r4, #1
    bne 1b
    mov r8, r3
    add r8, r1
    mov r5, r8
    str r5, [r0]
    adcs r3, r4
    sbcs r4, r3
    str r3, [r0, #4]
    str r4, [r0, #8]
    bx lr
    .ltorg

    .data
    .balign 4
    .global out_start, out_end
out_start:
    .space 80
out_end:
