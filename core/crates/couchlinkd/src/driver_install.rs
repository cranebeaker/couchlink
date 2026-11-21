//! Installing the two drivers "Give to Windows" uses, from their makers' own releases: ViGEmBus (the virtual controller)
//! and HidHide (hiding the originals), both by Nefarius. Couchlink does not carry them: it downloads the newest signed
//! installer from the maker's GitHub page, checks that it is signed by the maker, and runs it; Windows asks the player
//! for permission (the administrator prompt) before anything is installed.

#![cfg_attr(not(windows), allow(dead_code))]

/// The drivers that can be installed here: the name it goes by in the protocol and the maker's repository.
pub fn repo(which: &str) -> Option<&'static str> {
    match which {
        "controller" => Some("ViGEmBus"),
        "hide" => Some("HidHide"),
        _ => None,
    }
}

/// The PowerShell that finds the newest installer, downloads it, checks its signature and (unless `download_only`) runs
/// it as administrator and waits for it. `repo` is always one of [`repo`]'s fixed names.
pub fn script(repo: &str, download_only: bool) -> String {
    let run = if download_only {
        "Write-Output ('signed by ' + $sig.SignerCertificate.Subject)"
    } else {
        "$p = Start-Process -FilePath $file -Verb RunAs -Wait -PassThru; if ($p.ExitCode -ne 0 -and $p.ExitCode -ne 3010) { throw ('the installer ended with code ' + $p.ExitCode) }"
    };
    format!(
        "$ErrorActionPreference = 'Stop'; $ProgressPreference = 'SilentlyContinue'; \
         [Net.ServicePointManager]::SecurityProtocol = [Net.SecurityProtocolType]::Tls12; \
         $rel = Invoke-RestMethod -Uri 'https://api.github.com/repos/nefarius/{repo}/releases/latest' -Headers @{{'User-Agent' = 'couchlinkd'}}; \
         $asset = $rel.assets | Where-Object {{ $_.name -like '*.exe' }} | Select-Object -First 1; \
         if (-not $asset) {{ throw 'no installer in the newest release' }}; \
         if ($asset.browser_download_url -notlike 'https://github.com/nefarius/{repo}/*') {{ throw 'the installer is not on the maker page' }}; \
         $file = Join-Path $env:TEMP $asset.name; \
         Invoke-WebRequest -Uri $asset.browser_download_url -OutFile $file; \
         $sig = Get-AuthenticodeSignature $file; \
         if ($sig.Status -ne 'Valid' -or $sig.SignerCertificate.Subject -notmatch 'Nefarius') {{ Remove-Item $file -Force; throw 'the installer is not signed by its maker' }}; \
         {run}; \
         if ({dl}) {{ Remove-Item $file -Force }}",
        dl = if download_only { "$true" } else { "$false" },
    )
}

/// Downloads, checks and runs the installer of one driver; waits until it is done. Err is what went wrong.
pub fn install(which: &str) -> Result<(), String> {
    run(which, false).map(|_| ())
}

/// Downloads and checks the newest installer without running it (for tests); Ok is who signed it.
pub fn check_download(which: &str) -> Result<String, String> {
    run(which, true)
}

fn run(which: &str, download_only: bool) -> Result<String, String> {
    let repo = repo(which).ok_or_else(|| format!("{which} is not a driver Couchlink installs"))?;
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        let out = std::process::Command::new("powershell")
            .args(["-NoProfile", "-NonInteractive", "-ExecutionPolicy", "Bypass", "-Command", &script(repo, download_only)])
            .creation_flags(0x0800_0000)
            .output()
            .map_err(|e| format!("cannot start PowerShell: {e}"))?;
        if out.status.success() {
            Ok(String::from_utf8_lossy(&out.stdout).trim().to_string())
        } else {
            let err = String::from_utf8_lossy(&out.stderr);
            Err(err.lines().next().unwrap_or("the installer failed").trim().to_string())
        }
    }
    #[cfg(not(windows))]
    {
        let _ = (repo, download_only);
        Err("the drivers are for Windows".to_string())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_the_two_drivers_can_be_asked_for() {
        assert_eq!(repo("controller"), Some("ViGEmBus"));
        assert_eq!(repo("hide"), Some("HidHide"));
        assert_eq!(repo("anything; calc"), None);
    }

    /// The script is valid PowerShell (the parser reads it without a complaint) for both drivers and both modes.
    #[cfg(windows)]
    #[test]
    fn the_scripts_parse() {
        use std::os::windows::process::CommandExt;
        for repo in ["ViGEmBus", "HidHide"] {
            for download_only in [true, false] {
                let check = format!("$e = $null; [void][System.Management.Automation.Language.Parser]::ParseInput([Console]::In.ReadToEnd(), [ref]$null, [ref]$e); if ($e.Count) {{ $e | ForEach-Object {{ Write-Output $_.Message }}; exit 1 }}");
                let mut child = std::process::Command::new("powershell")
                    .args(["-NoProfile", "-NonInteractive", "-Command", &check])
                    .stdin(std::process::Stdio::piped())
                    .stdout(std::process::Stdio::piped())
                    .creation_flags(0x0800_0000)
                    .spawn()
                    .unwrap();
                use std::io::Write;
                child.stdin.take().unwrap().write_all(script(repo, download_only).as_bytes()).unwrap();
                let out = child.wait_with_output().unwrap();
                assert!(out.status.success(), "{repo} {download_only}: {}", String::from_utf8_lossy(&out.stdout));
            }
        }
    }

    /// Needs the network: the newest installers of both drivers download from their makers and are signed by them.
    /// Run with `cargo test -p couchlinkd --bin couchlinkd -- --ignored installers`.
    #[cfg(windows)]
    #[test]
    #[ignore]
    fn installers_download_and_are_signed_by_their_maker() {
        for which in ["controller", "hide"] {
            let signer = check_download(which).unwrap_or_else(|e| panic!("{which}: {e}"));
            assert!(signer.contains("Nefarius"), "{signer}");
        }
    }
}
