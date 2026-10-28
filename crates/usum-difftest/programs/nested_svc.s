@ A write syscall from three calls deep: the host must resume inside the
@ nested calls after the SVC unwinds them.
    .syntax unified
    .arm
    .text
    .global test_main
test_main:
    push {r4, lr}
    mov r4, #7
    bl level1
    ldr r1, =out_start
    str r0, [r1]
    str r4, [r1, #4]
    pop {r4, pc}
level1:
    push {lr}
    bl level2
    add r0, r0, #1
    pop {pc}
level2:
    push {lr}
    bl level3
    add r0, r0, #10
    pop {pc}
level3:
    push {r7, lr}
    mov r0, #1
    adr r1, message
    mov r2, #12
    mov r7, #4
    svc #0                      @ write(1, message, 12)
    mov r0, #100
    pop {r7, pc}
message:
    .ascii "from level3\n"
    .balign 4

    .data
    .balign 4
    .global out_start, out_end
out_start:
    .space 8
out_end:
