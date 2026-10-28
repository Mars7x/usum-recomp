/* Layout for verifying real game functions (USUM_DIFFTEST_DUMP): the
   user's decompressed .code at its real address, the harness above it.
   Nothing linked with this script is ever committed: it is built in
   target/ from the user's own dump. */
ENTRY(_start)
PHDRS { image PT_LOAD FLAGS(7); }
SECTIONS
{
    . = 0x00100000;
    .game : { *(.game) } :image
    . = 0x00800000;
    .text : { *(.text*) } :image
    .data : { *(.data*) *(.bss*) *(COMMON) } :image
    /DISCARD/ : { *(.ARM.attributes) *(.comment) }
}
