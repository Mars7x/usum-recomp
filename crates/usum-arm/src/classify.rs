//! A coarse name for the class of an encoding the decoder does not
//! support yet, for coverage reports. Classes follow the encoding tables
//! of the ARM ARM (DDI 0406C A5, A6); this is reporting, not decoding.

use crate::{bits, Instr};

fn coprocessor(raw: u32) -> u32 {
    bits(raw, 11, 8)
}

fn vfp(raw: u32) -> bool {
    matches!(coprocessor(raw), 10 | 11)
}

/// The class of an unsupported instruction.
pub fn unsupported_class(instr: &Instr) -> &'static str {
    if instr.thumb {
        return match instr.raw & 0xFFFF {
            hw if hw >> 8 == 0xDE => "Thumb UDF",
            hw if hw >> 11 == 0b11101 || hw >> 11 == 0b11111 || hw >> 11 == 0b11110 => "Thumb lone BL/BLX half",
            hw if hw >> 12 == 0b1011 => "Thumb CPS/SETEND/BKPT",
            _ => "Thumb other (UNPREDICTABLE forms)",
        };
    }
    let raw = instr.raw;
    if raw >> 28 == 0xF {
        return match raw {
            r if r & 0x0D70_F000 == 0x0550_F000 => "PLD (preload hint)",
            r if r & 0x0FF0_0000 == 0x0100_0000 => "CPS/SETEND",
            0xF57F_F01F => "CLREX",
            r if bits(r, 27, 25) == 0b100 => "SRS/RFE",
            _ => "other unconditional (cond = 1111)",
        };
    }
    match bits(raw, 27, 24) {
        0b1110 if raw & 0x10 != 0 && vfp(raw) => "VFP register transfer (VMOV/VMRS/VMSR)",
        0b1110 if raw & 0x10 != 0 && coprocessor(raw) == 15 => "CP15 MRC/MCR",
        0b1110 if raw & 0x10 != 0 => "other coprocessor MRC/MCR",
        0b1110 if vfp(raw) => "VFP data processing",
        0b1110 => "other coprocessor CDP",
        0b1100 | 0b1101 if vfp(raw) && bits(raw, 27, 21) == 0b1100010 => "VFP 64-bit transfer (VMOV)",
        0b1100 | 0b1101 if vfp(raw) => "VFP load/store (VLDR/VSTR/VLDM/VSTM)",
        0b1100 | 0b1101 => "other coprocessor LDC/STC/MCRR",
        0b0110 | 0b0111 if raw & 0x10 != 0 => match bits(raw, 24, 23) {
            0b00 => "media: parallel add/subtract",
            0b01 => "media: pack/saturate/extend-and-add/select",
            0b10 => "media: signed multiplies (SMUAD/SMLAD/SMMUL...)",
            _ => "media: USAD8/USADA8/UDF",
        },
        0b0001 if raw & 0x0F90_00F0 == 0x0100_0090 => "SWP/SWPB",
        0b0001 if raw & 0x0F80_00F0 == 0x0180_0090 => "exclusive (LDREX/STREX)",
        0b0001 if raw & 0x0F90_0090 == 0x0100_0080 => "signed halfword multiply (SMLAxy/SMULxy...)",
        0b0001 if raw & 0x0F90_00F0 == 0x0100_0050 => "saturating add/subtract (QADD...)",
        0b0001 if raw & 0x0FF0_00F0 == 0x0120_0070 => "BKPT",
        0b0000 if raw & 0x0FE0_00F0 == 0x0040_0090 => "UMAAL",
        _ => "other (UNPREDICTABLE forms or not decoded yet)",
    }
}

/// Whether an unsupported instruction might change the PC. Only the
/// classes named below are known never to: VFP and coprocessor transfers
/// (an MRC to R15 sets flags, it does not branch), exclusives, media,
/// multiplies, saturating arithmetic, PLD/CLREX, SWP.
pub(crate) fn may_branch(instr: &Instr) -> bool {
    let class = unsupported_class(instr);
    class.starts_with("other (")
        || class.starts_with("other unconditional")
        || class.starts_with("Thumb")
        || class == "SRS/RFE"
        || class == "BKPT"
        || class == "CPS/SETEND"
        || (class.starts_with("media") && bits(instr.raw, 15, 12) == 15)
}
