use serde::Serialize;
use std::collections::HashMap;
use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};
use tokio::sync::Mutex as AsyncMutex;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum ConsoleLevel {
    Error,
    Warning,
    Log,
}

impl ConsoleLevel {
    pub fn from_cdp(kind: &str) -> Self {
        match kind {
            "error" | "assert" => ConsoleLevel::Error,
            "warning" => ConsoleLevel::Warning,
            _ => ConsoleLevel::Log,
        }
    }
}

#[derive(Debug, Clone, Serialize)]
pub struct ConsoleEntry {
    pub level: ConsoleLevel,
    pub text: String,
    pub count: u32,
    pub first_ts: u64,
    pub last_ts: u64,
}

/// Buffer dédupliqué des messages console d'une page: agrège les occurrences
/// répétées (level+text identiques) plutôt que de renvoyer un flux brut —
/// évite de noyer le contexte de l'agent avec le même warning répété 500 fois.
/// Capacité bornée (ring FIFO) pour ne jamais grossir indéfiniment sur une
/// session longue.
#[derive(Debug, Default)]
pub struct ConsoleBuffer {
    entries: HashMap<(String, String), ConsoleEntry>,
    order: Vec<(String, String)>,
    max_distinct: usize,
}

pub type SharedConsoleBuffer = Arc<AsyncMutex<ConsoleBuffer>>;

pub fn new_shared() -> SharedConsoleBuffer {
    Arc::new(AsyncMutex::new(ConsoleBuffer::with_capacity(500)))
}

impl ConsoleBuffer {
    pub fn with_capacity(max_distinct: usize) -> Self {
        Self { entries: HashMap::new(), order: Vec::new(), max_distinct }
    }

    pub fn push(&mut self, kind: &str, text: String) {
        let level = ConsoleLevel::from_cdp(kind);
        let key = (format!("{level:?}"), text.clone());
        let now = now_ms();
        match self.entries.get_mut(&key) {
            Some(e) => {
                e.count += 1;
                e.last_ts = now;
            }
            None => {
                if self.order.len() >= self.max_distinct {
                    let oldest = self.order.remove(0);
                    self.entries.remove(&oldest);
                }
                self.entries.insert(
                    key.clone(),
                    ConsoleEntry { level, text, count: 1, first_ts: now, last_ts: now },
                );
                self.order.push(key);
            }
        }
    }

    pub fn messages(&self, level_filter: Option<ConsoleLevel>, since_ts: Option<u64>) -> Vec<ConsoleEntry> {
        self.order
            .iter()
            .filter_map(|k| self.entries.get(k))
            .filter(|e| level_filter.is_none_or(|lvl| e.level == lvl))
            .filter(|e| since_ts.is_none_or(|since| e.last_ts >= since))
            .cloned()
            .collect()
    }
}

pub fn now_ms() -> u64 {
    SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_millis() as u64).unwrap_or(0)
}
