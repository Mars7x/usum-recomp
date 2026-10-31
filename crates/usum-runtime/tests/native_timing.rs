use usum_guest::{Code, Cpu, Exit, Fault, Memory};

use usum_runtime::{
    kernel::Kernel,
    probe::{self, Modules, Outcome},
};

fn observe(cpu: &Cpu, expected: u64) {
    assert_eq!(usum_runtime::native_timing::guest_time_ns(cpu.budget), (cpu.r[4] != 0).then_some(expected));
}

fn timed_code(cpu: &mut Cpu, _mem: &mut Memory) -> Result<Exit, Fault> {
    if cpu.r[15] == 0x1000 {
        observe(cpu, 1000);
        cpu.budget -= 100;
        observe(cpu, 1372);
        cpu.r[15] = 0x1004;
        Ok(Exit::Svc(0x28)) // Actual GetSystemTick dispatch, then a new clock scope.
    } else {
        observe(cpu, 1372);
        cpu.budget -= 200;
        observe(cpu, 2117);
        Err(Fault::NoTranslation { address: 0x1008, thumb: false })
    }
}

#[test]
fn dispatch_clock_agrees_with_kernel_accounting_across_svc() {
    let entries: [(u32, Code); 2] = [(0x1000, timed_code), (0x1004, timed_code)];
    for enabled in [false, true] {
        let mut kernel = Kernel::default();
        kernel.virtual_clock = Some(1000);
        kernel.native_timing = enabled;
        let mut cpu = Cpu::default();
        cpu.r[15] = 0x1000;
        cpu.r[4] = u32::from(enabled);
        let mut memory = Memory::new();
        let (outcome, instructions) = probe::run_with(
            &mut cpu,
            &mut memory,
            &mut kernel,
            &entries,
            &Modules { functions: &[], entries: &[], translated: &[] },
            2,
            &mut |_, _| true,
        );
        assert!(matches!(outcome, Outcome::Fault(Fault::NoTranslation { address: 0x1008, .. })));
        assert_eq!(instructions, 300);
        assert_eq!(kernel.virtual_clock, Some(2117));
        assert_eq!(usum_runtime::native_timing::guest_time_ns(cpu.budget), None);
    }
}

fn deadline_code(cpu: &mut Cpu, _mem: &mut Memory) -> Result<Exit, Fault> {
    use std::num::NonZeroU64;
    use usum_runtime::native_timing::{guest_time_ns, UpdateDeadline};
    const INTERVALS: [u64; 4] = [5_000_000, 11_111_111, 6_944_444, 1_000];
    let now = guest_time_ns(cpu.budget).unwrap();
    if cpu.r[15] == 0x2004 {
        let expected = u64::from(cpu.r[8]) | (u64::from(cpu.r[9]) << 32);
        assert_eq!(now, expected, "kernel sleep changed the native update deadline");
        cpu.r[4] += 1;
        cpu.r[15] = 0x2000;
        return Ok(Exit::Jump);
    }
    let Some(&interval) = INTERVALS.get(cpu.r[4] as usize) else {
        return Err(Fault::NoTranslation { address: 0x2008, thumb: false });
    };
    let mut deadline = UpdateDeadline::new();
    let due = deadline.arm(now, NonZeroU64::new(interval).unwrap()).unwrap();
    cpu.budget -= 300 + i64::from(cpu.r[4]) * 50;
    let after_work = guest_time_ns(cpu.budget).unwrap();
    let wait = deadline.remaining_ns(after_work).unwrap();
    let expected = due.max(after_work);
    cpu.r[8] = expected as u32;
    cpu.r[9] = (expected >> 32) as u32;
    cpu.r[0] = wait as u32;
    cpu.r[1] = (wait >> 32) as u32;
    cpu.r[15] = 0x2004;
    Ok(Exit::Svc(0x0A))
}

#[test]
fn variable_deadlines_use_real_kernel_sleep_and_do_not_replay_late_updates() {
    let mut kernel = Kernel::new(
        usum_runtime::boot::Process {
            linear_base: 0x3000_0000,
            app_region: 0x0500_0000,
            main_priority: 0x30,
            executable_end: 0x0070_0000,
            save_data_size: 0,
        },
        Vec::new(),
        Vec::new(),
    );
    kernel.virtual_clock = Some(1_000_000);
    kernel.native_timing = true;
    let mut cpu = Cpu::default();
    cpu.r[15] = 0x2000;
    let mut memory = Memory::new();
    let entries: [(u32, Code); 2] = [(0x2000, deadline_code), (0x2004, deadline_code)];
    let (outcome, instructions) = probe::run_with(
        &mut cpu,
        &mut memory,
        &mut kernel,
        &entries,
        &Modules { functions: &[], entries: &[], translated: &[] },
        2,
        &mut |_, _| true,
    );
    assert!(matches!(outcome, Outcome::Fault(Fault::NoTranslation { address: 0x2008, .. })));
    assert_eq!(cpu.r[4], 4);
    assert_eq!(instructions, 1500);
    assert_eq!(kernel.virtual_clock, Some(24_057_233));
    assert_eq!(usum_runtime::native_timing::guest_time_ns(cpu.budget), None);
}
