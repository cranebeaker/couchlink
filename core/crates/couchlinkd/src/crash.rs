//! Traces of a daemon that ends without a word: a panic goes into couchlinkd.log with its backtrace, and on Linux a
//! fatal signal (a crash in a library, or something that kills the daemon) writes which signal and which process sent
//! it there too, and the daemon's own stderr (what SDL and the libraries say) goes to `logs/couchlinkd-stderr.log`.
//! SIGHUP is logged and survived: a background service has no terminal to lose.

/// Install the panic hook and, on Linux, the signal handlers and the stderr file. Call once, early.
pub fn install() {
    let previous = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        let thread = std::thread::current();
        let name = thread.name().unwrap_or("unnamed");
        let backtrace = std::backtrace::Backtrace::force_capture();
        crate::logfile::write("error", &format!("couchlinkd panicked on thread {name}: {info}\n{backtrace}"));
        previous(info);
    }));
    #[cfg(target_os = "linux")]
    unix::install();
}

#[cfg(target_os = "linux")]
mod unix {
    use std::os::unix::io::IntoRawFd;
    use std::sync::atomic::{AtomicI32, Ordering};

    /// couchlinkd.log, opened for the signal handler (which may only make plain system calls).
    static LOG_FD: AtomicI32 = AtomicI32::new(-1);

    const FATAL: [libc::c_int; 7] = [libc::SIGSEGV, libc::SIGBUS, libc::SIGILL, libc::SIGFPE, libc::SIGABRT, libc::SIGTERM, libc::SIGHUP];

    pub fn install() {
        let dir = crate::logfile::dir();
        let _ = std::fs::create_dir_all(&dir);
        if let Ok(f) = std::fs::OpenOptions::new().create(true).append(true).open(dir.join("couchlinkd.log")) {
            LOG_FD.store(f.into_raw_fd(), Ordering::Relaxed);
        }
        stderr_to_file(&dir);
        for sig in FATAL {
            // SAFETY: a zeroed sigaction with our handler; SA_RESETHAND puts the default back after one fatal signal,
            // so re-raising ends the process as it would have ended. SIGHUP keeps the handler.
            unsafe {
                let mut sa: libc::sigaction = std::mem::zeroed();
                sa.sa_sigaction = handler as usize;
                sa.sa_flags = libc::SA_SIGINFO | libc::SA_ONSTACK | if sig == libc::SIGHUP { libc::SA_RESTART } else { libc::SA_RESETHAND };
                libc::sigemptyset(&mut sa.sa_mask);
                libc::sigaction(sig, &sa, std::ptr::null_mut());
            }
        }
    }

    /// The daemon's stderr into `couchlinkd-stderr.log` (the previous run's kept as `.1`), unless it is a terminal.
    fn stderr_to_file(dir: &std::path::Path) {
        // SAFETY: isatty on a standard descriptor.
        if unsafe { libc::isatty(2) } == 1 {
            return;
        }
        let path = dir.join("couchlinkd-stderr.log");
        let _ = std::fs::rename(&path, dir.join("couchlinkd-stderr.1.log"));
        if let Ok(f) = std::fs::File::create(&path) {
            let fd = f.into_raw_fd();
            // SAFETY: both descriptors are valid; fd 2 now points at the file.
            unsafe {
                libc::dup2(fd, 2);
                libc::close(fd);
            }
        }
    }

    /// `n` in decimal into `buf`; returns how many bytes.
    fn digits(mut n: u64, buf: &mut [u8; 20]) -> usize {
        let mut tmp = [0u8; 20];
        let mut i = 0;
        loop {
            tmp[i] = b'0' + (n % 10) as u8;
            n /= 10;
            i += 1;
            if n == 0 {
                break;
            }
        }
        for j in 0..i {
            buf[j] = tmp[i - 1 - j];
        }
        i
    }

    /// The line the handler writes: `couchlinkd: signal <n> from pid <pid> (<name>)`.
    pub(super) fn line(sig: i32, pid: i32, name: &[u8], out: &mut [u8; 160]) -> usize {
        let mut len = 0;
        let mut put = |b: &[u8], out: &mut [u8; 160]| {
            let n = b.len().min(out.len() - len);
            out[len..len + n].copy_from_slice(&b[..n]);
            len += n;
        };
        let mut num = [0u8; 20];
        put(b"couchlinkd: signal ", out);
        let n = digits(sig as u64, &mut num);
        put(&num[..n], out);
        if sig == libc::SIGHUP {
            put(b" (hangup, ignored)", out);
        }
        put(b" from pid ", out);
        let n = digits(pid.max(0) as u64, &mut num);
        put(&num[..n], out);
        if !name.is_empty() {
            put(b" (", out);
            put(name, out);
            put(b")", out);
        }
        put(b"\n", out);
        len
    }

