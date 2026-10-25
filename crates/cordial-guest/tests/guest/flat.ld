/* One flat, position-independent image at offset 0. The tests map it at a
   page-aligned address, which is all adrp needs to stay correct. */
SECTIONS {
    . = 0;
    .text : { *(.text .text.*) }
    .rodata : { *(.rodata .rodata.*) }
    .data : { *(.data .data.*) *(.bss .bss.*) }
    /DISCARD/ : { *(.comment) *(.note*) *(.eh_frame*) }
}
