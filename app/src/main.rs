use std::cell::RefCell;
use std::collections::{HashMap, VecDeque};
use std::rc::Rc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use slint::{ModelRc, SharedString, StandardListViewItem, Timer, TimerMode, VecModel};
use taskman_core::{
    Group, ProcessInfo, Sampler, ServiceAction, ServiceInfo, Snapshot, StartupEntry,
};

slint::include_modules!();

/// Seconds of CPU history shown in the Performance graph.
const HISTORY: usize = 60;
/// Virtual coordinate space of the graph paths (matches the .slint viewbox).
const VIEW_W: f32 = 600.0;
const VIEW_H: f32 = 100.0;
/// Refresh the service list every N sampling ticks.
const SERVICE_REFRESH_TICKS: u32 = 5;

/// Set by worker threads after a service action so the next tick re-lists.
static REFRESH_SERVICES: AtomicBool = AtomicBool::new(false);

struct AppState {
    sampler: Sampler,
    history: VecDeque<f32>,
    users: HashMap<u32, String>,
    processes: Vec<ProcessInfo>,
    tick: u32,

    // İşlemler (grouped view)
    filter: String,
    /// pid for each visible row; -1 marks a group header row.
    visible_pids: Vec<i32>,
    rows_model: Rc<VecModel<ModelRc<StandardListViewItem>>>,
    sort_col: i32,
    sort_asc: bool,

    // Başlangıç
    startup: Vec<StartupEntry>,
    b_ids: Vec<String>,
    b_rows_model: Rc<VecModel<ModelRc<StandardListViewItem>>>,

    // Ayrıntılar (flat view, includes kernel threads and other users)
    d_filter: String,
    d_pids: Vec<i32>,
    d_rows_model: Rc<VecModel<ModelRc<StandardListViewItem>>>,
    d_sort_col: i32,
    d_sort_asc: bool,

    // Hizmetler
    services: Vec<ServiceInfo>,
    s_filter: String,
    s_names: Vec<String>,
    s_rows_model: Rc<VecModel<ModelRc<StandardListViewItem>>>,
}

fn main() -> Result<(), slint::PlatformError> {
    let app = MainWindow::new()?;

    let rows_model = Rc::new(VecModel::default());
    app.set_process_rows(ModelRc::from(rows_model.clone()));
    let b_rows_model = Rc::new(VecModel::default());
    app.set_startup_rows(ModelRc::from(b_rows_model.clone()));
    let d_rows_model = Rc::new(VecModel::default());
    app.set_detail_rows(ModelRc::from(d_rows_model.clone()));
    let s_rows_model = Rc::new(VecModel::default());
    app.set_service_rows(ModelRc::from(s_rows_model.clone()));

    let state = Rc::new(RefCell::new(AppState {
        sampler: Sampler::new(),
        history: VecDeque::with_capacity(HISTORY),
        users: taskman_core::load_users(),
        processes: Vec::new(),
        tick: 0,
        filter: String::new(),
        visible_pids: Vec::new(),
        rows_model,
        sort_col: 2, // CPU
        sort_asc: false,
        startup: Vec::new(),
        b_ids: Vec::new(),
        b_rows_model,
        d_filter: String::new(),
        d_pids: Vec::new(),
        d_rows_model,
        d_sort_col: 4, // CPU
        d_sort_asc: false,
        services: Vec::new(),
        s_filter: String::new(),
        s_names: Vec::new(),
        s_rows_model,
    }));

    // --- İşlemler callbacks ---
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
        app.on_process_action(move |action, row| {
            let Some(app) = weak.upgrade() else { return };
            let st = state.borrow();
            let Some(&pid) = st.visible_pids.get(row as usize) else {
                return;
            };
            if pid <= 0 {
                app.set_status_text("Bir işlem satırı seç (grup başlığına uygulanamaz)".into());
                return;
            }
            app.set_status_text(do_process_action(pid, &action).into());
        });
    }

    // --- Başlangıç callbacks ---
    {
        let state = state.clone();
        let weak = app.as_weak();
        app.on_startup_enable(move |row| {
            let Some(app) = weak.upgrade() else { return };
            let mut st = state.borrow_mut();
            toggle_startup(&app, &mut st, row, true);
        });
    }
    {
        let state = state.clone();
        let weak = app.as_weak();
        app.on_startup_disable(move |row| {
            let Some(app) = weak.upgrade() else { return };
            let mut st = state.borrow_mut();
            toggle_startup(&app, &mut st, row, false);
        });
    }

    // --- Ayrıntılar callbacks ---
    {
        let state = state.clone();
        let weak = app.as_weak();
        app.on_detail_search_edited(move |text| {
            let Some(app) = weak.upgrade() else { return };
            let mut st = state.borrow_mut();
            st.d_filter = text.trim().to_lowercase();
            refresh_details(&app, &mut st);
        });
    }
    {
        let state = state.clone();
        let weak = app.as_weak();
        app.on_detail_sort_ascending(move |col| {
            let Some(app) = weak.upgrade() else { return };
            let mut st = state.borrow_mut();
            st.d_sort_col = col;
            st.d_sort_asc = true;
            refresh_details(&app, &mut st);
        });
    }
    {
        let state = state.clone();
        let weak = app.as_weak();
        app.on_detail_sort_descending(move |col| {
            let Some(app) = weak.upgrade() else { return };
            let mut st = state.borrow_mut();
            st.d_sort_col = col;
            st.d_sort_asc = false;
            refresh_details(&app, &mut st);
        });
    }
    {
        let state = state.clone();
        let weak = app.as_weak();
        app.on_detail_action(move |action, row| {
            let Some(app) = weak.upgrade() else { return };
            let st = state.borrow();
            let Some(&pid) = st.d_pids.get(row as usize) else {
                return;
            };
            app.set_detail_status(do_process_action(pid, &action).into());
        });
    }

    // --- Hizmetler callbacks ---
    {
        let state = state.clone();
        let weak = app.as_weak();
        app.on_service_search_edited(move |text| {
            let Some(app) = weak.upgrade() else { return };
            let mut st = state.borrow_mut();
            st.s_filter = text.trim().to_lowercase();
            refresh_services_table(&app, &mut st);
        });
    }
    for (action, which) in [
        (ServiceAction::Start, 0),
        (ServiceAction::Stop, 1),
        (ServiceAction::Restart, 2),
    ] {
        let state = state.clone();
        let weak = app.as_weak();
        let handler = move |row: i32| {
            let Some(app) = weak.upgrade() else { return };
            let st = state.borrow();
            run_service_action(&app, &st, row, action);
        };
        match which {
            0 => app.on_service_start(handler),
            1 => app.on_service_stop(handler),
            _ => app.on_service_restart(handler),
        }
    }

    // Prime the UI so the window is not empty for the first second.
    {
        let mut st = state.borrow_mut();
        let snap = st.sampler.sample();
        apply_snapshot(&app, &mut st, snap);
        st.services = taskman_core::list_services();
        refresh_services_table(&app, &mut st);
        st.startup = taskman_core::list_startup();
        refresh_startup_table(&app, &mut st);
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
            st.tick = st.tick.wrapping_add(1);
            if st.tick % SERVICE_REFRESH_TICKS == 0
                || REFRESH_SERVICES.swap(false, Ordering::Relaxed)
            {
                st.services = taskman_core::list_services();
                refresh_services_table(&app, &mut st);
            }
        });
    }

    app.run()
}

/// Shared dispatcher for the context-menu / button actions on a process.
fn do_process_action(pid: i32, action: &str) -> String {
    let signal_result = |name: &str, r: std::io::Result<()>| match r {
        Ok(()) => format!("{name} gönderildi (PID {pid})"),
        Err(err) => format!("Yapılamadı (PID {pid}): {err}"),
    };
    match action {
        "term" => signal_result("SIGTERM", taskman_core::terminate(pid)),
        "kill" => signal_result("SIGKILL", taskman_core::force_kill(pid)),
        "stop" => signal_result("SIGSTOP (dondur)", taskman_core::stop_process(pid)),
        "cont" => signal_result("SIGCONT (devam)", taskman_core::continue_process(pid)),
        "nice-down" => match taskman_core::set_nice_delta(pid, 5) {
            Ok(nice) => format!("Öncelik düşürüldü (nice {nice}, PID {pid})"),
            Err(err) => format!("Öncelik değiştirilemedi (PID {pid}): {err}"),
        },
        "nice-up" => match taskman_core::set_nice_delta(pid, -5) {
            Ok(nice) => format!("Öncelik yükseltildi (nice {nice}, PID {pid})"),
            Err(err) => format!(
                "Öncelik yükseltilemedi (PID {pid}): {err} — yükseltmek genelde root ister"
            ),
        },
        "open" => match taskman_core::exe_dir(pid) {
            Some(dir) => {
                let _ = std::process::Command::new("xdg-open").arg(&dir).spawn();
                format!("Açılıyor: {}", dir.display())
            }
            None => format!("Konum okunamadı (PID {pid}) — başka kullanıcının süreci olabilir"),
        },
        _ => String::new(),
    }
}

fn toggle_startup(app: &MainWindow, st: &mut AppState, row: i32, enable: bool) {
    let Some(id) = usize::try_from(row).ok().and_then(|r| st.b_ids.get(r)).cloned() else {
        return;
    };
    let msg = match taskman_core::set_startup_enabled(&id, enable) {
        Ok(()) => format!(
            "{id}: {}",
            if enable { "etkinleştirildi" } else { "devre dışı bırakıldı" }
        ),
        Err(err) => format!("{id}: yapılamadı — {err}"),
    };
    st.startup = taskman_core::list_startup();
    refresh_startup_table(app, st);
    app.set_startup_status(msg.into());
}

fn run_service_action(app: &MainWindow, st: &AppState, row: i32, action: ServiceAction) {
    let Some(name) = usize::try_from(row)
        .ok()
        .and_then(|r| st.s_names.get(r))
        .cloned()
    else {
        return;
    };
    app.set_service_status(format!("{name}: istek gönderildi, yetki gerekiyorsa sorulacak…").into());
    let weak = app.as_weak();
    // Blocking systemctl call (may wait on a polkit prompt) goes to a worker thread.
    std::thread::spawn(move || {
        let result = taskman_core::service_action(action, &name);
        REFRESH_SERVICES.store(true, Ordering::Relaxed);
        let msg = match result {
            Ok(()) => format!("{name}: işlem tamamlandı"),
            Err(err) => format!("{name}: {err}"),
        };
        let _ = weak.upgrade_in_event_loop(move |app| app.set_service_status(msg.into()));
    });
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
    refresh_details(app, st);
}

fn refresh_table(app: &MainWindow, st: &mut AppState) {
    // Remember the selection by PID so it survives re-sorting, like Windows TM.
    let selected_pid = usize::try_from(app.get_selected_row())
        .ok()
        .and_then(|row| st.visible_pids.get(row).copied())
        .filter(|&pid| pid > 0);

    let mut groups: [Vec<&ProcessInfo>; 3] = [Vec::new(), Vec::new(), Vec::new()];
    for p in &st.processes {
        if p.kernel {
            continue;
        }
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
                0 => a.name.to_lowercase().cmp(&b.name.to_lowercase()),
                1 => a.pid.cmp(&b.pid),
                3 => a.mem_bytes.cmp(&b.mem_bytes),
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
            rows.push(ModelRc::new(VecModel::from(vec![
                cell(&format!("    {}", p.name)),
                cell(&p.pid.to_string()),
                cell(&format!("%{:.1}", p.cpu_percent)),
                cell(&fmt_bytes(p.mem_bytes)),
            ])));
            pids.push(p.pid);
        }
    }
    st.visible_pids = pids;
    st.rows_model.set_vec(rows);

    let new_row = selected_pid
        .and_then(|pid| st.visible_pids.iter().position(|&p| p == pid))
        .map(|i| i as i32)
        .unwrap_or(-1);
    app.set_selected_row(new_row);
}

fn refresh_startup_table(app: &MainWindow, st: &mut AppState) {
    let selected_id = usize::try_from(app.get_startup_selected_row())
        .ok()
        .and_then(|row| st.b_ids.get(row).cloned());

    let mut rows: Vec<ModelRc<StandardListViewItem>> = Vec::new();
    let mut ids: Vec<String> = Vec::new();
    for e in &st.startup {
        rows.push(ModelRc::new(VecModel::from(vec![
            cell(&e.name),
            cell(if e.enabled { "Etkin" } else { "Devre dışı" }),
            cell(&e.exec),
            cell(if e.user_level { "Kullanıcı" } else { "Sistem" }),
        ])));
        ids.push(e.id.clone());
    }
    st.b_ids = ids;
    st.b_rows_model.set_vec(rows);

    let new_row = selected_id
        .and_then(|id| st.b_ids.iter().position(|i| *i == id))
        .map(|i| i as i32)
        .unwrap_or(-1);
    app.set_startup_selected_row(new_row);
}

