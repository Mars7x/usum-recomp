@ Recursive Fibonacci: nested BL, push/pop of callee-saved registers.
    .syntax unified
    .arm
    .text
    .global test_main
test_main:
    push {r4, lr}
    mov r0, #18
    bl fib
    ldr r1, =out_start
    str r0, [r1]
    pop {r4, pc}
fib:
    cmp r0, #2
    bxlt lr
    push {r4, r5, lr}
    mov r4, r0
    sub r0, r4, #1
    bl fib
    mov r5, r0
    sub r0, r4, #2
    bl fib
    add r0, r0, r5
    pop {r4, r5, pc}

    .data
    .balign 4
    .global out_start, out_end
out_start:
    .space 4
out_end:
