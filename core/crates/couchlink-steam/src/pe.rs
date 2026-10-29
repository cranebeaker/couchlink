//! Bitness detection from a PE header.

use crate::error::{Result, SteamError, classify_io};
use std::io::Read;
use std::path::Path;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Bitness {
    X86,
    X64,
}

/// Read `e_lfanew`, check the `PE\0\0` signature and look at the COFF machine field.
pub fn pe_bitness_bytes(b: &[u8]) -> Result<Bitness> {
    let bad = |s: &str| SteamError::NotPe(s.to_string());
    if b.len() < 0x40 || &b[0..2] != b"MZ" {
        return Err(bad("missing MZ header"));
    }
    let e_lfanew = u32::from_le_bytes([b[0x3c], b[0x3d], b[0x3e], b[0x3f]]) as usize;
    let end = e_lfanew.checked_add(4 + 20).ok_or_else(|| bad("bad e_lfanew"))?;
    if b.len() < end || &b[e_lfanew..e_lfanew + 4] != b"PE\0\0" {
        return Err(bad("missing PE signature"));
    }
    let machine = u16::from_le_bytes([b[e_lfanew + 4], b[e_lfanew + 5]]);
    match machine {
        0x014c => Ok(Bitness::X86),
        0x8664 => Ok(Bitness::X64),
        0xaa64 => Err(bad("ARM64 binaries are not supported")),
        m => Err(bad(&format!("unknown machine type 0x{m:04x}"))),
    }
}

pub fn pe_bitness(path: &Path) -> Result<Bitness> {
    let mut f = std::fs::File::open(path).map_err(|e| classify_io("opening", path, e))?;
    // The headers sit in the first few KB of every real DLL.
    let mut buf = Vec::with_capacity(4096);
    f.by_ref().take(4096).read_to_end(&mut buf).map_err(|e| classify_io("reading", path, e))?;
    pe_bitness_bytes(&buf)
}

/// Build a minimal fake PE header (used by tests in this crate).
#[doc(hidden)]
pub fn fake_pe(bitness: Bitness) -> Vec<u8> {
    let mut b = vec![0u8; 0x200];
    b[0] = b'M';
    b[1] = b'Z';
    let e_lfanew: u32 = 0x80;
    b[0x3c..0x40].copy_from_slice(&e_lfanew.to_le_bytes());
    let p = e_lfanew as usize;
    b[p..p + 4].copy_from_slice(b"PE\0\0");
    let (machine, magic): (u16, u16) = match bitness {
        Bitness::X86 => (0x014c, 0x010b),
        Bitness::X64 => (0x8664, 0x020b),
    };
    b[p + 4..p + 6].copy_from_slice(&machine.to_le_bytes());
    let opt = p + 4 + 20;
    b[opt..opt + 2].copy_from_slice(&magic.to_le_bytes());
    b
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn detects_x86_and_x64() {
        assert_eq!(pe_bitness_bytes(&fake_pe(Bitness::X86)).unwrap(), Bitness::X86);
        assert_eq!(pe_bitness_bytes(&fake_pe(Bitness::X64)).unwrap(), Bitness::X64);
    }

    #[test]
    fn rejects_garbage() {
        assert!(pe_bitness_bytes(b"hello").is_err());
        assert!(pe_bitness_bytes(&[0u8; 0x100]).is_err());
        let mut b = fake_pe(Bitness::X64);
        b[0x80] = b'X';
        assert!(pe_bitness_bytes(&b).is_err());
        let mut b = fake_pe(Bitness::X64);
        b[0x3c..0x40].copy_from_slice(&0xffff_fff0u32.to_le_bytes());
        assert!(pe_bitness_bytes(&b).is_err());
    }

    #[test]
    fn arm64_is_rejected() {
        let mut b = fake_pe(Bitness::X64);
        b[0x84..0x86].copy_from_slice(&0xaa64u16.to_le_bytes());
        assert!(pe_bitness_bytes(&b).is_err());
    }

    #[test]
    fn reads_from_file() {
        let d = std::env::temp_dir().join(format!("cl-steam-pe-{}", std::process::id()));
        std::fs::create_dir_all(&d).unwrap();
        let f = d.join("x.dll");
        std::fs::write(&f, fake_pe(Bitness::X86)).unwrap();
        assert_eq!(pe_bitness(&f).unwrap(), Bitness::X86);
        std::fs::remove_dir_all(&d).unwrap();
    }
}
