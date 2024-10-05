use std::{env, ptr, sync::Arc};

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
use tracing::{info, warn};

type LockId = u32;

/// A [`Locker`] is a simple service that manages a system-wide [`RwLock`] for multiple possible
/// rustup instances (i.e. clients), each identified with a [`LockId`].
///
/// For subprocesses launched by a rustup instance, they inherit the [`LockId`] from their parent
/// process.
///
/// Thus, we can enforce the classical multiple concurrent reads XOR single exclusive write
/// policy on the user's Rust installation.
struct Locker {
    /// The underlying [`RwLock`].
    lock: Arc<RwLock<()>>,
    /// The collection of clients,
    clients: DashMap<LockId, LockGuard>,
    /// The token indicating whether this locker has expired.
    cancel: CancellationToken,
    /// The sender for this [`Locker`]'s [`TimerStateSignal`],
    /// associated with its own expiration timer.
    timer_state_tx: watch::Sender<TimerStateSignal>,
}

/// An owned [`RwLock`] guard that is either a read or write guard.
enum LockGuard {
    // HACK: `_inner` is required to avoid "dead code" warnings.
    // I have no idea why I can't write `Read(..)` and `Write(..)` here.
    Read { _inner: OwnedRwLockReadGuard<()> },
    Write { _inner: OwnedRwLockWriteGuard<()> },
}

/// A signal indicating how the [`Locker`]'s expiration timer should behave.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum TimerStateSignal {
    /// The timer should be stopped and reset.
    Stop,
    /// The timer should be started.
    Start,
}

impl Locker {
    fn new() -> Arc<Self> {
        let (timer_state_tx, mut timer_state_rx) = watch::channel(TimerStateSignal::Stop);
        let res = Arc::new(Self {
            lock: Arc::default(),
            clients: DashMap::new(),
            cancel: CancellationToken::new(),
            timer_state_tx,
        });

        // Start the expiration timer.
        let self_ = Arc::clone(&res);
        tokio::spawn(async move {
            let timeout = std::time::Duration::from_secs(20);
            debug_assert!(self_.lock.try_write().is_ok());

            let mut timer_cancel = CancellationToken::new();
            loop {
                let changed = timer_state_rx.changed().await;
                match *timer_state_rx.borrow() {
                    TimerStateSignal::Start if changed.is_ok() => {
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
                            // The sender is gone. Stop the loop right now.
                            break;
                        }
                        timer_cancel = CancellationToken::new();
                    }
                }
            }
        });

        res
    }

    /// Acquires a read lock from this [`Locker`].
    /// If a lock already exists for this [`LockId`], and:
    /// - The lock is a read lock: this will be a no-op;
    /// - The lock is the write lock: it will be downgraded to a read lock.
    ///
    /// This also stops the expiration timer.
    async fn read(&self, id: LockId) {
        self.timer_state_tx.send(TimerStateSignal::Stop).unwrap();
        match self.clients.entry(id) {
            Entry::Vacant(e) => {
                let guard = LockGuard::Read {
                    _inner: Arc::clone(&self.lock).read_owned().await,
                };
                e.insert(guard);
            }
            Entry::Occupied(mut e) => {
                if let guard @ LockGuard::Write { .. } = e.get_mut() {
                    unsafe {
                        ptr::drop_in_place(guard);
                        let new_guard = LockGuard::Read {
                            _inner: Arc::clone(&self.lock).read_owned().await,
                        };
                        ptr::write(guard, new_guard);
                    }
                }
            }
        };
    }

    /// Acquires the write lock from this [`Locker`].
    /// If a lock already exists for this [`LockId`], and:
    /// - The lock is the write lock: this will be a no-op;
    /// - The lock is a read lock: it will be upgrade to the write lock.
    ///
    /// This also stops the expiration timer.
    async fn write(&self, id: LockId) {
        self.timer_state_tx.send(TimerStateSignal::Stop).unwrap();
        match self.clients.entry(id) {
            Entry::Vacant(e) => {
                let guard = LockGuard::Write {
                    _inner: Arc::clone(&self.lock).write_owned().await,
                };
                e.insert(guard);
            }
            Entry::Occupied(mut e) => {
                if let guard @ LockGuard::Read { .. } = e.get_mut() {
                    unsafe {
                        ptr::drop_in_place(guard);
                        let new_guard = LockGuard::Write {
                            _inner: Arc::clone(&self.lock).write_owned().await,
                        };
                        ptr::write(guard, new_guard);
                    }
                }
            }
        };
    }

    /// Releases the lock associated with this [`LockId`].
    ///
    /// This also starts the expiration timer if there are no more clients
    /// associated with this [`Locker`].
    fn unlock(&self, id: LockId) {
        let entry = self.clients.entry(id);
        if let Entry::Occupied(e) = entry {
            e.remove();
            if let Ok(guard) = self.lock.try_write() {
                self.timer_state_tx.send(TimerStateSignal::Start).unwrap();
                drop(guard);
            }
        }
    }
}

#[tokio::main]
async fn main() -> Result<()> {
    tracing_subscriber::fmt::init();

    // Ensure that only one locker instance is running.
    //
    // TODO: This could be integrated into `Locker::try_new()`,
    // not sure if this is a good idea though.
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
                        if let Err(e) = socket.write_u8(n).await {
                            warn!("failed to write response to {id}: {e:?}");
                            locker.unlock(id);
                        }
                    }
                });
            }
        }
    }
}
