use std::cell::RefCell;
use std::cmp::Reverse;
use std::collections::{HashMap, VecDeque};
use std::rc::Rc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use slint::{Color, ModelRc, SharedString, StandardListViewItem, Timer, TimerMode, VecModel};
use synoptic_core::{
    DiskStats, GpuStats, Group, NetStats, ProcessInfo, Sampler, ServiceAction, ServiceInfo,
    Snapshot, StartupEntry,
};

slint::include_modules!();

/// Reverse-DNS application id: Wayland app_id and X11 WM_CLASS.
const APP_ID: &str = "io.github.justtahsin.Synoptic";

/// Seconds of history shown in the Performance graphs.
const HISTORY: usize = 60;
/// Virtual coordinate space of the graph paths (matches the .slint viewbox).
const VIEW_W: f32 = 600.0;
/// Mini card thumb viewbox height (matches the 86x46 thumb aspect).
const MINI_VBH: f32 = 321.0;
/// Refresh the service list every N sampling ticks.
const SERVICE_REFRESH_TICKS: u32 = 5;
/// Minimum network graph scale so idle noise does not fill the graph.
const MIN_NET_SCALE: f32 = 50.0 * 1024.0;

const BLUE: Color = Color::from_rgb_u8(0x00, 0x67, 0xC0);
const PURPLE: Color = Color::from_rgb_u8(0x9A, 0x48, 0xD0);
const GREEN: Color = Color::from_rgb_u8(0x12, 0x85, 0x5F);
const ORANGE: Color = Color::from_rgb_u8(0xC4, 0x6A, 0x00);
const CYAN: Color = Color::from_rgb_u8(0x00, 0x99, 0xBC);

/// Set by worker threads after a service action so the next tick re-lists.
static REFRESH_SERVICES: AtomicBool = AtomicBool::new(false);

enum CardKey {
    Cpu,
    Mem,
    Disk(String),
    Net(String),
    Gpu(String),
}

struct AppState {
    sampler: Sampler,
    users: HashMap<u32, String>,
    processes: Vec<ProcessInfo>,
    tick: u32,

    // Performans
    hist_cpu: VecDeque<f32>,
    hist_mem: VecDeque<f32>,
    hist_disk: HashMap<String, VecDeque<f32>>,
    hist_net_rx: HashMap<String, VecDeque<f32>>,
    hist_net_tx: HashMap<String, VecDeque<f32>>,
    hist_gpu: HashMap<String, VecDeque<f32>>,
    last_cpu: f32,
    last_cores: usize,
    last_proc_count: usize,
    last_mem_total: u64,
    last_mem_used: u64,
    last_disks: Vec<DiskStats>,
    last_nets: Vec<NetStats>,
    last_gpus: Vec<GpuStats>,
    card_keys: Vec<CardKey>,
    perf_model: Rc<VecModel<PerfCard>>,

    // İşlemler (grouped view)
    filter: String,
    /// pid for each visible row; -1 marks a group header row.
    visible_pids: Vec<i32>,
    rows_model: Rc<VecModel<ModelRc<StandardListViewItem>>>,
    sort_col: i32,
    sort_asc: bool,

    // Kullanıcılar
    u_rows_model: Rc<VecModel<ModelRc<StandardListViewItem>>>,

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
    // Install a winit backend that stamps our app id on every window, so
    // taskbars, icons and compositor window rules can match the app.
    let backend = i_slint_backend_winit::Backend::builder()
        .with_window_attributes_hook(|attrs| {
            let attrs = i_slint_backend_winit::winit::platform::wayland::WindowAttributesExtWayland::with_name(attrs, APP_ID, APP_ID);
            i_slint_backend_winit::winit::platform::x11::WindowAttributesExtX11::with_name(attrs, APP_ID, APP_ID)
        })
        .build()?;
    slint::platform::set_platform(Box::new(backend))
        .expect("set_platform must be called before the first window is created");

    let app = MainWindow::new()?;

