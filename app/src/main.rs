use std::cell::RefCell;
use std::collections::VecDeque;
use std::rc::Rc;
use std::time::Duration;

use slint::{ModelRc, SharedString, StandardListViewItem, Timer, TimerMode, VecModel};
use taskman_core::{Sampler, Snapshot};

slint::include_modules!();

/// Seconds of CPU history shown in the Performance graph.
const HISTORY: usize = 60;
/// Virtual coordinate space of the graph paths (matches the .slint viewbox).
const VIEW_W: f32 = 600.0;
const VIEW_H: f32 = 100.0;

struct AppState {
    sampler: Sampler,
    history: VecDeque<f32>,
    filter: String,
    processes: Vec<taskman_core::ProcessInfo>,
    /// pid for each currently visible table row (same order as the model).
    visible_pids: Vec<i32>,
}

fn main() -> Result<(), slint::PlatformError> {
    let app = MainWindow::new()?;
    let state = Rc::new(RefCell::new(AppState {
        sampler: Sampler::new(),
        history: VecDeque::with_capacity(HISTORY),
        filter: String::new(),
        processes: Vec::new(),
        visible_pids: Vec::new(),
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
        app.on_kill_requested(move |row| {
            let Some(app) = weak.upgrade() else { return };
            let st = state.borrow();
            let Some(&pid) = st.visible_pids.get(row as usize) else {
                return;
            };
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

    let mut procs = snap.processes;
    procs.sort_by(|a, b| {
        b.cpu_percent
            .total_cmp(&a.cpu_percent)
            .then(b.mem_bytes.cmp(&a.mem_bytes))
    });
    app.set_cpu_detail(format!("{} işlem • son {} sn", procs.len(), HISTORY).into());
    st.processes = procs;

    refresh_table(app, st);
}

fn refresh_table(app: &MainWindow, st: &mut AppState) {
    let mut rows: Vec<ModelRc<StandardListViewItem>> = Vec::new();
    let mut pids = Vec::new();
    for p in &st.processes {
        if !st.filter.is_empty()
            && !p.name.to_lowercase().contains(&st.filter)
            && !p.pid.to_string().contains(&st.filter)
        {
            continue;
        }
        rows.push(ModelRc::new(VecModel::from(vec![
            StandardListViewItem::from(SharedString::from(p.name.as_str())),
            StandardListViewItem::from(SharedString::from(p.pid.to_string().as_str())),
            StandardListViewItem::from(SharedString::from(
                format!("%{:.1}", p.cpu_percent).as_str(),
            )),
            StandardListViewItem::from(SharedString::from(fmt_bytes(p.mem_bytes).as_str())),
        ])));
        pids.push(p.pid);
    }
    st.visible_pids = pids;
    app.set_process_rows(ModelRc::new(VecModel::from(rows)));
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
