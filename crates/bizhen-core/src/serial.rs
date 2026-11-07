//! GRBL 串口联机：发 G-code、等 `ok`、查状态。
//!
//! 协议要点（与原版 `machine/grbl.py` + `serial_link.py` 一致）：
//! * 每行指令必须等上一个 `ok` 再发（GRBL 的接收缓冲有限）；
//! * 行号/校验和是可选的，这里用最简单的 `line + "\n"` 形式（GRBL 默认接受）；
//! * `?` 查询实时状态，返回 `<Idle|MPos:...>` 形式的字符串；
//! * 报错是 `error:N`；被锁时先发 `$X` 解锁。

use std::io::{BufRead, BufReader, Write};
use std::time::{Duration, Instant};

use serialport::{SerialPort, SerialPortType};

/// 一个可用的串口。
#[derive(Clone, Debug)]
pub struct PortInfo {
    pub name: String,
    pub description: String,
}

/// 列出本机串口（带描述，便于用户认出是哪台机器）。
pub fn list_ports() -> Vec<PortInfo> {
    serialport::available_ports()
        .unwrap_or_default()
        .into_iter()
        .map(|p| {
            let description = match &p.port_type {
                SerialPortType::UsbPort(info) => format!(
                    "USB {:04x}:{:04x}{}",
                    info.vid,
                    info.pid,
                    info.product
                        .as_deref()
                        .map(|s| format!(" {s}"))
                        .unwrap_or_default()
                ),
                SerialPortType::PciPort => "PCI".into(),
                SerialPortType::BluetoothPort => "蓝牙".into(),
                SerialPortType::Unknown => "未知设备".into(),
            };
            PortInfo {
                name: p.port_name,
                description,
            }
        })
        .collect()
}

/// GRBL 连接。
pub struct GrblLink {
    port: Box<dyn SerialPort>,
    reader: BufReader<Box<dyn SerialPort>>,
    timeout: Duration,
}

impl GrblLink {
    /// 打开串口（GRBL 常用 115200）。
    pub fn open(name: &str, baud: u32) -> Result<Self, String> {
        let port = serialport::new(name, baud)
            .timeout(Duration::from_millis(200))
            .open()
            .map_err(|e| format!("打开串口 {name} 失败：{e}"))?;
        let reader = BufReader::new(
            port.try_clone()
                .map_err(|e| format!("复制串口句柄失败：{e}"))?,
        );
        Ok(Self {
            port,
            reader,
            timeout: Duration::from_secs(30),
        })
    }

    pub fn set_timeout(&mut self, t: Duration) {
        self.timeout = t;
    }

    /// 逐行读，直到出现 `ok` / `error` / `ALARM`；返回最后一条应答。
    fn wait_ack(&mut self) -> Result<String, String> {
        let start = Instant::now();
        let mut line = String::new();
        loop {
            if start.elapsed() > self.timeout {
                return Err("等待 GRBL 应答超时".into());
            }
            line.clear();
            match self.reader.read_line(&mut line) {
                Ok(0) => continue,
                Ok(_) => {
                    let t = line.trim();
                    if t.is_empty() {
                        continue;
                    }
                    if t.starts_with("error") || t.starts_with("ALARM") {
                        return Err(format!("GRBL 报错：{t}"));
                    }
                    if t == "ok" || t.starts_with("ok") {
                        return Ok(t.to_string());
                    }
                    // 其它（如启动问候、状态行）继续读
                }
                Err(ref e) if e.kind() == std::io::ErrorKind::TimedOut => continue,
                Err(e) => return Err(format!("读取串口失败：{e}")),
            }
        }
    }

    /// 发一行并等应答。
    pub fn send_line(&mut self, line: &str) -> Result<(), String> {
        let trimmed = line.trim();
        if trimmed.is_empty() {
            return Ok(());
        }
        self.port
            .write_all(format!("{trimmed}\n").as_bytes())
            .map_err(|e| format!("写入串口失败：{e}"))?;
        self.port.flush().ok();
        self.wait_ack().map(|_| ())
    }

    /// 批量发送一份程序；`progress(已发, 总数)` 用于界面进度。
    pub fn send_program(
        &mut self,
        lines: &[String],
        progress: impl FnMut(usize, usize),
    ) -> Result<(), String> {
        self.send_program_controlled(lines, progress, &JobControl::default(), &[], &[], || {})
            .map(|_| ())
    }

