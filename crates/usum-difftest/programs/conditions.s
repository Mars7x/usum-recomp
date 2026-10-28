@ Every condition code after a set of comparisons.
    .syntax unified
    .arm
    .text
    .global test_main
test_main:
    push {r4-r6, lr}
    ldr r12, =out_start
    adr r4, operands
    mov r5, #6
1:  ldmia r4!, {r0, r1}
    mov r6, #0
    cmp r0, r1
    orreq r6, r6, #0x0001
    orrne r6, r6, #0x0002
    orrcs r6, r6, #0x0004
    orrcc r6, r6, #0x0008
    orrmi r6, r6, #0x0010
    orrpl r6, r6, #0x0020
    orrvs r6, r6, #0x0040
    orrvc r6, r6, #0x0080
    orrhi r6, r6, #0x0100
    orrls r6, r6, #0x0200
    orrge r6, r6, #0x0400
    orrlt r6, r6, #0x0800
    orrgt r6, r6, #0x1000
    orrle r6, r6, #0x2000
    str r6, [r12], #4
    subs r5, r5, #1
    bne 1b
    pop {r4-r6, pc}
operands:
    .word 1, 1, 1, 2, 2, 1, 0x80000000, 1, 0x7FFFFFFF, 0xFFFFFFFF, 0, 0x80000000

    .data
    .balign 4
    .global out_start, out_end
out_start:
    .space 24
out_end:
