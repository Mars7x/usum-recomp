use super::*;

const BASE: u32 = 0x1000_0000;

fn fixture() -> (Kernel, Memory) {
    let mut kernel = Kernel::default();
    let event = kernel.open(Object::Event { signaled: false, reset: 1 });
    let memory = kernel.open(Object::SharedMemory { size: 0x1000, mapped: Some(BASE) });
    kernel.gsp = Some(Gsp { flags: 0, event, memory });
    kernel.virtual_clock = Some(5_000_000);
    kernel.next_vblank = 16_713_680;
    kernel.next_audio_frame = 4_888_780;
    let mut mem = Memory::new();
    mem.map(BASE, vec![0; 0x1000], Perm::RW).unwrap();
    (kernel, mem)
}

fn submit(mem: &mut Memory, screen: u32, index: u8, left: u32) {
    let info = BASE + 0x200 + screen * 0x40;
    mem.write_u8(info, index).unwrap();
    mem.write_u8(info + 1, 0x81).unwrap();
    let entry = info + 4 + u32::from(index) * 0x1c;
    for (offset, value) in [u32::from(index), left, left + 0x100, 720, 1, 0, 0].into_iter().enumerate() {
        mem.write_u32(entry + offset as u32 * 4, value).unwrap();
    }
}

#[test]
fn native_submission_acknowledges_only_fresh_screens_without_hardware_ticks() {
    let (mut kernel, mut mem) = fixture();
    submit(&mut mem, 0, 1, 0x3000_0000);
    assert_eq!(kernel.present_native_frame(&mut mem).unwrap(), [true, false]);
    assert_eq!(
        kernel.presented,
        [Present { screen: 0, active: 1, left: 0x3000_0000, right: 0x3000_0100, stride: 720, format: 1 }]
    );
    assert_eq!(mem.read_u8(BASE + 0x201).unwrap(), 0x80);
    assert_eq!(kernel.present_native_frame(&mut mem).unwrap(), [false, false]);
    assert_eq!(kernel.presented.len(), 1);
    submit(&mut mem, 1, 0, 0x3100_0000);
    assert_eq!(kernel.present_native_frame(&mut mem).unwrap(), [false, true]);
    assert_eq!(kernel.presented.len(), 2);
    assert_eq!((kernel.vblanks, kernel.audio_frames), (0, 0));
    assert_eq!(kernel.virtual_clock, Some(5_000_000));
    assert_eq!((kernel.next_vblank, kernel.next_audio_frame), (16_713_680, 4_888_780));
    assert_eq!(mem.read_u8(BASE + gsp::interrupt_queue(0) + 1).unwrap(), 0);
    assert!(matches!(kernel.object(kernel.gsp.unwrap().event), Some(Object::Event { signaled: false, .. })));
}

#[test]
fn normal_vblank_still_consumes_submissions_and_queues_both_display_interrupts() {
    let (mut kernel, mut mem) = fixture();
    submit(&mut mem, 0, 0, 0x3000_0000);
    submit(&mut mem, 1, 1, 0x3100_0000);
    kernel.vblank(&mut mem).unwrap();
    assert_eq!(kernel.presented.len(), 2);
    assert_eq!(kernel.vblanks, 1);
    assert_eq!(kernel.audio_frames, 0);
    assert_eq!(mem.read_u8(BASE + 0x201).unwrap(), 0x80);
    assert_eq!(mem.read_u8(BASE + 0x241).unwrap(), 0x80);
    assert_eq!(mem.read_u8(BASE + gsp::interrupt_queue(0) + 1).unwrap(), 2);
    assert!(matches!(kernel.object(kernel.gsp.unwrap().event), Some(Object::Event { signaled: true, .. })));
}

#[test]
fn native_publication_delivers_pixels_once_without_incrementing_mailbox_vblanks() {
    use std::sync::{
        atomic::{AtomicU32, Ordering},
        Arc, Mutex,
    };
    let (mut kernel, mut mem) = fixture();
    mem.map(0x3000_0000, vec![0x12; 400 * 720], Perm::RW).unwrap();
    let notifications = Arc::new(AtomicU32::new(0));
    let observer = notifications.clone();
    let out = Arc::new(FrameOut {
        frames: Mutex::new(crate::timing::FrameMailbox::default()),
        resolution_scale: AtomicU32::new(1),
        dialog: Mutex::new(None),
        notify: Box::new(move || {
            observer.fetch_add(1, Ordering::Relaxed);
        }),
    });
    kernel.frame_out = Some(out.clone());
    submit(&mut mem, 0, 0, 0x3000_0000);
    assert_eq!(kernel.present_native_frame(&mut mem).unwrap(), [true, false]);
    {
        let frames = out.frames.lock().unwrap();
        assert!(frames.frames[0].is_some());
        assert!(frames.frames[1].is_none());
        assert_eq!(frames.versions, [1, 0]);
        assert_eq!(frames.stamp.updates, [1, 0]);
        assert_eq!(frames.stamp.vblanks, 0);
        assert_eq!(frames.stamp.guest_ns, 5_000_000);
    }
    kernel.present_native_frame(&mut mem).unwrap();
    assert_eq!(notifications.load(Ordering::Relaxed), 1);
    kernel.vblank(&mut mem).unwrap();
    let frames = out.frames.lock().unwrap();
    assert_eq!(frames.stamp.vblanks, 1);
    assert_eq!(frames.stamp.updates, [1, 0]);
    assert_eq!(notifications.load(Ordering::Relaxed), 2);
}

#[test]
fn dispatch_publishes_only_enabled_successful_requests_once() {
    use crate::probe::{self, Modules, Outcome};
    use usum_guest::{Code, Cpu, Exit, Fault};
    fn request(cpu: &mut Cpu, _: &mut Memory) -> Result<Exit, Fault> {
        assert_eq!(crate::native_timing::request_frame_presentation(), cpu.r[4] != 0);
        assert_eq!(crate::native_timing::request_frame_presentation(), cpu.r[4] != 0);
        if cpu.r[4] == 2 {
            return Err(Fault::NoTranslation { address: 0x2008, thumb: false });
        }
        cpu.r[15] = 0x2004;
        Ok(Exit::Jump)
    }
    for mode in 0..3 {
        let (mut kernel, mut mem) = fixture();
        kernel.native_timing = mode != 0;
        submit(&mut mem, 0, 0, 0x3000_0000);
        let mut cpu = Cpu::default();
        cpu.r[15] = 0x2000;
        cpu.r[4] = mode;
        let entries: [(u32, Code); 1] = [(0x2000, request)];
        let (outcome, _) = probe::run_with(
            &mut cpu,
            &mut mem,
            &mut kernel,
            &entries,
            &Modules { functions: &[], entries: &[], translated: &[] },
            2,
            &mut |_, _| true,
        );
        assert!(matches!(outcome, Outcome::Fault(Fault::NoTranslation { .. })));
        assert_eq!(kernel.presented.len(), usize::from(mode == 1));
        assert_eq!(mem.read_u8(BASE + 0x201).unwrap(), if mode == 1 { 0x80 } else { 0x81 });
        assert_eq!(kernel.vblanks, 0);
        assert!(!crate::native_timing::request_frame_presentation());
    }
}
