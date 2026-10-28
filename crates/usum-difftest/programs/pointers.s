@ Calls through function pointers: blx reg, bx to ARM code, a table of
@ pointers loaded into pc, a tail call with b, and mov pc, lr returns.
    .syntax unified
    .arm
    .text
    .global test_main
test_main:
    push {r4, r5, lr}
    ldr r4, =out_start
    adr r5, handlers
    mov r0, #5
    ldr r3, [r5]
    blx r3                      @ double
    str r0, [r4], #4
    ldr r3, [r5, #4]
    blx r3                      @ square, via a tail call
    str r0, [r4], #4
    mov r0, #3
    adr lr, 1f
    ldr pc, [r5, #8]            @ negate, returning to 1f
1:  str r0, [r4], #4
    pop {r4, r5, pc}
handlers:
    .word double, square_tail, negate
double:
    add r0, r0, r0
    bx lr
square_tail:
    b square
square:
    mul r1, r0, r0
    mov r0, r1
    mov pc, lr
negate:
    rsb r0, r0, #0
    bx lr

    .data
    .balign 4
    .global out_start, out_end
out_start:
    .space 12
out_end:
