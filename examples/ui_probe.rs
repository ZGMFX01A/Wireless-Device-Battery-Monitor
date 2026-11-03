//! Offline native UI regression; launches only --gui-child / --exercise-tray.
//! Run manually on an interactive Windows desktop, never as a hardware test.
#[path = "probe/controls.rs"]
mod controls;
#[path = "probe/native.rs"]
mod native;
use native::*;
use serde_json::{Value, json};
use std::{
    fs,
    io::{BufRead, BufReader, Write},
    path::{Path, PathBuf},
    process::{Child, ChildStdin, Command, Stdio},
    sync::mpsc::{self, Receiver},
    thread,
    time::{Duration, Instant},
};
use windows_sys::Win32::Foundation::HWND;

struct Process {
    child: Child,
    input: ChildStdin,
    lines: Receiver<String>,
    exit: &'static str,
    owned: Vec<ProcessHandle>,
}
impl Process {
    fn new(exe: &Path, tray: bool) -> Result<Self> {
        let mut child = Command::new(exe)
            .arg(if tray {
                "--exercise-tray"
            } else {
                "--gui-child"
            })
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::inherit())
            .spawn()?;
        let input = child.stdin.take().ok_or("missing stdin")?;
        let output = child.stdout.take().ok_or("missing stdout")?;
        let (tx, lines) = mpsc::channel();
        thread::spawn(move || {
            for line in BufReader::new(output)
                .lines()
                .map_while(std::result::Result::ok)
            {
                if tx.send(line).is_err() {
                    break;
                }
            }
        });
        Ok(Self {
            child,
            input,
            lines,
            exit: if tray { "quit" } else { "\"Exit\"" },
            owned: Vec::new(),
        })
    }
    fn line(&mut self, value: &str) -> Result<()> {
        writeln!(self.input, "{value}")?;
        self.input.flush()?;
        Ok(())
    }
    fn emit(&mut self, value: Value) -> Result<()> {
        self.line(&value.to_string())
    }
    fn expect(&self, seconds: u64, accept: impl Fn(&str) -> bool) -> Result<String> {
        let end = Instant::now() + Duration::from_secs(seconds);
        loop {
            let line = self
                .lines
                .recv_timeout(end.saturating_duration_since(Instant::now()))?;
            if accept(&line) {
                return Ok(line);
            }
        }
    }
    fn json(&self, accept: impl Fn(&Value) -> bool) -> Result<Value> {
        let line = self.expect(15, |s| serde_json::from_str(s).is_ok_and(|v| accept(&v)))?;
        Ok(serde_json::from_str(&line)?)
    }
    fn pid(&mut self, exe: &Path) -> Result<u32> {
        let line = self.expect(12, |s| s.starts_with("GUI_PID "))?;
        let pid = line
            .split_whitespace()
            .nth(1)
            .ok_or("no child PID")?
            .parse()?;
        self.owned.push(ProcessHandle::own(pid, exe)?);
        Ok(pid)
    }
    fn close(&mut self) -> Result<()> {
        self.line(self.exit)?;
        let status = wait(7, || self.child.try_wait().ok().flatten())?;
        require(status.success(), "test process returned failure")
    }
}
impl Drop for Process {
    fn drop(&mut self) {
        if self.child.try_wait().ok().flatten().is_none() {
            let _ = self.line(self.exit);
            if wait(3, || self.child.try_wait().ok().flatten()).is_err() {
                let _ = self.child.kill();
                let _ = self.child.wait();
            }
        }
        for h in &self.owned {
            if !h.exited() {
                let _ = h.terminate();
            }
        }
    }
}