    /// 可暂停 / 可中止地发送程序。
    ///
    /// 每发一行前检查 `ctl`：
    /// * 置为暂停 → 先发抬笔指令，停止投喂（在途运动走完自然停下，不洇墨）；
    /// * 恢复 → 先发落笔指令，继续投喂；
    /// * 中止 → 发软复位 `\x18` 清空气动缓冲，返回 `Ok(Aborted)`。
    ///
    /// `poll` 在每个循环周期都会被调用一次（包括暂停自旋期间），调用方用它来
    /// 消费自己的命令队列——否则暂停时没人能读到「继续 / 终止」，作业会卡死。
    pub fn send_program_controlled(
        &mut self,
        lines: &[String],
        mut progress: impl FnMut(usize, usize),
        ctl: &JobControl,
        pen_up: &[String],
        pen_down: &[String],
        mut poll: impl FnMut(),
    ) -> Result<SendOutcome, String> {
        let total = lines.len();
        let mut pen_is_up = false;
        for (i, line) in lines.iter().enumerate() {
            if ctl.aborted() {
                // 软复位：清缓冲、停运动
                self.port
                    .write_all(&[0x18])
                    .map_err(|e| format!("写入串口失败：{e}"))?;
                self.port.flush().ok();
                return Ok(SendOutcome::Aborted);
            }
            if ctl.paused() && !pen_is_up {
                for l in pen_up {
                    self.send_line(l)?;
                }
                pen_is_up = true;
            }
            while ctl.paused() && !ctl.aborted() {
                poll();
                std::thread::sleep(std::time::Duration::from_millis(30));
            }
            // 暂停中被终止：不要再落笔，直接软复位
            if ctl.aborted() {
                self.port
                    .write_all(&[0x18])
                    .map_err(|e| format!("写入串口失败：{e}"))?;
                self.port.flush().ok();
                return Ok(SendOutcome::Aborted);
            }
            if pen_is_up {
                for l in pen_down {
                    self.send_line(l)?;
                }
                pen_is_up = false;
            }
            self.send_line(line)?;
            progress(i + 1, total);
        }
        Ok(SendOutcome::Done)
    }

    /// 查询实时状态（`?`），返回原始应答行。
    pub fn status(&mut self) -> Result<String, String> {
        self.port
            .write_all(b"?")
            .map_err(|e| format!("写入串口失败：{e}"))?;
        self.port.flush().ok();
        let start = Instant::now();
        let mut line = String::new();
        loop {
            if start.elapsed() > Duration::from_secs(2) {
                return Err("查询状态超时".into());
            }
            line.clear();
            match self.reader.read_line(&mut line) {
                Ok(0) => continue,
                Ok(_) => {
                    let t = line.trim();
                    if t.starts_with('<') {
                        return Ok(t.to_string());
                    }
                }
                Err(ref e) if e.kind() == std::io::ErrorKind::TimedOut => continue,
                Err(e) => return Err(format!("读取串口失败：{e}")),
            }
        }
    }

    /// 解锁（`$X`）：GRBL 上电/报警后会进 hold，需要先解锁才能动。
    pub fn unlock(&mut self) -> Result<(), String> {
        self.send_line("$X")
    }

    /// 回零（`$H`）。
    pub fn home(&mut self) -> Result<(), String> {
        self.send_line("$H")
    }

    /// 点动：默认发专用 jog 指令，不改变 G 组状态。
    ///
    /// * GRBL ≥ 1.1：`$J=G21G91 X.. Y.. F..`
    /// * 老固件不支持 `$J=`，退化为 `G21G91 X.. Y.. F..`
    pub fn jog(
        &mut self,
        dx: f64,
        dy: f64,
        dz: f64,
        feed: f64,
        modern: bool,
    ) -> Result<(), String> {
        let mut cmd = String::from(if modern { "$J=G21G91" } else { "G21G91" });
        if dx.abs() > 1e-9 {
            cmd.push_str(&format!(" X{}", fmt_num(dx)));
        }
        if dy.abs() > 1e-9 {
            cmd.push_str(&format!(" Y{}", fmt_num(dy)));
        }
        if dz.abs() > 1e-9 {
            cmd.push_str(&format!(" Z{}", fmt_num(dz)));
        }
        cmd.push_str(&format!(" F{}", fmt_num(feed)));
        self.send_line(&cmd)
    }

    /// 软复位（Ctrl-X）。
    pub fn soft_reset(&mut self) -> Result<(), String> {
        self.port
            .write_all(&[0x18])
            .map_err(|e| format!("写入串口失败：{e}"))?;
        self.port.flush().ok();
        Ok(())
    }
}

