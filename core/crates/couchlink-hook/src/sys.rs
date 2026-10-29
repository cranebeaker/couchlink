//! Windows implementation: start a process, stop it at its entry point, load the hook DLL into it and talk to the
//! DLL over a per-process named pipe.
//!
//! Why the entry point: a process that was just created suspended has no kernel32 yet, so `LoadLibraryW` cannot be
//! run in it, and for a 32-bit target its address cannot even be found. So the main thread is let run until it
//! reaches the exe's entry point (a `jmp $` is written there for that moment). At that point every static import
//! is loaded and the game's own code has not run yet; the DLL is injected and the thread is left suspended until
//! `resume`.

use std::collections::BTreeMap;
use std::ffi::{OsStr, OsString, c_void};
use std::os::windows::ffi::{OsStrExt, OsStringExt};
use std::path::Path;
use std::ptr::{null, null_mut};
use std::sync::Mutex;
use std::time::{Duration, Instant};

use windows_sys::Wdk::System::Threading::NtQueryInformationProcess;
use windows_sys::Win32::Foundation::{
    CloseHandle, ERROR_PIPE_BUSY, ERROR_PIPE_CONNECTED, GENERIC_READ, GENERIC_WRITE, GetLastError, HANDLE,
    INVALID_HANDLE_VALUE, WAIT_OBJECT_0, WAIT_TIMEOUT,
};
use windows_sys::Win32::Storage::FileSystem::{
    CreateFileW, FILE_ATTRIBUTE_NORMAL, OPEN_EXISTING, PIPE_ACCESS_DUPLEX, ReadFile, WriteFile,
};
use windows_sys::Win32::System::Diagnostics::Debug::{
    CONTEXT, CONTEXT_CONTROL_AMD64, FlushInstructionCache, GetThreadContext, ReadProcessMemory, WOW64_CONTEXT,
    WOW64_CONTEXT_CONTROL, Wow64GetThreadContext, WriteProcessMemory,
};
use windows_sys::Win32::System::Diagnostics::ToolHelp::{
    CreateToolhelp32Snapshot, MODULEENTRY32W, Module32FirstW, Module32NextW, TH32CS_SNAPMODULE, TH32CS_SNAPMODULE32,
};
use windows_sys::Win32::System::Memory::{
    MEM_COMMIT, MEM_RELEASE, MEM_RESERVE, PAGE_EXECUTE_READWRITE, PAGE_READWRITE, VirtualAllocEx, VirtualFreeEx,
    VirtualProtectEx,
};
use windows_sys::Win32::System::Pipes::{
    ConnectNamedPipe, CreateNamedPipeW, PIPE_NOWAIT, PIPE_READMODE_BYTE, PIPE_TYPE_BYTE, PIPE_WAIT, PeekNamedPipe,
    SetNamedPipeHandleState, WaitNamedPipeW,
};
use windows_sys::Win32::System::Threading::{
    CREATE_SUSPENDED, CREATE_UNICODE_ENVIRONMENT, CreateProcessW, CreateRemoteThread, GetExitCodeProcess,
    GetExitCodeThread, IsWow64Process, PROCESS_INFORMATION, ResumeThread, STARTUPINFOW, SuspendThread, TerminateProcess,
    WaitForSingleObject,
};

use crate::config::{HookConfig, HookOptions, HookStatus, config_message};
use crate::dll::dll_path;
use crate::error::{HookError, Result};

const PIPE_REJECT_REMOTE_CLIENTS: u32 = 8;
const ENTRY_TIMEOUT: Duration = Duration::from_secs(20);
const CONNECT_TIMEOUT: Duration = Duration::from_secs(15);
const REPLY_TIMEOUT: Duration = Duration::from_secs(15);

/// Owned kernel handle.
struct Owned(HANDLE);

// SAFETY: kernel handles may be used and closed from any thread.
unsafe impl Send for Owned {}
unsafe impl Sync for Owned {}

impl Drop for Owned {
    fn drop(&mut self) {
        if !self.0.is_null() && self.0 != INVALID_HANDLE_VALUE {
            // SAFETY: the handle is owned by this value and closed once.
            unsafe { CloseHandle(self.0) };
        }
    }
}

/// A game process that was started suspended.
pub struct Launched {
    pid: u32,
    x86: bool,
    process: Owned,
    thread: Owned,
}

impl std::fmt::Debug for Launched {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Launched").field("pid", &self.pid).field("x86", &self.x86).finish()
    }
}

impl Launched {
    pub fn pid(&self) -> u32 {
        self.pid
    }

    /// True for a 32-bit target (it gets the x86 hook DLL).
    pub fn is_x86(&self) -> bool {
        self.x86
    }

    /// Exit code when the process has ended.
    pub fn exit_code(&self) -> Option<u32> {
        let mut code = 0u32;
        // SAFETY: the process handle is valid for the life of self.
        unsafe {
            if WaitForSingleObject(self.process.0, 0) == WAIT_OBJECT_0 && GetExitCodeProcess(self.process.0, &mut code) != 0 {
                return Some(code);
            }
        }
        None
    }

    pub fn is_running(&self) -> bool {
        self.exit_code().is_none()
    }

    /// Waits for the process to end; returns its exit code, or None on timeout.
    pub fn wait(&self, timeout: Duration) -> Option<u32> {
        let ms = timeout.as_millis().min(u32::MAX as u128 - 1) as u32;
        // SAFETY: valid handle.
        let r = unsafe { WaitForSingleObject(self.process.0, ms) };
        if r == WAIT_OBJECT_0 { self.exit_code() } else { None }
    }

    /// Kills the process (a no-op when it already ended).
    pub fn terminate(&self) {
        // SAFETY: valid handle.
        unsafe { TerminateProcess(self.process.0, 1) };
    }
}

/// Quotes arguments for a Windows command line (MSVCRT rules) and joins them with spaces.
pub fn join_args<S: AsRef<OsStr>>(args: &[S]) -> String {
    let mut out = String::new();
    for (i, a) in args.iter().enumerate() {
        if i > 0 {
            out.push(' ');
        }
        let a = a.as_ref().to_string_lossy();
        if !a.is_empty() && !a.contains([' ', '\t', '"']) {
            out.push_str(&a);
            continue;
        }
        out.push('"');
        let mut backslashes = 0;
        for c in a.chars() {
            match c {
                '\\' => backslashes += 1,
                '"' => {
                    out.push_str(&"\\".repeat(backslashes * 2 + 1));
                    out.push('"');
                    backslashes = 0;
                }
                _ => {
                    out.push_str(&"\\".repeat(backslashes));
                    backslashes = 0;
                    out.push(c);
                }
            }
        }
        out.push_str(&"\\".repeat(backslashes * 2));
        out.push('"');
    }
    out
}

fn wide(s: &OsStr) -> Vec<u16> {
    s.encode_wide().chain(std::iter::once(0)).collect()
}

fn env_block<I, K, V>(overrides: I) -> Vec<u16>
where
    I: IntoIterator<Item = (K, V)>,
    K: AsRef<OsStr>,
    V: AsRef<OsStr>,
{
    let upper = |k: &OsStr| k.to_string_lossy().to_uppercase();
    let mut map: BTreeMap<String, (OsString, OsString)> = BTreeMap::new();
    for (k, v) in std::env::vars_os() {
        map.insert(upper(&k), (k, v));
    }
    for (k, v) in overrides {
        map.insert(upper(k.as_ref()), (k.as_ref().to_owned(), v.as_ref().to_owned()));
    }
    let mut out = Vec::new();
    for (_, (k, v)) in map {
        out.extend(k.encode_wide());
        out.push(b'=' as u16);
        out.extend(v.encode_wide());
        out.push(0);
    }
    out.push(0);
    if out.len() == 1 {
        out.push(0);
    }
    out
}

