//! Random single-instruction test programs, as assembly text.
//!
//! Each case loads r0-r12 and the flags from an input table, runs one
//! random instruction, and stores r0-r12 and the flags to an output table.
//! GNU as encodes the instruction, so the encoding never depends on our
//! decoder. Cases are grouped 100 to a function, and programs hold
//! `PER_PROGRAM` cases, so each translated module stays a moderate size.

use std::fmt::Write;

const PER_GROUP: usize = 100;
const PER_PROGRAM: usize = 5_000;
/// r0-r12 and one flags word.
const WORDS: usize = 14;

struct Rng(u64);

/// `USUM_DIFFTEST_SEED` selects a different deterministic corpus, so large
/// totals can be tested as several runs of a size one compiler can build
/// (a single 110k-case crate needed over 13 GB; RESEARCH.md R-013).
fn seed() -> u64 {
    std::env::var("USUM_DIFFTEST_SEED")
        .ok()
        .and_then(|v| v.parse::<u64>().ok())
        .map_or(0, |s| s.wrapping_mul(0x9E37_79B9_7F4A_7C15))
}

impl Rng {
    fn next(&mut self) -> u64 {
        // xorshift64*
        self.0 ^= self.0 >> 12;
        self.0 ^= self.0 << 25;
        self.0 ^= self.0 >> 27;
        self.0.wrapping_mul(0x2545_F491_4F6C_DD1D)
    }

    fn below(&mut self, n: u64) -> u64 {
        self.next() % n
    }

    fn pick<'a>(&mut self, items: &[&'a str]) -> &'a str {
        items[self.below(items.len() as u64) as usize]
    }

    fn reg(&mut self) -> String {
        format!("r{}", self.below(13))
    }

    /// Register values biased toward edge cases: small numbers (shift
    /// amounts around 32), sign boundaries, all ones.
    fn value(&mut self) -> u32 {
        match self.below(8) {
            0 => self.below(40) as u32,
            1 => [0, 1, 0x7FFF_FFFF, 0x8000_0000, 0xFFFF_FFFF, 0xFFFF_FFFE][self.below(6) as usize],
            2 => 0x100 | self.below(0x40) as u32, // register shift amounts with high bits set
            _ => self.next() as u32,
        }
    }
}

const CONDITIONS: &[&str] = &["eq", "ne", "cs", "cc", "mi", "pl", "vs", "vc", "hi", "ls", "ge", "lt", "gt", "le"];

fn cond(rng: &mut Rng) -> &'static str {
    if rng.below(2) == 0 {
        ""
    } else {
        rng.pick(CONDITIONS)
    }
}

fn s(rng: &mut Rng) -> &'static str {
    if rng.below(2) == 0 {
        ""
    } else {
        "s"
    }
}

fn operand2(rng: &mut Rng) -> String {
    match rng.below(5) {
        0 => {
            let imm8 = rng.below(256) as u32;
            let rotation = 2 * rng.below(16) as u32;
            format!("#{:#x}", imm8.rotate_right(rotation))
        }
        1 => rng.reg(),
        2 => {
            let shift = rng.pick(&["lsl", "lsr", "asr", "ror"]);
            let max = if matches!(shift, "lsl" | "ror") { 31 } else { 32 };
            // half the time an edge of the encoding: 1, 31, and 32 (encoded as 0)
            let amount = match rng.below(6) {
                0 => 1,
                1 => 31,
                2 => max,
                _ => 1 + rng.below(max),
            };
            format!("{}, {shift} #{amount}", rng.reg())
        }
        3 => format!("{}, rrx", rng.reg()),
        _ => format!("{}, {} {}", rng.reg(), rng.pick(&["lsl", "lsr", "asr", "ror"]), rng.reg()),
    }
}

fn instruction(rng: &mut Rng) -> String {
    match rng.below(10) {
        0..=4 => {
            let op = rng.pick(&["and", "eor", "sub", "rsb", "add", "adc", "sbc", "rsc", "orr", "bic"]);
            format!("{op}{}{} {}, {}, {}", s(rng), cond(rng), rng.reg(), rng.reg(), operand2(rng))
        }
        5 => format!("{}{}{} {}, {}", rng.pick(&["mov", "mvn"]), s(rng), cond(rng), rng.reg(), operand2(rng)),
        6 => format!("{}{} {}, {}", rng.pick(&["tst", "teq", "cmp", "cmn"]), cond(rng), rng.reg(), operand2(rng)),
        7 => {
            if rng.below(2) == 0 {
                format!("mul{}{} {}, {}, {}", s(rng), cond(rng), rng.reg(), rng.reg(), rng.reg())
            } else {
                format!("mla{}{} {}, {}, {}, {}", s(rng), cond(rng), rng.reg(), rng.reg(), rng.reg(), rng.reg())
            }
        }
        8 => {
            let lo = rng.below(13);
            let hi = (lo + 1 + rng.below(12)) % 13;
            let op = rng.pick(&["umull", "umlal", "smull", "smlal"]);
            format!("{op}{}{} r{lo}, r{hi}, {}, {}", s(rng), cond(rng), rng.reg(), rng.reg())
        }
        _ if rng.below(2) == 0 => format!("clz{} {}, {}", cond(rng), rng.reg(), rng.reg()),
        _ => {
            let op = rng.pick(&["sxtb", "sxth", "uxtb", "uxth", "rev", "rev16", "revsh"]);
            let rotate = if op.starts_with('r') || rng.below(2) == 0 {
                String::new()
            } else {
                format!(", ror #{}", 8 * (1 + rng.below(3)))
            };
            format!("{op}{} {}, {}{rotate}", cond(rng), rng.reg(), rng.reg())
        }
    }
}

