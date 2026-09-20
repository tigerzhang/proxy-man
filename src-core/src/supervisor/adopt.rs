use crate::config::PersistedRuntime;
use std::ffi::{OsStr, OsString};
use std::path::{Path, PathBuf};
use std::process::Stdio;
use sysinfo::{Pid, ProcessesToUpdate, System};

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AdoptLookup {
    Found(u32),
    PortBusy { pid: u32, exe: Option<String> },
    None,
}

pub fn find_adoptable_pid(
    persisted: Option<&PersistedRuntime>,
    expected_exe: &Path,
    listen_host: &str,
    listen_port: u16,
) -> AdoptLookup {
    if let Some(rt) = persisted {
        if process_is_alive(rt.pid) {
            match process_info(rt.pid) {
                Some((exe, name, cmd))
                    if exe_matches(expected_exe, exe.as_deref(), &name, &cmd) =>
                {
                    return AdoptLookup::Found(rt.pid);
                }
                None => {
                    // Alive but not listed yet (or no permission); trust our PID file.
                    return AdoptLookup::Found(rt.pid);
                }
                Some(_) => {}
            }
        }
    }

    let listeners = find_listening_pids(listen_host, listen_port);
    for pid in &listeners {
        if process_exe_matches(*pid, expected_exe) {
            return AdoptLookup::Found(*pid);
        }
    }

    if let Some(&pid) = listeners.first() {
        return AdoptLookup::PortBusy {
            pid,
            exe: process_exe_display(pid),
        };
    }

    AdoptLookup::None
}

pub fn process_is_alive(pid: u32) -> bool {
    if pid == 0 || pid > i32::MAX as u32 {
        return false;
    }

    #[cfg(unix)]
    {
        let ret = unsafe { libc::kill(pid as i32, 0) };
        if ret == 0 {
            return true;
        }
        std::io::Error::last_os_error().raw_os_error() == Some(libc::EPERM)
    }

    #[cfg(not(unix))]
    {
        process_info(pid).is_some()
    }
}

pub fn process_exe_matches(pid: u32, expected_exe: &Path) -> bool {
    let Some((exe, name, cmd)) = process_info(pid) else {
        return false;
    };
    exe_matches(expected_exe, exe.as_deref(), &name, &cmd)
}

pub fn process_exe_display(pid: u32) -> Option<String> {
    let (exe, name, cmd) = process_info(pid)?;
    if let Some(exe) = exe {
        return Some(exe.display().to_string());
    }
    if let Some(first) = cmd.first() {
        return Some(first.to_string_lossy().into_owned());
    }
    let name = name.to_string_lossy();
    if name.is_empty() {
        None
    } else {
        Some(name.into_owned())
    }
}

pub fn find_listening_pids(host: &str, port: u16) -> Vec<u32> {
    if port == 0 {
        return Vec::new();
    }

    let mut pids = find_listening_pids_lsof(Some(host), port);
    if pids.is_empty() {
        pids = find_listening_pids_lsof(None, port);
    }

    #[cfg(target_os = "linux")]
    if pids.is_empty() {
        pids = find_listening_pids_proc(port);
    }

    pids.sort_unstable();
    pids.dedup();
    pids
}

fn process_info(pid: u32) -> Option<(Option<PathBuf>, OsString, Vec<OsString>)> {
    let mut sys = System::new();
    let pid = Pid::from_u32(pid);
    sys.refresh_processes(ProcessesToUpdate::Some(&[pid]), true);
    let proc = sys.process(pid)?;
    Some((
        proc.exe().map(Path::to_path_buf),
        proc.name().to_os_string(),
        proc.cmd().to_vec(),
    ))
}

pub(crate) fn exe_matches(
    expected: &Path,
    process_exe: Option<&Path>,
    process_name: &OsStr,
    cmd: &[OsString],
) -> bool {
    let expected_name = expected.file_name().unwrap_or(expected.as_os_str());

    if let Some(exe) = process_exe {
        if exe == expected {
            return true;
        }
        if let (Ok(a), Ok(b)) = (expected.canonicalize(), exe.canonicalize()) {
            if a == b {
                return true;
            }
        }
        if stem_matches_os(expected_name, exe.file_name().unwrap_or(exe.as_os_str())) {
            return true;
        }
    }

    if stem_matches_os(expected_name, process_name) {
        return true;
    }

    if let Some(first) = cmd.first() {
        let p = Path::new(first);
        if stem_matches_os(expected_name, p.file_name().unwrap_or(p.as_os_str())) {
            return true;
        }
    }

    false
}

fn stem_matches_os(expected: &OsStr, actual: &OsStr) -> bool {
    if expected == actual {
        return true;
    }
    stem_matches(&expected.to_string_lossy(), &actual.to_string_lossy())
}

