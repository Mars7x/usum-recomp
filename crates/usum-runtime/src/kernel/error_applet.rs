//! Error Display library-applet transport, restricted to applet 0x406.
//! IPC/handshake definitions: libctru apt.h/apt.c and 3dbrew APT services.
use super::*;

pub(super) const ERROR_APPLET: u32 = 0x406;

#[derive(Debug, Default)]
pub(super) struct ErrorApplet {
    pub parameter: Option<Vec<u8>>,
    completed: bool,
}

impl Kernel {
    /// A new button press dismisses the host error dialog. The guest continues
    /// through its normal APT resume path; neither clock nor VBlank is changed.
    pub(super) fn error_applet_input(&mut self, pressed: u32) -> Result<(), String> {
        if pressed & 3 == 0 {
            return Ok(());
        }
        let Some(applet) = &mut self.error_applet else { return Ok(()) };
        let Some(data) = applet.parameter.as_ref() else { return Ok(()) };
        let Some(apt) = &mut self.apt else { return Err("Error Display lost APT session".into()) };
        if apt.parameter.is_some() {
            return Err("Error Display resume parameter already pending".into());
        }
        let mut data = data.clone();
        let kind = u32::from_le_bytes(data[..4].try_into().expect("validated")) & 0xff;
        // This SDK's 0xF80-byte layout initializes and reads the return value
        // at 0xF70. Acknowledging a network error succeeds; an unavailable EULA
        // returns NOT_SUPPORTED and never changes the agreed version.
        data[0xf70..0xf74].copy_from_slice(&(if kind == 0 { 1u32 } else { 2u32 }).to_le_bytes());
        apt.parameter = Some(apt::Parameter { sender: ERROR_APPLET, command: 10, data, handle: 0 });
        if let Some(Object::Event { signaled, .. }) = self.handles.get_mut(&apt.resume) {
            *signaled = true;
        }
        applet.parameter = None;
        applet.completed = true;
        if let Some(frames) = &self.frame_out {
            *frames.dialog.lock().map_err(|_| "Error Display dialog lock poisoned")? = None;
        }
        Ok(())
    }

    pub(super) fn error_applet_request(
        &mut self,
        mem: &mut Memory,
        message: &Message,
    ) -> Result<(Option<Vec<u32>>, bool), String> {
        let w = &message.words;
        let command = (w[0] >> 16) as u16;
        match w[0] {
            0x0018_0040 if w[1] == ERROR_APPLET && self.apt.is_some() => {
                if self.error_applet.as_ref().is_some_and(|a| !a.completed) {
                    return Err("Error Display applet prepared twice".into());
                }
                self.error_applet = Some(ErrorApplet::default());
                Ok((Some(services::result_reply(command)), false))
            }
            0x0009_0040 if w[1] == ERROR_APPLET => Ok((
                Some(vec![
                    services::header(command, 2, 0),
                    0,
                    u32::from(self.error_applet.as_ref().is_some_and(|a| !a.completed)),
                ]),
                false,
            )),
            0x0013_0000 if self.error_applet.is_some() => Ok((Some(vec![services::header(command, 2, 0), 0, 5]), false)),
            0x000C_0104
                if w[2] == ERROR_APPLET
                    && w[3] == 2
                    && w[4] == 0x20
                    && w[5] == srv::COPY_HANDLE
                    && w[6] == 0
                    && w[7] == (0x20 << 14) | 2
                    && self.error_applet.is_some() =>
            {
                let Some(a) = &self.apt else {
                    return Err("Error Display capture without APT".into());
                };
                if a.app_id != w[1] || a.parameter.is_some() {
                    return Err("Error Display capture sender or pending parameter".into());
                }
                let size = mem.read_u32(w[8]).map_err(|e| format!("Error Display capture layout: {e}"))?;
                if size == 0 || size > 0x400000 {
                    return Err(format!("Error Display capture size {size:#x}"));
                }
                mem.read_range(w[8], 0x20).map_err(|e| format!("Error Display capture layout: {e}"))?;
                let size = (size + 0xFFF) & !0xFFF;
                let handle = self.open(Object::SharedMemory { size, mapped: None });
                let a = self.apt.as_mut().expect("checked");
                a.parameter = Some(apt::Parameter { sender: ERROR_APPLET, command: 3, data: Vec::new(), handle });
                if let Some(Object::Event { signaled, .. }) = self.handles.get_mut(&a.resume) {
                    *signaled = true;
                }
                Ok((Some(services::result_reply(command)), true))
            }
            0x0040_0042 if w[1] == 0x20 && w[2] == (0x20 << 14) | 2 && self.error_applet.is_some() => {
                let data = mem.read_range(w[3], 0x20).map_err(|e| format!("Error Display capture info: {e}"))?;
                self.settings.insert(
                    ("APT:capture".into(), w[0]),
                    data.as_chunks::<4>().0.iter().map(|x| u32::from_le_bytes(*x)).collect(),
                );
                Ok((Some(services::result_reply(command)), false))
            }
            0x001E_0084
                if w[1] == ERROR_APPLET
                    && self.error_applet.is_some()
                    && w[2] == 0xf80
                    && w[3] == srv::COPY_HANDLE
                    && w[4] == 0
                    && w[5] == (w[2] << 14) | 2 =>
            {
                let data = mem.read_range(w[6], w[2]).map_err(|e| format!("Error Display parameters: {e}"))?;
                let kind = u32::from_le_bytes(data[..4].try_into().expect("size checked"));
                let text = match kind {
                    0 | 0x100 => {
                        let code = u32::from_le_bytes(data[4..8].try_into().expect("size checked"));
                        format!("NETWORK ERROR\nERROR CODE {code}\nONLINE SERVICES UNAVAILABLE\nPRESS A OR B TO RETURN")
                    }
                    2 | 0x102 => "AGREEMENT UNAVAILABLE\nONLINE SERVICES UNAVAILABLE\nPRESS A OR B TO RETURN".into(),
                    _ => return Err(format!("Error Display unsupported type {kind:#x}")),
                };
                if self.error_applet.as_ref().is_some_and(|a| a.completed || a.parameter.is_some()) {
                    return Err("Error Display start without fresh preparation".into());
                }
                eprintln!("Error Display: {}", text.replace('\n', "; "));
                self.error_applet.as_mut().expect("checked").parameter = Some(data);
                if let Some(frames) = &self.frame_out {
                    *frames.dialog.lock().map_err(|_| "Error Display dialog lock poisoned")? = Some(text);
                }
                Ok((Some(services::result_reply(command)), false))
            }
            _ => Ok((None, false)),
        }
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

    fn setup() -> (Kernel, Memory) {
        let mut k = Kernel { next_handle: 0x8000, virtual_clock: Some(123_000_000), vblanks: 7, ..Kernel::default() };
        let resume = k.open(Object::Event { signaled: false, reset: 0 });
        k.apt = Some(Apt { app_id: 0x300, attributes: 0, notification: 0, resume, parameter: None });
        let mut mem = Memory::new();
        mem.map(0x1000, vec![0; 0x4000], Perm::RW).unwrap();
        (k, mem)
    }

    #[test]
    fn acknowledgment_resumes_once_and_preserves_timing_and_parameters() {
        for kind in [0u32, 0x100, 2, 0x102] {
            let (mut k, mut mem) = setup();
            let prepare = message(&[0x0018_0040, ERROR_APPLET]);
            k.error_applet_request(&mut mem, &prepare).unwrap();
            let mut data = vec![0xa5; 0xf80];
            data[..4].copy_from_slice(&kind.to_le_bytes());
            data[4..8].copy_from_slice(&12345u32.to_le_bytes());
            mem.write_range(0x3000, &data).unwrap();
            let start = message(&[0x001E_0084, ERROR_APPLET, 0xf80, srv::COPY_HANDLE, 0, (0xf80 << 14) | 2, 0x3000]);
            k.error_applet_request(&mut mem, &start).unwrap();
            k.error_applet_input(0).unwrap();
            assert!(k.apt.as_ref().unwrap().parameter.is_none());
            assert!(k.error_applet_request(&mut mem, &prepare).is_err());
            k.error_applet_input(2).unwrap();
            let apt = k.apt.as_ref().unwrap();
            let returned = apt.parameter.as_ref().unwrap();
            assert_eq!((returned.sender, returned.command, returned.handle), (ERROR_APPLET, 10, 0));
            data[0xf70..0xf74].copy_from_slice(&(if kind & 0xff == 0 { 1u32 } else { 2u32 }).to_le_bytes());
            assert_eq!(returned.data, data);
            assert!(matches!(k.object(apt.resume), Some(Object::Event { signaled: true, .. })));
            assert_eq!((k.virtual_clock, k.vblanks), (Some(123_000_000), 7));
            k.error_applet_input(1).unwrap();
            k.apt.as_mut().unwrap().parameter = None;
            k.error_applet_request(&mut mem, &prepare).unwrap();
        }
    }

    #[test]
    fn invalid_parameters_do_not_start_a_dialog() {
        let (mut k, mut mem) = setup();
        k.error_applet_request(&mut mem, &message(&[0x0018_0040, ERROR_APPLET])).unwrap();
        let mut start = message(&[0x001E_0084, ERROR_APPLET, 0xf80, srv::COPY_HANDLE, 0, (0xf80 << 14) | 2, 0x3000]);
        mem.write_u32(0x3000, 0xffff).unwrap();
        assert!(k.error_applet_request(&mut mem, &start).is_err());
        start.words[6] = 0x4800;
        assert!(k.error_applet_request(&mut mem, &start).is_err());
        assert!(k.error_applet.as_ref().unwrap().parameter.is_none());
        assert!(k.apt.as_ref().unwrap().parameter.is_none());
    }

    #[test]
    fn closing_glanced_capture_handle_does_not_destroy_the_pending_parameter() {
        let (mut k, mut mem) = setup();
        k.error_applet_request(&mut mem, &message(&[0x0018_0040, ERROR_APPLET])).unwrap();
        mem.write_u32(0x3000, 0x1234).unwrap();
        let capture = message(&[0x000C_0104, 0x300, ERROR_APPLET, 2, 0x20, srv::COPY_HANDLE, 0, (0x20 << 14) | 2, 0x3000]);
        k.error_applet_request(&mut mem, &capture).unwrap();
        let queued = k.apt.as_ref().unwrap().parameter.as_ref().unwrap().handle;
        let mut cpu = Cpu { tls: 0x1000, ..Cpu::default() };
        let ipc = cpu.tls + services::COMMAND_BUFFER;
        let mut delivered = Vec::new();
        for command in [0x000E_0080, 0x000E_0080, 0x000D_0080] {
            Message::reply(&mut mem, ipc, &[command, 0x300, 0]).unwrap();
            assert!(matches!(k.request(&mut cpu, &mut mem, "APT:A".into()), Outcome::Continue | Outcome::Switch));
            let handle = mem.read_u32(ipc + 6 * 4).unwrap();
            assert_ne!(handle, queued);
            assert!(!delivered.contains(&handle));
            assert!(matches!(k.object(handle), Some(Object::SharedMemory { size: 0x2000, mapped: None })));
            delivered.push(handle);
            cpu.r[0] = handle;
            assert!(matches!(k.svc(&mut cpu, &mut mem, 0x23), Outcome::Continue));
        }
        assert!(k.apt.as_ref().unwrap().parameter.is_none());
        assert!(k.object(queued).is_none());
    }
}
