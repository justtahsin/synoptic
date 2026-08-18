use std::cell::RefCell;
use std::collections::VecDeque;
use std::rc::Rc;
use std::time::Duration;

use slint::{ModelRc, SharedString, StandardListViewItem, Timer, TimerMode, VecModel};
use taskman_core::{Group, ProcessInfo, Sampler, Snapshot};

slint::include_modules!();

/// Seconds of CPU history shown in the Performance graph.
const HISTORY: usize = 60;
/// Virtual coordinate space of the graph paths (matches the .slint viewbox).
const VIEW_W: f32 = 600.0;
const VIEW_H: f32 = 100.0;

/// Table columns, by index: Ad, PID, CPU, Bellek.
const COL_NAME: i32 = 0;
const COL_PID: i32 = 1;
const COL_MEM: i32 = 3;

struct AppState {
    sampler: Sampler,
    history: VecDeque<f32>,
    filter: String,
    processes: Vec<ProcessInfo>,
    /// pid for each currently visible table row; -1 marks a group header row.
    visible_pids: Vec<i32>,
    /// Reused row model: updated in place instead of being recreated every tick.
    rows_model: Rc<VecModel<ModelRc<StandardListViewItem>>>,
    sort_col: i32,
    sort_asc: bool,
}

fn main() -> Result<(), slint::PlatformError> {
    let app = MainWindow::new()?;
    let rows_model = Rc::new(VecModel::default());
    app.set_process_rows(ModelRc::from(rows_model.clone()));

    let state = Rc::new(RefCell::new(AppState {
        sampler: Sampler::new(),
        history: VecDeque::with_capacity(HISTORY),
        filter: String::new(),
        processes: Vec::new(),
        visible_pids: Vec::new(),
        rows_model,
        sort_col: 2, // CPU
        sort_asc: false,
    }));

    {
        let state = state.clone();
        let weak = app.as_weak();
        app.on_search_edited(move |text| {
            let Some(app) = weak.upgrade() else { return };
            let mut st = state.borrow_mut();
            st.filter = text.trim().to_lowercase();
            refresh_table(&app, &mut st);
        });
    }

    {
        let state = state.clone();
        let weak = app.as_weak();
        app.on_sort_ascending(move |col| {
            let Some(app) = weak.upgrade() else { return };
            let mut st = state.borrow_mut();
            st.sort_col = col;
            st.sort_asc = true;
            refresh_table(&app, &mut st);
        });
    }

    {
        let state = state.clone();
        let weak = app.as_weak();
        app.on_sort_descending(move |col| {
            let Some(app) = weak.upgrade() else { return };
            let mut st = state.borrow_mut();
            st.sort_col = col;
            st.sort_asc = false;
            refresh_table(&app, &mut st);
        });
    }

    {
        let state = state.clone();
        let weak = app.as_weak();
        app.on_kill_requested(move |row| {
            let Some(app) = weak.upgrade() else { return };
            let st = state.borrow();
            let Some(&pid) = st.visible_pids.get(row as usize) else {
                return;
            };
            if pid <= 0 {
                app.set_status_text("Bir işlem satırı seç (grup başlığı sonlandırılamaz)".into());
                return;
            }
            let msg = match taskman_core::terminate(pid) {
                Ok(()) => format!("SIGTERM gönderildi (PID {pid})"),
                Err(err) => format!("Sonlandırılamadı (PID {pid}): {err}"),
            };
            app.set_status_text(msg.into());
        });
    }

    // Prime the UI so the window is not empty for the first second.
    {
        let mut st = state.borrow_mut();
        let snap = st.sampler.sample();
        apply_snapshot(&app, &mut st, snap);
    }

    let timer = Timer::default();
    {
        let state = state.clone();
        let weak = app.as_weak();
        timer.start(TimerMode::Repeated, Duration::from_millis(1000), move || {
            let Some(app) = weak.upgrade() else { return };
            let mut st = state.borrow_mut();
            let snap = st.sampler.sample();
            apply_snapshot(&app, &mut st, snap);
        });
    }

    app.run()
}

fn apply_snapshot(app: &MainWindow, st: &mut AppState, snap: Snapshot) {
    if st.history.len() >= HISTORY {
        st.history.pop_front();
    }
    st.history.push_back(snap.cpu_percent);

    let (line, fill) = cpu_paths(&st.history);
    app.set_cpu_line(line.into());
    app.set_cpu_fill(fill.into());
    app.set_cpu_text(format!("%{:.0}", snap.cpu_percent).into());
    app.set_cpu_detail(
        format!(
            "{} işlem • {} çekirdek • son {} sn",
            snap.processes.len(),
            snap.per_core.len(),
            HISTORY
        )
        .into(),
    );
    app.set_core_loads(ModelRc::new(VecModel::from(snap.per_core.clone())));

    let frac = if snap.mem_total > 0 {
        snap.mem_used as f32 / snap.mem_total as f32
    } else {
        0.0
    };
    app.set_mem_fraction(frac);
    app.set_mem_text(
        format!(
            "{} / {} kullanımda (%{:.0})",
            fmt_bytes(snap.mem_used),
            fmt_bytes(snap.mem_total),
            100.0 * frac
        )
        .into(),
    );

    st.processes = snap.processes;
    refresh_table(app, st);
}

fn refresh_table(app: &MainWindow, st: &mut AppState) {
    // Remember the selection by PID so it survives re-sorting, like Windows TM.
    let selected_pid = usize::try_from(app.get_selected_row())
        .ok()
        .and_then(|row| st.visible_pids.get(row).copied())
        .filter(|&pid| pid > 0);

    let mut groups: [Vec<&ProcessInfo>; 3] = [Vec::new(), Vec::new(), Vec::new()];
    for p in &st.processes {
        if !st.filter.is_empty()
            && !p.name.to_lowercase().contains(&st.filter)
            && !p.pid.to_string().contains(&st.filter)
        {
            continue;
        }
        let idx = match p.group {
            Group::App => 0,
            Group::Background => 1,
            Group::System => 2,
        };
        groups[idx].push(p);
    }

    let (col, asc) = (st.sort_col, st.sort_asc);
    for g in &mut groups {
        g.sort_by(|a, b| {
            let ord = match col {
                COL_NAME => a.name.to_lowercase().cmp(&b.name.to_lowercase()),
                COL_PID => a.pid.cmp(&b.pid),
                COL_MEM => a.mem_bytes.cmp(&b.mem_bytes),
                _ => a.cpu_percent.total_cmp(&b.cpu_percent),
            };
            if asc {
                ord
            } else {
                ord.reverse()
            }
        });
    }

    const TITLES: [&str; 3] = ["Uygulamalar", "Arka plan işlemleri", "Sistem işlemleri"];
    let mut rows: Vec<ModelRc<StandardListViewItem>> = Vec::new();
    let mut pids: Vec<i32> = Vec::new();
    for (gi, g) in groups.iter().enumerate() {
        if g.is_empty() {
            continue;
        }
        rows.push(header_row(&format!("{} ({})", TITLES[gi], g.len())));
        pids.push(-1);
        for p in g {
            rows.push(process_row(p));
            pids.push(p.pid);
        }
    }
    st.visible_pids = pids;
    st.rows_model.set_vec(rows);

    // Restore selection to the same process if it is still visible.
    let new_row = selected_pid
        .and_then(|pid| st.visible_pids.iter().position(|&p| p == pid))
        .map(|i| i as i32)
        .unwrap_or(-1);
    app.set_selected_row(new_row);
}

fn cell(text: &str) -> StandardListViewItem {
    StandardListViewItem::from(SharedString::from(text))
}

fn header_row(title: &str) -> ModelRc<StandardListViewItem> {
    ModelRc::new(VecModel::from(vec![cell(title), cell(""), cell(""), cell("")]))
}

fn process_row(p: &ProcessInfo) -> ModelRc<StandardListViewItem> {
    ModelRc::new(VecModel::from(vec![
        cell(&format!("    {}", p.name)),
        cell(&p.pid.to_string()),
        cell(&format!("%{:.1}", p.cpu_percent)),
        cell(&fmt_bytes(p.mem_bytes)),
    ]))
}

fn cpu_paths(history: &VecDeque<f32>) -> (String, String) {
    let n = history.len();
    if n < 2 {
        return (String::new(), String::new());
    }
    let step = VIEW_W / (HISTORY as f32 - 1.0);
    let left = VIEW_W - (n as f32 - 1.0) * step;
    let mut line = String::new();
    for (i, v) in history.iter().enumerate() {
        let x = left + i as f32 * step;
        let y = (VIEW_H - v.clamp(0.0, 100.0) * VIEW_H / 100.0).clamp(0.0, VIEW_H);
        if i == 0 {
            line.push_str(&format!("M {x:.1} {y:.1} "));
        } else {
            line.push_str(&format!("L {x:.1} {y:.1} "));
        }
    }
    let fill = format!("{line}L {VIEW_W} {VIEW_H} L {left:.1} {VIEW_H} Z");
    (line, fill)
}

fn fmt_bytes(bytes: u64) -> String {
    const MB: f64 = 1024.0 * 1024.0;
    const GB: f64 = 1024.0 * MB;
    let b = bytes as f64;
    if b >= GB {
        format!("{:.1} GB", b / GB)
    } else {
        format!("{:.1} MB", b / MB)
    }
}
