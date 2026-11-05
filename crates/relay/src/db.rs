use chrono::{DateTime, Utc};
use sqlx::PgPool;
use uuid::Uuid;

/// Accounts, devices and session tokens. Passwords are Argon2id hashes and
/// tokens SHA-256 hashes, so a database leak opens no account.
#[derive(Clone)]
pub struct Db {
    pool: PgPool,
}

#[derive(sqlx::FromRow)]
pub struct User {
    pub id: Uuid,
    pub email: String,
    pub password_hash: String,
}

#[derive(sqlx::FromRow, Clone)]
pub struct Device {
    pub id: Uuid,
    pub user_id: Uuid,
    pub name: String,
    pub kind: String,
    pub public_key: Vec<u8>,
    pub created_at: DateTime<Utc>,
    pub last_seen_at: Option<DateTime<Utc>>,
}

const DEVICE_COLUMNS: &str = "id, user_id, name, kind, public_key, created_at, last_seen_at";

impl Db {
    pub fn new(pool: PgPool) -> Self {
        Self { pool }
    }

    /// None when an account with this email exists.
    pub async fn create_user(
        &self,
        email: &str,
        password_hash: &str,
    ) -> sqlx::Result<Option<User>> {
        sqlx::query_as::<_, User>(
            "INSERT INTO users (email, password_hash) VALUES ($1, $2) ON CONFLICT DO NOTHING \
             RETURNING id, email, password_hash",
        )
        .bind(email)
        .bind(password_hash)
        .fetch_optional(&self.pool)
        .await
    }

    pub async fn user_by_email(&self, email: &str) -> sqlx::Result<Option<User>> {
        sqlx::query_as::<_, User>(
            "SELECT id, email, password_hash FROM users WHERE lower(email) = lower($1)",
        )
        .bind(email)
        .fetch_optional(&self.pool)
        .await
    }

    pub async fn user(&self, id: Uuid) -> sqlx::Result<Option<User>> {
        sqlx::query_as::<_, User>("SELECT id, email, password_hash FROM users WHERE id = $1")
            .bind(id)
            .fetch_optional(&self.pool)
            .await
    }

    pub async fn delete_user(&self, id: Uuid) -> sqlx::Result<()> {
        sqlx::query("DELETE FROM users WHERE id = $1")
            .bind(id)
            .execute(&self.pool)
            .await?;
        Ok(())
    }

    /// Registers a device, or renames the one this account already has with
    /// this key (signing in again on the same phone).
    pub async fn upsert_device(
        &self,
        user_id: Uuid,
        name: &str,
        kind: &str,
        public_key: &[u8],
    ) -> sqlx::Result<Device> {
        sqlx::query_as::<_, Device>(&format!(
            "INSERT INTO devices (user_id, name, kind, public_key) VALUES ($1, $2, $3, $4) \
             ON CONFLICT (user_id, public_key) DO UPDATE SET name = EXCLUDED.name, kind = EXCLUDED.kind \
             RETURNING {DEVICE_COLUMNS}"
        ))
        .bind(user_id)
        .bind(name)
        .bind(kind)
        .bind(public_key)
        .fetch_one(&self.pool)
        .await
    }

    pub async fn devices(&self, user_id: Uuid) -> sqlx::Result<Vec<Device>> {
        sqlx::query_as::<_, Device>(&format!(
            "SELECT {DEVICE_COLUMNS} FROM devices WHERE user_id = $1 ORDER BY created_at"
        ))
        .bind(user_id)
        .fetch_all(&self.pool)
        .await
    }

    pub async fn delete_device(&self, id: Uuid, user_id: Uuid) -> sqlx::Result<bool> {
        let result = sqlx::query("DELETE FROM devices WHERE id = $1 AND user_id = $2")
            .bind(id)
            .bind(user_id)
            .execute(&self.pool)
            .await?;
        Ok(result.rows_affected() > 0)
    }

    pub async fn touch_device(&self, id: Uuid) -> sqlx::Result<()> {
        sqlx::query("UPDATE devices SET last_seen_at = now() WHERE id = $1")
            .bind(id)
            .execute(&self.pool)
            .await?;
        Ok(())
    }

    /// Replaces the device's session token.
    pub async fn create_session(
        &self,
        token_hash: &[u8],
        user_id: Uuid,
        device_id: Uuid,
    ) -> sqlx::Result<()> {
        let mut tx = self.pool.begin().await?;
        sqlx::query("DELETE FROM sessions WHERE device_id = $1")
            .bind(device_id)
            .execute(&mut *tx)
            .await?;
        sqlx::query("INSERT INTO sessions (token_hash, user_id, device_id) VALUES ($1, $2, $3)")
            .bind(token_hash)
            .bind(user_id)
            .bind(device_id)
            .execute(&mut *tx)
            .await?;
        tx.commit().await
    }

    pub async fn session(&self, token_hash: &[u8]) -> sqlx::Result<Option<(Uuid, Uuid)>> {
        sqlx::query_as::<_, (Uuid, Uuid)>(
            "SELECT user_id, device_id FROM sessions WHERE token_hash = $1",
        )
        .bind(token_hash)
        .fetch_optional(&self.pool)
        .await
    }
}
