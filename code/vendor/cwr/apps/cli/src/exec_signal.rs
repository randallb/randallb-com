use std::{
    io,
    sync::atomic::{AtomicBool, Ordering},
};

static EXEC_SIGINT: AtomicBool = AtomicBool::new(false);

pub(super) struct ExecSigint {
    saved_action: libc::sigaction,
}

impl ExecSigint {
    pub(super) fn install() -> io::Result<Self> {
        EXEC_SIGINT.store(false, Ordering::SeqCst);
        // SAFETY: `libc::sigaction` is a C POD type for which an all-zero value is a valid
        // initialization before its mask and handler fields are populated.
        let mut action = unsafe { std::mem::zeroed::<libc::sigaction>() };
        action.sa_sigaction = interrupt_exec_delivery as *const () as usize;
        // SAFETY: `action.sa_mask` is live, writable, and correctly aligned for `sigemptyset`.
        if unsafe { libc::sigemptyset(&mut action.sa_mask) } != 0 {
            return Err(io::Error::last_os_error());
        }
        // SAFETY: an all-zero `sigaction` is valid storage for the previous action written by
        // `sigaction` before the value is read.
        let mut saved_action = unsafe { std::mem::zeroed::<libc::sigaction>() };
        // SAFETY: both action pointers are live for the call, and the installed handler has the
        // required C ABI and performs only an atomic store.
        if unsafe { libc::sigaction(libc::SIGINT, &action, &mut saved_action) } != 0 {
            return Err(io::Error::last_os_error());
        }
        Ok(Self { saved_action })
    }

    pub(super) fn take_interrupted(&self) -> bool {
        EXEC_SIGINT.swap(false, Ordering::SeqCst)
    }

    pub(super) fn finish(&mut self) -> io::Result<()> {
        // SAFETY: `saved_action` was initialized by a successful `sigaction` call and remains
        // live while it is restored; no previous-action output is requested.
        if unsafe { libc::sigaction(libc::SIGINT, &self.saved_action, std::ptr::null_mut()) } != 0 {
            return Err(io::Error::last_os_error());
        }
        Ok(())
    }
}

impl Drop for ExecSigint {
    fn drop(&mut self) {
        let _ = self.finish();
    }
}

extern "C" fn interrupt_exec_delivery(_: libc::c_int) {
    EXEC_SIGINT.store(true, Ordering::Relaxed);
}
