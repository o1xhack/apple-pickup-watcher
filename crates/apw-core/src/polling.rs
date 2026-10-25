//! 实际响应驱动的恢复节奏。基础间隔由设置持有，这里从不改写设置。
use std::{sync::Arc, time::Duration};
use tokio::time::Instant;

#[derive(Debug, Clone, Default)]
pub struct PollingAdvice {
    /// 全部可用出口都在冷却时的最早截止时间，优先于普通轮询/失败退避。
    pub cooldown_until: Option<Instant>,
    pub interval_floor: Duration,
}

/// Non-secret restart checkpoint reconstructed from persisted network lifecycle events.
#[derive(Debug, Clone, Default)]
pub struct SavedPolling {
    pub locale: String,
    pub route: String,
    pub cooldown_until_ms: Option<u64>,
    pub last_block_ms: Option<u64>,
    pub cooldown_secs: u64,
    pub interval_floor_secs: u64,
}

#[derive(Debug, Default)]
pub(crate) struct Recovery {
    level: u8,
    successes: u32,
    last_block: Option<Instant>,
}
impl Recovery {
    pub fn restore(floor: u64, last_block: Option<Instant>) -> Self {
        Self {
            level: if floor >= 600 {
                2
            } else if floor >= 300 {
                1
            } else {
                0
            },
            successes: 0,
            last_block,
        }
    }
    pub fn blocked(&mut self, now: Instant) {
        self.level = if self
            .last_block
            .is_some_and(|t| now.duration_since(t) < Duration::from_secs(3600))
        {
            2
        } else {
            1
        };
        self.last_block = Some(now);
        self.successes = 0;
    }
    pub fn failed(&mut self) {
        self.successes = 0;
    }
    pub fn success(&mut self) {
        if self.level == 0 {
            return;
        }
        self.successes += 1;
        // 一次成功探测不能证明稳定恢复。每连续 6 次成功再降一档。
        if self.successes >= 6 {
            self.level -= 1;
            self.successes = 0;
        }
    }
    pub fn floor(&self) -> Duration {
        Duration::from_secs(match self.level {
            1 => 300,
            2 => 600,
            _ => 0,
        })
    }
}

/// 落盘字段白名单：没有 URL、Cookie、代理地址或原始错误。
#[derive(Debug, Clone, serde::Serialize)]
pub struct NetworkEvent {
    pub at_ms: u64,
    pub kind: &'static str,
    pub locale: String,
    pub route: String,
    pub request_kind: Option<&'static str>,
    pub status: Option<u16>,
    pub parts: usize,
    pub duration_ms: u64,
    pub outcome: &'static str,
    pub cooldown_until_ms: Option<u64>,
    pub interval_floor_secs: u64,
}
#[derive(Clone)]
pub struct NetworkObserver(pub Arc<dyn Fn(NetworkEvent) + Send + Sync>);
impl std::fmt::Debug for NetworkObserver {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("NetworkObserver")
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn transport_failure_breaks_success_streak() {
        let mut policy = Recovery::default();
        policy.blocked(Instant::now());
        for _ in 0..5 {
            policy.success();
        }
        policy.failed();
        policy.success();
        assert_eq!(policy.floor().as_secs(), 300);
        for _ in 0..5 {
            policy.success();
        }
        assert_eq!(policy.floor(), Duration::ZERO);
    }
    #[test]
    fn repeated_blocks_slow_down_and_only_stable_success_recovers() {
        let now = Instant::now();
        let mut policy = Recovery::default();
        policy.blocked(now);
        assert_eq!(policy.floor().as_secs(), 300);
        policy.blocked(now + Duration::from_secs(500));
        assert_eq!(policy.floor().as_secs(), 600);
        for _ in 0..5 {
            policy.success();
        }
        assert_eq!(policy.floor().as_secs(), 600);
        policy.success();
        assert_eq!(policy.floor().as_secs(), 300);
        for _ in 0..6 {
            policy.success();
        }
        assert_eq!(policy.floor(), Duration::ZERO);
        policy.blocked(now + Duration::from_secs(4200));
        assert_eq!(policy.floor().as_secs(), 300);
    }
}
