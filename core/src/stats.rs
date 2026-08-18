//! Disk and network throughput sampling from /proc.

use std::collections::HashMap;
use std::fs;
use std::time::Instant;

pub struct DiskStats {
    pub name: String,
    pub read_bps: f64,
    pub write_bps: f64,
    /// Share of the interval the device spent doing I/O (Windows' "active time").
    pub busy_percent: f32,
}

pub struct NetStats {
    pub name: String,
    pub rx_bps: f64,
    pub tx_bps: f64,
}

#[derive(Clone, Copy)]
struct DiskCounters {
    read_sectors: u64,
    write_sectors: u64,
    io_ms: u64,
}

pub(crate) struct IoSampler {
    prev_disk: HashMap<String, DiskCounters>,
    prev_net: HashMap<String, (u64, u64)>,
    last: Option<Instant>,
}

impl IoSampler {
    pub(crate) fn new() -> Self {
        Self {
            prev_disk: HashMap::new(),
            prev_net: HashMap::new(),
            last: None,
        }
    }

    pub(crate) fn sample(&mut self) -> (Vec<DiskStats>, Vec<NetStats>) {
        let now = Instant::now();
        let elapsed = self
            .last
            .map(|t| now.duration_since(t).as_secs_f64())
            .unwrap_or(0.0);
        self.last = Some(now);
        let first = elapsed <= 0.0;
        let dt = elapsed.max(0.001);

        let mut disks = Vec::new();
        let mut next_disk = HashMap::new();
        if let Ok(text) = fs::read_to_string("/proc/diskstats") {
            for line in text.lines() {
                let t: Vec<&str> = line.split_ascii_whitespace().collect();
                if t.len() < 13 {
                    continue;
                }
                let name = t[2];
                if !is_physical_disk(name) {
                    continue;
                }
                let cur = DiskCounters {
                    read_sectors: t[5].parse().unwrap_or(0),
                    write_sectors: t[9].parse().unwrap_or(0),
                    io_ms: t[12].parse().unwrap_or(0),
                };
                let stats = match (first, self.prev_disk.get(name)) {
                    (false, Some(prev)) => DiskStats {
                        name: name.to_string(),
                        read_bps: (cur.read_sectors.saturating_sub(prev.read_sectors) * 512)
                            as f64
                            / dt,
                        write_bps: (cur.write_sectors.saturating_sub(prev.write_sectors) * 512)
                            as f64
                            / dt,
                        busy_percent: (cur.io_ms.saturating_sub(prev.io_ms) as f64
                            / (dt * 10.0))
                            .min(100.0) as f32,
                    },
                    _ => DiskStats {
                        name: name.to_string(),
                        read_bps: 0.0,
                        write_bps: 0.0,
                        busy_percent: 0.0,
                    },
                };
                disks.push(stats);
                next_disk.insert(name.to_string(), cur);
            }
        }

        let mut nets = Vec::new();
        let mut next_net = HashMap::new();
        if let Ok(text) = fs::read_to_string("/proc/net/dev") {
            for line in text.lines().skip(2) {
                let Some((name, rest)) = line.split_once(':') else {
                    continue;
                };
                let name = name.trim();
                if name == "lo" {
                    continue;
                }
                let t: Vec<&str> = rest.split_ascii_whitespace().collect();
                if t.len() < 16 {
                    continue;
                }
                let rx: u64 = t[0].parse().unwrap_or(0);
                let tx: u64 = t[8].parse().unwrap_or(0);
                let stats = match (first, self.prev_net.get(name)) {
                    (false, Some(&(prx, ptx))) => NetStats {
                        name: name.to_string(),
                        rx_bps: rx.saturating_sub(prx) as f64 / dt,
                        tx_bps: tx.saturating_sub(ptx) as f64 / dt,
                    },
                    _ => NetStats {
                        name: name.to_string(),
                        rx_bps: 0.0,
                        tx_bps: 0.0,
                    },
                };
                // Only show interfaces that have ever moved data.
                if rx > 0 || tx > 0 {
                    nets.push(stats);
                }
                next_net.insert(name.to_string(), (rx, tx));
            }
        }

        self.prev_disk = next_disk;
        self.prev_net = next_net;
        disks.sort_by(|a, b| a.name.cmp(&b.name));
        nets.sort_by(|a, b| a.name.cmp(&b.name));
        (disks, nets)
    }
}

/// Whole physical devices only (partitions, loop and ram devices excluded).
fn is_physical_disk(name: &str) -> bool {
    if let Some(rest) = name.strip_prefix("sd").or_else(|| name.strip_prefix("vd")) {
        return !rest.is_empty() && rest.chars().all(|c| c.is_ascii_alphabetic());
    }
    if name.starts_with("nvme") || name.starts_with("mmcblk") {
        return !name.contains('p');
    }
    false
}