/// Starts `exe` with the given command-line tail, environment additions and working folder. The main thread is
/// left suspended at the exe's entry point (see the module notes): no game code has run yet. Use `inject` and
/// then `resume`, or just `resume`.
pub fn launch_suspended<I, K, V>(exe: &Path, args: &str, env: I, cwd: Option<&Path>) -> Result<Launched>
where
    I: IntoIterator<Item = (K, V)>,
    K: AsRef<OsStr>,
    V: AsRef<OsStr>,
{
    let mut cmd: Vec<u16> = Vec::new();
    cmd.push(b'"' as u16);
    cmd.extend(exe.as_os_str().encode_wide());
    cmd.push(b'"' as u16);
    if !args.trim().is_empty() {
        cmd.push(b' ' as u16);
        cmd.extend(OsStr::new(args).encode_wide());
    }
    cmd.push(0);
    let exe_w = wide(exe.as_os_str());
    let cwd_w = cwd.map(|c| wide(c.as_os_str()));
    let env_w = env_block(env);

    // SAFETY: all-zero is a valid STARTUPINFOW / PROCESS_INFORMATION before the calls fill them in.
    let mut si: STARTUPINFOW = unsafe { std::mem::zeroed() };
    si.cb = std::mem::size_of::<STARTUPINFOW>() as u32;
    let mut pi: PROCESS_INFORMATION = unsafe { std::mem::zeroed() };
    // SAFETY: every pointer points to a live NUL-terminated buffer for the duration of the call.
    let ok = unsafe {
        CreateProcessW(
            exe_w.as_ptr(),
            cmd.as_mut_ptr(),
            null(),
            null(),
            0,
            CREATE_SUSPENDED | CREATE_UNICODE_ENVIRONMENT,
            env_w.as_ptr() as *const c_void,
            cwd_w.as_ref().map_or(null(), |c| c.as_ptr()),
            &si,
            &mut pi,
        )
    };
    if ok == 0 {
        return Err(HookError::last("CreateProcessW"));
    }
    let launched = Launched { pid: pi.dwProcessId, x86: false, process: Owned(pi.hProcess), thread: Owned(pi.hThread) };
    let mut wow = 0i32;
    // SAFETY: valid process handle.
    if unsafe { IsWow64Process(launched.process.0, &mut wow) } == 0 {
        let e = HookError::last("IsWow64Process");
        launched.terminate();
        return Err(e);
    }
    let launched = Launched { x86: wow != 0, ..launched };
    if let Err(e) = run_to_entry(&launched) {
        launched.terminate();
        return Err(e);
    }
    Ok(launched)
}

/// Lets the process run (after `inject`, or without any hook).
pub fn resume(l: &Launched) -> Result<()> {
    // SAFETY: valid thread handle.
    let prev = unsafe { ResumeThread(l.thread.0) };
    if prev == u32::MAX {
        return Err(HookError::last("ResumeThread"));
    }
    Ok(())
}

// ---------------------------------------------------------------------------------------------- remote memory

fn read_mem(l: &Launched, addr: usize, buf: &mut [u8]) -> Result<()> {
    let mut got = 0usize;
    // SAFETY: buf is a live, writable buffer of buf.len() bytes.
    let ok = unsafe { ReadProcessMemory(l.process.0, addr as *const c_void, buf.as_mut_ptr() as *mut c_void, buf.len(), &mut got) };
    if ok == 0 || got != buf.len() {
        return Err(HookError::last("ReadProcessMemory"));
    }
    Ok(())
}

fn write_mem(l: &Launched, addr: usize, data: &[u8]) -> Result<()> {
    let mut put = 0usize;
    // SAFETY: data is a live buffer of data.len() bytes.
    let ok = unsafe { WriteProcessMemory(l.process.0, addr as *const c_void, data.as_ptr() as *const c_void, data.len(), &mut put) };
    if ok == 0 || put != data.len() {
        return Err(HookError::last("WriteProcessMemory"));
    }
    Ok(())
}

fn le32(b: &[u8], at: usize) -> u32 {
    u32::from_le_bytes([b[at], b[at + 1], b[at + 2], b[at + 3]])
}

#[repr(C)]
struct ProcessBasicInformation {
    exit_status: isize,
    peb_base: usize,
    affinity_mask: usize,
    base_priority: isize,
    pid: usize,
    parent_pid: usize,
}

