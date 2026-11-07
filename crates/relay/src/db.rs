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
    pub async fn touch(&self, id: Uuid) {
        let _ = sqlx::query("UPDATE remote_devices SET last_seen_at=now() WHERE id=$1")
            .bind(id)
            .execute(&self.pool)
            .await;
    }
}
