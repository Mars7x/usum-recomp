use super::*;

fn command(mem: &mut Memory, cpu: &Cpu, words: &[u32]) {
    for (i, word) in words.iter().enumerate() {
        mem.write_u32(cpu.tls + services::COMMAND_BUFFER + 4 * i as u32, *word).unwrap();
    }
}

#[test]
fn offline_disconnect_registration_validates_handles_without_signaling() {
    let mut kernel = Kernel { next_handle: 0x8000, ..Kernel::default() };
    let mut cpu = Cpu { tls: 0x1000, ..Cpu::default() };
    let mut mem = Memory::new();
    mem.map(cpu.tls, vec![0; 0x1000], Perm::RW).unwrap();
    let event = kernel.open(Object::Event { signaled: false, reset: 0 });
    let mutex = kernel.open(Object::Mutex { owner: None, count: 0 });
    for (handle, result) in [(event, 0), (event, 0), (mutex, RESULT_INVALID_HANDLE), (0xFFFF, RESULT_INVALID_HANDLE)] {
        command(&mut mem, &cpu, &[0x0030_0004, fs::PROCESS_ID, 1, srv::COPY_HANDLE, handle]);
        assert!(matches!(kernel.request(&mut cpu, &mut mem, "ac:u".into()), Outcome::Continue));
        assert_eq!(mem.read_u32(cpu.tls + services::COMMAND_BUFFER + 4).unwrap(), result);
        assert!(matches!(kernel.handles[&event], Object::Event { signaled: false, .. }));
        assert_eq!(kernel.settings[&("ac:u".into(), 0x0030_0004)], vec![fs::PROCESS_ID, 1, srv::COPY_HANDLE, event]);
    }
}

#[test]
fn self_ncch_archive_reads_dump_bytes_and_closes_independently_of_files() {
    let source = std::env::temp_dir().join(format!("usum-self-ncch-{}", std::process::id()));
    std::fs::write(&source, (0u8..64).collect::<Vec<_>>()).unwrap();
    let image = fs::Image::open(&source, usum_import::romfs::Image { offset: 8, size: 16 }).unwrap();
    std::fs::remove_file(source).unwrap();
    let mut kernel = Kernel { romfs: Some(image), next_archive: u64::from(u32::MAX), next_handle: 0x8000, ..Kernel::default() };
    let mut cpu = Cpu { tls: 0x1000, ..Cpu::default() };
    let mut mem = Memory::new();
    mem.map(cpu.tls, vec![0; 0x1000], Perm::RW).unwrap();
    let buffer = cpu.tls + services::COMMAND_BUFFER;
    // PATH_EMPTY's dummy byte is ignored, as in the recorded SDK request.
    mem.write_u8(0x1500, 0xCC).unwrap();
    command(&mut mem, &cpu, &[0x080C_00C2, fs::ARCHIVE_SELF_NCCH, fs::PATH_EMPTY, 1, (1 << 14) | 2, 0x1500]);
    assert!(matches!(kernel.request(&mut cpu, &mut mem, "fs:USER".into()), Outcome::Continue));
    assert_eq!(mem.read_u32(buffer + 4).unwrap(), 0);
    let (lo, hi) = (mem.read_u32(buffer + 8).unwrap(), mem.read_u32(buffer + 12).unwrap());
    assert_eq!((lo, hi), (0, 1), "archive handles retain their high word");
    assert!(kernel.archives.is_empty(), "RomFS never maps into the save directory");
    mem.write_range(0x1500, &[0; 12]).unwrap();
    let open = [0x0802_01C2, 0, lo, hi, fs::PATH_BINARY, 12, fs::OPEN_READ, 0, (12 << 14) | 2, 0x1500];
    for flags in [fs::OPEN_CREATE, fs::OPEN_READ | 2] {
        let mut invalid = open;
        invalid[6] = flags;
        command(&mut mem, &cpu, &invalid);
        assert!(matches!(kernel.request(&mut cpu, &mut mem, "fs:USER".into()), Outcome::Unimplemented(_)));
        assert!(kernel.handles.is_empty());
    }
    command(&mut mem, &cpu, &open);
    assert!(matches!(kernel.request(&mut cpu, &mut mem, "fs:USER".into()), Outcome::Continue));
    let file = mem.read_u32(buffer + 12).unwrap();
    assert_eq!(kernel.handles[&file], Object::RomFsFile { start: 0, size: 16 });
    command(&mut mem, &cpu, &[0x080E_0080, lo, hi]);
    assert!(matches!(kernel.request(&mut cpu, &mut mem, "fs:USER".into()), Outcome::Continue));
    assert!(kernel.self_ncch_archives.is_empty());
    command(&mut mem, &cpu, &[0x0802_00C2, 2, 0, 4, (4 << 4) | 0xC, 0x1600]);
    cpu.r[0] = file;
    assert!(matches!(kernel.svc(&mut cpu, &mut mem, 0x32), Outcome::Continue));
    assert_eq!(mem.read_range(0x1600, 4).unwrap(), vec![10, 11, 12, 13]);
    assert_eq!(kernel.romfs_read, 4);
    command(&mut mem, &cpu, &open);
    assert!(
        matches!(kernel.request(&mut cpu, &mut mem, "fs:USER".into()), Outcome::Unimplemented(_)),
        "closed archive stays closed"
    );
}

