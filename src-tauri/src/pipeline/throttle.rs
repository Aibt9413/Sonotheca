//! 全局下载限速：令牌桶。
//!
//! 所有下载任务共享同一个 `TokenBucket`（AppState 持有），因此 1MB/s 是
//! 全局上限，而不是每个任务各 1MB/s。令牌容量按 1.2 秒补充量设置，
//! 允许短时突发但不至于一次放行整个 chunk。

use std::sync::Mutex;
use std::time::{Duration, Instant};

struct Bucket {
    rate: f64,
    tokens: f64,
    capacity: f64,
    last: Instant,
    limited: bool,
}

pub struct TokenBucket {
    inner: Mutex<Bucket>,
}

impl TokenBucket {
    pub fn new(bytes_per_sec: u64) -> Self {
        let rate = bytes_per_sec as f64;
        let capacity = if bytes_per_sec == 0 {
            0.0
        } else {
            (rate * 1.2).max(64.0 * 1024.0)
        };

        Self {
            inner: Mutex::new(Bucket {
                rate,
                tokens: capacity,
                capacity,
                last: Instant::now(),
                limited: bytes_per_sec > 0,
            }),
        }
    }

    /// 0 = 不限速。运行中修改会立即影响后续 acquire。
    pub fn set_rate(&self, bytes_per_sec: u64) {
        if let Ok(mut bucket) = self.inner.lock() {
            let rate = bytes_per_sec as f64;
            bucket.rate = rate;
            bucket.limited = bytes_per_sec > 0;
            bucket.capacity = if bytes_per_sec == 0 {
                0.0
            } else {
                (rate * 1.2).max(64.0 * 1024.0)
            };
            bucket.tokens = bucket.capacity;
            bucket.last = Instant::now();
        }
    }

    /// 扣掉 bytes 对应的令牌；不足时 sleep 到足够为止。
    pub async fn acquire(&self, bytes: u64) {
        if bytes == 0 {
            return;
        }

        loop {
            let wait = {
                let Ok(mut bucket) = self.inner.lock() else {
                    return;
                };

                if !bucket.limited {
                    return;
                }

                let now = Instant::now();
                let elapsed = now.duration_since(bucket.last).as_secs_f64();
                bucket.last = now;
                bucket.tokens =
                    (bucket.tokens + elapsed * bucket.rate).min(bucket.capacity);

                let need = bytes as f64;
                if bucket.tokens >= need {
                    bucket.tokens -= need;
                    return;
                }

                (need - bucket.tokens) / bucket.rate
            };

            tokio::time::sleep(Duration::from_secs_f64(wait.max(0.005))).await;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::TokenBucket;
    use std::time::Instant;

    #[tokio::test]
    async fn unlimited_never_waits() {
        let bucket = TokenBucket::new(0);
        let start = Instant::now();
        bucket.acquire(8 * 1024 * 1024).await;
        assert!(start.elapsed().as_millis() < 50);
    }

    #[tokio::test]
    async fn limited_waits_for_tokens() {
        let bucket = TokenBucket::new(64 * 1024);
        let start = Instant::now();
        // 容量 76.8KB，先扣 64KB，再 64KB 需要等下一轮补充
        bucket.acquire(64 * 1024).await;
        bucket.acquire(64 * 1024).await;
        assert!(start.elapsed().as_millis() >= 300);
    }
}
