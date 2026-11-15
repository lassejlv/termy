use tokio::sync::{Mutex, MutexGuard};
use uuid::Uuid;

/// Keeps a device's links in the database and in the hub in step: whoever
/// changes or reads them for a device holds that device's gate until the hub
/// reflects it. Devices share a fixed set of gates, so unrelated devices
/// rarely wait on each other and one slow request can't stall the relay.
pub struct Gates(Vec<Mutex<()>>);

impl Default for Gates {
    fn default() -> Self {
        Self((0..256).map(|_| Mutex::new(())).collect())
    }
}

impl Gates {
    /// Locks the gates of `devices`, always in the same order so two requests
    /// naming the same devices can't deadlock.
    pub async fn lock(&self, devices: &[Uuid]) -> Vec<MutexGuard<'_, ()>> {
        let mut gates: Vec<usize> = devices
            .iter()
            .map(|device| (device.as_u128() % self.0.len() as u128) as usize)
            .collect();
        gates.sort_unstable();
        gates.dedup();
        let mut guards = Vec::with_capacity(gates.len());
        for gate in gates {
            guards.push(self.0[gate].lock().await);
        }
        guards
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;
    use std::time::Duration;

    #[tokio::test]
    async fn opposite_orders_and_shared_gates_do_not_deadlock() {
        let gates = Arc::new(Gates::default());
        let (a, b) = (Uuid::from_u128(1), Uuid::from_u128(2));
        let same_gate_as_a = Uuid::from_u128(1 + 256);
        let mut tasks = vec![];
        for pair in [[a, b], [b, a], [a, same_gate_as_a], [a, a]] {
            let gates = gates.clone();
            tasks.push(tokio::spawn(async move {
                for _ in 0..200 {
                    let _guards = gates.lock(&pair).await;
                    tokio::task::yield_now().await;
                }
            }));
        }
        for task in tasks {
            tokio::time::timeout(Duration::from_secs(10), task)
                .await
                .expect("deadlock")
                .unwrap();
        }
    }

    #[tokio::test]
    async fn a_held_device_blocks_only_requests_naming_it() {
        let gates = Gates::default();
        let (a, b, c) = (Uuid::from_u128(1), Uuid::from_u128(2), Uuid::from_u128(3));
        let _held = gates.lock(&[a]).await;
        let waits = tokio::time::timeout(Duration::from_millis(50), gates.lock(&[b, a])).await;
        assert!(waits.is_err());
        let _free = gates.lock(&[b, c]).await;
    }
}
