//! 串口工作线程：所有串口读写都在后台线程，界面只读状态、投递命令。
//!
//! 这样点动、解锁、发作业都不会卡住界面；状态每 400ms 轮询一次 `?`。

use std::sync::mpsc::{self, Receiver, RecvTimeoutError, Sender};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use bizhen_core::serial::{parse_status, GrblLink};

/// 界面可读的机器状态快照。
#[derive(Clone, Debug, Default)]
pub struct State {
    pub connected: bool,
    pub port: String,
    pub baud: u32,
    /// GRBL 状态字：Idle / Run / Jog / Alarm …
    pub grbl_state: String,
    pub mpos: Option<(f64, f64, f64)>,
    pub feed: f64,
    pub message: String,
    /// 正在打开串口（可能被驱动卡住几秒，界面据此保持刷新）
    pub pending: bool,
    /// 正在发作业
    pub busy: bool,
    /// 作业已暂停（等「继续」）
    pub paused: bool,
    pub progress: (usize, usize),
}

/// 投递给工作线程的命令。
pub enum Cmd {
    Connect {
        port: String,
        baud: u32,
    },
    Disconnect,
    Jog {
        dx: f64,
        dy: f64,
        step: f64,
        feed: f64,
    },
    Unlock,
    Home,
    Reset,
    Send {
        lines: Vec<String>,
        pen_up: Vec<String>,
        pen_down: Vec<String>,
    },
    Pause,
    Resume,
    Abort,
}

pub struct Machine {
    tx: Sender<Cmd>,
    state: Arc<Mutex<State>>,
}

impl Machine {
    pub fn new() -> Self {
        let (tx, rx) = mpsc::channel();
        let state = Arc::new(Mutex::new(State::default()));
        let shared = state.clone();
        std::thread::spawn(move || worker(rx, shared));
        Self { tx, state }
    }

    pub fn snapshot(&self) -> State {
        self.state.lock().map(|s| s.clone()).unwrap_or_default()
    }

    /// 界面侧直接写一条提示（例如「正在连接…」），不等后台线程回报。
    pub fn set_message(&self, msg: impl Into<String>) {
        let msg = msg.into();
        set(&self.state, |s| s.message = msg);
    }

    pub fn send(&self, cmd: Cmd) {
        let _ = self.tx.send(cmd);
    }
}

fn set(state: &Arc<Mutex<State>>, f: impl FnOnce(&mut State)) {
    if let Ok(mut s) = state.lock() {
        f(&mut s);
    }
}

/// 作业进行中把命令队列里的控制命令消费掉。
///
/// `send_program_controlled` 是阻塞调用，worker 一旦进入它就不再 `recv`，
/// 所以「暂停 / 继续 / 终止」必须在发送循环里主动 `try_recv`，否则按钮按了没反应。
fn drain_control(
    rx: &Receiver<Cmd>,
    ctl: &std::sync::Arc<bizhen_core::serial::JobControl>,
    state: &Arc<Mutex<State>>,
) {
    while let Ok(cmd) = rx.try_recv() {
        match cmd {
            Cmd::Pause => {
                ctl.pause();
                set(state, |s| {
                    s.paused = true;
                    s.message = "已暂停（先抬笔，再停投喂）".into();
                });
            }
            Cmd::Resume => {
                ctl.resume();
                set(state, |s| {
                    s.paused = false;
                    s.message = "继续发送".into();
                });
            }
            Cmd::Abort => {
                ctl.abort();
                set(state, |s| s.message = "正在中止…".into());
            }
            // 作业进行中的其它命令（点动、回零等）直接忽略，免得搅乱发送节奏
            _ => {}
        }
    }
}

