//! Offline IRUSER shared-memory lifecycle. No peer or packet is fabricated.
//! Protocol/layout: 3dbrew IRUSER:InitializeIrnopShared and IRUSER Shared Memory.
use super::*;

#[derive(Debug)]
pub(super) struct Infrared {
    address: u32,
    receive_size: u32,
    trying: bool,
    events: [Option<u32>; 3],
}

impl Kernel {
    pub(super) fn infrared_request(&mut self, mem: &mut Memory, message: &Message) -> Result<(Option<Vec<u32>>, bool), String> {
        let w = &message.words;
        let command = (w[0] >> 16) as u16;
        let mut wake = false;
        if w[0] == 0x0018_0182 && w[7] == srv::COPY_HANDLE {
            if self.infrared.is_some() {
                return Err("IRUSER initialized twice".into());
            }
            let Some(&Object::SharedMemory { size, mapped: Some(address) }) = self.handles.get(&w[8]) else {
                return Ok((Some(srv::error_reply(command, RESULT_INVALID_HANDLE)), false));
            };
            let total = u64::from(w[2]) + u64::from(w[4]) + 0x30;
            if total > u64::from(w[1])
                || w[1] > size
                || u64::from(w[3]) * 8 > u64::from(w[2])
                || u64::from(w[5]) * 8 > u64::from(w[4])
                || address.checked_add(w[1]).is_none()
            {
                return Err("IRUSER shared-memory layout exceeds its allocation".into());
            }
            // Empty packet rings, disconnected, no errors. The documented
            // initialization marker is one; no connection markers are set.
            let mut bytes = vec![0; total as usize];
            bytes[0xE] = 1;
            mem.write_range(address, &bytes).map_err(|e| format!("IRUSER initialization: {e}"))?;
            self.infrared = Some(Infrared { address, receive_size: w[2], trying: false, events: [None; 3] });
            return Ok((Some(services::result_reply(command)), false));
        }
        let Some(ir) = self.infrared.as_mut() else {
            return Ok((None, false));
        };
        let reply = match w[0] {
            0x0002_0000 => {
                mem.write_range(ir.address, &[0; 16]).map_err(|e| format!("IRUSER finalize: {e}"))?;
                self.infrared = None;
                Some(services::result_reply(command))
            }
            0x0003_0000 | 0x0004_0000 => {
                let at = ir.address + if command == 3 { 0x10 } else { 0x20 + ir.receive_size };
                mem.write_range(at, &[0; 16]).map_err(|e| format!("IRUSER clear ring: {e}"))?;
                Some(services::result_reply(command))
            }
            // RequireConnection / AutoConnection / AnyConnection initiate a
            // search. There are no peers in this runtime; cancellation remains
            // explicit. The observed SDK AutoConnection wrapper reads only Result.
            0x0006_0040 | 0x0007_02C0 | 0x0008_0000 => {
                mem.write_u8(ir.address + 9, 1).map_err(|e| format!("IRUSER search: {e}"))?;
                ir.trying = true;
                Some(services::result_reply(command))
            }
            0x0009_0000 => {
                mem.write_u8(ir.address + 9, 0).map_err(|e| format!("IRUSER disconnect: {e}"))?;
                if ir.trying {
                    if let Some(Object::Event { signaled, .. }) = ir.events[2].and_then(|h| self.handles.get_mut(&h)) {
                        *signaled = true;
                        wake = true;
                    }
                }
                ir.trying = false;
                Some(services::result_reply(command))
            }
            0x000A_0000 | 0x000B_0000 | 0x000C_0000 => {
                let index = usize::from(command - 0xA);
                let handle = ir.events[index].filter(|h| self.handles.contains_key(h));
                let handle = handle.unwrap_or_else(|| self.open(Object::Event { signaled: false, reset: 1 }));
                self.infrared.as_mut().expect("initialized").events[index] = Some(handle);
                Some(vec![services::header(command, 1, 2), 0, srv::COPY_HANDLE, handle])
            }
            0x0011_0000 | 0x0012_0000 | 0x0013_0000 | 0x0017_0000 => Some(vec![services::header(command, 2, 0), 0, 0]),
            0x0014_0000 => Some(vec![services::header(command, 2, 0), 0, u32::from(ir.trying)]),
            0x001A_0040 => {
                mem.write_u8(ir.address + 0xB, w[1] as u8).map_err(|e| format!("IRUSER machine ID: {e}"))?;
                Some(services::result_reply(command))
            }
            _ => None,
        };
        Ok((reply, wake))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn message(words: &[u32]) -> Message {
        let mut m = Message { words: [0; services::COMMAND_WORDS], statics: [0; services::STATIC_WORDS] };
        m.words[..words.len()].copy_from_slice(words);
        m
    }

    #[test]
    fn offline_search_can_be_cancelled_without_fabricating_a_peer() {
        let mut kernel = Kernel { next_handle: 0x8000, ..Kernel::default() };
        let mut mem = Memory::new();
        mem.map(0x1000, vec![0xCC; 0x1000], Perm::RW).unwrap();
        let shared = kernel.open(Object::SharedMemory { size: 0x1000, mapped: Some(0x1000) });
        let init = message(&[0x0018_0182, 0x1000, 0x25A, 2, 0x25A, 2, 3, srv::COPY_HANDLE, shared]);
        assert_eq!(kernel.infrared_request(&mut mem, &init).unwrap(), (Some(services::result_reply(0x18)), false));
        let total = 0x25A * 2 + 0x30;
        assert_eq!(mem.read_u8(0x100E).unwrap(), 1);
        assert_eq!(mem.read_range(0x1000 + total, 0x1000 - total).unwrap(), vec![0xCC; (0x1000 - total) as usize]);
        assert!(kernel.infrared_request(&mut mem, &init).is_err());
        let (Some(reply), false) = kernel.infrared_request(&mut mem, &message(&[0x000C_0000])).unwrap() else {
            panic!("event reply")
        };
        let event = reply[3];
        kernel.infrared_request(&mut mem, &message(&[0x001A_0040, 3])).unwrap();
        kernel.infrared_request(&mut mem, &message(&[0x0006_0040, 3])).unwrap();
        assert_eq!(mem.read_range(0x1008, 4).unwrap(), [0, 1, 0, 3]);
        assert_eq!(
            kernel.infrared_request(&mut mem, &message(&[0x0013_0000])).unwrap().0,
            Some(vec![services::header(0x13, 2, 0), 0, 0])
        );
        assert!(matches!(kernel.handles[&event], Object::Event { signaled: false, .. }));
        assert_eq!(kernel.infrared_request(&mut mem, &message(&[0x0009_0000])).unwrap(), (Some(services::result_reply(9)), true));
        assert_eq!(mem.read_u8(0x1009).unwrap(), 0);
        assert!(matches!(kernel.handles[&event], Object::Event { signaled: true, .. }));
        kernel.infrared_request(&mut mem, &message(&[0x0002_0000])).unwrap();
        assert!(kernel.infrared.is_none());
        assert_eq!(mem.read_range(0x1000, 16).unwrap(), [0; 16]);
        assert!(kernel.handles.contains_key(&event), "client closes its own event handle");
        assert!(kernel.infrared_request(&mut mem, &init).is_ok());
        assert_eq!(kernel.vblanks, 0);
    }

    #[test]
    fn initialization_rejects_invalid_layout_and_handles_before_writing() {
        let mut kernel = Kernel { next_handle: 0x8000, ..Kernel::default() };
        let mut mem = Memory::new();
        mem.map(0x1000, vec![0xCC; 0x1000], Perm::RW).unwrap();
        let shared = kernel.open(Object::SharedMemory { size: 0x1000, mapped: Some(0x1000) });
        for (size, receive, packets) in [(0x2000, 0x25A, 2), (0x1000, u32::MAX, 2), (0x1000, 0x25A, u32::MAX)] {
            let init = message(&[0x0018_0182, size, receive, packets, 0x25A, 2, 3, srv::COPY_HANDLE, shared]);
            assert!(kernel.infrared_request(&mut mem, &init).is_err());
            assert!(kernel.infrared.is_none());
            assert_eq!(mem.read_range(0x1000, 0x1000).unwrap(), vec![0xCC; 0x1000]);
        }
        let init = message(&[0x0018_0182, 0x1000, 0x25A, 2, 0x25A, 2, 3, srv::COPY_HANDLE, 0xBAD]);
        assert_eq!(kernel.infrared_request(&mut mem, &init).unwrap().0, Some(srv::error_reply(0x18, RESULT_INVALID_HANDLE)));
    }
}
