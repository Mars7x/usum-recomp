@ Straight-line arithmetic and flag setting; each result goes to out.
    .syntax unified
    .arm
    .text
    .global test_main
test_main:
    ldr r12, =out_start
    ldr r0, =0x7FFFFFFF
    adds r1, r0, #1            @ signed overflow: N V
    mrs r2, cpsr
    stmia r12!, {r1, r2}
    subs r1, r0, r0            @ zero: Z C
    mrs r2, cpsr
    stmia r12!, {r1, r2}
    mov r3, #3
    rsbs r1, r3, #2            @ borrow: N, no C
    adc r4, r3, r3             @ uses C = 0
    sbc r5, r3, #1             @ uses C = 0: 3 - 1 - 1
    rsc r6, r3, #10            @ 10 - 3 - 1
    mvn r7, r3
    bic r8, r0, #0xFF
    eor r9, r0, r3, ror #1
    orr r10, r3, r3, lsl #8
    stmia r12!, {r1, r4, r5, r6, r7, r8, r9, r10}
    movs r1, r0, lsl #1        @ shifter carry out = 0
    mrs r2, cpsr
    movs r3, r0, asr #31       @ = 0, carry out = bit 30 = 1
    mrs r4, cpsr
    stmia r12!, {r1, r2, r3, r4}
    teq r0, r0
    cmn r0, #1                 @ 0x80000000: N V
    mrs r1, cpsr
    str r1, [r12], #4
    bx lr

    .data
    .global out_start, out_end
out_start:
    .space 64
out_end:
