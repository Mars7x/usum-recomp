@ Wide GPR transfers exercise the checked RAM block path in every addressing
@ mode, writeback, base-in-list stores, and loading all registers including PC.
    .syntax unified
    .arm
    .text
    .global test_main
test_main:
    push {r4-r11, lr}
    mov r4, #0x14
    mov r5, #0x25
    mov r6, #0x36
    mov r7, #0x47
    mov r8, #0x58
    mov r9, #0x69
    mov r10, #0x7a
    mov r11, #0x8b
    ldr r3, =out_start

    add r12, r3, #0
    stmia r12!, {r4-r11}
    str r12, [r3, #128]
    add r12, r3, #64
    stmdb r12!, {r4-r11}
    str r12, [r3, #132]
    add r12, r3, #60
    stmib r12!, {r4-r11}
    str r12, [r3, #136]
    add r12, r3, #124
    stmda r12!, {r4-r11}
    str r12, [r3, #140]

    add r12, r3, #0
    ldmia r12!, {r4-r11}
    str r12, [r3, #144]
    add r2, r3, #160
    stmia r2, {r4-r11}
    add r12, r3, #64
    ldmdb r12!, {r4-r11}
    str r12, [r3, #148]
    add r2, r3, #192
    stmia r2, {r4-r11}
    add r12, r3, #60
    ldmib r12!, {r4-r11}
    str r12, [r3, #152]
    add r2, r3, #224
    stmia r2, {r4-r11}
    add r12, r3, #124
    ldmda r12!, {r4-r11}
    str r12, [r3, #156]
    add r2, r3, #256
    stmia r2, {r4-r11}

    @ Base is lowest in the list: store its original value, then write back.
    add r0, r3, #512
    stmia r0!, {r0-r15}
    sub r0, r0, #64
    adr r1, after_all
    str r1, [r0, #60]
    ldmia r0, {r0-r15}
after_all:
    pop {r4-r11, pc}
    .ltorg

    .data
    .balign 4
    .global out_start, out_end
out_start:
    .space 640
out_end:
