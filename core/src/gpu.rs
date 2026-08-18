//! GPU sampling via DRM sysfs.
//!
//! amdgpu exposes everything we need (busy %, VRAM, temperature). Other
//! drivers get best-effort: fields are `None` when the kernel does not
//! provide them. NVIDIA's proprietary driver needs NVML and is on the
//! roadmap; such cards still appear by name.

use std::collections::HashMap;
use std::fs;
use std::path::{Path, PathBuf};

pub struct GpuStats {
    /// DRM node id, e.g. "card1". Stable while the card is present.
    pub id: String,
    /// Marketing name from lspci when available, driver name otherwise.
    pub name: String,
    pub busy_percent: Option<f32>,
    pub vram_used: Option<u64>,
    pub vram_total: Option<u64>,
    pub temp_c: Option<f32>,
}

pub(crate) struct GpuSampler {
    cards: Vec<Card>,
}

struct Card {
    id: String,
    device: PathBuf,
    name: String,
    temp_path: Option<PathBuf>,
}

impl GpuSampler {
    pub(crate) fn new() -> Self {
        let pci_names = lspci_names();
        let mut cards = Vec::new();
        if let Ok(rd) = fs::read_dir("/sys/class/drm") {
            for entry in rd.flatten() {
                let id = entry.file_name().to_string_lossy().into_owned();
                // Whole cards only ("card0"), not connectors ("card0-DP-1").
                if !id.starts_with("card") || id[4..].parse::<u32>().is_err() {
                    continue;
                }
                let device = entry.path().join("device");
                if !device.exists() {
                    continue;
                }
                let uevent = fs::read_to_string(device.join("uevent")).unwrap_or_default();
                let mut driver = "";
                let mut slot = "";
                for line in uevent.lines() {
                    if let Some(v) = line.strip_prefix("DRIVER=") {
                        driver = v;
                    } else if let Some(v) = line.strip_prefix("PCI_SLOT_NAME=") {
                        slot = v;
                    }
                }
                let short_slot = slot.strip_prefix("0000:").unwrap_or(slot);
                let name = pci_names.get(short_slot).cloned().unwrap_or_else(|| {
                    if driver.is_empty() {
                        id.clone()
                    } else {
                        format!("GPU ({driver})")
                    }
                });
                let temp_path = find_temp(&device);
                cards.push(Card {
                    id,
                    device,
                    name,
                    temp_path,
                });
            }
        }
        cards.sort_by(|a, b| a.id.cmp(&b.id));
        Self { cards }
    }

    pub(crate) fn sample(&self) -> Vec<GpuStats> {
        self.cards
            .iter()
            .map(|c| GpuStats {
                id: c.id.clone(),
                name: c.name.clone(),
                busy_percent: read_u64(&c.device.join("gpu_busy_percent"))
                    .map(|v| v.min(100) as f32),
                vram_used: read_u64(&c.device.join("mem_info_vram_used")),
                vram_total: read_u64(&c.device.join("mem_info_vram_total")),
                temp_c: c
                    .temp_path
                    .as_ref()
                    .and_then(|p| read_u64(p))
                    .map(|v| v as f32 / 1000.0),
            })
            .collect()
    }
}

fn read_u64(path: &Path) -> Option<u64> {
    fs::read_to_string(path).ok()?.trim().parse().ok()
}

fn find_temp(device: &Path) -> Option<PathBuf> {
    if let Ok(rd) = fs::read_dir(device.join("hwmon")) {
        for entry in rd.flatten() {
            let p = entry.path().join("temp1_input");
            if p.exists() {
                return Some(p);
            }
        }
    }
    None
}

/// Marketing names by PCI slot via `lspci -mm` (best effort, runs once).
fn lspci_names() -> HashMap<String, String> {
    let mut map = HashMap::new();
    let Ok(out) = std::process::Command::new("lspci").arg("-mm").output() else {
        return map;
    };
    for line in String::from_utf8_lossy(&out.stdout).lines() {
        let fields = parse_lspci_line(line);
        if fields.len() >= 4 {
            let class = &fields[1];
            if class.contains("VGA") || class.contains("3D") || class.contains("Display") {
                map.insert(fields[0].clone(), clean_gpu_name(&fields[3]));
            }
        }
    }
    map
}

/// Split an `lspci -mm` line into fields, respecting double quotes.
fn parse_lspci_line(line: &str) -> Vec<String> {
    let mut fields = Vec::new();
    let mut cur = String::new();
    let mut in_quotes = false;
    for ch in line.chars() {
        match ch {
            '"' => in_quotes = !in_quotes,
            ' ' if !in_quotes => {
                if !cur.is_empty() {
                    fields.push(std::mem::take(&mut cur));
                }
            }
            _ => cur.push(ch),
        }
    }
    if !cur.is_empty() {
        fields.push(cur);
    }
    fields
}

/// "Navi 24 [Radeon RX 6400/6500 XT/6500M]" -> "Radeon RX 6400/6500 XT/6500M"
fn clean_gpu_name(name: &str) -> String {
    if let (Some(open), Some(close)) = (name.find('['), name.rfind(']')) {
        if open < close {
            return name[open + 1..close].to_string();
        }
    }
    name.to_string()
}