fn fixture() -> Value {
    let values = [
        (
            "hid:046d:1:Mouse:test",
            "G PRO X SUPERLIGHT",
            "Mouse",
            72,
            "Discharging",
            json!({"Hidpp":4100}),
        ),
        (
            "hid:0b05:2:Keyboard:test",
            "ROG Azoth",
            "Keyboard",
            35,
            "Charging",
            json!("Rog"),
        ),
        (
            "bt:test",
            "Bluetooth Headset",
            "Bluetooth",
            12,
            "Unknown",
            json!("Classic"),
        ),
    ];
    let rows:Vec<Value>=values.into_iter().map(|(id,name,kind,pct,charge,source)|json!({"device":{"id":id,"name":name,"kind":kind},"battery":{"raw":pct,"display":pct,"charge":charge,"source":source,"mv":null},"sampled":0,"error":null})).collect();
    let mut candidates: Vec<Value> = rows.iter().map(|r| r["device"].clone()).collect();
    candidates.push(json!({"id":"spare","name":"NUT75","kind":"Keyboard"}));
    json!({"prefs":{"theme":"Dark","lang":"Zh","keyboard":rows[1]["device"]["id"],"bluetooth":[rows[2]["device"]["id"]]},"rows":rows,"candidates":candidates,"note":"","demo":true,"autostart":false,"update":"Idle"})
}
struct Ui {
    p: Process,
    state: Value,
    generation: u64,
    hwnd: HWND,
    out: PathBuf,
}
impl Ui {
    fn new(exe: &Path, out: &Path) -> Result<Self> {
        let p = Process::new(exe, false)?;
        p.json(|v| v == "Ready")?;
        Ok(Self {
            p,
            state: fixture(),
            generation: 0,
            hwnd: std::ptr::null_mut(),
            out: out.to_owned(),
        })
    }
    fn update(&mut self) -> Result<()> {
        self.p.emit(json!({"State":self.state}))?;
        pause(120);
        Ok(())
    }
    fn open(&mut self, mode: &str) -> Result<()> {
        self.generation += 1;
        self.p.emit(json!({"Open":[mode,self.generation,null]}))?;
        self.p
            .json(|v| v == &json!({"Presented":self.generation}))?;
        self.hwnd = screen(self.p.child.id(), true)?;
        pause(220);
        let (_, r) = bounds(self.hwnd)?;
        let s = scale(self.hwnd);
        let expected = if mode == "Settings" { 540. } else { 380. };
        require(
            (f64::from(r.right) / s - expected).abs() < 2.,
            "client width mismatch",
        )
    }
    fn photo(&self, name: &str) -> Result<image::RgbImage> {
        capture(self.hwnd, &self.out.join(format!("{name}.png")))
    }
    fn click(&self, x: f64, y: f64) -> Result<()> {
        click(self.hwnd, x, y)
    }
    fn exact(&self, x: f64, y: f64, value: Value) -> Result<()> {
        self.click(x, y)?;
        self.p.json(|v| v == &value)?;
        Ok(())
    }
    fn primary(&self, name: &str) -> Result<()> {
        native::primary(self.hwnd, &self.out.join(format!("{name}.png")))
    }
    fn key(&self, v: usize) -> Result<()> {
        native::key(self.hwnd, v)
    }
}
fn window(exe: &Path) -> Result<Value> {
    let mut p = Process::new(exe, false)?;
    p.json(|v| v == "Ready")?;
    p.emit(json!({"Hide":0}))?;
    p.json(|v| v == &json!({"Hidden":0}))?;
    p.emit(json!({"Open":["Settings",1,null]}))?;
    p.json(|v| v == &json!({"Presented":1}))?;
    let first = screen(p.child.id(), true)?;
    let mut times = Vec::new();
    for g in 1..=100 {
        p.emit(json!({"Hide":g}))?;
        p.json(|v| v == &json!({"Hidden":g}))?;
        screen(p.child.id(), false)?;
        let start = Instant::now();
        p.emit(json!({"Open":[if g%2==0{"Flyout"}else{"Settings"},g+1,null]}))?;
        p.json(|v| v == &json!({"Presented":g+1}))?;
        require(
            screen(p.child.id(), true)? == first,
            "window was not reused",
        )?;
        times.push(start.elapsed().as_millis());
    }
    p.close()?;
    Ok(
        json!({"ok":true,"presents":101,"hides":100,"same_window":true,"warm_open_ms":times,"hardware":false}),
    )
}
fn tray(exe: &Path) -> Result<Value> {
    let mut p = Process::new(exe, true)?;
    p.line("flyout")?;
    let first = p.pid(exe)?;
    p.expect(12, |s| s.starts_with("PRESENTED "))?;
    screen(first, true)?;
    p.line("flyout")?;
    screen(first, false)?;
    p.line("settings")?;
    p.expect(12, |s| s.starts_with("PRESENTED "))?;
    screen(first, true)?;
    p.line("flyout")?;
    p.expect(12, |s| s.starts_with("PRESENTED "))?;
    p.line("flyout")?;
    screen(first, false)?;
    let start = Instant::now();
    wait(40, || p.owned[0].exited().then_some(()))?;
    let idle = start.elapsed().as_secs_f64();
    require(
        p.child.try_wait()?.is_none(),
        "tray exited during GUI reclaim",
    )?;
    let start = Instant::now();
    p.line("settings")?;
    let second = p.pid(exe)?;
    p.expect(12, |s| s.starts_with("PRESENTED "))?;
    screen(second, true)?;
    let cold = start.elapsed().as_millis();
    p.owned[1].terminate()?;
    pause(250);
    p.line("settings")?;
    let third = p.pid(exe)?;
    p.expect(12, |s| s.starts_with("PRESENTED "))?;
    screen(third, true)?;
    p.close()?;
    Ok(
        json!({"ok":true,"hardware":false,"idle_exit_s":idle,"cold_reopen_ms":cold,"crash_reopened":true}),
    )
}
fn hung(exe: &Path) -> Result<Value> {
    let mut p = Process::new(exe, true)?;
    p.line("flyout")?;
    let pid = p.pid(exe)?;
    p.expect(12, |s| s.starts_with("PRESENTED "))?;
    p.line("flyout")?;
    screen(pid, false)?;
    pause(400);
    p.owned[0].freeze()?;
    pause(30150);
    require(!p.owned[0].exited(), "frozen process unexpectedly exited")?;
    let start = Instant::now();
    p.line("settings")?;
    let next = p.pid(exe)?;
    p.expect(12, |s| s.starts_with("PRESENTED "))?;
    screen(next, true)?;
    require(p.owned[0].exited(), "frozen GUI was not reclaimed")?;
    p.close()?;
    Ok(json!({"ok":true,"hardware":false,"reopen_elapsed_s":start.elapsed().as_secs_f64()}))
}
fn visual(exe: &Path, out: &Path) -> Result<Value> {
    let mut ui = Ui::new(exe, out)?;
    let mut results = serde_json::Map::new();
    for (theme, lang) in [("Dark", "Zh"), ("Light", "Zh"), ("Dark", "En")] {
        for mode in ["Flyout", "Settings"] {
            ui.state = fixture();
            ui.state["prefs"]["theme"] = json!(theme);
            ui.state["prefs"]["lang"] = json!(lang);
            ui.update()?;
            ui.open(mode)?;
            let name = format!("{theme}-{lang}-{mode}");
            let img = ui.photo(&name)?;
            let expected = if mode == "Flyout" {
                (380, 540)
            } else {
                (540, 680)
            };
            require(
                img.dimensions() == expected,
                "incorrect screenshot dimensions",
            )?;
            results.insert(
                name,
                json!({"logical":[img.width(),img.height()],"dpi":scale(ui.hwnd)*96.}),
            );
        }
    }
    for theme in ["Light", "Dark"] {
        ui.state = fixture();
        ui.state["prefs"]["theme"] = json!(theme);
        ui.state["rows"][0]["battery"]["raw"] = json!(5);
        ui.state["rows"][0]["battery"]["display"] = json!(5);
        ui.state["rows"][2]["battery"] = Value::Null;
        ui.state["rows"][2]["error"] = json!("offline");
        ui.update()?;
        ui.open("Flyout")?;
        ui.photo(&format!("{theme}-states"))?;
    }
    ui.p.close()?;
    Ok(json!({"ok":true,"cases":results,"state_screenshots":2}))
}
fn glass(exe: &Path, out: &Path) -> Result<Value> {
    let mut ui = Ui::new(exe, out)?;
    ui.state["rows"] = json!([]);
    ui.update()?;
    ui.open("Flyout")?;
    let board = Board::new(ui.hwnd)?;
    let (hr, material) = board.material();
    let mut samples = serde_json::Map::new();
    let mut full = 0f64;
    for n in [0, 25, 100] {
        ui.state["prefs"]["glass"] = json!(n);
        ui.update()?;
        let red = board.samples(0x2323dd)?;
        let blue = board.samples(0xdd2323)?;
        full = ((red[0] - red[2]) - (blue[0] - blue[2])).abs();
        let response: f64 = red.iter().zip(blue).map(|(a, b)| (a - b).abs()).sum();
        samples.insert(
            n.to_string(),
            json!({"red":red,"blue":blue,"response":response}),
        );
    }
    let ratio = board.stripes()? / full.max(1.);
    let zero = samples["0"]["response"].as_f64().unwrap_or_default();
    let hundred = samples["100"]["response"].as_f64().unwrap_or_default();
    require(
        hundred > zero + 10. && full > 40. && ratio < 0.45,
        "glass blur/transparency check failed",
    )?;
    drop(board);
    ui.p.close()?;
    Ok(
        json!({"ok":true,"blur_observed":true,"control_changes_backdrop":true,"contrast_ratio":ratio,"dwm_hresult":hr,"dwm_material":material,"samples":samples}),
    )
}
fn run() -> Result<()> {
    let args: Vec<_> = std::env::args_os().skip(1).collect();
    require(args.len() == 2, "usage: ui_probe EXE EVIDENCE_DIR")?;
    let exe = Path::new(&args[0]).canonicalize()?;
    let out = PathBuf::from(&args[1]);
    fs::create_dir_all(&out)?;
    dpi_aware();
    let mut result = json!({"ok":false,"hardware":false});
    let tested = (|| {
        eprintln!("window reuse");
        result["window"] = window(&exe)?;
        eprintln!("controls");
        result["controls"] = controls::run(&exe, &out)?;
        eprintln!("native screenshots");
        result["visual"] = visual(&exe, &out)?;
        eprintln!("glass");
        result["glass"] = glass(&exe, &out)?;
        eprintln!("tray reclaim");
        result["tray"] = tray(&exe)?;
        eprintln!("frozen child recovery");
        result["hung_reap"] = hung(&exe)?;
        Ok::<_, Box<dyn std::error::Error>>(())
    })();
    if let Err(e) = &tested {
        result["error"] = json!(e.to_string());
    } else {
        result["ok"] = json!(true);
    }
    fs::write(
        out.join("results.json"),
        serde_json::to_vec_pretty(&result)?,
    )?;
    println!("{result}");
    tested
}
fn main() {
    if let Err(e) = run() {
        eprintln!("{e}");
        std::process::exit(1)
    }
}
