@ Halfword and signed loads/stores, pre-indexed writeback, negative offsets.
    .syntax unified
    .arm
    .text
    .global test_main
test_main:
    adr r0, table
    ldr r12, =out_end
    ldrh r1, [r0]
    ldrsh r2, [r0, #2]
    ldrsb r3, [r0, #5]
    ldrb r4, [r0, #5]
    ldrh r5, [r0, #4]!
    ldrsh r6, [r0, #-2]
    mov r7, #6
    ldrsh r8, [r0, r7]
    strh r2, [r12, #-2]!
    strh r1, [r12, #-2]!
    strb r3, [r12, #-1]!
    strb r4, [r12, #-1]!
    bic r12, r12, #3            @ word stores stay aligned (see RESEARCH.md R-010)
    mov r9, #-4
    str r5, [r12, r9]!
    sub r12, r12, #20
    stmia r12, {r0-r8}
    bx lr
table:
    .hword 0x8001, 0xF00F, 0x7F80, 0x1234, 0xFFFF
    .balign 4

    .data
    .balign 4
    .global out_start, out_end
out_start:
    .space 64
out_end:
