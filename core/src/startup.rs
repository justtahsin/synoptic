//! XDG autostart entries: list and enable/disable.
//!
//! Follows the freedesktop autostart spec: system entries in
//! /etc/xdg/autostart, user entries (which override system ones by file name)
//! in ~/.config/autostart. Disabling writes a user-level copy with
//! `Hidden=true`, the same approach desktop tweak tools use.

use std::collections::HashMap;
use std::fs;
use std::path::{Path, PathBuf};

pub struct StartupEntry {
    /// Desktop file name, e.g. "org.example.Tool.desktop". Stable identifier.
    pub id: String,
    pub name: String,
    pub exec: String,
    pub enabled: bool,
    /// true when the effective file lives in the user's config dir.
    pub user_level: bool,
}

fn autostart_dirs() -> (PathBuf, Option<PathBuf>) {
    let system = PathBuf::from("/etc/xdg/autostart");
    let user = std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".config/autostart"));
    (system, user)
}

fn desktop_files(dir: &Path) -> Vec<PathBuf> {
    let mut files = Vec::new();
    if let Ok(rd) = fs::read_dir(dir) {
        for entry in rd.flatten() {
            let path = entry.path();
            if path.extension().is_some_and(|e| e == "desktop") {
                files.push(path);
            }
        }
    }
    files
}

struct DesktopFile {
    name: String,
    exec: String,
    hidden: bool,
}

fn parse_desktop(path: &Path) -> Option<DesktopFile> {
    let text = fs::read_to_string(path).ok()?;
    // Prefer the localized Name[..] for the current locale.
    let lang = std::env::var("LANG").unwrap_or_default();
    let lang_full = lang.split('.').next().unwrap_or("");
    let lang_short = lang_full.split('_').next().unwrap_or("");
    let mut in_entry = false;
    let mut name = String::new();
    let mut name_full: Option<String> = None;
    let mut name_short: Option<String> = None;
    let mut exec = String::new();
    let mut hidden = false;
    for line in text.lines() {
        let line = line.trim();
        if line.starts_with('[') {
            in_entry = line == "[Desktop Entry]";
            continue;
        }
        if !in_entry {
            continue;
        }
        let Some((key, value)) = line.split_once('=') else {
            continue;
        };
        let (key, value) = (key.trim(), value.trim());
        if key == "Name" {
            name = value.to_string();
        } else if !lang_full.is_empty() && key == format!("Name[{lang_full}]") {
            name_full = Some(value.to_string());
        } else if !lang_short.is_empty() && key == format!("Name[{lang_short}]") {
            name_short = Some(value.to_string());
        } else if key == "Exec" {
            exec = value.to_string();
        } else if key == "Hidden" {
            hidden = value.eq_ignore_ascii_case("true");
        }
    }
    let display = name_full.or(name_short).unwrap_or(name);
    if display.is_empty() && exec.is_empty() {
        return None;
    }
    Some(DesktopFile {
        name: display,
        exec,
        hidden,
    })
}

pub fn list_startup() -> Vec<StartupEntry> {
    let (system, user) = autostart_dirs();
    let mut by_id: HashMap<String, (PathBuf, bool)> = HashMap::new();
    for path in desktop_files(&system) {
        if let Some(id) = path.file_name().and_then(|f| f.to_str()) {
            by_id.insert(id.to_string(), (path.clone(), false));
        }
    }
    if let Some(user_dir) = &user {
        for path in desktop_files(user_dir) {
            if let Some(id) = path.file_name().and_then(|f| f.to_str()) {
                by_id.insert(id.to_string(), (path.clone(), true));
            }
        }
    }
    let mut entries: Vec<StartupEntry> = by_id
        .into_iter()
        .filter_map(|(id, (path, user_level))| {
            let d = parse_desktop(&path)?;
            let name = if d.name.is_empty() {
                path.file_stem()?.to_string_lossy().into_owned()
            } else {
                d.name
            };
            Some(StartupEntry {
                id,
                name,
                exec: d.exec,
                enabled: !d.hidden,
                user_level,
            })
        })
        .collect();
    entries.sort_by(|a, b| a.name.to_lowercase().cmp(&b.name.to_lowercase()));
    entries
}

/// Enable/disable by managing the `Hidden` key in a user-level copy.
pub fn set_startup_enabled(id: &str, enable: bool) -> Result<(), String> {
    let (system, user) = autostart_dirs();
    let Some(user_dir) = user else {
        return Err("HOME is not set".into());
    };
    let user_path = user_dir.join(id);
    let source = if user_path.exists() {
        user_path.clone()
    } else {
        system.join(id)
    };
    let text = fs::read_to_string(&source).map_err(|e| e.to_string())?;
    let mut out: Vec<String> = Vec::new();
    for line in text.lines() {
        if line.trim_start().starts_with("Hidden=") {
            continue;
        }
        out.push(line.to_string());
        // Keys must live inside the [Desktop Entry] section.
        if !enable && line.trim() == "[Desktop Entry]" {
            out.push("Hidden=true".to_string());
        }
    }
    fs::create_dir_all(&user_dir).map_err(|e| e.to_string())?;
    fs::write(&user_path, out.join("\n") + "\n").map_err(|e| e.to_string())?;
    Ok(())
}