/// Entry point address of the exe image in the target.
fn entry_point(l: &Launched) -> Result<usize> {
    let image_base: usize = if l.x86 {
        let mut peb32 = 0usize;
        // ProcessWow64Information = 26
        // SAFETY: the out buffer is one usize.
        let st = unsafe {
            NtQueryInformationProcess(l.process.0, 26, &mut peb32 as *mut usize as *mut c_void, std::mem::size_of::<usize>() as u32, null_mut())
        };
        if st != 0 || peb32 == 0 {
            return Err(HookError::Protocol(format!("NtQueryInformationProcess(Wow64) status {st:#x}")));
        }
        let mut b = [0u8; 4];
        read_mem(l, peb32 + 8, &mut b)?;
        u32::from_le_bytes(b) as usize
    } else {
        // SAFETY: all-zero is a valid value for this plain struct.
        let mut pbi: ProcessBasicInformation = unsafe { std::mem::zeroed() };
        // ProcessBasicInformation = 0
        // SAFETY: pbi is a live buffer of the size passed.
        let st = unsafe {
            NtQueryInformationProcess(
                l.process.0,
                0,
                &mut pbi as *mut ProcessBasicInformation as *mut c_void,
                std::mem::size_of::<ProcessBasicInformation>() as u32,
                null_mut(),
            )
        };
        if st != 0 || pbi.peb_base == 0 {
            return Err(HookError::Protocol(format!("NtQueryInformationProcess status {st:#x}")));
        }
        let mut b = [0u8; 8];
        read_mem(l, pbi.peb_base + 0x10, &mut b)?;
        u64::from_le_bytes(b) as usize
    };
    let mut dos = [0u8; 0x40];
    read_mem(l, image_base, &mut dos)?;
    if &dos[0..2] != b"MZ" {
        return Err(HookError::Protocol("target image has no MZ header".into()));
    }
    let nt = le32(&dos, 0x3c) as usize;
    let mut hdr = [0u8; 0x30];
    read_mem(l, image_base + nt, &mut hdr)?;
    if &hdr[0..4] != b"PE\0\0" {
        return Err(HookError::Protocol("target image has no PE header".into()));
    }
    // PE signature (4) + file header (20) = optional header; AddressOfEntryPoint is at offset 16 in both PE32 and PE32+.
    let rva = le32(&hdr, 24 + 16) as usize;
    if rva == 0 {
        return Err(HookError::Unsupported("target exe has no entry point".into()));
    }
    Ok(image_base + rva)
}

/// Current instruction pointer of the (suspended) main thread; None when it cannot be read yet.
fn thread_ip(l: &Launched) -> Option<usize> {
    if l.x86 {
        // SAFETY: all-zero is valid for the plain-data context.
        let mut ctx: WOW64_CONTEXT = unsafe { std::mem::zeroed() };
        ctx.ContextFlags = WOW64_CONTEXT_CONTROL;
        // SAFETY: ctx is live and correctly aligned.
        if unsafe { Wow64GetThreadContext(l.thread.0, &mut ctx) } == 0 {
            return None;
        }
        Some(ctx.Eip as usize)
    } else {
        // SAFETY: as above; CONTEXT carries its own 16 byte alignment.
        let mut ctx: CONTEXT = unsafe { std::mem::zeroed() };
        ctx.ContextFlags = CONTEXT_CONTROL_AMD64;
        // SAFETY: ctx is live.
        if unsafe { GetThreadContext(l.thread.0, &mut ctx) } == 0 {
            return None;
        }
        Some(ctx.Rip as usize)
    }
}

/// Runs the main thread until it sits at the entry point, then leaves it suspended there.
fn run_to_entry(l: &Launched) -> Result<()> {
    let entry = entry_point(l)?;
    let mut original = [0u8; 2];
    read_mem(l, entry, &mut original)?;
    let mut old = 0u32;
    // SAFETY: changing protection of two bytes of the target's code page.
    if unsafe { VirtualProtectEx(l.process.0, entry as *const c_void, 2, PAGE_EXECUTE_READWRITE, &mut old) } == 0 {
        return Err(HookError::last("VirtualProtectEx"));
    }
    write_mem(l, entry, &[0xEB, 0xFE])?; // jmp $
    // SAFETY: valid process handle.
    unsafe { FlushInstructionCache(l.process.0, entry as *const c_void, 2) };

    // SAFETY: valid thread handle.
    if unsafe { ResumeThread(l.thread.0) } == u32::MAX {
        return Err(HookError::last("ResumeThread"));
    }
    let deadline = Instant::now() + ENTRY_TIMEOUT;
    loop {
        std::thread::sleep(Duration::from_millis(3));
        if !l.is_running() {
            return Err(HookError::Protocol(format!("the program exited before reaching its entry point (code {:?})", l.exit_code())));
        }
        // SAFETY: valid thread handle.
        if unsafe { SuspendThread(l.thread.0) } == u32::MAX {
            return Err(HookError::last("SuspendThread"));
        }
        if thread_ip(l) == Some(entry) {
            break;
        }
        // SAFETY: valid thread handle.
        unsafe { ResumeThread(l.thread.0) };
        if Instant::now() > deadline {
            return Err(HookError::Timeout("the program did not reach its entry point".into()));
        }
    }
    write_mem(l, entry, &original)?;
    // SAFETY: valid handles; restore the page protection and flush.
    unsafe {
        let mut ignored = 0u32;
        VirtualProtectEx(l.process.0, entry as *const c_void, 2, old, &mut ignored);
        FlushInstructionCache(l.process.0, entry as *const c_void, 2);
    }
    Ok(())
}

// ---------------------------------------------------------------------------------------------- export lookup

/// Base address of a module in the target, by file name.
fn remote_module_base(l: &Launched, name: &str) -> Result<usize> {
    let flags = if l.x86 { TH32CS_SNAPMODULE | TH32CS_SNAPMODULE32 } else { TH32CS_SNAPMODULE };
    let mut last = 0;
    for _ in 0..100 {
        // SAFETY: plain snapshot call.
        let snap = unsafe { CreateToolhelp32Snapshot(flags, l.pid) };
        if snap == INVALID_HANDLE_VALUE {
            // SAFETY: no preconditions.
            last = unsafe { GetLastError() };
            std::thread::sleep(Duration::from_millis(10));
            continue;
        }
        let snap = Owned(snap);
        // SAFETY: all-zero is valid before dwSize is set.
        let mut me: MODULEENTRY32W = unsafe { std::mem::zeroed() };
        me.dwSize = std::mem::size_of::<MODULEENTRY32W>() as u32;
        // SAFETY: me is a live entry with dwSize set.
        let mut ok = unsafe { Module32FirstW(snap.0, &mut me) };
        while ok != 0 {
            let len = me.szModule.iter().position(|&c| c == 0).unwrap_or(me.szModule.len());
            let module = OsString::from_wide(&me.szModule[..len]).to_string_lossy().to_lowercase();
            if module == name {
                return Ok(me.modBaseAddr as usize);
            }
            // SAFETY: as above.
            ok = unsafe { Module32NextW(snap.0, &mut me) };
        }
        std::thread::sleep(Duration::from_millis(10));
    }
    Err(HookError::Protocol(format!("{name} is not loaded in the target (last error {last})")))
}

/// Address of an exported function of a module loaded in the target, found by reading its export table from the
/// target's memory (works for 32-bit targets from a 64-bit host).
fn remote_export(l: &Launched, base: usize, func: &str) -> Result<usize> {
    let mut head = vec![0u8; 0x1000];
    read_mem(l, base, &mut head)?;
    let nt = le32(&head, 0x3c) as usize;
    if nt + 0x90 > head.len() || &head[nt..nt + 4] != b"PE\0\0" {
        return Err(HookError::Protocol("bad PE header in target module".into()));
    }
    let opt = nt + 24;
    let magic = u16::from_le_bytes([head[opt], head[opt + 1]]);
    let dd = if magic == 0x20b { opt + 112 } else { opt + 96 };
    let exp_rva = le32(&head, dd) as usize;
    let exp_size = le32(&head, dd + 4) as usize;
    if exp_rva == 0 || exp_size < 40 {
        return Err(HookError::Protocol("target module has no export table".into()));
    }
    let mut exp = vec![0u8; exp_size];
    read_mem(l, base + exp_rva, &mut exp)?;
    // Read `len` bytes at `rva`, from the cached export block when possible.
    let rd = |rva: usize, len: usize| -> Result<Vec<u8>> {
        if rva >= exp_rva && rva + len <= exp_rva + exp_size {
            Ok(exp[rva - exp_rva..rva - exp_rva + len].to_vec())
        } else {
            let mut v = vec![0u8; len];
            read_mem(l, base + rva, &mut v)?;
            Ok(v)
        }
    };
    let n_names = le32(&exp, 24) as usize;
    let funcs_rva = le32(&exp, 28) as usize;
    let names_rva = le32(&exp, 32) as usize;
    let ords_rva = le32(&exp, 36) as usize;
    let names = rd(names_rva, n_names * 4)?;
    let ords = rd(ords_rva, n_names * 2)?;
    for i in 0..n_names {
        let name_rva = le32(&names, i * 4) as usize;
        let s = rd(name_rva, func.len() + 1);
        let s = match s {
            Ok(s) => s,
            Err(_) => continue,
        };
        if &s[..func.len()] == func.as_bytes() && s[func.len()] == 0 {
            let ord = u16::from_le_bytes([ords[i * 2], ords[i * 2 + 1]]) as usize;
            let f = rd(funcs_rva + ord * 4, 4)?;
            let f_rva = le32(&f, 0) as usize;
            if f_rva >= exp_rva && f_rva < exp_rva + exp_size {
                return Err(HookError::Unsupported(format!("{func} is a forwarded export")));
            }
            return Ok(base + f_rva);
        }
    }
    Err(HookError::Protocol(format!("{func} not exported by the target module")))
}

