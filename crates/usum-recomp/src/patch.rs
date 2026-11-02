//! Native cadence patch points (ADR-0008).
//!
//! Each entry names a guest instruction, by address in its translation
//! (executable addresses, or module virtual addresses), and the
//! `usum_guest::native` call the emitter places after it in every generated
//! copy of that instruction. The calls do nothing unless the runtime
//! publishes an update step, so a patched translation runs normal cadence
//! unchanged. Entries hold addresses and register numbers only, never game
//! code (ADR-0004), and each cites the research entry that justifies it.

use std::collections::BTreeMap;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Patch {
    /// After the instruction: multiply single-precision register `reg` by
    /// the update's logical step.
    ScaleSingle { reg: u8 },
    /// Around the instruction: scale `reg` for this instruction only, then
    /// restore it, for an operand the following code reads unscaled. Only
    /// for instructions that do not branch.
    ScaleOperand { reg: u8 },
    /// Before the instruction (at a call's return label): scale `reg` if
    /// the call went to `callee`, still held in callee-saved `callee_reg`,
    /// for `caller`, the return address `sp_offset` bytes up the stack.
    ScaleResult { reg: u8, callee_reg: u8, callee: u32, sp_offset: u32, caller: u32 },
    /// Before the instruction: convert per-update blend factor `reg` to the
    /// elapsed step.
    Blend { reg: u8 },
}

impl Patch {
    /// The Rust statements emitted before the patched instruction, at the
    /// end of its body (run only if a conditional instruction executes), and
    /// after it (always run).
    pub fn code(self, address: u32) -> [Option<String>; 3] {
        match self {
            Patch::ScaleSingle { reg } => [None, Some(format!("usum_guest::native::scale_single(cpu, {reg});")), None],
            Patch::ScaleOperand { reg } => [
                Some(format!("let native_{address:08x} = cpu.s[{reg}]; usum_guest::native::scale_single(cpu, {reg});")),
                None,
                Some(format!("cpu.s[{reg}] = native_{address:08x};")),
            ],
            Patch::ScaleResult { reg, callee_reg, callee, sp_offset, caller } => [
                Some(format!(
                    "usum_guest::native::scale_result(cpu, mem, {reg}, {callee_reg}, {callee:#010x}, {sp_offset}, {caller:#010x});"
                )),
                None,
                None,
            ],
            Patch::Blend { reg } => [Some(format!("usum_guest::native::blend_single(cpu, {reg});")), None, None],
        }
    }
}

pub type Patches = BTreeMap<u32, Patch>;

/// What the emitter changes in one translation.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Table {
    /// Instruction patch points, by address.
    pub points: Patches,
    /// Function keys the runtime hooks through the function table
    /// (`usum_runtime::native_cadence`); every call to them goes through the
    /// table, even from the same unit, so the hook sees all of them.
    pub hooked: std::collections::BTreeSet<u32>,
}

/// The emitter table for the executable of a recorded version profile.
pub fn executable(profile: &str) -> Table {
    match profile {
        "ultra-sun-usa-cart" => Table {
            points: executable_points(profile),
            // Engine update start, where the runtime publishes each step.
            hooked: [0x0040_1438].into(),
        },
        _ => Table::default(),
    }
}

fn executable_points(profile: &str) -> Patches {
    match profile {
        "ultra-sun-usa-cart" => Patches::from([
            // Per-update movement cap after `vsub.f32 s2, s23, s3` (R-136, R-179).
            (0x003a_9988, Patch::ScaleSingle { reg: 2 }),
            // Stored root-motion rate after `vldr s1, [r3, #4]` and
            // `vldr s1, [r2, #4]` (R-136, R-179).
            (0x004b_80dc, Patch::ScaleSingle { reg: 1 }),
            (0x004b_81a8, Patch::ScaleSingle { reg: 1 }),
            // Animation rate read at phase advance, `vldr s1, [r0, #4]` (R-136).
            (0x0036_a880, Patch::ScaleSingle { reg: 1 }),
            // Phase increment `vadd.f32 s2, s2, s3`: only the added step is
            // scaled (R-136, R-165).
            (0x0036_e160, Patch::ScaleOperand { reg: 3 }),
            // The rate getter's result, for the movement consumer only: return
            // label of the dispatcher's `blx r7` (R-148).
            (0x005a_da88, Patch::ScaleResult { reg: 0, callee_reg: 7, callee: 0x004b_7f94, sp_offset: 140, caller: 0x003a_975c }),
        ]),
        _ => Patches::new(),
    }
}

