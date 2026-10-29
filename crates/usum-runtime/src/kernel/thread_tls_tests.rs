use super::*;

fn setup(perm: Perm) -> (Kernel, Cpu, Memory) {
    let process = Process {
        linear_base: 0x3000_0000,
        app_region: 0x0500_0000,
        main_priority: 0x30,
        executable_end: 0x0070_0000,
        save_data_size: 0,
    };
    let mut kernel = Kernel::new(process, Vec::new(), Vec::new());
    kernel.thread_lifetime_reference = false;
    kernel.use_virtual_clock();
    let cpu = Cpu { tls: TLS, ..Cpu::default() };
    kernel.threads[0].cpu = cpu.clone();
    let mut mem = Memory::new();
    mem.map(TLS, vec![0; (TLS_SLOT * TLS_SLOTS) as usize], perm).unwrap();
    (kernel, cpu, mem)
}

fn spawn(kernel: &mut Kernel, cpu: &mut Cpu, mem: &mut Memory) -> Outcome {
    (cpu.r[0], cpu.r[1], cpu.r[2], cpu.r[3], cpu.r[4]) = (0x30, 0x0020_0001, 0x1234, 0x0800_1004, (-2i32) as u32);
    kernel.svc(cpu, mem, 0x08)
}

fn exit(kernel: &mut Kernel, cpu: &mut Cpu, mem: &mut Memory, index: usize) {
    kernel.current = index;
    *cpu = kernel.threads[index].cpu.clone();
    assert!(matches!(kernel.svc(cpu, mem, 0x09), Outcome::Switch));
    kernel.current = 0;
    *cpu = kernel.threads[0].cpu.clone();
}

#[test]
fn exited_threads_release_storage_without_aliasing_retained_join_handles() {
    let (mut kernel, mut cpu, mut mem) = setup(Perm::RW);
    let mut handles = Vec::new();
    let mut ids = std::collections::BTreeSet::new();
    let mut reused = false;
    for lifetime in 1..=128 {
        assert!(matches!(spawn(&mut kernel, &mut cpu, &mut mem), Outcome::Continue));
        assert_eq!(cpu.r[0], RESULT_SUCCESS);
        let handle = cpu.r[1];
        let index = kernel.threads.len() - 1;
        let thread = &kernel.threads[index];
        assert!(ids.insert(thread.id));
        let tls = thread.cpu.tls;
        assert_ne!(tls, TLS, "main retains its storage");
        assert!(tls < TLS + TLS_SLOT * TLS_SLOTS);
        assert_eq!(mem.read_range(tls, TLS_SLOT).unwrap(), vec![0; TLS_SLOT as usize]);
        if lifetime < TLS_SLOTS {
            assert_eq!(tls, TLS + lifetime * TLS_SLOT, "initial addresses retain reference placement");
        } else {
            reused = true;
        }
        assert!(!kernel.signaled(handle, MAIN_THREAD_ID));
        assert!(handles.iter().all(|old| kernel.signaled(*old, MAIN_THREAD_ID)));
        mem.write_range(tls, &[0xa7; TLS_SLOT as usize]).unwrap();
        exit(&mut kernel, &mut cpu, &mut mem, index);
        assert!(kernel.signaled(handle, MAIN_THREAD_ID));
        handles.push(handle);
    }
    assert!(reused);
    assert_eq!(kernel.virtual_clock, Some(0), "allocation cannot change simulation time");
}

#[test]
fn simultaneous_threads_exhaust_capacity_but_an_exit_frees_exactly_one_slot() {
    let (mut kernel, mut cpu, mut mem) = setup(Perm::RW);
    let mut addresses = std::collections::BTreeSet::from([TLS]);
    for _ in 1..TLS_SLOTS {
        assert!(matches!(spawn(&mut kernel, &mut cpu, &mut mem), Outcome::Continue));
        assert!(addresses.insert(kernel.threads.last().unwrap().cpu.tls));
    }
    let count = kernel.threads.len();
    let id = kernel.next_thread_id;
    assert!(
        matches!(spawn(&mut kernel, &mut cpu, &mut mem), Outcome::Unimplemented(message) if message.contains("live threads"))
    );
    assert_eq!((kernel.threads.len(), kernel.next_thread_id), (count, id));
    let freed = kernel.threads[7].cpu.tls;
    exit(&mut kernel, &mut cpu, &mut mem, 7);
    assert!(matches!(spawn(&mut kernel, &mut cpu, &mut mem), Outcome::Continue));
    assert_eq!(kernel.threads.last().unwrap().cpu.tls, freed);
    assert!(matches!(spawn(&mut kernel, &mut cpu, &mut mem), Outcome::Unimplemented(_)));
}

#[test]
fn legacy_policy_and_recycled_storage_faults_leave_thread_state_unallocated() {
    for reference in [false, true] {
        let (mut kernel, mut cpu, mut mem) = setup(Perm::R);
        kernel.thread_lifetime_reference = reference;
        for _ in 1..TLS_SLOTS {
            assert!(matches!(spawn(&mut kernel, &mut cpu, &mut mem), Outcome::Continue));
            let index = kernel.threads.len() - 1;
            exit(&mut kernel, &mut cpu, &mut mem, index);
        }
        let count = kernel.threads.len();
        let id = kernel.next_thread_id;
        let Outcome::Unimplemented(message) = spawn(&mut kernel, &mut cpu, &mut mem) else {
            panic!("expected legacy limit or a read-only TLS fault");
        };
        assert_eq!(message.contains("cannot initialize recycled TLS"), !reference);
        assert_eq!((kernel.threads.len(), kernel.next_thread_id), (count, id));
        assert_eq!(mem.read_range(TLS, TLS_SLOT * TLS_SLOTS).unwrap(), vec![0; (TLS_SLOT * TLS_SLOTS) as usize]);
    }
}
