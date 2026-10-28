@ Translated in "split" mode with units of 8 instructions (build.rs), so
@ nearly every call crosses units and goes through the function table:
@ recursion, recursion past the depth limit, ARM <-> Thumb calls both
@ ways, and calls through function pointers.
    .syntax unified
    .text
    .arm
    .global test_main
test_main:
    push {r4-r6, lr}
    ldr r4, =out_start
    mov r0, #15
    bl fib
    str r0, [r4], #4
    ldr r0, =600
    bl down
    str r0, [r4], #4
    mov r0, #7
    blx t_twice_then_arm
    str r0, [r4], #4
    adr r5, pointers
    mov r6, #0
1:  ldr r3, [r5, r6, lsl #2]
    mov r0, r6
    blx r3
    str r0, [r4], #4
    add r6, r6, #1
    cmp r6, #3
    blt 1b
    pop {r4-r6, pc}
pointers:
    .word a_plus1, t_times3, a_square

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

down:                           @ sum of n..1, 600 calls deep
    cmp r0, #0
    bxeq lr
    push {r0, lr}
    sub r0, r0, #1
    bl down
    pop {r1, lr}
    add r0, r0, r1
    bx lr

a_plus1:
    add r0, r0, #1
    bx lr

a_square:
    mul r1, r0, r0
    mov r0, r1
    bx lr

a_add100:
    add r0, r0, #100
    bx lr

    .thumb
    .thumb_func
t_times3:
    movs r1, #3
    muls r0, r1, r0
    bx lr

    .thumb_func
t_twice_then_arm:
    push {r4, lr}
    adds r0, r0, r0
    blx a_add100
    bl t_times3
    pop {r4, pc}
    .ltorg

    .data
    .balign 4
    .global out_start, out_end
out_start:
    .space 32
out_end:
