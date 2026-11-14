//! The first 64 bytes of an ELF file: enough to tell which machine a program or library is
//! built for, and whether it is an AppImage.
//!
//! Used on the libraries of an imported version (`lib/x86_64/*.so`, which the client's own
//! linker maps into the game's process) and on the client program itself. Nothing beyond the
//! file header is read: no section, no dynamic table, no string. The header is the first
//! 64 bytes of every ELF64 file, so a shorter input is simply not an ELF64 file.
//!
//! Not guaranteed: that a file which passes is a well-formed or harmless library. The check
//! answers one question — "built for x86-64?" — so that a version for another machine is
//! refused with that reason instead of failing somewhere inside the sandbox.

/// Bytes read from the start of a file.
pub const HEADER_BYTES: usize = 64;

const ELFCLASS64: u8 = 2;
const ELFDATA2LSB: u8 = 1;
const EV_CURRENT: u8 = 1;
const ET_EXEC: u16 = 2;
const ET_DYN: u16 = 3;
pub const EM_X86_64: u16 = 62;

/// What a header says.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Header {
    pub machine: u16,
    pub kind: u16,
    /// AppImage type 1 or 2: the magic `AI\x01` / `AI\x02` in the padding of `e_ident`.
    pub appimage: bool,
}

/// Reads the header of an ELF64 little-endian file. `Err` says, in a few words, why the bytes
/// are not one.
pub fn header(bytes: &[u8]) -> Result<Header, &'static str> {
    if bytes.len() >= 2 && &bytes[..2] == b"#!" {
        return Err("a script, not a program");
    }
    if bytes.len() < 4 || &bytes[..4] != b"\x7fELF" {
        return Err("not an ELF file");
    }
    if bytes.len() < HEADER_BYTES {
        return Err("a truncated ELF file");
    }
    if bytes[4] != ELFCLASS64 {
        return Err("a 32-bit ELF file");
    }
    if bytes[5] != ELFDATA2LSB {
        return Err("a big-endian ELF file");
    }
    if bytes[6] != EV_CURRENT {
        return Err("an ELF file of an unknown version");
    }
    let kind = u16::from_le_bytes([bytes[16], bytes[17]]);
    let machine = u16::from_le_bytes([bytes[18], bytes[19]]);
    let appimage = bytes[8] == b'A' && bytes[9] == b'I' && matches!(bytes[10], 1 | 2);
    Ok(Header { machine, kind, appimage })
}

/// A name for the machine of a header, for messages.
pub fn machine_name(machine: u16) -> &'static str {
    match machine {
        3 => "x86 (32-bit)",
        40 => "ARM (32-bit)",
        EM_X86_64 => "x86-64",
        183 => "ARM64",
        _ => "another machine",
    }
}

/// An x86-64 shared library (what `lib/x86_64/` of a version must hold).
pub fn check_x86_64_library(bytes: &[u8]) -> Result<(), String> {
    let header = header(bytes).map_err(str::to_string)?;
    if header.machine != EM_X86_64 {
        return Err(format!("a library for {}, not x86-64", machine_name(header.machine)));
    }
    if header.kind != ET_DYN {
        return Err("not a shared library".into());
    }
    Ok(())
}

/// An x86-64 program that is not an AppImage (what the client must be).
pub fn check_x86_64_program(bytes: &[u8]) -> Result<(), String> {
    let header = header(bytes).map_err(str::to_string)?;
    if header.appimage {
        return Err("an AppImage".into());
    }
    if header.machine != EM_X86_64 {
        return Err(format!("a program for {}, not x86-64", machine_name(header.machine)));
    }
    if !matches!(header.kind, ET_EXEC | ET_DYN) {
        return Err("not an executable program".into());
    }
    Ok(())
}

/// A minimal header for tests and fuzz seeds: ELF64, little-endian, the given type and
/// machine, the rest zero.
#[doc(hidden)]
pub fn synthetic(kind: u16, machine: u16) -> Vec<u8> {
    let mut bytes = vec![0u8; HEADER_BYTES];
    bytes[..4].copy_from_slice(b"\x7fELF");
    bytes[4] = ELFCLASS64;
    bytes[5] = ELFDATA2LSB;
    bytes[6] = EV_CURRENT;
    bytes[16..18].copy_from_slice(&kind.to_le_bytes());
    bytes[18..20].copy_from_slice(&machine.to_le_bytes());
    bytes[20] = EV_CURRENT;
    bytes
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_x86_64_library_passes_and_libraries_for_other_machines_say_which() {
        assert_eq!(check_x86_64_library(&synthetic(ET_DYN, EM_X86_64)), Ok(()));
        let arm = check_x86_64_library(&synthetic(ET_DYN, 183)).unwrap_err();
        assert!(arm.contains("ARM64"), "{arm}");
        assert!(check_x86_64_library(&synthetic(ET_DYN, 3)).unwrap_err().contains("x86 (32-bit)"));
        assert!(check_x86_64_library(&synthetic(ET_EXEC, EM_X86_64)).unwrap_err().contains("not a shared library"));
        let mut thirty_two = synthetic(ET_DYN, EM_X86_64);
        thirty_two[4] = 1;
        assert_eq!(check_x86_64_library(&thirty_two), Err("a 32-bit ELF file".into()));
    }

    #[test]
    fn a_program_must_be_x86_64_and_neither_a_script_nor_an_appimage() {
        assert_eq!(check_x86_64_program(&synthetic(ET_DYN, EM_X86_64)), Ok(()));
        assert_eq!(check_x86_64_program(&synthetic(ET_EXEC, EM_X86_64)), Ok(()));
        let mut appimage = synthetic(ET_DYN, EM_X86_64);
        appimage[8..11].copy_from_slice(b"AI\x02");
        assert_eq!(check_x86_64_program(&appimage), Err("an AppImage".into()));
        assert_eq!(
            check_x86_64_program(b"#!/bin/sh\nexec flatpak run io.mrarm.mcpelauncher\n"),
            Err("a script, not a program".into())
        );
        assert_eq!(check_x86_64_program(b""), Err("not an ELF file".into()));
        assert_eq!(check_x86_64_program(&synthetic(ET_DYN, EM_X86_64)[..40]), Err("a truncated ELF file".into()));
    }
}
