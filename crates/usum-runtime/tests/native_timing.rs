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