fn refresh_details(app: &MainWindow, st: &mut AppState) {
    let selected_pid = usize::try_from(app.get_detail_selected_row())
        .ok()
        .and_then(|row| st.d_pids.get(row).copied());

    let users = &st.users;
    let mut list: Vec<&ProcessInfo> = st
        .processes
        .iter()
        .filter(|p| {
            st.d_filter.is_empty()
                || p.name.to_lowercase().contains(&st.d_filter)
                || p.pid.to_string().contains(&st.d_filter)
        })
        .collect();

    let (col, asc) = (st.d_sort_col, st.d_sort_asc);
    list.sort_by(|a, b| {
        let ord = match col {
            0 => a.pid.cmp(&b.pid),
            1 => a.name.to_lowercase().cmp(&b.name.to_lowercase()),
            2 => a.state.cmp(&b.state),
            3 => users
                .get(&a.uid)
                .cmp(&users.get(&b.uid))
                .then(a.uid.cmp(&b.uid)),
            5 => a.mem_bytes.cmp(&b.mem_bytes),
            _ => a.cpu_percent.total_cmp(&b.cpu_percent),
        };
        if asc {
            ord
        } else {
            ord.reverse()
        }
    });

    let mut rows: Vec<ModelRc<StandardListViewItem>> = Vec::with_capacity(list.len());
    let mut pids: Vec<i32> = Vec::with_capacity(list.len());
    for p in &list {
        let user = users
            .get(&p.uid)
            .cloned()
            .unwrap_or_else(|| p.uid.to_string());
        rows.push(ModelRc::new(VecModel::from(vec![
            cell(&p.pid.to_string()),
            cell(&p.name),
            cell(state_label(p.state)),
            cell(&user),
            cell(&format!("%{:.1}", p.cpu_percent)),
            cell(&fmt_bytes(p.mem_bytes)),
        ])));
        pids.push(p.pid);
    }
    st.d_pids = pids;
    st.d_rows_model.set_vec(rows);

    let new_row = selected_pid
        .and_then(|pid| st.d_pids.iter().position(|&p| p == pid))
        .map(|i| i as i32)
        .unwrap_or(-1);
    app.set_detail_selected_row(new_row);
}

fn refresh_services_table(app: &MainWindow, st: &mut AppState) {
    let selected_name = usize::try_from(app.get_service_selected_row())
        .ok()
        .and_then(|row| st.s_names.get(row).cloned());

    let mut rows: Vec<ModelRc<StandardListViewItem>> = Vec::new();
    let mut names: Vec<String> = Vec::new();
    for s in &st.services {
        if !st.s_filter.is_empty() && !s.name.to_lowercase().contains(&st.s_filter) {
            continue;
        }
        rows.push(ModelRc::new(VecModel::from(vec![
            cell(&s.name),
            cell(&s.active),
            cell(&s.sub),
            cell(&s.description),
        ])));
        names.push(s.name.clone());
    }
    st.s_names = names;
    st.s_rows_model.set_vec(rows);

    let new_row = selected_name
        .and_then(|name| st.s_names.iter().position(|n| *n == name))
        .map(|i| i as i32)
        .unwrap_or(-1);
    app.set_service_selected_row(new_row);
}

fn cell(text: &str) -> StandardListViewItem {
    StandardListViewItem::from(SharedString::from(text))
}

fn header_row(title: &str) -> ModelRc<StandardListViewItem> {
    ModelRc::new(VecModel::from(vec![cell(title), cell(""), cell(""), cell("")]))
}

fn state_label(state: char) -> &'static str {
    match state {
        'R' => "Çalışıyor",
        'S' => "Bekliyor",
        'D' => "Disk G/Ç",
        'Z' => "Zombi",
        'T' | 't' => "Durduruldu",
        'I' => "Boşta",
        'X' => "Ölü",
        _ => "-",
    }
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
