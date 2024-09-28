use std::{env, sync::Arc};

use anyhow::{Context, Result};
use dashmap::{DashMap, Entry};
use named_lock::NamedLock;
use playground_common::localhost;
use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    net::TcpListener,
    select,
    sync::{watch, OwnedRwLockReadGuard, OwnedRwLockWriteGuard, RwLock},
    time,
};
use tokio_util::sync::CancellationToken;
use tracing::info;

type LockId = u32;

struct Locker {
    lock: Arc<RwLock<()>>,
    clients: DashMap<LockId, Option<LockGuard>>,
    cancel: CancellationToken,
    timer_state_tx: watch::Sender<TimerState>,
}

enum LockGuard {
    Read { _inner: OwnedRwLockReadGuard<()> },
    Write { _inner: OwnedRwLockWriteGuard<()> },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum TimerState {
    Stopped,
    Counting,
}

impl Locker {
    fn new() -> Arc<Self> {
        let (timer_state_tx, mut timer_state_rx) = watch::channel(TimerState::Stopped);
        let res = Arc::new(Self {
            lock: Arc::default(),
            clients: DashMap::new(),
            cancel: CancellationToken::new(),
            timer_state_tx,
        });

        let self_ = Arc::clone(&res);
        tokio::spawn(async move {
            let timeout = std::time::Duration::from_secs(20);
            debug_assert!(self_.lock.try_write().is_ok());

            let mut timer_cancel = CancellationToken::new();
            loop {
                let changed = timer_state_rx.changed().await;
                match *timer_state_rx.borrow() {
                    TimerState::Counting if changed.is_ok() => {
                        let self_ = Arc::clone(&self_);
                        let timer_cancel = timer_cancel.clone();
                        tokio::spawn(async move {
                            info!(
                                "no more clients, starting an expiration timer of {:.1}s",
                                timeout.as_secs_f32()
                            );
                            select! {
                                _ = timer_cancel.cancelled() => (),
                                _ = time::sleep(timeout) => {
                                    debug_assert!(self_.lock.try_write().is_ok());
                                    self_.cancel.cancel();
                                }
                            }
                        });
                    }
                    _ => {
                        info!("cancelling the previous timer...");
                        timer_cancel.cancel();
                        if changed.is_err() {
                            break;
                        }
                        timer_cancel = CancellationToken::new();
                    }
                }
            }
        });

        res
    }

    async fn read(&self, id: LockId) {
        self.timer_state_tx.send(TimerState::Stopped).unwrap();
        match self.clients.entry(id) {
            Entry::Vacant(e) => {
                let guard = LockGuard::Read {
                    _inner: Arc::clone(&self.lock).read_owned().await,
                };
                e.insert(Some(guard));
            }
            Entry::Occupied(mut e) => {
                let taken = e
                    .get_mut()
                    .take_if(|it| !matches!(it, LockGuard::Read { .. }));
                if let Some(guard) = taken {
                    drop(guard);
                    let guard = LockGuard::Read {
                        _inner: Arc::clone(&self.lock).read_owned().await,
                    };
                    e.insert(Some(guard));
                }
            }
        };
    }

    async fn write(&self, id: LockId) {
        self.timer_state_tx.send(TimerState::Stopped).unwrap();
        match self.clients.entry(id) {
            Entry::Vacant(e) => {
                let guard = LockGuard::Write {
                    _inner: Arc::clone(&self.lock).write_owned().await,
                };
                e.insert(Some(guard));
            }
            Entry::Occupied(mut e) => {
                let taken = e
                    .get_mut()
                    .take_if(|it| !matches!(it, LockGuard::Write { .. }));
                if let Some(guard) = taken {
                    drop(guard);
                    let guard = LockGuard::Write {
                        _inner: Arc::clone(&self.lock).write_owned().await,
                    };
                    e.insert(Some(guard));
                }
            }
        };
    }

    fn unlock(&self, id: LockId) {
        let entry = self.clients.entry(id);
        if let Entry::Occupied(e) = entry {
            e.remove();
            if let Ok(guard) = self.lock.try_write() {
                self.timer_state_tx.send(TimerState::Counting).unwrap();
                drop(guard);
            }
        }
    }
}

#[tokio::main]
async fn main() -> Result<()> {
    tracing_subscriber::fmt::init();

    let named_lock = NamedLock::create("rustup-locker")?;
    let Ok(_guard) = named_lock.try_lock() else {
        info!("another locker instance is running, exiting...");
        return Ok(());
    };

    let port = env::args().nth(1).context("missing port")?.parse()?;

    let listener = TcpListener::bind(&localhost(port)[..]).await?;
    info!("listening on port {port}");

    let locker = Arc::new(Locker::new());
    loop {
        select! {
            _ = locker.cancel.cancelled() => {
                info!("exiting...");
                break Ok(())
            }
            accepted = listener.accept() => {
                let (mut socket, _) = accepted?;
                let locker = Arc::clone(&locker);
                tokio::spawn(async move {
                    while let Ok(n) = socket.read_u8().await {
                        let id = socket.read_u32().await.expect("missing LockId");
                        info!("received command {:?} from {id}", n as char);
                        match n {
                            b'r' => locker.read(id).await,
                            b'w' => locker.write(id).await,
                            b'u' => locker.unlock(id),
                            n => panic!("invalid command {:?}", n as char),
                        }
                        socket.write_u8(n).await.expect("failed to write response");
                    }
                });
            }
        }
    }
}
