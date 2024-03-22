use std::sync::{Arc};
use std::sync::atomic::{AtomicU32, Ordering};
use std::time::{Duration, Instant};
use log::debug;
use serde::{Deserialize, Serialize};
use tokio::sync::mpsc::{channel, Receiver, Sender};

use tokio::sync::RwLock;


#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all="camelCase")]
pub struct WatcherInfo {
    id: u32,
    name: String,
    buffered: f64,
    position: f64,
}

impl From<&Watcher> for WatcherInfo {
    fn from(value: &Watcher) -> Self {
        WatcherInfo {
            id: value.id,
            name: value.name.clone(),
            buffered: value.buffered,
            position: value.position,
        }
    }
}

#[derive(Debug, Copy, Clone, Serialize, Deserialize)]
#[serde(rename_all="camelCase")]
pub enum RoomState {
    Playing,
    Paused,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all="camelCase")]
pub struct RoomInfo {
    name: String,
    url: String,
    position: f64,
    state: RoomState,
    watchers: Vec<WatcherInfo>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all="camelCase")]
pub enum ToBrowser {
    Metadata(RoomInfo),
    Play {
        id: u32,
        request_id: u32,
        time: f64,
    },
    Pause {
        id: u32,
        request_id: u32,
        time: f64,
    },
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all="camelCase")]
pub enum FromBrowser {
    Join { name: String },
    Play {
        id: u32,
        request_id: u32,
        time: f64,
    },
    Pause {
        id: u32,
        request_id: u32,
        time: f64,
    },
    Status {
        id: u32,
        position: f64,
        buffered: f64,
        state: RoomState,
    }
}

pub struct Watcher {
    name: String,
    id: u32,
    send_to_browser: Sender<ToBrowser>,
    estimated_latency: Duration,
    buffered: f64,
    position: f64,
    time_set: Instant,
    state: RoomState,
}

impl Watcher {
    pub fn new(name: String, id: u32) -> (Self, Receiver<ToBrowser>) {
        let (send_to_browser, browser_receiver) = channel(64);

        let watcher = Watcher {
            name,
            id,
            send_to_browser,
            estimated_latency: Duration::from_secs(0),
            buffered: 0.0,
            position: 0.0,
            time_set: Instant::now(),
            state: RoomState::Paused,
        };

        (watcher, browser_receiver)
    }
}

pub struct Room {
    pub name: String,
    pub url: String,
    state: RoomState,
    send: Sender<FromBrowser>,
    pub watchers: RwLock<Vec<Watcher>>,
    id: AtomicU32,
    last_request_id: AtomicU32,
}

async fn broadcast(room: &Room, msg: ToBrowser) {
    let watchers = room.watchers.write().await;

    for w in &*watchers {
        // TODO: handle error
        let _ = w.send_to_browser.send(msg.clone()).await;
    }
}

pub async fn room_thread(room: Arc<Room>, mut recv: Receiver<FromBrowser>) {
    debug!("Starting room thread");

    loop {
        while let Some(msg) = recv.recv().await {
            debug!("{} got message: {:?}", room.name, msg);

            match msg {
                FromBrowser::Join { .. } => {
                    let room_info = room.get_info().await;

                    broadcast(&room, ToBrowser::Metadata(room_info)).await;
                }
                FromBrowser::Play { id, request_id, time } => {
                    broadcast(&room, ToBrowser::Play {id, request_id, time}).await;
                }
                FromBrowser::Pause { id, request_id, time } => {
                    broadcast(&room, ToBrowser::Pause {id, request_id, time}).await;
                }
                FromBrowser::Status { id, position, buffered, state } => {
                    room.update_status(id, position, buffered, state).await;

                    let room_info = room.get_info().await;
                    broadcast(&room, ToBrowser::Metadata(room_info)).await;
                }
            }
        }
    }
}

impl Room {
    pub fn new(name: String, url: String) -> (Self, Receiver<FromBrowser>) {
        let (send, recv) = channel(64);

        let room = Room {
            name,
            url,
            state: RoomState::Paused,
            send,
            watchers: RwLock::new(Vec::new()),
            id: AtomicU32::new(1),
            last_request_id: AtomicU32::new(0),
        };

        (room, recv)
    }

    pub async fn add_watcher(&self, name: String) -> Receiver<ToBrowser> {
        let mut watchers = self.watchers.write().await;

        let id = self.id.fetch_add(1, Ordering::SeqCst);
        let (watcher, browser_receiver) = Watcher::new(name.clone(), id);
        watchers.push(watcher);

        self.send.send(FromBrowser::Join { name }).await.unwrap();

        browser_receiver
    }

    pub async fn update_status(&self, id: u32, position: f64, buffered: f64, state: RoomState) {
        let mut watchers = self.watchers.write().await;

        if let Some(watcher) = watchers.iter_mut().find(|w| w.id == id) {
            watcher.position = position;
            watcher.buffered = buffered;
            watcher.state = state;
        }
    }

    pub async fn send(&self, msg: FromBrowser) {
        self.send.send(msg).await.unwrap();
    }

    pub async fn get_info(&self) -> RoomInfo {
        let watchers = self.watchers.read().await;
        let watchers = watchers.iter().map(|w| w.into()).collect();

        RoomInfo {
            name: self.name.clone(),
            url: self.url.clone(),
            position: 0.0,
            state: self.state,
            watchers,
        }
    }
}