// ---------------------------------------------------------------------------------------------- pipe

/// Server end of the per-process pipe.
struct Pipe {
    handle: Owned,
    pending: Vec<u8>,
}

impl Pipe {
    fn create(pid: u32) -> Result<Pipe> {
        let name = wide(OsStr::new(&format!(r"\\.\pipe\couchlink-hook-{pid}")));
        // SAFETY: name is NUL-terminated.
        let h = unsafe {
            CreateNamedPipeW(
                name.as_ptr(),
                PIPE_ACCESS_DUPLEX,
                PIPE_TYPE_BYTE | PIPE_READMODE_BYTE | PIPE_NOWAIT | PIPE_REJECT_REMOTE_CLIENTS,
                1,
                64 * 1024,
                64 * 1024,
                0,
                null(),
            )
        };
        if h == INVALID_HANDLE_VALUE {
            return Err(HookError::last("CreateNamedPipeW"));
        }
        Ok(Pipe { handle: Owned(h), pending: Vec::new() })
    }

    /// Client end, for a hook that was loaded as a stand-in DLL: there the DLL is the pipe server (it exists once the
    /// game has loaded the stand-in and started it), so this waits for the pipe to appear.
    fn connect(pid: u32, timeout: Duration) -> Result<Pipe> {
        let name = wide(OsStr::new(&format!(r"\\.\pipe\couchlink-hook-{pid}")));
        let deadline = Instant::now() + timeout;
        loop {
            // SAFETY: name is NUL-terminated; the other arguments are plain values or null.
            let h = unsafe {
                CreateFileW(
                    name.as_ptr(),
                    GENERIC_READ | GENERIC_WRITE,
                    0,
                    null(),
                    OPEN_EXISTING,
                    FILE_ATTRIBUTE_NORMAL,
                    null_mut(),
                )
            };
            if h != INVALID_HANDLE_VALUE {
                return Ok(Pipe { handle: Owned(h), pending: Vec::new() });
            }
            // SAFETY: no preconditions.
            let code = unsafe { GetLastError() };
            if Instant::now() > deadline {
                return Err(HookError::Timeout(format!(
                    "the stand-in hook DLL never opened its pipe in process {pid} (Windows error {code}); the game did not load it, or the config file was missing"
                )));
            }
            if code == ERROR_PIPE_BUSY {
                // SAFETY: name is NUL-terminated.
                unsafe { WaitNamedPipeW(name.as_ptr(), 100) };
            } else {
                std::thread::sleep(Duration::from_millis(25));
            }
        }
    }

    /// Waits for the DLL to connect, then switches to blocking mode.
    fn accept(&mut self, l: &Launched, timeout: Duration) -> Result<()> {
        let deadline = Instant::now() + timeout;
        loop {
            // SAFETY: valid pipe handle, no overlapped I/O.
            let ok = unsafe { ConnectNamedPipe(self.handle.0, null_mut()) };
            // SAFETY: no preconditions.
            if ok != 0 || unsafe { GetLastError() } == ERROR_PIPE_CONNECTED {
                break;
            }
            if !l.is_running() {
                return Err(HookError::Protocol("the program exited while the hook was loading".into()));
            }
            if Instant::now() > deadline {
                return Err(HookError::Timeout("the hook DLL did not connect to its pipe".into()));
            }
            std::thread::sleep(Duration::from_millis(5));
        }
        let mode = PIPE_WAIT | PIPE_READMODE_BYTE;
        // SAFETY: mode outlives the call.
        if unsafe { SetNamedPipeHandleState(self.handle.0, &mode, null(), null()) } == 0 {
            return Err(HookError::last("SetNamedPipeHandleState"));
        }
        Ok(())
    }