const SUPPORT: &str = "
load_state:
    ldr r12, =in_ptr
    ldr r12, [r12]
    ldr r0, [r12, #52]
    msr cpsr_fs, r0
    ldmia r12, {r0-r12}
    bx lr
save_state:
    str r12, tmp12
    ldr r12, =out_ptr
    ldr r12, [r12]
    stmia r12, {r0-r11}
    ldr r0, tmp12
    str r0, [r12, #48]
    mrs r0, cpsr
    and r1, r0, #0x000F0000
    and r0, r0, #0xF8000000
    orr r0, r0, r1
    str r0, [r12, #52]
    add r12, r12, #56
    ldr r0, =out_ptr
    str r12, [r0]
    ldr r0, =in_ptr
    ldr r12, [r0]
    add r12, r12, #56
    str r12, [r0]
    bx lr
tmp12:
    .word 0
    .ltorg
";

fn program(rng: &mut Rng, cases: usize, generate: fn(&mut Rng) -> String) -> String {
    let mut s =
        String::from("    .syntax unified\n    .arm\n    .text\n    .global test_main\ntest_main:\n    push {r4-r11, lr}\n");
    let groups = cases.div_ceil(PER_GROUP);
    for g in 0..groups {
        writeln!(s, "    bl group_{g}").unwrap();
    }
    s += "    pop {r4-r11, pc}\n";
    for g in 0..groups {
        writeln!(s, "group_{g}:\n    push {{lr}}").unwrap();
        for _ in 0..PER_GROUP.min(cases - g * PER_GROUP) {
            writeln!(s, "    bl load_state\n    {}\n    bl save_state", generate(rng)).unwrap();
        }
        s += "    pop {pc}\n";
    }
    s += SUPPORT;
    s += "    .data\n    .balign 4\nin_ptr:\n    .word inputs\nout_ptr:\n    .word out_start\ninputs:\n";
    for _ in 0..cases {
        let mut words: Vec<u32> = (0..WORDS - 1).map(|_| rng.value()).collect();
        // NZCVQ and GE[3:0]
        words.push((rng.next() as u32 & 0xF800_0000) | (rng.next() as u32 & 0x000F_0000));
        let text: Vec<String> = words.iter().map(|w| format!("{w:#x}")).collect();
        writeln!(s, "    .word {}", text.join(", ")).unwrap();
    }
    writeln!(s, "    .global out_start, out_end\nout_start:\n    .space {}\nout_end:", cases * WORDS * 4).unwrap();
    s
}

/// The random programs for `cases` cases in total: (name, assembly).
pub fn programs(cases: usize) -> Vec<(String, String)> {
    let mut rng = Rng(0x0005_EED0_FA11_0001 ^ seed());
    let mut out = Vec::new();
    let mut left = cases;
    let mut index = 0;
    while left > 0 {
        let n = left.min(PER_PROGRAM);
        out.push((format!("random_dp_{index}"), program(&mut rng, n, instruction)));
        left -= n;
        index += 1;
    }
    out
}

// ---------------------------------------------------------------------
// Loads and stores. Base and index registers are given values that keep
// every access inside a 512-byte scratch buffer and aligned to its size:
// QEMU's ARM11 model faults on unaligned word accesses, and what the 3DS
// does is not established yet (RESEARCH.md R-010). The buffer is part of
// the output, so stores are checked too.

const SCRATCH: u32 = 512;

/// One memory case: the instruction, and register values it requires
/// (register, assembler expression).
struct MemCase {
    text: String,
    fixed: Vec<(u8, String)>,
}

fn distinct(rng: &mut Rng, avoid: &[u8]) -> u8 {
    loop {
        let r = rng.below(13) as u8;
        if !avoid.contains(&r) {
            return r;
        }
    }
}

fn mem_case(rng: &mut Rng) -> MemCase {
    let c = cond(rng);
    // base in the middle of the buffer, 8-aligned
    let base_value = format!("scratch + {}", 8 * (16 + rng.below(32)));
    match rng.below(4) {
        // word and byte
        0 | 1 => {
            let (op, align) = *[("ldr", 4), ("str", 4), ("ldrb", 1), ("strb", 1)][rng.below(4) as usize..].first().unwrap();
            let rn = rng.below(13) as u8;
            let rt = distinct(rng, &[rn]);
            let sign = if rng.below(2) == 0 { "" } else { "-" };
            let mut fixed = vec![(rn, base_value)];
            let address = match rng.below(6) {
                0 => format!("[r{rn}, #{sign}{}]", align * rng.below(30)),
                1 => format!("[r{rn}, #{sign}{}]!", align * rng.below(30)),
                2 => format!("[r{rn}], #{sign}{}", align * rng.below(30)),
                k => {
                    let rm = distinct(rng, &[rn, rt]);
                    let (shift, scale) = if align == 4 && k != 5 { (", lsl #2", 1) } else { ("", align) };
                    fixed.push((rm, format!("{}", scale * rng.below(30))));
                    match k {
                        3 => format!("[r{rn}, {sign}r{rm}{shift}]"),
                        4 => format!("[r{rn}, {sign}r{rm}{shift}]!"),
                        _ => format!("[r{rn}], {sign}r{rm}{shift}"),
                    }
                }
            };
            MemCase { text: format!("{op}{c} r{rt}, {address}"), fixed }
        }
        // halfword and signed
        2 => {
            let (op, align) = *[("ldrh", 2), ("strh", 2), ("ldrsh", 2), ("ldrsb", 1), ("ldrd", 8), ("strd", 8)]
                [rng.below(6) as usize..]
                .first()
                .unwrap();
            let double = op.ends_with('d');
            let rt = if double { 2 * rng.below(6) as u8 } else { rng.below(13) as u8 };
            let rt_list = if double { [rt, rt + 1] } else { [rt, rt] };
            let rn = distinct(rng, &rt_list);
            let pair = if double { format!("r{rt}, r{}", rt + 1) } else { format!("r{rt}") };
            let sign = if rng.below(2) == 0 { "" } else { "-" };
            let mut fixed = vec![(rn, base_value)];
            let address = match rng.below(5) {
                0 => format!("[r{rn}, #{sign}{}]", align * rng.below(16)),
                1 => format!("[r{rn}, #{sign}{}]!", align * rng.below(16)),
                2 => format!("[r{rn}], #{sign}{}", align * rng.below(16)),
                k => {
                    let rm = distinct(rng, &[rn, rt_list[0], rt_list[1]]);
                    fixed.push((rm, format!("{}", align * rng.below(16))));
                    if k == 3 {
                        format!("[r{rn}, {sign}r{rm}]")
                    } else {
                        format!("[r{rn}, {sign}r{rm}]!")
                    }
                }
            };
            MemCase { text: format!("{op}{c} {pair}, {address}"), fixed }
        }
        // block transfers; the base is never in the list
        _ => {
            let op = rng.pick(&["ldmia", "ldmib", "ldmda", "ldmdb", "stmia", "stmib", "stmda", "stmdb"]);
            let rn = rng.below(13) as u8;
            let mut list: Vec<u8> = (0..13u8).filter(|&r| r != rn && rng.below(3) == 0).collect();
            if list.is_empty() {
                list.push(distinct(rng, &[rn]));
            }
            let regs: Vec<String> = list.iter().map(|r| format!("r{r}")).collect();
            let wb = if rng.below(2) == 0 { "" } else { "!" };
            // UAL puts the condition after the mode: ldmiaeq
            MemCase { text: format!("{op}{c} r{rn}{wb}, {{{}}}", regs.join(", ")), fixed: vec![(rn, base_value)] }
        }
    }
}

fn mem_program(rng: &mut Rng, cases: usize) -> String {
    let mut s =
        String::from("    .syntax unified\n    .arm\n    .text\n    .global test_main\ntest_main:\n    push {r4-r11, lr}\n");
    let groups = cases.div_ceil(PER_GROUP);
    for g in 0..groups {
        writeln!(s, "    bl group_{g}").unwrap();
    }
    s += "    pop {r4-r11, pc}\n";
    let mut inputs = Vec::new();
    for g in 0..groups {
        writeln!(s, "group_{g}:\n    push {{lr}}").unwrap();
        for _ in 0..PER_GROUP.min(cases - g * PER_GROUP) {
            let case = mem_case(rng);
            writeln!(s, "    bl load_state\n    {}\n    bl save_state", case.text).unwrap();
            let mut words: Vec<String> = (0..WORDS - 1).map(|_| format!("{:#x}", rng.value())).collect();
            for (reg, value) in case.fixed {
                words[reg as usize] = value;
            }
            words.push(format!("{:#x}", (rng.next() as u32 & 0xF800_0000) | (rng.next() as u32 & 0x000F_0000)));
            inputs.push(words.join(", "));
        }
        s += "    pop {pc}\n";
    }
    s += SUPPORT;
    s += "    .data\n    .balign 8\nin_ptr:\n    .word inputs\nout_ptr:\n    .word out_start\ninputs:\n";
    for line in inputs {
        writeln!(s, "    .word {line}").unwrap();
    }
    writeln!(s, "    .global out_start, out_end\nout_start:\n    .space {}\n    .balign 8\nscratch:", cases * WORDS * 4).unwrap();
    for i in 0..SCRATCH {
        writeln!(s, "    .byte {:#x}", (i.wrapping_mul(0x9D) ^ (i >> 3)) as u8).unwrap();
    }
    s += "out_end:\n";
    s
}

/// The random load/store programs for `cases` cases in total.
pub fn mem_programs(cases: usize) -> Vec<(String, String)> {
    let mut rng = Rng(0x0005_EED0_FA11_0002 ^ seed());
    let mut out = Vec::new();
    let mut left = cases;
    let mut index = 0;
    while left > 0 {
        let n = left.min(PER_PROGRAM);
        out.push((format!("random_mem_{index}"), mem_program(&mut rng, n)));
        left -= n;
        index += 1;
    }
    out
}

// ---------------------------------------------------------------------
// Thumb. Each group is a Thumb function that calls the ARM load_state and
// save_state with BLX, so every case also exercises interworking.

fn low(rng: &mut Rng) -> String {
    format!("r{}", rng.below(8))
}

fn high(rng: &mut Rng) -> String {
    format!("r{}", 8 + rng.below(5))
}

fn thumb_instruction(rng: &mut Rng) -> String {
    match rng.below(12) {
        0 => {
            let op = rng.pick(&["lsls", "lsrs", "asrs"]);
            let max = if op == "lsls" { 31 } else { 32 };
            let amount = match rng.below(4) {
                0 => max,
                1 => 1,
                _ => 1 + rng.below(max),
            };
            format!("{op} {}, {}, #{amount}", low(rng), low(rng))
        }
        1 => format!("{} {}, {}, {}", rng.pick(&["adds", "subs"]), low(rng), low(rng), low(rng)),
        2 => format!("{} {}, {}, #{}", rng.pick(&["adds", "subs"]), low(rng), low(rng), rng.below(8)),
        3 => format!("{} {}, #{}", rng.pick(&["movs", "cmp", "adds", "subs"]), low(rng), rng.below(256)),
        4..=6 => {
            let op = rng.pick(&[
                "ands", "eors", "lsls", "lsrs", "asrs", "adcs", "sbcs", "rors", "tst", "cmp", "cmn", "orrs", "bics", "mvns",
            ]);
            format!("{op} {}, {}", low(rng), low(rng))
        }
        7 => format!("negs {}, {}", low(rng), low(rng)),
        8 => {
            let rd = low(rng);
            format!("muls {rd}, {}, {rd}", low(rng))
        }
        9 => {
            // high-register forms: at least one high register, never PC
            let (a, b) = if rng.below(2) == 0 { (high(rng), rng.reg()) } else { (rng.reg(), high(rng)) };
            format!("{} {a}, {b}", rng.pick(&["add", "mov", "cmp"]))
        }
        _ => format!("{} {}, {}", rng.pick(&["sxtb", "sxth", "uxtb", "uxth", "rev", "rev16", "revsh"]), low(rng), low(rng)),
    }
}

fn thumb_program(rng: &mut Rng, cases: usize) -> String {
    let mut s =
        String::from("    .syntax unified\n    .text\n    .arm\n    .global test_main\ntest_main:\n    push {r4-r11, lr}\n");
    let groups = cases.div_ceil(PER_GROUP);
    for g in 0..groups {
        writeln!(s, "    blx group_{g}").unwrap();
    }
    s += "    pop {r4-r11, pc}\n";
    s += SUPPORT;
    s += "    .thumb\n";
    for g in 0..groups {
        writeln!(s, "    .thumb_func\ngroup_{g}:\n    push {{lr}}").unwrap();
        for _ in 0..PER_GROUP.min(cases - g * PER_GROUP) {
            writeln!(s, "    blx load_state\n    {}\n    blx save_state", thumb_instruction(rng)).unwrap();
        }
        s += "    pop {pc}\n";
    }
    s += "    .data\n    .balign 4\nin_ptr:\n    .word inputs\nout_ptr:\n    .word out_start\ninputs:\n";
    for _ in 0..cases {
        let mut words: Vec<u32> = (0..WORDS - 1).map(|_| rng.value()).collect();
        words.push((rng.next() as u32 & 0xF800_0000) | (rng.next() as u32 & 0x000F_0000));
        let text: Vec<String> = words.iter().map(|w| format!("{w:#x}")).collect();
        writeln!(s, "    .word {}", text.join(", ")).unwrap();
    }
    writeln!(s, "    .global out_start, out_end\nout_start:\n    .space {}\nout_end:", cases * WORDS * 4).unwrap();
    s
}

/// The random Thumb programs for `cases` cases in total.
pub fn thumb_programs(cases: usize) -> Vec<(String, String)> {
    let mut rng = Rng(0x0005_EED0_FA11_0003 ^ seed());
    let mut out = Vec::new();
    let mut left = cases;
    let mut index = 0;
    while left > 0 {
        let n = left.min(PER_PROGRAM);
        out.push((format!("random_thumb_{index}"), thumb_program(&mut rng, n)));
        left -= n;
        index += 1;
    }
    out
}

// ---------------------------------------------------------------------
// VFP. Records hold r0-r12, the flags, s0-s31 and FPSCR (47 words); the
// cumulative exception flags are masked out of FPSCR (RESEARCH.md R-017).

const VWORDS: usize = 47;

const VFP_SUPPORT: &str = "
vload_state:
    ldr r12, =in_ptr
    ldr r12, [r12]
    add r0, r12, #56
    vldmia r0, {s0-s31}
    ldr r0, [r12, #184]
    vmsr fpscr, r0
    ldr r0, [r12, #52]
    msr cpsr_fs, r0
    ldmia r12, {r0-r12}
    bx lr
vsave_state:
    str r12, vtmp12
    ldr r12, =out_ptr
    ldr r12, [r12]
    stmia r12, {r0-r11}
    ldr r0, vtmp12
    str r0, [r12, #48]
    mrs r0, cpsr
    and r1, r0, #0x000F0000
    and r0, r0, #0xF8000000
    orr r0, r0, r1
    str r0, [r12, #52]
    add r0, r12, #56
    vstmia r0, {s0-s31}
    vmrs r0, fpscr
    bic r0, r0, #0x9F
    str r0, [r12, #184]
    add r12, r12, #188
    ldr r0, =out_ptr
    str r12, [r0]
    ldr r0, =in_ptr
    ldr r12, [r0]
    add r12, r12, #188
    str r12, [r0]
    bx lr
vtmp12:
    .word 0
    .ltorg
";

const SPECIAL32: &[u32] = &[
    0x0000_0000,
    0x8000_0000,
    0x7F80_0000,
    0xFF80_0000, // zeros, infinities
    0x7FC0_0000,
    0xFFC1_2345,
    0x7F80_0001,
    0xFFA0_0000, // quiet and signaling NaNs
    0x0000_0001,
    0x807F_FFFF,
    0x0080_0000,
    0x8080_0000, // subnormals, min normals
    0x7F7F_FFFF,
    0x3F80_0000,
    0xC020_0000,
    0x3F00_0000, // max, 1.0, -2.5, 0.5
    0x4B80_0000,
    0x4F00_0000,
    0x4F80_0000,
    0xCF00_0001, // 2^24, 2^31, 2^32, past -2^31
    0x3FC0_0000,
    0x4020_0000,
    0xBF40_0000,
    0x0040_0000, // 1.5, 2.5, -0.75, a subnormal
];

const SPECIAL64: &[u64] = &[
    0,
    0x8000_0000_0000_0000,
    0x7FF0_0000_0000_0000,
    0xFFF0_0000_0000_0000,
    0x7FF8_0000_0000_0000,
    0xFFF8_1234_5678_9ABC,
    0x7FF0_0000_0000_0001,
    0xFFF4_0000_0000_0000,
    0x0000_0000_0000_0001,
    0x800F_FFFF_FFFF_FFFF,
    0x0010_0000_0000_0000,
    0x7FEF_FFFF_FFFF_FFFF,
    0x3FF0_0000_0000_0000,
    0xC004_0000_0000_0000,
    0x41E0_0000_0000_0000,
    0x41F0_0000_0000_0000,
    0x3FF8_0000_0000_0000,
    0x3810_0000_0000_0000,
    0x47F0_0000_0000_0000,
    0xC1E0_0000_0020_0000,
];

impl Rng {
    fn f32_bits(&mut self) -> u32 {
        match self.below(3) {
            0 => SPECIAL32[self.below(SPECIAL32.len() as u64) as usize],
            // a "nice" value: small integer or fraction, either sign
            1 => {
                let v = (self.below(2001) as f32 - 1000.0) / [1.0, 4.0, 3.0, 1000.0][self.below(4) as usize];
                v.to_bits()
            }
            _ => self.next() as u32,
        }
    }

    fn f64_bits(&mut self) -> u64 {
        match self.below(3) {
            0 => SPECIAL64[self.below(SPECIAL64.len() as u64) as usize],
            1 => ((self.below(2001) as f64 - 1000.0) / [1.0, 4.0, 3.0, 1000.0][self.below(4) as usize]).to_bits(),
            _ => self.next(),
        }
    }
}

/// Doubles near the edges math routines branch on: integer and rounding
/// boundaries, powers of two, the special values.
const DIRECTED64: &[f64] = &[
    0.0,
    -0.0,
    0.5,
    -0.5,
    1.0,
    -1.0,
    1.5,
    2.5,
    -2.5,
    0.49999999999999994,
    4503599627370496.0,
    4503599627370497.0,
    9007199254740992.0,
    2147483648.0,
    -2147483648.0,
    4294967296.0,
    1e-300,
    2.2250738585072014e-308,
    1.7976931348623157e308,
    f64::INFINITY,
    f64::NEG_INFINITY,
    f64::NAN,
    std::f64::consts::PI,
    1e20,
    -1e20,
    0.1,
];

impl Rng {
    fn directed64(&mut self) -> u64 {
        match self.below(4) {
            0 => DIRECTED64[self.below(DIRECTED64.len() as u64) as usize].to_bits(),
            // a power of two times a small integer, either sign
            1 => {
                let v = (self.below(64) as f64 - 32.0) * 2f64.powi(self.below(120) as i32 - 60);
                v.to_bits()
            }
            _ => self.f64_bits(),
        }
    }
}

/// One VFP case: the text, whether it uses doubles, whether it may run in
/// short-vector mode, and whether its rounding mode may be directed (only
/// where the translator supports it: conversions, compares, moves).
struct VfpCase {
    text: String,
    double: bool,
    vector: bool,
    directed: bool,
}

fn vfp_case(rng: &mut Rng) -> VfpCase {
    let double = rng.below(3) == 0;
    let (p, reg) = if double { ("f64", "d") } else { ("f32", "s") };
    let max = if double { 16 } else { 32 };
    let mut r = || format!("{reg}{}", rng.below(max));
    let (a, b, c) = (r(), r(), r());
    match rng.below(10) {
        0..=3 => {
            let op = rng.pick(&["vmla", "vmls", "vnmla", "vnmls", "vmul", "vnmul", "vadd", "vsub", "vdiv"]);
            VfpCase { text: format!("{op}.{p} {a}, {b}, {c}"), double, vector: true, directed: false }
        }
        // two-operand double-precision vectors are not compared with QEMU:
        // its do_vfp_2op_dp advances the destination from the source
        // register (RESEARCH.md R-018); a usum-guest unit test covers them
        4 => {
            let op = rng.pick(&["vmov", "vabs", "vneg"]);
            VfpCase { text: format!("{op}.{p} {a}, {b}"), double, vector: !double, directed: true }
        }
        5 => VfpCase { text: format!("vsqrt.{p} {a}, {b}"), double, vector: !double, directed: false },
        6 => {
            let op = rng.pick(&["vcmp", "vcmpe"]);
            let rhs = if rng.below(4) == 0 { "#0".to_owned() } else { b };
            let tail = if rng.below(2) == 0 { "\n    vmrs APSR_nzcv, fpscr" } else { "" };
            VfpCase { text: format!("{op}.{p} {a}, {rhs}{tail}"), double, vector: false, directed: true }
        }
        _ => {
            let s = |rng: &mut Rng| format!("s{}", rng.below(32));
            let d = |rng: &mut Rng| format!("d{}", rng.below(16));
            let text = match rng.below(5) {
                0 => format!("vcvt.f64.f32 {}, {}", d(rng), s(rng)),
                1 => format!("vcvt.f32.f64 {}, {}", s(rng), d(rng)),
                2 => {
                    let int = rng.pick(&["s32", "u32"]);
                    if double {
                        format!("vcvt.f64.{int} {}, {}", d(rng), s(rng))
                    } else {
                        format!("vcvt.f32.{int} {}, {}", s(rng), s(rng))
                    }
                }
                _ => {
                    let op = rng.pick(&["vcvt", "vcvtr"]);
                    let int = rng.pick(&["s32", "u32"]);
                    let src = if double { d(rng) } else { s(rng) };
                    format!("{op}.{int}.{p} {}, {src}", s(rng))
                }
            };
            VfpCase { text, double, vector: false, directed: true }
        }
    }
}

/// A legal FPSCR.LEN/STRIDE pair for the precision (bank of 8 singles or
/// 4 doubles, length * stride within the bank).
///
/// Stride 2 is only generated for doubles: QEMU 8.2.2 steps single-precision
/// vectors by STRIDE + 1 on the raw field, so 0b11 gives 4 instead of the
/// architectural 2 (RESEARCH.md R-018). That case is covered by a unit test
/// in usum-guest instead, against values derived from the ARM ARM.
fn vector_bits(rng: &mut Rng, double: bool) -> u32 {
    let bank: u64 = if double { 4 } else { 8 };
    let stride = if double && rng.below(3) == 0 { 2 } else { 1 };
    let len = 2 + rng.below(bank / stride - 1); // 2 ..= bank / stride
    let stride_field = if stride == 2 { 3 } else { 0 };
    ((len as u32 - 1) << 16) | (stride_field << 20)
}

fn vfp_program(rng: &mut Rng, cases: usize) -> String {
    let mut s = String::from(
        "    .syntax unified\n    .arm\n    .fpu vfpv2\n    .text\n    .global test_main\ntest_main:\n    push {r4-r11, lr}\n",
    );
    let groups = cases.div_ceil(PER_GROUP);
    for g in 0..groups {
        writeln!(s, "    bl group_{g}").unwrap();
    }
    s += "    pop {r4-r11, pc}\n";
    let mut inputs = Vec::new();
    for g in 0..groups {
        writeln!(s, "group_{g}:\n    push {{lr}}").unwrap();
        for _ in 0..PER_GROUP.min(cases - g * PER_GROUP) {
            let case = vfp_case(rng);
            writeln!(s, "    bl vload_state\n    {}\n    bl vsave_state", case.text).unwrap();
            let mut words: Vec<u32> = (0..13).map(|_| rng.value()).collect();
            words.push((rng.next() as u32 & 0xF800_0000) | (rng.next() as u32 & 0x000F_0000));
            if case.double {
                for _ in 0..16 {
                    let v = rng.f64_bits();
                    words.extend([v as u32, (v >> 32) as u32]);
                }
            } else {
                words.extend((0..32).map(|_| rng.f32_bits()));
            }
            // FPSCR: random NZCV, FZ, DN; rounding and vectors where allowed
            let mut fpscr = rng.next() as u32 & 0xF300_0000;
            if case.directed {
                fpscr |= (rng.below(4) as u32) << 22;
            }
            if case.vector && rng.below(3) == 0 {
                fpscr |= vector_bits(rng, case.double);
            }
            words.push(fpscr);
            inputs.push(words.iter().map(|w| format!("{w:#x}")).collect::<Vec<_>>().join(", "));
        }
        s += "    pop {pc}\n";
    }
    s += VFP_SUPPORT;
    s += "    .data\n    .balign 4\nin_ptr:\n    .word inputs\nout_ptr:\n    .word out_start\ninputs:\n";
    for line in inputs {
        writeln!(s, "    .word {line}").unwrap();
    }
    writeln!(s, "    .global out_start, out_end\nout_start:\n    .space {}\nout_end:", cases * VWORDS * 4).unwrap();
    s
}

/// The random VFP programs for `cases` cases in total.
pub fn vfp_programs(cases: usize) -> Vec<(String, String)> {
    let mut rng = Rng(0x0005_EED0_FA11_0004 ^ seed());
    let mut out = Vec::new();
    let mut left = cases;
    let mut index = 0;
    while left > 0 {
        let n = left.min(PER_PROGRAM);
        out.push((format!("random_vfp_{index}"), vfp_program(&mut rng, n)));
        left -= n;
        index += 1;
    }
    out
}

// ---------------------------------------------------------------------
// ARMv6 media and DSP instructions (GE and Q flags come from the random
// flags word and are part of every record).

fn media_instruction(rng: &mut Rng) -> String {
    let c = cond(rng);
    let (d, n, m, a) = (rng.reg(), rng.reg(), rng.reg(), rng.reg());
    match rng.below(14) {
        0..=3 => {
            let prefix = rng.pick(&["s", "q", "sh", "u", "uq", "uh"]);
            let op = rng.pick(&["add16", "asx", "sax", "sub16", "add8", "sub8"]);
            format!("{prefix}{op}{c} {d}, {n}, {m}")
        }
        4 => format!("sel{c} {d}, {n}, {m}"),
        5 => {
            if rng.below(2) == 0 {
                format!("pkhbt{c} {d}, {n}, {m}, lsl #{}", rng.below(32))
            } else {
                format!("pkhtb{c} {d}, {n}, {m}, asr #{}", 1 + rng.below(32))
            }
        }
        6 => {
            let shift = match rng.below(3) {
                0 => String::new(),
                1 => format!(", lsl #{}", rng.below(32)),
                _ => format!(", asr #{}", 1 + rng.below(32)),
            };
            if rng.below(2) == 0 {
                format!("ssat{c} {d}, #{}, {m}{shift}", 1 + rng.below(32))
            } else {
                format!("usat{c} {d}, #{}, {m}{shift}", rng.below(32))
            }
        }
        7 => {
            if rng.below(2) == 0 {
                format!("ssat16{c} {d}, #{}, {m}", 1 + rng.below(16))
            } else {
                format!("usat16{c} {d}, #{}, {m}", rng.below(16))
            }
        }
        8 => {
            let op = rng.pick(&["sxtab", "sxtah", "uxtab", "uxtah", "sxtab16", "uxtab16", "sxtb16", "uxtb16"]);
            let rotate = if rng.below(2) == 0 { String::new() } else { format!(", ror #{}", 8 * (1 + rng.below(3))) };
            if op.ends_with("b16") && !op.contains("ab") {
                format!("{op}{c} {d}, {m}{rotate}")
            } else {
                format!("{op}{c} {d}, {n}, {m}{rotate}")
            }
        }
        9 => {
            let (x, y) = (rng.pick(&["b", "t"]), rng.pick(&["b", "t"]));
            match rng.below(5) {
                0 => format!("smul{x}{y}{c} {d}, {n}, {m}"),
                1 => format!("smla{x}{y}{c} {d}, {n}, {m}, {a}"),
                2 => format!("smulw{y}{c} {d}, {n}, {m}"),
                3 => format!("smlaw{y}{c} {d}, {n}, {m}, {a}"),
                _ => {
                    let lo = rng.below(13);
                    let hi = (lo + 1 + rng.below(12)) % 13;
                    format!("smlal{x}{y}{c} r{lo}, r{hi}, {n}, {m}")
                }
            }
        }
        10 => {
            let x = if rng.below(2) == 0 { "" } else { "x" };
            match rng.below(6) {
                0 => format!("smuad{x}{c} {d}, {n}, {m}"),
                1 => format!("smusd{x}{c} {d}, {n}, {m}"),
                2 => format!("smlad{x}{c} {d}, {n}, {m}, {a}"),
                3 => format!("smlsd{x}{c} {d}, {n}, {m}, {a}"),
                _ => {
                    let lo = rng.below(13);
                    let hi = (lo + 1 + rng.below(12)) % 13;
                    format!("{}{x}{c} r{lo}, r{hi}, {n}, {m}", rng.pick(&["smlald", "smlsld"]))
                }
            }
        }
        11 => {
            let r = if rng.below(2) == 0 { "" } else { "r" };
            match rng.below(3) {
                0 => format!("smmul{r}{c} {d}, {n}, {m}"),
                1 => format!("smmla{r}{c} {d}, {n}, {m}, {a}"),
                _ => format!("smmls{r}{c} {d}, {n}, {m}, {a}"),
            }
        }
        12 => {
            if rng.below(2) == 0 {
                format!("usad8{c} {d}, {n}, {m}")
            } else {
                format!("usada8{c} {d}, {n}, {m}, {a}")
            }
        }
        _ => {
            if rng.below(3) == 0 {
                let lo = rng.below(13);
                let hi = (lo + 1 + rng.below(12)) % 13;
                format!("umaal{c} r{lo}, r{hi}, {n}, {m}")
            } else {
                format!("{}{c} {d}, {m}, {n}", rng.pick(&["qadd", "qsub", "qdadd", "qdsub"]))
            }
        }
    }
}

/// The random media/DSP programs for `cases` cases in total.
pub fn media_programs(cases: usize) -> Vec<(String, String)> {
    let mut rng = Rng(0x0005_EED0_FA11_0005 ^ seed());
    let mut out = Vec::new();
    let mut left = cases;
    let mut index = 0;
    while left > 0 {
        let n = left.min(PER_PROGRAM);
        out.push((format!("random_media_{index}"), program(&mut rng, n, media_instruction)));
        left -= n;
        index += 1;
    }
    out
}

// ---------------------------------------------------------------------
// Game mode: call one real game function (by address) with random
// register inputs; record the registers and flags it returns.

pub fn game_program(address: u32, cases: usize) -> String {
    let mut rng = Rng(0x0005_EED0_FA11_0006 ^ seed() ^ u64::from(address));
    let mut s = format!("    .syntax unified\n    .arm\n    .fpu vfpv2\n    .text\n    .set game_function, {address:#x}\n    .global test_main\ntest_main:\n    push {{r4-r11, lr}}\n");
    let groups = cases.div_ceil(PER_GROUP);
    for g in 0..groups {
        writeln!(s, "    bl group_{g}").unwrap();
    }
    s += "    pop {r4-r11, pc}\n";
    for g in 0..groups {
        writeln!(s, "group_{g}:\n    push {{lr}}").unwrap();
        for _ in 0..PER_GROUP.min(cases - g * PER_GROUP) {
            s += "    bl vload_state\n    bl game_function\n    bl vsave_state\n";
        }
        s += "    pop {pc}\n";
    }
    s += VFP_SUPPORT;
    s += "    .data\n    .balign 4\nin_ptr:\n    .word inputs\nout_ptr:\n    .word out_start\ninputs:\n";
    for _ in 0..cases {
        let mut words: Vec<u32> = (0..13).map(|_| rng.value()).collect();
        // a quarter of the cases pass equal operands, so equality paths run
        let equal = rng.below(4) == 0;
        // half the cases pass doubles in r0:r1 and r2:r3, as the soft-float ABI does
        if rng.below(2) == 0 {
            let a = rng.directed64();
            let b = if equal { a } else { rng.directed64() };
            words[..4].copy_from_slice(&[a as u32, (a >> 32) as u32, b as u32, (b >> 32) as u32]);
        } else if equal {
            words[1] = words[0];
        }
        words.push((rng.next() as u32 & 0xF800_0000) | (rng.next() as u32 & 0x000F_0000));
        // VFP registers: doubles, for functions taking them in d0-d7
        let first = rng.directed64();
        for i in 0..16 {
            let v = if i == 0 || (i == 1 && equal) { first } else { rng.directed64() };
            words.extend([v as u32, (v >> 32) as u32]);
        }
        // FPSCR: RunFast-like or IEEE settings, round to nearest, scalar
        words.push(rng.next() as u32 & 0x0300_0000);
        writeln!(s, "    .word {}", words.iter().map(|w| format!("{w:#x}")).collect::<Vec<_>>().join(", ")).unwrap();
    }
    writeln!(s, "    .global out_start, out_end\nout_start:\n    .space {}\nout_end:", cases * VWORDS * 4).unwrap();
    s
}
