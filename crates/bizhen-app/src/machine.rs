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
    /// 正在发作业
    pub busy: bool,
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
    Send(Vec<String>),
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

    pub fn send(&self, cmd: Cmd) {
        let _ = self.tx.send(cmd);
    }
}

fn set(state: &Arc<Mutex<State>>, f: impl FnOnce(&mut State)) {
    if let Ok(mut s) = state.lock() {
        f(&mut s);
    }
}

fn worker(rx: Receiver<Cmd>, state: Arc<Mutex<State>>) {
    let mut link: Option<GrblLink> = None;

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
            Ok(Cmd::Connect { port, baud }) => match GrblLink::open(&port, baud) {
                Ok(mut l) => {
                    let _ = l.unlock();
                    set(&state, |s| {
                        s.connected = true;
                        s.port = port.clone();
                        s.baud = baud;
                        s.message = format!("已连接 {port} @ {baud}");
                        s.grbl_state = "读状态…".into();
                    });
                    refresh_status(&mut l, &state);
                    link = Some(l);
                }
                Err(e) => {
                    set(&state, |s| {
                        s.connected = false;
                        s.message = e;
                    });
                    link = None;
                }
            },
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
            Ok(Cmd::Send(lines)) => {
                let Some(l) = link.as_mut() else {
                    set(&state, |s| s.message = "未连接".into());
                    continue;
                };
                let total = lines.len();
                set(&state, |s| {
                    s.busy = true;
                    s.progress = (0, total);
                });
                let result = l.send_program(&lines, |done, total| {
                    set(&state, |s| s.progress = (done, total));
                });
                set(&state, |s| {
                    s.busy = false;
                    s.message = match result {
                        Ok(()) => format!("发送完成（{total} 行）"),
                        Err(e) => format!("发送失败：{e}"),
                    };
                });
            }
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
