/* One loadable segment, readable, writable and executable, at the 3DS
   application text address. Writable text lets the harness keep its
   register area next to its code. */
ENTRY(_start)
PHDRS { image PT_LOAD FLAGS(7); }
SECTIONS
{
    . = 0x00100000;
    .text : { *(.text*) } :image
    .data : { *(.data*) *(.bss*) *(COMMON) } :image
    /DISCARD/ : { *(.ARM.attributes) *(.comment) }
}
