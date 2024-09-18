use std::{env, sync::Arc};

use anyhow::{Context, Result};
use dashmap::{DashMap, Entry};
use named_lock::NamedLock;
use playground_common::localhost;
use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    net::TcpListener,
    select,
    sync::{OwnedRwLockReadGuard, OwnedRwLockWriteGuard, RwLock},
    time,
};
use tokio_util::sync::CancellationToken;
use tracing::info;

type LockId = u32;

#[derive(Default)]
struct Locker {
    lock: Arc<RwLock<()>>,
    clients: DashMap<LockId, Option<LockGuard>>,
}

enum LockGuard {
    Read { _inner: OwnedRwLockReadGuard<()> },
    Write { _inner: OwnedRwLockWriteGuard<()> },
}

impl Locker {
    async fn read(&self, id: LockId) {
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
        }
    }

    async fn write(&self, id: LockId) {
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
        }
    }

    fn unlock(&self, id: LockId) {
        self.clients.remove(&id);
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

    let token = CancellationToken::new();
    let locker = Arc::new(Locker::default());

    {
        let token = token.clone();
        tokio::spawn(async move {
            // TODO: Add proper quit condition
            time::sleep(std::time::Duration::from_secs(60)).await;
            token.cancel();
        });
    }

    loop {
        select! {
            _ = token.cancelled() => {
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
