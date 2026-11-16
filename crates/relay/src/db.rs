use chrono::{DateTime, Utc};
use sqlx::PgPool;
use uuid::Uuid;

#[derive(Clone)]
pub struct Db {
    pub pool: PgPool,
}
#[derive(sqlx::FromRow, Clone)]
pub struct Device {
    pub id: Uuid,
    pub kind: String,
    pub public_key: Vec<u8>,
    pub created_at: DateTime<Utc>,
    pub last_seen_at: Option<DateTime<Utc>>,
}
#[derive(sqlx::FromRow)]
pub struct Pairing {
    pub id: Uuid,
    pub host_id: Uuid,
    pub client_id: Option<Uuid>,
    pub init_packet: Option<Vec<u8>>,
    pub proof: Option<Vec<u8>>,
    pub response_packet: Option<Vec<u8>>,
    pub encrypted_client_name: Option<Vec<u8>>,
    pub expires_at: DateTime<Utc>,
}
impl Db {
    pub fn new(pool: PgPool) -> Self {
        Self { pool }
    }
    pub async fn device(&self, id: Uuid) -> sqlx::Result<Device> {
        sqlx::query_as(
            "SELECT id,kind,public_key,created_at,last_seen_at FROM remote_devices WHERE id=$1",
        )
        .bind(id)
        .fetch_one(&self.pool)
        .await
    }
    pub async fn authenticate(&self, hash: &[u8]) -> sqlx::Result<Option<Device>> {
        sqlx::query_as("SELECT id,kind,public_key,created_at,last_seen_at FROM remote_devices WHERE token_hash=$1")
            .bind(hash).fetch_optional(&self.pool).await
    }
    pub async fn peers(&self, id: Uuid) -> sqlx::Result<Vec<Device>> {
        sqlx::query_as("SELECT d.id,d.kind,d.public_key,d.created_at,d.last_seen_at FROM remote_devices d JOIN remote_links l ON (l.host_id=$1 AND l.client_id=d.id) OR (l.client_id=$1 AND l.host_id=d.id) ORDER BY d.created_at")
            .bind(id).fetch_all(&self.pool).await
    }
    pub async fn pairing(&self, id: Uuid) -> sqlx::Result<Option<Pairing>> {
        // Keep an approved result briefly so the phone can retrieve it after a network interruption.
        sqlx::query_as("SELECT * FROM remote_pairings WHERE id=$1 AND (expires_at>now() OR (response_packet IS NOT NULL AND expires_at>now()-interval '10 minutes'))")
            .bind(id).fetch_optional(&self.pool).await
    }
    /// Removes invitations past retrieval, then registrations that haven't
    /// connected for 90 days and that no link or invitation refers to.
    /// Returns the removed devices.
    pub async fn prune(&self, online: &[Uuid]) -> sqlx::Result<Vec<Uuid>> {
        sqlx::query("DELETE FROM remote_pairings WHERE expires_at<now()-interval '10 minutes'")
            .execute(&self.pool)
            .await?;
        sqlx::query_scalar("DELETE FROM remote_devices d WHERE coalesce(d.last_seen_at,d.created_at)<now()-interval '90 days' AND NOT (d.id=ANY($1)) AND NOT EXISTS(SELECT 1 FROM remote_links l WHERE l.host_id=d.id OR l.client_id=d.id) AND NOT EXISTS(SELECT 1 FROM remote_pairings p WHERE p.host_id=d.id OR p.client_id=d.id) RETURNING d.id")
            .bind(online).fetch_all(&self.pool).await
    }
    pub async fn linked(&self, host: Uuid, client: Uuid) -> sqlx::Result<bool> {
        sqlx::query_scalar(
            "SELECT EXISTS(SELECT 1 FROM remote_links WHERE host_id=$1 AND client_id=$2)",
        )
        .bind(host)
        .bind(client)
        .fetch_one(&self.pool)
        .await
    }
    /// Stores a phone's APNs token. A reinstall can hand the same token to a
    /// new registration, so no other device keeps it.
    pub async fn set_push_token(
        &self,
        device: Uuid,
        token: &[u8],
        environment: &str,
    ) -> sqlx::Result<()> {
        let mut tx = self.pool.begin().await?;
        sqlx::query("DELETE FROM remote_push_tokens WHERE token=$2 AND device_id<>$1")
            .bind(device)
            .bind(token)
            .execute(&mut *tx)
            .await?;
        sqlx::query("INSERT INTO remote_push_tokens(device_id,token,environment) VALUES($1,$2,$3) ON CONFLICT (device_id) DO UPDATE SET token=excluded.token,environment=excluded.environment,updated_at=now()")
            .bind(device).bind(token).bind(environment).execute(&mut *tx).await?;
        tx.commit().await
    }
    pub async fn push_token(&self, device: Uuid) -> sqlx::Result<Option<(Vec<u8>, String)>> {
        sqlx::query_as("SELECT token,environment FROM remote_push_tokens WHERE device_id=$1")
            .bind(device)
            .fetch_optional(&self.pool)
            .await
    }
    pub async fn clear_push_token(&self, device: Uuid) -> sqlx::Result<()> {
        sqlx::query("DELETE FROM remote_push_tokens WHERE device_id=$1")
            .bind(device)
            .execute(&self.pool)
            .await
            .map(drop)
    }
    /// Forgets a token APNs refused, unless the phone has replaced it meanwhile.
    pub async fn forget_push_token(&self, device: Uuid, token: &[u8]) -> sqlx::Result<()> {
        sqlx::query("DELETE FROM remote_push_tokens WHERE device_id=$1 AND token=$2")
            .bind(device)
            .bind(token)
            .execute(&self.pool)
            .await
            .map(drop)
    }
    /// Forgets the tokens of these devices that no Mac is linked to any more.
    pub async fn drop_unlinked_push_tokens(&self, devices: &[Uuid]) -> sqlx::Result<()> {
        sqlx::query("DELETE FROM remote_push_tokens t WHERE t.device_id=ANY($1) AND NOT EXISTS(SELECT 1 FROM remote_links l WHERE l.client_id=t.device_id)")
            .bind(devices)
            .execute(&self.pool)
            .await
            .map(drop)
    }
    pub async fn touch(&self, id: Uuid) {
        let _ = sqlx::query("UPDATE remote_devices SET last_seen_at=now() WHERE id=$1")
            .bind(id)
            .execute(&self.pool)
            .await;
    }
}
