@ Thumb recursion (Fibonacci) with push/pop {pc}, and a conditional-branch loop.
    .syntax unified
    .text
    .arm
    .global test_main
test_main:
    push {r4, lr}
    movs r0, #16
    blx tfib
    ldr r1, =out_start
    str r0, [r1]
    movs r0, #50
    blx triangle
    ldr r1, =out_start
    str r0, [r1, #4]
    pop {r4, pc}

    .thumb
    .thumb_func
tfib:
    cmp r0, #2
    blt 9f
    push {r4, r5, lr}
    mov r4, r0
    subs r0, r4, #1
    bl tfib
    mov r5, r0
    subs r0, r4, #2
    bl tfib
    adds r0, r0, r5
    pop {r4, r5, pc}
9:  bx lr

    .thumb_func
triangle:                       @ sum 1..n with a backward bgt
    movs r1, #0
1:  adds r1, r1, r0
    subs r0, #1
    cmp r0, #0
    bgt 1b
    mov r0, r1
    bx lr

    .data
    .balign 4
    .global out_start, out_end
out_start:
    .space 8
out_end:
