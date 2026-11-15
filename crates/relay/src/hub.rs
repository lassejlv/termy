use std::collections::{HashMap, HashSet};
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};

use axum::extract::ws::{CloseFrame, Message};
use tokio::sync::{RwLock, mpsc};
use uuid::Uuid;

/// Kind byte of a binary relay frame carrying a sealed packet.
pub const DATA_KIND: u8 = 1;
/// Frames a slow device may have queued before it is dropped.
const QUEUE: usize = 32;
/// Sealed bytes one device may have waiting: a burst of full-size frames,
/// not 32 of them.
const CONNECTION_BUDGET: usize = 16 << 20;
/// Sealed bytes waiting across all devices, so sockets that stop reading
/// can't use up the relay's memory.
const HUB_BUDGET: usize = 256 << 20;
/// With the hub budget spent, a frame this small still reaches a device that
/// has nothing waiting, so devices that keep up aren't cut off.
const ALWAYS_FITS: usize = 64 << 10;

fn weight(message: &Message) -> usize {
    match message {
        Message::Binary(bytes) => bytes.len(),
        _ => 0,
    }
}

/// The sending half of a socket's outgoing queue. Sealed bytes count against
/// the device's and the hub's budget until they are written to the socket.
#[derive(Clone)]
pub struct Outbox {
    tx: mpsc::Sender<Message>,
    waiting: Arc<AtomicUsize>,
    hub_waiting: Arc<AtomicUsize>,
}

impl Outbox {
    /// Queues a message; false when the device is too far behind to take it.
    pub fn try_send(&self, message: Message) -> bool {
        let size = weight(&message);
        let own = self.waiting.fetch_add(size, Ordering::Relaxed);
        let all = self.hub_waiting.fetch_add(size, Ordering::Relaxed);
        let fits = (own + size <= CONNECTION_BUDGET && all + size <= HUB_BUDGET)
            || (own == 0 && size <= ALWAYS_FITS);
        if fits && self.tx.try_send(message).is_ok() {
            return true;
        }
        self.waiting.fetch_sub(size, Ordering::Relaxed);
        self.hub_waiting.fetch_sub(size, Ordering::Relaxed);
        false
    }
}

/// The receiving half of a socket's outgoing queue.
pub struct Inbox {
    rx: mpsc::Receiver<Message>,
    waiting: Arc<AtomicUsize>,
    hub_waiting: Arc<AtomicUsize>,
    /// Size of the message handed out last, which is still being written.
    writing: usize,
}

impl Inbox {
    /// The next message to write. Asking for it says the previous one is written.
    pub async fn recv(&mut self) -> Option<Message> {
        let written = std::mem::take(&mut self.writing);
        self.release(written);
        let message = self.rx.recv().await?;
        self.writing = weight(&message);
        Some(message)
    }

    fn release(&self, size: usize) {
        self.waiting.fetch_sub(size, Ordering::Relaxed);
        self.hub_waiting.fetch_sub(size, Ordering::Relaxed);
    }
}

impl Drop for Inbox {
    fn drop(&mut self) {
        // Closing first makes later sends fail and return their own bytes.
        self.rx.close();
        let mut unwritten = self.writing;
        while let Ok(message) = self.rx.try_recv() {
            unwritten += weight(&message);
        }
        self.release(unwritten);
    }
}

struct Connection {
    peers: HashSet<Uuid>,
    /// Tells a replaced socket apart from the one that took its place.
    serial: u64,
    tx: Outbox,
}

/// Routes sealed packets between explicitly paired online devices. A binary
/// frame is the kind byte, the 16-byte peer device ID, then bytes the relay
/// can't read; the relay swaps the recipient's ID for the sender's.
#[derive(Default)]
pub struct Hub {
    connections: RwLock<HashMap<Uuid, Connection>>,
    next_serial: std::sync::atomic::AtomicU64,
    waiting: Arc<AtomicUsize>,
}

pub fn event(kind: &str, device: Uuid, online: Option<bool>) -> Message {
    let mut value = serde_json::json!({ "type": kind, "device": device });
    if let Some(online) = online {
        value["online"] = online.into();
    }
    Message::Text(value.to_string().into())
}

impl Hub {
    pub async fn is_online(&self, device: Uuid) -> bool {
        self.connections.read().await.contains_key(&device)
    }

    pub async fn online(&self) -> Vec<Uuid> {
        self.connections.read().await.keys().copied().collect()
    }

    pub async fn is_current(&self, device: Uuid, serial: u64) -> bool {
        self.connections
            .read()
            .await
            .get(&device)
            .is_some_and(|c| c.serial == serial)
    }

    /// Registers a socket's outgoing queue; returns its serial for `disconnect`.
    pub async fn connect(&self, device: Uuid, peers: HashSet<Uuid>, tx: Outbox) -> u64 {
        let serial = self
            .next_serial
            .fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        let mut connections = self.connections.write().await;
        // A device has one socket; a reconnect replaces the old one.
        if let Some(old) = connections.insert(
            device,
            Connection {
                peers: peers.clone(),
                serial,
                tx: tx.clone(),
            },
        ) {
            old.tx.try_send(Message::Close(Some(CloseFrame {
                code: 4000,
                reason: "replaced".into(),
            })));
        }
        tx.try_send(event("ready", device, None));
        for (id, other) in connections.iter() {
            if peers.contains(id) && other.peers.contains(&device) {
                tx.try_send(event("presence", *id, Some(true)));
                other.tx.try_send(event("presence", device, Some(true)));
            }
        }
        serial
    }

    pub async fn disconnect(&self, device: Uuid, serial: u64) {
        let mut connections = self.connections.write().await;
        let Some(current) = connections.get(&device) else {
            return;
        };
        if current.serial != serial {
            return;
        }
        let peers = current.peers.clone();
        connections.remove(&device);
        Self::announce_gone(&connections, &peers, device);
    }

    /// Closes a socket when its device registration is removed.
    pub async fn kick(&self, device: Uuid) {
        let mut connections = self.connections.write().await;
        if let Some(connection) = connections.remove(&device) {
            connection.tx.try_send(Message::Close(Some(CloseFrame {
                code: 4001,
                reason: "device removed".into(),
            })));
            Self::announce_gone(&connections, &connection.peers, device);
        }
    }

    fn announce_gone(connections: &HashMap<Uuid, Connection>, peers: &HashSet<Uuid>, device: Uuid) {
        for other in connections
            .iter()
            .filter(|(id, _)| peers.contains(id))
            .map(|(_, c)| c)
        {
            other.tx.try_send(event("presence", device, Some(false)));
        }
    }

    /// Forwards a frame to its peer only between explicitly paired devices.
    pub async fn route(&self, sender: Uuid, serial: u64, mut frame: Vec<u8>) {
        if frame.len() <= 17 || frame[0] != DATA_KIND {
            return;
        }
        let Ok(peer) = Uuid::from_slice(&frame[1..17]) else {
            return;
        };
        let connections = self.connections.read().await;
        let Some(from) = connections.get(&sender) else {
            return;
        };
        if from.serial != serial {
            return;
        }
        let target = connections
            .get(&peer)
            .filter(|t| from.peers.contains(&peer) && t.peers.contains(&sender) && peer != sender);
        let Some(target) = target else {
            from.tx.try_send(event("undeliverable", peer, None));
            return;
        };
        frame[1..17].copy_from_slice(sender.as_bytes());
        if !target.tx.try_send(Message::Binary(frame.into())) {
            // The peer can't keep up; its session resynchronises on reconnect.
            let serial = target.serial;
            drop(connections);
            self.disconnect(peer, serial).await;
        }
    }

    pub async fn authorize(&self, host: Uuid, client: Uuid) {
        let mut connections = self.connections.write().await;
        let host_online = connections.contains_key(&host);
        let client_online = connections.contains_key(&client);
        if let Some(c) = connections.get_mut(&host) {
            c.peers.insert(client);
            c.tx.try_send(event("presence", client, Some(client_online)));
        }
        if let Some(c) = connections.get_mut(&client) {
            c.peers.insert(host);
            c.tx.try_send(event("presence", host, Some(host_online)));
        }
    }

    /// Ends routing between two devices. Only a device that had the other as
    /// a peer hears about it: the caller picks `b`, which may be a stranger.
    pub async fn revoke(&self, a: Uuid, b: Uuid) {
        let mut connections = self.connections.write().await;
        for (device, peer) in [(a, b), (b, a)] {
            if let Some(c) = connections.get_mut(&device)
                && c.peers.remove(&peer)
            {
                c.tx.try_send(event("presence", peer, Some(false)));
            }
        }
    }

    pub fn queue(&self) -> (Outbox, Inbox) {
        let (tx, rx) = mpsc::channel(QUEUE);
        let waiting = Arc::new(AtomicUsize::new(0));
        (
            Outbox {
                tx,
                waiting: waiting.clone(),
                hub_waiting: self.waiting.clone(),
            },
            Inbox {
                rx,
                waiting,
                hub_waiting: self.waiting.clone(),
                writing: 0,
            },
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn frame(peer: Uuid) -> Vec<u8> {
        let mut out = vec![DATA_KIND];
        out.extend_from_slice(peer.as_bytes());
        out.extend_from_slice(b"sealed");
        out
    }
    fn sized(peer: Uuid, size: usize) -> Vec<u8> {
        let mut out = frame(peer);
        out.resize(size, 0);
        out
    }
    /// Takes what is queued without counting it as written.
    fn drain(inbox: &mut Inbox) -> Vec<Message> {
        let mut out = vec![];
        while let Ok(m) = inbox.rx.try_recv() {
            out.push(m);
        }
        out
    }
    #[tokio::test]
    async fn requires_explicit_pairing_and_rewrites_sender() {
        let hub = Hub::default();
        let (a, b) = (Uuid::new_v4(), Uuid::new_v4());
        let (at, mut ar) = hub.queue();
        let (bt, mut br) = hub.queue();
        hub.connect(a, HashSet::new(), at).await;
        hub.connect(b, HashSet::new(), bt).await;
        drain(&mut ar);
        drain(&mut br);
        hub.route(a, 0, frame(b)).await;
        assert!(drain(&mut br).is_empty());
        assert!(
            matches!(drain(&mut ar).as_slice(),[Message::Text(t)] if t.contains("undeliverable"))
        );
        hub.authorize(a, b).await;
        drain(&mut ar);
        drain(&mut br);
        hub.route(a, 0, frame(b)).await;
        let got = drain(&mut br);
        let [Message::Binary(bytes)] = got.as_slice() else {
            panic!("{got:?}")
        };
        assert_eq!(&bytes[1..17], a.as_bytes());
        assert_eq!(&bytes[17..], b"sealed");
        hub.revoke(a, b).await;
        drain(&mut ar);
        drain(&mut br);
        hub.route(a, 0, frame(b)).await;
        assert!(drain(&mut br).is_empty());
    }
    #[tokio::test]
    async fn pairing_is_not_transitive() {
        let hub = Hub::default();
        let (a, b, c) = (Uuid::new_v4(), Uuid::new_v4(), Uuid::new_v4());
        let (at, mut ar) = hub.queue();
        let (bt, _br) = hub.queue();
        let (ct, mut cr) = hub.queue();
        hub.connect(a, HashSet::from([b]), at).await;
        hub.connect(b, HashSet::from([a, c]), bt).await;
        hub.connect(c, HashSet::from([b]), ct).await;
        drain(&mut ar);
        drain(&mut cr);
        hub.route(a, 0, frame(c)).await;
        assert!(drain(&mut cr).is_empty());
    }
    #[tokio::test]
    async fn slow_peer_is_removed_when_queue_fills() {
        let hub = Hub::default();
        let (a, b) = (Uuid::new_v4(), Uuid::new_v4());
        let (at, _ar) = hub.queue();
        let (bt, _br) = hub.queue();
        let serial = hub.connect(a, HashSet::from([b]), at).await;
        hub.connect(b, HashSet::from([a]), bt).await;
        for _ in 0..QUEUE {
            hub.route(a, serial, frame(b)).await;
        }
        assert!(!hub.is_online(b).await);
    }
    #[tokio::test]
    async fn replaced_socket_cannot_route_as_its_successor() {
        let hub = Hub::default();
        let (a, b) = (Uuid::new_v4(), Uuid::new_v4());
        let (at, _ar) = hub.queue();
        let (bt, mut br) = hub.queue();
        let old = hub.connect(a, HashSet::from([b]), at).await;
        hub.connect(b, HashSet::from([a]), bt).await;
        let (replacement, _rx) = hub.queue();
        hub.connect(a, HashSet::from([b]), replacement).await;
        drain(&mut br);
        hub.route(a, old, frame(b)).await;
        assert!(drain(&mut br).is_empty());
    }
    #[tokio::test]
    async fn replaced_socket_cannot_unregister_successor() {
        let hub = Hub::default();
        let id = Uuid::new_v4();
        let (a, _ar) = hub.queue();
        let (b, _br) = hub.queue();
        let old = hub.connect(id, HashSet::new(), a).await;
        hub.connect(id, HashSet::new(), b).await;
        hub.disconnect(id, old).await;
        assert!(hub.is_online(id).await);
    }
    #[tokio::test]
    async fn revoking_a_stranger_tells_them_nothing() {
        let hub = Hub::default();
        let (a, b, stranger) = (Uuid::new_v4(), Uuid::new_v4(), Uuid::new_v4());
        let (at, mut ar) = hub.queue();
        let (bt, mut br) = hub.queue();
        let (st, mut sr) = hub.queue();
        hub.connect(a, HashSet::from([b]), at).await;
        hub.connect(b, HashSet::from([a]), bt).await;
        hub.connect(stranger, HashSet::new(), st).await;
        for inbox in [&mut ar, &mut br, &mut sr] {
            drain(inbox);
        }
        hub.revoke(a, stranger).await;
        assert!(drain(&mut sr).is_empty());
        assert!(drain(&mut ar).is_empty());
        hub.revoke(a, b).await;
        assert_eq!(drain(&mut ar).len(), 1);
        assert_eq!(drain(&mut br).len(), 1);
    }
    #[tokio::test]
    async fn waiting_bytes_are_bounded_per_device_and_freed_once_written() {
        let hub = Hub::default();
        let (a, b) = (Uuid::new_v4(), Uuid::new_v4());
        let (at, _ar) = hub.queue();
        let (bt, mut br) = hub.queue();
        let serial = hub.connect(a, HashSet::from([b]), at).await;
        hub.connect(b, HashSet::from([a]), bt).await;
        drain(&mut br);
        let full = CONNECTION_BUDGET / 4;
        for _ in 0..4 {
            hub.route(a, serial, sized(b, full)).await;
        }
        assert_eq!(hub.waiting.load(Ordering::Relaxed), CONNECTION_BUDGET);
        // Handing out the second frame says the first one is written.
        br.recv().await.unwrap();
        br.recv().await.unwrap();
        hub.route(a, serial, sized(b, full)).await;
        assert!(hub.is_online(b).await);
        hub.route(a, serial, sized(b, full)).await;
        assert!(!hub.is_online(b).await, "a device over budget is dropped");
        drop(br);
        assert_eq!(hub.waiting.load(Ordering::Relaxed), 0);
        hub.route(a, serial, sized(b, full)).await;
        assert_eq!(hub.waiting.load(Ordering::Relaxed), 0);
    }
    #[tokio::test]
    async fn a_spent_hub_budget_still_serves_devices_that_keep_up() {
        let hub = Hub::default();
        let (a, b) = (Uuid::new_v4(), Uuid::new_v4());
        let (at, _ar) = hub.queue();
        let (bt, mut br) = hub.queue();
        let serial = hub.connect(a, HashSet::from([b]), at).await;
        hub.connect(b, HashSet::from([a]), bt).await;
        drain(&mut br);
        hub.waiting.store(HUB_BUDGET, Ordering::Relaxed);
        hub.route(a, serial, sized(b, ALWAYS_FITS)).await;
        assert!(hub.is_online(b).await);
        // A second frame behind the first, or a large one, has to wait for room.
        hub.route(a, serial, sized(b, ALWAYS_FITS)).await;
        assert!(!hub.is_online(b).await);
        assert_eq!(
            hub.waiting.load(Ordering::Relaxed),
            HUB_BUDGET + ALWAYS_FITS
        );
    }
}
