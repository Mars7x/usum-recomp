//! Commands that only hand the system a setting: the reply is the result
//! alone and the system keeps the value. Each entry is one command 3dbrew
//! names, added when the game first sends it. Whether the setting changes
//! anything a game can observe later is decided per entry when it does.
//!
//! An entry here is a claim that replying success and recording the value
//! is faithful enough for now; it is listed (not matched by pattern) so
//! the claim is made once per command, on purpose.

/// (service, request header, 3dbrew name).
pub const RESULT_ONLY: &[(&str, u32, &str)] = &[
    // GSP ReleaseRight: this single-application runtime has no competing GPU
    // owner to wake; record the release alongside AcquireRight's flags.
    ("gsp::Gpu", 0x0017_0000, "ReleaseRight"),
    // Sleep acknowledgements: the virtual shell remains open/awake. Record
    // the application's reply; there is no host sleep transition to perform.
    ("APT:A", 0x003E_0080, "ReplySleepQuery"),
    ("APT:U", 0x003E_0080, "ReplySleepQuery"),
    ("APT:S", 0x003E_0080, "ReplySleepQuery"),
    ("APT:A", 0x003F_0040, "ReplySleepNotificationComplete"),
    ("APT:U", 0x003F_0040, "ReplySleepNotificationComplete"),
    ("APT:S", 0x003F_0040, "ReplySleepNotificationComplete"),
    // 3dbrew "NS and APT Services", available since 7.0.0-13
    ("APT:A", 0x0055_0040, "SetScreenCapturePostPermission"),
    ("APT:U", 0x0055_0040, "SetScreenCapturePostPermission"),
    // 3dbrew "APT:Enable": request AppletAttr, response the result only.
    // What NS does on Enable (notifications sent?) is not documented.
    ("APT:A", 0x0003_0040, "Enable"),
    ("APT:U", 0x0003_0040, "Enable"),
    // 3dbrew "APT:NotifyToWait": "a stub in the NS module", returns 0
    ("APT:A", 0x0043_0040, "NotifyToWait"),
    ("APT:U", 0x0043_0040, "NotifyToWait"),
    // 3dbrew "Services API": subscribe to a notification ID; delivered
    // through the EnableNotification semaphore (none are sent yet)
    ("srv:", 0x0009_0040, "Subscribe"),
    // 3dbrew "NDM Services": which background daemons may run
    ("ndm:u", 0x0014_0040, "OverrideDefaultDaemons"),
    ("ndm:u", 0x0006_0040, "SuspendDaemons"),
    ("ndm:u", 0x0007_0040, "ResumeDaemons"),
    // 3dbrew "NWM Services": vendor data for probe responses; nothing
    // ever probes this console
    ("nwm::UDS", 0x0021_0080, "SetProbeResponseParam"),
    // 3dbrew "DSP Services": which DSP semaphore bits raise the event
    ("dsp::DSP", 0x0017_0040, "SetSemaphoreMask"),
    // raises a semaphore bit towards the DSP; no DSP runs to see it
    ("dsp::DSP", 0x0007_0040, "SetSemaphore"),
    // 3dbrew "BOSS Services"; nothing is registered to unregister
    ("boss:U", 0x0003_0000, "UnregisterStorage"),
    // 3dbrew "GSP Services": blank the LCDs (1) or not (0)
    ("gsp::Gpu", 0x000B_0040, "SetLcdForceBlack"),
    // 3dbrew "GSP Services": the AXI bus QoS mode; no bus contention is
    // modelled, so it changes nothing here
    ("gsp::Gpu", 0x0010_0040, "SetAxiConfigQoSMode"),
    // 3dbrew "Filesystem services": the session's I/O priority
    ("fs:USER", 0x0862_0040, "SetPriority"),
];

/// The same, for commands whose only translate parameter is the
/// process-ID descriptor (0x20) the kernel fills in.
pub const RESULT_ONLY_WITH_PROCESS_ID: &[(&str, u32, &str)] = &[
    // AC SetClientVersion: SDK version followed by the process-ID
    // descriptor; the reply does not establish a network connection.
    ("ac:u", 0x0040_0042, "SetClientVersion"),
    // 3dbrew "Friend Services"
    ("frd:u", 0x0032_0042, "SetClientSdkVersion"),
    ("frd:a", 0x0032_0042, "SetClientSdkVersion"),
    // 3dbrew "BOSS Services": a u64 program ID (0: the caller), then the
    // process ID
    ("boss:U", 0x0001_0082, "InitializeSession"),
    // 3dbrew "NDM Services": which exclusive state (2: local
    // communications) the caller enters; no daemons run to be held off
    ("ndm:u", 0x0001_0042, "EnterExclusiveState"),
    // LeaveExclusiveState is the matching teardown (libctru ndm.c): no
    // normal words, only the process-ID descriptor. No network daemons
    // run in this offline runtime, so there is no daemon to resume.
    ("ndm:u", 0x0002_0002, "LeaveExclusiveState"),
    // 3dbrew "SSL Services": SSLC:Initialize; no connection is ever made
    // (the console is offline), so nothing to set up
    ("ssl:C", 0x0001_0002, "Initialize"),
];

/// The 3dbrew name if `service` command `words` (header first) is a
/// listed setting.
pub fn lookup(service: &str, words: &[u32]) -> Option<&'static str> {
    let header = words[0];
    let find = |table: &[(&str, u32, &'static str)]| table.iter().find(|(s, h, _)| *s == service && *h == header).map(|e| e.2);
    if let Some(name) = find(RESULT_ONLY) {
        return Some(name);
    }
    let normal = ((header >> 6) & 0x3F) as usize;
    find(RESULT_ONLY_WITH_PROCESS_ID).filter(|_| words.get(normal + 1) == Some(&super::fs::PROCESS_ID))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn entries_have_no_translate_words_and_are_unique() {
        for (i, (service, header, _)) in RESULT_ONLY.iter().enumerate() {
            assert_eq!(header & 0x3F, 0, "{service} {header:#x} carries handles or buffers");
            assert!(!RESULT_ONLY[..i].iter().any(|(s, h, _)| s == service && h == header), "{service} {header:#x} twice");
        }
        for (service, header, _) in RESULT_ONLY_WITH_PROCESS_ID {
            assert_eq!(header & 0x3F, 2, "{service} {header:#x}: a process ID is two translate words");
        }
    }
}