/// The emitter table for CRO modules, by virtual address.
pub fn modules(profile: &str) -> Table {
    match profile {
        "ultra-sun-usa-cart" => Table {
            points: Patches::from([
                // FieldRo (virtual base 0x6f000000): the rotation smoothing
                // factor in s16, before the call that follows its choice (R-165).
                (0x6f05_1338, Patch::Blend { reg: 16 }),
            ]),
            ..Table::default()
        },
        _ => Table::default(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{analyze, emit, Image};

    #[test]
    fn patch_calls_follow_their_instruction_in_every_copy() {
        // mov r0, #1; vmul.f32 s2, s0, s1; bx lr — the patch names the vmul.
        let words: [u32; 3] = [0xe3a0_0001, 0xee20_1a20, 0xe12f_ff1e];
        let bytes: Vec<u8> = words.iter().flat_map(|w| w.to_le_bytes()).collect();
        let image = Image { base: 0x1000, bytes: &bytes, code: 0x1000..0x100c };
        let analysis = analyze(&image, &[0x1000, 0x1004]);
        let patches = Table { points: Patches::from([(0x1004, Patch::ScaleSingle { reg: 2 })]), ..Table::default() };
        let split = emit::split_patched(&analysis, 1000, &|i| format!("u{i}"), 0, false, &patches);
        let source: String = split.units.iter().map(|u| u.source.as_str()).collect();
        let calls = source.matches("usum_guest::native::scale_single(cpu, 2);").count();
        assert_eq!(calls, analysis.functions.len(), "{source}");
        let around = Table { points: Patches::from([(0x1004, Patch::ScaleOperand { reg: 1 })]), ..Table::default() };
        let split = emit::split_patched(&analysis, 1000, &|i| format!("u{i}"), 0, false, &around);
        let source: String = split.units.iter().map(|u| u.source.as_str()).collect();
        let save = source.find("let native_00001004 = cpu.s[1];").expect("operand saved");
        let restore = source.find("cpu.s[1] = native_00001004;").expect("operand restored");
        assert!(save < source.find("vfp::arith32(cpu, vfp::Arith::Mul").unwrap() && restore > save);
        let unpatched = emit::split(&analysis, 1000, &|i| format!("u{i}"));
        assert!(!unpatched.units.iter().any(|u| u.source.contains("usum_guest::native")));
    }

    #[test]
    fn only_recorded_profiles_have_patch_points() {
        assert_eq!(executable("ultra-sun-usa-cart").points.len(), 6);
        assert_eq!(modules("ultra-sun-usa-cart").points.len(), 1);
        assert_eq!(executable("ultra-moon-usa-cart"), Table::default());
    }

    #[test]
    fn hooked_functions_are_called_through_the_table_within_a_unit() {
        // f_1000: bl 0x1008; bx lr. f_1008: bx lr. One unit, so normally direct.
        let words: [u32; 3] = [0xeb00_0000, 0xe12f_ff1e, 0xe12f_ff1e];
        let bytes: Vec<u8> = words.iter().flat_map(|w| w.to_le_bytes()).collect();
        let image = Image { base: 0x1000, bytes: &bytes, code: 0x1000..0x100c };
        let analysis = analyze(&image, &[0x1000]);
        let direct = emit::split(&analysis, 1000, &|i| format!("u{i}"));
        assert!(direct.units[0].source.contains("f_00001008)"), "{}", direct.units[0].source);
        let table = Table { hooked: [0x1008].into(), ..Table::default() };
        let hooked = emit::split_patched(&analysis, 1000, &|i| format!("u{i}"), 0, false, &table);
        assert!(!hooked.units[0].source.contains(", f_00001008)"), "{}", hooked.units[0].source);
        assert!(hooked.units[0].source.contains("function("), "{}", hooked.units[0].source);
    }
}