fn stem_matches(expected: &str, actual: &str) -> bool {
    if expected.eq_ignore_ascii_case(actual) {
        return true;
    }
    if let Some(rest) = actual.strip_prefix(expected) {
        // Allow version suffixes like python3 vs python3.12, not overtls vs overtls-chain.
        if let Some(suffix) = rest.strip_prefix('.') {
            return suffix.chars().next().is_some_and(|c| c.is_ascii_digit());
        }
    }
    false
}

fn find_listening_pids_lsof(host: Option<&str>, port: u16) -> Vec<u32> {
    let spec = match host {
        Some(h) if !h.is_empty() && h != "0.0.0.0" && h != "::" && h != "*" => {
            format!("-iTCP@{h}:{port}")
        }
        _ => format!("-iTCP:{port}"),
    };

    let output = std::process::Command::new("lsof")
        .args(["-nP", "-t", "-sTCP:LISTEN", &spec])
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .output();

    let Ok(output) = output else {
        return Vec::new();
    };
    if !output.status.success() && output.stdout.is_empty() {
        return Vec::new();
    }

    parse_pid_lines(&output.stdout)
}

fn parse_pid_lines(stdout: &[u8]) -> Vec<u32> {
    String::from_utf8_lossy(stdout)
        .lines()
        .filter_map(|line| line.trim().parse::<u32>().ok())
        .filter(|pid| *pid > 0)
        .collect()
}

#[cfg(target_os = "linux")]
fn find_listening_pids_proc(port: u16) -> Vec<u32> {
    let mut inodes = Vec::new();
    collect_listen_inodes("/proc/net/tcp", port, &mut inodes);
    collect_listen_inodes("/proc/net/tcp6", port, &mut inodes);
    if inodes.is_empty() {
        return Vec::new();
    }

    let mut pids = Vec::new();
    let Ok(entries) = std::fs::read_dir("/proc") else {
        return pids;
    };
    for entry in entries.flatten() {
        let pid: u32 = match entry.file_name().to_string_lossy().parse() {
            Ok(pid) => pid,
            Err(_) => continue,
        };
        let fd_dir = format!("/proc/{pid}/fd");
        let Ok(fds) = std::fs::read_dir(fd_dir) else {
            continue;
        };
        for fd in fds.flatten() {
            let Ok(target) = std::fs::read_link(fd.path()) else {
                continue;
            };
            let target = target.to_string_lossy();
            let Some(inode_str) = target
                .strip_prefix("socket:[")
                .and_then(|s| s.strip_suffix(']'))
            else {
                continue;
            };
            if let Ok(inode) = inode_str.parse::<u64>() {
                if inodes.contains(&inode) {
                    pids.push(pid);
                    break;
                }
            }
        }
    }
    pids
}

#[cfg(target_os = "linux")]
fn collect_listen_inodes(path: &str, port: u16, inodes: &mut Vec<u64>) {
    let Ok(content) = std::fs::read_to_string(path) else {
        return;
    };
    for line in content.lines().skip(1) {
        let cols: Vec<&str> = line.split_whitespace().collect();
        if cols.len() < 10 {
            continue;
        }
        // local_address is ip:port in hex; st 0A = LISTEN
        if cols[3] != "0A" {
            continue;
        }
        let Some((_, hex_port)) = cols[1].rsplit_once(':') else {
            continue;
        };
        let Ok(p) = u16::from_str_radix(hex_port, 16) else {
            continue;
        };
        if p != port {
            continue;
        }
        if let Ok(inode) = cols[9].parse::<u64>() {
            inodes.push(inode);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn exe_matches_same_path() {
        assert!(exe_matches(
            Path::new("/usr/local/bin/overtls"),
            Some(Path::new("/usr/local/bin/overtls")),
            OsStr::new("overtls"),
            &[]
        ));
    }

    #[test]
    fn exe_matches_by_file_name() {
        assert!(exe_matches(
            Path::new("sleep"),
            Some(Path::new("/bin/sleep")),
            OsStr::new("sleep"),
            &[]
        ));
    }

    #[test]
    fn exe_matches_python_version_suffix() {
        assert!(exe_matches(
            Path::new("python3"),
            Some(Path::new("/usr/bin/python3.12")),
            OsStr::new("python3.12"),
            &[]
        ));
    }

    #[test]
    fn exe_does_not_match_similar_prefix() {
        assert!(!exe_matches(
            Path::new("overtls"),
            Some(Path::new("/usr/bin/overtls-chain")),
            OsStr::new("overtls-chain"),
            &[]
        ));
    }

    #[test]
    fn exe_matches_cmd0() {
        assert!(exe_matches(
            Path::new("clean-dns"),
            None,
            OsStr::new("clean-dns"),
            &[OsString::from("/Users/zhanghu/vpn/clean-dns/target/debug/clean-dns")]
        ));
    }

    #[test]
    fn dead_pid_is_not_alive() {
        assert!(!process_is_alive(0));
        assert!(!process_is_alive(u32::MAX));
        assert!(!process_is_alive(2_000_000_000));
    }
}
