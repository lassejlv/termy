use std::collections::{HashMap, HashSet};

use axum::extract::ws::{CloseFrame, Message};
use tokio::sync::{RwLock, mpsc};
use uuid::Uuid;

/// Kind byte of a binary relay frame carrying a sealed packet.
pub const DATA_KIND: u8 = 1;
/// Frames a slow device may have queued before it is dropped.
const QUEUE: usize = 32;

struct Connection {
    peers: HashSet<Uuid>,
    /// Tells a replaced socket apart from the one that took its place.
    serial: u64,
    tx: mpsc::Sender<Message>,
}

/// Routes sealed packets between explicitly paired online devices. A binary
/// frame is the kind byte, the 16-byte peer device ID, then bytes the relay
/// can't read; the relay swaps the recipient's ID for the sender's.
#[derive(Default)]
pub struct Hub {
    connections: RwLock<HashMap<Uuid, Connection>>,
    next_serial: std::sync::atomic::AtomicU64,
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

    pub async fn is_current(&self, device: Uuid, serial: u64) -> bool {
        self.connections
            .read()
            .await
            .get(&device)
            .is_some_and(|c| c.serial == serial)
    }

    /// Registers a socket's outgoing queue; returns its serial for `disconnect`.
    pub async fn connect(
        &self,
        device: Uuid,
        peers: HashSet<Uuid>,
        tx: mpsc::Sender<Message>,
    ) -> u64 {
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
            let _ = old.tx.try_send(Message::Close(Some(CloseFrame {
                code: 4000,
                reason: "replaced".into(),
            })));
        }
        let _ = tx.try_send(event("ready", device, None));
        for (id, other) in connections.iter() {
            if peers.contains(id) && other.peers.contains(&device) {
                let _ = tx.try_send(event("presence", *id, Some(true)));
                let _ = other.tx.try_send(event("presence", device, Some(true)));
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
            let _ = connection.tx.try_send(Message::Close(Some(CloseFrame {
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
            let _ = other.tx.try_send(event("presence", device, Some(false)));
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
            let _ = from.tx.try_send(event("undeliverable", peer, None));
            return;
        };
        frame[1..17].copy_from_slice(sender.as_bytes());
        if target.tx.try_send(Message::Binary(frame.into())).is_err() {
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
            let _ =
                c.tx.try_send(event("presence", client, Some(client_online)));
        }
        if let Some(c) = connections.get_mut(&client) {
            c.peers.insert(host);
            let _ = c.tx.try_send(event("presence", host, Some(host_online)));
        }
    }

    pub async fn revoke(&self, a: Uuid, b: Uuid) {
        let mut connections = self.connections.write().await;
        for (device, peer) in [(a, b), (b, a)] {
            if let Some(c) = connections.get_mut(&device) {
                c.peers.remove(&peer);
                let _ = c.tx.try_send(event("presence", peer, Some(false)));
            }
        }
    }

    pub fn queue() -> (mpsc::Sender<Message>, mpsc::Receiver<Message>) {
        mpsc::channel(QUEUE)
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
    fn drain(rx: &mut mpsc::Receiver<Message>) -> Vec<Message> {
        let mut out = vec![];
        while let Ok(m) = rx.try_recv() {
            out.push(m);
        }
        out
    }
    #[tokio::test]
    async fn requires_explicit_pairing_and_rewrites_sender() {
        let hub = Hub::default();
        let (a, b) = (Uuid::new_v4(), Uuid::new_v4());
        let (at, mut ar) = Hub::queue();
        let (bt, mut br) = Hub::queue();
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
        let (at, mut ar) = Hub::queue();
        let (bt, _br) = Hub::queue();
        let (ct, mut cr) = Hub::queue();
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
        let (at, _ar) = Hub::queue();
        let (bt, _br) = Hub::queue();
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
        let (at, _ar) = Hub::queue();
        let (bt, mut br) = Hub::queue();
        let old = hub.connect(a, HashSet::from([b]), at).await;
        hub.connect(b, HashSet::from([a]), bt).await;
        let (replacement, _rx) = Hub::queue();
        hub.connect(a, HashSet::from([b]), replacement).await;
        drain(&mut br);
        hub.route(a, old, frame(b)).await;
        assert!(drain(&mut br).is_empty());
    }
    #[tokio::test]
    async fn replaced_socket_cannot_unregister_successor() {
        let hub = Hub::default();
        let id = Uuid::new_v4();
        let (a, _ar) = Hub::queue();
        let (b, _br) = Hub::queue();
        let old = hub.connect(id, HashSet::new(), a).await;
        hub.connect(id, HashSet::new(), b).await;
        hub.disconnect(id, old).await;
        assert!(hub.is_online(id).await);
    }
}