#[allow(unused_assignments)] // job 在其它 match 分支里读取，编译器按单次迭代看不出来
fn worker(rx: Receiver<Cmd>, state: Arc<Mutex<State>>) {
    let mut link: Option<GrblLink> = None;
    // 当前作业的控制标志（暂停/中止）
    let mut job: Option<std::sync::Arc<bizhen_core::serial::JobControl>> = None;

    let refresh_status = |link: &mut GrblLink, state: &Arc<Mutex<State>>| match link.status() {
        Ok(line) => {
            if let Some(st) = parse_status(&line) {
                let pos = st.position();
                set(state, |s| {
                    s.grbl_state = st.state.clone();
                    s.mpos = pos;
                    s.feed = st.feed.unwrap_or(0.0);
                });
            }
        }
        Err(e) => set(state, |s| s.message = e),
    };

    loop {
        match rx.recv_timeout(Duration::from_millis(400)) {
            Ok(Cmd::Connect { port, baud }) => {
                // 打开串口可能被驱动卡住（死端口/被占用），放到子线程里带超时等待，
                // 免得主工作线程被拖死、界面一直停在「正在连接」。
                set(&state, |s| {
                    s.pending = true;
                    s.message = format!("正在连接 {port} @ {baud}…");
                });
                let (otx, orx) = mpsc::channel();
                {
                    let port = port.clone();
                    std::thread::spawn(move || {
                        let _ = otx.send(GrblLink::open(&port, baud));
                    });
                }
                let opened = orx.recv_timeout(Duration::from_secs(8));
                match opened {
                    Ok(Ok(mut l)) => {
                        let _ = l.unlock();
                        set(&state, |s| {
                            s.pending = false;
                            s.connected = true;
                            s.port = port.clone();
                            s.baud = baud;
                            s.message = format!("已连接 {port} @ {baud}");
                            s.grbl_state = "读状态…".into();
                        });
                        refresh_status(&mut l, &state);
                        link = Some(l);
                    }
                    Ok(Err(e)) => {
                        set(&state, |s| {
                            s.pending = false;
                            s.connected = false;
                            s.message = e;
                        });
                        link = None;
                    }
                    Err(_) => {
                        set(&state, |s| {
                            s.pending = false;
                            s.connected = false;
                            s.message = format!(
                                "打开 {port} 超时（8 秒）：端口可能已被其它程序占用，或设备已拔出"
                            );
                        });
                        link = None;
                    }
                }
            }
            Ok(Cmd::Disconnect) => {
                link = None;
                set(&state, |s| {
                    s.connected = false;
                    s.grbl_state.clear();
                    s.mpos = None;
                    s.message = "已断开".into();
                });
            }
            Ok(Cmd::Jog { dx, dy, step, feed }) => {
                if let Some(l) = link.as_mut() {
                    let r = l.jog(dx * step, dy * step, 0.0, feed, true);
                    if let Err(e) = r {
                        set(&state, |s| s.message = e);
                    }
                } else {
                    set(&state, |s| s.message = "未连接".into());
                }
            }
            Ok(Cmd::Unlock) => {
                if let Some(l) = link.as_mut() {
                    let r = l.unlock();
                    set(&state, |s| {
                        s.message = match r {
                            Ok(()) => "已解锁（$X）".into(),
                            Err(e) => e,
                        }
                    });
                }
            }
            Ok(Cmd::Home) => {
                if let Some(l) = link.as_mut() {
                    let r = l.home();
                    set(&state, |s| {
                        s.message = match r {
                            Ok(()) => "回零中（$H）".into(),
                            Err(e) => e,
                        }
                    });
                }
            }
            Ok(Cmd::Reset) => {
                if let Some(l) = link.as_mut() {
                    let _ = l.soft_reset();
                    set(&state, |s| s.message = "已发送软复位".into());
                }
            }
            Ok(Cmd::Send {
                lines,
                pen_up,
                pen_down,
            }) => {
                let Some(l) = link.as_mut() else {
                    set(&state, |s| s.message = "未连接".into());
                    continue;
                };
                let total = lines.len();
                let ctl = std::sync::Arc::new(bizhen_core::serial::JobControl::default());
                job = Some(ctl.clone());
                set(&state, |s| {
                    s.busy = true;
                    s.paused = false;
                    s.progress = (0, total);
                });
                let result = l.send_program_controlled(
                    &lines,
                    |done, total| {
                        set(&state, |s| s.progress = (done, total));
                        drain_control(&rx, &ctl, &state);
                    },
                    &ctl,
                    &pen_up,
                    &pen_down,
                    // 暂停自旋期间也要把队列里的「继续 / 终止」读出来，
                    // 否则一旦暂停就再没人处理命令，只能杀进程
                    || drain_control(&rx, &ctl, &state),
                );
                job = None;
                let sent = state.lock().map(|s| s.progress.0).unwrap_or(0);
                set(&state, |s| {
                    s.busy = false;
                    s.paused = false;
                    s.message = match result {
                        Ok(bizhen_core::serial::SendOutcome::Done) => {
                            format!("发送完成（{total} 行）")
                        }
                        Ok(bizhen_core::serial::SendOutcome::Aborted) => {
                            format!("已中止（发到第 {sent} 行）")
                        }
                        Err(e) => format!("发送失败：{e}"),
                    };
                });
            }
            Ok(Cmd::Pause) => match job.as_ref() {
                Some(ctl) => {
                    ctl.pause();
                    set(&state, |s| {
                        s.paused = true;
                        s.message = "已暂停（先抬笔，再停投喂）".into();
                    });
                }
                None => set(&state, |s| s.message = "没有正在发送的作业".into()),
            },
            Ok(Cmd::Resume) => match job.as_ref() {
                Some(ctl) => {
                    ctl.resume();
                    set(&state, |s| {
                        s.paused = false;
                        s.message = "继续发送".into();
                    });
                }
                None => set(&state, |s| s.message = "没有暂停中的作业".into()),
            },
            Ok(Cmd::Abort) => match job.as_ref() {
                Some(ctl) => {
                    ctl.abort();
                    set(&state, |s| s.message = "正在中止…".into());
                }
                None => set(&state, |s| s.message = "没有正在发送的作业".into()),
            },
            Err(RecvTimeoutError::Timeout) => {
                if let Some(l) = link.as_mut() {
                    let busy = state.lock().map(|s| s.busy).unwrap_or(false);
                    if !busy {
                        refresh_status(l, &state);
                    }
                }
            }
            Err(RecvTimeoutError::Disconnected) => break,
        }
    }
}
