use crossterm::event;
use std::time::Duration;
use tokio::sync::mpsc::UnboundedSender;

use super::app::AppEvent;

/// 独立线程轮询 crossterm 事件（键盘/鼠标/尺寸），送入 UI 事件通道。
pub fn spawn_input_thread(tx: UnboundedSender<AppEvent>) {
    std::thread::spawn(move || {
        loop {
            match event::poll(Duration::from_millis(200)) {
                Ok(true) => match event::read() {
                    Ok(ev) => {
                        if tx.send(AppEvent::Input(ev)).is_err() {
                            break;
                        }
                    }
                    Err(_) => break,
                },
                Ok(false) => {}
                Err(_) => break,
            }
        }
    });
}