    fn send_line(&self, line: &str) -> Result<()> {
        let mut data = line.as_bytes().to_vec();
        data.push(b'\n');
        let mut off = 0;
        while off < data.len() {
            let mut wrote = 0u32;
            // SAFETY: the slice is live for the call.
            let ok = unsafe {
                WriteFile(self.handle.0, data[off..].as_ptr(), (data.len() - off) as u32, &mut wrote, null_mut())
            };
            if ok == 0 || wrote == 0 {
                return Err(HookError::last("WriteFile(hook pipe)"));
            }
            off += wrote as usize;
        }
        Ok(())
    }

    fn read_line(&mut self, timeout: Duration) -> Result<String> {
        let deadline = Instant::now() + timeout;
        loop {
            if let Some(pos) = self.pending.iter().position(|&b| b == b'\n') {
                let line: Vec<u8> = self.pending.drain(..=pos).collect();
                return Ok(String::from_utf8_lossy(&line[..line.len() - 1]).trim_end_matches('\r').to_string());
            }
            let mut avail = 0u32;
            // SAFETY: valid pipe handle; only the available-bytes out parameter is used.
            if unsafe { PeekNamedPipe(self.handle.0, null_mut(), 0, null_mut(), &mut avail, null_mut()) } == 0 {
                return Err(HookError::last("PeekNamedPipe(hook pipe)"));
            }
            if avail > 0 {
                let mut buf = vec![0u8; avail as usize];
                let mut got = 0u32;
                // SAFETY: buf has `avail` bytes.
                if unsafe { ReadFile(self.handle.0, buf.as_mut_ptr(), avail, &mut got, null_mut()) } == 0 {
                    return Err(HookError::last("ReadFile(hook pipe)"));
                }
                self.pending.extend_from_slice(&buf[..got as usize]);
                continue;
            }
            if Instant::now() > deadline {
                return Err(HookError::Timeout("the hook did not answer".into()));
            }
            std::thread::sleep(Duration::from_millis(2));
        }
    }

    fn request(&mut self, msg: &serde_json::Value, timeout: Duration) -> Result<serde_json::Value> {
        self.send_line(&msg.to_string())?;
        let line = self.read_line(timeout)?;
        serde_json::from_str(&line).map_err(|e| HookError::Protocol(format!("bad reply from the hook: {e}: {line}")))
    }
}

/// Connection to the hook inside one copy. Dropping it closes the pipe; the hooks stay installed.
pub struct HookHandle {
    pid: u32,
    pipe: Mutex<Pipe>,
    first: HookStatus,
}

impl std::fmt::Debug for HookHandle {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("HookHandle").field("pid", &self.pid).finish()
    }
}

impl HookHandle {
    pub fn pid(&self) -> u32 {
        self.pid
    }

    /// Connects to a hook that a stand-in DLL started in process `pid` (see [`crate::proxy`]), waiting up to `timeout`
    /// for the game to load it. The hook already has its configuration (from the file), so the first status is read
    /// here; an error is returned if the stand-in stayed inert.
    pub fn connect(pid: u32, timeout: Duration) -> Result<HookHandle> {
        let mut pipe = Pipe::connect(pid, timeout)?;
        let first = parse_status(pipe.request(&serde_json::json!({ "cmd": "status" }), REPLY_TIMEOUT)?)?;
        Ok(HookHandle { pid, pipe: Mutex::new(pipe), first })
    }

    /// The status the hook reported right after applying the configuration.
    pub fn initial_status(&self) -> &HookStatus {
        &self.first
    }

    /// Asks the hook for its current status: installed hook groups, counters, notes.
    pub fn status(&self) -> Result<HookStatus> {
        let mut pipe = self.pipe.lock().unwrap_or_else(|e| e.into_inner());
        let v = pipe.request(&serde_json::json!({ "cmd": "status" }), REPLY_TIMEOUT)?;
        parse_status(v)
    }

