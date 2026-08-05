use sqlx::postgres::PgPoolOptions;
use sqlx::Executor;
use std::sync::Arc;

#[derive(Clone)]
pub struct PgPool {
    inner: Arc<PgPoolInner>,
}

struct PgPoolInner {
    pool: sqlx::PgPool,
    tenant_id: String,
}

impl PgPool {
    pub async fn connect(database_url: &str, tenant_id: &str) -> Result<Self, sqlx::Error> {
        let pool = PgPoolOptions::new()
            .max_connections(10)
            .connect(database_url)
            .await?;

        Ok(Self {
            inner: Arc::new(PgPoolInner {
                pool,
                tenant_id: tenant_id.to_string(),
            }),
        })
    }

    pub fn from_pool(pool: sqlx::PgPool, tenant_id: &str) -> Self {
        Self {
            inner: Arc::new(PgPoolInner {
                pool,
                tenant_id: tenant_id.to_string(),
            }),
        }
    }

    pub fn pool(&self) -> &sqlx::PgPool {
        &self.inner.pool
    }

    pub fn tenant_id(&self) -> &str {
        &self.inner.tenant_id
    }

    pub async fn set_tenant(&self, conn: &mut sqlx::PgConnection) -> Result<(), sqlx::Error> {
        sqlx::query("SELECT set_config('app.tenant_id', $1, true)")
            .bind(&self.inner.tenant_id)
            .execute(conn)
            .await?;
        Ok(())
    }

    pub async fn migrate(&self) -> Result<(), sqlx::Error> {
        let schema = include_str!("schema.sql");
        self.inner.pool.execute(schema).await?;
        Ok(())
    }
}
