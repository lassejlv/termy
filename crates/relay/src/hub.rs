use std::collections::HashMap;

use axum::extract::ws::{CloseFrame, Message};
use tokio::sync::{RwLock, mpsc};
use uuid::Uuid;

/// Kind byte of a binary relay frame carrying a sealed packet.
pub const DATA_KIND: u8 = 1;
/// Frames a slow device may have queued before it is dropped.
const QUEUE: usize = 1024;

struct Connection {
    user_id: Uuid,
    /// Tells a replaced socket apart from the one that took its place.
    serial: u64,
    tx: mpsc::Sender<Message>,
}

/// Routes sealed packets between the online devices of one account. A binary
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

    /// Registers a socket's outgoing queue; returns its serial for `disconnect`.
    pub async fn connect(&self, device: Uuid, user_id: Uuid, tx: mpsc::Sender<Message>) -> u64 {
        let serial = self
            .next_serial
            .fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        let mut connections = self.connections.write().await;
        // A device has one socket; a reconnect replaces the old one.
        if let Some(old) = connections.insert(
            device,
            Connection {
                user_id,
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
            if other.user_id == user_id && *id != device {
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
        let user_id = current.user_id;
        connections.remove(&device);
        Self::announce_gone(&connections, user_id, device);
    }

    /// Closes a device's socket: signed out, removed, or account deleted.
    pub async fn kick(&self, device: Uuid) {
        let mut connections = self.connections.write().await;
        if let Some(connection) = connections.remove(&device) {
            let _ = connection.tx.try_send(Message::Close(Some(CloseFrame {
                code: 4001,
                reason: "signed out".into(),
            })));
            Self::announce_gone(&connections, connection.user_id, device);
        }
    }

    fn announce_gone(connections: &HashMap<Uuid, Connection>, user_id: Uuid, device: Uuid) {
        for other in connections.values().filter(|c| c.user_id == user_id) {
            let _ = other.tx.try_send(event("presence", device, Some(false)));
        }
    }

    /// Forwards a frame to its peer when both devices belong to one account.
    pub async fn route(&self, sender: Uuid, mut frame: Vec<u8>) {
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
        let target = connections
            .get(&peer)
            .filter(|t| t.user_id == from.user_id && peer != sender);
        let Some(target) = target else {
            let _ = from.tx.try_send(event("undeliverable", peer, None));
            return;
        };
        frame[1..17].copy_from_slice(sender.as_bytes());
        if target.tx.try_send(Message::Binary(frame.into())).is_err() {
            // The peer can't keep up; its session resynchronises on reconnect.
            let _ = target.tx.try_send(Message::Close(None));
        }
    }

    pub fn queue() -> (mpsc::Sender<Message>, mpsc::Receiver<Message>) {
        mpsc::channel(QUEUE)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn frame(peer: Uuid, payload: &[u8]) -> Vec<u8> {
        let mut frame = vec![DATA_KIND];
        frame.extend_from_slice(peer.as_bytes());
        frame.extend_from_slice(payload);
        frame
    }

    async fn drain(rx: &mut mpsc::Receiver<Message>) -> Vec<Message> {
        let mut out = vec![];
        while let Ok(message) = rx.try_recv() {
            out.push(message);
        }
        out
    }

    #[tokio::test]
    async fn routes_within_an_account_and_rewrites_the_sender() {
        let hub = Hub::default();
        let (user, phone, mac) = (Uuid::new_v4(), Uuid::new_v4(), Uuid::new_v4());
        let (phone_tx, mut phone_rx) = Hub::queue();
        let (mac_tx, mut mac_rx) = Hub::queue();
        hub.connect(phone, user, phone_tx).await;
        hub.connect(mac, user, mac_tx).await;
        drain(&mut phone_rx).await;
        drain(&mut mac_rx).await;

        hub.route(phone, frame(mac, b"sealed")).await;
        let got = drain(&mut mac_rx).await;
        let [Message::Binary(bytes)] = got.as_slice() else {
            panic!("{got:?}")
        };
        assert_eq!(&bytes[1..17], phone.as_bytes());
        assert_eq!(&bytes[17..], b"sealed");
    }

    #[tokio::test]
    async fn refuses_other_accounts() {
        let hub = Hub::default();
        let (alice, mallory) = (Uuid::new_v4(), Uuid::new_v4());
        let (mac, attacker) = (Uuid::new_v4(), Uuid::new_v4());
        let (mac_tx, mut mac_rx) = Hub::queue();
        let (attacker_tx, mut attacker_rx) = Hub::queue();
        hub.connect(mac, alice, mac_tx).await;
        hub.connect(attacker, mallory, attacker_tx).await;
        drain(&mut mac_rx).await;
        drain(&mut attacker_rx).await;

        hub.route(attacker, frame(mac, b"x")).await;
        assert!(drain(&mut mac_rx).await.is_empty());
        let got = drain(&mut attacker_rx).await;
        assert!(matches!(got.as_slice(), [Message::Text(t)] if t.contains("undeliverable")));
    }

    #[tokio::test]
    async fn a_replaced_socket_does_not_unregister_its_successor() {
        let hub = Hub::default();
        let (user, phone) = (Uuid::new_v4(), Uuid::new_v4());
        let (first, _rx1) = Hub::queue();
        let (second, _rx2) = Hub::queue();
        let old = hub.connect(phone, user, first).await;
        hub.connect(phone, user, second).await;
        hub.disconnect(phone, old).await;
        assert!(hub.is_online(phone).await);
    }
}
