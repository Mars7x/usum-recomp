@ Recursion 1000 deep (past the translator's call-depth limit of 256),
@ so returns must resume through the host after unwinding.
    .syntax unified
    .arm
    .text
    .global test_main
test_main:
    push {lr}
    ldr r0, =1000
    bl down
    ldr r1, =out_start
    str r0, [r1]
    pop {pc}
down:                           @ returns the sum n + (n-1) + ... + 1
    cmp r0, #0
    bxeq lr
    push {r0, lr}
    sub r0, r0, #1
    bl down
    pop {r1, lr}
    add r0, r0, r1
    bx lr

    .data
    .balign 4
    .global out_start, out_end
out_start:
    .space 4
out_end:
