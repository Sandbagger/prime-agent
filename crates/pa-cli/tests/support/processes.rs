//! Process ownership probes shared by Unix end-to-end teardown fixtures.

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
