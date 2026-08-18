//! taskman-core: UI-independent data collection layer.
//!
//! Reads system and per-process metrics from /proc. Designed to be cheap
//! enough to call once per second.

use std::collections::HashMap;
use std::fs;

/// One process, as shown in the process list.
pub struct ProcessInfo {
    pub pid: i32,
    pub name: String,
    /// Percentage of total machine capacity (all cores = 100%), like Windows Task Manager.
    pub cpu_percent: f32,
    /// Resident set size in bytes (RSS; cheap but approximate - PSS comes later).
    pub mem_bytes: u64,
}

/// A single sampling pass over the whole system.
pub struct Snapshot {
    pub cpu_percent: f32,
    pub mem_total: u64,
    pub mem_used: u64,
    pub processes: Vec<ProcessInfo>,
}

#[derive(Clone, Copy)]
struct CpuTimes {
    total: u64,
    idle: u64,
}

/// Stateful sampler: keeps previous readings to compute deltas.
pub struct Sampler {
    prev_cpu: Option<CpuTimes>,
    /// pid -> utime+stime jiffies at the previous sample.
    prev_proc: HashMap<i32, u64>,
    page_size: u64,
}

impl Sampler {
    pub fn new() -> Self {
        Self {
            prev_cpu: None,
            prev_proc: HashMap::new(),
            page_size: page_size(),
        }
    }

    pub fn sample(&mut self) -> Snapshot {
        let cpu = read_cpu_times();
        let (cpu_percent, total_delta) = match (self.prev_cpu, cpu) {
            (Some(prev), Some(cur)) => {
                let dt = cur.total.saturating_sub(prev.total);
                let di = cur.idle.saturating_sub(prev.idle);
                let pct = if dt > 0 {
                    100.0 * dt.saturating_sub(di) as f32 / dt as f32
                } else {
                    0.0
                };
                (pct, dt)
            }
            _ => (0.0, 0),
        };
        if let Some(cur) = cpu {
            self.prev_cpu = Some(cur);
        }

        let (mem_total, mem_used) = read_meminfo();

        let mut next_prev = HashMap::with_capacity(self.prev_proc.len().max(64));
        let mut processes = Vec::with_capacity(self.prev_proc.len().max(64));
        if let Ok(dir) = fs::read_dir("/proc") {
            for entry in dir.flatten() {
                let fname = entry.file_name();
                let Some(pid) = fname.to_str().and_then(|s| s.parse::<i32>().ok()) else {
                    continue;
                };
                let Some(raw) = read_process(pid, self.page_size) else {
                    continue;
                };
                let cpu_p = match self.prev_proc.get(&pid) {
                    Some(&prev) if total_delta > 0 => {
                        100.0 * raw.jiffies.saturating_sub(prev) as f32 / total_delta as f32
                    }
                    _ => 0.0,
                };
                next_prev.insert(pid, raw.jiffies);
                processes.push(ProcessInfo {
                    pid,
                    name: raw.name,
                    cpu_percent: cpu_p,
                    mem_bytes: raw.mem_bytes,
                });
            }
        }
        self.prev_proc = next_prev;

        Snapshot {
            cpu_percent,
            mem_total,
            mem_used,
            processes,
        }
    }
}

impl Default for Sampler {
    fn default() -> Self {
        Self::new()
    }
}

/// Send SIGTERM: the graceful "End task" equivalent.
pub fn terminate(pid: i32) -> std::io::Result<()> {
    let ret = unsafe { libc::kill(pid, libc::SIGTERM) };
    if ret == 0 {
        Ok(())
    } else {
        Err(std::io::Error::last_os_error())
    }
}

struct RawProc {
    name: String,
    jiffies: u64,
    mem_bytes: u64,
}

fn read_process(pid: i32, page_size: u64) -> Option<RawProc> {
    let base = format!("/proc/{pid}");
    // Kernel threads have an empty cmdline; hide them like the Windows process view does.
    let cmdline = fs::read(format!("{base}/cmdline")).ok()?;
    if cmdline.is_empty() {
        return None;
    }
    let stat = fs::read_to_string(format!("{base}/stat")).ok()?;
    // comm may contain spaces and parentheses: parse around the *last* ')'.
    let open = stat.find('(')?;
    let close = stat.rfind(')')?;
    let name = stat.get(open + 1..close)?.to_string();
    let rest: Vec<&str> = stat.get(close + 1..)?.split_ascii_whitespace().collect();
    // Fields after comm: rest[0] is state (field 3), so utime (field 14) is rest[11].
    let utime: u64 = rest.get(11)?.parse().ok()?;
    let stime: u64 = rest.get(12)?.parse().ok()?;
    let statm = fs::read_to_string(format!("{base}/statm")).ok()?;
    let resident_pages: u64 = statm.split_ascii_whitespace().nth(1)?.parse().ok()?;
    Some(RawProc {
        name,
        jiffies: utime + stime,
        mem_bytes: resident_pages * page_size,
    })
}

fn read_cpu_times() -> Option<CpuTimes> {
    let stat = fs::read_to_string("/proc/stat").ok()?;
    let line = stat.lines().next()?;
    let mut fields = line.split_ascii_whitespace();
    if fields.next()? != "cpu" {
        return None;
    }
    let vals: Vec<u64> = fields.filter_map(|v| v.parse().ok()).collect();
    if vals.len() < 5 {
        return None;
    }
    // user nice system idle iowait irq softirq steal
    let idle = vals[3] + vals.get(4).copied().unwrap_or(0);
    let total: u64 = vals.iter().take(8).sum();
    Some(CpuTimes { total, idle })
}

fn read_meminfo() -> (u64, u64) {
    let mut total = 0u64;
    let mut avail = 0u64;
    if let Ok(text) = fs::read_to_string("/proc/meminfo") {
        for line in text.lines() {
            let mut fields = line.split_ascii_whitespace();
            match fields.next() {
                Some("MemTotal:") => {
                    total = fields.next().and_then(|v| v.parse().ok()).unwrap_or(0) * 1024
                }
                Some("MemAvailable:") => {
                    avail = fields.next().and_then(|v| v.parse().ok()).unwrap_or(0) * 1024
                }
                _ => {}
            }
        }
    }
    (total, total.saturating_sub(avail))
}

fn page_size() -> u64 {
    let ret = unsafe { libc::sysconf(libc::_SC_PAGESIZE) };
    if ret > 0 {
        ret as u64
    } else {
        4096
    }
}