    /// True while the hook answers.
    pub fn ping(&self) -> bool {
        let mut pipe = self.pipe.lock().unwrap_or_else(|e| e.into_inner());
        pipe.request(&serde_json::json!({ "cmd": "ping" }), Duration::from_secs(2)).is_ok()
    }
}

fn parse_status(v: serde_json::Value) -> Result<HookStatus> {
    let s: HookStatus = serde_json::from_value(v.clone()).map_err(|e| HookError::Protocol(format!("bad status: {e}")))?;
    if !s.ok {
        return Err(HookError::Rejected(s.error.unwrap_or_else(|| v.to_string())));
    }
    Ok(s)
}

// ---------------------------------------------------------------------------------------------- inject

fn load_library_in_target(l: &Launched, dll: &Path) -> Result<u32> {
    let path = wide(dll.as_os_str());
    let bytes = path.len() * 2;
    // SAFETY: allocating in our own child; the memory is freed below.
    let remote = unsafe { VirtualAllocEx(l.process.0, null(), bytes, MEM_COMMIT | MEM_RESERVE, PAGE_READWRITE) };
    if remote.is_null() {
        return Err(HookError::last("VirtualAllocEx"));
    }
    let free = |remote: *mut c_void| {
        // SAFETY: remote was returned by VirtualAllocEx for this process.
        unsafe { VirtualFreeEx(l.process.0, remote, 0, MEM_RELEASE) };
    };
    // SAFETY: path is a live buffer of `bytes` bytes.
    let src = unsafe { std::slice::from_raw_parts(path.as_ptr() as *const u8, bytes) };
    if let Err(e) = write_mem(l, remote as usize, src) {
        free(remote);
        return Err(e);
    }
    let k32 = remote_module_base(l, "kernel32.dll")?;
    let load = remote_export(l, k32, "LoadLibraryW")?;
    // SAFETY: `load` is the address of LoadLibraryW in the target, which has the thread start signature in effect.
    let start: unsafe extern "system" fn(*mut c_void) -> u32 = unsafe { std::mem::transmute(load) };
    // SAFETY: creating a thread in our own child process.
    let thread = unsafe { CreateRemoteThread(l.process.0, null(), 0, Some(start), remote, 0, null_mut()) };
    if thread.is_null() {
        let e = HookError::last("CreateRemoteThread");
        free(remote);
        return Err(e);
    }
    let thread = Owned(thread);
    // SAFETY: valid handle.
    let wait = unsafe { WaitForSingleObject(thread.0, 15_000) };
    if wait == WAIT_TIMEOUT {
        return Err(HookError::Timeout("LoadLibrary in the target did not return".into()));
    }
    let mut code = 0u32;
    // SAFETY: valid handle.
    unsafe { GetExitCodeThread(thread.0, &mut code) };
    free(remote);
    Ok(code)
}

/// Loads the hook DLL into the suspended process and configures it. The process stays suspended; call `resume`.
pub fn inject(l: &Launched, config: &HookConfig) -> Result<HookHandle> {
    inject_with(l, config, &HookOptions::default())
}

/// Like `inject` with the extra switches in `HookOptions`.
pub fn inject_with(l: &Launched, config: &HookConfig, options: &HookOptions) -> Result<HookHandle> {
    let dll = dll_path(l.x86)?;
    let mut pipe = Pipe::create(l.pid)?;
    // The exit code is the module handle cut to 32 bits: zero means LoadLibrary failed (or, rarely, a base address
    // with zero low bits), so the wait for the pipe is short in that case.
    let loaded = load_library_in_target(l, &dll)?;
    let wait = if loaded == 0 { Duration::from_secs(3) } else { CONNECT_TIMEOUT };
    pipe.accept(l, wait).map_err(|e| match e {
        HookError::Timeout(_) => HookError::Protocol(format!(
            "the hook DLL did not start in the target ({}); LoadLibrary returned {loaded:#x}",
            dll.display()
        )),
        other => other,
    })?;
    let reply = pipe.request(&config_message(config, options), REPLY_TIMEOUT)?;
    let first = parse_status(reply)?;
    Ok(HookHandle { pid: l.pid, pipe: Mutex::new(pipe), first })
}
