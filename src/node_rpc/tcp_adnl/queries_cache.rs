use std::sync::{Arc, Weak};

use tokio::sync::Barrier;

use crate::util::FxDashMap;

#[derive(Default)]
pub struct QueriesCache {
    queries: FxDashMap<[u8; 32], QueryState>,
}

impl QueriesCache {
    pub fn add_query(self: &Arc<Self>, query_id: [u8; 32]) -> PendingAdnlQuery {
        let barrier = Arc::new(Barrier::new(2));
        let query = QueryState::Sent(barrier.clone());

        self.queries.insert(query_id, query);

        PendingAdnlQuery {
            query_id,
            barrier,
            cache: Arc::downgrade(self),
            finished: false,
        }
    }

    pub async fn update_query(
        &self,
        query_id: [u8; 32],
        answer: Option<&[u8]>,
    ) -> Result<bool, QueriesCacheError> {
        use dashmap::mapref::entry::Entry;

        let old = match self.queries.entry(query_id) {
            Entry::Vacant(_) => None,
            Entry::Occupied(mut entry) => match entry.get() {
                QueryState::Sent(_) => Some(entry.insert(match answer {
                    Some(bytes) => QueryState::Received(bytes.to_vec()),
                    None => QueryState::Timeout,
                })),
                _ => None,
            },
        };

        match old {
            Some(QueryState::Sent(barrier)) => {
                barrier.wait().await;
                Ok(true)
            }
            Some(_) => Err(QueriesCacheError::UnexpectedState),
            None => Ok(false),
        }
    }
}

pub struct PendingAdnlQuery {
    query_id: [u8; 32],
    barrier: Arc<Barrier>,
    cache: Weak<QueriesCache>,
    finished: bool,
}

impl PendingAdnlQuery {
    pub async fn wait(mut self) -> Result<Option<Vec<u8>>, QueriesCacheError> {
        self.barrier.wait().await;
        self.finished = true;

        let cache = match self.cache.upgrade() {
            Some(cache) => cache,
            None => return Err(QueriesCacheError::CacheDropped),
        };

        match cache.queries.remove(&self.query_id) {
            Some((_, QueryState::Received(answer))) => Ok(Some(answer)),
            Some((_, QueryState::Timeout)) => Ok(None),
            Some(_) => Err(QueriesCacheError::InvalidQueryState),
            None => Err(QueriesCacheError::UnknownId),
        }
    }
}

impl Drop for PendingAdnlQuery {
    fn drop(&mut self) {
        if self.finished {
            return;
        }

        if let Some(cache) = self.cache.upgrade() {
            cache.queries.remove(&self.query_id);
        }
    }
}

enum QueryState {
    /// Initial state. Barrier is used to block receiver part until answer is received
    Sent(Arc<Barrier>),
    /// Query was resolved with some data
    Received(Vec<u8>),
    /// Query was timed out
    Timeout,
}

#[derive(thiserror::Error, Debug)]
pub enum QueriesCacheError {
    #[error("queries cache was dropped")]
    CacheDropped,
    #[error("invalid query state")]
    InvalidQueryState,
    #[error("unknown query id")]
    UnknownId,
    #[error("unexpected query state")]
    UnexpectedState,
}
