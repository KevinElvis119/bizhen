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
        mut progress: impl FnMut(usize, usize),
    ) -> Result<(), String> {
        let total = lines.len();
        for (i, line) in lines.iter().enumerate() {
            self.send_line(line)?;
            progress(i + 1, total);
        }
        Ok(())
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

    /// 软复位（Ctrl-X）。
    pub fn soft_reset(&mut self) -> Result<(), String> {
        self.port
            .write_all(&[0x18])
            .map_err(|e| format!("写入串口失败：{e}"))?;
        self.port.flush().ok();
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn lists_ports_without_panicking() {
        // CI/无串口机器上应返回空列表而不是崩
        let ports = list_ports();
        for p in ports {
            assert!(!p.name.is_empty());
        }
    }
}