    extern "C" fn handler(sig: libc::c_int, info: *mut libc::siginfo_t, _: *mut libc::c_void) {
        // Only plain system calls in here: no allocation, no locks.
        // SAFETY: the kernel passes a valid siginfo for SA_SIGINFO handlers.
        let pid = if info.is_null() { 0 } else { unsafe { (*info).si_pid() } };
        let mut name = [0u8; 64];
        let mut name_len = 0;
        if pid > 0 {
            // /proc/<pid>/comm of the sender.
            let mut path = [0u8; 40];
            let mut len = 0;
            for b in b"/proc/" {
                path[len] = *b;
                len += 1;
            }
            let mut num = [0u8; 20];
            let n = digits(pid as u64, &mut num);
            path[len..len + n].copy_from_slice(&num[..n]);
            len += n;
            for b in b"/comm\0" {
                path[len] = *b;
                len += 1;
            }
            // SAFETY: a NUL-terminated path on the stack; the descriptor is closed right after.
            unsafe {
                let fd = libc::open(path.as_ptr() as *const libc::c_char, libc::O_RDONLY | libc::O_CLOEXEC);
                if fd >= 0 {
                    let r = libc::read(fd, name.as_mut_ptr() as *mut libc::c_void, name.len());
                    if r > 0 {
                        name_len = r as usize;
                        while name_len > 0 && (name[name_len - 1] == b'\n' || name[name_len - 1] == 0) {
                            name_len -= 1;
                        }
                    }
                    libc::close(fd);
                }
            }
        }
        let mut out = [0u8; 160];
        let n = line(sig, pid, &name[..name_len], &mut out);
        let fd = LOG_FD.load(Ordering::Relaxed);
        // SAFETY: writes from a stack buffer; raise ends the process with the default action restored.
        unsafe {
            if fd >= 0 {
                libc::write(fd, out.as_ptr() as *const libc::c_void, n);
            }
            libc::write(2, out.as_ptr() as *const libc::c_void, n);
            if sig != libc::SIGHUP {
                libc::raise(sig);
            }
        }
    }
}

#[cfg(all(test, target_os = "linux"))]
mod tests {
    #[test]
    fn the_signal_line_names_the_sender() {
        let mut out = [0u8; 160];
        let n = super::unix::line(libc::SIGTERM, 4242, b"steam", &mut out);
        assert_eq!(std::str::from_utf8(&out[..n]).unwrap(), format!("couchlinkd: signal {} from pid 4242 (steam)\n", libc::SIGTERM));
        let n = super::unix::line(libc::SIGHUP, 0, b"", &mut out);
        assert_eq!(std::str::from_utf8(&out[..n]).unwrap(), format!("couchlinkd: signal {} (hangup, ignored) from pid 0\n", libc::SIGHUP));
    }

    /// A child couchlinkd-like process (this test binary) with the handlers, sent SIGTERM by us, leaves the line.
    #[test]
    fn a_killed_daemon_says_who_killed_it() {
        if std::env::var("COUCHLINK_CRASH_CHILD").is_ok() {
            return;
        }
        let dir = std::env::temp_dir().join(format!("couchlink-crash-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let mut child = std::process::Command::new(std::env::current_exe().unwrap())
            .args(["--exact", "crash::tests::crash_child", "--ignored", "--nocapture", "-q"])
            .env("COUCHLINK_CRASH_CHILD", "1")
            .env("COUCHLINK_STATE_DIR", &dir)
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .spawn()
            .unwrap();
        let log = dir.join("logs").join("couchlinkd.log");
        let t = std::time::Instant::now();
        while !std::fs::read_to_string(&log).is_ok_and(|s| s.contains("ready")) {
            assert!(t.elapsed() < std::time::Duration::from_secs(20), "the child never got ready");
            std::thread::sleep(std::time::Duration::from_millis(50));
        }
        // SAFETY: a signal to our own child.
        unsafe { libc::kill(child.id() as i32, libc::SIGHUP) };
        std::thread::sleep(std::time::Duration::from_millis(300));
        assert!(matches!(child.try_wait(), Ok(None)), "a hangup is survived");
        unsafe { libc::kill(child.id() as i32, libc::SIGTERM) };
        let status = child.wait().unwrap();
        use std::os::unix::process::ExitStatusExt;
        assert_eq!(status.signal(), Some(libc::SIGTERM), "it still ends as SIGTERM ends it");
        let text = std::fs::read_to_string(&log).unwrap();
        let me = std::process::id();
        assert!(text.contains(&format!("signal {} (hangup, ignored) from pid {me}", libc::SIGHUP)), "{text}");
        assert!(text.contains(&format!("signal {} from pid {me}", libc::SIGTERM)), "{text}");
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// The child for the test above: installs the handlers and waits to be killed.
    #[test]
    #[ignore]
    fn crash_child() {
        if std::env::var("COUCHLINK_CRASH_CHILD").is_err() {
            return;
        }
        super::install();
        crate::logfile::write("info", "ready");
        std::thread::sleep(std::time::Duration::from_secs(30));
    }
}
