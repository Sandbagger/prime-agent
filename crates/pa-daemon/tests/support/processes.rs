//! Test process probes. Observation failure must not masquerade as cleanup.
#![allow(dead_code)] // Each integration target uses a different subset.

pub fn process_alive(pid: u32) -> bool {
    pa_types::platform::is_process_alive(pid).expect("probe process liveness")
}

pub fn child_pids_of(ppid: u32) -> Vec<u32> {
    process_parents()
        .into_iter()
        .filter_map(|(pid, parent)| (parent == ppid).then_some(pid))
        .collect()
}

#[cfg(target_os = "linux")]
pub fn process_parents() -> Vec<(u32, u32)> {
    std::fs::read_dir("/proc")
        .expect("read /proc")
        .flatten()
        .filter_map(|entry| {
            let pid = entry.file_name().to_str()?.parse().ok()?;
            let stat = std::fs::read_to_string(entry.path().join("stat")).ok()?;
            let (_, rest) = stat.rsplit_once(')')?;
            let parent = rest.split_whitespace().nth(1)?.parse().ok()?;
            Some((pid, parent))
        })
        .collect()
}

#[cfg(not(target_os = "linux"))]
pub fn process_parents() -> Vec<(u32, u32)> {
    let output = std::process::Command::new("/bin/ps")
        .args(["-axo", "pid=,ppid="])
        .output()
        .expect("enumerate process ownership with ps");
    assert!(output.status.success(), "ps process enumeration failed");
    String::from_utf8(output.stdout)
        .expect("ps emitted non-UTF-8 process IDs")
        .lines()
        .map(|line| {
            let mut fields = line.split_whitespace();
            let pid = fields.next().expect("ps PID").parse().expect("numeric PID");
            let parent = fields
                .next()
                .expect("ps PPID")
                .parse()
                .expect("numeric PPID");
            assert!(fields.next().is_none(), "unexpected ps ownership fields");
            (pid, parent)
        })
        .collect()
}

/// Numeric descriptors only: lsof's cwd, text and root pseudo-descriptors
/// are not open file descriptors and must not inflate the leak baseline.
#[cfg(not(target_os = "linux"))]
fn parse_lsof(output: &str) -> Vec<String> {
    let mut descriptors = Vec::new();
    let mut numeric = false;
    let mut kind = String::new();
    let mut name = String::new();
    let finish = |numeric: bool, kind: &str, name: &str, result: &mut Vec<String>| {
        if numeric {
            result.push(match kind {
                "IPv4" | "IPv6" | "unix" => format!("socket:{name}"),
                "PIPE" | "FIFO" => format!("pipe:{name}"),
                _ => name.to_owned(),
            });
        }
    };
    for line in output.lines() {
        if let Some(fd) = line.strip_prefix('f') {
            finish(numeric, &kind, &name, &mut descriptors);
            numeric = !fd.is_empty() && fd.bytes().all(|byte| byte.is_ascii_digit());
            kind.clear();
            name.clear();
        } else if let Some(value) = line.strip_prefix('t') {
            value.clone_into(&mut kind);
        } else if let Some(value) = line.strip_prefix('n') {
            value.clone_into(&mut name);
        }
    }
    finish(numeric, &kind, &name, &mut descriptors);
    descriptors
}

pub fn fd_snapshot(pid: u32) -> Vec<String> {
    if !process_alive(pid) {
        return Vec::new();
    }
    #[cfg(target_os = "linux")]
    {
        let entries = match std::fs::read_dir(format!("/proc/{pid}/fd")) {
            Ok(entries) => entries,
            Err(error) => {
                assert!(
                    !process_alive(pid),
                    "cannot inspect live process FDs: {error}"
                );
                return Vec::new();
            }
        };
        entries
            .map(|entry| {
                let entry = entry.expect("read descriptor entry");
                // Closing a descriptor during sampling is normal; count the
                // observed slot even when its target has already disappeared.
                std::fs::read_link(entry.path()).map_or_else(
                    |_| "?".to_string(),
                    |path| path.to_string_lossy().into_owned(),
                )
            })
            .collect()
    }
    #[cfg(not(target_os = "linux"))]
    {
        let output = std::process::Command::new("/usr/sbin/lsof")
            .args(["-nP", "-a", "-p", &pid.to_string(), "-Fftn"])
            .output()
            .expect("inspect process descriptors with lsof");
        if !process_alive(pid) {
            return Vec::new();
        }
        assert!(
            output.status.success(),
            "lsof failed for live process {pid}: {}",
            String::from_utf8_lossy(&output.stderr)
        );
        let rows =
            parse_lsof(&String::from_utf8(output.stdout).expect("lsof descriptor output encoding"));
        assert!(
            !rows.is_empty(),
            "lsof returned no descriptors for live test process {pid}"
        );
        rows
    }
}

/// These tests launch Python with the literal adjacent arguments -m rlm.repl.
/// Preserve that match on Unix hosts without Linux procfs.
pub fn kernel_pids() -> Vec<u32> {
    #[cfg(target_os = "linux")]
    let mut found: Vec<u32> = std::fs::read_dir("/proc")
        .expect("read /proc")
        .flatten()
        .filter_map(|entry| {
            let pid = entry.file_name().to_str()?.parse().ok()?;
            let cmdline = std::fs::read_to_string(entry.path().join("cmdline")).ok()?;
            let args: Vec<_> = cmdline.split('\0').collect();
            args.windows(2)
                .any(|pair| pair == ["-m", "rlm.repl"])
                .then_some(pid)
        })
        .collect();
    #[cfg(not(target_os = "linux"))]
    let mut found: Vec<u32> = {
        let output = std::process::Command::new("/bin/ps")
            .args(["-axww", "-o", "pid=,args="])
            .output()
            .expect("enumerate kernel processes");
        assert!(output.status.success(), "ps kernel enumeration failed");
        String::from_utf8(output.stdout)
            .expect("ps kernel output encoding")
            .lines()
            .filter_map(|line| {
                let mut fields = line.split_whitespace();
                let pid = fields
                    .next()
                    .expect("ps PID")
                    .parse::<u32>()
                    .expect("numeric PID");
                let args: Vec<_> = fields.collect();
                args.windows(2)
                    .any(|pair| pair == ["-m", "rlm.repl"])
                    .then_some(pid)
            })
            .collect()
    };
    found.retain(|pid| process_alive(*pid));
    found.sort_unstable();
    found
}

#[cfg(all(test, not(target_os = "linux")))]
mod tests {
    use super::parse_lsof;

    #[test]
    fn lsof_counts_numeric_descriptors_and_preserves_classes() {
        assert_eq!(parse_lsof("p42\nfcwd\ntDIR\nn/tmp\nftxt\ntREG\nn/bin/tool\nf0\ntCHR\nn/dev/null\nf3\ntunix\nnendpoint\nf4\ntPIPE\n"),
            vec!["/dev/null", "socket:endpoint", "pipe:"]);
    }

    #[test]
    fn lsof_empty_and_pseudo_only_outputs_do_not_invent_descriptors() {
        for output in ["", "p42\n", "p42\nfcwd\ntDIR\nn/tmp\n"] {
            assert!(parse_lsof(output).is_empty());
        }
    }
}