    // Test aid: `--page N` opens directly on the given page.
    let args: Vec<String> = std::env::args().collect();
    if let Some(i) = args.iter().position(|a| a == "--page") {
        if let Some(n) = args.get(i + 1).and_then(|v| v.parse::<i32>().ok()) {
            app.set_page(n.clamp(0, 5));
        }
    }

    let rows_model = Rc::new(VecModel::default());
    app.set_process_rows(ModelRc::from(rows_model.clone()));
    let b_rows_model = Rc::new(VecModel::default());
    app.set_startup_rows(ModelRc::from(b_rows_model.clone()));
    let d_rows_model = Rc::new(VecModel::default());
    app.set_detail_rows(ModelRc::from(d_rows_model.clone()));
    let s_rows_model = Rc::new(VecModel::default());
    app.set_service_rows(ModelRc::from(s_rows_model.clone()));
    let perf_model = Rc::new(VecModel::default());
    app.set_perf_cards(ModelRc::from(perf_model.clone()));
    let u_rows_model = Rc::new(VecModel::default());
    app.set_user_rows(ModelRc::from(u_rows_model.clone()));

    let state = Rc::new(RefCell::new(AppState {
        sampler: Sampler::new(),
        users: synoptic_core::load_users(),
        processes: Vec::new(),
        tick: 0,
        hist_cpu: VecDeque::with_capacity(HISTORY),
        hist_mem: VecDeque::with_capacity(HISTORY),
        hist_disk: HashMap::new(),
        hist_net_rx: HashMap::new(),
        hist_net_tx: HashMap::new(),
        hist_gpu: HashMap::new(),
        last_cpu: 0.0,
        last_cores: 0,
        last_proc_count: 0,
        last_mem_total: 0,
        last_mem_used: 0,
        last_disks: Vec::new(),
        last_nets: Vec::new(),
        last_gpus: Vec::new(),
        card_keys: Vec::new(),
        perf_model,
        u_rows_model,
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

    // --- Performans callbacks ---
    {
        let state = state.clone();
        let weak = app.as_weak();
        app.on_perf_select(move |idx| {
            let Some(app) = weak.upgrade() else { return };
            app.set_perf_selected(idx);
            let mut st = state.borrow_mut();
            rebuild_perf(&app, &mut st);
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
        st.services = synoptic_core::list_services();
        refresh_services_table(&app, &mut st);
        st.startup = synoptic_core::list_startup();
        refresh_startup_table(&app, &mut st);
    }

    let timer = Timer::default();
    {
        let state = state.clone();
        let weak = app.as_weak();
        timer.start(
            TimerMode::Repeated,
            Duration::from_millis(1000),
            move || {
                let Some(app) = weak.upgrade() else { return };
                let mut st = state.borrow_mut();
                let snap = st.sampler.sample();
                apply_snapshot(&app, &mut st, snap);
                st.tick = st.tick.wrapping_add(1);
                if st.tick.is_multiple_of(SERVICE_REFRESH_TICKS)
                    || REFRESH_SERVICES.swap(false, Ordering::Relaxed)
                {
                    st.services = synoptic_core::list_services();
                    refresh_services_table(&app, &mut st);
                }
            },
        );
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
        "term" => signal_result("SIGTERM", synoptic_core::terminate(pid)),
        "kill" => signal_result("SIGKILL", synoptic_core::force_kill(pid)),
        "stop" => signal_result("SIGSTOP (dondur)", synoptic_core::stop_process(pid)),
        "cont" => signal_result("SIGCONT (devam)", synoptic_core::continue_process(pid)),
        "nice-down" => match synoptic_core::set_nice_delta(pid, 5) {
            Ok(nice) => format!("Öncelik düşürüldü (nice {nice}, PID {pid})"),
            Err(err) => format!("Öncelik değiştirilemedi (PID {pid}): {err}"),
        },
        "nice-up" => match synoptic_core::set_nice_delta(pid, -5) {
            Ok(nice) => format!("Öncelik yükseltildi (nice {nice}, PID {pid})"),
            Err(err) => {
                format!("Öncelik yükseltilemedi (PID {pid}): {err} — yükseltmek genelde root ister")
            }
        },
        "open" => match synoptic_core::exe_dir(pid) {
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
    let Some(id) = usize::try_from(row)
        .ok()
        .and_then(|r| st.b_ids.get(r))
        .cloned()
    else {
        return;
    };
    let msg = match synoptic_core::set_startup_enabled(&id, enable) {
        Ok(()) => format!(
            "{id}: {}",
            if enable {
                "etkinleştirildi"
            } else {
                "devre dışı bırakıldı"
            }
        ),
        Err(err) => format!("{id}: yapılamadı — {err}"),
    };
    st.startup = synoptic_core::list_startup();
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
    app.set_service_status(
        format!("{name}: istek gönderildi, yetki gerekiyorsa sorulacak…").into(),
    );
    let weak = app.as_weak();
    // Blocking systemctl call (may wait on a polkit prompt) goes to a worker thread.
    std::thread::spawn(move || {
        let result = synoptic_core::service_action(action, &name);
        REFRESH_SERVICES.store(true, Ordering::Relaxed);
        let msg = match result {
            Ok(()) => format!("{name}: işlem tamamlandı"),
            Err(err) => format!("{name}: {err}"),
        };
        let _ = weak.upgrade_in_event_loop(move |app| app.set_service_status(msg.into()));
    });
}

fn push_capped(series: &mut VecDeque<f32>, value: f32) {
    if series.len() >= HISTORY {
        series.pop_front();
    }
    series.push_back(value);
}

fn apply_snapshot(app: &MainWindow, st: &mut AppState, snap: Snapshot) {
    let Snapshot {
        cpu_percent,
        per_core,
        mem_total,
        mem_used,
        disks,
        nets,
        gpus,
        processes,
    } = snap;

    st.last_cpu = cpu_percent;
    st.last_cores = per_core.len();
    st.last_proc_count = processes.iter().filter(|p| !p.kernel).count();
    push_capped(&mut st.hist_cpu, cpu_percent);

    let frac = if mem_total > 0 {
        mem_used as f32 / mem_total as f32
    } else {
        0.0
    };
    st.last_mem_total = mem_total;
    st.last_mem_used = mem_used;
    push_capped(&mut st.hist_mem, 100.0 * frac);
    app.set_mem_fraction(frac);
    app.set_core_loads(ModelRc::new(VecModel::from(per_core)));

    st.hist_disk
        .retain(|k, _| disks.iter().any(|d| &d.name == k));
    for d in &disks {
        push_capped(
            st.hist_disk.entry(d.name.clone()).or_default(),
            d.busy_percent,
        );
    }
    st.hist_net_rx
        .retain(|k, _| nets.iter().any(|n| &n.name == k));
    st.hist_net_tx
        .retain(|k, _| nets.iter().any(|n| &n.name == k));
    for n in &nets {
        push_capped(
            st.hist_net_rx.entry(n.name.clone()).or_default(),
            n.rx_bps as f32,
        );
        push_capped(
            st.hist_net_tx.entry(n.name.clone()).or_default(),
            n.tx_bps as f32,
        );
    }
    st.last_disks = disks;
    st.last_nets = nets;

    st.hist_gpu.retain(|k, _| gpus.iter().any(|g| &g.id == k));
    for g in &gpus {
        let value = g
            .busy_percent
            .or_else(|| match (g.vram_used, g.vram_total) {
                (Some(u), Some(t)) if t > 0 => Some(100.0 * u as f32 / t as f32),
                _ => None,
            })
            .unwrap_or(0.0);
        push_capped(st.hist_gpu.entry(g.id.clone()).or_default(), value);
    }
    st.last_gpus = gpus;

    st.processes = processes;
    refresh_table(app, st);
    refresh_details(app, st);
    refresh_users_table(app, st);
    rebuild_perf(app, st);
}

/// Rebuild the Performance page: resource cards on the left, selected detail on the right.
fn rebuild_perf(app: &MainWindow, st: &mut AppState) {
    let mut cards: Vec<PerfCard> = Vec::new();
    let mut keys: Vec<CardKey> = Vec::new();

    cards.push(PerfCard {
        title: "CPU".into(),
        value: format!("%{:.0}", st.last_cpu).into(),
        line: series_paths(&st.hist_cpu, 100.0, MINI_VBH).0.into(),
        color: BLUE,
    });
    keys.push(CardKey::Cpu);

    let mem_pct = st.hist_mem.back().copied().unwrap_or(0.0);
    cards.push(PerfCard {
        title: "Bellek".into(),
        value: format!(
            "{}/{} (%{:.0})",
            fmt_bytes(st.last_mem_used),
            fmt_bytes(st.last_mem_total),
            mem_pct
        )
        .into(),
        line: series_paths(&st.hist_mem, 100.0, MINI_VBH).0.into(),
        color: PURPLE,
    });
    keys.push(CardKey::Mem);

    for d in &st.last_disks {
        let line = st
            .hist_disk
            .get(&d.name)
            .map(|h| series_paths(h, 100.0, MINI_VBH).0)
            .unwrap_or_default();
        cards.push(PerfCard {
            title: format!("Disk ({})", d.name).into(),
            value: format!("%{:.0}", d.busy_percent).into(),
            line: line.into(),
            color: GREEN,
        });
        keys.push(CardKey::Disk(d.name.clone()));
    }

    for n in &st.last_nets {
        let combined: VecDeque<f32> =
            match (st.hist_net_rx.get(&n.name), st.hist_net_tx.get(&n.name)) {
                (Some(rx), Some(tx)) => rx.iter().zip(tx.iter()).map(|(a, b)| a + b).collect(),
                _ => VecDeque::new(),
            };
        let scale = combined.iter().copied().fold(MIN_NET_SCALE, f32::max);
        cards.push(PerfCard {
            title: format!("Ağ ({})", n.name).into(),
            value: fmt_rate(n.rx_bps + n.tx_bps).into(),
            line: series_paths(&combined, scale, MINI_VBH).0.into(),
            color: ORANGE,
        });
        keys.push(CardKey::Net(n.name.clone()));
    }

    let gpu_count = st.last_gpus.len();
    for (i, g) in st.last_gpus.iter().enumerate() {
        let line = st
            .hist_gpu
            .get(&g.id)
            .map(|h| series_paths(h, 100.0, MINI_VBH).0)
            .unwrap_or_default();
        let mut value = match g.busy_percent {
            Some(b) => format!("%{b:.0}"),
            None => "—".to_string(),
        };
        if let Some(t) = g.temp_c {
            value.push_str(&format!(" • {t:.0}°C"));
        }
        cards.push(PerfCard {
            title: if gpu_count > 1 {
                format!("GPU {i}").into()
            } else {
                "GPU".into()
            },
            value: value.into(),
            line: line.into(),
            color: CYAN,
        });
        keys.push(CardKey::Gpu(g.id.clone()));
    }

    let count = cards.len() as i32;
    st.card_keys = keys;
    st.perf_model.set_vec(cards);
    let sel = app.get_perf_selected().clamp(0, (count - 1).max(0));
    app.set_perf_selected(sel);

    let gw = app.get_graph_w();
    let gh = app.get_graph_h();
    let vbh = if gw > 1.0 && gh > 1.0 {
        VIEW_W * gh / gw
    } else {
        150.0
    };
    app.set_perf_vbh(vbh);

    let empty = SharedString::default();
    match st.card_keys.get(sel as usize) {
        Some(CardKey::Cpu) => {
            let (line, fill) = series_paths(&st.hist_cpu, 100.0, vbh);
            app.set_perf_title("CPU".into());
            app.set_perf_value(format!("%{:.0}", st.last_cpu).into());
            app.set_perf_sub1(
                format!("{} işlem • {} çekirdek", st.last_proc_count, st.last_cores).into(),
            );
            app.set_perf_sub2(
                format!("Kullanım (%) • son {HISTORY} sn • çekirdek başına yük aşağıda").into(),
            );
            app.set_perf_line(line.into());
            app.set_perf_fill(fill.into());
            app.set_perf_line2(empty);
            app.set_perf_color(BLUE);
        }
        Some(CardKey::Mem) => {
            let (line, fill) = series_paths(&st.hist_mem, 100.0, vbh);
            app.set_perf_title("Bellek".into());
            app.set_perf_value(
                format!("%{:.0}", st.hist_mem.back().copied().unwrap_or(0.0)).into(),
            );
            app.set_perf_sub1(
                format!(
                    "{} / {} kullanımda",
                    fmt_bytes(st.last_mem_used),
                    fmt_bytes(st.last_mem_total)
                )
                .into(),
            );
            app.set_perf_sub2(format!("Kullanım (%) • son {HISTORY} sn").into());
            app.set_perf_line(line.into());
            app.set_perf_fill(fill.into());
            app.set_perf_line2(empty);
            app.set_perf_color(PURPLE);
        }
        Some(CardKey::Disk(name)) => {
            let hist = st.hist_disk.get(name);
            let (line, fill) = hist
                .map(|h| series_paths(h, 100.0, vbh))
                .unwrap_or_default();
            let d = st.last_disks.iter().find(|d| &d.name == name);
            app.set_perf_title(format!("Disk ({name})").into());
            app.set_perf_value(format!("%{:.0}", d.map(|d| d.busy_percent).unwrap_or(0.0)).into());
            app.set_perf_sub1(
                format!(
                    "Okuma {} • Yazma {}",
                    fmt_rate(d.map(|d| d.read_bps).unwrap_or(0.0)),
                    fmt_rate(d.map(|d| d.write_bps).unwrap_or(0.0))
                )
                .into(),
            );
            app.set_perf_sub2(format!("Etkin süre (%) • son {HISTORY} sn").into());
            app.set_perf_line(line.into());
            app.set_perf_fill(fill.into());
            app.set_perf_line2(empty);
            app.set_perf_color(GREEN);
        }
        Some(CardKey::Net(name)) => {
            let rx = st.hist_net_rx.get(name);
            let tx = st.hist_net_tx.get(name);
            let scale = rx
                .into_iter()
                .chain(tx)
                .flat_map(|h| h.iter().copied())
                .fold(MIN_NET_SCALE, f32::max);
            let (line, fill) = rx.map(|h| series_paths(h, scale, vbh)).unwrap_or_default();
            let (line2, _) = tx.map(|h| series_paths(h, scale, vbh)).unwrap_or_default();
            let n = st.last_nets.iter().find(|n| &n.name == name);
            app.set_perf_title(format!("Ağ ({name})").into());
            app.set_perf_value(fmt_rate(n.map(|n| n.rx_bps + n.tx_bps).unwrap_or(0.0)).into());
            app.set_perf_sub1(
                format!(
                    "Alma {} • Gönderme {} (soluk çizgi)",
                    fmt_rate(n.map(|n| n.rx_bps).unwrap_or(0.0)),
                    fmt_rate(n.map(|n| n.tx_bps).unwrap_or(0.0))
                )
                .into(),
            );
            app.set_perf_sub2(
                format!("Ölçek: {} • son {HISTORY} sn", fmt_rate(scale as f64)).into(),
            );
            app.set_perf_line(line.into());
            app.set_perf_fill(fill.into());
            app.set_perf_line2(line2.into());
            app.set_perf_color(ORANGE);
        }
        Some(CardKey::Gpu(id)) => {
            let (line, fill) = st
                .hist_gpu
                .get(id)
                .map(|h| series_paths(h, 100.0, vbh))
                .unwrap_or_default();
            let g = st.last_gpus.iter().find(|g| &g.id == id);
            let name = g.map(|g| g.name.as_str()).unwrap_or("GPU");
            app.set_perf_title(format!("GPU ({name})").into());
            app.set_perf_value(
                match g.and_then(|g| g.busy_percent) {
                    Some(b) => format!("%{b:.0}"),
                    None => "—".to_string(),
                }
                .into(),
            );
            let vram = match (g.and_then(|g| g.vram_used), g.and_then(|g| g.vram_total)) {
                (Some(u), Some(t)) => format!("VRAM {} / {}", fmt_bytes(u), fmt_bytes(t)),
                _ => "VRAM bilgisi yok".to_string(),
            };
            let temp = match g.and_then(|g| g.temp_c) {
                Some(t) => format!(" • {t:.0}°C"),
                None => String::new(),
            };
            app.set_perf_sub1(format!("{vram}{temp}").into());
            app.set_perf_sub2(format!("Kullanım (%) • son {HISTORY} sn").into());
            app.set_perf_line(line.into());
            app.set_perf_fill(fill.into());
            app.set_perf_line2(empty);
            app.set_perf_color(CYAN);
        }
        None => {}
    }
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
            cell(if e.user_level {
                "Kullanıcı"
            } else {
                "Sistem"
            }),
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

/// Aggregate per-user resource usage, Windows "Users" style.
fn refresh_users_table(_app: &MainWindow, st: &mut AppState) {
    struct Agg {
        count: usize,
        cpu: f32,
        mem: u64,
    }
    let mut by_uid: HashMap<u32, Agg> = HashMap::new();
    for p in &st.processes {
        let a = by_uid.entry(p.uid).or_insert(Agg {
            count: 0,
            cpu: 0.0,
            mem: 0,
        });
        a.count += 1;
        a.cpu += p.cpu_percent;
        a.mem += p.mem_bytes;
    }
    let mut list: Vec<(u32, Agg)> = by_uid.into_iter().collect();
    list.sort_by_key(|(_, a)| Reverse(a.mem));
    let rows: Vec<ModelRc<StandardListViewItem>> = list
        .iter()
        .map(|(uid, a)| {
            let name = st
                .users
                .get(uid)
                .cloned()
                .unwrap_or_else(|| uid.to_string());
            ModelRc::new(VecModel::from(vec![
                cell(&name),
                cell(&a.count.to_string()),
                cell(&format!("%{:.1}", a.cpu)),
                cell(&fmt_bytes(a.mem)),
            ]))
        })
        .collect();
    st.u_rows_model.set_vec(rows);
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
    ModelRc::new(VecModel::from(vec![
        cell(title),
        cell(""),
        cell(""),
        cell(""),
    ]))
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

/// Build SVG path commands for a series scaled to `max` (line, filled area).
/// `view_h` is the viewbox height the target Path element uses.
fn series_paths(history: &VecDeque<f32>, max: f32, view_h: f32) -> (String, String) {
    let n = history.len();
    if n < 2 || max <= 0.0 {
        return (String::new(), String::new());
    }
    let step = VIEW_W / (HISTORY as f32 - 1.0);
    let left = VIEW_W - (n as f32 - 1.0) * step;
    let mut line = String::new();
    for (i, v) in history.iter().enumerate() {
        let x = left + i as f32 * step;
        let y = (view_h - (v / max).clamp(0.0, 1.0) * view_h).clamp(0.0, view_h);
        if i == 0 {
            line.push_str(&format!("M {x:.1} {y:.1} "));
        } else {
            line.push_str(&format!("L {x:.1} {y:.1} "));
        }
    }
    let fill = format!("{line}L {VIEW_W} {view_h:.1} L {left:.1} {view_h:.1} Z");
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

fn fmt_rate(bps: f64) -> String {
    const KB: f64 = 1024.0;
    const MB: f64 = KB * 1024.0;
    const GB: f64 = MB * 1024.0;
    if bps >= GB {
        format!("{:.1} GB/s", bps / GB)
    } else if bps >= MB {
        format!("{:.1} MB/s", bps / MB)
    } else {
        format!("{:.0} KB/s", bps / KB)
    }
}
