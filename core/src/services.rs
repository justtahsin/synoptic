//! systemd service listing and control.
//!
//! PoC backend shells out to `systemctl`: listing is fast (<20ms) and actions
//! get interactive polkit authorization for free (the desktop agent prompts).
//! A native D-Bus (zbus) backend can replace this behind the same API later.

use std::process::Command;

pub struct ServiceInfo {
    pub name: String,
    pub load: String,
    pub active: String,
    pub sub: String,
    pub description: String,
}

#[derive(Clone, Copy)]
pub enum ServiceAction {
    Start,
    Stop,
    Restart,
}

pub fn list_services() -> Vec<ServiceInfo> {
    let out = Command::new("systemctl")
        .args([
            "list-units",
            "--type=service",
            "--all",
            "--no-legend",
            "--no-pager",
            "--plain",
            "--full",
        ])
        .output();
    let Ok(out) = out else { return Vec::new() };
    let text = String::from_utf8_lossy(&out.stdout);
    let mut services = Vec::new();
    for line in text.lines() {
        let mut tokens = line.split_ascii_whitespace();
        // Failed units carry a leading status marker token; skip until the unit name.
        let name = loop {
            match tokens.next() {
                Some(t) if t.ends_with(".service") => break t.to_string(),
                Some(_) => continue,
                None => break String::new(),
            }
        };
        if name.is_empty() {
            continue;
        }
        let load = tokens.next().unwrap_or("").to_string();
        let active = tokens.next().unwrap_or("").to_string();
        let sub = tokens.next().unwrap_or("").to_string();
        let description = tokens.collect::<Vec<_>>().join(" ");
        services.push(ServiceInfo {
            name,
            load,
            active,
            sub,
            description,
        });
    }
    services.sort_by(|a, b| a.name.cmp(&b.name));
    services
}

/// Blocking: run from a worker thread. May wait on a polkit password prompt.
pub fn service_action(action: ServiceAction, unit: &str) -> Result<(), String> {
    let verb = match action {
        ServiceAction::Start => "start",
        ServiceAction::Stop => "stop",
        ServiceAction::Restart => "restart",
    };
    match Command::new("systemctl").args([verb, unit]).output() {
        Ok(out) if out.status.success() => Ok(()),
        Ok(out) => Err(String::from_utf8_lossy(&out.stderr)
            .lines()
            .next()
            .unwrap_or("unknown error")
            .to_string()),
        Err(err) => Err(err.to_string()),
    }
}