fn fmt_num(v: f64) -> String {
    let s = format!("{v:.3}");
    let s = s.trim_end_matches('0').trim_end_matches('.');
    if s.is_empty() || s == "-0" {
        "0".into()
    } else {
        s.to_string()
    }
}

/// GRBL 实时状态（`?` 的应答 ``<Idle|MPos:0.000,0.000,0.000|FS:0,0>``）。
#[derive(Clone, Debug, Default)]
pub struct MachineStatus {
    /// Idle / Run / Jog / Hold / Door / Home / Alarm / Check / Sleep
    pub state: String,
    pub mpos: Option<(f64, f64, f64)>,
    pub wpos: Option<(f64, f64, f64)>,
    pub feed: Option<f64>,
    pub spindle: Option<f64>,
}

impl MachineStatus {
    /// 当前显示用的坐标：优先机械坐标。
    pub fn position(&self) -> Option<(f64, f64, f64)> {
        self.mpos.or(self.wpos)
    }
}

/// 解析状态行；不是状态行时返回 `None`。
pub fn parse_status(line: &str) -> Option<MachineStatus> {
    let t = line.trim();
    let inner = t.strip_prefix('<')?.strip_suffix('>')?;
    let mut st = MachineStatus::default();
    let mut parts = inner.split('|');
    st.state = parts.next()?.to_string();
    for p in parts {
        if let Some(v) = p.strip_prefix("MPos:") {
            st.mpos = parse_xyz(v);
        } else if let Some(v) = p.strip_prefix("WPos:") {
            st.wpos = parse_xyz(v);
        } else if let Some(v) = p.strip_prefix("FS:") {
            let mut it = v.split(',');
            st.feed = it.next().and_then(|x| x.parse().ok());
            st.spindle = it.next().and_then(|x| x.parse().ok());
        }
    }
    Some(st)
}

fn parse_xyz(v: &str) -> Option<(f64, f64, f64)> {
    let mut it = v.split(',');
    let x = it.next()?.parse().ok()?;
    let y = it.next()?.parse().ok()?;
    let z = it.next().unwrap_or("0").parse().unwrap_or(0.0);
    Some((x, y, z))
}

/// 作业控制标志：界面线程置位，发送线程在每行之间检查。
#[derive(Debug, Default)]
pub struct JobControl {
    paused: std::sync::atomic::AtomicBool,
    aborted: std::sync::atomic::AtomicBool,
}

use std::sync::atomic::Ordering as AtomicOrdering;

impl JobControl {
    pub fn pause(&self) {
        self.paused.store(true, AtomicOrdering::SeqCst);
    }
    pub fn resume(&self) {
        self.paused.store(false, AtomicOrdering::SeqCst);
    }
    pub fn abort(&self) {
        self.aborted.store(true, AtomicOrdering::SeqCst);
    }
    pub fn paused(&self) -> bool {
        self.paused.load(AtomicOrdering::SeqCst)
    }
    pub fn aborted(&self) -> bool {
        self.aborted.load(AtomicOrdering::SeqCst)
    }
}

/// 一次发送的结果。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SendOutcome {
    Done,
    Aborted,
}
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn job_control_flags() {
        let c = JobControl::default();
        assert!(!c.paused() && !c.aborted());
        c.pause();
        assert!(c.paused());
        c.resume();
        assert!(!c.paused());
        c.abort();
        assert!(c.aborted());
    }

    #[test]
    fn parses_grbl_status() {
        let st = parse_status("<Idle|MPos:1.500,-2.000,3.250|FS:0,0>").unwrap();
        assert_eq!(st.state, "Idle");
        assert_eq!(st.mpos, Some((1.5, -2.0, 3.25)));
        assert_eq!(st.feed, Some(0.0));
        let run = parse_status("<Run|WPos:10,20,0|FS:1200,1000>").unwrap();
        assert_eq!(run.state, "Run");
        assert_eq!(run.position(), Some((10.0, 20.0, 0.0)));
        assert!(parse_status("ok").is_none());
    }

    #[test]
    fn jog_command_shape() {
        let mut cmd = String::from("$J=G21G91");
        cmd.push_str(&format!(" X{}", fmt_num(10.0)));
        assert!(cmd.contains("X10"));
        assert_eq!(fmt_num(-0.0), "0");
        assert_eq!(fmt_num(1.500), "1.5");
    }

    #[test]
    fn lists_ports_without_panicking() {
        // CI/无串口机器上应返回空列表而不是崩
        let ports = list_ports();
        for p in ports {
            assert!(!p.name.is_empty());
        }
    }
}