#[test]
fn internet_status_remains_offline_after_client_version_registration() {
    let mut kernel = Kernel::default();
    let mut cpu = Cpu { tls: 0x1000, ..Cpu::default() };
    let mut mem = Memory::new();
    mem.map(cpu.tls, vec![0; 0x1000], Perm::RW).unwrap();
    let buffer = cpu.tls + services::COMMAND_BUFFER;
    for (request, reply) in [
        (vec![0x0040_0042, 0x0B04_01C8, services::fs::PROCESS_ID, 0], vec![services::header(0x40, 1, 0), 0]),
        (vec![0x000C_0000], vec![services::header(0xC, 2, 0), 0, 1]),
        (vec![0x000D_0000], vec![services::header(0xD, 2, 0), 0, 0]),
        (vec![0x000A_0000], vec![services::header(0xA, 2, 0), 0, 0]),
        (vec![0x000B_0000], vec![services::header(0xB, 2, 0), 0, 0]),
    ] {
        for (i, word) in request.iter().enumerate() {
            mem.write_u32(buffer + 4 * i as u32, *word).unwrap();
        }
        assert!(matches!(kernel.request(&mut cpu, &mut mem, "ac:u".into()), Outcome::Continue));
        for (i, word) in reply.iter().enumerate() {
            assert_eq!(mem.read_u32(buffer + 4 * i as u32).unwrap(), *word);
        }
        assert_eq!(cpu.r[0], RESULT_SUCCESS);
    }
}

#[test]
fn offline_network_config_respects_static_buffer_bounds_and_permissions() {
    let mut kernel = Kernel::default();
    let mut cpu = Cpu { tls: 0x1000, ..Cpu::default() };
    let mut mem = Memory::new();
    mem.map(cpu.tls, vec![0xA5; 0x1000], Perm::RW).unwrap();
    mem.map(0x4000, vec![0xA5; 0x200], Perm::R).unwrap();
    let buffer = cpu.tls + services::COMMAND_BUFFER;
    for (descriptor, address, succeeds) in [
        ((0x1FF << 14) | 2, 0x1800, false),
        ((0x200 << 14) | 0x402, 0x1800, false),
        ((0x200 << 14) | 2, 0x4000, false),
        ((0x200 << 14) | 2, 0x1800, true),
    ] {
        mem.write_u32(buffer, 0x0001_0000).unwrap();
        mem.write_u32(buffer + 0x100, descriptor).unwrap();
        mem.write_u32(buffer + 0x104, address).unwrap();
        let outcome = kernel.request(&mut cpu, &mut mem, "ac:u".into());
        assert_eq!(matches!(outcome, Outcome::Continue), succeeds);
        if succeeds {
            assert_eq!(mem.read_range(0x1800, 0x200).unwrap(), vec![0; 0x200]);
            assert_eq!(mem.read_u32(buffer).unwrap(), services::header(1, 1, 2));
            assert_eq!(mem.read_u32(buffer + 8).unwrap(), descriptor);
            assert_eq!(mem.read_u32(buffer + 12).unwrap(), address);
        } else {
            assert!(matches!(outcome, Outcome::Unimplemented(_)));
            assert_eq!(mem.read_range(0x1800, 0x200).unwrap(), vec![0xA5; 0x200]);
        }
        assert_eq!(mem.read_u8(0x17FF).unwrap(), 0xA5);
        assert_eq!(mem.read_u8(0x1A00).unwrap(), 0xA5);
        assert_eq!(mem.read_range(0x4000, 0x200).unwrap(), vec![0xA5; 0x200]);
    }
}

