//! Bounded, optional host measurements. Unsupported hosts report null fields.
use serde_json::{json, Value};
use std::io::Read;
use std::path::Path;

fn read(path: &str) -> String {
    let mut text = String::new();
    if let Ok(file) = std::fs::File::open(path) {
        let _ = file.take(64 * 1024).read_to_string(&mut text);
    }
    text
}

fn number(text: &str, field: &str) -> Option<u64> {
    text.lines().find_map(|line| {
        let (name, rest) = line.split_once(':')?;
        (name == field)
            .then(|| rest.split_whitespace().next()?.parse().ok())
            .flatten()
    })
}

/// Host-wide busy and total clock ticks from the aggregate `cpu` line of
/// `/proc/stat`. Busy is user, nice, system, irq, softirq and steal; total adds
/// idle and iowait, so a host waiting on its disk counts as not busy, as `top`
/// has it. The guest columns are already inside user and nice and are not
/// added again.
fn cpu_ticks(stat: &str) -> Option<(u64, u64)> {
    let line = stat.lines().find_map(|line| line.strip_prefix("cpu "))?;
    let ticks: Vec<u64> = line
        .split_whitespace()
        .take(8)
        .map(|field| field.parse().ok())
        .collect::<Option<_>>()?;
    let [user, nice, system, idle, iowait, irq, softirq, steal] = ticks[..] else {
        return None;
    };
    let busy = [user, nice, system, irq, softirq, steal]
        .into_iter()
        .fold(0u64, u64::saturating_add);
    Some((busy, busy.saturating_add(idle).saturating_add(iowait)))
}

/// `primary` is the deployment directory whose filesystem the disk figures
/// describe; with none, they are null.
pub fn snapshot(primary: Option<&Path>) -> Value {
    let status = read("/proc/self/status");
    let memory = read("/proc/meminfo");
    let io = read("/proc/self/io");
    let scheduler = read("/proc/self/schedstat");
    let process = read("/proc/self/stat");
    let cgroup_limit = read("/sys/fs/cgroup/memory.max").trim().parse::<u64>().ok();
    let cgroup_used = read("/sys/fs/cgroup/memory.current")
        .trim()
        .parse::<u64>()
        .ok();
    // The command name may contain spaces and parentheses; fields after its
    // last closing parenthesis start with the process state (field 3).
    let fields: Vec<_> = process
        .rsplit_once(')')
        .map(|(_, rest)| rest.split_whitespace().collect())
        .unwrap_or_default();
    let ticks = |index| {
        fields
            .get(index)
            .and_then(|value: &&str| value.parse::<u64>().ok())
    };
    // The statvfs pair is (total, available to an unprivileged writer).
    let disk = primary.and_then(|path| librepaper_base::util::disk_space(path).ok());
    let cpu = cpu_ticks(&read("/proc/stat"));
    json!({
        "rss_bytes":number(&status,"VmRSS").map(|n|n.saturating_mul(1024)),
        "peak_rss_bytes":number(&status,"VmHWM").map(|n|n.saturating_mul(1024)),
        "host_memory_bytes":number(&memory,"MemTotal").map(|n|n.saturating_mul(1024)),
        "host_available_memory_bytes":number(&memory,"MemAvailable").map(|n|n.saturating_mul(1024)),
        "cgroup_memory_limit_bytes":cgroup_limit,
        "cgroup_memory_used_bytes":cgroup_used,
        "cpu_user_clock_ticks":ticks(11),
        "cpu_system_clock_ticks":ticks(12),
        "main_thread_runtime_nanoseconds":scheduler.split_whitespace().next().and_then(|n|n.parse::<u64>().ok()),
        "disk_read_bytes":number(&io,"read_bytes"),
        "disk_write_bytes":number(&io,"write_bytes"),
        "disk_available_bytes":disk.map(|(_, available)| available),
        "disk_total_bytes":disk.map(|(total, _)| total),
        "host_cpu_busy_ticks":cpu.map(|(busy, _)| busy),
        "host_cpu_total_ticks":cpu.map(|(_, total)| total)
    })
}

pub fn warn(config: &librepaper_base::config::Configuration, primary: &Path) {
    if let Ok((_, free)) = librepaper_base::util::disk_space(primary) {
        if config.storage.total >= 0
            && config.storage.total as u64 > free.saturating_sub(256 * 1024 * 1024)
        {
            tracing::warn!("the storage ceiling leaves less than 256 MiB of currently available filesystem headroom");
        }
    }
    let measurements = snapshot(Some(primary));
    let memory = ["host_memory_bytes", "cgroup_memory_limit_bytes"]
        .iter()
        .filter_map(|name| measurements[*name].as_u64())
        .min();
    if let Some(memory) = memory {
        let decoded_docs = config.memory_budget_bytes;
        let pending_source = config.pending_bytes;
        let pending_scratch = config.pending_scratch_bytes;
        if decoded_docs
            .saturating_add(pending_source)
            .saturating_add(pending_scratch)
            > memory.saturating_mul(3) / 4
        {
            tracing::warn!("the decoded-document and pending-source memory ceilings exceed three quarters of detected host/container memory; leave space for the binary, proxy and operating system");
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_cpu_line_gives_busy_and_total_without_the_guest_columns() {
        let stat = "cpu  100 20 30 400 50 6 7 8 9 10\ncpu0 1 2 3 4 5 6 7 8 9 10\nintr 1\n";
        // busy = 100 + 20 + 30 + 6 + 7 + 8; total = busy + 400 + 50.
        assert_eq!(cpu_ticks(stat), Some((171, 621)));
        assert_eq!(cpu_ticks(""), None);
        assert_eq!(cpu_ticks("cpu0 1 2 3 4 5 6 7 8\n"), None);
        assert_eq!(cpu_ticks("cpu  1 2 3\n"), None);
        assert_eq!(cpu_ticks("cpu  1 2 3 four 5 6 7 8\n"), None);
    }

    #[test]
    fn without_a_directory_the_disk_fields_are_null() {
        let snapshot = snapshot(None);
        assert!(snapshot["disk_available_bytes"].is_null());
        assert!(snapshot["disk_total_bytes"].is_null());
    }

    #[cfg(unix)]
    #[test]
    fn the_disk_fields_are_numbers_for_a_directory() {
        let directory = tempfile::tempdir().unwrap();
        let snapshot = snapshot(Some(directory.path()));
        let total = snapshot["disk_total_bytes"].as_u64().unwrap();
        let available = snapshot["disk_available_bytes"].as_u64().unwrap();
        assert!(total > 0);
        assert!(available <= total);
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn the_cpu_fields_are_numbers_on_linux() {
        let snapshot = snapshot(None);
        let busy = snapshot["host_cpu_busy_ticks"].as_u64().unwrap();
        let total = snapshot["host_cpu_total_ticks"].as_u64().unwrap();
        assert!(busy <= total);
        assert!(total > 0);
    }
}