#[test]
fn self_ncch_menu_metadata_is_read_only_and_clipped_to_its_file() {
    let source = std::env::temp_dir().join(format!("usum-exefs-{}", std::process::id()));
    std::fs::write(&source, (0u8..64).collect::<Vec<_>>()).unwrap();
    let image = fs::Image::open(&source, usum_import::romfs::Image { offset: 16, size: 8 }).unwrap();
    std::fs::remove_file(source).unwrap();
    let mut kernel = Kernel { next_handle: 0x8000, ..Kernel::default() };
    kernel.exefs.insert("icon".into(), image);
    let mut cpu = Cpu { tls: 0x1000, ..Cpu::default() };
    let mut mem = Memory::new();
    mem.map(cpu.tls, vec![0; 0x1000], Perm::RW).unwrap();
    let buffer = cpu.tls + services::COMMAND_BUFFER;
    command(&mut mem, &cpu, &[0x080C_00C2, fs::ARCHIVE_SELF_NCCH, fs::PATH_EMPTY, 0, 2, 0x1500]);
    assert!(matches!(kernel.request(&mut cpu, &mut mem, "fs:USER".into()), Outcome::Continue));
    let archive = mem.read_u32(buffer + 8).unwrap();
    let open = [0x0802_01C2, 0, archive, 0, fs::PATH_BINARY, 12, fs::OPEN_READ, 0, (12 << 14) | 2, 0x1500];
    mem.write_range(0x1500, b"\x02\0\0\0icon\0\0\0\0").unwrap();
    command(&mut mem, &cpu, &open);
    assert!(matches!(kernel.request(&mut cpu, &mut mem, "fs:USER".into()), Outcome::Continue));
    let file = mem.read_u32(buffer + 12).unwrap();
    assert_eq!(kernel.handles[&file], Object::ExeFsFile { name: "icon".into(), size: 8 });
    mem.write_range(0x1600, &[0xCC; 8]).unwrap();
    command(&mut mem, &cpu, &[0x0802_00C2, 6, 0, 8, (8 << 4) | 0xC, 0x1600]);
    cpu.r[0] = file;
    assert!(matches!(kernel.svc(&mut cpu, &mut mem, 0x32), Outcome::Continue));
    assert_eq!(mem.read_range(0x1600, 8).unwrap(), vec![22, 23, 0xCC, 0xCC, 0xCC, 0xCC, 0xCC, 0xCC]);
    assert_eq!(mem.read_u32(buffer + 8).unwrap(), 2);
    mem.write_range(0x1500, b"\x02\0\0\0banner\0\0").unwrap();
    command(&mut mem, &cpu, &open);
    assert!(matches!(kernel.request(&mut cpu, &mut mem, "fs:USER".into()), Outcome::Continue));
    assert_eq!(mem.read_u32(buffer + 4).unwrap(), fs::RESULT_NOT_FOUND);
    for path in [b"\x01\0\0\0.code\0\0\0", b"\x02\0\0\0icon\0bad"] {
        mem.write_range(0x1500, path).unwrap();
        command(&mut mem, &cpu, &open);
        assert!(matches!(kernel.request(&mut cpu, &mut mem, "fs:USER".into()), Outcome::Unimplemented(_)));
    }
    mem.write_range(0x1500, b"\x02\0\0\0icon\0\0\0\0").unwrap();
    let mut write = open;
    write[6] = fs::OPEN_READ | 2;
    command(&mut mem, &cpu, &write);
    assert!(matches!(kernel.request(&mut cpu, &mut mem, "fs:USER".into()), Outcome::Unimplemented(_)));
}
